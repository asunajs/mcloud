//! 直播口令 → 小红花：**编排层**（`live-room` 任务）。
//!
//! 这一条链把两件事串起来：**获取口令**（听弹幕 + 人工喂）与**领取小红花**
//! （六步链）。请求构造与响应判定全在 [`mcloud_upstream`] 的纯函数层
//! （`live_flower` / `live_room` / `live_kouling` / `live_ws`），本模块只做
//! **编排**：串步骤、判分支、把结论说成一句人话。
//!
//! # 为什么判据住在这一层（而不是散在流程里）
//!
//! 这条链一年里只有主播开播的那几小时才能真跑一次，所以"哪一步该发什么、
//! 拿到什么响应该往哪走"必须能在单测里全部走完。三处最容易写错的地方被提成了
//! 纯函数：[`JudgeKoulingGate`]（门）、[`RetryPlan`] / [`PickNewCodes`]（重试）、
//! [`LiveRoomParams::check`]（参数）。判定只有一份，两个产物（CLI / 服务端）
//! 各自只负责"把用户的选择翻成参数、把结果翻成人话"。
//!
//! # 四条产品事实（不是提示文案，是判据）
//!
//! 1. **`codes` 不是可选项**：小红书那条口令没法自动化（那边无法鉴权），
//!    必须由人工喂进来 —— 这就是为什么"听弹幕"永远拿不全两条。
//! 2. **任何"历史口令"都是废值**：口令只在**本次直播**有效，上一场的不能用。
//!    ⇒ 不保存口令、不跨场复用、**不拿已结束场次的弹幕当口令来源**。
//! 3. 每场 **2~3 条**是正常形状：别把"只收到 1 条"当成已收全，
//!    也别把"收到 3 条"当脏数据。
//! 4. 弹幕里的口令**署名是谁都能用**：**不要**按"发信人是不是我"过滤。
//!
//! # ⚠ 本仓铁律：禁止全链吞错
//!
//! 旧实现每个失败分支只打一句日志、**从不抛错** ⇒ 对 runner 永远"成功"，
//! 于是定时任务能在"天天报错"的情况下绿灯一个月。所以本模块**失败一律变成
//! `Err` 或 [`TaskOutcome::failed`]**。见 [`LiveRoomRun::run_account`] 的错误传播。
//!
//! # 分层证据等级（实话实说）
//!
//! ⚠ **门那一段与 WS 那一段的证据等级不同，别一刀切**：
//!
//! | 部分 | 依据 |
//! |---|---|
//! | 四态结论、三种"没领"、进账 = ⑥−① | **照抄参考实现**（有实拍值支撑，见下） |
//! | `200108` / `200112` / `200118` / `200110` 四分类 | 协议层已钉（**实拍/向量**） |
//! | 埋点发 5 条 | 照抄参考实现，**实拍只证实其中 1 条**（见 [`JOURNALING_EVENTS`]） |
//! | 只听第一间房间 | **刻意的取舍**，代价见 [`LiveRoomRun::run_account`] |
//! | 有 codes ⇒ 跳过开门 | **刻意的取舍**，理由见 [`LiveRoomRun::run_account`] |
//! | 票用云盘侧配方 | **实拍**（拿直播侧 JWT 回 `90001`，见下） |
//! | 🌸 **门的三步**（[`LiveRoomRun::open_gate`]） | **✅ 实证** —— 见下 |
//! | ⏸ **WS / 弹幕 / 口令**（[`LiveKoulingCollector`]） | **结构性抓不到** —— 见下 |
//!
//! ## ✅ 门的三步：**实证**
//!
//! 出处 `../mcloud/docs/capture/app-live-redflower-samples.txt`
//! 第 345–365 行（§5.3）与第 466–475 行的证据等级表：
//!
//! ```text
//! ① POST user-njs.yun.139.com/user/querySpecTokenV2  {"toSourceId":"017027"}
//!                                                       ⇒ SSO token 42 字符
//! ② POST v.cmcc-cs.cn/cmlive/api/user/auth/cmpassLoginByCmToken
//!        body {"cmpassToken":"<SSO>"}  ⇒ {"rtnMsg":null,"rtnCode":"0","object":"<JWT 180 字符>"}
//! ③ GET  …/api/channel/channel/tabContentList?tabId=<id>&contentType=0&pageNo=1&channelId=134
//! ```
//!
//! 那一轮用**临时只读探针主动走了一遍** —— 因为门第③步是 **GET、不需要开播**，
//! §5.2 那个"结构性抓不到"只解释了 WS 那半段，抓包没抓到**不等于**它不能打。
//!
//! 两处实证细节：`querySpecTokenV2(017027)` **能**换到 SSO
//! ⇒ 库里那份 `authorization` 在直播族上也认；
//! `cmpassLoginByCmToken` 的 `object` **就是 JWT 本身**（180 字符），不是包着它的对象。
//! 全程 `rtnCode` 都是**字符串 `"0"`** ✅ 再证第三套响应约定。
//!
//! ## ⏸ WS / 弹幕 / 口令：**结构性抓不到**
//!
//! 未开播（`chatRoomId: null`），所以
//! `userToken` / `member/apply` / SockJS 帧**一个样本都没有**。
//! ⇒ 这是**结构性的**，不是"没抓到、下次努力"。
//! [`LiveKoulingCollector::listen_danmaku_with`] 那一支是"接线完成、真机未验证"。
//!
//! ⚠ **两者分开标**：门那条路是实证的，WS 那条不是。混成一句
//! "直播这条链没实拍"会让人以为门也不能用（而它刚刚被走通）。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result, TaskOutcome};
use mcloud_engine::{ParamSpec, Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::live_flower as proto;
use mcloud_upstream::live_flower::{
    FirstParticipateKind, RedFlowerBalance, RedFlowerDrawKind, RedFlowerDrawOutcome,
    RedFlowerEnvelope,
};
use mcloud_upstream::live_room as live;
use mcloud_upstream::live_ws_client::{self, ListenRequest, LiveWsEndpoint};

// ══════════════════════════ 参数 ══════════════════════════

/// 把上游给的房间变成池要留的**日程**（含门会筛掉的预告/回放）。
///
/// ⚠ 这里是"日程不能跟着丢"的落点：判定门只留"正在直播"，
/// 而"下一场几点"读的恰恰是 `status == 0` 那张预告场的 `expectStartTime`。
///
/// ⚠⚠ 主播昵称**必须一起带走**（用户口径 2026-10-02）：
/// 上游那份列表里**别的主播也在**（判定门注释里的实拍 147490），
/// 不带上昵称，池子就分不出"哪一场是那个主播的"，界面上那行「下一场」
/// 会指向**别人的场次** —— 用户看到 10-02 那场直接反问
/// 「你这 10 月 2 号不是目标直播间吧」。
fn schedule_rooms(rooms: &[mcloud_upstream::live_room::LiveRoom]) -> Vec<mcloud_core::ProbeRoom> {
    rooms
        .iter()
        .map(|r| mcloud_core::ProbeRoom {
            room_id: r.id,
            title: r.title.clone(),
            status: r.status,
            expect_start_time: r.expect_start_time.clone(),
            anchor_nickname: r.anchor_nickname.clone(),
        })
        .collect()
}

/// 把一份领取报告里"验成有效"的口令并进集合（本地计"够不够两条"用）。
///
/// # 为什么判据要**本地算**，而不是回读池
///
/// 池写失败是**只警告不失败**的（那是刻意的：不能拿优化否定目的）。若判据靠
/// "回读池"，写不进盘时就永远读到 0 ⇒ 继续听的循环会一直转到硬上限为止 ✗。
/// 这个 bug 是被测试逼出来的：测试的替身存储不支持下写池，循环因此停不下来。
fn note_valid_codes(set: &mut std::collections::HashSet<String>, report: &RedFlowerReport) {
    for d in &report.draws {
        if matches!(
            d.outcome.kind,
            RedFlowerDrawKind::Succeeded | RedFlowerDrawKind::AlreadyDrawn
        ) {
            set.insert(d.code.clone());
        }
    }
}

/// 把这次运行的领取结论**写回共享口令池**。
///
/// # 为什么池写不进去**只警告、不失败**
///
/// 本任务的价值是"把花领到手"；池是"让别的账号少抓一次"的**优化**。
/// 因为池写失败就把已经领到的花判成失败，是拿优化去否定目的 ——
/// 而且那种失败最难查：日志里只有一句存储错误，用户看到的是"任务失败"，
/// 而他明明领到了。
///
/// # ⚠ 已知的并发窗口（刻意留着，不引锁）
///
/// 池是"读 → 改 → 存"，两个账号同时跑时，后写的会把先写的那条**备注**盖掉。
/// 后果是"那条死口令下一次还会被投一次"（再得一次 `200108`），
/// **不是**文件损坏（存储的写入本身是原子的）。
/// 引一把进程内锁能消掉它，但代价是把"池"这个概念和进程生命周期绑在一起 ——
/// 而池的意义恰恰是**跨进程重启**。所以这里选择如实标注，而不是假装没有。
async fn settle_pool(
    ctx: &TaskContext,
    account_id: &str,
    out: &LiveRoomAccountResult,
) -> Result<()> {
    let now = ctx.clock.now_ms();
    let mut pool = ctx.store.kouling_pool().await?;
    pool.normalize(now);
    // 这次实际拿到并投出去的口令先进池（未知态）—— 包括重听那一轮拿到的。
    pool.note(&out.codes, mcloud_core::KoulingSource::Auto, now);
    if let Some(report) = &out.report {
        for d in &report.draws {
            // ⚠ **状态跃迁的唯一映射点**（用户 2026-10-01 拍的那张表）：
            //
            // 成功 / 已兑换 ⇒ 有效；口令错 ⇒ 无效；**过期视同无效**（但原因留着）；
            // 别的码 ⇒ 不动（本层没分类，不能当有效性判据）。
            //
            // ⚠⚠ 注意这里与"要不要重听"是**两个问题**：那边 `rejected_kouling()`
            // 对过期必须返回 `false`（否则会白听 60 秒）。别把两处合并。
            let verdict = match d.outcome.kind {
                RedFlowerDrawKind::Succeeded => mcloud_core::ClaimVerdict::Claimed,
                RedFlowerDrawKind::AlreadyDrawn => mcloud_core::ClaimVerdict::AlreadyClaimed,
                RedFlowerDrawKind::InvalidKouling => mcloud_core::ClaimVerdict::Invalid,
                RedFlowerDrawKind::KoulingExpired => mcloud_core::ClaimVerdict::Expired,
                _ => mcloud_core::ClaimVerdict::Other,
            };
            pool.note_claim(account_id, &d.code, verdict, now);
        }
    }
    if let Err(e) = ctx.store.save_kouling_pool(&pool).await {
        ctx.warn(format!(
            "口令池写回失败（不影响这次领取，只是别的账号这次用不上共享结果）：{e}"
        ));
    }
    Ok(())
}

/// "继续听"最多再听几轮（防死循环）。
///
/// # 为什么必须有这个上限
///
/// 弹幕是**不可信输入**：任何一行都可能是候选口令（观众发言也算），
/// 所以"还不足两条有效就继续听"在理论上可以永远听下去。
/// 上限只兜底，不参与判据 —— 正常情况下第一、二轮就够了。
pub const MAX_LISTEN_ROUNDS: usize = 5;

/// 一趟运行里**由使用者决定**的那几件事。
///
/// ⚠ 构造出来不等于"这组参数能跑"：组合是否成立由 [`LiveRoomParams::check`] 判
/// （判据只有一份），而**报不报错、怎么说**留在各调用方自己的边界上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveRoomParams {
    /// 手工口令。**非空就不再收集**（这一趟没有口令要听弹幕）。
    ///
    /// ⚠ 它不是"锦上添花"：小红书那条口令**只能**从这里进来（见模块头事实 1）。
    pub codes: Vec<String>,
    /// 自动收集时听弹幕的时长（秒）。
    pub listen_seconds: u64,
    /// "只用给定口令"：不开弹幕。它为真时 `codes` 必须非空。
    pub skip_kouling: bool,
}

