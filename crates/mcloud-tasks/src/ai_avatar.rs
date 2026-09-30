//! AI 头像（`ai-avatar`，界面那句是「AI 新头像」）—— **任务层编排**。
//!
//! 协议层（[`mcloud_upstream::ai_avatar`]）只负责造请求 / 解响应，它不认识
//! 「该不该发」。本模块补上那一步：按开关与每日轮数生成、按上游剩余次数抽奖、
//! 最后**只收走本趟自己造出来的**网盘文件。
//!
//! 依据只有旧 TS（优先级 ①，那份**能跑**）：
//! `packages/core/src/tasks/ai-avatar/{task.ts,api.ts}`。每处行为都在注释里标了
//! 函数名与行号，便于对拍。
//!
//! # ⚠ 这条链今天可能整条打不通（线索，不是结论）
//!
//! 参考实现 2026-09-25 真账号实测：`getHashLeft` / `draw` 回 **404**、
//! `merge` 回 **405**，两个账号一字不差（协议层模块头有逐字读数）。
//! 「活动下线」与「路径/方法拼错」今天**同形**。
//!
//! ⇒ 本模块**照 TS 发**，但运行期遇到**非 200 一律如实上抛**
//! （[`is_transport`] 那一档直接 `?` 出去，带 HTTP 码与响应体），
//! **绝不**静默当成成功 —— 否则那三行真读数会被洗成一句「跑完了」。
//!
//! # 开关与轮数
//!
//! `ctx.account.settings.ai_avatar_enabled`（默认 **false**，`account.rs:184`）为假
//! ⇒ 直接 [`TaskOutcome::skipped`]，**一枪都不发**。
//! `ai_avatar_daily_limit`（默认 10）语义 = **本地生成轮数**，**不是**上游余量：
//! 旧 TS 的生成侧压根不看上游计数（`task.ts:80-83` 那个 `每日生成次数` 只是
//! 循环次数），上游真实余量只在抽奖侧由 `getHashLeft` 读。这一点与
//! `account.rs:161` 的默认值 10 一致，别把两者混成一个额度。
//!
//! # 不幂等：这是旧实现的事实，必须说实话
//!
//! 旧 TS **没有**「已完成 / 已领奖」这类判据（连一个 last-run 标记都没有）：
//! 第二次进链照样 `merge`（最多 count 次）、照样按 `getHashLeft` 抽。
//! ⇒ 本模块**不发明**一个「今天做过了」的开关（那是凭空造一个上游没有的语义），
//! 而是把「这一趟做了什么」如实写进小结与明细：
//! 生成了几轮、抽了几次、清理了几个。那句话是常量
//! [`AI_AVATAR_NON_IDEMPOTENT_HEDGE`]，**小结与明细都带**，免得它被读成
//! 「跑过就不会再跑」。
//!
//! # ⚠ 收尾护栏：**刻意不照抄**旧 TS 的「按文件名匹配收 fileId」
//!
//! 旧 TS（`task.ts:12-36`）是拿作品图的名字去网盘目录里**按名字**匹配 `fileId`，
//! 再把匹配到的全部送进回收站。**同名碰撞即误删**：活动目录里本来就有一张与
//! 本趟生成图同名的文件，它会被一起删掉，而 TS 没有任何 pre-existing 判据
//! （spec §5 把这条列为重点风险）。
//!
//! ⇒ 本模块的处置是：**先取一份基线快照**（本趟动笔之前那次列目录的 `fileId` 集合），
//! 收尾时只把「名字对得上 **且** 基线里没有」的那些登记成**本趟产物**
//! （[`TaskContext::register_cloud_file`]，`pre_existing` 就是「在不在基线里」），
//! 然后只清账本上这一批。同名旧文件在基线里 ⇒ `pre_existing=true` ⇒
//! **既不进账本、也不会被删**。
//!
//! 代价是比旧 TS 多两枪**只读**请求（活动目录 + 一次列目录），而且
//! `ai_avatar_daily_limit == 0` 时**不取**（本趟不会写东西，没有要归属的对象）。
//! 基线拿不到时**跳过收尾**并如实说明：**宁可留下，也不误删**。
//!
//! ⚠ 这不是洁癖：`crates/mcloud-engine/src/artifacts.rs` 把「错记」列为比
//! 「漏记」严重得多的一类失效 —— 漏记是文件留下来，错记是**删用户的东西**。
//!
//! # ⚠ 生成链的「下载合成图」那一腿：已接上 `request_raw`
//!
//! 旧 TS 的 `generateOnce` 是 `merge → 轮询 getStatus → 裸 fetch 下载生成图`
//! `→ upload(原图, 合成图)`（`task.ts:48-76`）。第四条腿要求把**图片字节**
//! 读回来做 base64，而本仓的 [`mcloud_core::HttpResponse::body`] 是 `String`、
//! 发送层原先走 `reqwest::Response::text()`（**有损** UTF-8）⇒ 图片内容会被
//! 破坏且**不报错**。现在下载那一枪改走
//! [`mcloud_core::HttpClient::request_raw`]（真字节）：**有损解码那条坑已经绕开**，
//! base64 之后交给 `upload`。
//!
//! 判据随之**重新表述**（旧判据作废，别再照抄）：不再看 `content-type`（上游
//! 随手给，不说明内容对不对）、也不找 `U+FFFD`（那只是"字节已被毁"的痕迹），
//! 只做**形状校验** —— 非空 + 头部是 JPEG(`FF D8`) 或 PNG(`89 50`)
//! （见 [`image_download_base64`]）。形状对不上就如实判失败，**绝不**把来路不明的
//! 字节 base64 上去：那会写成一张打不开的图，而上游仍可能回 `code:0`。
//!
//! ⚠⚠ **真机路径上还差一跳（不是本模块能补的）**：业务客户端是
//! `SmidClient → H5Client → ReqwestClient` —— 两层装饰器套一个 leaf，而
//! **包着另一个 `HttpClient` 的装饰器必须把 `request_raw` 转发给它的 transport**，
//! 否则默认实现会落到内层的 `request()`（有损文本）上。这两个装饰器今天**都还
//! 没有**转发（在本次改动的允许范围之外）。⇒ 真机上这一腿会由形状校验**判失败**
//! （不会静默写坏图，这是本仓要防的方向）；本模块这一侧的形状是对的，替身
//! `request_raw` 的测试能验通「任务层这一腿」。补齐见
//! [`mcloud_core::HttpClient::request_raw`] 的说明。
//!
//! # 等待做成字段
//!
//! 旧 TS 有三处 sleep：轮询间隔 3s（`task.ts:10`，最多 8 次）、轮间 2s
//! （`task.ts:102`）、抽奖间隔 1s（`task.ts:135`）。它们全装进 [`AiAvatarPlan`]，
//! 测试设 0 ⇒ 真 sleep 只出现在生产路径（与 `app_task` / `cloud_phone_redpack`
//! 同一套做法）。
//!
//! # 随机量是**注入**的
//!
//! 原图是一张 200×200 的随机噪点 PNG（旧 TS `utils/png-generator.ts`），
//! 随机源走 [`mcloud_crypto::Random`]（生产 [`mcloud_crypto::OsRandom`]，
//! 测试 [`mcloud_crypto::SeqRandom`]），**不碰** `SystemTime::now()` 之类的全局源 ——
//! 那样 fixture 回放就无从谈起。

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use mcloud_core::{Error, HttpRequest, Result, TaskOutcome};
use mcloud_crypto::{Random, SharedRandom};
use mcloud_engine::{Task, TaskContext, TaskResult};
use mcloud_upstream::ai_avatar as proto;
use mcloud_upstream::cloud_trash;

