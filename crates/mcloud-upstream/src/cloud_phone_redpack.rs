//! 红包派对（旧 TS 能力名 `cloud-phone-redpack`）—— 协议层。
//!
//! # 这一族为什么单独一个模块
//!
//! 它挂在一个**别的站点**上，跟仓库里已有的族都不共用凭证：
//!
//! | 族 | 站点 | 凭证 |
//! |---|---|---|
//! | 签到族 / 网盘 | `m.mcloud.139.com`、`personal-kd-njs.yun.139.com` | `authorization` / `jwttoken` |
//! | 消息推送 / 邮箱 | `caiyun.feixin.10086.cn` | `jwttoken` |
//! | **红包派对** | **`cpactiv.buy.139.com/cloudphone-market`** | **自己的 `token`** |
//!
//! 那个 `token` 是 `/user/tokenValidate` 用 SSO 票换来的，见
//! [`token_validate_request`] —— 它和网盘的 `authorization`、H5 的 `jwttoken`
//! 是三套东西，**不能互相顶替**。
//!
//! # 依据的级别
//!
//! 本仓对这一族**零抓包**，所以全部依据是旧 TS（优先级 ①，那份 TS 能跑）：
//!
//! - `tasks/cloud-phone-redpack/api.ts` —— 17 条路径与各自的请求体
//! - `tasks/cloud-phone-redpack/task.ts` —— 头画像、签名配方、状态码语义
//! - `packages/shared/src/http/base.ts` —— 这一族**默认**带哪些头
//!
//! 凡从 TS 逐字抄来的，下面都注明出处；凡 TS 里也读不出来的，写「未定」
//! 并说明为什么定不了 —— 不猜，也不拿「等实拍」当不做的理由。
//!
//! # 这一族的信封跟签到族**不是一回事**
//!
//! 签到族判成功看 `code`（`"0000"` / 数值 `0`），红包派对看
//! `String(header.status) === "200"`，错误文案取
//! `data.errorMsg || header.errMsg || header.respMsg`。
//! ⇒ **不能**把它塞进 `SignInEnvelope::parse`（那个会把形态不认识的响应
//! 悄悄判成成功），这里自带 [`RedpackEnvelope`]。
//!
//! # 读写分界
//!
//! 17 条路径里 **11 条只读、6 条写**：写的那 6 条
//! （`userSign` / `userCompleteTask` / `userBrowse` / `userTopicAnswer` /
//! `apkInstall` / `userInstallApp`，外加有副作用的 `getRcsToken`）
//! 在这里**只构造请求**，发不发由编排层决定 —— 协议层不认识「该不该发」。
//! 请求构造函数上都标了「**写**」。

use serde_json::{json, Map, Value};

use mcloud_core::{Error, HttpRequest, Result};
use mcloud_crypto::md5_hex;
use mcloud_crypto::sign_headers;

// ══════════════════════════════ 常量 ══════════════════════════════

/// 这一族的站点。**不在** `endpoints.rs` 的那几张域名表里，也不该加进去：
/// 那几张表是「按域名注入 authorization / jwttoken」用的，而这一族两个都不认。
/// 参考先例是 `app_notice::NOTICE_HOST`（活动站各自持自己的 host）。
pub const REDPACK_HOST: &str = "https://cpactiv.buy.139.com";

/// 全部 17 条路径的前缀（旧 TS 的 `BASE_URL`）。
pub const REDPACK_BASE: &str = "https://cpactiv.buy.139.com/cloudphone-market";

/// 旧 TS 的 `VERSION`。**除 `tokenValidate` 外**每条请求体的 `version` 都是它。
pub const REDPACK_VERSION: &str = "SYS_CONFIG_Y";

/// `tokenValidate` 请求体里的 `version`。
///
/// ⚠ 这一条**故意与其余 16 条不同**：旧 TS 的 `req()` 先写
/// `{ version: VERSION, ...data }`，而 `tokenValidate` 的 `data` 里也有
/// 一个 `version: "1.0"`，对象展开是后面的赢 ⇒ 发出去的是 `"1.0"`。
/// 「顺手统一成 SYS_CONFIG_Y」就会打到一个上游不认的版本上。
pub const REDPACK_TOKEN_VALIDATE_VERSION: &str = "1.0";

/// `appId` 头，同时参与签名。
pub const REDPACK_APP_ID: &str = "12345681";

/// `xChannelSrc` 头，也是 `x-origin` 里那个查询参数。
pub const REDPACK_CHANNEL_SRC: &str = "red-mcloudapp-001";

/// 签名的盐。旧 TS 里就明文写死在源码中，不是运行时下发的。
pub const REDPACK_SIGN_SALT: &str = "e10adc3949ba59abbe56e057f20f883e";

/// `x-origin` 头：活动页地址（不是 `Origin` 头）。
pub const REDPACK_X_ORIGIN: &str = "https://cpactiv.buy.139.com/#/redEnvelopeParty/home?loginType=0&enableShare=1&channelSrc=red-mcloudapp-001";

/// `x-DeviceInfo` 的固定中段。
pub const REDPACK_DEVICE_INFO_HEAD: &str = "|h5|1.0.0|v1.0.0||Mozilla/5.0 (iPad; CPU OS 18_5) AppleWebKit/605.1.15 (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN|";

/// `x-DeviceInfo` 的后半段（屏幕那一截）。
///
/// ⚠ 这一整段是**写死的 iPad 串**，与这一族实际跑在什么设备上无关 ——
/// 旧 TS 就是这么写死的，照抄。别按本机 profile 重拼。
pub const REDPACK_DEVICE_INFO_TAIL: &str = "|||375X667|zh||";

/// `tokenValidate` 请求体里的 `pintype`。
pub const REDPACK_PINTYPE: i64 = 13;

/// `tokenValidate` 请求体里的 `loginConfig`。
pub const REDPACK_LOGIN_CONFIG: &str = "redpacketLoginConfig";

/// 换 SSO 票时用的 `toSourceId`（旧 TS 的第四个实参，别的族各有各的值）。
pub const REDPACK_SSO_SOURCE_ID: &str = "001216";

/// 这一族请求体里的 `platformType`。
pub const REDPACK_PLATFORM_TYPE: i64 = 1;

/// 信封判成功的那个值。
pub const REDPACK_STATUS_OK: &str = "200";

/// 任务状态：未完成（旧 TS `statusText[0]`）。
pub const REDPACK_STATUS_TODO: i64 = 0;
/// 任务状态：已完成。
pub const REDPACK_STATUS_DONE: i64 = 1;
/// 任务状态：奖品已兑完。
pub const REDPACK_STATUS_SOLD_OUT: i64 = 3;
/// 任务状态：明天再来。
pub const REDPACK_STATUS_TOMORROW: i64 = 4;

/// 把状态码翻成旧 TS 的文案。**未知码回 `None`**。
///
/// ⚠ 刻意回 `Option` 而不是给一句兜底：编排层要能把「不认识的状态」
/// 单独计一档，不能让 `None` 悄悄落进「未完成」里 —— 那就等于替上游下结论。
pub fn redpack_status_text(status: i64) -> Option<&'static str> {
    match status {
        REDPACK_STATUS_TODO => Some("未完成"),
        REDPACK_STATUS_DONE => Some("已完成"),
        REDPACK_STATUS_SOLD_OUT => Some("奖品已兑完"),
        REDPACK_STATUS_TOMORROW => Some("明天再来"),
        _ => None,
    }
}
// ══════════════════════════════ 签名与头 ══════════════════════════════

/// 旧 TS 的签名配方：`md5(requestId + appId + token + salt + timestamp)`。
///
/// ⚠ **五项的顺序就是配方本身**，且中间**没有任何分隔符** —— 少一项、
/// 换一次序、插一个换行，都会得到一个格式完全合法、上游直接拒的签名。
/// 所以它是本文件里最该被钉死的函数，测试用一条**外部算出来的**摘要钉它
/// （不是拿 `md5_hex` 自证）。
pub fn redpack_sign(request_id: &str, token: &str, timestamp_ms: i64) -> String {
    let concat = format!("{request_id}{REDPACK_APP_ID}{token}{REDPACK_SIGN_SALT}{timestamp_ms}");
    md5_hex(&concat)
}

/// 一次调用要带的那几样「每次现算」的东西。
///
/// `token` 放在这里（而不是从别处读）是**故意的**：`tokenValidate` 那一枪
/// 的 `token` 是空串，而**签名也用同一个空串算**（旧 TS `getHeaders("")`）。
/// 一个字段同时喂签名和头，就不可能出现「签名用一个值、头用另一个值」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedpackCall {
    /// 这一族自己的 `token`；换票那一枪是空串。
    pub token: String,
    /// `Date.now()`，毫秒。
    pub timestamp_ms: i64,
    /// `requestId`，参与签名。
    pub request_id: String,
    /// `x-DeviceInfo` 里那个 16 位随机串。
    pub device_nonce: String,
}