/// 参数组合上的冲突**种类** —— 只有种类，没有措辞。
///
/// # 为什么判据在这层、措辞不在
///
/// 判据本身只有一个答案，两个产物各写一遍必然漂移（一处放行、一处拒绝，
/// 而"哪一处才是对的"没人能从那两份代码里读出来）。措辞则**必须**各写一遍：
/// CLI 要说 `--skip-kouling` 并退 2，服务端要说"关掉了唯一的弹幕来源"并回 400
/// —— 把 flag 名塞进领域层会让界面上输出一句没有 flag 的话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveRoomFault {
    /// 关掉了唯一的口令来源，又没给手工口令。
    SkipWithoutCodes,
    /// 要听弹幕，可时长不是正数。
    ListenNotPositive,
}

impl LiveRoomFault {
    /// 判据的中性描述（**不含 flag 名**，措辞由调用方自己写）。
    pub fn describe(self) -> &'static str {
        match self {
            Self::SkipWithoutCodes => "关掉了弹幕来源，却没有给手工口令",
            Self::ListenNotPositive => "听弹幕的时长必须是正数",
        }
    }
}

impl LiveRoomParams {
    /// 判这一组参数**是否成立**（纯函数）。
    ///
    /// ```text
    /// skip_kouling && codes.is_empty()    → SkipWithoutCodes
    /// listen_seconds == 0                 → ListenNotPositive
    /// ```
    ///
    /// ⚠ 时长那一条**不看 `skip_kouling`**：非正在**任何**情况下都不是合法输入。
    /// 把它挂在 `skip_kouling` 后面会让"不用弹幕 + 0 秒"悄悄放行，
    /// 而那时 0 秒会被传下去、让那次 WS 与并行的 GET **一起**立刻超时，
    /// 表现成一句方向错的通知（"两个口令来源都失败了"）。
    pub fn check(&self) -> std::result::Result<(), LiveRoomFault> {
        if self.skip_kouling && self.codes.is_empty() {
            return Err(LiveRoomFault::SkipWithoutCodes);
        }
        if self.listen_seconds == 0 {
            return Err(LiveRoomFault::ListenNotPositive);
        }
        Ok(())
    }

    /// 把参数翻成听写时长。
    ///
    /// ⚠ 非正数在这里**不当错误抛出去**：用法校验的措辞是两个产物各自的，
    /// 而这里抛错会让"没校验就调"变成一句与参数无关的运行时错误。
    /// 调用方**仍须**先挡住非正数（[`Self::check`]）。
    fn listen_for(&self) -> Duration {
        Duration::from_secs(self.listen_seconds)
    }
}

// ══════════════════════════ 门 ══════════════════════════

/// [`mcloud_upstream::live_room::LiveRoom`] 的重导出（编排层判据要用到它）。
pub use mcloud_upstream::live_room::LiveRoom as UpstreamLiveRoom;

/// "要不要去听口令"的判定结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KoulingGate {
    /// **有**符合条件（直播中 + 主播是那个固定昵称）的直播间。
    pub active: bool,
    /// 通过筛选的直播间（已按 id 去重）。
    pub rooms: Vec<live::LiveRoom>,
    /// 筛完 `status == 1` 之后剩下的房间数（**用于区分两种"没有"**）。
    ///
    /// ⚠ 它与 `active` 一起才说得清现状：
    ///
    /// ```text
    /// live_count == 0            → 谁都没在播（今天没有直播）
    /// live_count > 0 && !active  → 有人**在播**，但不是那个主播
    /// ```
    ///
    /// 实拍踩过：房间 147490「福建移动」是 `status == 1`，与真正要听的那间
    /// 在**同一个页签**里。只报 `active == false` 会把这两种情况混成一句
    /// "没有直播"，而它们的下一步**完全不同**（一个是等，一个是别等了）。
    ///
    /// ⚠ 这个字段存在的**唯一理由**就是区分前两种 —— 别拿它当"在播房间数"用。
    pub live_count: usize,
}

/// 判定门的开合（纯函数）。
///
/// # 判据是**两个条件同时成立**
///
/// ```text
/// room.status == 1  且  room.anchor.nickname == "中国移动云盘"
/// ```
///
/// ⚠ **只看 `status == 1` 会误判**：同一个页签里别的主播也在播（实拍 147490）。
/// ⚠ **`channelId` 不能当过滤条件**：返回的房间的 `channelId` 有 29/51/243/134
/// ——它表达的是"房间归属哪个频道"，不是"我拉的是哪个频道"。
///
/// 判据（`status` 与昵称**精确相等**）本身在协议层 [`live::kouling_rooms`] 里，
/// 本函数只负责把"两种没有"的计数也算出来。
pub fn judge_kouling_gate(rooms: &[live::LiveRoom]) -> KoulingGate {
    let live_count = rooms.iter().filter(|r| r.is_live()).count();
    let kouling = live::kouling_rooms(rooms);
    KoulingGate {
        active: !kouling.is_empty(),
        rooms: kouling,
        live_count,
    }
}

// ══════════════════════════ 口令合并与重试 ══════════════════════════

/// 合并多个来源的口令（保序去重、逐条 trim、丢掉空串）。
///
/// 两个来源（WebSocket 弹幕 / 小红书主页）**地位相同**：任一来源为空不影响另一个。
pub fn merge_kouling<S: AsRef<str>>(sources: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(sources.len());
    for value in sources {
        let code = value.as_ref().trim();
        if code.is_empty() || out.iter().any(|c| c == code) {
            continue;
        }
        out.push(code.to_string());
    }
    out
}

/// 把**一栏自由文本**拆成口令（空白 / 换行 / 逗号 / 顿号都算分隔符）。
///
/// 分隔符集合是**边界**而不是内部实现：CLI 的 `--codes` 帮助文案与界面上
/// 那一栏的占位文案都指向它，所以两个产物必须共用一份。
///
/// # ⚠ 空白也算分隔符，而这一点**原先漏了**
///
/// 早先这里只拆 `\n \r , ， 、`，于是 `"口令甲 口令乙"` 会被当成**一条**口令
/// 发上去 —— 服务端必然回 `200108 口令无效`，而用户看到的是
/// "我明明填了两条，怎么只领了一条还说口令错"。
///
/// 为什么会漏：WebUI 那边自己在 `api.ts` 里先按空格拆了一遍（作为"超集"），
/// 于是**界面上看着是对的**；而 CLI 与 HTTP API 两条路都直接走本函数，
/// 没有那层补偿。⇒ 修在**这一处**（所有入口共用），而不是在各调用方各补一次。
///
/// ⚠ 用 [`char::is_whitespace`]（含全角空格 `U+3000`、`\t`、`\u{a0}`），
/// **不是** ASCII 的 `' '` —— 手机输入法默认就是全角空格，而口令是
/// 从手机里复制粘贴出来的，全角空格在这里是**常见**输入而不是边角情况。
pub fn split_kouling(raw: &str) -> Vec<String> {
    let fields: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、'))
        .map(str::to_string)
        .collect();
    merge_kouling(&fields)
}

/// 回答"要不要**继续听**一次，以及听的时候该排除哪些"（纯函数）。
///
/// # ⚠⚠ 判据在 2026-10-01 换掉了（用户口径）
///
/// **要的是两条*有效*的口令，不是"收到两条"。** 原来的实现是"有口令被判无效才重听" ✗
/// —— 于是"一条都没收到""收到两条全是没分类的码""只验出一条有效"这三种情况
/// 一趟就结束了，而它们恰恰是**最该继续听下去**的情形。
/// 现在看的是**池里（含这一趟刚验出来的）有效条数够不够 [`VALID_ENOUGH`]**。
///
/// # 两个容易写错的边界
///
/// - **够了就不再听**：验出两条有效之后多听 60 秒纯属白喂风控；
/// - **`exclude` 必须含第一轮"已经试过的"**（含**第一次就成功的**）——
///   不排除已成功的话，第二轮会把同一批口令再领一遍，而**同一账号**对同一条
///   **正确**口令的第二次请求会回 `200112`（已兑换）：它不是新结果，
///   却会污染 [`RedFlowerReport::draws`] / `invalid_codes` 的归因，
///   看起来像"口令坏了"。
///
/// ⚠⚠ 上面那条的理由**不是**"口令会被消耗" —— 口令是**全场通用**的
/// （事实 3），所有观众共用同一批，不存在"用掉"。**别把"消耗"写回来。**
pub fn retry_plan(
    first_batch: &[String],
    invalid_codes: &[String],
    valid_count: usize,
) -> (bool, Vec<String>) {
    // 够了（两条有效）：这一趟收工，别再占着弹幕窗口。
    if valid_count >= mcloud_core::kouling::VALID_ENOUGH {
        return (false, Vec::new());
    }
    // 还不够：继续听。排除表 = 第一批 + 判无效的（**含第一次就成功的**，
    // 否则第二轮会把同一批口令再领一遍，而"已兑换"会污染归因）。
    let mut all: Vec<String> = first_batch.to_vec();
    all.extend(invalid_codes.iter().cloned());
    (true, merge_kouling(&all))
}

/// 从重听结果里挑出**这次真正要领取**的口令（纯函数）。
///
/// 判据是"既不在第一批里、也不在无效表里" —— 与 [`retry_plan`] 的排除表同一份
/// 依据，但用途不同：那个决定"听的时候排除谁"，这个决定"领的时候领谁"。
pub fn pick_new_codes(
    retried: &[String],
    already_tried: &[String],
    invalid_codes: &[String],
) -> Vec<String> {
    let mut tried: Vec<&str> = already_tried.iter().map(|s| s.trim()).collect();
    tried.extend(invalid_codes.iter().map(|s| s.trim()));
    merge_kouling(retried)
        .into_iter()
        .filter(|code| !tried.contains(&code.as_str()))
        .collect()
}

// ══════════════════════════ 六步链 ══════════════════════════

