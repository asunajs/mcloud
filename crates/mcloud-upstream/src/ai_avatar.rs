//! AI 头像族（`ai-avatar`，界面那句是「AI 新头像」）—— **协议层**。
//!
//! 八口：读余量 / 抽奖 / 合成 / 查合成 / 传两张图 / 作品表 / 活动目录 /
//! 列网盘活动目录。判定（哪几口该发、发几次、发完怎么收尾）在
//! `mcloud-tasks::ai_avatar`；本文件只造请求、解响应，**零 IO**。
//!
//! 全部字面量来自旧 TS `packages/core/src/tasks/ai-avatar/api.ts`（那份**能跑**，
//! 见 AGENTS.md「依据的优先级」），逐条对过行号。
//!
//! # 下载合成图那一枪：取字节与形状校验都在任务层
//!
//! 这一枪回的是图片**字节**，不是 JSON。本层只造请求（[`image_download_request`]）；
//! 真正取字节走任务层的 [`mcloud_core::HttpClient::request_raw`]（线上原样字节，
//! 不经 UTF-8 解码），PNG 签名等形状校验与 base64 都在 `mcloud-tasks::ai_avatar`。
//!
//! ⚠ 本文档早先写过「生成链走不到 upload」—— 那是 `request_raw` 落地之前的
//! **过期结论**，现在不成立：这条腿已经通了。
//!
//! # ⚠ 这条链今天**可能整条打不通** —— 这是一条线索，不是结论
//!
//! 参考实现 2026-09-25 真账号跑过一趟，逐字读数是：
//!
//! ```text
//! GET  /market/rabactive/image/getHashLeft   → HTTP 404
//! GET  /market/rabactive/image/draw          → HTTP 404
//! POST /market/rabactive/ai/image/merge      → HTTP 405
//! ```
//!
//! 两个账号一字不差。⇒「这一期活动下线了」与「我们少一环（路径或方法拼错）」
//! 在今天**同形**，分辨只能靠人工在活动站 H5 里打开这一页、抓一次加载链。
//!
//! ⚠ 本层**不据此改动任何路径或方法**：那两样是旧 TS 的字面量（能跑的那份），
//! 拿一个尚未分辨的现象去覆盖一条有出处的读数，是把证据换成猜测。
//!
//! ⚠⚠ 但运行期遇到**非 200** 必须**如实上抛**：这一族的每一枪都过
//! [`crate::session::ensure_http_ok`]，它把 HTTP 状态码与响应体前 200 字符
//! 一起交回。**绝不允许**把 404/405 静默当成成功 —— 否则上面那三行读数
//! 会被洗成一句「跑完了」。
//!
//! # 两套信封，别混用
//!
//! * 前七口（`rabactive/*`）是 `{code: <数字>, msg, result}` ⇒ 用
//!   [`crate::app_signin::SignInEnvelope`]（它认数字 `code` 与 `msg`/`message`
//!   两个键名）。
//! * 第八口（`hcy/file/list`）是 `{code:"0000"(字符串), success:true, message,
//!   data:{items}}` ⇒ 载荷在 `data`、成功判据是 **`success`**（旧 TS
//!   `task.ts:150` 读的就是它）。两种信封互换会永远判失败，且不报「形状不对」。
//!
//! # 头画像：这是仓库里**第三套**头（不是笔误）
//!
//! 这一族走的是旧 TS 的**主 http 实例**（`shared/src/http/base.ts:34` 的
//! `createHttp`），不是 H5 那一套（`createTaskHttp`）。所以：
//!
//! * 静态头只有 `user-agent` + `content-type`（**GET 也发**，base.ts:25-32）+
//!   `deviceid`。本文件的构造**只显式补 `content-type`**：
//!   `user-agent` / `deviceid` / `authorization` / `jwttoken` / cookie 全归
//!   [`crate::client::ReqwestClient`] 按 host 与 profile 注入（`endpoints::accepts_basic`
//!   / `accepts_jwt`）。在这里再设一遍**不会覆盖**，而是发出去两根同名线
//!   （reqwest 的 `.header()` 是追加）—— 那是本仓实测踩过的坑（见
//!   [`crate::client::should_inject_credential`]）。
//!   ⚠ 为什么 GET 也要显式补 `content-type`：client 的默认值只给**带体**请求补
//!   （`client.rs` 的 `default_content_type`），而旧 TS 把它放在静态头里、与有没有体
//!   无关。照抄旧实现这一侧（多一枚头），不是照抄 client 那一侧。
//! * **不发** `activityid` / `appversion` / `origin` / `referer` / `showloading`
//!   —— 那是签到族的 `h5()` 那套（`app_signin.rs`），本族**从不发**。所以这一族
//!   **没有签名头**（`x-request-id` / `x-timestamp` / `x-nonce` / `x-signature`
//!   一个都没有）：签名只在建会话那一刻用（见 `session.rs` 的 `H5Signature`）。
//! * 第八口另显式补 4 枚**业务头**（api.ts:87-92）。
//!
//! # 第八口那 4 枚头里有一格**未定**
//!
//! `x-yun-app-channel: 10214200` 属哪一端**没有出处**：本仓已知三档是
//! android `10000023` / web `10000034` / PC `10200153`（`app_cloud_upload.rs`），
//! 而 TS 别处没有这个值。这里照抄 TS 的字面量，**不猜它属于哪一档**。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};

use crate::app_signin::SignInEnvelope;
use crate::endpoints::{MCLOUD_HOST, PERSONAL_KD};

// ══════════════════════════ 常量 ══════════════════════════

/// 活动图片族的路径前缀（旧 TS `api.ts:5` 的 `IMAGE_URL` 去掉主机）。
///
/// ⚠ 前缀是 `/market/` 而**不是**这一族其它活动常见的 `/ycloud/`
/// （坑表「签到域的主机与路径前缀有两套」）。拼成 `/ycloud/market/rabactive/…`
/// 不会报「路径不存在」，会回一个 404 页面 —— 而 404 在这一层看着就像
/// 「活动下线」。所以这三段是**一个常量一个来源**。
pub const RABACTIVE_IMAGE_PATH: &str = "/market/rabactive/image";