impl RedpackCall {
    /// 组装一次调用。
    pub fn new(
        token: impl Into<String>,
        timestamp_ms: i64,
        request_id: impl Into<String>,
        device_nonce: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            timestamp_ms,
            request_id: request_id.into(),
            device_nonce: device_nonce.into(),
        }
    }

    /// 这一枪的 `sign` 头。token 取自本结构，见类型注释。
    pub fn signature(&self) -> String {
        redpack_sign(&self.request_id, &self.token, self.timestamp_ms)
    }

    /// `x-DeviceInfo` 的完整值。
    ///
    /// ⚠ 中间那截是 `{timestamp}{nonce}` **直接拼、没有分隔符**，与末尾的
    /// `|||` 是两回事 —— 别顺手补一个分隔符「让它更好看」。
    pub fn device_info(&self) -> String {
        format!(
            "{REDPACK_DEVICE_INFO_HEAD}{}{}{REDPACK_DEVICE_INFO_TAIL}",
            self.timestamp_ms, self.device_nonce
        )
    }
}

/// 给这一族的 **_所有_** 请求补上头画像。
///
/// # 哪些头**不**在这里设，以及为什么
///
/// - `content-type`：旧 TS 的 `ctx.http` 默认就补
///   `application/json;charset=UTF-8`（`packages/shared/src/http/base.ts:30`），
///   而本仓客户端在调用方没声明时补的
///   [`CONTENT_TYPE_JSON`](crate::client::CONTENT_TYPE_JSON) **也是这一串**
///   ⇒ 两边一致，所以这里**不写**。⚠ 若哪天本仓的默认值改了，
///   `content_type_default_matches_the_old_ts` 那条测试会红。
/// - `user-agent`：旧 TS 的默认 UA 是那串 Android WebView UA；本仓的 UA
///   归 profile（`effective_user_agent()`），在这儿加会是**第二根**。
///   要复现参考行为，应在**组装层**把 profile 的 UA 设成那一串。
/// - `authorization` / `jwttoken`：旧 TS 按域名注入，而
///   `cpactiv.buy.139.com` **两个名单都不在** ⇒ 这一族压根不带。
///   本仓同理由 `ReqwestClient` 的域名表决定，这里补会是**第二根**。
/// - `deviceid`：旧 TS 只在 `createHttp` 传了 `options.deviceid` 时才给
///   **所有**请求加它；那一格由组装层决定，不归这里。
/// - `cookie`：旧 TS 有 CookieJar，本仓没有。这一族是否需要 cookie
///   **没有样本** —— 未定，此处不造。
fn redpack_headers(req: HttpRequest, call: &RedpackCall) -> HttpRequest {
    req.header("appId", REDPACK_APP_ID)
        .header("token", call.token.as_str())
        .header("sign", call.signature())
        .header("timestamp", call.timestamp_ms.to_string())
        .header("requestId", call.request_id.as_str())
        .header("xChannelSrc", REDPACK_CHANNEL_SRC)
        .header("x-DeviceInfo", call.device_info())
        .header("x-origin", REDPACK_X_ORIGIN)
        .header("Referer", format!("{REDPACK_HOST}/"))
        .header("Origin", REDPACK_HOST)
}
// ══════════════════════════════ 请求 ══════════════════════════════

/// 把 `{ version: VERSION, ...rest }` 拼成请求体。
///
/// ⚠ `version` 放第一位、且**允许被 `rest` 覆盖** —— 这是照抄旧 TS 的
/// 对象展开语义（`{ version: VERSION, ...data }`）。`tokenValidate` 正是
/// 靠这一条把版本顶成 `"1.0"` 的，见 [`REDPACK_TOKEN_VALIDATE_VERSION`]。
///
/// 说明：`serde_json` 的 `Map` 默认按键排序，所以**请求体的字节顺序**
/// 与旧 TS 不一致。这一族的上游按 JSON 解析（不是按字节比对），
/// 测试因此也比对 `Value` 而不是比对字符串 —— 不假装字节级复刻。
fn redpack_body(rest: Value) -> Value {
    let mut map = Map::new();
    map.insert(
        "version".to_string(),
        Value::String(REDPACK_VERSION.to_string()),
    );
    if let Value::Object(rest) = rest {
        for (k, v) in rest {
            map.insert(k, v);
        }
    }
    Value::Object(map)
}

fn redpack_post(path: &str, body: Value, call: &RedpackCall) -> HttpRequest {
    let req = HttpRequest::post(format!("{REDPACK_BASE}{path}")).json_body(&body);
    redpack_headers(req, call)
}

/// 换 SSO 票那一步的路径（旧 TS 的 querySpecTokenNew）。
///
/// ⚠ 它跟网盘会话的 querySpecTokenV2 是**两条不同的路**：那条是
/// POST user-njs.yun.139.com/user/querySpecTokenV2，这条是 GET m.mcloud.139.com。
/// 混用的表现是 404/405，不是「换个域名还能用」。
pub const REDPACK_SPEC_TOKEN_PATH: &str = "/ycloud/api/cloud/userdomain/v2/querySpecToken";

/// 换 SSO 票时用的签名盐序号（旧 TS：getSignHeader(undefined, 2)）。
///
/// TS 里那两个盐只有 1/2 两个值，这一族用的是 2，别按签到族的 1 写。
pub const REDPACK_SPEC_TOKEN_SALT_KEY: u8 = 2;

/// GET querySpecToken —— 会话第 ① 段：拿 SSO 票。**只读**
///
/// 这一枪**不带** appId/token/sign 那十个头，也不带 authorization：
/// 它本身就是「还没有凭证」时打的那一枪，头只有四个 x-*（签名头）。
///
/// 随机量（x-request-id / x-nonce）由 [sign_headers] 内部产生 ⇒ 同一个
/// timestamp_ms 调两次，签名也不同。要复现签名的测试用 sign_headers_with。
pub fn spec_token_request(timestamp_ms: i64, source_id: &str) -> HttpRequest {
    let url = format!(
        "{}{REDPACK_SPEC_TOKEN_PATH}?targetSourceId={source_id}&hideLoading=true",
        crate::endpoints::MCLOUD_HOST
    );
    let mut req = HttpRequest::get(url);
    for (name, value) in sign_headers(timestamp_ms, REDPACK_SPEC_TOKEN_SALT_KEY, "").to_pairs() {
        req = req.header(name, value);
    }
    req
}

/// 解析 querySpecToken 的响应，取 result 里的 SSO 票。
///
/// 响应形状只有三个字段：code / msg / result，而 result **本身就是**票字符串
/// （不是网盘那种 data.token 嵌套）。
///
/// ⚠ 旧 TS 只判 result 真假、**不判 code** —— 这里照做：code 非 0 但 result
/// 有值仍然算拿到了票；只是 code/msg 会被带进错误文案，好让「没拿到」的时候
/// 能看出上游到底说了什么。
pub fn parse_spec_token(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("querySpecToken 响应不是 JSON：{e}")))?;
    match v.get("result").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => Err(Error::Parse(format!(
            "querySpecToken 没给 result：{}",
            spec_token_reason(&v)
        ))),
    }
}

/// 把 querySpecToken 的 code/msg 摘成一句，只用于报错文案。
fn spec_token_reason(v: &Value) -> String {
    let code = v
        .get("code")
        .map(Value::to_string)
        .unwrap_or_else(|| "无".to_string());
    let msg = v.get("msg").and_then(Value::as_str).unwrap_or("无");
    format!("code={code} msg={msg}")
}

/// `POST /user/tokenValidate` —— 用 SSO 票换这一族自己的 token。
///
/// ⚠ 这一枪的 `token` 头是**空串**（旧 TS `getHeaders("")`），签名也按空串算。
/// 传进来的 `sso_token` 是**请求体**里的字段，不是头。
pub fn token_validate_request(call: &RedpackCall, sso_token: &str, device_id: &str) -> HttpRequest {
    redpack_post(
        "/user/tokenValidate",
        redpack_body(json!({
            "version": REDPACK_TOKEN_VALIDATE_VERSION,
            "pintype": REDPACK_PINTYPE,
            "token": sso_token,
            "deviceId": device_id,
            "loginConfig": REDPACK_LOGIN_CONFIG,
        })),
        call,
    )
}

/// `POST /redpacket/configTaskLoginList` —— 三个任务组 + 每日签到那一格。**只读**
pub fn task_login_list_request(call: &RedpackCall) -> HttpRequest {
    redpack_post(
        "/redpacket/configTaskLoginList",
        redpack_body(json!({})),
        call,
    )
}

