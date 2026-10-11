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

/// PE 页面自己的**行为上报像素** —— `peCommonFun.js` 里的 `addBehavior` ✓。
///
/// ```js
/// function addBehavior(act, thi) {
///   var img = new Image();
///   img.src = location.protocol + '//smsrebuild1.mail.10086.cn/weather/weather'
///           + '?func=user:logBehaviorAction&action=' + act + '&thingId=' + thi
///           + '&version=html5&cguid=' + Math.random().toString(16).replace('.', '');
/// }
/// ```
///
/// 出处：**`https://html5.mail.10086.cn/html5/js/lib/peCommonFun.js`**（2026-10-11 取 ✓）。
/// 它是"用户打开了某个 thing"的埋点（那页在点下载/打开时调 `addBehavior(111949, thingId)` ✓）。
/// ⚠ 这一发有**两台主机**（同一功能的两个版本，2026-10-11 各读到一处 ✓）：
///
/// * PE 落地页那一版（`peCommonFun.js` ✓）：`location.protocol + '//smsrebuild1.mail.10086.cn/weather/weather'`
///   —— 绝对地址、带 `cguid` ✓（**试过 ✗**，见 §5-14）；
/// * **webmail 框架那一版**（`/html5/js/T.mvc.pack.js` 的 `T.addBehaviorAction` ✓）：
///   `"/mw2/weather/weather?func=user:logBehaviorAction&action=…&thingId=…&version=html5"`
///   —— **相对路径** ⇒ 落在页面自己的主机 `html5.mail.10086.cn` 上、**不带 `cguid`** ✓。
///
/// 本仓取**后者**：它就在我们邮箱会话所在的域上（cookie 天然对上 ✓），
/// 而且这一族其余口（`disk:*` ✓）也都在那台主机上 ✓。
pub const BEHAVIOR_ACTION_URL: &str = "https://html5.mail.10086.cn/mw2/weather/weather";

/// 账单页**加载时**那一发的 actionId ✓（`108365` / thingId `66`）。
///
/// 出处：**账单页自己的包** `https://html5.mail.10086.cn/js/mobilebill.html.pack.js`
/// （2026-10-11 逐字读的 ✓）—— 模型里那张表写着：
///
/// ```js
/// CONFIG_INFO: { common: {}, 1000006: { loadCount: [108365, 66] } }
/// // 页面初始化：
/// initConfigData: function () {
///   this.sendId = T.url.query("sendId") || "1000006";
///   var CONFIG_INFO = this.model.CONFIG_INFO[this.sendId];
///   CONFIG_INFO && T.addBehavior.apply(T, CONFIG_INFO.loadCount),   // ← 这一发 ✓
///   … }
/// ```
///
/// ⇒ 打开账单页 ⇒ **`addBehavior(108365, 66)`** ✓（`sendId` 缺省就是 `1000006` ✓）。
///
/// ⚠ 走过的两条岔路（都 ✗，见 §5-14）：PE 落地页那两个像素 `addBehavior(111949)`（下载 App ✗）与
/// `addBehavior(111948)`（在 App 里打开 ✗）—— 那两个是"装/打开 App"，不是"看账单" ✓。
pub const BILL_VIEW_ACTION: i64 = 108365;

/// 账单页加载那一发的 thingId（与 [`BILL_VIEW_ACTION`] 同源：`CONFIG_INFO[1000006].loadCount` ✓）。
pub const BILL_THING_ID: i64 = 66;

