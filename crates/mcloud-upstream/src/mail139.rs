//! 139 邮箱（`mail139`）族协议层 —— 登录取 session、发邮件、PDF 转换、邮箱备份、
//! 市场打卡、PE 活跃上报。
//!
//! 依据：旧 TS `packages/core/src/tasks/auth-service.ts`（登录）、
//! `packages/core/src/tasks/app/runners.ts:132-186`（各 runner 的编排）、
//! `packages/core/src/apis/mail139.ts`（compose / convert / PE 上报）、
//! `packages/core/src/apis/auth.ts:126-144`（`loginMail`）。
//! 全部**逐字照抄请求形状**（URL、查询串、头、体）。
//!
//! # 这一族的主机与本仓别处**都不一样**
//!
//! `mail.10086.cn` / `html5.mail.10086.cn` / `appmail.mail.10086.cn` /
//! `pesupport.mail.10086.cn` —— 四台主机，都不吃云盘那套 `Authorization` /
//! `jwttoken` / `x-yun-*` 头。凭证是 **Cookie：`RMKEY=<rmkey>; Os_SSo_Sid=<sid>`**
//! ⇒ 这里每一枪都把 Cookie 显式拼出来，**不经过** `H5Client` 的按主机注入
//!（那层注入的是云盘凭证，打到这几台主机上是多余的、且没有样本支持）。
//!
//! # ⚠ 三处**今天已经跑不通**（2026-10-07 实测，见 `docs/review/ts-vs-rust-task-gap-261007.md`）
//!
//! * [`market_task_click_v2_request`] —— `POST …/ycloud/signin/task/taskClickV2` **整体 404**
//!   （两个 `marketname` 都试过）。TS 的 472/473 就靠它推进，所以那两条今天**没有可见效果**。
//! * [`mail_backup_request`] —— `POST /mailsms/sms/backup` **404**。
//! * 107 云笔记的鉴权口（不在本模块）回不到 `app_auth`。
//!
//! ⇒ 照抄实现（口径：TS 能跑就照它写），但**调用方必须如实把失败报出来**，
//! 不能因为"上游坏了"就静默当成功。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};
use mcloud_crypto::{gzip_with_os19, md5_hex};

/// 邮箱登录（用 SSO token 换 sid/rmkey）。
pub const MAIL_LOGIN_URL: &str = "https://mail.10086.cn/login/inlogin.action";
/// 邮箱自动登录（拿 session 去打一次，算"活跃"）。
pub const MAIL_AUTOLOGIN_URL: &str = "https://mail.10086.cn/login/autologin.action";
/// 邮箱资产备份（⚠ 2026-10-07 实测 404）。
pub const MAIL_BACKUP_URL: &str = "https://mail.10086.cn/mailsms/sms/backup";
/// 邮箱族换 SSO token 用的 `toSourceId`（旧 TS `auth-service.ts:12` 的 `001003`）。
pub const MAIL_SSO_SOURCE_ID: &str = "001003";
/// 市场任务打卡（⚠ 2026-10-07 实测 404）。
pub const MARKET_TASK_CLICK_V2_URL: &str =
    "https://caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskClickV2";
/// 客户端版本号（旧 TS 从 `@asunajs/shared/constants` 取 `CAIYUN_APP_VERSION`）。
pub const CAIYUN_APP_VERSION: &str = "11.2.1";
/// 发信/取 composeId 的主机。
pub const APPMAIL_HOST: &str = "https://appmail.mail.10086.cn";
/// 网页版邮箱主机（PDF 转换 / `queryUserToken` 都打这里）。
pub const HTML5_MAIL_HOST: &str = "https://html5.mail.10086.cn";
/// PE 活跃事件上报口。
pub const PE_REPORT_URL: &str = "https://pesupport.mail.10086.cn/logreport/s";
/// PE 鉴权密钥 = `MD5("password")`（旧 TS `mail139.ts:7` 逐字）。
pub const PE_SECRET: &str = ":5f4dcc3b5aa765d61d8327deb882cf99";
/// 邮箱族那几枪共用的 UA（旧 TS `mail139.ts:8-9` 逐字）。
pub const OKHTTP_UA: &str = "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) \
AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36";

