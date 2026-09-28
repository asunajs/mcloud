//! H5 会话：换 SSO token → 取服务端时间戳 → `tyrzLogin` 换 `jwttoken`。
//!
//! # 这三条是一条链，不是三个接口
//!
//! 没有第 ③ 步的 `jwttoken`，后面所有签到请求一律回 `90001 未登录`。
//! 拆成三个"想到谁调谁"的函数，漏掉中间一步的表现是**每个业务接口都报未登录** ——
//! 看上去像鉴权坏了，实际是会话没建。所以三条并排写在这里，调用方看得见整条链。
//!
//! # 为什么是"配方"而不是"页面名"
//!
//! 真正决定一条会话长什么样的是**换票那几枪用的参数**
//! （`toSourceId` / `marketName` / `sourceId` / `referer` 指向哪个页面），
//! 不是页面上有个什么按钮。所以键叫 [`SessionRecipe`]。
//!
//! mcloud 的实测结论：**每个页面用自己的参数换自己的票，票与票之间逐字节不等**。
//! 已实拍的配方（`mcloud/docs/capture/`）：
//!
//! | 页面 | `marketName` | `sourceId` |
//! |---|---|---|
//! | 签到页 | `sign_in_3` | `1002` |
//! | 备份有礼页 | `backupgift` | `1134` |
//! | 开启通知页 | `msgpushon` | `1062` |
//! | 拍拍门户页 | `National_playAI` | `1044` |
//!
//! 所以这里**不写死签到页那一套**。mcloud 自己吃过这个亏：它先把换票参数写死在
//! `OpenSignInSession` 里，后来发现各页有自己配方，切的时候"动的是**共享会话层**"，
//! 于是只能"先登记、等第二条页面一起改"。我们先按配方建模，就不欠这笔债。
//!
//! # ⚠ 会变的东西不要缓存
//!
//! - **SSO token 不缓存**：实测"14:38:54 换到的 token，14:42:09 重放已被拒"。
//!   换完立刻用。
//! - **`jwttoken` 缓存，但只在内存**（见 [`SessionCache`]）。它 2 小时即废，
//!   而库里那一列装的是**登录凭据**（同名不同物）。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use mcloud_core::{Account, Error, HttpRequest, HttpResponse, Result};

/// 一台主机两套头：原生接口走 `user-njs`，H5 接口走 `m.mcloud`。
pub const BASE_MCLOUD: &str = "https://m.mcloud.139.com";

/// 查 SSO token 的主机（原生 A 族头）。
pub const BASE_USER_YUN: &str = "https://user-njs.yun.139.com";

/// H5 建会话用的签名盐位。
///
/// 与 App 业务签名**同一个盐表**，只是取第 2 个 —— 所以直接复用
/// [`mcloud_crypto::generate_signature`]，不另写一份。
pub const H5_SALT_KEY: u8 = 2;

/// H5 请求的 content-type。带 `charset` 是抓包实样。
pub const H5_CONTENT_TYPE: &str = "application/json;charset=UTF-8";

/// H5 的 `appversion` 头。
///
/// ⚠ 抓包显示 WebView 打开的页面里这个值是 `0.0.0.0`（H5 拿不到 bridge 时的兜底），
/// 而**签到主体**（`infoV3` / `startSignIn`）是 `12.6.0.0`。实测两种都能过，
/// 这里统一用 `12.6.0.0` 并在测试里钉住，免得实现时以为抄错。
pub const H5_APP_VERSION: &str = "12.6.0.0";

