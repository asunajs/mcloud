//! 直播口令的**共享池**。
//!
//! # 它解决什么问题
//!
//! 直播口令（弹幕里那串东西）是**全实例共享**的资源：任何账号拿到一条口令，
//! 别的账号、别的 mcloud 用户都能拿它去领小红花。改之前的口径是"每个账号各抓一次"
//! —— N 个账号就是 N 次 WebSocket + N 次小红书主页请求，而口令**是同一批**
//! （上游一场直播只发那么几条）。旧 TS 实现（`mcloud-sign`）已经在**单进程内**做了
//! 这件事（`let sharedKouling: {codes, at}` + `SHARED_KOULING_TTL = 5h`），
//! 这一层把它提升到**实例级、跨用户、跨重启**。
//!
//! # 三条口径（用户 2026-10-01 逐条定的）
//!
//! 1. **按周期划界**：沿用旧 TS 的 **5 小时 TTL** 兜底；另外"开播判定门"关上之后
//!    再等 [`GATE_CLOSE_GRACE_MS`] 就封场（见 [`KoulingPool::normalize`]）。
//! 2. **够 2 条有效就全场停止抓取**（[`KoulingPool::stop_scraping`]）。
//! 3. **过期视同无效**（行为上不可复用），但**原因留着** —— 界面上要能看出
//!    "这条是口令错还是过期了"，那对判断上游什么时候作废口令有用。
//!
//! # ⚠⚠ 一处刻意分开的判据，改动前务必读完
//!
//! 领取结果里 `200118`（**过期**）在本层算 **无效** ✓；但任务层有一条**相反的**规矩：
//! `rejected_kouling()` 对 `200118` 必须返回 `false`，否则会触发一次毫无意义的
//! 60 秒重听（见 `crates/mcloud-tasks/src/live_room.rs` 里 `RedFlowerDrawKind` 的注释）。
//!
//! 这**不是矛盾**，是两个不同的问题：
//!
//! * 池问的是"**这条口令还能不能再拿去用**" ⇒ 过期了就不能 ⇒ 无效 ✓
//! * 重听听的是"**我是不是把口令认错了**" ⇒ 过期说明我认得没错 ⇒ 不重听 ✓
//!
//! 谁要是把这两处合并成一处，就会得到"要么不停地重听、要么拿着过期口令反复投"。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 周期的兜底长度：**5 小时** —— 与旧 TS 实现的 `SHARED_KOULING_TTL` 同值。
///
/// 为什么不改成"自然日"：直播会跨零点，按日切会把一场直播从中间劈开，
/// 于是后半场又要重新抓一遍（正是这一层要消灭的事）。
pub const CYCLE_TTL_MS: i64 = 5 * 60 * 60 * 1000;

/// 判定门关上之后再等多久才封场。
///
/// ⚠ 判定门会因网络抖动"闪"一下（这一趟没拉到房间 ≠ 直播结束了），
/// 立刻封场会把一场直播切碎 ⇒ 留一个宽限窗口。宽限期内池子照旧可用。
pub const GATE_CLOSE_GRACE_MS: i64 = 10 * 60 * 1000;

/// 够多少条**有效**口令就不再抓取。用户口径：2。
pub const VALID_ENOUGH: usize = 2;

/// 历史场次最多留几条（够界面上"最近几场"用，不无限长）。
pub const MAX_SESSIONS: usize = 12;

/// 一次探测里看到的一间直播房 —— **预告 / 在播 / 回放都留**。
///
/// # ⚠ 为什么必须留"门会筛掉"的那些房间
///
/// 判定门只认「正在直播」+ 主播昵称精确相等，所以**预告场与回放场必然被它筛掉**。
/// 而界面上那行「下一场几点」读的正是**预告场的 `expect_start_time`** ——
/// 门筛掉之后不在这里留一份，日历就跟着丢了。
/// Go 参考实现踩过这个坑，注释原话是「**日程不能跟着丢**」（`live_room_probe.go`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeRoom {
    pub room_id: i64,
    /// 房间标题（预告场常常写的就是场次名）。
    pub title: String,
    /// `0` 未开播（预告）/ `1` 直播中 / `2` 已结束（回放）。
    pub status: i64,
    /// 预告开播时间：`2026-09-23 14:30:00` 形态的**本地**字符串。
    ///
    /// ⚠ **不要**解析成时间戳：它没有时区，解析只会凭空造出一个错误的时刻。
    /// 界面**原样显示**它。
    pub expect_start_time: String,
}

/// 观测到的一场直播：门开 ⇒ 开场；门关到过了宽限 ⇒ 散场。
///
/// ⚠ 散场时刻记的是「**最后一次看到它在播**」而不是"发现它关了的那一刻" ✗ ——
/// 后者会把我们没在看的那几个小时也算成直播（门是按分钟抖动的）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSession {
    pub started_at_ms: i64,
    /// `None` = 还在播（或我们还没看到它结束）。
    pub ended_at_ms: Option<i64>,
}