/// 任务名（注册表、测试、`/api/capabilities` 都认它）。
pub const AI_AVATAR_TASK_NAME: &str = "ai-avatar";
/// 任务标题。
pub const AI_AVATAR_TASK_TITLE: &str = "AI 头像";
/// 任务说明。
pub const AI_AVATAR_TASK_DESCRIPTION: &str =
    "移动云盘 AI 新头像：生成噪点图换合成任务、按上游剩余次数抽奖，并只收走本趟自己写进网盘的活动文件";

/// 抽奖次数读不到时的兜底值（旧 TS `task.ts:8` 的 `MAX_DRAW_FALLBACK`）。
///
/// ⚠ 它是**一条猜测**而不是判据：旧 TS 在 `getHashLeft` 失败/非数时就用它去抽 5 次。
/// 本模块照抄（行为以 TS 为准），但会把它写进小结与明细、**不静默**。
pub const AI_AVATAR_DRAW_FALLBACK: i64 = 5;

/// 原图边长（旧 TS `task.ts:39` 的 `generateRandomNoisePNG(200, 200)`）。
pub const AI_AVATAR_IMAGE_SIDE: u32 = 200;

/// 轮询 `getStatus` 的最多次数（旧 TS `task.ts:9` 的 `MAX_POLL_COUNT`）。
pub const AI_AVATAR_POLL_MAX: u32 = 8;
/// 轮询间隔（旧 TS `task.ts:10` 的 `POLL_INTERVAL`）。
pub const AI_AVATAR_POLL_INTERVAL_MS: i64 = 3_000;
/// 每轮生成之间的等待（旧 TS `task.ts:102` 的 `sleep(2000)`）。
pub const AI_AVATAR_ROUND_INTERVAL_MS: i64 = 2_000;
/// 每次抽奖之间的等待（旧 TS `task.ts:135` 的 `sleep(1000)`）。
pub const AI_AVATAR_DRAW_INTERVAL_MS: i64 = 1_000;
/// 每一轮最多试几次（旧 TS `task.ts:88` 的 `attempt <= 2`）。
pub const AI_AVATAR_MAX_ATTEMPTS: u32 = 2;

/// 「不幂等」那句话 —— **只有一处**：小结、明细与测试都引它。
///
/// 抄成两处就早晚只改一处，而这句话是这一条链对外最要紧的一句实话。
pub const AI_AVATAR_NON_IDEMPOTENT_HEDGE: &str =
    "本链没有「已完成 / 已领奖」判据（旧实现也没有），重复跑会重复合成与抽奖";

/// 这条链的三处等待 + 三条上限，测试把它设成 0 / 1 以便离线复现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiAvatarPlan {
    /// 轮询 `getStatus` 的最多次数。
    pub poll_max: u32,
    /// 每次轮询之前等多久。
    pub poll_interval_ms: i64,
    /// 每轮生成之间等多久（**包括失败那一轮之后**，旧 TS 就是这么写的）。
    pub round_interval_ms: i64,
    /// 每次**成功**抽奖之后等多久（旧实现里失败那一支 `continue` 会跳过这一等）。
    pub draw_interval_ms: i64,
    /// 每一轮最多试几次。
    pub max_attempts: u32,
    /// 原图边长。
    pub image_side: u32,
}