/// 合成那一族的路径前缀（旧 TS `api.ts:4` 的 `AI_IMAGE_URL` 去掉主机）。
///
/// ⚠ 它比 [`RABACTIVE_IMAGE_PATH`] 多一段 `ai`：
/// `/market/rabactive/ai/image/merge`。两族**不是一个前缀**，别互相搬。
pub const RABACTIVE_AI_IMAGE_PATH: &str = "/market/rabactive/ai/image";

/// 网盘侧列目录那一口（旧 TS `api.ts:6` 写死 `personal-kd-njs.yun.139.com`）。
///
/// ⚠ 这一台主机**不在** [`crate::endpoints::JWT_HOSTS`] 里 ⇒ 它拿
/// `authorization`（`yun.139.com` 后缀命中 `AUTH_DOMAINS`）而**不拿 `jwttoken`**。
/// 这不是漏写，是旧 TS 的 `base.ts:86-90` 就这么分的。
pub const CLOUD_FILE_LIST_PATH: &str = "/hcy/file/list";

/// 这一族的静态 `content-type`（旧 TS `base.ts:30` 逐字，**带 charset**）。
pub const AI_AVATAR_CONTENT_TYPE: &str = "application/json;charset=UTF-8";

/// `merge` 体里那个固定的扩展名（旧 TS `api.ts:54` 写死 `"jpg"`）。
///
/// ⚠ 与「喂进去的其实是 PNG 噪点图」**不矛盾，也不要「顺手改对」**：
/// 上游按哪个扩展名处理这张图**未测试**，改这一个字节等于改一次接口。
pub const AI_AVATAR_IMAGE_EXT: &str = "jpg";

/// `catalogFullPath` 的 `marketName` 默认值（旧 TS `api.ts:70`）。
///
/// ⚠ 语义**未验证**（零实拍）：看着像「活动侧给这个玩法建的那个网盘目录」的键名。
/// 本层只用它换 `fullPath_sub`，不解释它。
pub const AI_AVATAR_MARKET_NAME: &str = "rabbityearpic";

/// `getWorkList` 的 `type` 默认值（旧 TS `api.ts:66`）。
///
/// ⚠ 取值表**没有成文依据**（零实拍），所以这里只钉住 TS 用的那一个数，
/// 不枚举「还有哪些 type」。
pub const AI_AVATAR_WORK_TYPE: i64 = 1;

/// `hcy/file/list` 的 `pageSize`（旧 TS `api.ts:80` 的默认参数）。
///
/// ⚠ 这一族**不翻页**：多于 100 个文件时后面的静默匹配不到，表现是
/// 「临时文件没被清掉」而不是报错。这是旧实现的行为，本层照抄并标出来。
pub const AI_AVATAR_FILE_LIST_PAGE_SIZE: i64 = 100;

/// 合成成功那一档的 `status`（旧 TS `task.ts:66`）。
///
/// ⚠ **档位表零实拍**：`3` 被读成成功、`4` 被读成失败，其余一律「继续等」，
/// 全是从旧实现的分支形状反推的。要核它：真跑一次生成，把每次 `getStatus`
/// 的 `status` 记下来。
pub const AI_AVATAR_STATUS_SUCCESS: i64 = 3;

/// 合成失败那一档的 `status`（旧 TS `task.ts:65`）。语义同上，**零实拍**。
pub const AI_AVATAR_STATUS_FAILED: i64 = 4;

/// `draw` 那个「今天就到此为止」的码（旧 TS `task.ts:128` 的 `408`）。
///
/// ⚠ 旧实现**只认这一个非 0 码**并 `break`，其余非 0 是 `continue`。
/// `408` 到底是「次数用完」还是「上游限流」**没有成文与样本** ⇒ 本层
/// 只把它折成一档「停止」，**不解释**它。
pub const AI_AVATAR_DRAW_CODE_STOPPED: i64 = 408;

/// 第八口那 4 枚业务头（旧 TS `api.ts:87-92` 逐字）。
///
/// ⚠ `x-yun-app-channel` 那个 `10214200` **未定**（见模块头）：本仓已知三档
/// 都不是它。照抄，不解释。
pub const AI_AVATAR_YUN_API_VERSION: &str = "v1";
pub const AI_AVATAR_YUN_APP_CHANNEL: &str = "10214200";
pub const AI_AVATAR_YUN_MODULE_TYPE: &str = "100";
pub const AI_AVATAR_YUN_OP_TYPE: &str = "1";

// ══════════════════════════ 请求 ══════════════════════════

/// 给请求补上这一族的**静态头**。
///
/// 只补 `content-type` —— 其余（`user-agent` / `deviceid` / `authorization` /
/// `jwttoken` / cookie）全归发送层，理由见模块头「头画像」。
fn with_static_headers(req: HttpRequest) -> HttpRequest {
    req.header("content-type", AI_AVATAR_CONTENT_TYPE)
}

/// `GET {RABACTIVE_IMAGE_PATH}/getHashLeft` —— 读「还能抽几次」。**只读**。
///
/// 无查询串、无体（旧 TS `api.ts:43-45`）。
pub fn hash_left_request() -> HttpRequest {
    with_static_headers(HttpRequest::get(rabactive_image_url("/getHashLeft")))
}

/// `GET {RABACTIVE_IMAGE_PATH}/draw` —— **写**：消耗一次抽奖次数并发奖。
///
/// 「该不该发、发几次」由调用方判（判据是 [`hash_left_request`] 那一格，
/// 见 `mcloud-tasks::ai_avatar`）。无查询串、无体（旧 TS `api.ts:47-49`）。
pub fn draw_request() -> HttpRequest {
    with_static_headers(HttpRequest::get(rabactive_image_url("/draw")))
}

/// `POST {RABACTIVE_AI_IMAGE_PATH}/merge` —— 提交一张图换一次合成任务。
///
/// body 两键逐字（旧 TS `api.ts:51-56`）：`imageBase64` 与 `imageExt`。
/// ⚠ `imageBase64` 是**裸 base64**，不带 `data:image/png;base64,` 前缀
/// （旧 TS `task.ts:38-40` 专门把那一段正则去掉）。带上前缀会怎样：零样本。
pub fn merge_request(image_base64: &str) -> HttpRequest {
    let body = serde_json::json!({
        "imageBase64": image_base64,
        "imageExt": AI_AVATAR_IMAGE_EXT,
    });
    with_static_headers(HttpRequest::post(ai_image_url("/merge")).json_body(&body))
}