/// 会话配方：这一页按什么参数换票。
///
/// ⚠ 它是**配方编号，不是页面名**。两个页面映射到同一个配方，意思是
/// "目前按同一套参数换票"，不是"这两个页面是同一个页面"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionRecipe {
    /// 签到页：`toSourceId=001005` → 时间戳 → `tyrzLogin(sign_in_3 / 1002)`。
    ///
    /// 这是**唯一有完整实拍**的配方，其余页面在拿到自己的实拍前映射到它。
    SignInPage,
    /// 「开启通知有礼」那一页：`tyrzLogin(marketName=msgpushon / sourceId=1062)`。
    ///
    /// # 证据
    ///
    /// `docs/capture/app-msg-push-on-samples.txt` §2 的**三张票核对表**：那一趟抓到
    /// 三张互不相等的 `jwttoken`，其中 `faa8d052` 由 `msgpushon` / `1062` 换出，
    /// 被用在 **24 个接口**上（含 `msgPushOn/task/status`、`msgPushOn/task/list`）；
    /// 而签到页那张 `f9048ce0`（`sign_in_3` / `1002`）被用在另外 16 个上。
    ///
    /// ⚠ **反向未验证**：实拍里**没有**出现"拿签到页那张打 `msgPushOn/*`"的流量
    /// ⇒ "借公共票行不行"这一问没有样本（Go 侧 `app_msg_push.go` 的"这一页的票"
    /// 一节把同一件事记成未测试）。所以这一刀切下去是**照实拍**，不是照推断。
    MsgPushOn,
}

impl SessionRecipe {
    /// 这个配方换票用的那几个参数。
    pub fn params(self) -> RecipeParams {
        match self {
            SessionRecipe::SignInPage => RecipeParams {
                to_source_id: "001005",
                market_name: "sign_in_3",
                source_id: "1002",
                h5_app_id: Some("cy62d5a5e8373a272aeca467accad61722c51cfd48"),
                page_path: "newsignin",
                // 签到页的入口在 `portal/mobilecloud/` 下。
                portal_page: "mobilecloud",
                // ⚠ 签到页的入口 URL **必须**带 `&targetSourceId=`，且排在 `token` 之后
                // —— 参考实现为漏掉它踩过一次坑（见 `page_url` 的说明）。
                target_source_id: Some("001005"),
            },
            SessionRecipe::MsgPushOn => RecipeParams {
                // ⚠ 这一页的**第一步没有实拍**（三张票那张核对表只拍到 `tyrzLogin`
                // 的参数）。这里沿用 `001005` 的依据是**旁证**：Go 侧 P5 记的
                // 拍拍页两步流程里，第一步打的也是 `targetSourceId=001005`
                // ⇒ "第一步与页面无关"有一格支持，但**不是**这一页的直接实拍。
                to_source_id: "001005",
                market_name: "msgpushon",
                source_id: "1062",
                // ⚠ 实拍这一页的入口 URL **没有** `appID`（见下 `page_url`）
                // ⇒ 这里是 `None`，不是"忘了填"。
                h5_app_id: None,
                page_path: "openNoticeGift",
                // 入口在 `portal/hcyview/` 下 —— 与签到页**不是同一个路径段**。
                portal_page: "hcyview",
                // ⚠ 实拍这一页的入口 URL **没有** `targetSourceId`。照抄，别顺手补齐：
                // "补一枚上游不认的参数"与"漏一枚上游必需的参数"是两种错，
                // 而哪一种是这里的真相**没有样本**。
                target_source_id: None,
            },
        }
    }
}

/// 直播侧**门的第①步**：只要 `querySpecTokenV2` 那一枪，不要完整配方。
///
/// # 为什么不做成 [`SessionRecipe`] 的一个分支
///
/// 门的第①步与"建 H5 会话"是**两件事**：
///
/// | | 门的第①步（本函数） | 建 H5 会话（[`SessionRecipe`]） |
/// |---|---|---|
/// | 打几枪 | **1**（`querySpecTokenV2`） | 3（+时间戳 +`tyrzLogin`） |
/// | 要哪些参数 | **只有 `toSourceId`** | `marketName` / `sourceId` / `appID` / `path` |
/// | 产物 | SSO token → 去 `/cmlive` 换**直播侧** JWT | H5 会话票（`/ycloud` 用） |
///
/// ⇒ 把门塞进 `SessionRecipe` 就必须为 `market_name` / `source_id` /
/// `h5_app_id` / `page_path` **四个用不到的字段填值**，而它们**没有任何依据**
/// （握手实测：`../mcloud/docs/capture/app-live-redflower-samples.txt` 第 351 行
/// 那一步只有 `{"toSourceId":"017027"}`，没有后四个）。
/// 编四个"看起来合理"的常量，正是本仓明确排斥的那类做法 ——
/// 下一个读代码的人会拿它当依据。
///
/// 所以这里只给**一个 `toSourceId`**，与实测形状一一对应。
///
/// # ⚠ 必须用直播侧那个值（`017027`）
///
/// 同文档第 494–495 行的原文纠正：
/// > `sourceCode=017027` 当过滤器是错的 —— 它只出现在直播房间页的 **URL 参数**
/// > `targetSourceId=017027` 里，**不在 `/ycloud` 侧**（小红花那边是 `targetSourceId=001005`）。
///
/// 混用的表现是"**一条房间都筛不出来**"，而它看起来像"今天没有直播"——
/// 一个**假结论**（参考实现坑表 #106）。
pub fn live_gate_spec_token_request(authorization: &str) -> HttpRequest {
    HttpRequest::post(format!("{BASE_USER_YUN}/user/querySpecTokenV2"))
        .header("Content-Type", H5_CONTENT_TYPE)
        .header("Authorization", authorization)
        .json_body(&serde_json::json!({
            "toSourceId": crate::live_consts::LIVE_SOURCE_ID,
        }))
}