impl Default for AiAvatarPlan {
    /// 生产形态：与旧 TS 的那几个常量逐字一致。
    fn default() -> Self {
        Self {
            poll_max: AI_AVATAR_POLL_MAX,
            poll_interval_ms: AI_AVATAR_POLL_INTERVAL_MS,
            round_interval_ms: AI_AVATAR_ROUND_INTERVAL_MS,
            draw_interval_ms: AI_AVATAR_DRAW_INTERVAL_MS,
            max_attempts: AI_AVATAR_MAX_ATTEMPTS,
            image_side: AI_AVATAR_IMAGE_SIDE,
        }
    }
}

// ══════════════════════════ 原图：随机噪点 PNG ══════════════════════════

/// 造一张 `width × height` 的随机噪点 PNG（旧 TS `utils/png-generator.ts`）。
///
/// 旧实现的三处形状**逐字复刻**（改一处都不报错，只是图变了或解不开）：
/// 8-bit **RGB**（colortype 2）、每行 filter 0、deflate **存储模式**（不压缩）、
/// 像素三字节全随机。
pub fn noise_png(width: u32, height: u32, rng: &mut dyn Random) -> Vec<u8> {
    let row_len = 1 + width as usize * 3;
    let mut raw = vec![0u8; height as usize * row_len];
    for row in raw.chunks_exact_mut(row_len) {
        row[0] = 0; // filter: None
        rng.fill_bytes(&mut row[1..]);
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 位深 8、色型 2（Truecolor RGB）、压缩 0、过滤 0、隔行 0。
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&raw)));
    out.extend_from_slice(&png_chunk(b"IEND", &[]));
    out
}

/// 同 [`noise_png`]，但直接给 `merge` 要的**裸 base64**。
///
/// ⚠ 旧 TS 的生成器返回的是带 `data:image/png;base64,` 前缀的串，调用方
/// 再用正则把那一段去掉（`task.ts:38-40`）。本函数**一次都不拼那个前缀**
/// —— 拼了再剥是同一件事，而多一次字符串往返就多一处能改错的地方。
pub fn noise_png_b64(width: u32, height: u32, rng: &mut dyn Random) -> String {
    mcloud_crypto::b64_encode(&noise_png(width, height, rng))
}

/// 一个 PNG 块：长度 + 类型 + 载荷 + CRC32。
fn png_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 12);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// zlib 容器 + stored（未压缩）DEFLATE 块 + Adler-32。
///
/// ⚠ 旧实现的注释把这一手写成「gzip 压缩」，其实 `deflateSync` 交的就是
/// **zlib**（`0x78 0x01` + deflate + adler32），而 PNG 要的正是 zlib。
/// 本函数照**实际字节**实现，不照那句注释。
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut zlib = vec![0x78, 0x01];
    let mut remaining = raw;
    while !remaining.is_empty() {
        let take = remaining.len().min(65_535);
        let last = take == remaining.len();
        zlib.push(if last { 1 } else { 0 });
        zlib.extend_from_slice(&(take as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
        zlib.extend_from_slice(&remaining[..take]);
        remaining = &remaining[take..];
    }
    let mut adler_a: u32 = 1;
    let mut adler_b: u32 = 0;
    for &byte in raw {
        adler_a = (adler_a + u32::from(byte)) % 65521;
        adler_b = (adler_b + adler_a) % 65521;
    }
    zlib.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());
    zlib
}

/// PNG 的 CRC32（IEEE，逐位算法）。
///
/// 逐位而不是查表：这一族一张图只有三个块、大块也只是线性扫一遍，
/// 而查表要把 256 个常量摆进源文件（或引一次 `OnceLock` 初始化）——
/// 为一张 200×200 的噪点图不值得。
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

// ══════════════════════════ 结论 ══════════════════════════

/// 一个账号这一趟的产出 —— **如实记账**，不做「看起来成功」的洗白。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiAvatarResult {
    /// 本趟**计划**生成几轮（= `ai_avatar_daily_limit`）。
    pub planned_generate: u32,
    /// 真的拿到 `upload` id 的轮数。
    pub generated: u32,
    /// 失败的轮数（每一轮最多试 [`AiAvatarPlan::max_attempts`] 次）。
    pub generate_failures: u32,
    /// `getHashLeft` 的读数。`None` = 上游没给数字（**不是 0**）。
    pub draw_left: Option<i64>,
    /// 是否用了兜底值 [`AI_AVATAR_DRAW_FALLBACK`]。
    pub draw_fallback_used: bool,
    /// 真发了几枪 `draw`（含失败的那几枪 —— 「抽了几次」就是它）。
    pub draw_attempted: u32,
    /// 其中 `code == 0` 的次数。
    pub draw_won: u32,
    /// 上游回了 408（[`proto::AI_AVATAR_DRAW_CODE_STOPPED`]）。
    pub draw_stopped: bool,
    /// 抽到的奖品名（按旧 TS 的优先序取 `contentName || prizeName`）。
    pub prizes: Vec<String>,
    /// 登记进账本的**本趟产物** fileId。
    pub registered_file_ids: Vec<String>,
    /// 名字对得上、但在基线里 ⇒ 判为**用户原有**、不登记也不删的条数。
    pub pre_existing_skipped: usize,
    /// 确认清掉的条数。
    pub cleaned_up: usize,
    /// 收尾失败的原话（空串 = 没失败）。
    pub cleanup_error: String,
    /// 逐条失败。
    pub failures: Vec<String>,
    /// 逐条**要人看见**的说明（兜底、跳过、缺口）。
    pub notes: Vec<String>,
}