/// `POST {RABACTIVE_AI_IMAGE_PATH}/getStatus` —— 查一次合成进度。**只读**。
///
/// body 只有 `taskId` 一键（旧 TS `api.ts:58-60`）。轮询节奏在调用方。
pub fn status_request(task_id: &str) -> HttpRequest {
    let body = serde_json::json!({ "taskId": task_id });
    with_static_headers(HttpRequest::post(ai_image_url("/getStatus")).json_body(&body))
}

/// `POST {RABACTIVE_IMAGE_PATH}/upload` —— **这一口会往用户网盘写图片**。
///
/// body 两键（旧 TS `api.ts:62-64`）：`originImage`（本地那张噪点原图）与
/// `mergeImage`（上游生成图**下载回来**后的 base64），两张**同时**上传。
/// ⚠ 写侧的粒度落在**上游**：不是本仓 `app_cloud_upload` 那条本地三枪直传。
pub fn upload_request(origin_image: &str, merge_image: &str) -> HttpRequest {
    let body = serde_json::json!({
        "originImage": origin_image,
        "mergeImage": merge_image,
    });
    with_static_headers(HttpRequest::post(rabactive_image_url("/upload")).json_body(&body))
}

/// `GET {RABACTIVE_IMAGE_PATH}/getWorkList?type=1` —— 作品表。**只读**。
///
/// ⚠ `type` 是**钉死的一个数**（常量 [`AI_AVATAR_WORK_TYPE`]）而不是入参：
/// 取值表没有依据，开成参数只会多一个「传错值静默匹配不到」的地方。
pub fn work_list_request() -> HttpRequest {
    with_static_headers(HttpRequest::get(format!(
        "{}?type={}",
        rabactive_image_url("/getWorkList"),
        AI_AVATAR_WORK_TYPE
    )))
}

/// `GET {RABACTIVE_IMAGE_PATH}/catalogFullPath?marketName=rabbityearpic`。**只读**。
///
/// ⚠ `marketName` 同样钉死（[`AI_AVATAR_MARKET_NAME`]）：它的语义未验证，
/// 开成参数只是把一处猜测摊到调用点。
pub fn catalog_full_path_request() -> HttpRequest {
    with_static_headers(HttpRequest::get(format!(
        "{}?marketName={}",
        rabactive_image_url("/catalogFullPath"),
        AI_AVATAR_MARKET_NAME
    )))
}

/// `POST {PERSONAL_KD}/hcy/file/list` —— 列活动目录下的文件。**只读**。
///
/// body 五键逐字（旧 TS `api.ts:76-85`）：`parentFileId` / `pageInfo.pageSize` /
/// `imageThumbnailStyleList` / `orderDirection` / `orderBy`。
///
/// ⚠ 旧 TS 的 `orderBy` 是 `"updated_at"`（**下划线**），`orderDirection` 是 `"DESC"`
/// —— 这一对决定「刚上传的排在最前」是否成立，改一个字母不会报错，只会让顺序变。
pub fn file_list_request(parent_file_id: &str) -> HttpRequest {
    let body = serde_json::json!({
        "parentFileId": parent_file_id,
        "pageInfo": { "pageSize": AI_AVATAR_FILE_LIST_PAGE_SIZE },
        "imageThumbnailStyleList": ["Big", "Small"],
        "orderDirection": "DESC",
        "orderBy": "updated_at",
    });
    with_static_headers(
        HttpRequest::post(format!("{PERSONAL_KD}{CLOUD_FILE_LIST_PATH}"))
            .json_body(&body)
            .header("x-yun-api-version", AI_AVATAR_YUN_API_VERSION)
            .header("x-yun-app-channel", AI_AVATAR_YUN_APP_CHANNEL)
            .header("x-yun-module-type", AI_AVATAR_YUN_MODULE_TYPE)
            .header("x-yun-op-type", AI_AVATAR_YUN_OP_TYPE),
    )
}

/// `GET <imageUrl>` —— 把上游生成图下载回来。
///
/// # ⚠ 这一枪**不是**主实例发的
///
/// 旧 TS 用的是**裸 `fetch`**（`task.ts:42-46`），**不带任何鉴权头**。
/// 所以这里只给一个裸 GET，**一枚头都不补**（连 `content-type` 也不补）——
/// 交给发送层按 host 决定要不要注入凭据。
///
/// # ⚠⚠ 这一枪的响应体是**图片字节**，本层不解析
///
/// [`mcloud_core::HttpResponse::body`] 是 `String`，装不下 PNG / JPEG 的字节：
/// 有损解码后的字符串再 base64，发出去的是一份**内容已被破坏**的图 ——
/// 而且这条链**不会因此报错**（上传照样可能回 `code:0`）。
///
/// ⇒ 取这一枪的响应必须走 [`mcloud_core::HttpClient::request_raw`]（线上原样字节），
/// 而**取字节与判形状都在任务层** `mcloud-tasks::ai_avatar`：它拿到字节后先做
/// 形状校验（PNG 签名等），再 base64 交给 upload。本层**只造请求**，
/// 不提供任何基于文本体的解析入口 —— 那正是会把字节毁掉的那条路。
pub fn image_download_request(image_url: &str) -> HttpRequest {
    HttpRequest::get(image_url)
}

fn rabactive_image_url(suffix: &str) -> String {
    format!("{MCLOUD_HOST}{RABACTIVE_IMAGE_PATH}{suffix}")
}

fn ai_image_url(suffix: &str) -> String {
    format!("{MCLOUD_HOST}{RABACTIVE_AI_IMAGE_PATH}{suffix}")
}

// ══════════════════════════ 解析（前七口） ══════════════════════════