/// 邮箱 session（sid + rmkey）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSession {
    pub sid: String,
    pub rmkey: String,
}

impl MailSession {
    /// 这一族每一枪都带的那枚 Cookie（旧 TS `buildHeaders` / `getComposeId` 逐字）。
    pub fn cookie(&self) -> String {
        format!("RMKEY={}; Os_SSo_Sid={}", self.rmkey, self.sid)
    }
}

/// 上游那几口回的是**单引号 JSON**（旧 TS `parseLooseJson`：`text.replace(/'/g, '"')`）。
///
/// ⚠ 照抄那个朴素替换，不"顺手"换成宽容解析器：多解析出来的东西
///（比如正文里带单引号的英文）会让我们比 TS **多认**一些字段，
/// 而"比参考实现多认"没有样本支持。
pub fn loose_json(text: &str) -> Result<Value> {
    // ① **先按严格 JSON 解** ✓
    //
    // ⚠⚠ 这一句是 2026-10-07 真机踩出来的（用户截图），顺序**不能反**：
    // 登录响应的 `var` 里有一格 `val2`，它的**值本身**就是一段带单引号的串：
    //
    // ```text
    // "val2":"{'billTime':'','cloudPostTime':'2024-04-09',…}"
    // ```
    //
    // 先做全局替换就会把**串里面**那些单引号也换掉 ⇒ 一份**完全合法**的 JSON
    // 被兜底改坏 ⇒ 报 `expected ',' or '}' at line 52 column 16` ✗
    // —— 而那句话听起来像"上游发了脏数据"，会把人带去查上游 ✗（实际是我们自己弄坏的）。
    // ⚠ 更阴的是它**按账号**触发：那格是用户自己的配置数据，`测试` 号有、`移动` 号没有
    // ⇒ 同一个功能在一个号上好好的、换个号就炸 ✗（最难查的那一类）。
    // ⚠ 而且这一口**本来就是严格 JSON**：旧 TS `tasks/auth-service.ts:35` 那句
    // `typeof res === "string" ? JSON.parse(res) : res` 是**纯 JSON.parse**，
    // 它压根没有这个替换 ✓ ⇒ 严格那一支才是"照抄参考实现" ✓。
    if let Ok(v) = serde_json::from_str(text) {
        return Ok(v);
    }
    // ② 解不动才退到"宽松"那一档（单引号当双引号用 ✓）—— 那是给**别的**几口用的
    //（旧 TS `parseLooseJson` ✓）。这一步**只会**在严格解析已经失败之后发生 ⇒
    // 不会比原来"多认"任何字段 ✓。
    let replaced = text.replace('\'', "\"");
    serde_json::from_str(&replaced).map_err(|e| {
        Error::Parse(format!(
            "邮箱响应不是（宽松）JSON：{e}；原文前 200 字：{}",
            text.chars().take(200).collect::<String>()
        ))
    })
}

/// XML 实体转义（旧 TS `auth.ts:102-109` 逐字）。
pub fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── ① 登录 ──────────────────────────────────────────────────────────────

/// `POST https://mail.10086.cn/login/inlogin.action`（旧 TS `auth.ts:129-144` 逐字）。
pub fn mail_login_request(sso_token: &str) -> HttpRequest {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n      <object>\n       \
         <string name=\"clientId\">10804</string>\n       \
         <string name=\"version\">9</string>\n       \
         <string name=\"loginType\">7</string>\n       \
         <string name=\"token\">{}</string>\n      </object>",
        escape_xml(sso_token)
    );
    HttpRequest::post(MAIL_LOGIN_URL)
        .header("content-type", "application/xml;charset=UTF-8")
        .text_body(body)
}