/// 两次**自动抓取**（开一次门 + 听一段弹幕）之间的最小间隔。
///
/// # 为什么是持久化字段而不是进程内标志
///
/// 用户口径（2026-10-01）：「抓取也是一个 mcloud 用户只需要开一个 ws/小红书抓取，
/// 不要每个移动云盘账号都抓取」。最直观的写法是进程内一个 `AtomicBool`，但两条都错：
///
/// * **重启就失效** —— 而池存在的意义恰恰是跨重启；
/// * **并行测试会随机变红** —— 同一进程里两个用例抢同一个标志，
///   谁把抓取"吃掉"取决于调度顺序。
///
/// 所以做成池上的一个时间戳：持久 ✓、跨重启 ✓、测试里（空池）天然放行 ✓。
///
/// 取 90 秒的理由：一次抓取本身要听满 `listenSeconds`（默认 60 秒），
/// 90 秒既能挡住"同一场里几个账号同时扑上去"，又不会让"上次什么都没抓到"
/// 卡住太久（下一分钟的运行就能重试）。
pub const SCRAPE_MIN_INTERVAL_MS: i64 = 90 * 1000;

/// 两次**小红书**主页请求之间的最小间隔。
///
/// 依据：小红书面**不需要鉴权**（本仓与 Go 侧都只带 `Accept`/`Referer`/`UA`），
/// 低频调用是允许的；频繁调用会被风控挡下 —— 而风控挡下的表现是"页面里没有口令"，
/// 与"这一期还没发笔记"在解析层**长得一模一样**，查都没法查。所以宁可慢。
pub const XHS_MIN_INTERVAL_MS: i64 = 30 * 60 * 1000;

/// 池里最多留多少条口令（防呆上限：弹幕里什么都可能被当成口令）。
pub const MAX_CODES: usize = 200;

/// 一条口令在池里的**可复用性**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KoulingState {
    /// 见过、还没验过（抓到了但还没拿去领）。
    Unknown,
    /// 验过、能用。**只有这个状态计入 [`KoulingPool::valid_count`]**。
    Valid,
    /// 不能用（口令错 / 过期）。不再投给任何账号，也不再算作"抓取成果"。
    Invalid,
}

/// 口令从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KoulingSource {
    /// 直播间弹幕（WebSocket）。
    Danmaku,
    /// 小红书主页。
    Xiaohongshu,
    /// 手工粘贴（**验证通过后与抓到的同等待遇**，用户口径 2026-10-01）。
    Manual,
    /// **自动抓取**来的，但没细分是弹幕还是小红书。
    ///
    /// 任务层的收集器把两个来源并发合并在一个结果里（一个源坏了不影响另一个），
    /// 所以它交上来的口令**分不出**谁给的 —— 与其编一个来源，不如如实说"抓来的"。
    Auto,
}

/// 归一化后的领取结论。
///
/// ⚠ 这**不是**协议层那个 `RedFlowerDrawKind`（那个在 `mcloud-upstream`，
/// 本 crate 看不到它，方向也不该反过来）。任务层负责把协议结论映射到这里
/// —— 映射表就是用户拍过的那张，见 [`KoulingPool::note_claim`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaimVerdict {
    /// 领到了。
    Claimed,
    /// 已经领过（上游 `200112`）。**口令是对的**，所以算有效。
    AlreadyClaimed,
    /// 口令不对（上游 `200108`）。
    Invalid,
    /// 口令认识但已失效（上游 `200118`）。按用户口径**视同无效**。
    Expired,
    /// 别的业务码，本层不分类 ⇒ **不改状态**（不能拿它当有效性判据）。
    Other,
}

/// 池里的一条口令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolCode {
    /// 口令原文。
    pub value: String,
    pub state: KoulingState,
    pub source: KoulingSource,
    /// 判定为无效时的原因（人类的说法，直接给界面看）。
    #[serde(default)]
    pub reason: Option<String>,
    /// 第一次见到它的时刻。
    pub first_seen_ms: i64,
    /// 判定为有效的时刻。
    #[serde(default)]
    pub valid_at_ms: Option<i64>,
}