/// `merge` 与 `getStatus` **共用**的解析结果（旧 TS 同一个 `AiAvatarStatus`）。
///
/// # 三格全部可选，而且这不是偷懒
///
/// `merge` 那一次只给 `taskId`，`getStatus` 才有 `status` / `imageUrl`，
/// 而同一段代码要吃这两次 ⇒「缺哪格」是**调用方的判据**，不是这一层的报错理由
/// （旧 TS 也正是分两处各判各的：`task.ts:52` 与 `task.ts:60-69`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiAvatarTaskStatus {
    /// `result.taskId` —— `merge` 给出、`getStatus` 带回去。
    ///
    /// 旧实现在 `merge` 之后立刻判 `!taskId`（`task.ts:52`）⇒ 没有它这条链走不下去。
    /// 仍做成 `Option`：**缺了要能被叫出来**，而不是默认成空串继续发下一枪。
    pub task_id: Option<String>,
    /// `result.status` —— 档位见 [`AI_AVATAR_STATUS_SUCCESS`] / [`AI_AVATAR_STATUS_FAILED`]。
    pub status: Option<i64>,
    /// `result.imageUrl` —— 只在「成功」那一档有意义。
    pub image_url: Option<String>,
}

/// 解析 `merge` / `getStatus` 的响应。
///
/// 外层 `code` 非 0 一律报错（这一枪没打上）：那是 [`SignInEnvelope::ensure_ok`] 管的，
/// 与「result 里缺哪一格」是两件事。
/// ⚠ 这里用 `ensure_ok` 而**不是** `ok_result`：缺 `result` 时三格全 `None`
/// 交给调用方判，而不是报「响应没有 result」—— 后者会把「上游这次没给读数」
/// 念成「接口故障」，是两句不同的话（与 `app_msg_push` 同一口径）。
pub fn parse_task_status(env: &SignInEnvelope) -> Result<AiAvatarTaskStatus> {
    env.ensure_ok("AI 头像合成")?;
    let Some(obj) = env.result.as_ref().filter(|v| v.is_object()) else {
        return Ok(AiAvatarTaskStatus::default());
    };
    Ok(AiAvatarTaskStatus {
        task_id: id_string_at(obj, "taskId"),
        status: opt_int_at(obj, "status"),
        image_url: opt_string_at(obj, "imageUrl"),
    })
}

/// 解析 `getHashLeft` 的 `result`（「还能抽几次」）。
///
/// # `None` **不是** `0`
///
/// 旧 TS 的判据是 `code === 0 && typeof result === "number"`（`task.ts:114`），
/// 读不到就**保留兜底值**（5 次）。⇒ 本函数把「没给数字」交回 `None`、
/// **不报错也不折成 0**：折成 0 会让「还能抽」变成「已用完」。
/// ⚠ 但那是**业务层**的兜底：HTTP 非 200 仍由 `ensure_http_ok` 判成败，
/// 与本函数的 `None` 是两件事。
pub fn parse_hash_left(env: &SignInEnvelope) -> Result<Option<i64>> {
    env.ensure_ok("查询 AI 头像抽奖次数")?;
    Ok(env.result.as_ref().and_then(as_int))
}

/// 一次抽奖的结局。
///
/// # 为什么要在类型上分「停止」与「失败」
///
/// 旧实现（`task.ts:126-136`）对非 0 码分两种处置：[`AI_AVATAR_DRAW_CODE_STOPPED`]
/// 是 `break`（今天到此为止），其余是 `continue`（换下一次）。这个分叉**必须**
/// 在类型上分得出来，否则调用方只能拿错误消息做子串匹配 —— 改一个字就会把
/// 「停止」念成「重试」，表现是对一个已经拒绝的账号继续连打。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawOutcome {
    /// `code == 0`：抽了一次。
    ///
    /// 奖品名按旧 TS 的优先序（`contentName || prizeName`，`task.ts:133`）合成一个
    /// `Option`；两格都没有时是 `None`（旧实现那句是「抽奖完成」，不报名字）。
    Won { prize: Option<String> },
    /// `code == 408`：上游叫停。**这一档的响应体形状零样本** ⇒ 不下钻 `result`。
    Stopped,
}

/// 解析一次抽奖的响应。
///
/// 判据**只看 `code`**，不看错误消息（理由见 [`DrawOutcome`]）。
/// 其余非 0 码走 [`SignInEnvelope::ensure_ok`] 报成 `Error::Upstream`（带 code/message），
/// 由调用方按「换下一次」处置。
pub fn parse_draw(env: &SignInEnvelope) -> Result<DrawOutcome> {
    if env.code == AI_AVATAR_DRAW_CODE_STOPPED {
        return Ok(DrawOutcome::Stopped);
    }
    env.ensure_ok("AI 头像抽奖")?;
    let prize = env
        .result
        .as_ref()
        .and_then(|r| opt_string_at(r, "contentName").or_else(|| opt_string_at(r, "prizeName")));
    Ok(DrawOutcome::Won { prize })
}

/// 解析 `upload` 回的那个 id（`result` 是**裸字符串**）。
///
/// # 两处**刻意收紧**旧 TS
///
/// 旧实现只判 `code === 0` 就把 `result` 收下（`task.ts:72`），连空串也收。
/// 空 id 在后续「按 id 匹配作品」那一步**永远匹配不到任何东西** ⇒ 表现是
/// 「网盘里多了张图，没人知道」，而没有任何报错。所以这里：
/// ① `result` 不是字符串 ⇒ 解析失败；② 空串 ⇒ 解析失败。
pub fn parse_upload_id(env: &SignInEnvelope) -> Result<String> {
    env.ensure_ok("上传 AI 头像合成图")?;
    match env.result.as_ref().and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        Some(_) => Err(Error::Parse(
            "upload 回了空字符串 id（旧实现会照收，但拿着它后续一次都匹配不到）".into(),
        )),
        None => Err(Error::Parse(
            "upload 的 result 不是字符串（旧 TS 的类型是 ApiResponse<string>）".into(),
        )),
    }
}

/// `getWorkList` 里的一条作品。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiAvatarWork {
    /// `result[].id` —— 与 `upload` 回的那个 id 对得上才作数（旧 TS `task.ts:26`）。
    ///
    /// ⚠「`upload` 的 id 与 `work.id` 是不是同一物」**零实拍**，旧 TS 直接用
    /// 相等判 [推断：同物]。本层照抄那个判据，不另立一套。
    pub id: String,
    /// `imgUrl` —— 只用它的**文件名**去网盘里匹配（见 [`extract_image_file_name`]）。
    pub img_url: Option<String>,
    /// `qrImgUrl` —— 同上，二维码那张也是产物。
    pub qr_img_url: Option<String>,
}

