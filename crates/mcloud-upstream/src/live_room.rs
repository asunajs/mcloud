//! 直播侧（`/cmlive` + `videoim`）的上游接口层 —— 房间发现、`/cmlive` 信封、
//! `member/apply` 签名。
//!
//! 对应 Go 参考的 `internal/upstream/live_room.go`（683 行）。
//!
//! # ⚠⚠ 这一族用的是**第三套响应约定**
//!
//! | 侧 | 基址 | 响应约定 |
//! |---|---|---|
//! | 小红花（`/ycloud`） | `m.mcloud.139.com/ycloud` | `{code: 0, msg, result}` 数字码 + `result` |
//! | **直播（`/cmlive`）** | `v.cmcc-cs.cn/cmlive` | `{rtnCode: "0", rtnMsg, object}` **字符串**码 + `object` |
//! | IM（`videoim`） | `videoim.cmcc-cs.cn:20103` | `{code: 0, msg, data, timestamp}` 数字码 + `data` |
//!
//! 三套都在同一条链路上，**不能共用解析**。最要命的是 `/cmlive` 那一套：
//! [`crate::app_signin::SignInEnvelope`] 取的是 `result` 键，喂给它 `/cmlive` 的响应
//! 会**静默取到 `None` 而不报错** —— 表现为「登录成功却取不到 token」
//! （实拍记录 `../mcloud/docs/capture/app-live-redflower-samples.txt` §5.3 末尾，坑表 #108）。
//! 所以这里单独给一个 [`CmliveEnvelope`]。
//!
//! # `rtnCode` 刻意**只认字符串**
//!
//! [`crate::app_signin::SignInEnvelope`] 那种写法是「数字与数字字符串都收」。
//! 这里**不**这么干：`rtnCode` 是字符串这件事本身就是这一族的**身份特征** ——
//! 把类型差异抹平之后，「上游把这一族改成了数字码」这个信号就**再也不会浮出来**，
//! 而那正是最该立刻看见的一类协议变化。
//!
//! ⇒ 数字 `0` 会**解析失败**，而且失败信息里明说「这一族用字符串码」。
//! 这不是苛刻，是刻意留出来的报警灯（见 [`parse_cmlive`] 与测试
//! `rtn_code_refuses_numeric_code`）。
//!
//! # 本模块的证据等级
//!
//! | 部分 | 证据 |
//! |---|---|
//! | `/cmlive` 信封、`roomBeat`、`tabContentList`、三档 `status` | **实拍**（§5.2 / §5.3，2026-09-22） |
//! | `KoulingRooms` 的昵称精确相等判据 | **实拍**（§5.3 那张表：`status===1` 里有「福建移动」） |
//! | 两个页签都查（127 + 129） | **实拍**（147606 两个都在；147608 只在 129） |
//! | `TimestampSuffix10` / `SystemMD5` / `TokenMD5` 的**算式** | **实拍**（§8.3，两条记录逐字命中） |
//! | 那两个签名头**服务端不校验** | **实拍**（§8.6：打成全 0 也回 `code: 0`，坑表 #136） |
//! | [`LIVE_APPLY_VERSION`] 的取值 | **取自原项目**（`v23.0.2`）—— 抓包里那个头被脱敏成 `<REDACTED>` |
//! | `member/apply` 的**响应**字段（`appKey` 等） | **实拍**（`../mcloud/test/fixtures/http/直播716.json`） |
//!
//! # 本模块只构造与解析，一个请求都不发
//!
//! 零 IO、零网络。真实的 WS 连接在 `live_ws_client`（另一个模块），
//! 帧状态机在 [`crate::live_ws`]。

use mcloud_core::{Error, HttpRequest, Result};
use serde_json::Value;

use crate::live_consts::{
    live_room_referer, KOULING_ANCHOR, LIVE_APPLY_VERSION, LIVE_CHANNEL_ID, LIVE_ORIGIN,
    LIVE_ROOM_STATUS_ENDED, LIVE_ROOM_STATUS_LIVE, LIVE_ROOM_STATUS_NOT_STARTED,
    LIVE_TAB_ID_BENEFITS, LIVE_TAB_ID_REPLAY, LIVE_USER_AGENT,
};

// ══════════════════════════ `/cmlive` 的响应信封 ══════════════════════════