/// 发那一发行为上报像素（**纯 GET、响应体是空 200** ✓ —— 页面本身就是 `<img>` 干的 ✓）。
///
/// ⚠ **必须带上邮箱会话 cookie**：那一发是 fire-and-forget（回的是空 200，什么都不说 ✓），
/// 能不能记到**这个账号**头上全靠 cookie ✓。浏览器发那枚 `<img>` 时会带上
/// `.mail.10086.cn` 域下的 cookie（正是登录时种下的 `RMKEY` / `Os_Sso_Sid` ✓），
/// 所以这里照 [`MailSession::cookie`] 拼 ✓（本仓这一族一贯显式拼 cookie，不走 H5Client 注入 ✓）。
/// 实测教训（2026-10-11）：不带 cookie 发那一发 ⇒ 任务读数**不动** ✗（run 328）；
/// 带上 cookie、两个 actionId（111949 / 111948）各发一趟，读数**照样不动** ✗（run 329/330）
/// ⇒ **今天没有调用方**，留着是因为"端点存在、形状长这样、上游回空 200"已经是读数 ✓
///（见 `docs/app-task-status.md` §5-14 ✓）。
pub fn log_behavior_action_request(
    session: &MailSession,
    action: i64,
    thing_id: i64,
    cguid: &str,
    behavior_data: &str,
) -> HttpRequest {
    // ⚠ 照 webmail 那一版**不带 `cguid`**（它那一句就是 `…&version=html5` 结束 ✓）；
    //   参数留着不用，是为了两版形状都在签名里看得见 ✓。
    let _ = cguid;
    // ⚠⚠ `behaviorData` —— **2026-10-11 真机实拍才看清的一格**（抓包仓
    // `app-task-run-account-samples.txt` §12.6 ✓）：它**不是密文** ✓，
    // 是**明文的"行为对"** `<action>_<thingId>`，而且账单页那一串请求**每一发都挂一对** ✓：
    //
    // ```text
    // user:getCommConfig             behaviorData=108365_66        ← 账单页加载那一对
    // user:logBehaviorAction         behaviorData=110542_6         ← 本函数这一发自己带的那对
    // /addrsvr/GetUserAddrJsonDataV2 behaviorData=110542_2|30129_82
    // user:getTrustedAuthState       behaviorData=110511_46
    // ```
    //
    // ⇒ 这一格是"**我这一发属于哪个行为**"的标签 ✓。本仓此前**一发都没带** ✗
    //（所以"发过像素但不翻账"未必是像素本身没用，可能是**标签缺了** ✓）。
    let mut url = format!(
        "{BEHAVIOR_ACTION_URL}?func=user:logBehaviorAction&action={action}&thingId={thing_id}&version=html5"
    );
    if !behavior_data.is_empty() {
        url.push_str(&format!("&behaviorData={behavior_data}"));
    }
    HttpRequest::get(url).header("Cookie", session.cookie())
}