/// 解析作品列表。
///
/// `result` 缺席或不是数组 ⇒ 交回空列表（这一口是**只读**，为它判失败只会让
/// 一次真跑白跑）。条目里 `id` 取不到的直接**跳过**：它在后续只参与相等匹配，
/// 没有 id 的条目本来也匹配不上。
pub fn parse_work_list(env: &SignInEnvelope) -> Result<Vec<AiAvatarWork>> {
    env.ensure_ok("查询 AI 头像作品")?;
    let Some(Value::Array(items)) = env.result.as_ref() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = id_string_at(item, "id") else {
            continue;
        };
        out.push(AiAvatarWork {
            id,
            img_url: opt_string_at(item, "imgUrl"),
            qr_img_url: opt_string_at(item, "qrImgUrl"),
        });
    }
    Ok(out)
}

/// 从 `catalogFullPath` 的 `result.fullPath_sub` 取「这个目录的 id」。
///
/// 旧 TS（`task.ts:146`）：`fullPath_sub.split("/").filter(Boolean).pop()`
/// —— 按 `/` 切、丢空段、取最后一段。回 `None` = 拿不到（那就不找了）。
///
/// ⚠「路径最后一段就是 `fileId`」这一格**未验证**（零实拍）。本层照抄切法，
/// **不擅自改成别的取法**：换一种取法同样没有样本，却会把「拿不到就跳过」
/// 变成「拿一个错的 id 去列目录」。
pub fn parse_catalog_parent_file_id(env: &SignInEnvelope) -> Result<Option<String>> {
    env.ensure_ok("查询 AI 头像活动目录")?;
    let full = env
        .result
        .as_ref()
        .and_then(|r| opt_string_at(r, "fullPath_sub"));
    Ok(full
        .as_deref()
        .and_then(|s| s.split('/').filter(|seg| !seg.is_empty()).next_back())
        .map(str::to_string))
}

/// `hcy/file/list` 的一个条目 —— 只留本族要用的两格。
///
/// ⚠ 实拍条目里有二十余枚键（`updatedAt` / `type` / `ownerId` …），本层**只读两枚**：
/// 「读了别的键判什么」今天答不出来，而加一个不参与判定的字段只会让下一个读者
/// 以为它在参与判定。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudFileEntry {
    /// `data.items[].fileId`（实拍形态如 `/synbox`、`FFa…`）。
    pub file_id: String,
    /// `data.items[].name` —— 与作品图片的**文件名**比对。
    pub name: String,
}

/// `hcy/file/list` 的一页。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudFileList {
    pub items: Vec<CloudFileEntry>,
    /// `data.nextPageCursor` —— **实拍里有这一格**（值可能是 `null`）。
    ///
    /// 本族**不翻页**（旧 TS 固定 `pageSize: 100` 一次，`api.ts:76-80`），
    /// 所以这一格今天只用于诊断；把它登记出来的目的是让「不翻页」这件事
    /// 在类型上看得见，而不是等某天有人发现文件没清掉再去猜。
    pub next_page_cursor: Option<String>,
}