/// 解析登录响应里的 `var.sid` / `var.rmkey`。
///
/// ⚠ 旧 TS 先做 `typeof res === "string" ? JSON.parse(res) : res` —— 也就是说
/// 响应既可能是字符串也可能是已解析对象。本仓的 `HttpResponse.body` 恒是字符串
/// ⇒ 只有一支。这一格**今天实测是能拿到的**（`{"sid":"00c5…","rmkey":"2f0a…"}`）。
pub fn parse_mail_session(body: &str) -> Result<MailSession> {
    let v = loose_json(body)?;
    let var = v.get("var").ok_or_else(|| {
        Error::Parse(format!(
            "邮箱登录响应里没有 var：{}",
            body.chars().take(200).collect::<String>()
        ))
    })?;
    let sid = var.get("sid").and_then(Value::as_str).unwrap_or_default();
    let rmkey = var.get("rmkey").and_then(Value::as_str).unwrap_or_default();
    if sid.is_empty() || rmkey.is_empty() {
        return Err(Error::Parse(format!(
            "邮箱登录没给全 sid/rmkey：{}",
            body.chars().take(200).collect::<String>()
        )));
    }
    Ok(MailSession {
        sid: sid.to_string(),
        rmkey: rmkey.to_string(),
    })
}

/// `GET /login/autologin.action?sid=&rmkey=` —— 472/473 的"打一次活跃"。
pub fn mail_autologin_request(session: &MailSession) -> HttpRequest {
    HttpRequest::get(format!(
        "{MAIL_AUTOLOGIN_URL}?sid={}&rmkey={}",
        session.sid, session.rmkey
    ))
}

// ── ② 市场打卡（472/473/1004 的"推进市场步"）────────────────────────────

/// `POST …/ycloud/signin/task/taskClickV2`。
///
/// ⚠ **2026-10-07 实测：整个端点 404**（`sign_in_3` 与 `newsign_139mail` 都试过）。
/// 照抄形状；调用方按 `ensure_http_ok` 如实报失败。
pub fn market_task_click_v2_request(task_id: i64, marketname: &str) -> HttpRequest {
    let body = serde_json::json!({
        "marketname": marketname,
        "clientVersion": CAIYUN_APP_VERSION,
        "taskId": task_id,
    });
    HttpRequest::post(MARKET_TASK_CLICK_V2_URL)
        .header("Content-Type", "application/json")
        .json_body(&body)
}

// ── ③ 发邮件（1004）────────────────────────────────────────────────────

/// `POST {APPMAIL_HOST}/RmWeb/mail?func=mbox:getComposeId&sid=`。
pub fn get_compose_id_request(session: &MailSession) -> HttpRequest {
    HttpRequest::post(format!(
        "{APPMAIL_HOST}/RmWeb/mail?func=mbox:getComposeId&sid={}",
        session.sid
    ))
    .header("Cookie", format!("RMKEY={}", session.rmkey))
    .header("Content-Type", "application/xml;charset=UTF-8")
    .header("Accept", "*/*")
    .header("User-Agent", OKHTTP_UA)
    .text_body("")
}

/// 从 `getComposeId` 的响应里取 composeId。
///
/// 旧 TS 接受两种形状：`var` 是字符串，或 `var` 是对象（取 `m` / `id`）。
/// 取不到 ⇒ `None`（调用方报"取不到 composeId，跳过"）。
pub fn parse_compose_id(body: &str) -> Result<Option<String>> {
    let v = loose_json(body)?;
    let var = v.get("var");
    if let Some(s) = var.and_then(Value::as_str) {
        if !s.is_empty() {
            return Ok(Some(s.to_string()));
        }
    }
    if let Some(obj) = var.and_then(Value::as_object) {
        for key in ["m", "id"] {
            if let Some(s) = obj.get(key).and_then(Value::as_str) {
                if !s.is_empty() {
                    return Ok(Some(s.to_string()));
                }
            }
        }
    }
    Ok(None)
}