/// `POST /redpacket/userAccountInfo` —— 余额。**只读**
pub fn account_info_request(call: &RedpackCall) -> HttpRequest {
    redpack_post(
        "/redpacket/userAccountInfo",
        redpack_body(json!({ "platformType": REDPACK_PLATFORM_TYPE })),
        call,
    )
}

/// `POST /redpacket/userToastInfo` —— 某个任务最近一次拿到的奖。**只读**
pub fn toast_info_request(call: &RedpackCall, task_code: &str) -> HttpRequest {
    redpack_post(
        "/redpacket/userToastInfo",
        redpack_body(json!({ "configTaskCode": task_code })),
        call,
    )
}

/// `POST /redpacket/configTopicList` —— 题库。**只读**
pub fn topic_list_request(call: &RedpackCall) -> HttpRequest {
    redpack_post("/redpacket/configTopicList", redpack_body(json!({})), call)
}

/// `POST /thirdapi/huawei/thirdQueryUserRelationshipV2` —— 云机列表。**只读**
///
/// ⚠ 这个路径是**华为**那边的第三方接口名，看着像别人的域，其实挂在
/// `cpactiv` 站下 —— 照抄，别「顺手改成」`/redpacket/...`。
pub fn cloud_phones_request(call: &RedpackCall, status: RedpackPhoneStatus) -> HttpRequest {
    redpack_post(
        "/thirdapi/huawei/thirdQueryUserRelationshipV2",
        redpack_body(json!({
            "pageNumber": 1,
            "pageSize": 10,
            "status": status.as_str(),
        })),
        call,
    )
}

/// `POST /redpacket/configAppList` —— 可装的应用列表。**只读**
pub fn app_list_request(call: &RedpackCall) -> HttpRequest {
    redpack_post(
        "/redpacket/configAppList",
        redpack_body(json!({ "platformType": REDPACK_PLATFORM_TYPE })),
        call,
    )
}

/// `POST /user/gethwToken` —— 拿华为侧的 `hwToken`（装应用要用）。**只读**
pub fn hw_token_request(call: &RedpackCall) -> HttpRequest {
    redpack_post("/user/gethwToken", redpack_body(json!({})), call)
}

/// `POST /app/activity/instanceApkInstallStatus` —— 某台云机上某个包装没装。**只读**
pub fn install_status_request(
    call: &RedpackCall,
    hw_token: &str,
    instance_id: &str,
    package_name: &str,
) -> HttpRequest {
    redpack_post(
        "/app/activity/instanceApkInstallStatus",
        redpack_body(json!({
            "hwToken": hw_token,
            "apkList": [package_name],
            "instanceIdList": [instance_id],
        })),
        call,
    )
}

/// `POST /app/activity/appInstallTaskStatus` —— 装应用的异步任务进度。**只读**
pub fn install_task_status_request(
    call: &RedpackCall,
    install_task_id: &str,
    hw_token: &str,
) -> HttpRequest {
    redpack_post(
        "/app/activity/appInstallTaskStatus",
        redpack_body(json!({
            "taskId": install_task_id,
            "hwToken": hw_token,
        })),
        call,
    )
}

/// `POST /redpacket/userSign` —— 每日签到。**写**
pub fn user_sign_request(call: &RedpackCall) -> HttpRequest {
    redpack_post(
        "/redpacket/userSign",
        redpack_body(json!({ "platformType": REDPACK_PLATFORM_TYPE })),
        call,
    )
}

/// `POST /redpacket/userCompleteTask` —— 登记某个任务完成。**写**
///
/// `task_id` 收 `Value` 而不是函数式地取整：旧 TS 把 `task.id` **原样**
/// 发回上游，而这一族的 `id` 既见过数也见过串 ⇒ 由我们改成数，
/// 就是替上游重写它自己的类型。
pub fn complete_task_request(call: &RedpackCall, task_id: &Value) -> HttpRequest {
    redpack_post(
        "/redpacket/userCompleteTask",
        redpack_body(json!({
            "platformType": REDPACK_PLATFORM_TYPE,
            "taskId": task_id,
        })),
        call,
    )
}

/// `POST /redpacket/userBrowse` —— 报送「浏览过了」。**写**
pub fn browse_request(call: &RedpackCall, task_code: &str) -> HttpRequest {
    redpack_post(
        "/redpacket/userBrowse",
        redpack_body(json!({ "taskCode": task_code })),
        call,
    )
}

/// `POST /redpacket/userTopicAnswer` —— 交答案。**写**
///
/// ⚠ `topic_id` 在这里被取整（旧 TS `Number(topic.id)`），
/// 与 [`complete_task_request`] **刻意不一致** —— 那边是原样透传。
/// 两处都是照抄，不要「统一一下」。
pub fn answer_topic_request(
    call: &RedpackCall,
    task_id: i64,
    topic_id: i64,
    answer: &str,
) -> HttpRequest {
    redpack_post(
        "/redpacket/userTopicAnswer",
        redpack_body(json!({
            "taskId": task_id,
            "topicId": topic_id,
            "answer": answer,
            "platformType": REDPACK_PLATFORM_TYPE,
        })),
        call,
    )
}

/// `POST /user/getRcsToken` —— 为「进云机真用一下」换的票。**有副作用**
///
/// 归在写侧：旧 TS 只在真的要宣称「已在云机里用过」时才调它。
pub fn rcs_token_request(call: &RedpackCall, target_source_id: &str) -> HttpRequest {
    redpack_post(
        "/user/getRcsToken",
        redpack_body(json!({ "targetSourceId": target_source_id })),
        call,
    )
}

/// `POST /app/activity/apkInstall` —— 让云机装这个包。**写**
///
/// ⚠ `appId` 取的是 `String(app.packageId)`（**应用列表里的 packageId**），
/// 而不是 `app.id` —— 隔壁 [`report_install_app_request`] 用的才是 `app.id`。
/// 两个 id 长得像、来源不同，抄错一个会在云机上装错包。
pub fn apk_install_request(
    call: &RedpackCall,
    instance_id: &str,
    package_id: &str,
    hw_token: &str,
) -> HttpRequest {
    redpack_post(
        "/app/activity/apkInstall",
        redpack_body(json!({
            "instanceId": instance_id,
            "appId": package_id,
            "hwToken": hw_token,
        })),
        call,
    )
}

/// `POST /redpacket/userInstallApp` —— 领「装应用」这个任务的奖。**写**
///
/// `existed` 只在旧 TS 明确传了值的时候才进请求体（`1` = 本来就装过），
/// 所以它是 `Option`：`None` **不带这个键**，而不是发一个 `null`。
pub fn report_install_app_request(
    call: &RedpackCall,
    task_id: &Value,
    app_id: &Value,
    existed: Option<i64>,
) -> HttpRequest {
    let mut body = json!({
        "taskId": task_id,
        "appId": app_id,
        "platformType": REDPACK_PLATFORM_TYPE,
    });
    if let (Some(obj), Some(v)) = (body.as_object_mut(), existed) {
        obj.insert("existed".to_string(), json!(v));
    }
    redpack_post("/redpacket/userInstallApp", redpack_body(body), call)
}

/// 查云机列表时那一格 `status`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedpackPhoneStatus {
    /// 生效中。
    Active,
    /// 待生效。
    Pending,
}

impl RedpackPhoneStatus {
    /// 线上那个字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Pending => "PENDING",
        }
    }
}
// ══════════════════════════════ 信封 ══════════════════════════════

/// 红包派对那一族的信封：`{ header: { status, errMsg, respMsg }, data, errorMsg }`。
///
/// 与签到族信封的**关键分歧**在判成功的那一格：那边是 `code`，这边是
/// `header.status`。形态不认识的响应在这里**必须报错**，不能默认成功。
#[derive(Debug, Clone, PartialEq)]
pub struct RedpackEnvelope {
    /// `header.status`。留 `Value` 是**故意的**：旧 TS 判的是
    /// `String(status) === "200"`，即数值 `200` 与串 `"200"` 都算成功，
    /// 而哪种形态真的会出现**没有样本** ⇒ 不在这里替上游归一。
    pub status: Option<Value>,
    /// 顶层的 `errorMsg`（**不是** `header` 里的那个）。
    pub error_msg: Option<String>,
    /// `header.errMsg`。
    pub err_msg: Option<String>,
    /// `header.respMsg`。
    pub resp_msg: Option<String>,
    /// 顶层 `data`。**只挪位置、不做类型判断** —— 形状由各解析器管。
    pub payload: Option<Value>,
}