/// 六步链里的一步。
///
/// 顺序**固定**：
///
/// ```text
/// ① user/info            ← 余额（**终值核对也是它**）
/// ② visitlog/journaling ×5 ← 埋点
/// ③ firstParticipate/give  ← 首次参与惊喜奖
/// ④ redFlower/draw?code=   ← 逐个口令（**唯一写请求**）
/// ⑤ queryRevivalFlowers    ← 复活花
/// ⑥ user/info            ← 再取一次，前后相减
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedFlowerStep {
    /// ① `user/info`（**起始余额**）。
    BalanceBefore,
    /// ② `visitlog/journaling`（**要发 5 次**）。
    Journaling,
    /// ③ `firstParticipate/give`（首次参与惊喜奖）。
    FirstParticipate,
    /// ④ `redFlower/draw?code=`（**每个口令一次**，写操作）。
    Draw,
    /// ⑤ `queryRevivalFlowers`（复活花，**不是余额**）。
    RevivalFlowers,
    /// ⑥ `user/info`（**终值**，与①相减才是本次进账）。
    BalanceAfter,
    /// 链已走完。
    Done,
}

impl RedFlowerStep {
    /// 稳点的短名，**进错误前缀**（见 [`RedFlowerStep::label`] 的理由）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BalanceBefore => "user_info_before",
            Self::Journaling => "journaling",
            Self::FirstParticipate => "first_participate",
            Self::Draw => "draw",
            Self::RevivalFlowers => "revival_flowers",
            Self::BalanceAfter => "user_info_after",
            Self::Done => "done",
        }
    }
}

/// 一个口令的抽取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawRecord {
    /// 拿去领取的那条口令。
    pub code: String,
    /// 上游的判定。
    pub outcome: RedFlowerDrawOutcome,
}

/// 走完六步之后的结论。
///
/// ⚠ 这里**不带**任何票：六步链用的是云盘侧会话（[`SessionRecipe::SignInPage`]），
/// 那是能签到、能读AI豆的凭据，而它在这个结构里**零消费者** ——
/// 结果会顺着日志与 run 报告走，"顺手把凭据也带上"就是泄漏最常见的路径。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedFlowerReport {
    /// ① 的读数。
    pub balance_before: RedFlowerBalance,
    /// ⑥ 的读数。
    pub balance_after: RedFlowerBalance,
    /// ③ 的结论（`200110` 已参与过是**正常**的一支）。
    pub first_participate: Option<mcloud_upstream::live_flower::FirstParticipateOutcome>,
    /// ④ 逐口令的结果（**保序**，与传入的 codes 同序）。
    pub draws: Vec<DrawRecord>,
    /// ⑤ 的读数。
    pub revival_flowers: i64,
    /// 被判"口令不对"的那些（`200108`，**决定要不要重听一次**）。
    pub invalid_codes: Vec<String>,
    /// 回了**别的**业务码的那些（既不是无效也不是已兑换）。
    ///
    /// ⚠ 单独留一栏：它们是"上游说了个我们没分类的码"，与"口令错"排查方向
    /// 完全不同。混进 `invalid_codes` 会触发一次**毫无意义**的 60 秒重听。
    pub other_codes: Vec<String>,
    /// [`RedFlowerDrawKind::KoulingExpired`]（`200118`）—— **这条口令认识、
    /// 但本场直播已经结束了**。
    ///
    /// ⚠⚠ 单独一栏的理由在**用户可见的那句话**上：它最常见的成因是
    /// "直播已经结束了"，而「未分类口令」这个词会把人往**"口令抄错了"**
    /// 那边引，排查方向一开始就错。
    ///
    /// ⚠ 它**不是**无效口令：口令本身是**对的**（错的回 `200108`）。
    /// 所以既不进 `invalid_codes`、也**不触发**那次 60 秒重听
    /// —— 重听也只会再拿一次 `200118`。
    pub expired_codes: Vec<String>,
}

impl RedFlowerReport {
    /// 本次进账几朵（**⑥ − ①**）。
    ///
    /// # ⚠⚠ 用同一个接口的两次读数相减，不是把口令的 `flower_num` 加起来
    ///
    /// 相加会**漏掉③首次参与惊喜奖**进账的那几朵，而它**不在**任何一个口令的
    /// 结果里 —— 那正好是"总数对不上"这一类问题最常见的来源。
    pub fn gained(&self) -> i64 {
        self.balance_after.flower_cnt - self.balance_before.flower_cnt
    }

    /// 有几个口令真的换到了花。
    pub fn succeeded(&self) -> usize {
        self.draws
            .iter()
            .filter(|r| r.outcome.kind == RedFlowerDrawKind::Succeeded)
            .count()
    }

    /// 领了几个口令。
    pub fn attempted(&self) -> usize {
        self.draws.len()
    }

    /// 这次到底领没领（`false` = 一个口令都没发出去）。
    pub fn attempted_any(&self) -> bool {
        !self.draws.is_empty()
    }

    /// "这一趟跑成了什么"的那句话，**不含**账号前缀。
    pub fn summary_line(&self) -> String {
        let mut out = format!(
            "小红花 {} → {}（进账 {}，成功 {}/{}）",
            self.balance_before.flower_cnt,
            self.balance_after.flower_cnt,
            self.gained(),
            self.succeeded(),
            self.attempted()
        );
        let notes = self.notes_suffix();
        if !notes.is_empty() {
            out.push_str(&notes);
        }
        out
    }

    /// 拼"这次有什么值得多看一眼"的尾巴。
    fn notes_suffix(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.invalid_codes.is_empty() {
            parts.push(format!("无效口令 {}", self.invalid_codes.join(" / ")));
        }
        if !self.expired_codes.is_empty() {
            // ⚠⚠ 这句是**用户可见的**：说"口令已失效（本场已结束）"，
            // **不说**「未分类口令」—— 后者把人往"口令抄错了"（无效口令）那边引，
            // 而这两件事的排查方向完全不同。
            parts.push(format!(
                "口令已失效（本场已结束）{}",
                self.expired_codes.join(" / ")
            ));
        }
        if !self.other_codes.is_empty() {
            // ⚠ 与无效口令、与"已失效"都**分开说**：它是"上游回了个我们没分类的码"，
            // 排查方向完全不同（既不是口令抄错了，也不是直播结束了）。
            parts.push(format!("未分类口令 {}", self.other_codes.join(" / ")));
        }
        if let Some(fp) = &self.first_participate {
            // ⚠ `> 0` 不是洁癖：`Gained` 是零值，于是"没走过这一步"的报告
            // 会拖出一句"首次参与 +0"。
            if fp.kind == FirstParticipateKind::Gained && fp.flower_num > 0 {
                parts.push(format!("首次参与 +{}", fp.flower_num));
            }
        }
        if self.revival_flowers != 0 {
            parts.push(format!("复活花 {}", self.revival_flowers));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("（{}）", parts.join("；"))
    }
}

/// 把第二轮的结果并进第一轮。
///
/// ⚠ 余额取**第二轮**的终值（时间上更晚，也更接近"领完之后到底有多少"），
/// 而起始余额保留第一轮的 —— **两次读数之差**才是"这两轮加起来进账多少"。
pub fn merge_reports(first: &RedFlowerReport, second: &RedFlowerReport) -> RedFlowerReport {
    let mut merged = first.clone();
    merged.balance_after = second.balance_after.clone();
    merged.draws.extend(second.draws.iter().cloned());
    // ⚠ 无效口令取**并集**，不取第二轮那份。
    //
    // 第二轮里没有甲，只是因为它被重听时的 `exclude` 排除了 —— 而"甲被判无效"
    // 是一件**已经发生的事实**。取第二轮那份会让这条事实从结果里消失，
    // 于是"成功 1/2"变成一句没有下文的话：用户看不出那一个为什么没成，
    // 而它恰恰是最该看到的一条（意味着弹幕里那条口令是抄错了的）。
    let mut invalid: Vec<String> = first.invalid_codes.clone();
    invalid.extend(second.invalid_codes.iter().cloned());
    merged.invalid_codes = merge_kouling(&invalid);
    merged
        .other_codes
        .extend(second.other_codes.iter().cloned());
    // ⚠ 过期口令**直接追加**即可，不需要去重：[`pick_new_codes`] 只挑"既不
    // 在第一批、也不在无效表"的口令，而过期口令**在第一批里** ⇒ 第二轮
    // 根本不会重领它。
    merged
        .expired_codes
        .extend(second.expired_codes.iter().cloned());
    if second.revival_flowers != 0 {
        merged.revival_flowers = second.revival_flowers;
    }
    if second.first_participate.is_some() {
        merged.first_participate = second.first_participate.clone();
    }
    merged
}

// ══════════════════════════ 六步链状态机 ══════════════════════════

/// 六步链的状态机。
///
/// # 两个刻意的设计
///
/// - **[`Self::next`] 不推进**：它返回"当前这一步该发什么"，推进只发生在
///   [`Self::feed`]。这样"请求构造失败"与"发出去失败了"不会各自偷偷推进一次
///   —— 少领一个口令是**静默**的（余额会告诉你有问题，但不会告诉你是哪个）。
///   也正因为不推进，票过期时可以**原样再取一次这一步**。
/// - **本层不 sleep、不取时钟**：重听一次的等待由调用方负责，
///   "要不要重听"由 [`retry_plan`] 回答（纯函数）。
#[derive(Debug, Clone)]
pub struct RedFlowerSession {
    codes: Vec<String>,
    step: RedFlowerStep,
    journaling_sent: usize,
    draw_index: usize,
    report: RedFlowerReport,
}

impl RedFlowerSession {
    /// 构造一次六步链。
    ///
    /// `codes` 的顺序**有意义**（决定先领哪个口令）：**同一账号**对同一条
    /// **正确**口令的第二次请求会回 `200112`（已兑换），所以"先领最新的那个"
    /// 能让日志里第一条就是最有信息量的。
    pub fn new(codes: Vec<String>) -> Self {
        Self {
            codes: merge_kouling(&codes),
            step: RedFlowerStep::BalanceBefore,
            journaling_sent: 0,
            draw_index: 0,
            report: RedFlowerReport::default(),
        }
    }

    /// 链是否已走完。
    pub fn done(&self) -> bool {
        self.step == RedFlowerStep::Done
    }

    /// 当前这一步。
    pub fn step(&self) -> RedFlowerStep {
        self.step
    }

    /// 这次要领取的口令（已去重）。
    pub fn codes(&self) -> &[String] {
        &self.codes
    }

    /// 当前报告（未走到的步骤留零值）。
    pub fn report(&self) -> &RedFlowerReport {
        &self.report
    }