/// `/cmlive/api/...` 的统一响应：`{rtnCode, rtnMsg, object}`。
///
/// # ⚠ 它与 [`crate::app_signin::SignInEnvelope`] **不能互换**
///
/// 两个信封的**键名与值的类型都不同**：
///
/// | | 业务码键 | 码的类型 | 数据键 |
/// |---|---|---|---|
/// | 签到族 | `code` | **数字** | `result` |
/// | **直播族（本结构）** | `rtnCode` | **字符串** | `object` |
///
/// 「拿签到那个去解直播响应」的表现是 `result` 取到 `None`、码取到 `-1`
/// —— 于是错误信息说「响应没有 result」，而真正的原因是**用错了信封**。
#[derive(Debug, Clone, PartialEq)]
pub struct CmliveEnvelope {
    /// `rtnCode` 原文（**字符串**）。
    rtn_code: String,
    /// `rtnMsg` 原文。`None` = 响应里没有这个键，或它是 `null`。
    ///
    /// ⚠ 实拍里 `rtnMsg` **就是 `null`**（`"rtnMsg":null`，§5.2 与 §5.3 都如此）
    /// ⇒ 成功响应**不带**消息是常态，别把「没有 rtnMsg」当成异常。
    rtn_msg: Option<String>,
    /// `object` 原文。`None` = 响应里没有这个键。
    object: Option<Value>,
}

impl CmliveEnvelope {
    /// 解析 `/cmlive` 的响应体。
    ///
    /// # ⚠ `rtnCode` **必须是 JSON 字符串**
    ///
    /// 数字 `0` / `null` / 缺失 **一律** [`Error::Parse`] 失败，**不做**类型转换。
    /// 理由见模块头：把类型差异抹平等于把「上游换了一套码」这件事藏起来。
    ///
    /// 失败的措辞里保留「这一族用字符串码」—— 因为拿到这条错误的人
    /// 最可能的第一反应是「是不是解析写严了」，而那正好是需要被劝退的方向。
    pub fn parse(body: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(body)
            .map_err(|e| Error::Parse(format!("/cmlive 响应不是 JSON：{e}")))?;

        let raw = value
            .as_object()
            .ok_or_else(|| Error::Parse("/cmlive 响应不是对象".to_string()))?;

        let rtn_code = match raw.get("rtnCode") {
            Some(Value::String(s)) => s.clone(),
            // ⚠ 刻意不用 `Value::Number` 兜底：见模块头。
            Some(other) => {
                return Err(Error::Parse(format!(
                    "/cmlive 响应的 rtnCode 不是字符串（拿到 {}）—— \
                     这一族用字符串码（如 \"0\"），见 live_room.rs 文件头。\
                     上游若真改成了数字码，这里就该报错而不是被抹平。",
                    type_name(other)
                )));
            }
            None => {
                return Err(Error::Parse(
                    "/cmlive 响应缺少 rtnCode（这一族用 rtnCode 而不是 code）".to_string(),
                ));
            }
        };

        // `rtnMsg` 缺失与 `null` 都是常态（实拍就是 `null`）⇒ 都折成 `None`。
        let rtn_msg = raw
            .get("rtnMsg")
            .and_then(Value::as_str)
            .map(str::to_string);

        Ok(Self {
            rtn_code,
            rtn_msg,
            object: raw.get("object").cloned(),
        })
    }

    /// `rtnCode` 原文（字符串）。
    pub fn code(&self) -> &str {
        &self.rtn_code
    }

    /// `rtnMsg`（可能没有）。
    pub fn message(&self) -> Option<&str> {
        self.rtn_msg.as_deref()
    }

    /// `object` 原文。`None` = 响应里没有这个键（**也可能是显式的 `null`** ——
    /// 那时这里是 `Some(Value::Null)`）。
    pub fn object(&self) -> Option<&Value> {
        self.object.as_ref()
    }

    /// 业务是否成功。
    ///
    /// # ⚠ 判据是与 `"0"` **字符串相等**
    ///
    /// 不是「能不能转成数字」：`"-9999"`（`getCmPassToken` 实测值）与 `"0"`
    /// 在字符串意义上就不同。更关键的是将来若出现**非数字码**（如 `"AUTH_FAIL"`），
    /// 「能不能转数字」这种判据会把它静默归到某个分支里去。
    pub fn ok(&self) -> bool {
        self.rtn_code == "0"
    }

    /// 业务成功时给出 `object`；失败时给出错误。
    ///
    /// # `rtnMsg` 只进错误信息，不进对外文案之外的地方
    ///
    /// 上游的原文里常回显凭据（与 `mcloud-upstream` 其余部分同一口径）。
    pub fn ok_object(&self, what: &str) -> Result<&Value> {
        if !self.ok() {
            return Err(self.error(what));
        }
        self.object
            .as_ref()
            .ok_or_else(|| Error::Parse(format!("{what} 响应没有 object")))
    }