/// 全实例共享的口令池。
///
/// `#[serde(default)]` 与 `settings.rs` 同一个理由：文件是后加的，
/// 老实例第一次读到的是空文件 —— 那时按默认值走，而不是"配置损坏"。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KoulingPool {
    /// 本周期开始的时刻（`0` = 还没有任何周期）。
    pub cycle_at_ms: i64,
    /// 池里的口令。
    pub codes: Vec<PoolCode>,
    /// 账号 → 口令 → 领取结论。
    ///
    /// 为什么要记：同一条口令对**同一个账号**只能领一次，重复投会被上游拒、
    /// 界面上还会显示成一次"失败"。有这张表，挑口令时就能跳过它领过的。
    pub claims: BTreeMap<String, BTreeMap<String, ClaimVerdict>>,
    /// 最近一次**看见开播**的时刻。
    pub last_live_at_ms: Option<i64>,
    /// 最近一次**判定门关上**的时刻（用来算封场宽限）。
    pub gate_closed_at_ms: Option<i64>,
    /// 最近一次请求小红书主页的时刻（限速用）。
    pub last_xhs_at_ms: Option<i64>,
    /// 最近一次**自动抓取**开始的时刻（去重用，见 [`SCRAPE_MIN_INTERVAL_MS`]）。
    pub last_scrape_at_ms: Option<i64>,
    /// 最近一次探测看到的**全部**房间（含预告与回放）——「日程不能跟着丢」。
    pub schedule: Vec<ProbeRoom>,
    /// 上面那份日程是什么时候看到的。
    pub schedule_at_ms: Option<i64>,
    /// 观测到的场次（新的在后）。**不随周期翻篇清空** —— 它是历史。
    pub sessions: Vec<LiveSession>,
}

impl KoulingPool {
    /// 开一个新周期：**口令与领取记录一起清空**。
    ///
    /// ⚠ 领取记录必须跟着清 —— 它记的是"这个账号领过这条口令"，
    /// 而下一场直播的口令是新的；不清会让新口令被当成"已领过"跳过。
    fn start_cycle(&mut self, now_ms: i64) {
        self.cycle_at_ms = now_ms;
        self.codes.clear();
        self.claims.clear();
        self.last_live_at_ms = None;
        self.gate_closed_at_ms = None;
        // 小红书限速**不跟着清**：那是防封的节流，不是这一场的数据。
    }

    /// 周期是不是该翻篇了。
    fn is_stale(&self, now_ms: i64) -> bool {
        if self.cycle_at_ms == 0 {
            // 新池子：不算"过期"，由第一次写入时定周期起点。
            return false;
        }
        if now_ms.saturating_sub(self.cycle_at_ms) > CYCLE_TTL_MS {
            return true;
        }
        // 判定门关上 + 宽限期过了 ⇒ 这一场结束了。
        self.gate_closed_at_ms
            .is_some_and(|closed| now_ms.saturating_sub(closed) > GATE_CLOSE_GRACE_MS)
    }

    /// 读之前先归位：过期就翻篇。**所有对外操作都应该先走它。**
    pub fn normalize(&mut self, now_ms: i64) {
        // ⚠ 这里**不写日志**：本 crate 不依赖 `tracing`，而且分层上也不该由
        // 数据结构来记日志 —— 调用方（任务层）手上才有"翻篇是因为 TTL 还是因为
        // 判定门关了"这个上下文，它来说这句话才有信息量。
        if self.is_stale(now_ms) {
            // 翻篇 = 这一场结束了 ⇒ 把还开着的那一场收尾。
            // ⚠ 散场时刻用 `last_live_at_ms`（**最后一次看到它在播**），不是"现在"。
            self.close_open_session();
            self.start_cycle(now_ms);
        }
        if self.cycle_at_ms == 0 {
            self.cycle_at_ms = now_ms;
        }
    }

    /// 把还开着的那一场收尾（散场时刻 = 最后一次看到它在播）。
    ///
    /// 单独抽出来是因为它有两个触发点：周期翻篇、以及以后可能出现的"手工收场"。
    fn close_open_session(&mut self) {
        if let Some(open) = self.sessions.last_mut() {
            if open.ended_at_ms.is_none() {
                open.ended_at_ms = self.last_live_at_ms.or(Some(self.cycle_at_ms));
            }
        }
    }

    /// 下一场的开播时间（预告场的 `expect_start_time`，**原样**）。
    ///
    /// 判据与判定门**无关**：门只认 `status == 1`，这里要的恰恰是 `status == 0`。
    /// 没有预告场 ⇒ `None`（不是空串 —— 界面要能区分"没有下一场"与"上游没给时间"）。
    pub fn next_start(&self) -> Option<&str> {
        self.schedule
            .iter()
            .find(|r| r.status == 0 && !r.expect_start_time.is_empty())
            .map(|r| r.expect_start_time.as_str())
    }