/// 一个配方的换票参数。
///
/// 收成一个结构体而不是散在函数里：新增配方时只加一个 `match` 分支，
/// 加会话池、刷新逻辑一行都不用改。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecipeParams {
    /// `querySpecTokenV2` 的 body 与页面 URL 的 `targetSourceId`。
    ///
    /// ⚠ 它**不是端常量，是调用方参数** —— 同一个 App 进程里出现过三个值
    /// （`001005` 签到 / `001003` 权益 / `010001` 权益）。
    /// 跨端（PC ↔ App）、跨发起方（原生 ↔ H5）的常量**一律不许互搬**。
    pub to_source_id: &'static str,
    /// `tyrzLogin` body 的 `marketName`。
    pub market_name: &'static str,
    /// `tyrzLogin` body 的 `sourceId`。
    pub source_id: &'static str,
    /// 页面 URL 的 `appID`。`None` = **实拍里这一页的入口没有这个参数**。
    ///
    /// ⚠ 做成 `Option` 而不是给个空串：空串会拼出 `&appID=`，那是**多一枚空参数**，
    /// 与"没有这一枚"是两件事 —— 而上游认哪种没有样本。
    pub h5_app_id: Option<&'static str>,
    /// 页面 URL 的 `path`（同时决定 referer 长什么样）。
    pub page_path: &'static str,
    /// 入口 URL 里 `portal/<这一段>/index.html` 的那一段。
    ///
    /// ⚠ **逐页不同**：签到页是 `mobilecloud`，开启通知有礼那一页是 `hcyview`。
    /// 原先这里写死了 `mobilecloud` ⇒ 另一页的 referer 会指到一个**不存在的页面**，
    /// 而 referer 指错的表现是上游回一个业务码，不像"我们拼错了"。
    pub portal_page: &'static str,
    /// 入口 URL 要不要 `&targetSourceId=<to_source_id>`。
    ///
    /// ⚠ 两页**不同**（都是实拍）：签到页**有**（且有一条"漏了它"的坑），
    /// 开启通知有礼那一页**没有** ⇒ 按各页实拍照抄，不统一。
    pub target_source_id: Option<&'static str>,
}