    /// 业务失败时返回错误（不要求有 `object`）。
    pub fn ensure_ok(&self, what: &str) -> Result<()> {
        if self.ok() {
            return Ok(());
        }
        Err(self.error(what))
    }

    /// 把非 `"0"` 的业务码翻成一条错误。
    fn error(&self, what: &str) -> Error {
        match self.message() {
            Some(msg) if !msg.is_empty() => Error::Upstream {
                code: self.rtn_code.clone(),
                message: format!("{what}：{msg}"),
            },
            _ => Error::Upstream {
                code: self.rtn_code.clone(),
                message: what.to_string(),
            },
        }
    }
}

/// 给「类型不对」那句错误用的 JSON 类型名。
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "布尔",
        Value::Number(_) => "数字",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

// ══════════════════════════ 公共头 ══════════════════════════

/// 给 `/cmlive` 的请求补上头（逐个对应实拍的头集合）。
///
/// # ⚠ 不含 `Cookie` / `Sec-Fetch-*` / `Connection`
///
/// 那三个是**浏览器/WebView 自己产生的**：纯协议实现里既没有 cookie jar
/// 也不受 fetch 元数据约束。把它们的值抄进来只会让请求看起来像浏览器，
/// 而那既不是鉴权要求，也不是上游判据。
///
/// # ⚠ `X-Tingyun` 也不补
///
/// 实拍里它带 `c=…;x=<32hex>;u=base64#…`。那是听云 APM 的**采样串**，
/// 由浏览器端 SDK 现场生成、含本机标识 —— 我们造不出真值，
/// 而伪造一个既没有实测依据、又会往日志里多写一处设备标识。
///
/// 所以这里只补「测试证明有用」的那几片：内容协商、来源、UA。
fn cmlive_headers(req: HttpRequest, jwt: Option<&str>) -> HttpRequest {
    let req = req
        .header("Accept", "application/json, text/plain, */*")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept-Language", "zh-CN,zh-Hans;q=0.9")
        .header("Origin", LIVE_ORIGIN)
        .header("User-Agent", LIVE_USER_AGENT);
    match jwt {
        // 直播侧的凭据是**独立一套 JWT**（payload 含 `phone` / `userId`，≈30 天），
        // 与签到/兑换那串 `pc:` / `mobile:` 凭据不是同一个东西 —— 别互搬。
        // 鉴权走**标准 `Authorization` 头**，不是签到族的 `jwttoken` 头。
        Some(token) => req.header("Authorization", token),
        None => req,
    }
}

// ══════════════════════════ 登录链 ══════════════════════════

/// `cmpassLoginByCmToken` —— 用 SSO token 换直播侧的 JWT。
///
/// 成功时 `object` 是**一串 JWT 本身**（不是包着它的对象），实拍如此（§5.3）。
pub fn cmpass_login_request(cmpass_token: &str) -> Result<HttpRequest> {
    let body = serde_json::json!({ "cmpassToken": cmpass_token });
    Ok(cmlive_headers(
        HttpRequest::post(format!(
            "{}/api/user/auth/cmpassLoginByCmToken",
            crate::live_consts::BASE_CMLIVE
        ))
        .json_body(&body),
        None,
    ))
}

/// 取 `cmpassLoginByCmToken` 响应里的 JWT。
pub fn parse_cmpass_login(env: &CmliveEnvelope) -> Result<String> {
    let object = env.ok_object("直播侧登录")?;
    let jwt = object.as_str().unwrap_or_default();
    if jwt.is_empty() {
        return Err(Error::Parse(
            "cmpassLoginByCmToken 的 object 不是非空字符串（应当就是那串 JWT 本身）".to_string(),
        ));
    }
    Ok(jwt.to_string())
}

/// `getCurrentUserInfoNew` —— 只是**建立服务端 session**。
///
/// 原项目连响应都不看，但少了它后面的 `userToken` 会拿不到 token。
pub fn current_user_info_request(jwt: &str) -> HttpRequest {
    cmlive_headers(
        HttpRequest::get(format!(
            "{}/api/user/auth/getCurrentUserInfoNew",
            crate::live_consts::BASE_CMLIVE
        )),
        Some(jwt),
    )
}

/// `userToken` 的解析结果 —— 开 WS 要用的那一串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveUserToken {
    /// WS 的 `auth`（原实现拼成 `"$t$" + access_token`）。
    ///
    /// 实测形态：`A03` 前缀、约 66 字符（原项目注释如此；本仓库的实拍是脱敏值）。
    pub access_token: String,
    /// 有效期秒数。实拍 `5184000`（= 60 天）。
    pub expires_in: i64,
}