    /// 返回**当前这一步**要发的请求。**不推进状态**。
    ///
    /// 请求构造在协议层，全部是纯函数（`live_flower::*_request`）。
    pub fn next(&self) -> Result<(RedFlowerStep, HttpRequest)> {
        let req = match self.step {
            RedFlowerStep::BalanceBefore => proto::user_info_request(),
            // ⚠ 埋点发的是**固定 5 条**，顺序即 [`proto::JOURNALING_EVENTS`]。
            // 其中**只有 `_flowerBtn` 是实拍证实的**，其余 4 条照抄原项目。
            RedFlowerStep::Journaling => {
                let event = *proto::JOURNALING_EVENTS
                    .get(self.journaling_sent)
                    .ok_or_else(|| Error::Other("埋点事件下标越界".into()))?;
                proto::journaling_request(event)
            }
            RedFlowerStep::FirstParticipate => proto::first_participate_give_request(),
            RedFlowerStep::Draw => {
                let code = self
                    .codes
                    .get(self.draw_index)
                    .ok_or_else(|| Error::Other("口令下标越界".into()))?;
                proto::draw_request(code)
            }
            RedFlowerStep::RevivalFlowers => proto::query_revival_flowers_request(),
            RedFlowerStep::BalanceAfter => proto::user_info_request(),
            RedFlowerStep::Done => {
                return Err(Error::Other("六步链已走完，没有下一步".into()));
            }
        };
        Ok((self.step, req))
    }

    /// 用响应推进状态机。
    ///
    /// # ⚠ 三条"不中断整批"的判据
    ///
    /// - `firstParticipate/give` 的 `200110`（已参与过）是**正常**的一支
    ///   —— 每个账号第二次跑这条链都会拿到它；
    /// - `draw` 的 `200112`（同一账号第二次请求同一条正确口令）说明口令**是对的**，
    ///   不该算无效；
    /// - 某个口令回了别的码（[`RedFlowerDrawKind::Other`]）**不该**让其余口令
    ///   一起被跳过：逐个口令重试是正常路径。
    ///
    /// 所以本函数**只在解析失败（响应不像信封）时**返回错误，业务码一律记进报告。
    ///
    /// # ⚠ 解析失败要**中止**，不要跳过
    ///
    /// 六步里每一步的响应都带着后面要用的信息（余额、判定）。解析失败意味着
    /// "上游换了信封形状"或"我们打错了端点"，继续跑只会把同一个问题重复六遍，
    /// 而且日志里看不出是哪一步开始的。⇒ 中止 + 原样带回错误，排查面最小。
    ///
    /// ⚠ 但**复活花（⑤）是唯一例外**：它拿不到不该让整条链失败 ——
    /// 六步里只有它纯属"顺便看一眼"。留 0 并继续。
    pub fn feed(&mut self, body: &str) -> Result<()> {
        let env = RedFlowerEnvelope::parse(body);
        match self.step {
            RedFlowerStep::BalanceBefore => {
                let balance = proto::parse_red_flower_balance(&env)?;
                self.report.balance_before = balance;
                self.step = RedFlowerStep::Journaling;
            }

            RedFlowerStep::Journaling => {
                // 埋点的响应**不解析**：它是"到场"证据，回什么码都不改变后面的步骤。
                //
                // ⚠ 这不是偷懒 —— 拿它当门槛会让"埋点接口偶发 5xx"变成整条链失败，
                // 而它本来只是噪声。
                self.journaling_sent += 1;
                if self.journaling_sent >= proto::JOURNALING_EVENTS.len() {
                    self.step = RedFlowerStep::FirstParticipate;
                }
            }

            RedFlowerStep::FirstParticipate => {
                self.report.first_participate = Some(proto::classify_first_participate(&env));
                self.step = if self.codes.is_empty() {
                    RedFlowerStep::RevivalFlowers
                } else {
                    RedFlowerStep::Draw
                };
            }

            RedFlowerStep::Draw => {
                let outcome = proto::classify_red_flower_draw(&env);
                let code = self
                    .codes
                    .get(self.draw_index)
                    .cloned()
                    .ok_or_else(|| Error::Other("口令下标越界".into()))?;
                // ⚠ 用协议层的 `rejected_kouling()` 而不是自己判等 `200108`：
                // 那个函数**刻意**只认实拍确认的那一个码（`200112` / `200118`
                // 都不是无效口令）。自己再写一份等于把这条判据复制两份。
                if outcome.rejected_kouling() {
                    self.report.invalid_codes.push(code.clone());
                }
                if outcome.kind == RedFlowerDrawKind::Other {
                    self.report.other_codes.push(code.clone());
                }
                if outcome.is_kouling_expired() {
                    self.report.expired_codes.push(code.clone());
                }
                self.report.draws.push(DrawRecord { code, outcome });
                self.draw_index += 1;
                if self.draw_index >= self.codes.len() {
                    self.step = RedFlowerStep::RevivalFlowers;
                }
            }

            RedFlowerStep::RevivalFlowers => {
                // ⚠ 唯一一个"解析失败不算失败"的步骤：复活花**不是余额**，
                // 拿不到它不该让整条链失败。留 0 并继续。
                self.report.revival_flowers = proto::parse_revival_flowers(&env).unwrap_or(0);
                self.step = RedFlowerStep::BalanceAfter;
            }

            RedFlowerStep::BalanceAfter => {
                let balance = proto::parse_red_flower_balance(&env)?;
                self.report.balance_after = balance;
                self.step = RedFlowerStep::Done;
            }

            RedFlowerStep::Done => {}
        }
        Ok(())
    }
}

/// 把"哪一步出的问题"写进错误消息。
///
/// 六步里 ①②⑥ 用的是同一个 `user/info`，而 ② 会发 5 次 ——
/// 不带步名的错误（"user/info 响应没有 result"）会让人分不清是**起始余额**
/// 还是**终值**。
pub fn wrap_step_error(step: RedFlowerStep, err: Error) -> Error {
    Error::Other(format!("[{}] {err}", step.as_str()))
}

// ══════════════════════════ 双源收集 ══════════════════════════

/// 一个来源的收集结果。
#[derive(Debug, Clone, Default)]
pub struct SourceOutcome {
    /// 这个来源收到的口令。
    pub codes: Vec<String>,
    /// 失败的**人话**理由（`None` = 这个来源成功）。
    pub failure: Option<String>,
}

/// 口令收集器 —— 编排层与真实 IO 之间的**接缝**。
///
/// # 为什么要有这个 trait
///
/// 这条链一年里只有主播开播的那几小时才能真跑一次。把"听弹幕"直接写进任务里，
/// 唯一能验证它的方式就是**等开播**；抽成接缝之后，整条编排链
/// （门 → 获取 → 领取 → 重听 → 结论）都能在单测里用一个假收集器走完。
///
/// ⚠ 它与 [`mcloud_upstream::live_ws_client`] 的关系：真实实现
/// [`LiveKoulingCollector`] 就调那一层。本层**不碰网络**。
#[async_trait]
pub trait KoulingCollector: Send + Sync {
    /// 从两个来源收口令（WS 弹幕 + 小红书主页），返回合并后的**保序**结果。
    ///
    /// `exclude` 是这次要排除的口令（重听时为已试过的那些）。
    async fn collect(
        &self,
        ctx: &TaskContext,
        jwt: &str,
        room: &live::LiveRoom,
        listen_for: Duration,
        exclude: &[String],
    ) -> Result<SourceOutcome>;
}

/// 真实收集器：WS 弹幕 + 小红书主页，**并发**跑。
///
/// # 为什么必须并发
///
/// WS 那一源要**听满**一段（默认 60 秒）才有意义，而小红书只是一次 GET。
/// 串行的话总耗时就是 `60s + 1s`，而其中 60 秒里网络是空闲的。
///
/// # 一个源坏了不影响另一个
///
/// 两个来源**地位相同**。只有**两个都坏**才算失败 —— 一坏一好时应当照常抽奖，
/// 而不是因为"小红书被风控了"整场放弃。
///
/// # 合并顺序**固定**
///
/// WS 在前、小红书在后。并发完成顺序是不确定的，而口令顺序决定先领哪一个。
/// 顺序不定会让日志每次都不一样，也会让"同一个输入跑两遍结果不同"变成常态。
#[derive(Default)]
pub struct LiveKoulingCollector {
    /// 显式覆盖 WS 端点（**仅测试用**；生产走 [`LiveWsEndpoint::production`]）。
    pub endpoint: Option<LiveWsEndpoint>,
}

impl LiveKoulingCollector {
    /// 生产实现。
    pub fn production() -> Self {
        Self::default()
    }

    /// 听弹幕那一支（源 A）。
    ///
    /// # ⚠ 这一支依赖 T6（`live_ws_client`）
    ///
    /// 它接的是真实 WS 连接层。**本轮没有在真机上跑过**（需要主播开播），
    /// 所以这里是"接线完成、真机未验证"。
    ///
    /// # `entered_room() == false` **不是错误**
    ///
    /// `roomBeat` 回 `chatRoomId: null` ⇒ 今天没开播 ⇒ `member/apply` 与 WS
    /// **根本不该发**。这是**结构性**事实，不是"连不上"：判据在**字段**上。
    /// 把它当错误的话，"今天没开播"会表现成一条网络故障。
    async fn listen_danmaku_with(
        &self,
        ctx: &TaskContext,
        jwt: &str,
        room: &live::LiveRoom,
        listen_for: Duration,
    ) -> Result<SourceOutcome> {
        let room_id = live::live_room_id_string(room.id);

        // ① 建立服务端 session。响应**不看**（少了它 ② 会拿不到 token）。
        let resp = ctx
            .http
            .request(live::current_user_info_request(jwt))
            .await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "直播·建会话")?;
        live::CmliveEnvelope::parse(body)?.ensure_ok("直播·建会话")?;

        // ② 取 WS 凭据。
        let resp = ctx.http.request(live::user_token_request(jwt)?).await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "直播·取 WS 凭据")?;
        let token = live::parse_user_token(&live::CmliveEnvelope::parse(body)?)?;