impl RecipeParams {
    /// 这一页的 H5 落地页地址（纯协议实现里只用来当 `referer`）。
    ///
    /// 真实客户端把 SSO token 直接拼在查询串里交给 WebView，H5 自己拿它去
    /// `tyrzLogin` 建会话。
    ///
    /// # 为什么是"逐页"拼而不是一套模板套参数
    ///
    /// 两页的入口 URL **形状不同**，不是同一套模板换几个值：
    ///
    /// ```text
    /// 签到页  /portal/mobilecloud/index.html?path=newsignin&sourceid=1002
    ///         &enableShare=1&appID=<40 字符>&token=<SSO>&targetSourceId=001005
    /// 通知页  /portal/hcyview/index.html?path=openNoticeGift&sourceid=1062
    ///         &token=<42 字符>&enableShare=1
    /// ```
    ///
    /// 差别有三处：**路径段**（`mobilecloud` / `hcyview`）、通知页**没有** `appID`、
    /// 通知页**没有** `targetSourceId`，而且参数次序也不同。
    ///
    /// ⚠ 签到页那条 `&targetSourceId=…` 必须带、且排在 `token` **之后** ——
    /// 参考实现踩过一次坑：初版漏了它，因为当时是**照着实现**写的这一行，
    /// 而不是照着抓包，于是实现与文档一起漏。
    ///
    /// ⚠ 通知页那三处"没有"是**照抄实拍**，不是"还没补"。别顺手给它补齐：
    /// "补一枚上游不认的参数"与"漏一枚上游必需的参数"是两种错，
    /// 而这一页究竟是哪种**没有样本**。
    pub fn page_url(&self, sso_token: &str) -> String {
        let mut url = format!(
            "{BASE_MCLOUD}/portal/{}/index.html?path={}&sourceid={}",
            self.portal_page, self.page_path, self.source_id
        );
        if let Some(app_id) = self.h5_app_id {
            url.push_str(&format!("&enableShare=1&appID={app_id}"));
        }
        url.push_str(&format!("&token={sso_token}"));
        if let Some(target) = self.target_source_id {
            url.push_str(&format!("&targetSourceId={target}"));
        }
        // ⚠ 有 `appID` 的那一页（签到）实拍里 `enableShare=1` 在 `appID` **之前**，
        // 而没有 `appID` 的那一页它在 `token` **之后**。两处次序都是照抄，
        // 所以它跟着 `appID` 一起分支 —— 不在这里"统一一下顺序"。
        if self.h5_app_id.is_none() {
            url.push_str("&enableShare=1");
        }
        url
    }
}

/// 一次 H5 请求的签名头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H5Signature {
    pub request_id: String,
    pub timestamp: String,
    pub nonce: String,
    pub signature: String,
}

impl H5Signature {
    /// 用**给定的** requestID / nonce 算签名（纯函数，测试走这条）。
    ///
    /// 算式：`md5(盐 + requestID + timestamp + nonce + "" + 盐)`，
    /// 最后那个空串是 H5 留的预留位（实测恒为空）。
    pub fn with_ids(
        request_id: impl Into<String>,
        timestamp: impl Into<String>,
        nonce: impl Into<String>,
    ) -> Self {
        let request_id = request_id.into();
        let timestamp = timestamp.into();
        let nonce = nonce.into();
        let signature =
            mcloud_crypto::generate_signature(&request_id, &timestamp, &nonce, H5_SALT_KEY, "");
        Self {
            request_id,
            timestamp,
            nonce,
            signature,
        }
    }

    /// 现生成随机量并签名。
    pub fn generate(timestamp: impl Into<String>) -> Self {
        Self::with_ids(
            uuid::Uuid::new_v4().to_string(),
            timestamp,
            uuid::Uuid::new_v4().to_string(),
        )
    }
}

/// 换到的会话凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H5Session {
    /// 后面所有 H5 业务请求要带的 `jwttoken`。
    pub jwt_token: String,
    /// 响应回显的手机号（确认签的是哪个账号）。可能缺失。
    pub mobile: Option<String>,
    /// 这条会话的页面地址，业务请求要原样当 `referer`。
    ///
    /// 它里面带着**本次换到的 SSO token**，所以每个会话都不同；
    /// 它同时是"这条会话属于哪个页面"的凭据 ——
    /// 换票不换 referer（或反之）的表现是"凭据都在、却回 `90001`"，
    /// 看起来像凭据坏了。所以两者必须一起存在会话里。
    pub referer: String,
}

// ── 请求构造（纯函数，可离线断言）─────────────────────────────────────────

/// 第 ① 段：`POST /user/querySpecTokenV2`（原生，明文，A 族头）。
pub fn spec_token_request(params: RecipeParams, authorization: &str) -> HttpRequest {
    let body = serde_json::json!({ "toSourceId": params.to_source_id });
    HttpRequest::post(format!("{BASE_USER_YUN}/user/querySpecTokenV2"))
        .header("Content-Type", H5_CONTENT_TYPE)
        .header("Authorization", authorization)
        .json_body(&body)
}