/// 发信所需的字段（旧 TS `ComposeMailParams` 里 1004 用到的那些）。
#[derive(Debug, Clone)]
pub struct ComposeMail {
    pub compose_id: String,
    pub account: String,
    pub to: String,
    pub subject: String,
    pub content: String,
    /// 旧 TS 1004 那支传 `isHtml: 0`、`action: "asyn"`。
    pub is_html: i64,
    pub action: String,
}

/// `POST {APPMAIL_HOST}/RmWeb/mail?func=mbox:compose&peFlag=1&sid=`（旧 TS 逐字 XML）。
pub fn compose_mail_request(session: &MailSession, mail: &ComposeMail) -> HttpRequest {
    let e = escape_xml;
    let xml = format!(
        "<object>\
         <object name='attrs'>\
         <string name='id'>{}</string>\
         <string name='mid'></string>\
         <string name='messageId'></string>\
         <string name='account'>{}</string>\
         <string name='to'>{}</string>\
         <string name='cc'></string>\
         <string name='bcc'></string>\
         <string name='replyTo'></string>\
         <string name='subject'>{}</string>\
         <string name='content'>{}</string>\
         <int name='isHtml'>{}</int>\
         <int name='priority'>3</int>\
         <int name='saveSentCopy'>1</int>\
         <int name='scheduleDate'>0</int>\
         <array name='attachments'></array>\
         </object>\
         <string name='action'>{}</string>\
         </object>",
        e(&mail.compose_id),
        e(&mail.account),
        e(&mail.to),
        e(&mail.subject),
        e(&mail.content),
        mail.is_html,
        e(&mail.action),
    );
    HttpRequest::post(format!(
        "{APPMAIL_HOST}/RmWeb/mail?func=mbox:compose&peFlag=1&sid={}",
        session.sid
    ))
    .header("Cookie", format!("RMKEY={}", session.rmkey))
    .header("Content-Type", "text/xml; charset=UTF-8")
    .header("Accept", "*/*")
    .header("User-Agent", OKHTTP_UA)
    .text_body(xml)
}

/// 邮箱族那几口的公共回信封：`{ code, summary }`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailCode {
    pub code: String,
    pub summary: Option<String>,
}

impl MailCode {
    /// 旧 TS 的成功判据就是 `code === "S_OK"`（逐字）。
    pub fn is_ok(&self) -> bool {
        self.code == "S_OK"
    }
}

/// 解析 `{ code, summary }` 那一族。
pub fn parse_mail_code(body: &str) -> Result<MailCode> {
    let v = loose_json(body)?;
    let code = match v.get("code") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => {
            return Err(Error::Parse(format!(
                "邮箱响应里没有 code：{}",
                body.chars().take(200).collect::<String>()
            )))
        }
    };
    let summary = v.get("summary").and_then(Value::as_str).map(str::to_string);
    Ok(MailCode { code, summary })
}

// ── ④ PE 活跃上报（1004 发完信之后）──────────────────────────────────────

/// 一条 PE 事件（旧 TS `PeActiveLogEntry`；1004 只用到 `eventType = "3"`）。
#[derive(Debug, Clone)]
pub struct PeActiveEvent {
    /// `yyyy-MM-dd HH:mm:ss`（服务器本地时区，旧 TS `formatPeEventTime`）。
    pub event_time: String,
    /// `"1".."6"`；`"3"` = 发邮件。
    pub event_type: String,
}