impl AiAvatarResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// 三样必须写清：生成了几轮 / 抽了几次 / 清理了几个；
    /// 外加那句 [`AI_AVATAR_NON_IDEMPOTENT_HEDGE`]。
    pub fn summary_line(&self) -> String {
        const HEAD: &str = "AI 头像";
        let mut s = format!(
            "{HEAD}：生成 {}/{} 轮（失败 {}）；抽奖 {} 次（中 {}）",
            self.generated,
            self.planned_generate,
            self.generate_failures,
            self.draw_attempted,
            self.draw_won
        );
        s.push('；');
        s.push_str(&self.draw_left_line());
        s.push_str(&format!("；清理 {} 个", self.cleaned_up));
        if self.pre_existing_skipped > 0 {
            s.push_str(&format!(
                "（同名旧文件跳过 {} 个，不删）",
                self.pre_existing_skipped
            ));
        }
        if self.draw_stopped {
            s.push_str("；抽奖被上游叫停（408）");
        }
        if !self.cleanup_error.is_empty() {
            s.push_str(&format!("；收尾失败：{}", self.cleanup_error));
        }
        if !self.failures.is_empty() {
            s.push_str(&format!("；{} 处失败", self.failures.len()));
        }
        s.push_str(&format!("；{}", AI_AVATAR_NON_IDEMPOTENT_HEDGE));
        s
    }

    /// 念「剩余次数」那一格。
    ///
    /// ⚠ `None` 与 `Some(0)` 分开：`None` 是「上游没给数字」⇒ 用了兜底，
    /// 而兜底是一件**要人看见**的事（它意味着这一趟的抽奖次数是猜的）。
    fn draw_left_line(&self) -> String {
        match self.draw_left {
            Some(0) => "剩余次数读到 0 ⇒ 一枪都没抽".to_string(),
            Some(n) => format!("剩余次数读到 {n}"),
            None if self.draw_fallback_used => format!(
                "剩余次数没读到 ⇒ 按旧实现用兜底 {AI_AVATAR_DRAW_FALLBACK} 次（这是猜测，不是读数）"
            ),
            None => "剩余次数没读到".to_string(),
        }
    }

    /// 映射到 [`TaskOutcome`]。
    ///
    /// # 三档的判据
    ///
    /// * **一条都没成**：计划生成 > 0、生成 0 轮、抽奖也一枪没发 ⇒
    ///   [`TaskOutcome::failed`]。这正对「活动疑似下线」那一档的现场 ——
    ///   它**不能**被念成 success（那会把 404/405 洗成一句「跑完了」）。
    /// * **什么都没做**（计划 0 轮、没抽、**上游也没叫停**、没失败）⇒ skipped：
    ///   开关开着但这趟无活可干。
    ///   ⚠ 408 那一档**不算**「什么都没做」：那是上游明确回了一句话，
    ///   把它念成 skipped 会把一次真实交互吞掉（小结里那句「被上游叫停」就没人看见）。
    /// * 其余 ⇒ success（生成/抽奖/收尾的逐条失败已经在小结里）。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        if self.planned_generate > 0 && self.generated == 0 && self.draw_attempted == 0 {
            return TaskOutcome::failed(summary);
        }
        if self.planned_generate == 0
            && self.draw_attempted == 0
            && !self.draw_stopped
            && self.failures.is_empty()
            && self.cleanup_error.is_empty()
        {
            return TaskOutcome::skipped(summary);
        }
        TaskOutcome::success(summary)
    }
}

// ══════════════════════════ 执行层 ══════════════════════════

/// 上游 **HTTP 层**的失败（非 200、连不上、TLS）—— **一律上抛**，不折成本轮失败。
///
/// # 为什么这条判据值得单独一个函数
///
/// 旧 TS 的每一枪都包在 `try/catch` 里，任何失败都只是 `debug` 一句就往下走。
/// 那在「活动还在、只是这一轮业务失败」时是对的；但 2026-09-25 那三行真读数
/// （404/405）也会被它吞掉 ⇒ 界面上是一句「跑完 0 轮」，看着像正常跑完。
/// 本仓把这两类**分开**：HTTP/传输层失败上抛（带码与体），业务码失败才记本轮。
fn is_transport(e: &Error) -> bool {
    matches!(e, Error::Network(_))
}

/// 睡一会儿；`ms <= 0` 直接返回（测试就是靠这一条把等待关掉的）。
async fn sleep_ms(ms: i64) {
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms as u64)).await;
    }
}