    /// 记一次探测看到的东西：**全部房间**（含预告/回放）+ 门开没开。
    ///
    /// 场次也从这里推：门开 ⇒ 开场（已经有一场开着就不重复开）。
    /// 收场不在这里 —— 它要等"门关超过宽限"，也就是周期翻篇那一刻（见 [`Self::normalize`]）。
    pub fn note_probe(&mut self, rooms: &[ProbeRoom], active: bool, now_ms: i64) {
        self.schedule = rooms.to_vec();
        self.schedule_at_ms = Some(now_ms);
        // ⚠ **门的状态也从这里记**（内部调 [`Self::note_gate`]），不要让它和
        // `note_gate` 成为两个必须同步的入口：`last_live_at_ms` 就靠 `note_gate` 维护，
        // 而"散场时刻"要用它 ⇒ 少调一次就会把散场时刻算错（丢的那几小时）。
        self.note_gate(active, now_ms);
        if active {
            let open = matches!(self.sessions.last(), Some(s) if s.ended_at_ms.is_none());
            if !open {
                self.sessions.push(LiveSession {
                    started_at_ms: now_ms,
                    ended_at_ms: None,
                });
                while self.sessions.len() > MAX_SESSIONS {
                    self.sessions.remove(0);
                }
            }
        }
    }

    /// 记一次判定门的观察结果。
    ///
    /// 看见开播 ⇒ 清掉"门关着"的计时（宽限期重新开始算）；
    /// 没看见开播 ⇒ 开始计时（宽限期后才封场，抖动切不碎一场直播）。
    pub fn note_gate(&mut self, live: bool, now_ms: i64) {
        if live {
            self.last_live_at_ms = Some(now_ms);
            self.gate_closed_at_ms = None;
        } else if self.gate_closed_at_ms.is_none() {
            self.gate_closed_at_ms = Some(now_ms);
        }
    }

    /// 收一批口令进池。返回**新增**条数。
    ///
    /// 两条规矩：
    /// * 已知**无效**的直接丢 —— 抓取端会反复听到同一条死口令（弹幕里有人复读），
    ///   让它进池就等于让"有效计数"永远凑不够 ⇒ 一直抓。
    /// * 已存在的**不改来源也不刷新时间**：口令的有效期由上游决定，不由我们见到几次决定。
    pub fn note(&mut self, values: &[String], source: KoulingSource, now_ms: i64) -> usize {
        self.normalize(now_ms);
        let mut added = 0;
        for raw in values {
            let value = raw.trim();
            if value.is_empty() {
                continue;
            }
            if let Some(existing) = self.codes.iter().find(|c| c.value == value) {
                if existing.state == KoulingState::Invalid {
                    // 见上：死口令不许再进池，否则会把"够 2 条"无限推迟。
                    continue;
                }
                continue;
            }
            if self.codes.len() >= MAX_CODES {
                // 同上：满了这件事由调用方去说（它才知道是哪一次抓取撞上了上限）。
                break;
            }
            self.codes.push(PoolCode {
                value: value.to_string(),
                state: KoulingState::Unknown,
                source,
                reason: None,
                first_seen_ms: now_ms,
                valid_at_ms: None,
            });
            added += 1;
        }
        added
    }

    /// **状态跃迁的唯一入口**：记一条口令在某个账号上的领取结论。
    ///
    /// # 映射表（用户 2026-10-01 拍的口径）
    ///
    /// | 结论 | 口令状态 |
    /// |---|---|
    /// | [`ClaimVerdict::Claimed`] | 有效 |
    /// | [`ClaimVerdict::AlreadyClaimed`] | 有效（"口令本身是对的"） |
    /// | [`ClaimVerdict::Invalid`] | 无效 |
    /// | [`ClaimVerdict::Expired`] | 无效（**行为上**；原因留成"已过期"） |
    /// | [`ClaimVerdict::Other`] | **不变**（本层没分类，不能当判据） |
    ///
    /// ⚠ 「有效」**只升不降**：一条口令被验成有效之后，别的账号再来领只会得到
    /// "已领过"，那不该把它退回未知 —— 否则"够 2 条"会反复丢失。
    pub fn note_claim(&mut self, account: &str, code: &str, verdict: ClaimVerdict, now_ms: i64) {
        self.normalize(now_ms);
        let code = code.trim();
        if code.is_empty() {
            return;
        }
        // 先记账号级结果（无论池里有没有这条：手工口令也走这里进池）。
        self.claims
            .entry(account.to_string())
            .or_default()
            .insert(code.to_string(), verdict);

        let (state, reason): (Option<KoulingState>, Option<&str>) = match verdict {
            ClaimVerdict::Claimed => (Some(KoulingState::Valid), Some("已验证有效")),
            ClaimVerdict::AlreadyClaimed => (
                Some(KoulingState::Valid),
                Some("已验证有效（这个账号已领过）"),
            ),
            ClaimVerdict::Invalid => (Some(KoulingState::Invalid), Some("口令无效")),
            ClaimVerdict::Expired => (Some(KoulingState::Invalid), Some("口令已过期")),
            ClaimVerdict::Other => (None, None),
        };

        match self.codes.iter_mut().find(|c| c.value == code) {
            Some(slot) => {
                if let Some(state) = state {
                    // 「有效」不被后到的结论推翻（见上面那条 ⚠）。
                    if !(slot.state == KoulingState::Valid && state == KoulingState::Invalid) {
                        slot.state = state;
                    }
                    slot.reason = reason.map(str::to_string);
                    if state == KoulingState::Valid && slot.valid_at_ms.is_none() {
                        slot.valid_at_ms = Some(now_ms);
                    }
                }
            }
            None => {
                // 池里没有它的两种情况：① 手工粘贴的口令（用户口径：**验证后也进共享**）；
                // ② 抓取端因为别的原因没入池。两种都按同一条规矩收进来。
                if let Some(state) = state {
                    // 走到这里只有一种可能：这条口令不是"抓进池"的
                    // （抓进来的必在池里），而是**手工粘贴**后被验证的
                    // —— 用户口径：手工口令验证后也进共享。
                    let source = KoulingSource::Manual;
                    if self.codes.len() < MAX_CODES {
                        self.codes.push(PoolCode {
                            value: code.to_string(),
                            state,
                            source,
                            reason: reason.map(str::to_string),
                            first_seen_ms: now_ms,
                            valid_at_ms: (state == KoulingState::Valid).then_some(now_ms),
                        });
                    }
                }
            }
        }
    }