impl RedpackEnvelope {
    /// 解析。顶层不是对象就报错。
    ///
    /// ⚠ `header` **缺了不算错**（旧 TS 走到 `undefined?.status` ⇒ 判失败，
    /// 而不是抛）⇒ 这里也照判失败，由 [`Self::is_ok`] 决定。
    pub fn parse(body: &str) -> Result<Self> {
        let raw: Value = serde_json::from_str(body)
            .map_err(|e| Error::Parse(format!("红包派对响应不是 JSON: {e}")))?;
        let obj = raw
            .as_object()
            .ok_or_else(|| Error::Parse("红包派对响应不是 JSON 对象".to_string()))?;
        let header = obj.get("header").and_then(Value::as_object);
        Ok(Self {
            status: header.and_then(|h| h.get("status")).cloned(),
            err_msg: header.and_then(|h| h.get("errMsg")).and_then(non_empty_str),
            resp_msg: header
                .and_then(|h| h.get("respMsg"))
                .and_then(non_empty_str),
            error_msg: obj.get("errorMsg").and_then(non_empty_str),
            payload: obj.get("data").cloned(),
        })
    }

    /// `String(status) === "200"`。
    pub fn is_ok(&self) -> bool {
        matches!(
            self.status.as_ref().map(value_text).as_deref(),
            Some(REDPACK_STATUS_OK)
        )
    }

    /// 旧 TS 的 `errorMsg()`：`data.errorMsg || header.errMsg || header.respMsg || fallback`。
    ///
    /// ⚠ 空串按 `||` 的语义**当作没有**（会往后落），这是 JS 的怪癖，
    /// 但它是线上真实行为 —— 照抄。
    pub fn error_text(&self, fallback: &str) -> String {
        self.error_msg
            .as_ref()
            .or(self.err_msg.as_ref())
            .or(self.resp_msg.as_ref())
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }
}

/// 解析并**要求成功**，回信封本体（`payload` 从它上面取）。
///
/// ⚠ 失败一律回 [`Error::Upstream`]，**不映射 AuthExpired**：
/// 这一族「票过期了回什么码」**没有样本**，凭猜去映射会让编排层
/// 走一条凭空发明的重建会话路径。宁可让上层看到原始 status。
fn ok_envelope(body: &str, what: &str) -> Result<RedpackEnvelope> {
    let env = RedpackEnvelope::parse(body)?;
    if !env.is_ok() {
        let code = env
            .status
            .as_ref()
            .map(value_text)
            .unwrap_or_else(|| "-".to_string());
        return Err(Error::Upstream {
            code,
            message: env.error_text(what),
        });
    }
    Ok(env)
}

fn ok_payload(body: &str, what: &str) -> Result<Value> {
    let env = ok_envelope(body, what)?;
    env.payload
        .ok_or_else(|| Error::Parse(format!("{what}: 响应里没有 data")))
}
// ══════════════════════════════ 响应 ══════════════════════════════

/// `configTaskSignList` 的一项：每日签到那一格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedpackSignDay {
    /// `isToday`：`1` 表示这一格是今天的。
    pub is_today: Option<i64>,
    /// 见 [`redpack_status_text`]。
    pub status: Option<i64>,
    /// 签到奖励，**单位是分**，这里不换算。
    pub sign_amount: Option<i64>,
}

/// 三个任务组里的一项。
#[derive(Debug, Clone, PartialEq)]
pub struct RedpackTask {
    /// 见 [`complete_task_request`] 的说明 —— 原样存，原样发回。
    pub id: Option<Value>,
    /// 任务码，是这一族的**分派键**（`NOVICE_2` / `MONTHLY_3` …）。
    pub task_code: Option<String>,
    /// 任务名。
    pub task_name: Option<String>,
    /// 旧 TS 读它时是 `Number(userStatus ?? 0)` ⇒ 缺失按 `0`（未完成）。
    pub user_status: Option<i64>,
    /// 奖品金额，**单位是分**。
    pub prize_amount: Option<i64>,
    /// 任务金额，**单位是分**。
    pub task_amount: Option<i64>,
}

/// 任务总表：签到格 + 三组任务。
///
/// ⚠ 某一组为空 **≠** 没事做（可能只是「这一期没配」）⇒ 本层只搬数，
/// 不替编排层下结论。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RedpackTaskList {
    /// `configTaskSignList`
    pub sign: Vec<RedpackSignDay>,
    /// `configTaskNoviceList`
    pub novice: Vec<RedpackTask>,
    /// `configTaskDailyList`
    pub daily: Vec<RedpackTask>,
    /// `configTaskMonthlyList`
    pub monthly: Vec<RedpackTask>,
}

impl RedpackTaskList {
    /// 按任务码找一项，扫描顺序照抄旧 TS 的 `findTask`：
    /// 新手 → 每日 → 每月。
    pub fn find(&self, task_code: &str) -> Option<&RedpackTask> {
        self.novice
            .iter()
            .chain(self.daily.iter())
            .chain(self.monthly.iter())
            .find(|t| t.task_code.as_deref() == Some(task_code))
    }
}

/// 余额。两个数都是**分**。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedpackAccount {
    /// `info.canAmount` —— 可用。
    pub can_amount: Option<i64>,
    /// `info.totalAmount` —— 累计。
    pub total_amount: Option<i64>,
}

/// 某个任务最近一次拿到的奖。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedpackPrize {
    /// `lastUserPrize.prizeName`
    pub prize_name: Option<String>,
    /// `lastUserPrize.prizeAmount`，**分**。
    ///
    /// ⚠ 旧 TS 在 `!prize.prizeAmount` 时**什么都不打** —— 即「0 或没有」
    /// 等同「这格没奖可报」。这里照旧只搬数，让编排层照着判。
    pub prize_amount: Option<i64>,
}

/// 题库里的一道题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedpackTopic {
    /// 题目 id。原样存，取整由编排层在发答案时做。
    pub id: Option<Value>,
    /// 题干。
    pub topic_content: Option<String>,
    /// `topicOption` 是一个**装着 JSON 数组的字符串**（不是数组本身）。
    /// 用 [`parse_topic_options`] 解，别直接当数组读。
    pub topic_option: Option<String>,
}