/// 第 ② 段：`GET /ycloud/api/timestamp`（无鉴权）。
///
/// 它给的是 13 位毫秒。签名里的 `x-timestamp` 必须与它**逐字相同**，
/// 所以**不能**用本地时钟替代 —— 本地时钟偏一点就是 `401`。
pub fn timestamp_request(params: RecipeParams, referer: &str) -> HttpRequest {
    h5_headers(
        HttpRequest::get(format!("{BASE_MCLOUD}/ycloud/api/timestamp")),
        params,
        referer,
        None,
        "true",
    )
    .header("Accept", "*/*")
}

/// 第 ③ 段：`POST /ycloud/auth-service/auth/tyrzLogin`（带签名）。
pub fn tyrz_login_request(
    params: RecipeParams,
    sso_token: &str,
    sign: &H5Signature,
    referer: &str,
) -> HttpRequest {
    // 字段按 JSON 键的字典序（marketName → openAccount → sourceId → token）。
    //
    // 用 `json!` 而不是手拼字符串：`serde_json` 的 Map 默认按插入序输出，
    // 而这里按字典序写就是抓包顺序。手拼字符串没法处理 token 里的转义。
    let body = serde_json::json!({
        "marketName": params.market_name,
        "openAccount": 0,
        "sourceId": params.source_id,
        "token": sso_token,
    });
    // `jwt` 传 None：这一步正是**为了**拿 jwt，所以还没有。
    h5_headers(
        HttpRequest::post(format!("{BASE_MCLOUD}/ycloud/auth-service/auth/tyrzLogin"))
            .json_body(&body),
        params,
        referer,
        None,
        "true",
    )
    .header("Origin", BASE_MCLOUD)
    .header("x-request-id", &sign.request_id)
    .header("x-timestamp", &sign.timestamp)
    .header("x-nonce", &sign.nonce)
    .header("x-signature", &sign.signature)
}

/// 给 H5 请求补上那套自有头。
///
/// 与 App 原生头是**两套**：原生走 `Authorization` + 设备头（无签名），
/// H5 走 `activityid` / `appversion` / `jwttoken`。
///
/// `jwt` 为 `None` 时不加 `jwttoken` 头（建会话那三步就是这种情况）。
pub fn h5_headers(
    req: HttpRequest,
    params: RecipeParams,
    referer: &str,
    jwt: Option<&str>,
    show_loading: &str,
) -> HttpRequest {
    let mut req = req
        .header("Accept", "*/*")
        .header("activityid", params.market_name)
        .header("appversion", H5_APP_VERSION)
        .header("Referer", referer)
        .header("showloading", show_loading);
    if let Some(token) = jwt {
        // 头名是**小写** `jwttoken`（不是 `jwtToken`）—— H5 拦截器原样如此。
        req = req.header("jwttoken", token);
    }
    req
}

// ── 响应解析 ────────────────────────────────────────────────────────────

/// 解析 `querySpecTokenV2` 的响应，取出 SSO token。
///
/// 响应是**双层信封**：`{"success":true,"code":"0000","message":"…","data":{…}}`。
/// 注意 `code` 是**字符串** `"0000"`，不是数字 —— 按数字解会失败。
pub fn parse_sso_token(body: &str) -> Result<String> {
    let env: Envelope = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("querySpecTokenV2 响应不是 JSON：{e}")))?;
    if !env.success {
        return Err(Error::Other(format!(
            "querySpecTokenV2 失败：code={} message={}",
            env.code, env.message
        )));
    }
    env.data
        .as_ref()
        .and_then(|d| d.get("token"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| Error::Parse("querySpecTokenV2 响应没有 data.token".into()))
}

/// 解析 `GET /ycloud/api/timestamp`，取出**字符串形式**的 13 位毫秒。
///
/// 转成字符串是因为它下一步要原样进 `x-timestamp` 头与签名 ——
/// 中途过一遍数字类型只会多一次"格式化可能不一致"的机会。
pub fn parse_server_timestamp(body: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("api/timestamp 响应不是 JSON：{e}")))?;
    let result = v
        .get("result")
        .ok_or_else(|| Error::Parse("api/timestamp 响应没有 result".into()))?;
    match result {
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::String(s) => Ok(s.clone()),
        other => Err(Error::Parse(format!(
            "api/timestamp 的 result 不是整数：{other}"
        ))),
    }
}