    /// **有效**口令条数。停止抓取的唯一判据（未知/无效都不算）。
    pub fn valid_count(&self) -> usize {
        self.codes
            .iter()
            .filter(|c| c.state == KoulingState::Valid)
            .count()
    }

    /// 够不够了 —— 够了就全场停止抓取。
    pub fn stop_scraping(&self) -> bool {
        self.valid_count() >= VALID_ENOUGH
    }

    /// 所有**已验证有效**的口令原文。
    ///
    /// 用途：任务层算"够不够两条"时拿它当**起点** —— 见 `live_room.rs` 里
    /// 那个继续听的循环（判据**不能**以"池写得进盘"为前提）。
    pub fn valid_values(&self) -> Vec<String> {
        self.codes
            .iter()
            .filter(|c| c.state == KoulingState::Valid)
            .map(|c| c.value.clone())
            .collect()
    }

    /// 所有**不可复用**的口令原文（给抓取端当"别拿这些来凑数"的过滤器）。
    pub fn invalid_values(&self) -> Vec<String> {
        self.codes
            .iter()
            .filter(|c| c.state == KoulingState::Invalid)
            .map(|c| c.value.clone())
            .collect()
    }

    /// 该投给这个账号的口令：**它还没领过的**、且不是无效的。
    ///
    /// ⚠ 未知的也要投：池子里的"未知"正是"抓到了但还没验过"，
    /// 而**验证的唯一途径就是拿去领** —— 用户口径："在听的同时能够去领取，
    /// 直接验证是否有效"。
    pub fn pick_for(&self, account: &str) -> Vec<String> {
        let own = self.claims.get(account);
        self.codes
            .iter()
            .filter(|c| c.state != KoulingState::Invalid)
            .filter(|c| own.is_none_or(|m| !m.contains_key(&c.value)))
            .map(|c| c.value.clone())
            .collect()
    }

    /// 这个账号是不是已经把这条口令领过了（用来给界面解释"为什么这次没投它"）。
    pub fn account_claimed(&self, account: &str, code: &str) -> bool {
        self.claims
            .get(account)
            .is_some_and(|m| m.contains_key(code))
    }

    /// 现在该不该由**这一趟**去抓口令。
    ///
    /// ⚠ 空池（没抓过）⇒ 放行 —— 这正是"每个周期第一次谁先到谁抓"。
    pub fn scrape_ready(&self, now_ms: i64) -> bool {
        self.last_scrape_at_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= SCRAPE_MIN_INTERVAL_MS)
    }

    /// 记一次"我要开抓了"。
    ///
    /// ⚠ 在**动手之前**记，不是抓完之后：抓取要听满 60 秒，抓完再记等于
    /// 整个窗口都敞着，别的账号照样扑上来。
    pub fn note_scrape(&mut self, now_ms: i64) {
        self.last_scrape_at_ms = Some(now_ms);
    }

    /// 现在能不能请求小红书主页（限速见 [`XHS_MIN_INTERVAL_MS`]）。
    pub fn xhs_ready(&self, now_ms: i64) -> bool {
        self.last_xhs_at_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= XHS_MIN_INTERVAL_MS)
    }

    /// 记一次小红书主页请求（**成功失败都记**：被风控挡下也算一次请求）。
    pub fn note_xhs(&mut self, now_ms: i64) {
        self.last_xhs_at_ms = Some(now_ms);
    }

    /// 给界面看的快照。
    pub fn summary(&self) -> PoolSummary {
        PoolSummary {
            cycle_at_ms: self.cycle_at_ms,
            valid_count: self.valid_count(),
            stop_scraping: self.stop_scraping(),
            codes: self.codes.clone(),
            next_start: self.next_start().map(str::to_string),
            schedule_at_ms: self.schedule_at_ms,
            sessions: {
                // 界面要"最近几场"，所以**新的在前**（存的时候是新的在后）。
                let mut v = self.sessions.clone();
                v.reverse();
                v
            },
        }
    }
}