/// 发一个请求，把**原始响应体**交给协议层解析（第八口 `hcy/file/list` 走这条：
/// 它的信封不是 `{code:数字,result}`，不能过 [`SignInEnvelope`]）。
///
/// ⚠ HTTP 状态这一步**任何一口都不能省**：非 200 在这里就带码与体上抛
/// （[`mcloud_upstream::session::ensure_http_ok`]）。
async fn call_raw<T>(
    ctx: &TaskContext,
    req: HttpRequest,
    what: &str,
    parse: fn(&str) -> Result<T>,
) -> Result<T> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    parse(body)
}

/// 发一个 `rabactive/*` 请求：先判 HTTP 状态，再解信封，再交给协议层解析。
///
/// ⚠ 两步判据**不能省任何一步**：
/// * [`mcloud_upstream::session::ensure_http_ok`] 管「这一枪打没打上」（非 200 带码与体上抛）；
/// * [`SignInEnvelope::ensure_ok`] 管「上游认不认」（业务码，带 code/message）。
async fn call_env<T>(
    ctx: &TaskContext,
    req: HttpRequest,
    what: &str,
    parse: fn(&mcloud_upstream::app_signin::SignInEnvelope) -> Result<T>,
) -> Result<T> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    let env = mcloud_upstream::app_signin::SignInEnvelope::parse(body)?;
    parse(&env)
}

/// 下载回来的合成图字节 → `upload` 要的 base64（**形状校验在这里**）。
///
/// # 判据为什么只剩「非空 + 魔数」这两条
///
/// 旧判据（`content-type: image/*` 或体里出现 `U+FFFD`）是**只能拿到 `String`**
/// 时的替身：它的前提是"字节已经被有损解码毁了"，所以只能找被毁的痕迹。
/// 现在 [`mcloud_core::HttpClient::request_raw`] 交回的是**线上原样字节**
/// —— **有损解码那条坑已经绕开**，痕迹判据随之作废：
///
/// * `content-type` 是上游随手给的，与内容对不对无关（真图配错头、非图配
///   `image/*` 都发生过）；拿它当判据是拿"上游的声明"当"内容的证据"。
/// * `U+FFFD` 在真字节里出现只是巧合，不再说明任何事。
///
/// ⇒ 只做**形状校验**：非空 + 头部魔数对得上（JPEG `FF D8` / PNG `89 50`）。
/// 判不过就如实报错 —— 把来路不明的字节 base64 上去，上游仍可能回 `code:0`，
/// 结果是网盘里多一张打不开的图，而**没有任何一步会报错**。
fn image_download_base64(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() {
        return Err(Error::Other(
            "合成图那一枪回了空体：没有可上传的字节".into(),
        ));
    }
    let is_jpeg = bytes.starts_with(&[0xFF, 0xD8]);
    let is_png = bytes.starts_with(&[0x89, 0x50]);
    if !is_jpeg && !is_png {
        return Err(Error::Other(format!(
            "合成图那一枪回了 {} 字节，但头部既不是 JPEG(FF D8) 也不是 PNG(89 50) —— 形状对不上，不当成图上传",
            bytes.len()
        )));
    }
    Ok(mcloud_crypto::b64_encode(bytes))
}

/// 跑一轮生成：`merge → 轮询 getStatus →（下载）→ upload`。
///
/// 返回 `Ok(Some(id))` = 这一轮成了；`Ok(None)` = **业务上**判为失败
/// （没给 `taskId` / `status == 4` / 轮询用尽）；`Err(e)` = 这一枪本身有问题，
/// 由上层的 [`is_transport`] 决定「上抛」还是「换下一次」。
///
/// ⚠ 「下载生成图」那一枪走 [`mcloud_core::HttpClient::request_raw`]（真字节），
/// **不是** `request()`：后者经 `HttpResponse.body`（`String` / 有损 UTF-8）
/// 会把图片内容改掉且不报错。判据只有形状校验（见 [`image_download_base64`]）。
async fn generate_once(
    ctx: &TaskContext,
    plan: &AiAvatarPlan,
    rng: &SharedRandom,
) -> Result<Option<String>> {
    let origin = rng.with(|r| noise_png_b64(plan.image_side, plan.image_side, r));

    let merged = call_env(
        ctx,
        proto::merge_request(&origin),
        "提交 AI 头像合成",
        proto::parse_task_status,
    )
    .await?;
    let Some(task_id) = merged.task_id else {
        // 旧 TS `task.ts:52`：`code !== 0 || !taskId` ⇒ 本轮失败。
        // `code != 0` 已经在 call_env 那一层变成 Err 了，这里只剩「没给 id」。
        return Ok(None);
    };

    // 旧 TS `task.ts:57-73`：**先等、再查**（sleep 在循环体开头），最多 8 次。
    for _ in 0..plan.poll_max {
        ctx.check_cancelled()?;
        sleep_ms(plan.poll_interval_ms).await;
        let st = call_env(
            ctx,
            proto::status_request(&task_id),
            "查询 AI 头像合成状态",
            proto::parse_task_status,
        )
        .await?;
        match st.status {
            Some(proto::AI_AVATAR_STATUS_FAILED) => return Ok(None),
            Some(proto::AI_AVATAR_STATUS_SUCCESS) => {
                let Some(image_url) = st.image_url else {
                    return Ok(None);
                };
                // ⚠ 裸 GET（旧 TS `task.ts:42-46` 用的是不带鉴权的 `fetch`）。
                // ⚠⚠ 必须 `request_raw`：这一枪回的是图片字节，`request()` 会把
                // 它经 `HttpResponse.body`（String / 有损 UTF-8）改掉且不报错。
                let merged_bytes = ctx
                    .http
                    .request_raw(proto::image_download_request(&image_url))
                    .await?;
                let merged_image = image_download_base64(&merged_bytes)?;
                let id = call_env(
                    ctx,
                    proto::upload_request(&origin, &merged_image),
                    "上传 AI 头像合成图",
                    proto::parse_upload_id,
                )
                .await?;
                return Ok(Some(id));
            }
            // 其余（含「没给 status」）⇒ 继续等下一轮。
            _ => continue,
        }
    }
    Ok(None)
}