        // ③ 房间心跳 —— 它同时是"有没有聊天室"的唯一判据。
        let resp = ctx
            .http
            .request(live::room_beat_request(&room_id, jwt)?)
            .await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "直播·房间心跳")?;
        let beat = live::parse_room_beat(&live::CmliveEnvelope::parse(body)?)?;

        // ⚠ 没有聊天室 ⇒ 今天没开播 ⇒ 后面两枪**根本不该发**。
        // 返回空结果（不是错误）：这是"没开播"，与"连不上"不是一回事。
        let Some(chat_room_id) = beat.chat_room_id.clone() else {
            return Ok(SourceOutcome::default());
        };

        // ④ 进房间。
        let phone = ctx.account.auth.phone.clone();
        let user_name = format!("cmlive_c_{phone}");
        let timestamp = ctx.clock.now_ms().to_string();
        let resp = ctx
            .http
            .request(live::apply_room_request(
                &chat_room_id,
                &user_name,
                &user_name,
                &token.access_token,
                mcloud_upstream::live_msync::LIVE_DEVICE_INFO_HEX,
                &timestamp,
            )?)
            .await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "直播·进房间")?;
        live::parse_apply_room(body)?;

        // ⑤ 连 WS 并听。到点主动断开（`ListenRequest::listen_for` 是整体上限）。
        let endpoint = self
            .endpoint
            .clone()
            .unwrap_or_else(LiveWsEndpoint::production);
        let mut rng = mcloud_crypto::OsRandom;
        let report = live_ws_client::listen_danmaku(
            ListenRequest {
                access_token: &token.access_token,
                chat_room_id: &chat_room_id,
                phone: &phone,
                listen_for,
            },
            &endpoint,
            &mut rng,
            ctx.clock.as_ref(),
        )
        .await?;

        // ⚠ 只报**过程事实**，不在这里下"直播结束了"的结论 ——
        // 那是编排层的判断（它手上才有门的 `status`）。
        if !report.entered_room() {
            ctx.info(format!(
                "弹幕连接未进成房间（结束方式 {:?}，帧 {}，坏帧 {}）",
                report.end, report.frames, report.malformed
            ));
        }
        Ok(SourceOutcome {
            codes: report.codes,
            failure: None,
        })
    }

    /// 小红书那一支（源 B）。
    ///
    /// ⚠ 返回空**不是错误**：主页按时间倒序，这一期可能还没发笔记。
    /// 与"页面被风控挡了"在解析层看起来一样（都返回空）。
    async fn fetch_xiaohongshu(&self, ctx: &TaskContext) -> Result<SourceOutcome> {
        let resp = ctx
            .http
            .request(mcloud_upstream::live_kouling::xiaohongshu_profile_request())
            .await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "小红书主页")?;
        Ok(SourceOutcome {
            codes: mcloud_upstream::live_kouling::extract_kouling_from_xiaohongshu_html(body),
            failure: None,
        })
    }

    /// 并发收两个来源并合并（真正的实现，`listen_for` 由调用方给）。
    async fn collect_inner(
        &self,
        ctx: &TaskContext,
        jwt: &str,
        room: &live::LiveRoom,
        listen_for: Duration,
        exclude: &[String],
    ) -> Result<SourceOutcome> {
        let ws = self.listen_danmaku_with(ctx, jwt, room, listen_for);
        let xs = self.fetch_xiaohongshu(ctx);
        let (ws, xs) = futures::join!(ws, xs);

        // 一个源坏了不影响另一个；只有**两个都坏**才算失败。
        let mut failures: Vec<String> = Vec::new();
        let mut codes: Vec<String> = Vec::new();
        match ws {
            Ok(o) => codes.extend(o.codes),
            Err(e) => {
                ctx.warn(format!("弹幕来源失败：{e}"));
                failures.push(format!("websocket：{e}"));
            }
        }
        match xs {
            Ok(o) => codes.extend(o.codes),
            Err(e) => {
                ctx.warn(format!("小红书来源失败：{e}"));
                failures.push(format!("xiaohongshu：{e}"));
            }
        }
        if failures.len() == 2 {
            return Err(Error::Other(format!(
                "两个口令来源都失败了 —— {}",
                failures.join("；")
            )));
        }

        let merged = merge_kouling(&codes);
        let excluded: Vec<&str> = exclude.iter().map(|s| s.trim()).collect();
        Ok(SourceOutcome {
            codes: merged
                .into_iter()
                .filter(|c| !excluded.contains(&c.as_str()))
                .collect(),
            failure: None,
        })
    }
}

#[async_trait]
impl KoulingCollector for LiveKoulingCollector {
    async fn collect(
        &self,
        ctx: &TaskContext,
        jwt: &str,
        room: &live::LiveRoom,
        listen_for: Duration,
        exclude: &[String],
    ) -> Result<SourceOutcome> {
        self.collect_inner(ctx, jwt, room, listen_for, exclude)
            .await
    }
}

// ══════════════════════════ 编排 ══════════════════════════

/// 一个账号这一趟的全部产出。
#[derive(Debug, Clone, Default)]
pub struct LiveRoomAccountResult {
    /// 门的判定（含"两种没有"的区分）。
    pub gate: KoulingGate,
    /// 门那三步真的换到了直播侧凭据吗。
    ///
    /// ⚠ 只存"有没有"，**不存那一串**：它是凭据，不该顺着进度与结果流进日志。
    pub has_jwt: bool,
    /// 这次实际拿到并拿去领取的口令。
    pub codes: Vec<String>,
    /// 六步链的结论；`None` 表示**没领**（门没开 / 一个口令都没收到）。
    pub report: Option<RedFlowerReport>,
    /// 有没有触发过"重听一次"。
    pub retried: bool,
    /// 这一趟用的参数（回显给调用方拼结论用，**不是新事实**）。
    pub params: LiveRoomParams,
}

/// 四个结论状态 —— [`RedFlowerReport`] 的领域层映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunVerdict {
    /// 没领（门没开 / 一个口令都没收到）。
    Idle,
    /// 领了、全部成功。
    AllSucceeded,
    /// 领了、成了**一部分**。
    PartialSuccess,
    /// 领了、**一个都没成**。
    NoneSucceeded,
}

impl LiveRoomAccountResult {
    /// 把这一趟的产出合成**四态结论**。
    ///
    /// | 情形 | 结论 |
    /// |---|---|
    /// | **没领**（门没开 / 一个口令都没收到） | **成功** |
    /// | 领了、**一个都没成** | **失败** |
    /// | 领了、成了**一部分** | **部分成功** |
    /// | 领了、全成 | 成功 |
    ///
    /// ⚠ 「没领 ⇒ 成功」的理由：「今天没开播」是**正常结果**（与签到里
    /// "今天已经签过了"同一档），不是失败。但**失败**那一档不是"没跑到"，
    /// 是"目标没达成"，所以小结里必须带原因（无效 / 已失效 / 未分类），
    /// 不能只剩一个光秃秃的"失败"。
    pub fn verdict(&self) -> RunVerdict {
        let Some(report) = &self.report else {
            return RunVerdict::Idle;
        };
        let total = report.attempted();
        let succeeded = report.succeeded();
        match (total, succeeded) {
            (0, _) => RunVerdict::Idle,
            (_, 0) => RunVerdict::NoneSucceeded,
            (t, s) if s < t => RunVerdict::PartialSuccess,
            _ => RunVerdict::AllSucceeded,
        }
    }

    /// 翻成 runner 看得懂的结论。
    ///
    /// ⚠ **本仓铁律**：失败只能是 [`TaskOutcome::failed`]，不能打成日志就算了。
    pub fn outcome(&self) -> TaskOutcome {
        match self.verdict() {
            RunVerdict::Idle => TaskOutcome::success(self.idle_summary()),
            RunVerdict::NoneSucceeded => {
                let report = self.report.as_ref().expect("没领就不会走到这一档");
                TaskOutcome::failed(format!("{}（一个口令都没成）", report.summary_line()))
            }
            RunVerdict::PartialSuccess => {
                let report = self.report.as_ref().expect("没领就不会走到这一档");
                TaskOutcome::success(format!("{}（部分成功）", report.summary_line()))
            }
            RunVerdict::AllSucceeded => {
                let report = self.report.as_ref().expect("没领就不会走到这一档");
                TaskOutcome::success(report.summary_line())
            }
        }
    }

    /// 说清"为什么这一趟什么都没领"。
    ///
    /// # ⚠⚠ 三种"没领"必须**分开说**
    ///
    /// ```text
    /// 门没开 & live_count==0 → "今天没有直播，没有口令可收"     （下一步：等）
    /// 门没开 & live_count>0  → "有 N 间在播，但没有一间是「…」"  （下一步：别等了）
    /// 开了门但没收到        → "开了门但没收到口令"
    /// ```
    ///
    /// ⚠ 混成一句"没有可领的口令"会让人去查**收集**那一步，
    /// 而那一步根本没被走到过 —— 它们的下一步**完全不同**。
    pub fn idle_summary(&self) -> String {
        if !self.gate.active {
            if self.gate.live_count == 0 {
                return "今天没有直播，没有口令可收".to_string();
            }
            return format!(
                "有 {} 间在播，但没有一间是「{}」",
                self.gate.live_count,
                mcloud_upstream::live_consts::KOULING_ANCHOR
            );
        }
        "开了门但没收到口令".to_string()
    }
}

/// 一趟运行的**世界**（依赖注入点）。
///
/// # ⚠ 这里**没有** `live_jwt` 字段（曾经有一个，已删）
///
/// 早先它是 `Option<String>`，由调用方从外面填 —— 而两处调用方都填 `None`，
/// 于是"开门"那一段**永远进不去**：没配 `codes` 时一个口令都收不到，
/// 表现成"今天没有直播"。那是**假结论**，且没有任何测试会红。
///
/// 现在直播侧 JWT 由 [`LiveRoomRun::open_gate`] 在**需要时**自己换
/// （即"有 codes ⇒ 跳过开门"这一支之外），票活在这一个函数里、不外流。
///
/// # 为什么票不该从外面注入
///
/// 它是**直播侧**的一张票，与六步链的云盘侧 `jwttoken` 是两回事
/// （见 [`LiveRoomRun::open_gate`] 里那张对照表）。让调用方塞进来的话，
/// "塞错一张票"就变成了一个**编译得过**的错误 —— 而它的表现是
/// `90001 未登录`，排查方向指向"账号没登录"，完全错。
pub struct LiveRoomRun<'a> {
    /// 口令收集器（真实实现 = [`LiveKoulingCollector`]）。
    pub collector: &'a dyn KoulingCollector,
}