/// 解析 `hcy/file/list` 的回执。
///
/// # 这一口的信封与另外七口**不同**（见模块头）
///
/// 成功判据是顶层 `success == true`（旧 TS `task.ts:150` 读的就是它）。
/// ⚠ 本层**刻意收紧**旧 TS：`success` 缺席或不是 `true` ⇒ 报错，而不是
/// 静默交回空列表。旧实现的 `if (!fileList.success) return;` 会把「上游说这次
/// 失败了」念成「这个目录是空的」，两者在清理链上的后果完全不同 ——
/// 后者是「没什么要清」，前者是「这一趟的清点没做成」。
///
/// `data` / `data.items` 缺席 ⇒ **空列表**（实拍里 `items` 是常驻的，「没有」
/// 只能是这一页确实没文件）；`items` 存在但不是数组 ⇒ 报错（形状变了不能
/// 洗成「空目录」）。
pub fn parse_file_list(body: &str) -> Result<CloudFileList> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("hcy/file/list 回执不是合法 JSON：{e}")))?;
    match v.get("success") {
        Some(Value::Bool(true)) => {}
        Some(other) => {
            return Err(Error::Upstream {
                code: v
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                message: format!("hcy/file/list 回 success={other}：{}", upstream_msg(&v)),
            })
        }
        None => {
            return Err(Error::Parse(
                "hcy/file/list 回执没有 success 这一格 —— 按失败处置（旧实现读不到就当没清点）"
                    .into(),
            ))
        }
    }
    let data = v.get("data").filter(|d| d.is_object());
    let Some(raw_items) = data.and_then(|d| d.get("items")) else {
        return Ok(CloudFileList::default());
    };
    let Some(items) = raw_items.as_array() else {
        return Err(Error::Parse(
            "hcy/file/list 的 data.items 不是数组（形状变了，不能洗成「空目录」）".into(),
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let (Some(file_id), Some(name)) =
            (opt_string_at(item, "fileId"), opt_string_at(item, "name"))
        else {
            continue;
        };
        out.push(CloudFileEntry { file_id, name });
    }
    Ok(CloudFileList {
        items: out,
        next_page_cursor: data.and_then(|d| opt_string_at(d, "nextPageCursor")),
    })
}

// ══════════════════════════ 小工具 ══════════════════════════

/// 从图片 URL 里抽**文件名**（旧 TS `task.ts:12-16` 的 `extractImageFileName`）。
///
/// 逐字规则：取最后一个 `/` 之后那一段、在第一个 `?` 或 `#` 处截断，
/// 且只认 `.jpg` / `.jpeg` / `.png`（大小写不敏感）。
///
/// ⚠ 只认这三种后缀是**故意的**：这一族列目录后要按名字匹配，把任何串都当
/// 文件名会扩大匹配面，而匹配面的另一端是**删除**。
pub fn extract_image_file_name(url: &str) -> Option<String> {
    let tail = url.rsplit('/').next()?;
    let name = tail.split(['?', '#']).next().unwrap_or("");
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let ok = [".jpg", ".jpeg", ".png"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    ok.then(|| name.to_string())
}

/// 取一个**可选**整数：缺键与「给了非数字」都回 `None`。
///
/// 与 `app_msg_push::opt_int_at` 同一判据（那边是私有函数，本 crate 里各持一份）。
fn opt_int_at(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(as_int)
}

/// 把一个 `Value` 读成整数：数字与**字符串形式的数字**都收，其余 `None`。
///
/// 上游有的字段是字符串形式的数字，所以两种形态都要收 —— 但**不**做
/// 「非数字就编个 0」。
fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// 取一个**可选**字符串：只有 JSON 字符串才算，其余 `None`。
fn opt_string_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// 取一个**身份类**字符串：字符串照收，**数字也收**（转成十进制串）。
///
/// 旧 TS 把 `id` / `taskId` 声明成 `string`，而活动接口回数字 id 是这一族
/// 之外也见过的形态；两种都收才不会让「相等匹配」静默永远为假。
/// ⚠ 浮点、布尔、`null` 都不收 —— 那些不是 id。
fn id_string_at(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => n.as_i64().map(|i| i.to_string()),
        _ => None,
    }
}

/// 取上游那句 `msg` / `message`，缺席时给一句**可判别**的占位。
///
/// 占位不写成空串：空串在日志里与「上游返回了空 msg」长得一样，
/// 而这两件事的排查方向完全不同。
fn upstream_msg(v: &Value) -> &str {
    v.get("msg")
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("（上游没给 msg）")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn body_json(req: &HttpRequest) -> Value {
        serde_json::from_str(req.body_str().expect("这一枪该带文本体")).expect("体该是 JSON")
    }

    fn env_of(v: Value) -> SignInEnvelope {
        SignInEnvelope::parse(&v.to_string()).expect("信封该能解")
    }

    // ── URL 与方法：逐字对旧 TS 的八个字面量 ──────────────────────────

    #[test]
    fn the_five_rabactive_urls_are_verbatim() {
        assert_eq!(
            hash_left_request().url,
            "https://m.mcloud.139.com/market/rabactive/image/getHashLeft"
        );
        assert_eq!(
            draw_request().url,
            "https://m.mcloud.139.com/market/rabactive/image/draw"
        );
        assert_eq!(
            merge_request("AAAA").url,
            "https://m.mcloud.139.com/market/rabactive/ai/image/merge"
        );
        assert_eq!(
            status_request("T1").url,
            "https://m.mcloud.139.com/market/rabactive/ai/image/getStatus"
        );
        assert_eq!(
            upload_request("O", "M").url,
            "https://m.mcloud.139.com/market/rabactive/image/upload"
        );
    }

    /// ⚠ 前缀是 `/market/rabactive`，**不是** `/ycloud/…`；
    /// 合成那一族多一段 `ai`。
    #[test]
    fn the_path_prefixes_are_not_swapped() {
        assert!(merge_request("A")
            .url
            .contains("/market/rabactive/ai/image/merge"));
        assert!(upload_request("O", "M")
            .url
            .contains("/market/rabactive/image/upload"));
        for req in [
            hash_left_request(),
            draw_request(),
            upload_request("O", "M"),
        ] {
            assert!(
                !req.url.contains("/ycloud/"),
                "这一族不走 /ycloud/：{}",
                req.url
            );
        }
    }

    #[test]
    fn the_work_list_and_catalog_urls_carry_their_pinned_queries() {
        assert_eq!(
            work_list_request().url,
            "https://m.mcloud.139.com/market/rabactive/image/getWorkList?type=1"
        );
        assert_eq!(
            catalog_full_path_request().url,
            "https://m.mcloud.139.com/market/rabactive/image/catalogFullPath?marketName=rabbityearpic"
        );
    }

    /// 方法逐口：读余量 / 抽奖 / 作品表 / 活动目录是 GET，其余是 POST。
    #[test]
    fn every_method_matches_the_ts_literal() {
        let gets = [
            hash_left_request(),
            draw_request(),
            work_list_request(),
            catalog_full_path_request(),
        ];
        for req in gets {
            assert_eq!(req.method, "GET", "{} 该是 GET", req.url);
        }
        let posts = [
            merge_request("A"),
            status_request("T"),
            upload_request("O", "M"),
            file_list_request("/rab"),
        ];
        for req in posts {
            assert_eq!(req.method, "POST", "{} 该是 POST", req.url);
        }
    }

    // ── 静态 content-type：GET 也发 ──────────────────────────────────

    /// ⚠ 这一条钉的是「与 client 默认行为**不同**」的那一格：
    /// client 只给带体请求补 `content-type`，而旧 TS 的静态头恒发。
    #[test]
    fn bodyless_get_requests_still_carry_the_static_content_type() {
        let reqs = [
            hash_left_request(),
            draw_request(),
            work_list_request(),
            catalog_full_path_request(),
        ];
        for req in reqs {
            assert!(!req.has_body(), "这几枪无体：{}", req.url);
            assert_eq!(
                header(&req, "content-type"),
                Some(AI_AVATAR_CONTENT_TYPE),
                "旧 TS 的静态头在 GET 上也发：{}",
                req.url
            );
        }
    }

    // ── 请求体 ──────────────────────────────────────────────────────

    #[test]
    fn merge_body_is_exactly_the_two_ts_keys() {
        let body = body_json(&merge_request("QUJD"));
        assert_eq!(body["imageBase64"], "QUJD");
        assert_eq!(body["imageExt"], "jpg", "写死的扩展名，别顺手改");
        assert_eq!(body.as_object().expect("该是对象").len(), 2);
    }

    #[test]
    fn status_body_has_only_the_task_id() {
        let body = body_json(&status_request("TASK-1"));
        assert_eq!(body["taskId"], "TASK-1");
        assert_eq!(body.as_object().expect("该是对象").len(), 1);
    }

    #[test]
    fn upload_body_carries_both_images() {
        let body = body_json(&upload_request("ORIGIN", "MERGED"));
        assert_eq!(body["originImage"], "ORIGIN");
        assert_eq!(body["mergeImage"], "MERGED");
        assert_eq!(body.as_object().expect("该是对象").len(), 2);
    }

    #[test]
    fn file_list_body_is_the_five_ts_keys() {
        let body = body_json(&file_list_request("/rab"));
        assert_eq!(body["parentFileId"], "/rab");
        assert_eq!(body["pageInfo"]["pageSize"], 100);
        assert_eq!(
            body["imageThumbnailStyleList"],
            serde_json::json!(["Big", "Small"])
        );
        assert_eq!(body["orderDirection"], "DESC");
        assert_eq!(body["orderBy"], "updated_at", "下划线，不是 updatedAt");
        assert_eq!(body.as_object().expect("该是对象").len(), 5);
    }

    // ── 头画像 ──────────────────────────────────────────────────────

    /// 第八口**恰好**是静态 `content-type` + 4 枚业务头。
    /// 数头数（而不是只查「在不在」）能钉住「没有多补一枚」。
    #[test]
    fn the_file_list_request_carries_exactly_the_static_plus_four_business_headers() {
        let req = file_list_request("/rab");
        assert_eq!(req.headers.len(), 5, "实得：{:?}", req.headers);
        assert_eq!(header(&req, "x-yun-api-version"), Some("v1"));
        assert_eq!(header(&req, "x-yun-app-channel"), Some("10214200"));
        assert_eq!(header(&req, "x-yun-module-type"), Some("100"));
        assert_eq!(header(&req, "x-yun-op-type"), Some("1"));
        // 凭据一律**不在**构造层出现（由 client 按 host 注入）。
        let forbidden = [
            "authorization",
            "jwttoken",
            "deviceid",
            "cookie",
            "x-yun-client-info",
        ];
        for name in forbidden {
            assert!(
                header(&req, name).is_none(),
                "这一族不自己设 {name}（旧 TS 只发那 4 枚）"
            );
        }
    }

    /// 前七口：只有静态 `content-type`，**一枚凭据头都不该有**。
    #[test]
    fn the_rabactive_requests_carry_no_credential_headers() {
        let reqs = [
            hash_left_request(),
            draw_request(),
            merge_request("A"),
            status_request("T"),
            upload_request("O", "M"),
            work_list_request(),
            catalog_full_path_request(),
        ];
        let forbidden = [
            "authorization",
            "jwttoken",
            "deviceid",
            "activityid",
            "appversion",
            "referer",
            "showloading",
            "origin",
        ];
        for req in reqs {
            assert_eq!(
                req.headers.len(),
                1,
                "只该有 content-type：{:?}",
                req.headers
            );
            for name in forbidden {
                assert!(
                    header(&req, name).is_none(),
                    "这一族不发 {name}（旧 TS 的主实例没这些）：{}",
                    req.url
                );
            }
        }
    }

    /// 下载那一枪是**裸 GET**：连静态 `content-type` 都不补
    /// （旧 TS 用的是裸 `fetch`，不走主实例）。
    #[test]
    fn the_image_download_request_is_a_bare_get() {
        let req = image_download_request("https://cdn.example/a.png");
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "https://cdn.example/a.png");
        assert!(
            req.headers.is_empty(),
            "裸 fetch 一枚头都不补：{:?}",
            req.headers
        );
        assert!(!req.has_body());
    }

    // ── 解析：merge / getStatus ─────────────────────────────────────

    #[test]
    fn merge_parses_task_id_and_getstatus_parses_the_three_fields() {
        let merged = parse_task_status(&env_of(serde_json::json!({
            "code": 0, "msg": "success", "result": {"taskId": "T-9"}
        })))
        .unwrap();
        assert_eq!(merged.task_id.as_deref(), Some("T-9"));
        assert_eq!((merged.status, merged.image_url), (None, None));

        let st = parse_task_status(&env_of(serde_json::json!({
            "code": 0, "result": {"taskId": 9, "status": 3, "imageUrl": "https://x/a.png"}
        })))
        .unwrap();
        assert_eq!(
            st.task_id.as_deref(),
            Some("9"),
            "数字 id 也要收（转十进制串）"
        );
        assert_eq!(st.status, Some(3));
        assert_eq!(st.image_url.as_deref(), Some("https://x/a.png"));
    }

    /// 缺 `result` 不报错：三格全空交给调用方判（不是「接口故障」）。
    #[test]
    fn a_missing_result_is_not_an_error_for_the_task_status() {
        let st =
            parse_task_status(&env_of(serde_json::json!({"code": 0, "msg": "success"}))).unwrap();
        assert_eq!(st, AiAvatarTaskStatus::default());
    }

    #[test]
    fn a_non_zero_code_on_merge_is_an_upstream_error() {
        let err = parse_task_status(&env_of(serde_json::json!({
            "code": 500, "msg": "系统繁忙", "result": {}
        })))
        .unwrap_err();
        assert!(err.to_string().contains("500"), "实得：{err}");
        assert!(err.to_string().contains("系统繁忙"), "实得：{err}");
    }

    // ── 解析：getHashLeft ───────────────────────────────────────────

    /// ⚠ 这一条钉的是「`None` **不是** `0`」：读成 0 会让「还能抽」变成「已用完」。
    #[test]
    fn hash_left_distinguishes_missing_from_zero() {
        assert_eq!(
            parse_hash_left(&env_of(serde_json::json!({"code": 0, "result": 0}))).unwrap(),
            Some(0)
        );
        assert_eq!(
            parse_hash_left(&env_of(serde_json::json!({"code": 0, "result": "6"}))).unwrap(),
            Some(6),
            "字符串形式的数字也要收"
        );
        assert_eq!(
            parse_hash_left(&env_of(serde_json::json!({"code": 0, "result": null}))).unwrap(),
            None,
            "上游没给数字 ⇒ None，不是 0"
        );
        assert_eq!(
            parse_hash_left(&env_of(serde_json::json!({"code": 0, "msg": "success"}))).unwrap(),
            None
        );
    }

    // ── 解析：draw 的三态 ───────────────────────────────────────────

    #[test]
    fn draw_408_is_the_stopped_bucket_not_an_error() {
        let out = parse_draw(&env_of(serde_json::json!({
            "code": 408, "msg": "次数已用完", "result": {"anything": 1}
        })))
        .unwrap();
        assert_eq!(out, DrawOutcome::Stopped);
    }

    #[test]
    fn a_won_draw_prefers_content_name_over_prize_name() {
        let both = parse_draw(&env_of(serde_json::json!({
            "code": 0, "result": {"contentName": "云朵", "prizeName": "AI豆"}
        })))
        .unwrap();
        assert_eq!(
            both,
            DrawOutcome::Won {
                prize: Some("云朵".into())
            }
        );

        let only_prize = parse_draw(&env_of(serde_json::json!({
            "code": 0, "result": {"prizeName": "AI豆"}
        })))
        .unwrap();
        assert_eq!(
            only_prize,
            DrawOutcome::Won {
                prize: Some("AI豆".into())
            }
        );

        let none = parse_draw(&env_of(serde_json::json!({"code": 0, "msg": "success"}))).unwrap();
        assert_eq!(none, DrawOutcome::Won { prize: None });
    }

    #[test]
    fn any_other_non_zero_draw_code_is_a_real_error() {
        let err = parse_draw(&env_of(serde_json::json!({
            "code": 500, "msg": "系统繁忙"
        })))
        .unwrap_err();
        assert!(err.to_string().contains("500"), "实得：{err}");
        assert!(err.to_string().contains("系统繁忙"), "实得：{err}");

        // 90001 走信封那条**会话失效**的人话（不是一句干巴巴的 code），
        // 但码本身仍在文本里 —— 「停止」与「重试」的分叉只看 code，不看这句话。
        let expired = parse_draw(&env_of(serde_json::json!({
            "code": 90001, "msg": "未登录"
        })))
        .unwrap_err();
        assert!(expired.to_string().contains("90001"), "实得：{expired}");
    }

    // ── 解析：upload ────────────────────────────────────────────────

    #[test]
    fn upload_id_must_be_a_non_empty_string() {
        assert_eq!(
            parse_upload_id(&env_of(serde_json::json!({"code": 0, "result": "U-1"}))).unwrap(),
            "U-1"
        );
        assert!(parse_upload_id(&env_of(serde_json::json!({"code": 0, "result": ""}))).is_err());
        assert!(parse_upload_id(&env_of(serde_json::json!({"code": 0, "result": 7}))).is_err());
    }

    // ── 解析：作品表 / 活动目录 ─────────────────────────────────────

    #[test]
    fn work_list_skips_entries_without_an_id_and_keeps_the_two_urls() {
        let works = parse_work_list(&env_of(serde_json::json!({
            "code": 0,
            "result": [
                {"id": "W1", "imgUrl": "https://x/p/AAA.png", "qrImgUrl": "https://x/p/BBB.jpg"},
                {"imgUrl": "https://x/p/nope.png"},
                {"id": 2, "imgUrl": "https://x/p/CCC.jpeg"}
            ]
        })))
        .unwrap();
        assert_eq!(works.len(), 2);
        assert_eq!(works[0].id, "W1");
        assert_eq!(works[0].qr_img_url.as_deref(), Some("https://x/p/BBB.jpg"));
        assert_eq!(works[1].id, "2", "数字 id 归一成串");
    }

    #[test]
    fn work_list_without_an_array_is_empty_not_an_error() {
        assert!(
            parse_work_list(&env_of(serde_json::json!({"code": 0, "msg": "ok"})))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn catalog_takes_the_last_non_empty_segment() {
        let pid = parse_catalog_parent_file_id(&env_of(serde_json::json!({
            "code": 0, "result": {"fullPath_sub": "/a//b/rab/"}
        })))
        .unwrap();
        assert_eq!(pid.as_deref(), Some("rab"));
        assert_eq!(
            parse_catalog_parent_file_id(&env_of(serde_json::json!({"code": 0, "result": {}})))
                .unwrap(),
            None
        );
    }

    // ── 解析：hcy/file/list（另一个信封） ────────────────────────────

    #[test]
    fn file_list_parses_items_and_the_cursor() {
        let page = parse_file_list(
            &serde_json::json!({
                "code": "0000", "success": true, "message": "请求成功",
                "data": {
                    "items": [
                        {"fileId": "NEW", "name": "AAA.png", "updatedAt": "2026-01-01T00:00:00+08:00"},
                        {"name": "没有 id"}
                    ],
                    "nextPageCursor": null
                }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            page.items,
            vec![CloudFileEntry {
                file_id: "NEW".into(),
                name: "AAA.png".into()
            }]
        );
        assert_eq!(page.next_page_cursor, None);
    }

    #[test]
    fn file_list_without_items_is_empty() {
        let page =
            parse_file_list(&serde_json::json!({"code": "0000", "success": true}).to_string())
                .unwrap();
        assert!(page.items.is_empty());
    }

    /// ⚠ 与旧 TS 的**刻意分歧**：`success` 假/缺 ⇒ 报错，不是静默空列表。
    #[test]
    fn file_list_requires_success_true() {
        assert!(parse_file_list(
            &serde_json::json!({"code": "0000", "success": false}).to_string()
        )
        .is_err());
        assert!(parse_file_list(&serde_json::json!({"code": "0000"}).to_string()).is_err());
        assert!(parse_file_list(
            &serde_json::json!({"code": "0000", "success": true, "data": {"items": "oops"}})
                .to_string()
        )
        .is_err());
        assert!(parse_file_list("<html>404</html>").is_err());
    }

    // ── 文件名抽取 ──────────────────────────────────────────────────

    #[test]
    fn image_file_name_extraction_matches_the_ts_rule() {
        assert_eq!(
            extract_image_file_name("https://x/p/AAA.png").as_deref(),
            Some("AAA.png")
        );
        assert_eq!(
            extract_image_file_name("https://x/p/A.jpeg").as_deref(),
            Some("A.jpeg")
        );
        assert_eq!(
            extract_image_file_name("https://x/p/A.JPG").as_deref(),
            Some("A.JPG")
        );
        assert_eq!(
            extract_image_file_name("https://x/p/A.png?w=1").as_deref(),
            Some("A.png")
        );
        assert_eq!(
            extract_image_file_name("https://x/p/A.png#f").as_deref(),
            Some("A.png")
        );
        assert_eq!(
            extract_image_file_name("https://x/p/A.gif"),
            None,
            "只认三种后缀"
        );
        assert_eq!(extract_image_file_name("https://x/p/"), None);
        assert_eq!(extract_image_file_name(""), None);
    }
}