/// 抽奖：先读余量，再按它抽。
///
/// # 三条与旧 TS 逐字对齐的处置
///
/// 1. **余量读不到 ⇒ 兜底 [`AI_AVATAR_DRAW_FALLBACK`] 次**（`task.ts:111-119`），
///    并把这件事写进小结（兜底是猜测，不是读数）。
/// 2. **非 0 码里只有 408 是「停」**，其余记一句继续（`task.ts:127-132`）。
/// 3. **等待只在成功那一支之后**：旧实现的 `continue` 会跳过循环尾那句
///    `sleep(1000)`，所以失败那一支是**立刻**再抽的 —— 照抄，不「顺手统一」。
///
/// ⚠ 与旧 TS 的**一处刻意分歧**：余量那一枪若回**非 200**，这里**上抛**
/// （旧 TS 会吞掉并拿兜底去抽 5 次）。理由见模块头「活动可能打不通」：
/// 404 被吞掉之后，界面看到的是一句「抽了 5 次都失败」，而不是「这口没了」。
async fn draw_all(
    ctx: &TaskContext,
    plan: &AiAvatarPlan,
    result: &mut AiAvatarResult,
) -> Result<()> {
    let mut count = AI_AVATAR_DRAW_FALLBACK;
    match call_env(
        ctx,
        proto::hash_left_request(),
        "查询 AI 头像抽奖次数",
        proto::parse_hash_left,
    )
    .await
    {
        Ok(Some(n)) => {
            result.draw_left = Some(n);
            count = n.max(0);
        }
        Ok(None) => {
            result.draw_fallback_used = true;
            result.notes.push(format!(
                "抽奖余量那一格上游没给数字 ⇒ 按旧实现用兜底 {AI_AVATAR_DRAW_FALLBACK} 次"
            ));
        }
        Err(e) if is_transport(&e) => return Err(e),
        Err(e) => {
            result.draw_fallback_used = true;
            result.notes.push(format!(
                "读抽奖余量失败（{e}）⇒ 按旧实现用兜底 {AI_AVATAR_DRAW_FALLBACK} 次"
            ));
        }
    }

    if count == 0 {
        result.notes.push("抽奖余量读到 0 ⇒ 一枪都不抽".into());
        return Ok(());
    }

    for index in 0..count {
        ctx.check_cancelled()?;
        match call_env(ctx, proto::draw_request(), "AI 头像抽奖", proto::parse_draw).await {
            Ok(proto::DrawOutcome::Stopped) => {
                result.draw_stopped = true;
                break;
            }
            Ok(proto::DrawOutcome::Won { prize }) => {
                result.draw_attempted += 1;
                result.draw_won += 1;
                if let Some(p) = prize {
                    result.prizes.push(p);
                }
                // ⚠ 等待只在**成功**这一支之后（旧实现的 continue 跳过了它）。
                if index + 1 < count {
                    sleep_ms(plan.draw_interval_ms).await;
                }
            }
            Err(e) if is_transport(&e) => return Err(e),
            Err(e) => {
                result.draw_attempted += 1;
                result
                    .failures
                    .push(format!("第 {} 次抽奖失败：{e}", index + 1));
            }
        }
    }
    Ok(())
}

/// 取一份「动笔之前」的目录快照（只记 `fileId`）。
///
/// 这是 [`collect_and_clean`] 判「哪些是本趟造的」的**唯一依据**，见模块头
/// 「收尾护栏」。
async fn snapshot_file_ids(ctx: &TaskContext, parent_file_id: &str) -> Result<HashSet<String>> {
    let page = call_raw(
        ctx,
        proto::file_list_request(parent_file_id),
        "列 AI 头像活动目录（基线）",
        proto::parse_file_list,
    )
    .await?;
    Ok(page.items.into_iter().map(|e| e.file_id).collect())
}