/// `POST {PE_REPORT_URL}?func=user:activeAction&cguid=&sid=` —— **gzip 体 + MD5 鉴权头**。
///
/// 鉴权算法（旧 TS `peAuthHeader` 逐字）：
/// `ts = yyyyMMddHHmmss`，`Authorization = "Basic " + MD5(ts + PE_SECRET)`。
///
/// ⚠ `cguid` 是 8 位随机 hex（旧 TS `randomCguid`）⇒ 由调用方给，
/// 让本函数保持纯函数、测试可复现。
pub fn report_active_action_request(
    session: &MailSession,
    events: &[PeActiveEvent],
    cguid: &str,
    now: &PeClock,
) -> Result<HttpRequest> {
    let ts = now.stamp();
    let auth = format!("Basic {}", md5_hex(&format!("{ts}{PE_SECRET}")));

    let log_datas: Vec<Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "eventTime": e.event_time,
                "eventType": e.event_type,
            })
        })
        .collect();
    let payload = serde_json::json!({
        "body": { "logDatas": log_datas },
        "head": {
            "appCode": "2a76b8c7-e9c4-478b-b1fd-0b2516cffd86",
            "comeFrom": "4027001",
            "eMode": "1",
            "installPack": "139PushEmail_AC20X_10.3.2B20240906.4027001.apk",
            "manufacturers": "Xiaomi",
            "model": "MIX 2",
            "netType": "1",
            "os": "Android",
            "osVersion": "9",
            "version": "10.3.2",
        },
    });
    let gz = gzip_with_os19(payload.to_string().as_bytes())
        .map_err(|e| Error::Other(format!("PE 上报体 gzip 失败：{e}")))?;

    Ok(HttpRequest::post(format!(
        "{PE_REPORT_URL}?func=user:activeAction&cguid={cguid}&sid={}",
        session.sid
    ))
    .header("Authorization", auth)
    .header("Accept", "application/json")
    .header("Content-Encoding", "gzip")
    .header("Content-Type", "application/json")
    .header("Cookie", format!("RMKEY={}", session.rmkey))
    .header("User-Agent", "Apache-HttpClient/UNAVAILABLE (java 1.4)")
    .bytes_body(gz))
}

/// PE 鉴权头要的两个时间串。
///
/// 单独成类型是为了让"本地时区"这件事**只有一处**：旧 TS 用的是
/// `new Date()` 的**本地**字段（`getHours()` 那一家），不是 UTC。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeClock {
    /// `yyyyMMddHHmmss`（鉴权头用）。
    pub ts: String,
    /// `yyyy-MM-dd HH:mm:ss`（事件体用）。
    pub event_time: String,
}