/// `user/customer/userToken` 请求（body 是**空对象** `{}`，不是没有 body）。
pub fn user_token_request(jwt: &str) -> Result<HttpRequest> {
    let body = serde_json::json!({});
    Ok(cmlive_headers(
        HttpRequest::post(format!(
            "{}/api/user/customer/userToken",
            crate::live_consts::BASE_CMLIVE
        ))
        .json_body(&body),
        Some(jwt),
    ))
}

/// 解析 `userToken`。
pub fn parse_user_token(env: &CmliveEnvelope) -> Result<LiveUserToken> {
    let object = env.ok_object("取 userToken")?;
    let access_token = object
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Parse("userToken 响应缺少 object.access_token".to_string()))?;
    Ok(LiveUserToken {
        access_token: access_token.to_string(),
        expires_in: int_at(object, "expires_in").unwrap_or(0),
    })
}

// ══════════════════════════ 频道与房间 ══════════════════════════

/// 一个页签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTab {
    /// 页签 id（用来打 `tabContentList`）。实拍：`127` / `129`。
    pub id: i64,
    /// 页签名（「盘盘福利」/「精彩回放」）。
    pub name: String,
}

/// `tabList` 请求。
pub fn tab_list_request(jwt: &str) -> HttpRequest {
    cmlive_headers(
        HttpRequest::get(format!(
            "{}/api/channel/channel/tabList?channelId={LIVE_CHANNEL_ID}",
            crate::live_consts::BASE_CMLIVE
        )),
        Some(jwt),
    )
}

/// 解析 `tabList`。
///
/// ⚠ 页签 id 来自**响应**而不是写死的 —— 写死两个 id 的话，
/// 上游换页签编号时我们会去查一个不存在的页签，而它回的是「空列表」
/// （不是错误），于是表现成「今天没有直播」。
pub fn parse_tab_list(env: &CmliveEnvelope) -> Result<Vec<LiveTab>> {
    let object = env.ok_object("查页签列表")?;
    let raw = object
        .get("dataList")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("tabList 的 object 缺少 dataList 数组".to_string()))?;

    let mut tabs = Vec::with_capacity(raw.len());
    for item in raw {
        let Some(id) = int_at(item, "id") else {
            continue;
        };
        tabs.push(LiveTab {
            id,
            name: str_at(item, "name"),
        });
    }
    Ok(tabs)
}