/// 清点本趟产物并收尾。
///
/// # 只删「名字对得上 **且** 基线里没有」的那些
///
/// 见模块头：旧 TS 只按名字匹配，会把同名的旧文件一起删；本模块先过基线。
/// 基线或活动目录缺失 ⇒ **整个收尾跳过**（宁可留下，也不误删）。
///
/// # 为什么这一段的失败**不上抛**
///
/// 它跑在**写已经发生之后**。这里再 `?` 出去，会把「本趟写了什么」这份台账
/// 连同 Err 一起丢掉，而账本里那些条目正是将来人工清理的唯一线索。
/// 所以一律落进 [`AiAvatarResult::cleanup_error`]（进小结、进明细）——
/// 是**如实报告**，不是静默。
async fn collect_and_clean(
    ctx: &TaskContext,
    upload_ids: &[String],
    baseline: Option<&HashSet<String>>,
    parent_file_id: Option<&str>,
    result: &mut AiAvatarResult,
) {
    if upload_ids.is_empty() {
        return;
    }
    let (Some(baseline), Some(parent)) = (baseline, parent_file_id) else {
        result.notes.push(
            "缺收尾基线（或活动目录）⇒ 无法证明哪些文件是本趟造的，收尾跳过：宁可留下也不误删"
                .into(),
        );
        return;
    };

    // ① 作品表：把 uploadId 映成图片文件名。
    let works = match call_env(
        ctx,
        proto::work_list_request(),
        "查询 AI 头像作品",
        proto::parse_work_list,
    )
    .await
    {
        Ok(w) => w,
        Err(e) => {
            result.cleanup_error = format!("作品表没读到：{e}");
            return;
        }
    };
    let ours: HashSet<&str> = upload_ids.iter().map(String::as_str).collect();
    let mut target_names: HashSet<String> = HashSet::new();
    for work in &works {
        if !ours.contains(work.id.as_str()) {
            continue;
        }
        let urls = [work.img_url.as_deref(), work.qr_img_url.as_deref()];
        for url in urls.into_iter().flatten() {
            if let Some(name) = proto::extract_image_file_name(url) {
                target_names.insert(name);
            }
        }
    }
    if target_names.is_empty() {
        result.notes.push(format!(
            "本趟 {} 个 upload id 在作品表里一个都没对上 ⇒ 收尾没有可匹配的名字",
            upload_ids.len()
        ));
        return;
    }

    // ② 列目录（动笔之后）。
    let after = match call_raw(
        ctx,
        proto::file_list_request(parent),
        "列 AI 头像活动目录",
        proto::parse_file_list,
    )
    .await
    {
        Ok(page) => page.items,
        Err(e) => {
            result.cleanup_error = format!("列目录没读到：{e}");
            return;
        }
    };

    // ③ 名字对得上 ⇒ 交给账本判是不是本趟造的（`pre_existing` 就是「在不在基线里」）。
    for entry in after {
        if !target_names.contains(&entry.name) {
            continue;
        }
        let pre_existing = baseline.contains(&entry.file_id);
        match ctx.register_cloud_file(entry.file_id.clone(), pre_existing) {
            Ok(true) => result.registered_file_ids.push(entry.file_id),
            // 名字撞上、但它是**动笔之前就有**的 ⇒ 用户原有的东西，不登记。
            Ok(false) => result.pre_existing_skipped += 1,
            Err(e) => {
                result.cleanup_error = format!("产物登记失败：{e}");
                return;
            }
        }
    }

    cleanup_ledger(ctx, result).await;
}

/// 把账本上这一批本趟产物移进回收站，成功就销账。
///
/// ⚠ `batchTrash` 是**移进回收站**、不是彻底删除（旧实现 `utils/file.ts:111`
/// 的注释也这么写）—— 真删错了还能捞回来。但它是**写**，所以只对本趟登记过
/// 的 id 发，且空账本**不发**（旧实现也是先判 `files.length > 0`）。
async fn cleanup_ledger(ctx: &TaskContext, result: &mut AiAvatarResult) {
    let pending = ctx.artifacts.pending();
    if pending.is_empty() {
        return;
    }
    let ids: Vec<String> = pending.iter().map(|a| a.id.clone()).collect();
    let req = match cloud_trash::batch_trash_request(&ids) {
        Ok(r) => r,
        Err(e) => {
            result.cleanup_error = e.to_string();
            return;
        }
    };
    match ctx.http.request(req).await {
        Ok(resp) => {
            let parsed = mcloud_upstream::session::ensure_http_ok(&resp, "清理 AI 头像临时文件")
                .and_then(cloud_trash::parse_batch_trash);
            match parsed {
                // 销账要三字段完全对上才消得掉；删了几条与发了几条不相等时
                // `mark_cloud_files_removed` 交回的就是真实条数（见 artifacts.rs）。
                Ok(()) => result.cleaned_up = ctx.artifacts.mark_cloud_files_removed(&ids),
                Err(e) => result.cleanup_error = e.to_string(),
            }
        }
        Err(e) => result.cleanup_error = e.to_string(),
    }
}

// ══════════════════════════ 任务 ══════════════════════════

/// AI 头像（**会写**：合成并入账抽奖，并且只收走本趟写进网盘的活动文件）。
pub struct AiAvatarTask {
    /// 三处等待与三条上限，测试把它设成 0 / 1。
    pub plan: AiAvatarPlan,
    /// 原图随机源。**注入**而不是取全局（见模块头）。
    pub rng: SharedRandom,
}

impl AiAvatarTask {
    /// 生产形态：真等待 + OS 随机源。
    pub fn production() -> Self {
        Self {
            plan: AiAvatarPlan::default(),
            rng: SharedRandom::new(Box::new(mcloud_crypto::OsRandom)),
        }
    }
}