impl<'a> LiveRoomRun<'a> {
    /// 从任务参数解析出编排参数。
    ///
    /// `codes` 支持"一栏自由文本"（[`split_kouling`]）或字符串数组。
    pub fn params_from_body(body: &Value) -> LiveRoomParams {
        let codes = match body.get("codes") {
            Some(Value::String(raw)) => split_kouling(raw),
            Some(Value::Array(items)) => {
                let list: Vec<String> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                split_kouling(&list.join("\n"))
            }
            _ => Vec::new(),
        };
        LiveRoomParams {
            codes,
            listen_seconds: body
                .get("listenSeconds")
                .and_then(Value::as_u64)
                // 默认 60 秒：与参考实现一致（听满一段才有意义）。
                .unwrap_or(60),
            skip_kouling: body
                .get("skipKouling")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }

    /// 跑**一个账号**：开门 → 获取口令 → 领取 → （必要时）重听再领。
    ///
    /// # 返回的错误里带"崩在第几步"
    ///
    /// 返回的 [`LiveRoomAccountResult`] 在出错时也尽量带已经拿到的部分
    /// （门、口令），调用方据此能说出"崩在第几步"，而不是一句错误串。
    ///
    /// # ⚠⚠ 取舍：**有 `codes` 就跳过开门**（我做的决定，理由在此）
    ///
    /// 判据是 `out.codes.is_empty() && !skip_kouling` —— 两个条件都成立才开门。
    ///
    /// **采纳"跳过"的三条理由：**
    ///
    /// 1. **省两个请求**：`querySpecTokenV2` + `cmpassLoginByCmToken` 是纯开销
    ///    —— 手工 codes 已经够跑完六步链，门换来的 JWT 只用于房间发现与 WS，
    ///    而这两件事在"已有 codes"这一支**根本不会发生**。
    /// 2. **避免门失败拖累领取**（这条才是关键的）：若先开门再看 codes，
    ///    那么门那两枪偶发 5xx 会把一趟**本来能跑完**的领取整体拖黄。
    ///    用户给的口令是对的、六步链是好的，却因为"换个票失败了"一朵花没拿到
    ///    —— 归因方向全错（他会去查口令）。
    /// 3. **与 [`RedFlowerTask`] 口径一致**：那一支就是"已经有口令、直接领"，
    ///    它从来不开门。两者是同一条链的两个入口，不该在"要不要门"上分叉。
    ///
    /// ⚠ **代价必须说出来**：有 codes 时**收不到弹幕里的口令**（因为没开门）。
    /// 这是**预期**的 —— 模块头事实 1 说清了小红书那条只能人工喂，
    /// 而"既有 codes 又想听弹幕"这个组合在本任务里**不存在**：
    /// 给了 codes 就是"我不用你听"的意思。真想把两边都要到手，
    /// WebUI/CLI 的正确表达是跑两趟（一趟带 codes、一趟不带）。
    ///
    /// ⚠ 反过来说：**没有** codes 时开门失败**必须如实上抛**，
    /// 绝不能退化成"今天没有直播"。见 [`Self::open_gate`]。
    ///
    /// # ⚠ 2026-10-01 补第三条：**池里够 2 条有效也不再开门**
    ///
    /// 口令现在是**全实例共享**的资源（见 [`settle_pool`]）：池里已经有两张能用的票时，
    /// 全场都不该再去抓（用户口径）。于是"开门"的完整判据是**三个条件的与**：
    ///
    /// ```text
    /// 手里没有可用口令（含池里挑出来为空的）  且  池里有效 < 2 条  且  没开「只听手工」
    /// ```
    ///
    /// 再叠一道**90 秒去重**（[`mcloud_core::KoulingPool::scrape_ready`]）：
    /// 别的账号刚开抓就别再扑上去 —— 这就是"一个 mcloud 用户只开一个抓取"那一句。
    ///
    /// # ⚠ 票（`jwttoken`）的来源：**云盘侧**，不是直播侧
    ///
    /// 六步链全打在 `m.mcloud.139.com/ycloud`（与签到同域同族，公共头里那个
    /// `activityid: sign_in_3` 就是证据），它要的 `jwttoken` 是**云盘侧
    /// `tyrzLogin` 那一份**，**不是**门换到的直播侧 JWT —— 拿后者去读，
    /// 实测回 **`90001 未登录`**（2026-09-22 两个账号都是那一句）。
    ///
    /// ⇒ **jwttoken 使用公共来源**：这条六步链要的票是签到页那一套参数换的
    /// （[`SessionRecipe::SignInPage`]）。本任务不自己拼票：`TaskContext` 里的
    /// `http` 若已经是 [`mcloud_upstream::H5Client`]，票会被**透明注入**
    /// （它只接管 `m.mcloud.139.com` 这一族），任务代码一行都不用碰。
    ///
    /// # ⚠ 只听**第一间**房间 —— 是刻意的取舍，不是漏了循环
    ///
    /// 原项目 `for (const room of rooms)` 会把该主播**每一间**在播房间都听一遍。
    /// 这里只取第一间：同时开两间以上是**少数情况**，而顺序听 N 间会把墙钟
    /// 变成 `N × listen_seconds`（默认 60s ⇒ 3 间就是 3 分钟）。
    ///
    /// ⚠ **代价必须说出来**：同时开两间时，第一间之外的口令会被**整场漏掉**。
    /// 所以**只要房间数 > 1 就必须 warn**，否则用户看到"在播房间 X，听 60 秒"
    /// 会以为已经听全了。
    pub async fn run_account(
        &self,
        ctx: &TaskContext,
        params: &LiveRoomParams,
    ) -> Result<LiveRoomAccountResult> {
        let mut out = LiveRoomAccountResult {
            params: params.clone(),
            ..Default::default()
        };

        // ── 口令来源：**共享池** + 手工 ──────────────────────────────────
        //
        // 顺序很关键：这一段必须**在**开门之前 —— 门要不要开，
        // 取决于这里挑出来了几条（见下面那句判据）。
        //
        // 用户口径（2026-10-01）：口令是**全实例共享**的资源 ——
        // 别的账号、别的 mcloud 用户抓到并验过的，这里直接用；
        // 手工粘的**验证通过后也进共享**（见 [`settle_pool`]）。
        let now_ms = ctx.clock.now_ms();
        let mut pool = ctx.store.kouling_pool().await?;
        pool.normalize(now_ms);
        let account_id = ctx.account.id.to_string();
        // 手工给的排前面（"我就要用这几条"），池里挑的是补充。
        let mut picked = merge_kouling(&params.codes);
        for code in pool.pick_for(&account_id) {
            if !picked.contains(&code) {
                picked.push(code);
            }
        }
        out.codes = picked;
        // 池里已判死的别拿去凑数，也别让重听把它们再听一遍。
        let known_invalid = pool.invalid_values();

        // 门只在"需要收口令"时才开：有 codes 时一个请求都不多发。
        //
        // ⚠ `gate_jwt` 是这一趟**唯一**的直播侧票持有者：门第③步与收集器
        // 都从它借用。这样"重听"那一段也能拿到同一张票，
        // 而不会出现"第二次去取票"（那会多发两枪，且两张票不是同一张）。
        let mut gate_jwt: Option<String> = None;
        //
        // ⚠ 判据是**两个条件的与**（2026-10-01 定）：
        //
        // ```text
        // 手里没有可用的口令  且  池里有效不足 2 条  ⇒ 才开门去抓
        // ```
        //
        // 为什么不是"只要池里不够就抓"：手工口令是小红书那条的**唯一入口**
        // （它没有可鉴权的接口）。那样改会让"门那两枪偶发失败"把一趟
        // **本来能跑完**的领取整体拖黄 ✗ —— 这是本文件早就记着的取舍，
        // 而它并不与共享冲突：手工那几条**验证后也会进池**（见 [`settle_pool`]），
        // 池就是这么被填起来的。
        //
        // 反过来说，"这个账号还有池里没领过的"也算**有得用**（`pick_for` 非空）
        // ⇒ 不必抓。于是抓取天然只在"谁都还没拿到口令"时发生一次，
        // 而"有效够 2 条"之后全场都不再抓（用户口径）。
        //
        // ⚠ 再加一道**去重**：别的账号刚开抓（90 秒内）就别再扑上去 ——
        // 用户口径"一个 mcloud 用户只需要开一个 ws/小红书抓取"。这道判据是
        // **持久化**的（不是进程内标志）：重启后仍然有效，而池存在的意义就是跨重启。
        if out.codes.is_empty()
            && !params.skip_kouling
            && !pool.stop_scraping()
            && pool.scrape_ready(now_ms)
        {
            // ⚠ 开门失败**如实上抛** —— 绝不退化成"今天没有直播"（那是撒谎：
            // "我们凭据坏了"与"今天没开播"是两件事，后者是**假结论**）。
            let jwt = self.open_gate(ctx).await?;
            out.has_jwt = true;
            let all_rooms = self.gate_rooms(ctx, &jwt).await?;
            out.gate = judge_kouling_gate(&all_rooms);
            // ⚠ 把**全部**房间交给池（含门必然筛掉的预告与回放）：界面上那行
            // "下一场几点"读的就是预告场的时间。只留门认可的那几间 ⇒ 日历就丢了。
            // ⚠⚠ 但"全部"里混着**别的主播** ⇒ 得告诉池"我们只关心哪个主播"，
            // 否则「下一场」会挑到别人的场次（用户口径 2026-10-02）。
            pool.note_schedule_anchor(mcloud_upstream::live_consts::KOULING_ANCHOR);
            pool.note_probe(&schedule_rooms(&all_rooms), out.gate.active, now_ms);
            // ⚠ 记"我开抓了"必须在**动手之前**：收集要听满 listenSeconds（默认 60 秒），
            // 抓完再记 = 整个窗口都敞着，别的账号照样扑上来。
            //
            // ⚠⚠ 这里**连"门最后是关的"也会记**（`note_scrape` 在 `if !out.gate.active`
            // 之前）：效果是 90 秒内不再重复问门。这是**故意**的 —— 判定门的结果
            // 在一分钟内不会变，而"没开播"是这一整段里最正常的结局，
            // 每个账号每分钟都去打两枪只是白白喂风控。代价：开播最多晚 90 秒被发现。
            pool.note_scrape(now_ms);
            if let Err(e) = ctx.store.save_kouling_pool(&pool).await {
                ctx.warn(format!("口令池写回失败（不影响本次运行）：{e}"));
            }
            gate_jwt = Some(jwt);
            let jwt = gate_jwt.as_deref().expect("刚放进去");

            if !out.gate.active {
                // ⭐ 门的两种"没有"分开说 —— 它们的下一步完全不同。
                if out.gate.live_count == 0 {
                    ctx.warn(format!(
                        "今天没有直播（{} 一间都没在播）",
                        mcloud_upstream::live_consts::KOULING_ANCHOR
                    ));
                } else {
                    ctx.warn(format!(
                        "有 {} 间在播，但没有一间是「{}」",
                        out.gate.live_count,
                        mcloud_upstream::live_consts::KOULING_ANCHOR
                    ));
                }
                return Ok(out);
            }

            // ⚠⚠ 只听第一间 —— 代价见上面的说明。
            if out.gate.rooms.len() > 1 {
                ctx.warn(format!(
                    "⚠ 该主播有 {} 间在播，本次**只听第一间**（其余 {} 间这一趟不覆盖）",
                    out.gate.rooms.len(),
                    out.gate.rooms.len() - 1
                ));
            }
            let room = out
                .gate
                .rooms
                .first()
                .cloned()
                .ok_or_else(|| Error::Other("门说开了，却没有房间".into()))?;
            ctx.info(format!(
                "在播房间 {}，听 {} 秒弹幕…",
                room.id, params.listen_seconds
            ));
            let collected = self
                .collector
                .collect(ctx, jwt, &room, params.listen_for(), &known_invalid)
                .await?;
            // ⚠ 收集到的**先进池再领**：这一趟要是中途崩了，
            // 别人至少还能直接用这几条（口令本身是有时效的，捂在手里最亏）。
            pool.note(&collected.codes, mcloud_core::KoulingSource::Auto, now_ms);
            out.codes = collected.codes;
        }

        // 没得领就结束（"没领"是**正常结果**，见 [`Self::verdict`]）。
        if out.codes.is_empty() {
            return Ok(out);
        }

        ctx.progress(1, 3, "领取小红花");
        ctx.info(format!("用 {} 个口令领取小红花…", out.codes.len()));
        let session = RedFlowerSession::new(out.codes.clone());
        let report = self.run_chain(ctx, session).await?;

        // ⚠ 重听一次：只在**有口令被判无效**时做（判据是纯函数 [`retry_plan`]）。
        //
        // ⚠⚠ 重听**需要一间能听的房间**，而房间是门那一步的产物。
        // 这一趟可能**没跑过门**（手工 `codes` 那一支：有 codes ⇒ 跳过开门，
        // 见函数头的取舍）—— 那时 `gate.rooms` 是空的 ⇒ **没法重听**。
        //
        // ⚠ 这里**刻意不去补开门**：补开门意味着"口令抄错一个"要多打两个请求，
        // 而那两个请求**还要再成功一次**才有房间 —— 它的失败路径是新的，
        // 而且不产生任何新口令（重听本来只是补救）。取舍与主路径一致：
        // **手工 codes 这一支不发门的请求**。
        //
        // 这里把它做成"如实说一句、不重听"，而**不是**报错：
        // 重听是**补救措施**，没有它第一轮的结果依然成立；而把它变成 `Err`
        // 会让"口令抄错了一个"这种小事把整趟已经拿到的花全部作废
        // （归因方向也错了：真正的问题是那条口令无效，不是"没房间"）。
        // ⚠ 顺序：**先结算、再判"要不要继续听"**。
        //
        // 判据要看的是**池里**的有效条数，而这一趟刚验出来的那几条此刻还只在
        // `report` 里 —— 所以先把它们写进池（`out.report` 也必须先赋值，
        // [`settle_pool`] 读的就是它），再回读一份新鲜的池。
        // `settle_pool` 是幂等的（同一份结论重复写没有副作用），收尾还会再走一次。
        out.report = Some(report.clone());
        settle_pool(ctx, &account_id, &out).await?;
        // ── 还不够两条有效 ⇒ **继续听**（循环）────────────────────────────
        //
        // ⚠ 这里从前是"最多再听一次"，判据是"有没有口令被判无效"。
        // 2026-10-01 用户口径把判据换成"**要的是两条有效的**"，
        // 于是它自然该是个循环：每一轮都 听 → 立刻领取验证 → 回写池 → 再看够不够。
        //
        // 三条边界，缺一条就会变成"转圈"或"假收工"：
        // 1. 每轮必须**真的推进**：`to_draw` 为空（弹幕里没有新口令）就结束，
        //    否则同一批口令会被反复听、反复试；
        // 2. `exclude` 逐轮累积（**含第一次就成功的**）—— 否则下一轮会把同一批
        //    再领一遍，而同一账号对同一条正确口令的第二次请求回 `200112`，
        //    会污染归因、看起来像"口令坏了"；
        // 3. 硬上限 [`MAX_LISTEN_ROUNDS`]：弹幕是不可信输入，没有上限就是死循环。
        let mut merged = report.clone();
        let mut tried: Vec<String> = out.codes.clone();
        let mut rounds = 0usize;
        //
        // ⚠⚠ 有效条数**本地算**，不去 store 回读 —— 这一条是被测试逼出来的真问题：
        //
        // 池写失败是**只警告不失败**的（那是刻意的：不能拿优化否定目的）。若判据
        // 靠"回读池"，那么写不进盘时就永远读到 0 ⇒ 循环一直听到硬上限为止 ✗。
        // 所以起点取**进门之前那份池快照**里的有效口令，之后每轮把这一轮新验出来的
        // 成功/已兑换加进来（用集合去重 ⇒ 同一张嘴不会数两遍）。
        let mut valid: std::collections::HashSet<String> =
            pool.valid_values().into_iter().collect();
        // ⚠ **第一轮的结论也要算** —— 它不在循环里的 `next` 上，
        // 漏掉它就会把"第一轮已经验出一条有效"当成"一条都没有"⇒ 多听一轮 ✗
        // （实测就是这么多的听了第三轮）。
        note_valid_codes(&mut valid, &report);
        while !params.skip_kouling && rounds < MAX_LISTEN_ROUNDS {
            let (again, exclude) = retry_plan(&tried, &merged.invalid_codes, valid.len());
            if !again {
                break;
            }
            rounds += 1;
            // 没有可听的房间（这一趟没跑过门：手工 codes 那一支）⇒ 如实说一句、不再听。
            let (Some(jwt), Some(room)) = (gate_jwt.as_deref(), out.gate.rooms.first().cloned())
            else {
                ctx.warn(format!(
                    "有效口令还不足 {} 条，但这一趟没有可听的房间（未跑门）⇒ 不再听",
                    mcloud_core::kouling::VALID_ENOUGH
                ));
                break;
            };
            out.retried = true;
            ctx.progress(2, 3, "继续听口令");
            ctx.info(format!(
                "有效口令还不足 {} 条（现有 {} 条），再听 {} 秒…",
                mcloud_core::kouling::VALID_ENOUGH,
                valid.len(),
                params.listen_seconds
            ));
            // 再听失败**不致命**：它是补救措施，失败时保留已有结果。
            let fresh = match self
                .collector
                .collect(ctx, jwt, &room, params.listen_for(), &exclude)
                .await
            {
                Ok(o) => o.codes,
                Err(e) => {
                    ctx.warn(format!("再听失败（保留已有结果）：{e}"));
                    break;
                }
            };
            let to_draw = pick_new_codes(&fresh, &tried, &merged.invalid_codes);
            if to_draw.is_empty() {
                ctx.info("又听了一轮，没有新口令 —— 停在这儿（再听也不会有）");
                break;
            }
            ctx.info(format!("又收到 {} 个新口令，再领一轮…", to_draw.len()));
            ctx.progress(3, 3, "用新口令领取");
            // 每一轮**沿用同一个会话**（口径："一轮任务一条 jwttoken"）。
            let next = self
                .run_chain(ctx, RedFlowerSession::new(to_draw.clone()))
                .await?;
            // 结果**合并**：先成功的不能因为后一轮而消失。
            merged = merge_reports(&merged, &next);
            // 把这一轮"验成有效"的口令记进本地计数（判据看的就是它）。
            note_valid_codes(&mut valid, &next);
            out.report = Some(merged.clone());
            out.codes.extend(to_draw.clone());
            tried.extend(to_draw);
            // 每轮都回写池：这样"够不够两条"看的是**已验证**的事实。
            settle_pool(ctx, &account_id, &out).await?;
        }
        // ⚠ 这里**不能**再 `out.report = Some(report)`：循环里已经把各轮结果合并过，
        // 用第一轮覆盖会把后面几轮的成功记录抹掉（表现是"明明领到了却报没领"）。
        // 循环没进过时 `out.report` 也已经是第一轮 —— 见上面那句赋值。
        // 收尾结算：把这次每条口令的结论写回共享池（别的账号就不必再试一遍）。
        settle_pool(ctx, &account_id, &out).await?;
        Ok(out)
    }

    /// 把六步链一口气跑完（`next` → 发 → `feed` 的循环）。
    ///
    /// # 每一步都过 `90001` 的重放
    ///
    /// 票的管理若由 [`mcloud_upstream::H5Client`] 承担，它会**透明**处理
    /// `90001`（刷新 + 重放那一条）。而 `next` 不推进状态，所以万一重放发生，
    /// 发的还是同一步、同一份账。
    ///
    /// # ⚠ 唯一会被重放的写请求是 `draw`（第④步）
    ///
    /// 重放只在 `90001 未登录` 时发生 —— 那一次的响应说明服务端**没有**受理
    /// 这条口令；而"票过期发生在服务端受理**之后**"这种情形**没有样本**，
    /// 属**待验证**（怎么测：活窗口里让一笔 `draw` 在过期票上跑，看余额有没有动）。
    async fn run_chain(
        &self,
        ctx: &TaskContext,
        mut session: RedFlowerSession,
    ) -> Result<RedFlowerReport> {
        while !session.done() {
            ctx.check_cancelled()?;
            let (step, req) = session.next()?;
            let resp = ctx
                .http
                .request(req)
                .await
                .map_err(|e| wrap_step_error(step, e))?;
            let body = mcloud_upstream::session::ensure_http_ok(&resp, step.as_str())
                .map_err(|e| wrap_step_error(step, e))?;
            // ⚠ 解析失败**中止**（唯一例外是复活花，见 `feed` 的说明）。
            session.feed(body).map_err(|e| wrap_step_error(step, e))?;
        }
        Ok(session.report().clone())
    }

    /// 🌸 **开门**：换直播侧 JWT（三步里的前两步）。
    ///
    /// ```text
    /// ① POST user-njs.yun.139.com/user/querySpecTokenV2  {"toSourceId":"017027"}
    ///                                                       ⇒ SSO token（42 字符）
    /// ② POST v.cmcc-cs.cn/cmlive/api/user/auth/cmpassLoginByCmToken
    ///        body {"cmpassToken": <SSO>}  ⇒ {"rtnCode":"0", "object":"<JWT 180 字符>"}
    /// ```
    ///
    /// （第 ③ 步是 [`Self::gate_rooms`]，它要拿 ② 换到的 JWT 才能打。）
    ///
    /// # ⚠⚠ 这张票**只**给直播侧用（`/cmlive` 与弹幕 WS）
    ///
    /// 它与六步链要的**云盘侧** `jwttoken` **不是同一个东西**，两者绝不能互换：
    ///
    /// | | 直播侧 JWT（本函数换的） | 云盘侧 `jwttoken`（[`SessionRecipe::SignInPage`] 换的） |
    /// |---|---|---|
    /// | 谁换的 | `querySpecTokenV2(017027)` → `cmpassLoginByCmToken` | `tyrzLogin(sign_in_3)` |
    /// | 长什么样 | 独立 JWT，payload 含 `userId`（实测 **180 字符**） | `pc:` / `mobile:` 前缀那一串 |
    /// | 挂在哪个头 | **`Authorization`**（[`live::CmliveEnvelope`] 那一族） | **`jwttoken`**（[`mcloud_upstream::session::h5_headers`]） |
    /// | 给谁用 | `/cmlive` 房间发现 + 弹幕 WS | `/ycloud` 小红花六步链 |
    /// | 用错的后果 | —— | 实测回 **`90001 未登录`**（2026-09-22，两个账号都是那一句） |
    ///
    /// ⚠ 这两句的具体长相**不是**靠"看着像"区分的，是靠**域**区分的：
    /// [`live::cmlive_headers`] 只往 `Authorization` 上写，
    /// 而 H5 那一族只往 `jwttoken` 上写。本函数**只**返回 JWT，
    /// 绝不去碰六步链那一边 —— 那一边的票由 `H5Client` 透明注入
    /// （见 [`Self::run_account`] 里那段说明）。
    ///
    /// # ⚠ 失败**如实上抛**（带步名前缀），不照抄参考实现那条"失败即放行"
    ///
    /// 参考实现早先在 SSO / 登录失败时 `return true` 放行，它自己后来明确否定了：
    /// 那条让"凭据坏了"表现成"门开着、却一条口令也听不到"，
    /// 真因被盖住，排查方向会从"今天是不是没开播"开始 —— **完全错**。
    ///
    /// ⇒ 这里把失败原样返回，要不要继续由**调用方**决定 ——
    /// 想"不发请求先看看参数怎么拆"用 `dryRun`（它是纯本地的，见 `run`）。
    ///
    /// # 证据等级
    ///
    /// - **实证**：①②两步 + 第③步（`tabContentList`）—— 出处
    ///   `../mcloud/docs/capture/app-live-redflower-samples.txt` §5.3（第 345–365 行）
    ///   与证据等级表（第 466–475 行）。那一轮用临时只读探针主动走了一遍，
    ///   因为门第③步是 **GET、不需要开播**。
    /// - **本条实证的两处细节**：`querySpecTokenV2(017027)` 能换到 SSO
    ///   ⇒ 库里那份 `authorization` 在直播族上也认；
    ///   `cmpassLoginByCmToken` 的 `object` **就是 JWT 本身**（180 字符），不是包着它的对象。
    /// - ⏸ **仍结构性抓不到**：WS / 弹幕 / 口令（未开播，`chatRoomId: null`）。
    ///   那一段在 [`LiveKoulingCollector`] 里，**与本函数分开标**。
    pub async fn open_gate(&self, ctx: &TaskContext) -> Result<String> {
        // ① 换 SSO token。用**直播侧**的 `toSourceId`（`017027`）——
        // 不是签到的 `001005`。混用的表现是"一条房间都筛不出来"，
        // 而那看起来像"今天没有直播"：一个**假结论**（参考实现坑表 #106）。
        let authorization = mcloud_upstream::basic_authorization(&ctx.account);
        let resp = ctx
            .http
            .request(mcloud_upstream::session::live_gate_spec_token_request(
                &authorization,
            ))
            .await
            .map_err(|e| Error::Other(format!("[门·换 SSO token] {e}")))?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "门·换 SSO token")
            .map_err(|e| Error::Other(format!("[门·换 SSO token] {e}")))?;
        let sso = mcloud_upstream::session::parse_sso_token(body)
            .map_err(|e| Error::Other(format!("[门·换 SSO token] {e}")))?;

        // ② 用 SSO 换**直播侧** JWT。
        let resp = ctx
            .http
            .request(live::cmpass_login_request(&sso)?)
            .await
            .map_err(|e| Error::Other(format!("[门·换直播侧 JWT] {e}")))?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "门·换直播侧 JWT")
            .map_err(|e| Error::Other(format!("[门·换直播侧 JWT] {e}")))?;
        let env = live::CmliveEnvelope::parse(body)
            .map_err(|e| Error::Other(format!("[门·换直播侧 JWT] {e}")))?;
        live::parse_cmpass_login(&env).map_err(|e| Error::Other(format!("[门·换直播侧 JWT] {e}")))
    }

    /// 拉两个页签的房间（门的第一步）。
    ///
    /// ⚠ 只查 `127` 会在"房间还没进福利页签"时判成**没开播**，而那正是
    /// "漏掉整场口令"这一侧的错误 —— 代价远大于多打一个 GET。
    async fn gate_rooms(&self, ctx: &TaskContext, jwt: &str) -> Result<Vec<live::LiveRoom>> {
        let tab_ids = vec![
            mcloud_upstream::live_consts::LIVE_TAB_ID_BENEFITS,
            mcloud_upstream::live_consts::LIVE_TAB_ID_REPLAY,
        ];
        let mut groups: Vec<Vec<live::LiveRoom>> = Vec::new();
        for tab_id in tab_ids {
            let what = format!("直播·页签 {tab_id}");
            let resp = ctx
                .http
                .request(live::tab_content_list_request(tab_id, jwt))
                .await?;
            let body = mcloud_upstream::session::ensure_http_ok(&resp, &what)?;
            groups.push(live::parse_tab_content_list(&live::CmliveEnvelope::parse(
                body,
            )?)?);
        }
        Ok(live::merge_rooms(&groups))
    }
}