/// 解析 `tyrzLogin` 的响应。
///
/// 成功时 `result.token` 就是后面要带的 `jwttoken`。
/// 失败样本：`{"code":200050409,"msg":"token 已失效"}` —— 注意这条**没有 `success` 字段**，
/// 所以判据不能只看 `success`。
pub fn parse_jwt_token(body: &str, referer: impl Into<String>) -> Result<H5Session> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("tyrzLogin 响应不是 JSON：{e}")))?;

    // `code` 可能是数字 0 也可能是字符串 "0000"，统一按"非 0 即失败"处理
    let code_ok = match v.get("code") {
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(0),
        Some(serde_json::Value::String(s)) => s == "0" || s == "0000",
        Some(serde_json::Value::Null) | None => {
            v.get("success").and_then(|b| b.as_bool()).unwrap_or(false)
        }
        Some(other) => {
            return Err(Error::Parse(format!("tyrzLogin 的 code 类型意外：{other}")));
        }
    };
    if !code_ok {
        let msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("(无消息)");
        return Err(Error::Other(format!(
            "tyrzLogin 失败：code={} msg={msg}",
            v.get("code").map(|c| c.to_string()).unwrap_or_default()
        )));
    }

    let result = v
        .get("result")
        .ok_or_else(|| Error::Parse("tyrzLogin 响应没有 result".into()))?;
    let jwt_token = result
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| Error::Parse("tyrzLogin 响应没有 result.token".into()))?
        .to_string();

    // `mobile` 可能缺失 —— 它是"确认签的是哪个账号"用的回显，不是凭据的一部分，
    // 缺了不该让整次登录失败。
    let mobile = result
        .get("mobile")
        .and_then(|m| m.as_str())
        .map(str::to_string);

    Ok(H5Session {
        jwt_token,
        mobile,
        referer: referer.into(),
    })
}

/// `querySpecTokenV2` 那层信封。
#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

// ── 会话缓存（**只在内存**）────────────────────────────────────────────

/// 会话缓存的键。
///
/// 三段都不能省：
///
/// - **账号** —— 票是按账号签的。
/// - **配方** —— 不同页面各建各的票，且各自的票**互不顶替**
///   （mcloud 实拍：小红书页换了票之后，回头点签到带的还是签到页那张）。
/// - **凭据指纹** —— 外圈 `Authorization` 一旦轮换，旧会话再建票必然失败，
///   而失败会被读成"这张票刷不动"。
///
/// 存指纹（不可逆）而**不是**凭据本身：这个结构会进日志与测试输出。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    account: String,
    recipe: SessionRecipe,
    auth_fingerprint: String,
}

/// 进程内的会话缓存。
///
/// # 为什么不落地
///
/// `jwttoken` 2 小时即废，而库里那一列（`account_state`）装的是**登录凭据** ——
/// 同名不同物。落一张两小时就死的票，换来的是"重启后读到一张过期票"这一整类
/// 难查的现象。所以**只放内存**：进程重启就重取，代价是三条请求。
///
/// # 不加 TTL
///
/// 抓包只给了"这张票 2 小时即废"这一个数，**没给**"该在哪个时刻算它过期"的依据
/// （服务端还有没有别的失效路径、时钟偏差怎么算，全未实测）。所以失效判据只有
/// 上游回的那一句 `90001`，不自造时钟。
///
/// 代价说清楚：票自然过期后的第一次请求会先撞一次 `90001`，再由调用方刷新重放
/// ⇒ 用户看到的是"多等一次往返"，不是失败。反过来写死 2 小时重建，
/// 那是拿客户端猜测替换服务端语义。
#[derive(Default)]
pub struct SessionCache {
    inner: Mutex<HashMap<CacheKey, H5Session>>,
}

impl SessionCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<CacheKey, H5Session>> {
        // 中毒不构成不可用的理由：每次操作都在锁内一次做完，
        // 没有"写了一半"的中间态可被读到。跟着 panic 会让整个服务停摆。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 取缓存。**不发请求。**
    pub fn get(
        &self,
        account: &str,
        recipe: SessionRecipe,
        auth_fingerprint: &str,
    ) -> Option<H5Session> {
        self.lock()
            .get(&CacheKey {
                account: account.to_string(),
                recipe,
                auth_fingerprint: auth_fingerprint.to_string(),
            })
            .cloned()
    }