/// 给界面/接口的池状态快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolSummary {
    pub cycle_at_ms: i64,
    pub valid_count: usize,
    /// 够 2 条了 ⇒ 界面可以正大光明地写"不需要再抓口令了"。
    pub stop_scraping: bool,
    pub codes: Vec<PoolCode>,
    /// 下一场的开播时间（预告场给的**本地字符串**，原样）。
    ///
    /// ⚠ 它来自"最近一次探测"，所以**没跑过任务时是 `None`** ——
    /// 界面必须能说清"还没探测过"，而不是显示一个空白。
    pub next_start: Option<String>,
    /// 下一场是什么时候探测到的（用来判断上面那个时间还新不新）。
    pub schedule_at_ms: Option<i64>,
    /// 观测到的场次（新的在前，界面上"最近几场"）。
    pub sessions: Vec<LiveSession>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_790_000_000_000;
    const ACC_A: &str = "13800000000";
    const ACC_B: &str = "13900000001";

    fn pool() -> KoulingPool {
        let mut p = KoulingPool::default();
        p.normalize(T0);
        p
    }

    fn codes(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn collected_codes_enter_as_unknown_and_are_deduped() {
        let mut p = pool();
        assert_eq!(p.note(&codes(&["甲", "乙"]), KoulingSource::Danmaku, T0), 2);
        // 同一条再来一次：不加、也不重置时间戳
        assert_eq!(
            p.note(&codes(&["甲"]), KoulingSource::Xiaohongshu, T0 + 1000),
            0
        );
        assert_eq!(p.codes.len(), 2);
        assert!(p.codes.iter().all(|c| c.state == KoulingState::Unknown));
        assert_eq!(p.valid_count(), 0, "没验过的不能算有效");
        assert!(!p.stop_scraping());
    }

    #[test]
    fn a_known_dead_code_must_not_re_enter_the_pool() {
        // ⭐ 这条是"边听边领"能停下来的前提：弹幕里会有人复读死口令，
        //    让它再进池就等于"抓到的永远是新口令" ⇒ 有效计数永远凑不够 ⇒ 一直抓。
        let mut p = pool();
        p.note(&codes(&["死的"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "死的", ClaimVerdict::Invalid, T0 + 1);
        assert_eq!(p.note(&codes(&["死的"]), KoulingSource::Danmaku, T0 + 2), 0);
        assert_eq!(p.valid_count(), 0);
    }

    #[test]
    fn claimed_and_already_claimed_both_count_as_valid() {
        // 用户口径：已经领过（非过期）也算有效 —— 口令本身是对的。
        let mut p = pool();
        p.note(&codes(&["甲", "乙"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "甲", ClaimVerdict::Claimed, T0 + 1);
        p.note_claim(ACC_B, "乙", ClaimVerdict::AlreadyClaimed, T0 + 2);
        assert_eq!(p.valid_count(), 2);
        assert!(p.stop_scraping(), "两条有效 ⇒ 停止抓取");
    }

    #[test]
    fn expired_counts_as_invalid_but_keeps_its_reason() {
        let mut p = pool();
        p.note(&codes(&["过期的"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "过期的", ClaimVerdict::Expired, T0 + 1);
        let c = &p.codes[0];
        assert_eq!(c.state, KoulingState::Invalid, "过期视同无效（用户口径）");
        assert_eq!(c.reason.as_deref(), Some("口令已过期"), "但原因要留着");
    }

    #[test]
    fn other_verdicts_do_not_change_the_state() {
        let mut p = pool();
        p.note(&codes(&["说不清的"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "说不清的", ClaimVerdict::Other, T0 + 1);
        assert_eq!(
            p.codes[0].state,
            KoulingState::Unknown,
            "没分类的码不能当判据"
        );
    }

    #[test]
    fn valid_never_downgrades_to_invalid() {
        // 一条口令验成有效之后，别的账号再来只可能得到"已领过" ——
        // 万一上游回了个别的码，也不能把它退回无效（否则"够 2 条"会来回丢）。
        let mut p = pool();
        p.note(&codes(&["甲"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "甲", ClaimVerdict::Claimed, T0 + 1);
        p.note_claim(ACC_B, "甲", ClaimVerdict::Invalid, T0 + 2);
        assert_eq!(p.codes[0].state, KoulingState::Valid);
        assert_eq!(p.valid_count(), 1);
    }

    #[test]
    fn manual_codes_join_the_pool_once_verified() {
        // 用户口径：手工输入的口令在**验证后**也进共享。
        let mut p = pool();
        p.note_claim(ACC_A, "手输的", ClaimVerdict::Claimed, T0);
        assert_eq!(p.codes.len(), 1);
        assert_eq!(p.codes[0].source, KoulingSource::Manual);
        assert_eq!(p.valid_count(), 1);
        // 别的账号就能直接用了
        assert_eq!(p.pick_for(ACC_B), vec!["手输的".to_string()]);
    }

    #[test]
    fn pick_for_skips_what_this_account_already_claimed_and_all_invalid() {
        let mut p = pool();
        p.note(&codes(&["甲", "乙", "丙"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "甲", ClaimVerdict::Claimed, T0 + 1); // A 领过
        p.note_claim(ACC_A, "丙", ClaimVerdict::Invalid, T0 + 2); // 死口令
        assert_eq!(p.pick_for(ACC_A), vec!["乙".to_string()]);
        assert_eq!(
            p.pick_for(ACC_B),
            vec!["甲".to_string(), "乙".to_string()],
            "别的账号：有效的甲 + 未知的乙，死掉的丙不给"
        );
        assert!(p.account_claimed(ACC_A, "甲"));
        assert!(!p.account_claimed(ACC_B, "甲"));
    }

    #[test]
    fn ttl_of_five_hours_starts_a_new_cycle() {
        let mut p = pool();
        p.note(&codes(&["甲"]), KoulingSource::Danmaku, T0);
        p.note_claim(ACC_A, "甲", ClaimVerdict::Claimed, T0 + 1);
        p.normalize(T0 + CYCLE_TTL_MS - 1);
        assert_eq!(p.codes.len(), 1, "没到 5 小时不该翻篇");
        p.normalize(T0 + CYCLE_TTL_MS + 1);
        assert!(p.codes.is_empty(), "翻篇要把上一场的口令清掉");
        assert!(p.claims.is_empty(), "领取记录也得清（新口令是新的）");
        assert_eq!(p.valid_count(), 0);
    }

    #[test]
    fn a_closed_gate_seals_the_cycle_but_not_before_the_grace_window() {
        let mut p = pool();
        p.note_gate(true, T0);
        p.note(&codes(&["甲"]), KoulingSource::Danmaku, T0);
        // 门关了：宽限期内不算封场（判定门会因网络抖动闪一下）
        p.note_gate(false, T0 + 1000);
        p.normalize(T0 + 1000 + GATE_CLOSE_GRACE_MS - 1);
        assert_eq!(p.codes.len(), 1, "宽限期内池子照旧可用");
        p.normalize(T0 + 1000 + GATE_CLOSE_GRACE_MS + 1);
        assert!(p.codes.is_empty(), "宽限期过了 ⇒ 这一场结束");
    }

    #[test]
    fn seeing_the_gate_open_again_cancels_the_sealing_timer() {
        let mut p = pool();
        p.note_gate(false, T0);
        p.note_gate(true, T0 + 1000);
        p.normalize(T0 + 1000 + GATE_CLOSE_GRACE_MS + 1);
        // 门又开了（抖动），计时已清 ⇒ 不封场
        p.note(&codes(&["甲"]), KoulingSource::Danmaku, T0 + 2000);
        p.normalize(T0 + 2000 + 1000);
        assert_eq!(p.codes.len(), 1);
    }

    /// ⭐ 门筛掉的（预告/回放）必须在日程里留着 —— "日程不能跟着丢"。
    #[test]
    fn schedule_keeps_the_rooms_the_gate_throws_away() {
        let mut p = pool();
        // 门的判据只认 `status == 1`，所以下面两间它**必然**筛掉。
        let rooms = vec![
            ProbeRoom {
                room_id: 147608,
                title: "下一场".into(),
                status: 0,
                expect_start_time: "2026-09-23 14:30:00".into(),
            },
            ProbeRoom {
                room_id: 147607,
                title: "上一场回放".into(),
                status: 2,
                expect_start_time: "2026-09-22 14:30:00".into(),
            },
        ];
        p.note_probe(&rooms, false, T0);
        assert_eq!(p.schedule.len(), 2, "两间都要留着（哪怕门一间都不认）");
        assert_eq!(
            p.next_start(),
            Some("2026-09-23 14:30:00"),
            "下一场读的正是预告场的 expectStartTime"
        );
        assert_eq!(
            p.summary().next_start.as_deref(),
            Some("2026-09-23 14:30:00")
        );
        assert_eq!(
            p.summary().schedule_at_ms,
            Some(T0),
            "要能看出这个日程有多新"
        );
    }

    /// ⭐ 没有预告场 ⇒ `None`（不是空串：界面要能区分"没有下一场"与"上游没给时间"）。
    #[test]
    fn next_start_is_none_without_a_preview_room() {
        let mut p = pool();
        p.note_probe(
            &[ProbeRoom {
                room_id: 1,
                title: "正在播".into(),
                status: 1,
                expect_start_time: String::new(),
            }],
            true,
            T0,
        );
        assert_eq!(p.next_start(), None);
    }

    /// ⭐ 场次：门开 ⇒ 开场；门关到过了宽限（翻篇）⇒ 散场，且散场时刻是
    /// **最后一次看到它在播**（不是"发现它关了"的那一刻）。
    #[test]
    fn sessions_record_start_and_end_from_observations() {
        let mut p = pool();
        p.note_probe(&[], true, T0);
        assert_eq!(p.sessions.len(), 1, "看见开播 ⇒ 开一场");
        assert_eq!(p.sessions[0].started_at_ms, T0);
        assert_eq!(p.sessions[0].ended_at_ms, None, "还在播");

        // 再看一次还是"在播" ⇒ **不能**再开一场（同场直播会被每分钟的探测切成很多场）。
        p.note_probe(&[], true, T0 + 60_000);
        assert_eq!(p.sessions.len(), 1, "同一场只能有一条");

        // 门关了，但**宽限期内**不散场（抖动切不碎一场直播）。
        p.note_gate(false, T0 + 120_000);
        p.normalize(T0 + 120_000);
        assert_eq!(p.sessions[0].ended_at_ms, None, "宽限期内不散场");

        // 过了 5 小时 TTL + 宽限 ⇒ 翻篇 ⇒ 散场，且散场时刻 = 最后一次看到在播。
        let late = T0 + CYCLE_TTL_MS + GATE_CLOSE_GRACE_MS + 1000;
        p.normalize(late);
        assert_eq!(
            p.sessions[0].ended_at_ms,
            Some(T0 + 60_000),
            "散场时刻必须是**最后一次看到它在播**，不是晚几小时的'发现它关了'"
        );
        assert_eq!(p.summary().sessions.len(), 1, "历史要留着（翻篇不清历史）");
        assert_eq!(p.codes.len(), 0, "口令随翻篇清掉");
    }

    #[test]
    fn only_one_scrape_at_a_time_per_datacenter() {
        // 用户口径：一个 mcloud 用户只需要开一个抓取，别每个账号都抓一遍。
        let mut p = pool();
        assert!(p.scrape_ready(T0), "空池 ⇒ 谁先到谁抓");
        p.note_scrape(T0);
        assert!(!p.scrape_ready(T0 + 1000), "刚开抓，别的账号先别扑上来");
        assert!(p.scrape_ready(T0 + SCRAPE_MIN_INTERVAL_MS));
        // 跨重启也成立：这是**持久化**字段，不是进程内标志
        let reloaded: KoulingPool =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(
            !reloaded.scrape_ready(T0 + 2000),
            "重新读盘之后仍在去重窗口内"
        );
    }

    #[test]
    fn xiaohongshu_is_rate_limited() {
        let mut p = pool();
        assert!(p.xhs_ready(T0), "从没请求过 ⇒ 可以请求");
        p.note_xhs(T0);
        assert!(!p.xhs_ready(T0 + 1000));
        assert!(p.xhs_ready(T0 + XHS_MIN_INTERVAL_MS));
        // 限速**不随周期清空**（那是防封的节流，不是这一场的数据）。
        //
        // ⚠ 这里**不能**用 5 小时 TTL 来验：那时候限速窗口（30 分钟）早就过了，
        // 无论清没清都是"可以请求"，这条断言就成了恒真。改用"门关 + 10 分钟宽限"
        // 让它 ~11 分钟就翻篇 —— 此时仍在 30 分钟窗口内，才验得出来。
        p.note_gate(true, T0);
        p.note_gate(false, T0 + 1000);
        p.normalize(T0 + 1000 + GATE_CLOSE_GRACE_MS + 1);
        assert!(p.codes.is_empty(), "先确认：确实翻篇了");
        assert!(
            !p.xhs_ready(T0 + 1000 + GATE_CLOSE_GRACE_MS + 2),
            "翻篇不该把限速一起清掉"
        );
    }

    #[test]
    fn pool_is_capped() {
        let mut p = pool();
        let many: Vec<String> = (0..MAX_CODES + 50).map(|i| format!("口令{i}")).collect();
        p.note(&many, KoulingSource::Danmaku, T0);
        assert_eq!(p.codes.len(), MAX_CODES);
    }
}