// ══════════════════════════ 任务 ══════════════════════════

/// `live-room`：口令 → 小红花六步链。
///
/// 参数见 [`Task::params`]（`codes` / `listenSeconds` / `dryRun` 等）。
///
/// # ⚠ 它与 `red-flower` 的分工
///
/// 两者是**同一条链**的两个入口：`live-room` 走完整的"开门 → 获取口令 → 领取"
/// （含听弹幕），`red-flower` 是"我手上已经有口令、直接领"那一支。
/// 分开登记的理由与签到族一样：日常要跑的是前者，而后者在调试/补领时更方便。
pub struct LiveRoomTask {
    /// 收集器（生产 = [`LiveKoulingCollector`]）。
    pub collector: Arc<dyn KoulingCollector>,
}

impl Default for LiveRoomTask {
    fn default() -> Self {
        Self {
            collector: Arc::new(LiveKoulingCollector::production()),
        }
    }
}

impl LiveRoomTask {
    /// 生产实现。
    pub fn production() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Task for LiveRoomTask {
    fn name(&self) -> &'static str {
        "live-room"
    }

    fn title(&self) -> &'static str {
        "直播口令小红花"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Live
    }

    fn description(&self) -> &'static str {
        "听直播弹幕获取口令，并逐个领取小红花（小红书那条口令需人工用 codes 传入）"
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::text(
                "codes",
                "口令",
                "小红书那条口令只能人工粘进来；多个用逗号或换行分隔",
            ),
            ParamSpec::number(
                "listenSeconds",
                "听弹幕时长",
                60.0,
                5.0,
                1800.0,
                "秒",
                "听多久弹幕来抓口令（参考实现的默认是 60 秒）",
            ),
            ParamSpec::boolean("skipKouling", "跳过听弹幕", false, "直接用上面填的口令去领"),
            ParamSpec::boolean("dryRun", "试运行", false, "只查询、不真的领取"),
        ]
    }

    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        // ⚠ 开关是**最外层**的门（旧 TS 在 `cli/index.ts` 用
        // `if (config.直播口令?.开启)` 把整条调用包起来）：关 ⇒ 一个请求都不发，
        // 结论是 `Skipped` 而**不是** `Failed`。默认 `false` ⇒ **默认就是跳过**，
        // 与旧 TS 的默认一致。
        //
        // ⚠ 它刻意放在参数校验**之前**：开关关着就不该对"参数怎么填"发表意见
        // —— 那正是"任务根本没被调用"的等价物。
        if !ctx.account.settings.kouling_enabled {
            return Ok(TaskOutcome::skipped(
                "直播口令未启用（账号配置「直播口令·开启」为关）",
            ));
        }

        let params = LiveRoomRun::params_from_body(body);
        let dry_run = body.get("dryRun").and_then(Value::as_bool).unwrap_or(false);

        // ⚠ 参数校验：判据在 [`LiveRoomParams::check`]（只有一份），
        // 而**措辞在这里** —— 领域层里那句"互斥"没有 flag 名，
        // 直接输出给用户会少掉"哪个 flag 该去掉"这个关键信息。
        if let Err(fault) = params.check() {
            let wording = match fault {
                LiveRoomFault::SkipWithoutCodes => {
                    "skipKouling 关掉了唯一的口令来源，必须同时给 codes"
                }
                LiveRoomFault::ListenNotPositive => "listenSeconds 必须是正数",
            };
            return Err(Error::Config(format!("{wording}（{}）", fault.describe())));
        }

        if dry_run {
            ctx.info("演练模式：只校验参数与口令拆分，不发任何请求");
            return Ok(TaskOutcome::success(format!(
                "演练完成：识别出 {} 个口令（{}）",
                params.codes.len(),
                if params.codes.is_empty() {
                    "未提供 codes".to_string()
                } else {
                    params.codes.join("、")
                }
            )));
        }

        // 🌸 门在这里接上了：`LiveRoomRun::run_account` 会在**需要收口令**时
        // 自己调 `open_gate()` 换直播侧 JWT（三步：换 SSO → 换 JWT → 拉两个页签）。
        //
        // ⚠ 这里**不再**写死 `live_jwt: None`。那处写死（历史上在 1406 / 1511 行）
        // 让门那一段**永远进不去** ⇒ 没配 codes 时收不到任何口令，
        // 而结论显示"今天没有直播" —— 一个**假结论**，且查不出真因。
        let run = LiveRoomRun {
            collector: self.collector.as_ref(),
        };
        let result = run.run_account(ctx, &params).await?;

        if !result.codes.is_empty() {
            for code in &result.codes {
                ctx.info(format!("口令：{code}"));
            }
        }
        if let Some(report) = &result.report {
            ctx.info(report.summary_line());
        }
        Ok(result.outcome())
    }
}