/// `GET {HTML5_MAIL_HOST}/RmWeb/view.do?func=view:readMessage&…` —— **打开一封信**（账单页的"读"）。
///
/// 出处：**真机实拍**（2026-10-11 上午，1029 那两趟 ✓，抓包仓
/// `app-task-run-account-samples.txt` §12.6 ✓）—— 账单页在 WebView 里点开一封信时逐字发的就是这一发：
///
/// ```text
/// GET https://html5.mail.10086.cn/RmWeb/view.do
///     ?func=view:readMessage&sid=<38字符>&mid=<24字符>&callback=getMailInfo
///     &comefrom=166&fid=1&mode=mobile&resetdomain=0&cguid=<13字符>
/// Cookie: RMKEY=…; Os_Sso_Sid=…      Referer: https://html5.mail.10086.cn/html/readmail.html
/// → 200（回的是那封信的 HTML 正文，实测 35157 字节 ✓）
/// ```
///
/// ⚠ 三格容易写错的（都以实拍为准 ✓）：
/// * `comefrom=**166**` —— 不是 webmail 那套的 `5` ✗（166 是**账单页**这一格的实拍值 ✓）；
/// * `callback=getMailInfo` ✓（页面自己那枚 JSONP 回调名 ✓）；
/// * 必须带 `cguid` ✓，且 `Referer` 是 `readmail.html` ✓（账单页先开那个壳、再由它发这一发 ✓）。
pub fn read_message_request(session: &MailSession, cguid: &str, mid: &str) -> HttpRequest {
    HttpRequest::get(format!(
        "{HTML5_MAIL_HOST}/RmWeb/view.do?func=view:readMessage&sid={}&mid={mid}\
         &callback=getMailInfo&comefrom=166&fid=1&mode=mobile&resetdomain=0&cguid={cguid}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("Referer", "https://html5.mail.10086.cn/html/readmail.html")
}

/// `POST {HTML5_MAIL_HOST}/setting/s?func=bill:getBatterypitcherBill&sid=…` —— **账单月份清单**。
///
/// 出处：**账单页自己的包** `https://html5.mail.10086.cn/js/mobilebill.html.pack.js`
/// （2026-10-11 逐字读的 ✓；那一页是 App 字面量里的
/// `https://html5.mail.10086.cn/html/mobilebill.html?sid=` ✓ —— `mobilebill` = 手机账单 ✓）：
///
/// ```js
/// T.ajax.post({ url: "/setting/s?func=bill:getBatterypitcherBill&sid=" + T.url.query("sid"),
///               data: T.json2xml(null), headers: {"Content-Type":"application/xml"},
///               dataType: "xml",
///               success: function (responseText, res) {
///                   "S_OK" == res.code ? callback(res.billDates) : callback([]) } })
/// ```
///
/// ⚠ 体是 **`<null></null>`**（`T.json2xml(null)` 的产物 ✓，逐字照抄 ✓）—— 不是空体、也不是 `{}` ✗。
/// 这一发**是读**（回的是有账单的月份 ✓）。
///
/// ⚠ **今天没有调用方**：1029 试到这一版仍不翻账（见 §5-14）⇒ 半成品没进执行器表 ✗，
/// 只把这个"形状 + 上游回什么"的读数留下 ✓（2026-10-11 实测：移动那个号回
/// `{"code":"S_OK","summary":"","billDates":[]}` ✓ —— 路由通、会话可用、该号没有可补投月份 ✓）。
pub fn bill_dates_request(session: &MailSession) -> HttpRequest {
    HttpRequest::post(format!(
        "{HTML5_MAIL_HOST}/setting/s?func=bill:getBatterypitcherBill&sid={}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("Content-Type", "application/xml")
    .header("Accept", "*/*")
    .header("User-Agent", OKHTTP_UA)
    .text_body("<null></null>")
}

/// 从账单清单响应里取月份（`{"code":"S_OK","billDates":[…]}` ✓；拿不到就空表 ✓）。
pub fn bill_dates(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("billDates").and_then(|d| d.as_array()).map(|list| {
                list.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// 账单页主数据口：**在收件箱里按 `sendId` 条件搜账单邮件** ✓。
///
/// 出处：`/js/mobilebill.html.pack.js` 的 `getMailsList` ✓（2026-10-11 逐字读的）：
///
/// ```js
/// var reqURL = "/RmWeb/mail?func=mbox:searchMessages&sid=" + T.url.query("sid"),
///     mobileMails = this.mobileMailsSate,     // 那 5 条 sendId 条件
///     req = { fid:1, recursive:0, ignoreCase:0, isSearch:1, isFullSearch:0, start:…,
///             total:10, limit:1000, order:"receiveDate", desc:"1", highLight:0,
///             exceptFids:[4], condictions:mobileMails, addSendIdFlag:3, statType:1 };
/// T.ajax.post({url:reqURL, data:T.json2xml(req), headers:{"Content-Type":"application/xml"}, …})
/// ```
///
/// 体是 `T.json2xml` 的产物（`<int>/<string>/<array>/<object>` ✓，见 `T.mvc.pack.js` 的
/// `namedVarToXML` ✓）—— 这里逐格照抄 ✓。
pub fn bill_mails_search_request(session: &MailSession, cguid: &str) -> HttpRequest {
    let cond = |field: &str, op: &str, value: i64| {
        format!(
            "<object><string name=\"field\">{field}</string>\
             <string name=\"operator\">{op}</string>\
             <int name=\"value\">{value}</int></object>"
        )
    };
    let conditions = [
        cond("sendId", "=", 1000006),
        cond("sendId1", "=", 1000007),
        cond("sendId2", "GE", 1000301),
        cond("sendId2", "LE", 1000700),
        cond("sendId3", "=", 1000010),
    ]
    .join("");
    let body = format!(
        "<object>\
        <int name=\"fid\">1</int>\
        <int name=\"recursive\">0</int>\
        <int name=\"ignoreCase\">0</int>\
        <int name=\"isSearch\">1</int>\
        <int name=\"isFullSearch\">0</int>\
        <int name=\"start\">0</int>\
        <int name=\"total\">10</int>\
        <int name=\"limit\">1000</int>\
        <string name=\"order\">receiveDate</string>\
        <string name=\"desc\">1</string>\
        <int name=\"highLight\">0</int>\
        <array name=\"exceptFids\"><int>4</int></array>\
        <array name=\"condictions\">{conditions}</array>\
        <int name=\"addSendIdFlag\">3</int>\
        <int name=\"statType\">1</int>\
        </object>"
    );
    HttpRequest::post(format!(
        "{MAIL_SEARCH_URL}?func=mbox:searchMessages&sid={}&cguid={cguid}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("Content-Type", "application/xml")
    .header("Accept", "*/*")
    .header("User-Agent", OKHTTP_UA)
    .header("X-Requested-With", "cn.cj.pe")
    .text_body(body)
}

/// 账单页里的「转存账单邮件」：`POST {HTML5_MAIL_HOST}/mw2/file/disk?func=disk:backupAllBillMail&sid=`。
///
/// 体照 `saveBillMails` 拼（`{mailIds: mailMid}` ✓ → `T.json2xml` ✓）。
pub fn backup_all_bill_mail_request(
    session: &MailSession,
    cguid: &str,
    mail_ids: &str,
) -> HttpRequest {
    let body = format!("<object><string name=\"mailIds\">{mail_ids}</string></object>");
    HttpRequest::post(format!(
        "{HTML5_MAIL_HOST}/mw2/file/disk?func=disk:backupAllBillMail&sid={}&cguid={cguid}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("Content-Type", "application/xml")
    .header("Accept", "*/*")
    .header("User-Agent", OKHTTP_UA)
    .text_body(body)
}

/// 账单页的「**把这封信上报到 MQ**」：`POST {HTML5_MAIL_HOST}/mw2/file/disk?func=disk:mailReportToMQ&…`。
///
/// 出处：**真机实拍**（2026-10-11，抓包仓 `app-task-run-account-samples.txt` §12.6 ✓）——
/// 账单页在"打开那一封"之后连发**两发** ✓：
///
/// ```text
/// POST https://html5.mail.10086.cn/mw2/file/disk
///      ?func=disk:mailReportToMQ&sid=<掩>&rnd=<随机小数>&cguid=<掩>&comefrom=166
/// Content-Type: application/xml          ← ⚠ 头写 xml ✗ 而体是 **JSON** ✓（实拍如此 ✓）
/// Referer: https://html5.mail.10086.cn/html/readmail.html
/// 体: {"actionType":"1003","clientType":"2","imapId":"<24字符>","clientVersion":"h5","channel":""}
/// → { "code": "S_OK", "summary":"successed to process!"}
/// ```
///
/// ⚠ `imapId` 那一格实拍是 **24 字符**，与同一趟 `view:readMessage` 的 `mid` **同长度** ✓
/// ⇒ 这里就传 `mid` ✓（`actionType` 逐字 `"1003"` ✓，是**字符串**不是数字 ✗）。
pub fn mail_report_to_mq_request(
    session: &MailSession,
    cguid: &str,
    rnd: &str,
    body: &str,
) -> HttpRequest {
    HttpRequest::post(format!(
        "{HTML5_MAIL_HOST}/mw2/file/disk?func=disk:mailReportToMQ&sid={}&rnd={rnd}&cguid={cguid}&comefrom=166",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("Content-Type", "application/xml")
    .header("Referer", "https://html5.mail.10086.cn/html/readmail.html")
    .text_body(body.to_string())
}

/// `mailReportToMQ` 的体（**JSON** ✓，尽管请求头写的是 `application/xml` ✗ —— 实拍如此 ✓）。
///
/// 实拍那一趟**三发**，两种取值 ✓：
///
/// ```text
/// {"actionType":"1003","clientType":"2","imapId":"<24字符>","clientVersion":"h5","channel":""}        ×2
/// {"actionType":"1017","clientType":"1","imapId":"<24字符>","clientVersion":"h5","channel":"10242400"} ×1
/// ```
///
/// ⇒ 两个 `actionType` 各来一发 ✓（顺序照实拍：先 `1003` 后 `1017` ✓）。
pub fn mail_report_body(
    action_type: &str,
    client_type: &str,
    imap_id: &str,
    channel: &str,
) -> String {
    format!(
        "{{\"actionType\":\"{action_type}\",\"clientType\":\"{client_type}\",\
         \"imapId\":\"{imap_id}\",\"clientVersion\":\"h5\",\"channel\":\"{channel}\"}}"
    )
}

/// 账单页本体：`GET {HTML5_MAIL_HOST}/html/mobilebill.html?sendId=1000006&sid=&cguid=`。
///
/// 出处：**真机实拍**（2026-10-11，抓包仓 `app-task-run-account-samples.txt` §12.6 ✓）——
/// 那一趟逐字是 `…/html/mobilebill.html?sendId=1000006&sid=<掩>&cguid=<掩>` ✓，
/// 紧跟其后拉页面自己的包 `…/html5/js/mobilebill.html.pack.js` ✓。
///
/// ⚠ `sendId=**1000006**` 是**话费账单**那一格（页面 `initConfigData` 里缺省就是它 ✓）；
/// 这一格**必须带** ✗ —— 不带就是"没指定哪本账"（早先那一版就是这样 ✗，见 §5-14）。
/// `cguid` 也照实拍带上 ✓。
pub fn bill_page_request(session: &MailSession, cguid: &str) -> HttpRequest {
    HttpRequest::get(format!(
        "{HTML5_MAIL_HOST}/html/mobilebill.html?sendId=1000006&sid={}&cguid={cguid}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("User-Agent", OKHTTP_UA)
    .header("Referer", "https://html5.mail.10086.cn/?id=bill")
}

/// App 字面量里的另一条账单入口：`GET {HTML5_MAIL_HOST}/?id=bill`。
pub fn bill_entry_request() -> HttpRequest {
    HttpRequest::get(format!("{HTML5_MAIL_HOST}/?id=bill")).header("User-Agent", OKHTTP_UA)
}

/// 账单订阅 SSO（账单页里 `mobile.billAccount` 指的那一发 ✓）。
pub fn bill_subscribe_sso_request(session: &MailSession) -> HttpRequest {
    HttpRequest::get(format!(
        "https://happy.mail.10086.cn/jsp/cn/billsubscibe/sso.do?sid={}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("User-Agent", OKHTTP_UA)
}

/// 从 `mbox:searchMessages` 的响应里取第一枚邮件号（`mid`）——「转存账单邮件」那一发的入参。
pub fn first_mail_id(body: &str) -> Option<String> {
    // ⚠ 别用 `serde_json` 解这一族响应 ✗：实拍那 12886 字节的体不是标准 JSON
    //   （2026-10-11：`serde_json::from_str` 直接失败 ✓；键名靠文本扫才认得出来 ✓）。
    //   先走本仓 1020 那条链用的 [`first_field`]（`name="mid">…<` 与 `"mid":"…"` ✓），
    //   不中再补**第三种形状**：值**不带引号**（`"mid":12345,` ✓ —— 实拍就是这种 ✓）。
    if let Some(mid) = first_field(body, "mid") {
        return Some(mid);
    }
    // ⚠⚠ 实拍的壳是**混着的**：键单引号、值双引号 ——
    //   `{'code':'S_OK','var':[{'mid':"d506a18bd1dbbb2900000001",` ✓
    //   （2026-10-11 逐字读的 ✓）。所以取值要分两步：先认引号壳、再读引号里的内容 ✓。
    for needle in ["\"mid\":", "'mid':"] {
        let Some(pos) = body.find(needle) else {
            continue;
        };
        let tail = body[pos + needle.len()..].trim_start();
        let mut chars = tail.chars();
        let first = chars.next();
        let value: String = match first {
            Some(q @ ('"' | '\'')) => chars.take_while(|c| *c != q).collect(),
            Some(_) => tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect(),
            None => String::new(),
        };
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// 「把这封账单邮件标为已读」：`POST {MAIL_SEARCH_URL}?func=mbox:updateMessagesStatus&sid=`。
///
/// 体照 `setCardMailReaded` 拼（`{ids:[mid], type:"read", value:…}` ✓ → `T.json2xml` ✓）。
pub fn mark_mail_read_request(session: &MailSession, cguid: &str, mid: &str) -> HttpRequest {
    let body = format!(
        "<object><array name=\"ids\"><string>{mid}</string></array>\
         <string name=\"type\">read</string>\
         <string name=\"value\">1</string></object>"
    );
    HttpRequest::post(format!(
        "{MAIL_SEARCH_URL}?func=mbox:updateMessagesStatus&sid={}&cguid={cguid}",
        session.sid
    ))
    .header("Cookie", session.cookie())
    .header("Content-Type", "application/xml")
    .header("Accept", "*/*")
    .header("User-Agent", OKHTTP_UA)
    .header("X-Requested-With", "cn.cj.pe")
    .text_body(body)
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

// ── ⑥ 邮箱备份（1020，⚠ 旧路 `mailsms/sms/backup` 实测 404）────────────

/// `POST /mailsms/sms/backup?sid=`（旧 TS `runners.ts:285-294` 逐字，空 JSON 体）。
///
/// ⚠⚠ **这一条今天 404，已经没有调用方**（2026-09-26 Go 探针 + 本仓多趟真跑各一次，
/// 回的都是**网页**不是接口 ⇒ 那台主机上除了 `/login/inlogin.action` 之外本族试过的
/// 路径都不认）。保留它只为"TS 就是这么写的"这一条出处；**1020 现在走
/// [`backup_mail_request`]**（App 实拍的 `disk:backupMail`）✓。
pub fn mail_backup_request(session: &MailSession) -> HttpRequest {
    HttpRequest::post(format!("{MAIL_BACKUP_URL}?sid={}", session.sid))
        .header("Content-Type", "application/json")
        .text_body("{}")
}

// ── 1020 邮件转存：**活的那条路**（App 实拍三发，2026-10-10 接上）────────
//
// 旧 TS 的 1020 只有上面那条 404 的口 ⇒ 这一族在参考实现里**没有活路**。
// 活路来自 **139邮箱 App 自己的字面量**（本仓逆向，见
// `../mcloud-capture/docs/upstream/mail139-app-cloud-bridge.md` §3.2）：
//
// | 步 | App 里的字面量 | 触发它的方法（同簇字符串） |
// |---|---|---|
// | ① 拿云盘目录 | `%@/file/disk?func=disk:getDirectorys&sid=%@&comefrom=54&cguid=%@` | `PMCloudAPIHelper` |
// | ② 拿一枚邮件号 | `…/RmWeb/mail?func=mbox:SearchByFidOrCondition&peFlag=1&sid=%@&comefrom=54&cguid=%@&encryptType=1` | `PMHttpFetchMail` |
// | ③ 转存 | `%@?func=disk:backupMail&sid=%@&comefrom=16861&cguid=%@` | `-[PMCaixunInterface uploadMailToCloudWithMid:directoryId:user:onFinish:]` |
//
// ⚠ **主机**：App 的模板里是 `%@`（运行期路由域，`PMRouteDomainManager`），
// 本仓取 **`html5.mail.10086.cn` + `/mw2/file/disk`** —— 依据是**抓包**：
// 同一张邮箱会话在 `html5.mail.10086.cn/mw2/file/disk?func=disk:*` 上连**写**
// 都被认（`disk:surplusNum` 200 / `disk:convertPdfFromYunFile` `S_OK` ✓，
// 见 `app-mail139-samples.txt` §8/§9），而那台 `mail.10086.cn` 今天只认换会话那一条 ✓。
// 这是**拼装**（App 的体 + 抓包的宿主），不是逐字实拍 ⇒ 标注在这里，别当逐字用 ✓。

/// 邮件列表（拿 `mid`）—— App 实拍：`mbox:SearchByFidOrCondition`。
pub const MAIL_SEARCH_URL: &str = "https://appmail.mail.10086.cn/RmWeb/mail";

/// `POST {MAIL_SEARCH_URL}?func=mbox:SearchByFidOrCondition&…` —— 收件箱取一页。
///
/// 体是**拼装**的（App 里那一支是多条独立串）：收件箱 `fid=1`、按收信时间倒序、
/// 只要 10 条 —— 目的只有一个：**拿到一枚 `mid`**（转存那一发的入参）。
///
/// ⚠⚠ **不带给 `encryptType=1`**（App 那一支的字面量里**有**它）：2026-10-10 真机实测，
/// 带上它之后响应体是 **3296 个十六进制字符**（上游按客户端加密回的，而我们没有那一套密钥）
/// ⇒ 拿不到 `mid`。去掉之后回的是可读体 ✓（同族其余口也都不带它 ✓）。
pub fn mail_search_request(session: &MailSession, cguid: &str) -> HttpRequest {
    let url = format!(
        "{MAIL_SEARCH_URL}?func=mbox:SearchByFidOrCondition&peFlag=1&sid={}&comefrom=54&cguid={cguid}",
        session.sid
    );
    let body = "<object>\
        <int name=\"fid\">1</int>\
        <string name=\"order\">receiveDate</string>\
        <int name=\"desc\">1</int>\
        <int name=\"total\">1</int>\
        <int name=\"limit\">10</int>\
        <int name=\"start\">0</int>\
        <int name=\"ignoreCase\">0</int>\
        <int name=\"isSearch\">1</int>\
        <int name=\"statType\">1</int>\
        <int name=\"isFullSearch\">0</int>\
        </object>";
    HttpRequest::post(url)
        .header("Cookie", session.cookie())
        .header("Content-Type", "application/xml;charset=UTF-8")
        .header("Accept", "*/*")
        .header("User-Agent", OKHTTP_UA)
        .header("X-Requested-With", "cn.cj.pe")
        .text_body(body)
}

/// `POST {HTML5_MAIL_HOST}/mw2/file/disk?func=disk:getDirectorys&sid=&comefrom=54&cguid=`
/// —— 云盘目录列表（拿 `directoryId`，转存那一发的落点）。
///
/// ⚠ 体里的 `qureySubDirectory` 是**上游自己的拼写**（少一个 `e`），照抄别"修正" ✓。
pub fn cloud_directories_request(
    session: &MailSession,
    cguid: &str,
    parent_id: &str,
) -> HttpRequest {
    let url = format!(
        "{HTML5_MAIL_HOST}/mw2/file/disk?func=disk:getDirectorys&sid={}&comefrom=54&cguid={cguid}",
        session.sid
    );
    let body = format!(
        "<object>\
         <string name=\"qureySubDirectory\">1</string>\
         <string name=\"parentId\">{}</string>\
         </object>",
        escape_xml(parent_id)
    );
    HttpRequest::post(url)
        .header("Cookie", session.cookie())
        .header("Content-Type", "application/xml;charset=UTF-8")
        .header("Accept", "*/*")
        .header("Origin", HTML5_MAIL_HOST)
        .header("Referer", "https://html5.mail.10086.cn/")
        .header("User-Agent", OKHTTP_UA)
        .header("X-Requested-With", "cn.cj.pe")
        .text_body(body)
}

/// `POST {HTML5_MAIL_HOST}/mw2/file/disk?func=disk:backupMail&sid=&comefrom=16861&cguid=`
/// —— **整封邮件转存到云盘**（App 实拍体，逐字）。
///
/// `backupType` 在实拍里是**空串**（`<string name="backupType" />`）⇒ 这里也发空串 ✓。
pub fn backup_mail_request(
    session: &MailSession,
    cguid: &str,
    mail_ids: &str,
    directory_id: &str,
) -> HttpRequest {
    let url = format!(
        "{HTML5_MAIL_HOST}/mw2/file/disk?func=disk:backupMail&sid={}&comefrom=16861&cguid={cguid}",
        session.sid
    );
    let body = format!(
        "<object>\
         <string name=\"mailIds\">{}</string>\
         <string name=\"backupType\" />\
         <string name=\"channel\">16861</string>\
         <string name=\"directoryId\">{}</string>\
         </object>",
        escape_xml(mail_ids),
        escape_xml(directory_id)
    );
    HttpRequest::post(url)
        .header("Cookie", session.cookie())
        .header("Content-Type", "application/xml;charset=UTF-8")
        .header("Accept", "*/*")
        .header("Origin", HTML5_MAIL_HOST)
        .header("Referer", "https://html5.mail.10086.cn/")
        .header("User-Agent", OKHTTP_UA)
        .header("X-Requested-With", "cn.cj.pe")
        .text_body(body)
}

/// 从**任一形状**的体里取某一枚键的**第一枚值**。
///
/// 这一族两个口的体形状**都没实拍**（列表那一支 App 只给了请求），所以这里刻意做成
/// "XML 属性式 `<string name="mid">123</string>` 与 JSON 式 `"mid":"123"` 两种都收" ——
/// ⚠ 这不是"宽松即正确"：取不到就交回 `None`，由调用方如实报"拿不到"，**不猜** ✓。
pub fn first_field(body: &str, key: &str) -> Option<String> {
    // 形状一：`name="mid">123<`
    for quote in ['"', '\''] {
        let needle = format!("name={quote}{key}{quote}");
        if let Some(pos) = body.find(&needle) {
            let rest = &body[pos + needle.len()..];
            if let Some(gt) = rest.find('>') {
                let tail = &rest[gt + 1..];
                let value: String = tail.chars().take_while(|c| *c != '<').collect();
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    // 形状二：`"mid":"123"` / `'mid':'123'`（`getComposeId` 那一族的体就是单引号壳 ✓）
    for sep in [r#":""#, r#":'"#] {
        let needle = format!("\"{key}\"{sep}");
        if let Some(pos) = body.find(&needle) {
            let rest = &body[pos + needle.len() - 1..];
            let quote = rest.chars().next()?;
            let tail = &rest[1..];
            let value: String = tail.chars().take_while(|c| *c != quote).collect();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// 体的**掩码预览** —— 只在"这一口零实拍、要靠真机读数把形状说出来"时用。
///
/// 规则：**数字一律换成 `#`**（id / 计数那一类全在这里），按 `limit` 截断。
/// ⚠ 它**不是**脱敏器：字母串（邮件标题那种）原样留着 ⇒ 只用在"形状未知"的一次性读数上，
/// 别把它当成能进长期日志的东西 ✓。
pub fn masked_preview(body: &str, limit: usize) -> String {
    let mut out = String::with_capacity(limit + 8);
    for c in body.chars().take(limit) {
        if c.is_ascii_digit() {
            out.push('#');
        } else {
            out.push(c);
        }
    }
    if body.chars().count() > limit {
        out.push('…');
    }
    out
}

/// 体的**形状提示**：把出现过的键名去重列出来（**只列名、不列值**）。
///
/// 用途是"这一口零实拍时，先让真机读数把形状说出来"—— 拿不到 `mid` / `directoryId`
/// 时把它记进明细，下一次改动就有依据 ✓（别把整段体记进日志：那一族体里有邮件标题）。
pub fn shape_hint(body: &str, limit: usize) -> String {
    let mut keys: Vec<String> = Vec::new();
    // 形状一：XML 属性式 `name="mid"` / `name='mid'`
    for (idx, _) in body.match_indices("name=") {
        let rest = &body[idx + 5..];
        let mut chars = rest.chars();
        let Some(quote) = chars.next() else { continue };
        if quote != '"' && quote != '\'' {
            continue;
        }
        let value: String = chars.take_while(|c| *c != quote).collect();
        if !value.is_empty() && !keys.contains(&value) {
            keys.push(value);
        }
        if keys.len() >= limit {
            break;
        }
    }
    // 形状二：JSON 式 —— **单双引号都收**（这一族的体有单引号壳那一档 ✓）。
    if keys.is_empty() {
        for quote in ['"', '\''] {
            let needle = format!("{quote}:");
            for (idx, _) in body.match_indices(&needle) {
                let head = &body[..idx];
                if let Some(start) = head.rfind(quote) {
                    if let Some(prev) = head[..start].rfind(quote) {
                        let key = &head[prev + 1..start];
                        if !key.is_empty() && !keys.contains(&key.to_string()) {
                            keys.push(key.to_string());
                        }
                    }
                }
                if keys.len() >= limit {
                    break;
                }
            }
            if !keys.is_empty() {
                break;
            }
        }
    }
    format!("{}（体长 {}）", keys.join(" / "), body.len())
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

#[cfg(test)]
mod mail_report_tests {
    use super::*;

    /// 体的**逐字**形状（实拍那一趟三发 ✓）—— 注意是 **JSON** ✓，
    /// 而请求头写的是 `application/xml` ✗（实拍如此，别"顺手改对" ✗）。
    #[test]
    fn mail_report_body_matches_the_captured_shape() {
        assert_eq!(
            mail_report_body("1003", "2", "d5060000000000000000000a", ""),
            r#"{"actionType":"1003","clientType":"2","imapId":"d5060000000000000000000a","clientVersion":"h5","channel":""}"#
        );
        assert_eq!(
            mail_report_body("1017", "1", "d5060000000000000000000a", "10242400"),
            r#"{"actionType":"1017","clientType":"1","imapId":"d5060000000000000000000a","clientVersion":"h5","channel":"10242400"}"#
        );
    }

    /// URL 与头：`comefrom=**166**` ✓、`Content-Type` 是 xml ✗、`Referer` 指向 `readmail.html` ✓。
    #[test]
    fn mail_report_request_matches_the_captured_shape() {
        let session = MailSession {
            sid: "00c5TEST".into(),
            rmkey: "deadbeef".into(),
        };
        let req = mail_report_to_mq_request(&session, "12345678", "0.5", "{}");
        assert!(req.url.contains("/mw2/file/disk?func=disk:mailReportToMQ"));
        assert!(req.url.contains("comefrom=166"), "URL：{}", req.url);
        assert!(req.url.contains("cguid=12345678"), "URL：{}", req.url);
        let pick = |name: &str| -> Vec<String> {
            req.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
                .collect()
        };
        assert_eq!(pick("content-type"), vec!["application/xml".to_string()]);
        assert!(
            pick("referer")
                .iter()
                .any(|v| v.ends_with("/html/readmail.html")),
            "Referer：{:?}",
            pick("referer")
        );
    }
}