/// 要查的页签 id —— **两个都查**（127 福利 + 129 回放）。
///
/// # ⚠ 原项目只查 127，这是一处**刻意的偏离**
///
/// 依据是 2026-09-22 实拍（§5.3）：同一间房 `147606` **同时**出现在 127 与 129 里，
/// 而下一场 `147608`（国庆场）**只在 129 里**。
///
/// ⇒ 只查 127 会在「房间还没进福利页签」时判成**没开播**，
/// 而那正是「漏掉整场口令」这一侧的错误 —— 代价远大于多打一个 GET。
pub fn tab_ids_to_query(tabs: &[LiveTab]) -> Vec<i64> {
    // 先按响应里给的顺序，再兜底补上两个已知 id（响应少了哪个都不至于漏查）。
    let mut out: Vec<i64> = Vec::new();
    for tab in tabs {
        if !out.contains(&tab.id) {
            out.push(tab.id);
        }
    }
    for id in [LIVE_TAB_ID_BENEFITS, LIVE_TAB_ID_REPLAY] {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// `tabContentList` 请求。
///
/// 查询参数的**顺序与实拍逐字一致**（`tabId` → `contentType` → `pageNo` → `channelId`）。
pub fn tab_content_list_request(tab_id: i64, jwt: &str) -> HttpRequest {
    cmlive_headers(
        HttpRequest::get(format!(
            "{}/api/channel/channel/tabContentList?tabId={tab_id}&contentType=0&pageNo=1&channelId={LIVE_CHANNEL_ID}",
            crate::live_consts::BASE_CMLIVE
        )),
        Some(jwt),
    )
}

/// `tabContentList` 里的一间房。
///
/// 只保留**判据用得到**的字段。上游给的还有 `coverUrl` / `pullUrl` / `uv` 等十几个，
/// 全搬进来只会让「哪些字段在参与判断」变得看不出来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRoom {
    pub id: i64,
    pub title: String,
    /// 三档之一：[`LIVE_ROOM_STATUS_NOT_STARTED`] / [`LIVE_ROOM_STATUS_LIVE`] /
    /// [`LIVE_ROOM_STATUS_ENDED`]。
    pub status: i64,
    /// **房间的归属频道** —— 不是「我拉的是哪个频道」。
    ///
    /// ⚠ 实拍里查询参数写 `channelId=134`，返回的房间 `channelId` 有
    /// `29 / 51 / 243 / 134` 四种 ⇒ 拿它当过滤条件会一间都筛不出来。
    pub channel_id: i64,
    /// 预告开播时间（`2026-09-23 14:30:00` 形态的**本地**字符串，不是时间戳）。
    pub expect_start_time: String,
    pub anchor_id: i64,
    pub author_id: i64,
    /// 主播昵称 —— 门的**真正判据**（见 [`KOULING_ANCHOR`]）。
    pub anchor_nickname: String,
}

impl LiveRoom {
    /// 是否**正在直播**中。
    pub fn is_live(&self) -> bool {
        self.status == LIVE_ROOM_STATUS_LIVE
    }

    /// 是否**已结束**（回放）。
    pub fn is_ended(&self) -> bool {
        self.status == LIVE_ROOM_STATUS_ENDED
    }

    /// 是否**未开播**（预告）。
    pub fn is_not_started(&self) -> bool {
        self.status == LIVE_ROOM_STATUS_NOT_STARTED
    }

    /// 主播昵称是否**精确等于** [`KOULING_ANCHOR`]。
    ///
    /// # ⚠ 是**相等**，不是 `contains`
    ///
    /// 实拍那张表里还有「福建移动」「移动心选」「移动臻选」三个昵称 ——
    /// 用包含关系去判，它们一个都拦不住（都含「移动」）。
    pub fn is_kouling_anchor(&self) -> bool {
        self.anchor_nickname == KOULING_ANCHOR
    }
}

/// 解析 `tabContentList`。
///
/// # ⚠ 房间嵌在 `dataList[].room` 里
///
/// 别按 `dataList[].id` 找房间 —— 那一层是 `type` / `activity` / `video`
/// 的包装对象，`dataList[].id` 不是房间 id。
///
/// `dataList: null` 是**合法**的「这个页签没有房间」，不是解析错误 ⇒ 回空表。
pub fn parse_tab_content_list(env: &CmliveEnvelope) -> Result<Vec<LiveRoom>> {
    let object = env.ok_object("查房间列表")?;
    let Some(raw_list) = object.get("dataList").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut rooms = Vec::with_capacity(raw_list.len());
    for entry in raw_list {
        let Some(room) = entry.get("room") else {
            continue;
        };
        if !room.is_object() {
            continue;
        }
        rooms.push(LiveRoom {
            id: int_at(room, "id").unwrap_or(0),
            title: str_at(room, "title"),
            status: int_at(room, "status").unwrap_or(-1),
            channel_id: int_at(room, "channelId").unwrap_or(0),
            expect_start_time: str_at(room, "expectStartTime"),
            anchor_id: int_at(room, "anchorId").unwrap_or(0),
            author_id: int_at(room, "authorId").unwrap_or(0),
            anchor_nickname: room
                .get("anchor")
                .map(|a| str_at(a, "nickname"))
                .unwrap_or_default(),
        });
    }
    Ok(rooms)
}

/// 从房间列表里挑出**会发口令**的那些（门的判据）。
///
/// # 两个条件**都要**
///
/// 实拍 §5.3 那张表钉死：
///
/// - `status == 1`（直播中）—— **单看它不够**：同一页（`tabId=127`）里
///   就有一间 `status=1` 的「福建移动」（147490）；
/// - `anchor_nickname == "中国移动云盘"` —— **这个才真正干活**。
///
/// ⇒ 只有名字对了、且现在正在播，才值得去听弹幕。预告（`0`）与回放（`6`）
/// 都**不算** —— 那时没有聊天室，去了必然空手而归。
pub fn kouling_rooms(rooms: &[LiveRoom]) -> Vec<LiveRoom> {
    rooms
        .iter()
        .filter(|r| r.is_live() && r.is_kouling_anchor())
        .cloned()
        .collect()
}

/// 从房间列表里挑出「那个主播」名下的那些，**不限状态**。
///
/// # 它与 [`kouling_rooms`] 只差状态那一档，而用途完全不同
///
/// - **门**的判据要「现在值不值得去听」⇒ 只算 `status == 1`（[`kouling_rooms`]）；
/// - **界面**上那一行要「下一场几点、上一场是否已结束」⇒ 预告（`0`）与
///   回放（`6`）都得留着，因为 `expect_start_time` 只在预告那一档有用。
///
/// ⚠ 别拿它当门用：把预告与回放算进「在播」会让界面在没开播时喊去打弹幕。
pub fn anchor_rooms(rooms: &[LiveRoom]) -> Vec<LiveRoom> {
    rooms
        .iter()
        .filter(|r| r.is_kouling_anchor())
        .cloned()
        .collect()
}

/// 按房间 id 去重合并多个页签的结果（**先出现的赢**）。
///
/// 两个页签会返回**同一间房**（实拍：147606 同时在 127 与 129 里）——
/// 不去重就会对同一间房做两次 `roomBeat`、开两条 WS。
pub fn merge_rooms(groups: &[Vec<LiveRoom>]) -> Vec<LiveRoom> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for group in groups {
        for room in group {
            if seen.insert(room.id) {
                out.push(room.clone());
            }
        }
    }
    out
}