/// `red-flower`：**直接领取**（不获取口令）。
///
/// # 它与 `live-room` 的关系
///
/// 同一条六步链，两个入口。这一条不碰门、不开弹幕 —— 它假设调用方
/// **已经有口令**（`codes` 在这里是事实上的必填）。
///
/// ⚠ 产品事实（模块头事实 1）：小红书那条口令**只能**从 `codes` 进来。
/// 所以"只跑 red-flower"永远拿不到完整的两条 —— 那是这一支的固有形状，
/// 不是缺陷。
pub struct RedFlowerTask {
    /// 收集器（供重听用）。
    pub collector: Arc<dyn KoulingCollector>,
}

impl Default for RedFlowerTask {
    fn default() -> Self {
        Self {
            collector: Arc::new(LiveKoulingCollector::production()),
        }
    }
}

impl RedFlowerTask {
    /// 生产实现。
    pub fn production() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Task for RedFlowerTask {
    fn name(&self) -> &'static str {
        "red-flower"
    }

    fn title(&self) -> &'static str {
        "领取小红花"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Live
    }

    fn description(&self) -> &'static str {
        "用给定的口令逐个领取小红花（不听弹幕；给的第一个口令决定先领哪条）"
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::text("codes", "口令", "要领取的口令；多个用逗号或换行分隔"),
            ParamSpec::boolean("dryRun", "试运行", false, "只查询、不真的领取"),
        ]
    }

    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        let codes = match body.get("codes") {
            Some(Value::String(raw)) => split_kouling(raw),
            Some(Value::Array(items)) => {
                let list: Vec<String> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                split_kouling(&list.join("\n"))
            }
            _ => Vec::new(),
        };

        if codes.is_empty() {
            // 这一支**没有**口令来源可言（它不收集），所以空 codes 是配置错误。
            return Err(Error::Config(
                "red-flower 需要 codes：它不收集弹幕，没有别的口令来源".into(),
            ));
        }

        let dry_run = body.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
        if dry_run {
            return Ok(TaskOutcome::success(format!(
                "演练完成：将用 {} 个口令领取小红花（{}）",
                codes.len(),
                codes.join("、")
            )));
        }

        // ⚠ 这一支**不需要门**（不收集）：`skip_kouling = true` 且 codes 非空
        // ⇒ `run_account` 里"需要收口令"那个条件不成立 ⇒ **一个门请求都不发**。
        // 这正是 [`RedFlowerTask`] 的固有形状：口令已经有了，直接领。
        let params = LiveRoomParams {
            codes,
            // 它不收集，时长只需是个正数即可过校验。
            listen_seconds: 1,
            skip_kouling: true,
        };
        let run = LiveRoomRun {
            collector: self.collector.as_ref(),
        };
        let result = run.run_account(ctx, &params).await?;
        if let Some(report) = &result.report {
            ctx.info(report.summary_line());
        }
        Ok(result.outcome())
    }
}

#[cfg(test)]
mod tests;