/// 把这一趟的读数交给明细通道。
///
/// 小结那一句被刻意压成一行，逐条失败 / 说明 / 产物 id 只能走这里。
/// ⚠ 里面**没有**任何凭据（票、cookie、`authorization` 一个都没有）——
/// 明细会落进运行台账与界面。
fn report_details(ctx: &TaskContext, result: &AiAvatarResult) {
    ctx.details(json!({
        "kind": AI_AVATAR_TASK_NAME,
        "nonIdempotentHedge": AI_AVATAR_NON_IDEMPOTENT_HEDGE,
        "plannedGenerate": result.planned_generate,
        "generated": result.generated,
        "generateFailures": result.generate_failures,
        "drawLeft": result.draw_left,
        "drawFallbackUsed": result.draw_fallback_used,
        "drawAttempted": result.draw_attempted,
        "drawWon": result.draw_won,
        "drawStopped": result.draw_stopped,
        "prizes": result.prizes,
        "registeredFileIds": result.registered_file_ids,
        "preExistingSkipped": result.pre_existing_skipped,
        "cleanedUp": result.cleaned_up,
        "cleanupError": result.cleanup_error,
        "failures": result.failures,
        "notes": result.notes,
    }));
}

#[async_trait]
impl Task for AiAvatarTask {
    fn name(&self) -> &'static str {
        AI_AVATAR_TASK_NAME
    }
    fn title(&self) -> &'static str {
        AI_AVATAR_TASK_TITLE
    }
    fn description(&self) -> &'static str {
        AI_AVATAR_TASK_DESCRIPTION
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        // 账号级开关。关掉就一枪都不发（旧 TS 的 `config.AI新头像?.开启`，`task.ts:165`）。
        if !ctx.account.settings.ai_avatar_enabled {
            let reason = "AI 头像已在账号设置里关掉（ai_avatar_enabled=false）";
            ctx.details(json!({
                "kind": AI_AVATAR_TASK_NAME,
                "verdict": "skipped",
                "reason": reason,
            }));
            return Ok(TaskOutcome::skipped(reason));
        }
        ctx.check_cancelled()?;

        let limit = ctx.account.settings.ai_avatar_daily_limit;
        let mut result = AiAvatarResult {
            planned_generate: limit,
            ..Default::default()
        };

        // ① 收尾要的基线：**动笔之前**那次列目录。
        // ⚠ limit == 0 时不取：本趟不会写东西，也就没有要归属的对象。
        let mut parent_file_id: Option<String> = None;
        let mut baseline: Option<HashSet<String>> = None;
        if limit > 0 {
            ctx.progress(1, 4, "读取 AI 头像活动目录（收尾基线）");
            ctx.check_cancelled()?;
            parent_file_id = call_env(
                ctx,
                proto::catalog_full_path_request(),
                "查询 AI 头像活动目录",
                proto::parse_catalog_parent_file_id,
            )
            .await?;
            match parent_file_id.clone() {
                Some(parent) => {
                    ctx.check_cancelled()?;
                    baseline = Some(snapshot_file_ids(ctx, &parent).await?);
                }
                None => result
                    .notes
                    .push("活动目录没读到（fullPath_sub 缺）⇒ 收尾会跳过：宁可留下也不误删".into()),
            }
        }

        // ② 生成（旧 TS `aiAvatarGenerateTask`，`task.ts:78-107`）。
        let mut upload_ids: Vec<String> = Vec::new();
        if limit > 0 {
            ctx.progress(2, 4, "生成 AI 头像");
            for index in 0..limit {
                ctx.check_cancelled()?;
                let mut got: Option<String> = None;
                let mut last_error: Option<String> = None;
                for _ in 0..self.plan.max_attempts {
                    ctx.check_cancelled()?;
                    match generate_once(ctx, &self.plan, &self.rng).await {
                        Ok(Some(id)) => {
                            got = Some(id);
                            break;
                        }
                        Ok(None) => last_error = None,
                        // 非 200 / 连不上 ⇒ 如实上抛（不折成本轮失败）。
                        Err(e) if is_transport(&e) => return Err(e),
                        Err(e) => last_error = Some(e.to_string()),
                    }
                }
                match got {
                    Some(id) => {
                        upload_ids.push(id);
                        result.generated += 1;
                        ctx.info(format!("AI 头像第 {}/{} 轮生成成功", index + 1, limit));
                    }
                    None => {
                        result.generate_failures += 1;
                        let head = format!("AI 头像第 {}/{} 轮生成失败", index + 1, limit);
                        ctx.info(head.clone());
                        result.failures.push(match last_error {
                            Some(e) => format!("{head}：{e}"),
                            None => head,
                        });
                    }
                }
                // 旧 TS `task.ts:102`：**轮间**等 2 秒（最后一轮不等）。
                if index + 1 < limit {
                    sleep_ms(self.plan.round_interval_ms).await;
                }
            }
        }

        // ③ 抽奖（旧 TS `aiAvatarDraw`，`task.ts:109-137`）。
        ctx.progress(3, 4, "AI 头像抽奖");
        ctx.check_cancelled()?;
        draw_all(ctx, &self.plan, &mut result).await?;

        // ④ 清点 + 收尾（旧 TS `collectTemporaryFiles`，`task.ts:139-162`）。
        ctx.progress(4, 4, "收尾：清点并清理本趟产物");
        ctx.check_cancelled()?;
        collect_and_clean(
            ctx,
            &upload_ids,
            baseline.as_ref(),
            parent_file_id.as_deref(),
            &mut result,
        )
        .await;

        report_details(ctx, &result);
        ctx.info(result.summary_line());
        Ok(result.task_outcome())
    }
}

#[cfg(test)]
mod tests;