/// `roomBeat` 请求。
///
/// # 三个字段里有两个是**字符串**（实拍）
///
/// `{"roomId":"146316","channelId":"134","sourceApp":0}` —— 原项目传的是**数字**。
/// 照实拍走：`roomId` 本来就是 31 位的大整数，过一遍 JS 的 Number 有精度风险，
/// 客户端把它写成字符串是**对的**。
///
/// # 它同时是「这间房有没有聊天室」的唯一判据
///
/// 未开播时 `object.chatRoomId` 是 `null` ⇒ `member/apply` 与 WS **根本不会发**
/// —— 这是**结构性事实**，不是「抓漏了」（坑表 #107，实拍 §5.2）。
pub fn room_beat_request(room_id: &str, jwt: &str) -> Result<HttpRequest> {
    let body = serde_json::json!({
        "roomId": room_id,
        "channelId": LIVE_CHANNEL_ID,
        "sourceApp": 0,
    });
    Ok(cmlive_headers(
        HttpRequest::post(format!(
            "{}/api/room/front/roomBeat",
            crate::live_consts::BASE_CMLIVE
        ))
        .json_body(&body)
        .header("Referer", live_room_referer(room_id)),
        Some(jwt),
    ))
}

/// `roomBeat` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomBeat {
    pub status: i64,
    pub title: String,
    pub channel_id: i64,
    pub expect_start_time: String,
    pub anchor_nickname: String,
    /// 聊天室 id（31 位数字串）。
    ///
    /// `None` = 上游给的是 `null`，即**没有聊天室** ⇒ WS 与 `member/apply`
    /// 都**不该发**。用 `Option` 而不是空串，是为了让「没有」与「有但是空」
    /// 在**类型上**就分得开 —— 它们后面的动作完全相反。
    pub chat_room_id: Option<String>,
}

impl RoomBeat {
    /// 有没有聊天室 —— WS 与 `member/apply` 的前置判据。
    pub fn has_chat_room(&self) -> bool {
        self.chat_room_id.is_some()
    }
}