    /// 存一条会话。
    ///
    /// 同时清掉**同账号同配方、但凭据指纹不同**的旧条目：
    /// 留着它们既泄漏，又会被下次误用（凭据换了，旧票必然失效）。
    pub fn put(
        &self,
        account: &str,
        recipe: SessionRecipe,
        auth_fingerprint: &str,
        session: H5Session,
    ) {
        let mut map = self.lock();
        map.retain(|k, _| {
            !(k.account == account && k.recipe == recipe && k.auth_fingerprint != auth_fingerprint)
        });
        map.insert(
            CacheKey {
                account: account.to_string(),
                recipe,
                auth_fingerprint: auth_fingerprint.to_string(),
            },
            session,
        );
    }

    /// 丢掉一条（刷新时用：刚失败的那条不能留）。
    pub fn remove(&self, account: &str, recipe: SessionRecipe, auth_fingerprint: &str) {
        self.lock().remove(&CacheKey {
            account: account.to_string(),
            recipe,
            auth_fingerprint: auth_fingerprint.to_string(),
        });
    }

    /// 当前缓存条数（诊断与测试用）。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for SessionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 刻意不打印票值 —— 这个结构会进日志
        f.debug_struct("SessionCache")
            .field("entries", &self.len())
            .finish()
    }
}

/// 凭据指纹：SHA-256 前 8 字节的 hex。
///
/// 存指纹而不是凭据本身，因为这个键会进日志与测试输出。
pub fn auth_fingerprint(auth_raw: &str) -> String {
    // 取 SHA-256 hex 的前 16 个字符（= 前 8 字节）。
    // 复用 `sha256_hex` 而不是另开一个按字节取摘要的接口：指纹只用于分组，
    // 16 个 hex 字符的碰撞概率在这个规模下可以忽略。
    mcloud_crypto::sha256_hex(auth_raw)
        .chars()
        .take(16)
        .collect()
}

/// 从账号构造 `Authorization: Basic …` 头。
///
/// 形状：`Basic base64("mobile:<手机号>:<authToken>")`。
/// 与 App 登录链路同一个形状，所以直接复用账号里存的原始串。
pub fn basic_authorization(account: &Account) -> String {
    // ⚠⚠ **`auth.raw` 本身就带 `Basic ` 前缀，这里一个字都不能再加。**
    //
    // 契约在解析处写着：`auth.rs` 拼 `AuthInfo` 时是
    // `raw: format!("Basic {clean}")`；实拍的 `Authorization` 头值也是
    // `Basic base64("mobile:<手机号>:<authToken>")`，整串 302 字符
    // （`docs/capture/app-api-families.txt:86`）。
    //
    // 再加一次得到的是 **`Basic Basic bW9iaW…`**，上游的回话是
    // `{"code":"05050009","message":"非法的token！"}` —— 这句话**听起来像
    // "凭据过期了"**，于是会去查 token 有效期（那儿一切正常，还剩 29.8 天），
    // 而真相是**头值拼错了**。一次排查就这么被带到隔壁。
    //
    // 反证：发送层（`builder.rs` 的 `basic_token: Some(account.auth.raw)`）
    // 是**逐字**注入的 —— 那一处才是对的形状。
    account.auth.raw.clone()
}

/// 把一次响应的状态与体交给调用方判定。
///
/// 只做 HTTP 层的检查：业务码由各段自己的解析器判 —— 它们的信封形状不同
/// （`querySpecTokenV2` 是 `code:"0000"` 字符串，`tyrzLogin` 是 `code:0` 数字）。
pub fn ensure_http_ok<'a>(resp: &'a HttpResponse, what: &str) -> Result<&'a str> {
    if !(200..300).contains(&resp.status) {
        return Err(Error::Network(format!(
            "{what} 失败，HTTP {}：{}",
            resp.status,
            resp.body.chars().take(200).collect::<String>()
        )));
    }
    Ok(&resp.body)
}

#[cfg(test)]
mod tests;