impl PeClock {
    /// 用本地时间构造（两个串同源，避免"头用一个时刻、体用另一个"）。
    pub fn from_local(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Self {
        let p = |n: u32| format!("{n:02}");
        Self {
            ts: format!(
                "{year}{}{}{}{}{}",
                p(month),
                p(day),
                p(hour),
                p(minute),
                p(second)
            ),
            event_time: format!(
                "{year}-{}-{} {}:{}:{}",
                p(month),
                p(day),
                p(hour),
                p(minute),
                p(second)
            ),
        }
    }

    pub fn stamp(&self) -> &str {
        &self.ts
    }
}

// ── ⑤ PDF 转换（1014）──────────────────────────────────────────────────

/// `disk:convertPdfFromYunFile` 的入参。
#[derive(Debug, Clone)]
pub struct ConvertPdf {
    pub file_name: String,
    pub file_real_size: usize,
    pub file_id: String,
    /// 旧 TS 1014 传 `1`。
    pub convert_type: i64,
}

/// `html5.mail.10086.cn/mw2/file/disk?func=disk:convertPdfFromYunFile&…`
///（旧 TS `mail139.ts:72-76` 的 `buildQuery` + `:119-146`）。
///
/// `rnd` / `cguid` 是查询串里的随机格 ⇒ 由调用方给（纯函数、可复现）。
pub fn convert_pdf_request(
    session: &MailSession,
    params: &ConvertPdf,
    rnd: &str,
    cguid: &str,
) -> HttpRequest {
    let body = format!(
        "<object>\n  \
         <string name=\"fileName\">{}</string>\n  \
         <int name=\"fileRealSize\">{}</int>\n  \
         <string name=\"fileId\">{}</string>\n  \
         <int name=\"convertType\">{}</int>\n  \
         <string name=\"appType\">1</string>\n  \
         <string name=\"appVer\">10.0.3</string>\n  \
         <string name=\"channel\">10242000</string>\n\
         </object>",
        escape_xml(&params.file_name),
        params.file_real_size,
        escape_xml(&params.file_id),
        params.convert_type,
    );
    let url = format!(
        "{HTML5_MAIL_HOST}/mw2/file/disk?func=disk:convertPdfFromYunFile&sid={}&behaviorData=&rnd={rnd}&cguid={cguid}&k=&comefrom=166",
        session.sid
    );
    HttpRequest::post(url)
        .header("Content-Type", "application/xml;charset=UTF-8")
        .header("Accept", "*/*")
        .header("Cookie", session.cookie())
        .header("Origin", HTML5_MAIL_HOST)
        .header("Referer", "https://html5.mail.10086.cn/")
        .header("User-Agent", OKHTTP_UA)
        .header("X-Requested-With", "cn.cj.pe")
        .text_body(body)
}

// ── ⑥ 邮箱备份（1020，⚠ 实测 404）──────────────────────────────────────

/// `POST /mailsms/sms/backup?sid=`（旧 TS `runners.ts:285-294` 逐字，空 JSON 体）。
pub fn mail_backup_request(session: &MailSession) -> HttpRequest {
    HttpRequest::post(format!("{MAIL_BACKUP_URL}?sid={}", session.sid))
        .header("Content-Type", "application/json")
        .text_body("{}")
}

// ── ⑦ queryUserToken（1028 的 AI 域鉴权来源）────────────────────────────

/// `POST {HTML5_MAIL_HOST}/mw2/file/disk?func=disk:queryUserToken&…`
///（旧 TS `ai-workbench.ts:77-103` 逐字）。
pub fn query_user_token_request(session: &MailSession, rnd: &str, cguid: &str) -> HttpRequest {
    let url = format!(
        "{HTML5_MAIL_HOST}/mw2/file/disk?func=disk:queryUserToken&sid={}&behaviorData=&rnd={rnd}&cguid={cguid}&k=&comefrom=166",
        session.sid
    );
    HttpRequest::post(url)
        .header("Content-Type", "application/xml;charset=UTF-8")
        .header("Accept", "*/*")
        .header("Cookie", session.cookie())
        .header("Origin", HTML5_MAIL_HOST)
        .header("Referer", "https://html5.mail.10086.cn/")
        .header("User-Agent", OKHTTP_UA)
        .header("X-Requested-With", "cn.cj.pe")
        .text_body("<object></object>")
}

/// `queryUserToken` 解出来的内层（旧 TS `QueryUserTokenInner`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryUserToken {
    /// DES-ECB 加密的手机号（key = `139_mail`）。
    pub account: String,
    pub token: String,
    pub user_domain_id: Option<String>,
}

/// 解析 `var.data.data.{account,token,userDomainId}`。
pub fn parse_query_user_token(body: &str) -> Result<QueryUserToken> {
    let v = loose_json(body)?;
    let inner = v
        .get("var")
        .and_then(|x| x.get("data"))
        .and_then(|x| x.get("data"))
        .ok_or_else(|| {
            Error::Parse(format!(
                "queryUserToken 响应形状不对（缺 var.data.data）：{}",
                body.chars().take(200).collect::<String>()
            ))
        })?;
    let account = inner
        .get("account")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let token = inner
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if account.is_empty() || token.is_empty() {
        return Err(Error::Parse(format!(
            "queryUserToken 没给全 account/token：{}",
            body.chars().take(200).collect::<String>()
        )));
    }
    Ok(QueryUserToken {
        account,
        token,
        user_domain_id: inner
            .get("userDomainId")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests;