/// 解析 `roomBeat`。
pub fn parse_room_beat(env: &CmliveEnvelope) -> Result<RoomBeat> {
    let object = env.ok_object("进直播间")?;
    if !object.is_object() {
        return Err(Error::Parse("roomBeat 的 object 不是对象".to_string()));
    }
    Ok(RoomBeat {
        status: int_at(object, "status").unwrap_or(-1),
        title: str_at(object, "title"),
        channel_id: int_at(object, "channelId").unwrap_or(0),
        expect_start_time: str_at(object, "expectStartTime"),
        anchor_nickname: object
            .get("anchor")
            .map(|a| str_at(a, "nickname"))
            .unwrap_or_default(),
        // ⚠ 空串也当作「没有」：空串进不了 WS（会拼出一个空的 `cmlive_…`）。
        chat_room_id: object
            .get("chatRoomId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

// ══════════════════════════ 进房间：member/apply ══════════════════════════

/// 取时间戳字符串的**末 10 位**（两个签名头都用它）。
///
/// # ⚠ 长度不足 10 时**原样返回**，不是错误
///
/// 对齐 JS 的 `String.prototype.slice(-10)`：它对短串给的是**整串**。
/// 别在这里加长度校验 —— 那会把一次**协议差异**变成一个「参数错误」，
/// 而真正该看见的是**签名对不上**。
pub fn timestamp_suffix_10(timestamp: &str) -> &str {
    if timestamp.len() <= 10 {
        return timestamp;
    }
    &timestamp[timestamp.len() - 10..]
}

/// `member/apply` 的 `systemMd5` 头。
///
/// ```text
/// md5( deviceInfo 的 48 字节 ‖ utf8(timestamp 末 10 位) )
/// ```
///
/// ⚠ 第一段是**解码后的字节**（实拍 48 字节），不是那 96 个 hex 字符。
///
/// # ⚠⚠ 服务端**不校验**这个头（2026-09-23 实证）
///
/// `../mcloud/docs/capture/app-live-ws-samples.txt` §8.6：在**正在播**的房间里发
/// `member/apply`，把 `systemMd5` 打成 `0000…0`（32 个 0）**照样回
/// `{"code":0,"msg":"接口调用成功"}`**；`tokenMd5` 一起打成全 0 也过。
///
/// ⇒ 这条算式**不是闸门**，「传错口径」**不会**表现成「签名不对」
/// （早先那句注释就是错的，见坑表 **#136**）。
///
/// ⇒ 所以这里按「解码后的字节」算**不是为了过闸**，而是为了与实拍客户端的
/// 算式对照；实拍客户端的输入是**头值本身**（它的头值是 `other/unknown`，非 hex）。
///
/// # ⚠ hex 不合法是**硬失败**
///
/// 与「服务端不校验」并不矛盾：那个说的是**签名的值**不被校验，
/// 而这里判的是**我们自己**的输入是不是它声称的东西。
/// 拿一个非 hex 串去解码会得到 0 字节，于是摘要**静默**变成
/// `md5(时间戳末 10 位)` —— 那种错误只会在事后对比签名时才看得出来。
pub fn system_md5(device_info_hex: &str, timestamp: &str) -> Result<String> {
    let device_info = hex::decode(device_info_hex)
        .map_err(|e| Error::Parse(format!("deviceInfo 不是合法 hex：{e}")))?;
    let mut payload = device_info;
    payload.extend_from_slice(timestamp_suffix_10(timestamp).as_bytes());
    Ok(md5_hex_bytes(&payload))
}

/// 对**任意字节**求 MD5 小写 hex。
///
/// # ⚠ 为什么不用 `mcloud_crypto::md5_hex`
///
/// 那个只收 `&str`。而这里要哈希的两样东西都是**字节**：
///
/// - 设备串解码后的 48 字节里有 `0x83` / `0xda` 这种**非 UTF-8** 序列 ——
///   先转成 `String` 会把这些字节**改写**成 `U+FFFD`，摘要就不对了；
/// - `tokenMd5` 的输入里 token 与时间戳都是 ASCII，那条本可以用字符串版，
///   但两条签名走同一个原语更容易看出「它们是同一族」。
fn md5_hex_bytes(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// `member/apply` 的 `tokenMd5` 头。
///
/// ```text
/// md5( "Bearer " + token[0:10] + ts 末 10 位 + token[10:] )
/// ```
///
/// # ⚠ 它不是「对 token 求哈希」
///
/// 时间戳被**插在 token 的第 10 个字符之后**。这个形状是 2026-09-23
/// **实拍验证**过的（两条记录逐字命中，§8.3）—— 它描述的是**客户端怎么算**。
///
/// # ⚠⚠ 但服务端**不校验**它（§8.6：打成全 0 也回 `code: 0`，坑表 #136）
///
/// # ⚠ 短于 10 字符的 token
///
/// JS 的 `slice(0, 10)` 对短串返回**整串**、`slice(10)` 返回**空串** ——
/// 照抄那个语义（黄金向量 `short-token` 就是这条：`A03SHORT`，8 字符）。
/// 若照 Go 那样直接切片会 panic，若「补零到 10 位」则会得到一个
/// 与原客户端**不同**的串。
pub fn token_md5(access_token: &str, timestamp: &str) -> String {
    let ts10 = timestamp_suffix_10(timestamp);
    // ⚠ 按**字节**切：`access_token` 是 ASCII（`A03` 前缀），
    // 与 JS 按 UTF-16 码元切等价（见 `.chars()` 与字节的取舍：这里必须按字节，
    // 因为黄金向量的期望值是按 JS 的码元切算出来的，而该串全 ASCII）。
    let (head, tail) = if access_token.len() > 10 {
        (&access_token[..10], &access_token[10..])
    } else {
        (access_token, "")
    };
    md5_hex_bytes(format!("Bearer {head}{ts10}{tail}").as_bytes())
}

/// `member/apply` 请求 —— 进聊天室。
///
/// # 头集合以**实拍**为准
///
/// ```text
/// Content-Type: application/json      Accept: */*
/// Version: v23.0.2                    timestamp: <13 位毫秒>
/// Authorization: Bearer <token>       Origin: https://v.cmcc-cs.cn
/// deviceInfo: <字符串头值>
/// systemMd5 / tokenMd5                ⚠ 服务端**不校验**这两个头
/// ```
///
/// ⚠ `deviceInfo` 这个头装的是**字符串**（hex 字面量，96 个字符），
/// 不是 48 字节二进制。
///
/// # ⚠ **不**加 `appkey` 头
///
/// 原项目加了 `appkey: cmos/ngvlcs`，但实拍那 19 个头里**没有它** ——
/// 那个值是从 apply 的**响应**（`data.appKey`）里抄来的，属于把响应字段
/// 误当成请求要求。多发一个头通常无害，但「照抄原项目」在这里会掩盖
/// 一个事实：我们**并不知道**服务端要不要它。
///
/// # ⚠ `message` 字段**要发**
///
/// 原项目漏了它（只发 `{roomId, userName, invoker}`），实拍是
/// `{"invoker":…,"message":"","roomId":…,"userName":…}` —— 补上。
pub fn apply_room_request(
    chat_room_id: &str,
    user_name: &str,
    invoker: &str,
    access_token: &str,
    device_info_hex: &str,
    timestamp: &str,
) -> Result<HttpRequest> {
    let system_md5 = system_md5(device_info_hex, timestamp)?;
    let body = serde_json::json!({
        "invoker": invoker,
        "message": "",
        "roomId": chat_room_id,
        "userName": user_name,
    });
    Ok(HttpRequest::post(format!(
        "{}/v3/room/member/apply",
        crate::live_consts::BASE_VIDEO_IM
    ))
    .json_body(&body)
    .header("Accept", "*/*")
    .header("Version", LIVE_APPLY_VERSION)
    .header("timestamp", timestamp)
    .header("Authorization", format!("Bearer {access_token}"))
    .header("Origin", LIVE_ORIGIN)
    .header("User-Agent", LIVE_USER_AGENT)
    .header("deviceInfo", device_info_hex)
    .header("systemMd5", system_md5)
    .header("tokenMd5", token_md5(access_token, timestamp)))
}

/// `member/apply` 的解析结果。
///
/// ⚠ 这一条用的是**普通信封**（`{code: 0, msg, data, timestamp}`）——
/// 数字码 + `data`，与 `/cmlive` 的 `rtnCode` + `object` **不是一套**。
/// 别「顺手统一」成 [`CmliveEnvelope`]：那会让 `data` 永远取不到。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyRoomResult {
    pub app_id: String,
    /// **响应**里给的 `appKey`。
    ///
    /// ⚠ 它**不是**请求头（原项目把响应字段误当成了请求要求，见
    /// [`apply_room_request`] 的说明）。
    pub app_key: String,
    /// 回显的聊天室 id（与请求里的 `roomId` 同值）。
    pub room_id: String,
    /// 服务端的毫秒时间戳（13 位数字）。
    pub server_timestamp: i64,
}

/// 解析 `member/apply` 的响应。
///
/// # ⚠ 业务码是**数字** `code`，不是 `rtnCode`
///
/// 这是同一条链路上的**另一套**信封。`code == 0` 才是成功。
pub fn parse_apply_room(body: &str) -> Result<ApplyRoomResult> {
    let value: Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("member/apply 响应不是 JSON：{e}")))?;
    let code = int_at(&value, "code").unwrap_or(-1);
    if code != 0 {
        let msg = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Err(Error::Upstream {
            code: code.to_string(),
            message: format!("进聊天室失败：{msg}"),
        });
    }
    // `data` 可能缺失 —— 那时不判失败（进房间这个动作本身已经成功了），
    // 但 `roomId` 回显也就没有了。
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    Ok(ApplyRoomResult {
        app_id: str_at(&data, "appId"),
        app_key: str_at(&data, "appKey"),
        room_id: str_at(&data, "roomId"),
        server_timestamp: int_at(&value, "timestamp").unwrap_or(0),
    })
}

/// 房间 id 的字符串形态（`roomBeat` 与 `Referer` 都要字符串）。
///
/// 单独一个函数而不是各调用点 `to_string()`：房间 id 是 31 位的大整数，
/// 任何一处误用 `f64` 都会静默丢精度 —— 收敛到一处才好审。
pub fn live_room_id_string(room_id: i64) -> String {
    room_id.to_string()
}

// ══════════════════════════ 取值小助手 ══════════════════════════

/// 取一个整数。
///
/// # ⚠ 数字与数字字符串**都收**
///
/// 实拍踩过这个坑（§5.3 末尾）：上游解码保号（`json.Number`）时，
/// 只写「`as_i64` 一种形态」的取值助手会**静默返回 0** ——
/// 表现为「tab id 全是 0、拉到 0 间房」。
///
/// ⚠ 这里**只**对**数值型字段**放宽。业务码 `rtnCode` 走的是另一条路
/// （[`CmliveEnvelope::parse`]），**刻意**不放宽 —— 两件事不一样。
fn int_at(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// 取一个可选字符串，缺了或不是字符串一律当空串。
fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests;