/// 解析 `topicOption` 那个「串里套数组」。
///
/// ⚠ 解不开就回**空数组**（旧 TS 的 `catch` 分支）—— 不是报错。
/// 这一格是活动配的题面，配坏了不该让整条链断掉；
/// 编排层拿到空数组自然就走「未知题目 ⇒ 需手动完成」。
pub fn parse_topic_options(topic_option: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(topic_option) {
        Ok(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// 会话第 ② 段的响应：`/user/tokenValidate` 换回来的这一族自己的 token。
///
/// 它跟第 ① 段那张 SSO 票**不是一回事**：SSO 票是请求体里的入参，这个 token
/// 是往后每一条请求头里的 `token`。旧 TS 存在 `ctx.state.cloudPhoneRedpackToken`
/// 里，下一轮直接复用 —— 本仓没有 "state" 那个槽，所以由调用方自己决定存哪。
///
/// # 为什么判空算错而不是返回空串
///
/// 空 token 拿去打后面任何一条，上游只会回一个业务码；那时要排查的是
/// "为什么票是空的"，而不是"为什么任务没做成"。在这里就断掉，错误才指向真原因。
pub fn parse_token_validate(body: &str) -> Result<String> {
    let payload = ok_payload(body, "红包派对换取 token")?;
    payload
        .get("token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Parse("红包派对换取 token：响应里没有 data.token".to_string()))
}

/// `/redpacket/configTaskLoginList` 的响应。
pub fn parse_task_login_list(body: &str) -> Result<RedpackTaskList> {
    let payload = ok_payload(body, "红包派对任务列表")?;
    Ok(RedpackTaskList {
        sign: optional_array(&payload, "configTaskSignList")?
            .iter()
            .map(|v| RedpackSignDay {
                is_today: opt_int_at(v, "isToday"),
                status: opt_int_at(v, "status"),
                sign_amount: opt_int_at(v, "signAmount"),
            })
            .collect(),
        novice: parse_tasks(&payload, "configTaskNoviceList")?,
        daily: parse_tasks(&payload, "configTaskDailyList")?,
        monthly: parse_tasks(&payload, "configTaskMonthlyList")?,
    })
}

fn parse_tasks(payload: &Value, key: &str) -> Result<Vec<RedpackTask>> {
    Ok(optional_array(payload, key)?
        .iter()
        .map(|v| RedpackTask {
            id: v.get("id").cloned(),
            task_code: opt_str_at(v, "taskCode"),
            task_name: opt_str_at(v, "taskName"),
            user_status: opt_int_at(v, "userStatus"),
            prize_amount: opt_int_at(v, "prizeAmount"),
            task_amount: opt_int_at(v, "taskAmount"),
        })
        .collect())
}

/// `/redpacket/userAccountInfo` 的响应。
pub fn parse_account_info(body: &str) -> Result<RedpackAccount> {
    let payload = ok_payload(body, "红包派对余额")?;
    let info = payload.get("info").unwrap_or(&Value::Null);
    Ok(RedpackAccount {
        can_amount: opt_int_at(info, "canAmount"),
        total_amount: opt_int_at(info, "totalAmount"),
    })
}

/// `/redpacket/userToastInfo` 的响应。
pub fn parse_toast_info(body: &str) -> Result<RedpackPrize> {
    let payload = ok_payload(body, "红包派对奖励")?;
    let prize = payload.get("lastUserPrize").unwrap_or(&Value::Null);
    Ok(RedpackPrize {
        prize_name: opt_str_at(prize, "prizeName"),
        prize_amount: opt_int_at(prize, "prizeAmount"),
    })
}

/// `/redpacket/configTopicList` 的响应。`list` 缺失算空。
pub fn parse_topic_list(body: &str) -> Result<Vec<RedpackTopic>> {
    let payload = ok_payload(body, "红包派对题库")?;
    Ok(optional_array(&payload, "list")?
        .iter()
        .map(|v| RedpackTopic {
            id: v.get("id").cloned(),
            topic_content: opt_str_at(v, "topicContent"),
            topic_option: opt_str_at(v, "topicOption"),
        })
        .collect())
}

/// `/thirdapi/huawei/thirdQueryUserRelationshipV2` 的响应：云机原始记录。
///
/// # 为什么回 `Value` 而不是一个结构体
///
/// 这一格的**嵌套形状本身就怪**，旧 TS 得先判一次类型：
/// `data.data?.data` 是对象就用它，否则用 `data.data` —— 这里照抄那一步。
/// 而「每台云机的哪个字段是实例号 / 哪个是锁屏位」是**编排层**的事
/// （它要跟装应用那条链一起读），本层不替它编字段名。
pub fn parse_cloud_phones(body: &str) -> Result<Vec<Value>> {
    let payload = ok_payload(body, "红包派对云机列表")?;
    let nested = payload
        .get("data")
        .filter(|v| v.is_object())
        .unwrap_or(&payload);
    Ok(optional_array(nested, "subscribeDetails")?.to_vec())
}

/// `/redpacket/configAppList` 的响应：应用原始记录。
///
/// 同 [`parse_cloud_phones`]：字段名归编排层，本层只搬。
pub fn parse_app_list(body: &str) -> Result<Vec<Value>> {
    let payload = ok_payload(body, "红包派对应用列表")?;
    Ok(optional_array(&payload, "list")?.to_vec())
}

/// `/user/gethwToken` 的响应。
pub fn parse_hw_token(body: &str) -> Result<Option<String>> {
    let payload = ok_payload(body, "红包派对 hwToken")?;
    Ok(opt_str_at(&payload, "hwToken"))
}

/// 写侧那几枪的通用形状：`data.status`（缺失或 `null` 就是 `None`）。
///
/// ⚠ 回 `None` **不等于失败**：旧 TS 的判据是
/// `!isNil(status) && status !== 1` 才算失败 ⇒ 缺失是**放行**。
/// 所以这里只报「读到了什么」，判成功由调用方按那条规则做。
pub fn parse_write_status(body: &str) -> Result<Option<i64>> {
    let payload = ok_payload(body, "红包派对写操作")?;
    Ok(opt_int_at(&payload, "status"))
}

// ══════════════════════════════ 小工具 ══════════════════════════════

/// 缺失 / `null` ⇒ 空数组；在、却不是数组 ⇒ 报错。
///
/// ⚠ 后半句才是重点：任务表是**判据**。上游换了形状而我们静默当空，
/// 会把「这一期有任务」读成「没任务」—— 那比报错更危险。
fn optional_array<'a>(payload: &'a Value, key: &str) -> Result<&'a [Value]> {
    match payload.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(Error::Parse(format!("红包派对 {key} 不是数组"))),
    }
}

/// 把 `Value` 按 JS `String()` 的语义转成文本（判 `"200"` 用）。
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// 非空串才算有值 —— 对应 JS `||` 把 `""` 当假。
fn non_empty_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// 读一个整数：数或**数字串**都认（上游这两种都出现过），其余回 `None`。
///
/// ⚠ 缺失**绝不补齐成 0** —— 「0」和「读不到」在这一族是两件事
/// （`userStatus: 0` 就是「未完成」，而读不到是「不知道」）。
fn opt_int_at(v: &Value, key: &str) -> Option<i64> {
    let raw = v.get(key)?;
    if let Some(n) = raw.as_i64() {
        return Some(n);
    }
    raw.as_str().and_then(|s| s.trim().parse::<i64>().ok())
}

fn opt_str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(non_empty_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> RedpackCall {
        RedpackCall::new("TOK", 1_700_000_000_000, "RID1", "NONCE16CHARS0000")
    }

    fn h(req: &HttpRequest, name: &str) -> Option<String> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    fn header_count(req: &HttpRequest, name: &str) -> usize {
        req.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .count()
    }

    fn body_of(req: &HttpRequest) -> Value {
        serde_json::from_slice(req.body.as_deref().expect("这一族每条都带体")).unwrap()
    }

    /// 这一族发的每一枪都在这里列一遍。加端点时**必须**同时进这张表。
    fn every_request(c: &RedpackCall) -> Vec<(&'static str, HttpRequest)> {
        vec![
            ("tokenValidate", token_validate_request(c, "SSO", "DEV")),
            ("configTaskLoginList", task_login_list_request(c)),
            ("userAccountInfo", account_info_request(c)),
            ("userToastInfo", toast_info_request(c, "SIGN_1")),
            ("configTopicList", topic_list_request(c)),
            (
                "thirdQueryUserRelationshipV2",
                cloud_phones_request(c, RedpackPhoneStatus::Active),
            ),
            ("configAppList", app_list_request(c)),
            ("gethwToken", hw_token_request(c)),
            (
                "instanceApkInstallStatus",
                install_status_request(c, "HW", "INST", "com.example.app"),
            ),
            (
                "appInstallTaskStatus",
                install_task_status_request(c, "T9", "HW"),
            ),
            ("userSign", user_sign_request(c)),
            ("userCompleteTask", complete_task_request(c, &json!(7))),
            ("userBrowse", browse_request(c, "NOVICE_2")),
            ("userTopicAnswer", answer_topic_request(c, 7, 11, "B")),
            ("getRcsToken", rcs_token_request(c, "0")),
            ("apkInstall", apk_install_request(c, "INST", "PKG1", "HW")),
            (
                "userInstallApp",
                report_install_app_request(c, &json!(7), &json!(3), None),
            ),
        ]
    }

    #[test]
    fn signature_matches_the_old_ts_recipe() {
        // ⭐ 照旧 TS `md5(requestId + APP_ID + token + SALT + timestamp)` 算的：
        //    requestId=RID1, token=TOK, timestamp=1700000000000
        //    ⇒ RID1 + 12345681 + TOK + e10adc... + 1700000000000
        // 摘要由**外部 md5 工具**（macOS `md5`）算出，不是拿 md5_hex 自证。
        let expected = "90b072f712c9199dca87f89c4677406e";
        assert_eq!(call().signature(), expected);
    }

    #[test]
    fn signature_recipe_has_no_separators() {
        // 五项直接拼。构造一条「插了分隔符」的串，断言摘要**不同** ——
        // 这条测试钉的是「没有分隔符」这件事本身。
        let with_sep = md5_hex("RID1|12345681|TOK|e10adc3949ba59abbe56e057f20f883e|1700000000000");
        assert_ne!(call().signature(), with_sep);
    }

    #[test]
    fn token_validate_signs_with_the_empty_token_it_sends() {
        let anon = RedpackCall::new("", 1_700_000_000_000, "RID1", "NONCE16CHARS0000");
        let req = token_validate_request(&anon, "SSO", "DEV");
        assert_eq!(
            h(&req, "token").as_deref(),
            Some(""),
            "换票那一枪的 token 头是空串"
        );
        let expected = redpack_sign("RID1", "", 1_700_000_000_000);
        assert_eq!(h(&req, "sign").as_deref(), Some(expected.as_str()));
        // 与带票时的签名**必须不同**，否则说明 token 根本没进签名
        assert_ne!(expected, call().signature());
    }

    #[test]
    fn token_validate_body_overrides_version_to_one_dot_zero() {
        let body = body_of(&token_validate_request(&call(), "SSO", "DEV"));
        assert_eq!(body["version"], json!(REDPACK_TOKEN_VALIDATE_VERSION));
        assert_ne!(
            body["version"],
            json!(REDPACK_VERSION),
            "不能统一成 SYS_CONFIG_Y"
        );
        assert_eq!(body["pintype"], json!(REDPACK_PINTYPE));
        assert_eq!(body["token"], json!("SSO"), "换来的票在 body 里，不在头上");
        assert_eq!(body["deviceId"], json!("DEV"));
        assert_eq!(body["loginConfig"], json!(REDPACK_LOGIN_CONFIG));
    }

    #[test]
    fn other_bodies_carry_the_envelope_version() {
        let body = body_of(&user_sign_request(&call()));
        assert_eq!(body["version"], json!(REDPACK_VERSION));
        assert_eq!(body["platformType"], json!(REDPACK_PLATFORM_TYPE));
    }

    #[test]
    fn every_request_is_a_post_to_the_redpack_base_with_no_query() {
        for (name, req) in every_request(&call()) {
            assert_eq!(req.method, "POST", "{name}");
            assert!(
                req.url.starts_with(&format!("{REDPACK_BASE}/")),
                "{name} 打歪了：{}",
                req.url
            );
            assert!(
                !req.url.contains('?'),
                "{name} 这一族没有查询串：{}",
                req.url
            );
            assert!(req.body.is_some(), "{name} 每条都带体");
        }
    }

    #[test]
    fn paths_are_verbatim_from_the_old_ts_api() {
        // ⭐ README 的硬规矩：端点必须来自源码/抓包，不许凭印象写。
        // 这张表逐条对着 `mcloud-sign/.../cloud-phone-redpack/api.ts` 抄。
        let expect = [
            ("tokenValidate", "/user/tokenValidate"),
            ("configTaskLoginList", "/redpacket/configTaskLoginList"),
            ("userAccountInfo", "/redpacket/userAccountInfo"),
            ("userToastInfo", "/redpacket/userToastInfo"),
            ("configTopicList", "/redpacket/configTopicList"),
            (
                "thirdQueryUserRelationshipV2",
                "/thirdapi/huawei/thirdQueryUserRelationshipV2",
            ),
            ("configAppList", "/redpacket/configAppList"),
            ("gethwToken", "/user/gethwToken"),
            (
                "instanceApkInstallStatus",
                "/app/activity/instanceApkInstallStatus",
            ),
            ("appInstallTaskStatus", "/app/activity/appInstallTaskStatus"),
            ("userSign", "/redpacket/userSign"),
            ("userCompleteTask", "/redpacket/userCompleteTask"),
            ("userBrowse", "/redpacket/userBrowse"),
            ("userTopicAnswer", "/redpacket/userTopicAnswer"),
            ("getRcsToken", "/user/getRcsToken"),
            ("apkInstall", "/app/activity/apkInstall"),
            ("userInstallApp", "/redpacket/userInstallApp"),
        ];
        let got = every_request(&call());
        assert_eq!(got.len(), expect.len(), "17 条一条都不能少");
        for ((name, req), (ename, path)) in got.iter().zip(expect.iter()) {
            assert_eq!(name, ename);
            assert_eq!(req.url, format!("{REDPACK_BASE}{path}"), "{name}");
        }
    }

    #[test]
    fn headers_are_exactly_the_ten_the_old_ts_sends() {
        for (name, req) in every_request(&call()) {
            for key in [
                "appId",
                "token",
                "sign",
                "timestamp",
                "requestId",
                "xChannelSrc",
                "x-DeviceInfo",
                "x-origin",
                "Referer",
                "Origin",
            ] {
                assert_eq!(header_count(&req, key), 1, "{name} 少了/多了一根 {key}");
            }
            assert_eq!(
                req.headers.len(),
                10,
                "{name} 多了计划外的头：{:?}",
                req.headers
            );
        }
    }

    #[test]
    fn this_family_carries_none_of_the_other_families_credentials() {
        for (name, req) in every_request(&call()) {
            for key in [
                "authorization",
                "jwttoken",
                "deviceid",
                "content-type",
                "user-agent",
                "cookie",
            ] {
                assert_eq!(
                    header_count(&req, key),
                    0,
                    "{name} 不该带 {key}：这一族不认它，补了会是第二根（见 redpack_headers 的理由）"
                );
            }
        }
    }

    #[test]
    fn content_type_default_matches_the_old_ts() {
        // 旧 TS 的 `ctx.http` 默认头（`packages/shared/src/http/base.ts:30`）
        // 就是这一串，本仓客户端在调用方没声明时补的也是这一串
        // ⇒ 两边一致，所以这一族不显式写它。这行测试是那个「一致」的哨兵。
        assert_eq!(
            crate::client::CONTENT_TYPE_JSON,
            "application/json;charset=UTF-8"
        );
    }

    #[test]
    fn device_info_layout_is_timestamp_then_nonce_then_fixed_tail() {
        let expected = "|h5|1.0.0|v1.0.0||Mozilla/5.0 (iPad; CPU OS 18_5) AppleWebKit/605.1.15 (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN|1700000000000NONCE16CHARS0000|||375X667|zh||";
        assert_eq!(call().device_info(), expected);
        // 时间戳与随机串之间**没有分隔符** —— 单独再钉一次
        assert!(call()
            .device_info()
            .contains("1700000000000NONCE16CHARS0000"));
        assert_eq!(REDPACK_DEVICE_INFO_TAIL, "|||375X667|zh||");
    }

    #[test]
    fn x_origin_is_the_activity_page_not_the_bare_host() {
        let req = user_sign_request(&call());
        assert_eq!(h(&req, "x-origin").as_deref(), Some(REDPACK_X_ORIGIN));
        assert_eq!(
            h(&req, "Origin").as_deref(),
            Some(REDPACK_HOST),
            "Origin 是裸域名"
        );
        assert_eq!(
            h(&req, "Referer").as_deref(),
            Some("https://cpactiv.buy.139.com/")
        );
        assert!(REDPACK_X_ORIGIN.contains(REDPACK_CHANNEL_SRC));
    }

    #[test]
    fn cloud_phones_request_carries_its_paging_and_status() {
        for (status, text) in [
            (RedpackPhoneStatus::Active, "ACTIVE"),
            (RedpackPhoneStatus::Pending, "PENDING"),
        ] {
            let body = body_of(&cloud_phones_request(&call(), status));
            assert_eq!(body["status"], json!(text));
            assert_eq!(body["pageNumber"], json!(1));
            assert_eq!(body["pageSize"], json!(10));
        }
    }

    #[test]
    fn install_requests_send_lists_around_their_single_value() {
        let body = body_of(&install_status_request(&call(), "HW", "INST", "com.a.b"));
        assert_eq!(
            body["apkList"],
            json!(["com.a.b"]),
            "旧 TS 传的是单元素数组"
        );
        assert_eq!(body["instanceIdList"], json!(["INST"]));
        assert_eq!(body["hwToken"], json!("HW"));
    }

    #[test]
    fn report_install_app_only_sends_existed_when_it_is_known() {
        let body = body_of(&report_install_app_request(
            &call(),
            &json!(7),
            &json!(3),
            None,
        ));
        assert!(body.get("existed").is_none(), "不知道就别发这个键");

        let body = body_of(&report_install_app_request(
            &call(),
            &json!(7),
            &json!(3),
            Some(1),
        ));
        assert_eq!(body["existed"], json!(1));
    }

    #[test]
    fn complete_task_passes_the_id_through_unchanged() {
        // 数值 id 原样、字符串 id 也原样 —— 我们不替上游改写类型。
        for id in [json!(7), json!("7"), json!("AA-BB")] {
            let body = body_of(&complete_task_request(&call(), &id));
            assert_eq!(body["taskId"], id);
        }
    }

    #[test]
    fn answer_topic_sends_the_letter_and_integer_ids() {
        let body = body_of(&answer_topic_request(&call(), 7, 11, "B"));
        assert_eq!(body["taskId"], json!(7));
        assert_eq!(body["topicId"], json!(11));
        assert_eq!(body["answer"], json!("B"));
        assert_eq!(body["platformType"], json!(REDPACK_PLATFORM_TYPE));
    }

    #[test]
    fn apk_install_uses_package_id_while_report_uses_app_id() {
        // 两个 id 来源不同，抄错会在云机上装错包 ⇒ 用一条测试把分歧钉住。
        let install = body_of(&apk_install_request(&call(), "INST", "com.pkg", "HW"));
        assert_eq!(install["appId"], json!("com.pkg"));
        assert_eq!(install["instanceId"], json!("INST"));

        let report = body_of(&report_install_app_request(
            &call(),
            &json!(7),
            &json!(3),
            None,
        ));
        assert_eq!(report["appId"], json!(3), "这一枪用的是 app.id");
    }

    // ── 信封 ──────────────────────────────────────────────

    const OK: &str = r#"{"header":{"status":200,"respMsg":"ok"},"data":{"status":1}}"#;

    #[test]
    fn envelope_accepts_both_numeric_and_string_status() {
        // 旧 TS 判的是 `String(status) === "200"` ⇒ 两种都要认。
        for body in [
            r#"{"header":{"status":200}}"#,
            r#"{"header":{"status":"200"}}"#,
        ] {
            let env = RedpackEnvelope::parse(body).unwrap();
            assert!(env.is_ok(), "{body}");
        }
    }

    #[test]
    fn envelope_rejects_non_object_bodies() {
        for body in ["[]", "null", "3", "not json"] {
            let env = RedpackEnvelope::parse(body);
            assert!(env.is_err(), "{body} 该报错，不能默认成功");
        }
    }

    #[test]
    fn envelope_missing_header_is_a_failure_not_an_error() {
        // 旧 TS 走到 `undefined?.status` ⇒ 判失败（不是抛）。
        let env = RedpackEnvelope::parse(r#"{"data":{}}"#).unwrap();
        assert!(!env.is_ok());
        assert!(env.status.is_none());
    }

    #[test]
    fn envelope_failures_on_other_status_codes() {
        for status in [json!(401), json!("401"), json!(500), json!(null)] {
            let body = json!({ "header": { "status": status } }).to_string();
            let env = RedpackEnvelope::parse(&body).unwrap();
            assert!(!env.is_ok(), "{body}");
        }
    }

    #[test]
    fn error_text_follows_the_js_or_chain() {
        // `data.errorMsg || header.errMsg || header.respMsg || fallback`
        let cases = [
            (
                r#"{"errorMsg":"body 级","header":{"errMsg":"h 级","respMsg":"r 级"}}"#,
                "body 级",
            ),
            (r#"{"header":{"errMsg":"h 级","respMsg":"r 级"}}"#, "h 级"),
            (r#"{"header":{"respMsg":"r 级"}}"#, "r 级"),
            (r#"{"header":{}}"#, "兜底"),
            (r#"{}"#, "兜底"),
        ];
        for (body, want) in cases {
            let env = RedpackEnvelope::parse(body).unwrap();
            assert_eq!(env.error_text("兜底"), want, "{body}");
        }
    }

    #[test]
    fn error_text_treats_the_empty_string_as_absent() {
        // JS 的 `"" || x` 会落到 x —— 空串不算「有话说」。照抄这个怪癖。
        let env = RedpackEnvelope::parse(r#"{"errorMsg":"","header":{"errMsg":"h 级"}}"#).unwrap();
        assert_eq!(env.error_text("兜底"), "h 级");
    }

    #[test]
    fn failure_becomes_an_upstream_error_carrying_the_status() {
        let body = r#"{"header":{"status":401,"errMsg":"登录已过期"}}"#;
        let err = parse_account_info(body).unwrap_err();
        let Error::Upstream { code, .. } = &err else {
            panic!("该是 Upstream，实际 {err:?}");
        };
        assert_eq!(code, "401");
        assert!(err.to_string().contains("登录已过期"), "{err}");
    }

    #[test]
    fn failure_without_a_status_reports_a_dash_not_a_zero() {
        // 缺 status 时报 `-`，不报 `0` —— 0 在这一族是「未完成」，
        // 拿它当"没有状态"会把两种完全不同的情况说成一种。
        let err = parse_account_info(r#"{"data":{}}"#).unwrap_err();
        let Error::Upstream { code, .. } = &err else {
            panic!("该是 Upstream，实际 {err:?}");
        };
        assert_eq!(code, "-");
    }

    #[test]
    fn parse_rejects_responses_without_data() {
        let err = parse_account_info(r#"{"header":{"status":200}}"#).unwrap_err();
        assert!(err.to_string().contains("没有 data"), "{err}");
    }

    // ── 解析器 ──────────────────────────────────────────────

    const TASKS: &str = r#"{
      "header": {"status": "200"},
      "data": {
        "configTaskSignList": [{"isToday": 1, "status": 0, "signAmount": 30}],
        "configTaskNoviceList": [
          {"id": 11, "taskCode": "NOVICE_2", "taskName": "看视频", "userStatus": "1", "prizeAmount": 50}
        ],
        "configTaskDailyList": [{"id": 21, "taskCode": "DAILY_1", "userStatus": 0}],
        "configTaskMonthlyList": [{"id": "31", "taskCode": "MONTHLY_2", "taskAmount": 200}]
      }
    }"#;

    #[test]
    fn task_list_reads_all_four_groups() {
        let list = parse_task_login_list(TASKS).unwrap();
        assert_eq!(list.sign.len(), 1);
        assert_eq!(list.sign[0].is_today, Some(1));
        assert_eq!(list.sign[0].sign_amount, Some(30));
        assert_eq!(list.novice.len(), 1);
        assert_eq!(list.daily.len(), 1);
        assert_eq!(list.monthly.len(), 1);
        assert_eq!(list.novice[0].task_code.as_deref(), Some("NOVICE_2"));
        assert_eq!(list.novice[0].task_name.as_deref(), Some("看视频"));
        assert_eq!(list.novice[0].prize_amount, Some(50));
        assert_eq!(list.monthly[0].task_amount, Some(200));
    }

    #[test]
    fn task_list_reads_string_and_number_ids_and_string_user_status() {
        let list = parse_task_login_list(TASKS).unwrap();
        assert_eq!(list.novice[0].id, Some(json!(11)));
        assert_eq!(list.monthly[0].id, Some(json!("31")));
        // 上游把 userStatus 发成字符串「1」—— 要读成 1，不是读不到
        assert_eq!(list.novice[0].user_status, Some(1));
    }

    #[test]
    fn missing_user_status_stays_unknown_instead_of_becoming_zero() {
        let body =
            r#"{"header":{"status":200},"data":{"configTaskDailyList":[{"taskCode":"DAILY_9"}]}}"#;
        let list = parse_task_login_list(body).unwrap();
        assert_eq!(list.daily[0].user_status, None, "读不到 ≠ 未完成");
    }

    #[test]
    fn find_scans_novice_then_daily_then_monthly() {
        let list = parse_task_login_list(TASKS).unwrap();
        assert_eq!(
            list.find("NOVICE_2").unwrap().task_name.as_deref(),
            Some("看视频")
        );
        assert_eq!(list.find("DAILY_1").unwrap().id, Some(json!(21)));
        assert_eq!(list.find("MONTHLY_2").unwrap().id, Some(json!("31")));
        assert!(list.find("NOPE").is_none());
    }

    #[test]
    fn a_missing_group_is_empty_but_a_wrong_shaped_one_is_an_error() {
        let missing = r#"{"header":{"status":200},"data":{}}"#;
        let list = parse_task_login_list(missing).unwrap();
        assert!(list.novice.is_empty(), "这一期没配 ⇒ 空，不是错");

        // 上游换了形状而我们静默当空，会把「有任务」读成「没任务」⇒ 必须报错
        let wrong = r#"{"header":{"status":200},"data":{"configTaskNoviceList":{}}}"#;
        assert!(parse_task_login_list(wrong).is_err());
    }

    #[test]
    fn account_amounts_are_cents_and_never_default_to_zero() {
        let body =
            r#"{"header":{"status":200},"data":{"info":{"canAmount":1234,"totalAmount":"5600"}}}"#;
        let acct = parse_account_info(body).unwrap();
        assert_eq!(acct.can_amount, Some(1234));
        assert_eq!(acct.total_amount, Some(5600), "数字串也认");

        let empty = parse_account_info(r#"{"header":{"status":200},"data":{}}"#).unwrap();
        assert_eq!(empty.can_amount, None, "读不到 ≠ 0 元");
    }

    #[test]
    fn toast_prize_reads_last_user_prize() {
        let body = r#"{"header":{"status":200},"data":{"lastUserPrize":{"prizeName":"1元话费","prizeAmount":100}}}"#;
        let prize = parse_toast_info(body).unwrap();
        assert_eq!(prize.prize_name.as_deref(), Some("1元话费"));
        assert_eq!(prize.prize_amount, Some(100));
    }

    #[test]
    fn topic_list_keeps_topic_option_as_a_string() {
        let body = r#"{"header":{"status":200},"data":{"list":[{"id":5,"topicContent":"谁","topicOption":"[\"甲\",\"乙\"]"}]}}"#;
        let topics = parse_topic_list(body).unwrap();
        assert_eq!(topics[0].id, Some(json!(5)));
        assert_eq!(topics[0].topic_content.as_deref(), Some("谁"));
        assert_eq!(
            parse_topic_options(topics[0].topic_option.as_deref().unwrap()),
            vec!["甲".to_string(), "乙".to_string()]
        );
    }

    #[test]
    fn broken_topic_option_yields_an_empty_list_not_an_error() {
        // 旧 TS 的 `catch { return [] }` —— 题面配坏了不该让整条链断。
        for bad in ["", "not json", "{}", "3"] {
            assert!(parse_topic_options(bad).is_empty(), "{bad}");
        }
    }

    #[test]
    fn cloud_phones_unwrap_the_nested_data_only_when_it_is_an_object() {
        let nested = r#"{"header":{"status":200},"data":{"data":{"subscribeDetails":[{"a":1}]}}}"#;
        assert_eq!(parse_cloud_phones(nested).unwrap().len(), 1);

        let flat = r#"{"header":{"status":200},"data":{"subscribeDetails":[{"a":1},{"b":2}]}}"#;
        assert_eq!(parse_cloud_phones(flat).unwrap().len(), 2);

        // `data.data` 在但不是对象 ⇒ 旧 TS 的判类型那一步会退回 data 本身
        let scalar = r#"{"header":{"status":200},"data":{"data":3,"subscribeDetails":[]}}"#;
        assert!(parse_cloud_phones(scalar).unwrap().is_empty());
    }

    #[test]
    fn app_list_reads_the_list_key() {
        let body = r#"{"header":{"status":200},"data":{"list":[{"id":1,"packageId":"com.a","apkName":"a.apk"}]}}"#;
        let apps = parse_app_list(body).unwrap();
        assert_eq!(apps[0]["packageId"], json!("com.a"));
    }

    #[test]
    fn hw_token_is_read_but_absent_is_not_an_error() {
        let ok = r#"{"header":{"status":200},"data":{"hwToken":"HW"}}"#;
        assert_eq!(parse_hw_token(ok).unwrap().as_deref(), Some("HW"));
        let none = r#"{"header":{"status":200},"data":{}}"#;
        assert_eq!(parse_hw_token(none).unwrap(), None);
    }

    #[test]
    fn write_status_reports_what_it_read_rather_than_judging() {
        // 旧 TS：`!isNil(status) && status !== 1` 才算失败 ⇒ 缺失是放行。
        assert_eq!(parse_write_status(OK).unwrap(), Some(1));
        assert_eq!(
            parse_write_status(r#"{"header":{"status":200},"data":{}}"#).unwrap(),
            None,
            "缺失 ⇒ None ⇒ 调用方按「放行」处理"
        );
        assert_eq!(
            parse_write_status(r#"{"header":{"status":200},"data":{"status":0}}"#).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn status_text_covers_the_four_known_codes_and_nothing_else() {
        assert_eq!(redpack_status_text(REDPACK_STATUS_TODO), Some("未完成"));
        assert_eq!(redpack_status_text(REDPACK_STATUS_DONE), Some("已完成"));
        assert_eq!(
            redpack_status_text(REDPACK_STATUS_SOLD_OUT),
            Some("奖品已兑完")
        );
        assert_eq!(
            redpack_status_text(REDPACK_STATUS_TOMORROW),
            Some("明天再来")
        );
        // 未知码**不许**落进「未完成」—— 那就等于替上游下结论
        for unknown in [2, 5, -1, 99] {
            assert_eq!(redpack_status_text(unknown), None, "{unknown}");
        }
    }

    // ── 会话第 ① 段：querySpecToken ─────────────────────────────────
    // 这一族发的每一枪都在 every_request 里列一遍，但那一张表是**红包站点
    // 上那 17 条 POST**（它自带一条「全是 POST 且无查询串」的不变量）。
    // querySpecToken 打在 m.mcloud.139.com、是 GET、还带查询串，进不了那张表，
    // 所以它的 URL / 头 / 签名单独在这里钉住。

    #[test]
    fn spec_token_is_a_get_on_the_userdomain_path_with_the_source_id() {
        let req = spec_token_request(1_700_000_000_000, REDPACK_SSO_SOURCE_ID);
        assert_eq!(req.method, "GET");
        assert_eq!(
            req.url,
            "https://m.mcloud.139.com/ycloud/api/cloud/userdomain/v2/querySpecToken?targetSourceId=001216&hideLoading=true"
        );
        assert!(req.body.is_none(), "换票那一枪没有请求体");

        // 别族的 sourceId 各有各的值 ⇒ 必须真的用传进来的那个
        let other = spec_token_request(0, "001005");
        assert!(other.url.contains("targetSourceId=001005"), "{}", other.url);
    }

    #[test]
    fn spec_token_carries_exactly_the_four_signature_headers_and_nothing_else() {
        let req = spec_token_request(1_700_000_000_000, REDPACK_SSO_SOURCE_ID);
        for name in ["x-request-id", "x-timestamp", "x-nonce", "x-signature"] {
            assert_eq!(header_count(&req, name), 1, "{name}");
        }
        assert_eq!(req.headers.len(), 4);
        // 还没有凭证，所以不许出现这一族那十个业务头
        for name in [
            "appId",
            "token",
            "sign",
            "xChannelSrc",
            "x-DeviceInfo",
            "x-origin",
            "authorization",
            "content-type",
        ] {
            assert_eq!(header_count(&req, name), 0, "不该带 {name}");
        }
    }

    #[test]
    fn spec_token_signature_is_the_salt_2_recipe_over_the_emitted_id_and_nonce() {
        let ts = 1_700_000_000_000i64;
        let req = spec_token_request(ts, REDPACK_SSO_SOURCE_ID);
        let id = h(&req, "x-request-id").unwrap();
        let nonce = h(&req, "x-nonce").unwrap();
        assert_eq!(h(&req, "x-timestamp").unwrap(), ts.to_string());

        let salt = mcloud_crypto::sign::salt_for(REDPACK_SPEC_TOKEN_SALT_KEY);
        assert_ne!(
            salt,
            mcloud_crypto::sign::salt_for(1),
            "键 2 得是另一个盐，否则这里等于没测"
        );
        // 配方：salt + id + ts + nonce + other(空) + salt
        let expected = md5_hex(&format!("{salt}{id}{ts}{nonce}{salt}"));
        assert_eq!(h(&req, "x-signature").unwrap(), expected);

        // 随机量由 sign_headers 现产 ⇒ 同参数两次不该同签名
        let again = spec_token_request(ts, REDPACK_SSO_SOURCE_ID);
        assert_ne!(h(&again, "x-request-id"), Some(id));
    }

    #[test]
    fn spec_token_parser_reads_result_and_ignores_code_on_purpose() {
        assert_eq!(
            parse_spec_token(r#"{"code":0,"msg":"成功","result":"SSO-TOKEN"}"#).unwrap(),
            "SSO-TOKEN"
        );
        // 旧 TS 只判 result 真假、不判 code ⇒ 这里照做
        assert_eq!(
            parse_spec_token(r#"{"code":500,"msg":"boom","result":"T"}"#).unwrap(),
            "T"
        );
    }

    #[test]
    fn spec_token_parser_fails_loudly_and_keeps_the_upstream_reason() {
        for bad in [
            r#"{"code":500,"msg":"服务异常"}"#,
            r#"{"code":0,"msg":"成功","result":""}"#,
            r#"{"code":0,"msg":"成功","result":null}"#,
            r#"{"code":0,"msg":"成功","result":{"a":1}}"#,
        ] {
            let e = parse_spec_token(bad).unwrap_err();
            let Error::Parse(m) = &e else {
                panic!("该是 Parse，实际 {e:?}");
            };
            assert!(m.contains("code="), "{m}");
            assert!(m.contains("msg="), "{m}");
        }

        // 铁了心不是 JSON 也得是 Parse，不能 panic 也不能算成功
        assert!(matches!(
            parse_spec_token("<html>502</html>"),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn token_validate_reads_data_token_and_rejects_everything_else() {
        let ok = r#"{"header":{"status":"200"},"data":{"token":"CPT"}}"#;
        assert_eq!(parse_token_validate(ok).unwrap(), "CPT");

        for bad in [
            r#"{"header":{"status":"200"},"data":{"token":""}}"#,
            r#"{"header":{"status":"200"},"data":{"token":3}}"#,
            r#"{"header":{"status":"200"},"data":{}}"#,
            r#"{"header":{"status":"200"}}"#,
        ] {
            assert!(parse_token_validate(bad).is_err(), "{bad}");
        }

        // 失败走 Upstream（不是 AuthExpired）—— 这一族票过期回什么码没样本
        let e =
            parse_token_validate(r#"{"header":{"status":"401","errMsg":"票过期"}}"#).unwrap_err();
        let Error::Upstream { code, message } = &e else {
            panic!("该是 Upstream，实际 {e:?}");
        };
        assert_eq!(code, "401");
        assert!(message.contains("票过期"), "{message}");
    }
}
