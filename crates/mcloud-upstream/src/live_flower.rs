//! 小红花（`liveRoomFeedback`）—— 口令抽小红花的**六步链协议层**（纯函数、零 IO）。
//!
//! 对应原项目 `packages/core/src/apis/live-room.ts` 与
//! `packages/core/src/tasks/red-flower/standalone.ts`，
//! 与 Go 侧 `internal/upstream/live_flower.go` 同源同行为。
//!
//! # 这条链的六步（顺序固定）
//!
//! ```text
//! ① POST /ycloud/liveRoomFeedback/user/info             ← 余额，**终值核对也是它**
//! ② POST /ycloud/visitlog/journaling              × 5   ← 埋点
//! ③ POST /ycloud/liveRoomFeedback/firstParticipate/give ← 首次参与惊喜奖
//! ④ POST /ycloud/liveRoomFeedback/redFlower/draw?code=  ← 逐个口令（**写**）
//! ⑤ GET  /ycloud/liveRoomFeedback/redFlower/queryRevivalFlowers
//! ⑥ POST /ycloud/liveRoomFeedback/user/info             ← 再取一次，前后对比
//! ```
//!
//! 本模块**只构造请求、只判定响应**：它一个请求都不发（那是 `mcloud-tasks` 的编排层）。
//!
//! # ⚠⚠ 四个请求的"请求体形态"**各不相同**，别统一成 JSON
//!
//! 2026-09-22 实拍（`docs/capture/app-live-redflower-samples.txt` §5.1）逐条钉住：
//!
//! | 端点 | content-type | body |
//! |---|---|---|
//! | `user/info` | `application/json` | `{}` |
//! | `visitlog/journaling` | `application/x-www-form-urlencoded` | `module=uservisit&…` |
//! | `firstParticipate/give` | **无**（`content-length: 0`） | 空 |
//! | `redFlower/draw` | **无**（`content-length: 0`） | 空 |
//!
//! ⇒ `give` 与 `draw` **不能**走 [`HttpRequest::json_body`]：那会补一个
//! `content-type: application/json` 并发出一个 `{}`（`content-length: 2`）。
//! 多发两个字节通常无害，但"照抄原实现"在这里恰好是对的
//! （原项目传的是 `undefined`），而**照抄直觉**（"POST 就该有 JSON body"）是错的。
//! [`HttpRequest`] 的 `body: None` 正好表达"无 body"。
//!
//! # 余额是 [`RedFlowerBalance::flower_cnt`]，不是排行榜那个
//!
//! ⚠ `getRedFlowerTop10List` 的 `flowerCnt` 是**别人的**（实拍第一名 2055），
//! 与 `app-exchange-samples.txt` §1.5「`beforeTotal` 不是余额」是同一类坑。
//! 所以本模块**根本不提供**那个接口 —— **不提供就不会被误用**。
//! 同理，`queryRevivalFlowers` 回的**复活花数**也不是余额（见
//! [`query_revival_flowers_request`]）。
//!
//! # ⚠⚠⚠ 业务码必须**对上它自己的接口** —— 不许张冠李戴
//!
//! | 码 | 出自 | 语义 | 本模块的落点 |
//! |---|---|---|---|
//! | `200108` | `redFlower/draw` | 口令无效（需当场直播获取） | [`RedFlowerDrawKind::InvalidKouling`] |
//! | `200110` | `firstParticipate/give` | 已参与过，无首次惊喜奖 | [`FirstParticipateKind::AlreadyJoined`] |
//! | `200112` | `redFlower/draw` | 已兑换 | [`RedFlowerDrawKind::AlreadyDrawn`] |
//! | `200118` | `redFlower/draw` | 口令已失效（本场已结束） | [`RedFlowerDrawKind::KoulingExpired`] |
//!
//! 这些码全部来自 [`crate::live_consts`]（**单一事实来源**，本文件不重复定义数值）。
//! ⚠ 码**只判等、不判形状** —— 别按"都是 `2001xx`"去推：
//! `200103`（红花数不够）出自**别的**接口，`609` / `200101` / `200111` 等等
//! 在 H5 自己的映射表里都是**独立的**码。
//!
//! # ⚠ 一处与原项目**刻意**不同（不是随手改，是有理由的偏离）
//!
//! 原 TS `standalone.ts` 把 draw 的判据写成 `typeof code === "number"` ⇒
//! **任何数字码**都算"无效口令"。那是它**没记过 `200108`** 时的兜底写法。
//! 实拍补上 `200108` 之后，"其他数字"是一个**尚未分类**的集合
//! ——里面可能有"活动没开始""账号被限"等等，把它们都当成"口令错"
//! 会触发一次毫无意义的 60 秒重听。
//!
//! ⇒ 本模块的判据是 [`RedFlowerDrawOutcome::rejected_kouling`]：
//! **只有 `200108` 算无效口令**。而任务描述里"其它数字码 → 无效口令"
//! 用**另一个**成员表达（[`RedFlowerDrawKind::Other`] 表示"是个数字码、但没分类"），
//! 由编排层决定怎么处置 —— 本层不替它做决定。
//!
//! ⚠ 这一处的代价是**可能漏掉一个将来新增的"口令无效"码**；
//! 那一侧的代价是"少重听一次"，而重听还有第二条路（`200108` 已覆盖实测的那一个）。
//! 判错方向**不对称**：把 `200118` 当无效口令 = 白重听 60 秒 + 把"直播结束了"
//! 误报成"口令抄错了"，把人引到完全错误的方向。

use mcloud_core::{Error, HttpRequest, Result};
use serde_json::Value;

use crate::live_consts::{
    BASE_LIVE_ROOM_FEEDBACK, CODE_FIRST_PARTICIPATE_ALREADY, CODE_KOULING_ALREADY_DRAWN,
    CODE_KOULING_EXPIRED, CODE_KOULING_INVALID, LIVE_ROOM_ACTIVITY_ID, LIVE_ROOM_APP_VERSION,
    LIVE_ROOM_JOURNALING_SOURCE_ID, LIVE_ROOM_MARKET_NAME, LIVE_ROOM_USER_AGENT,
};

// ══════════════════════════ 端点 ══════════════════════════

/// 小红花余额（同时也是**终值核对**那一枪）。
pub const USER_INFO_PATH: &str = "/liveRoomFeedback/user/info";

/// 埋点。
pub const JOURNALING_PATH: &str = "/visitlog/journaling";

/// 首次参与惊喜奖。
pub const FIRST_PARTICIPATE_GIVE_PATH: &str = "/liveRoomFeedback/firstParticipate/give";

/// 抽小红花（口令走 **query string**，不在 body 里）。
///
/// ⚠ 它是这条链上**唯一一个写请求**（会真的记账）。
pub const DRAW_PATH: &str = "/liveRoomFeedback/redFlower/draw";

/// 复活花查询。
pub const QUERY_REVIVAL_FLOWERS_PATH: &str = "/liveRoomFeedback/redFlower/queryRevivalFlowers";

/// 这一族共用的 `Origin`。
///
/// ⚠ 与直播侧 [`crate::live_consts::LIVE_ORIGIN`]（`https://v.cmcc-cs.cn`）**不是一个值**
/// —— 小红花在 `m.mcloud.139.com` 上，与签到同域。别"顺手统一"。
pub const LIVE_ROOM_ORIGIN: &str = crate::live_consts::BASE_MCLOUD;

// ══════════════════════════ 公共头 ══════════════════════════

/// 给小红花族的请求补上公共头（逐条对齐实拍 §5.1 的「公共请求头」段）。
///
/// ```text
/// Accept:      */*
/// activityid:  sign_in_3     ⭐ 与签到同族，不是直播专属
/// appversion:  12.6.0.0
/// Origin:      https://m.mcloud.139.com
/// Referer:     https://m.mcloud.139.com/portal/yunClound/index.html?path=…&sourceid=1007&targetSourceId=001005
/// User-Agent:  <iPad App WebView>
/// ```
///
/// # 为什么**没有** `jwttoken` 入参（与原 TS 不同的一处设计）
///
/// 与 [`crate::app_signin`] 同一条纪律：`jwttoken` 与 `Referer` 是**会话的两半**，
/// 由 [`crate::h5_client::H5Client`] 从**同一次**建会话里取，构造器不该各拼一遍
/// —— 两处各拼一遍的典型后果是"票是对的、referer 差一个参数"，
/// 而表现是 **90001 未登录**（那个码通常被读成"票过期了"，排查方向整个跑偏）。
///
/// 所以本模块只负责**与请求内容有关**的那几片头。
///
/// # ⚠ 刻意**不碰** `content-type`
///
/// 这一族里四个请求的 body 形态各不相同（见模块头），
/// 由各构造函数自己决定。这与 [`crate::app_signin`] 里"统一补 JSON"的做法**不同**，
/// 差别是故意的 —— 那边一条链上全是 JSON，这边不是。
fn live_room_headers(req: HttpRequest) -> HttpRequest {
    req.header("Accept", "*/*")
        .header("activityid", LIVE_ROOM_ACTIVITY_ID)
        .header("appversion", LIVE_ROOM_APP_VERSION)
        .header("Origin", LIVE_ROOM_ORIGIN)
        .header("Referer", crate::live_consts::live_room_page_referer())
        .header("User-Agent", LIVE_ROOM_USER_AGENT)
}

// ══════════════════════════ ① 余额 ══════════════════════════

/// 小红花余额 —— **六步链的①与⑥都取它**，前后相减才是"这次进账多少"。
///
/// 实拍（2026-09-22 09:04:13）：
///
/// ```text
/// {"code":0,"msg":"success","result":{"isQuarterDrawed":0,"flowerCnt":235,
///  "prizeId":0,"secondPrizeId":0,"redFlowerValidDate":"2026年12月31日",
///  "hasNotReceivedRedFlower":0}}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedFlowerBalance {
    /// 小红花数量（`result.flowerCnt`）—— 余额的**唯一**判据。
    pub flower_cnt: i64,
    /// 有效期，**中文串**（`2026年12月31日`），不是时间戳。
    ///
    /// ⚠ 别拿它去解析成时间：它没有时区、没有零填充，解析失败只会得到一句
    /// 与业务无关的报错。它是**给人看**的。
    pub red_flower_valid_date: String,
    /// 有没有还没领的小红花。
    pub has_not_received_red_flower: i64,
    /// 季度奖是否已抽。
    pub is_quarter_drawed: i64,
    /// 一等奖 id（`0` = 没中）。
    pub prize_id: i64,
    /// 二等奖 id（`0` = 没中）。
    pub second_prize_id: i64,
}

/// 构造 `POST /ycloud/liveRoomFeedback/user/info`。
///
/// body 是**空对象** `{}`（对齐实拍的 `content-length: 2`），不是"没有 body"。
pub fn user_info_request() -> HttpRequest {
    live_room_headers(
        HttpRequest::post(format!("{BASE_LIVE_ROOM_FEEDBACK}{USER_INFO_PATH}"))
            .json_body(&serde_json::json!({})),
    )
}

/// 解析 `user/info`。
///
/// # 为什么只硬要求 `flowerCnt`
///
/// 它是**唯一**回答了本函数问题（"我有几朵"）的字段。其余五个都是附属信息
/// —— 少一个只该让那一项留空，不该让整次查询失败（那样会把一次**成功**的请求
/// 变成解析错误，而错误信息里还看不出是哪个字段）。
///
/// ⚠ 但 `flowerCnt` **必须**硬要求：把"字段没了"静默成 `0` 会让
/// "余额 235 → 0"看起来像**一次真实的清零**，而那正是最该立刻报警的形状。
pub fn parse_red_flower_balance(env: &RedFlowerEnvelope) -> Result<RedFlowerBalance> {
    let r = env.ok_result("查询小红花余额")?;
    let flower_cnt = int_at(r, "flowerCnt")
        .ok_or_else(|| Error::Parse("user/info 响应缺少 result.flowerCnt（余额）".into()))?;
    Ok(RedFlowerBalance {
        flower_cnt,
        red_flower_valid_date: str_at(r, "redFlowerValidDate"),
        has_not_received_red_flower: int_at(r, "hasNotReceivedRedFlower").unwrap_or(0),
        is_quarter_drawed: int_at(r, "isQuarterDrawed").unwrap_or(0),
        prize_id: int_at(r, "prizeId").unwrap_or(0),
        second_prize_id: int_at(r, "secondPrizeId").unwrap_or(0),
    })
}

// ══════════════════════════ ② 埋点 ══════════════════════════

/// 原项目在抽奖前发的 5 条埋点（**顺序即发送顺序**）。
///
/// # ⚠ 实拍只证实了其中一条
///
/// 2026-09-22 实拍抓到的只有 `National_LiveRoomFeedback_flowerBtn`
/// —— 而它是**点按钮那一刻**发的（`journaling` → `draw` 相邻）。
/// 其余四条（`pv` / `client` / `client_isApp` / `login`）在真实客户端里属于
/// **页面加载期**，本轮抓包窗口里没出现（很可能是窗口之前就发过了）。
///
/// 那为什么还照原项目发全 5 条？因为这四条是**服务端活动系统的"到场"证据**，
/// 而"它们到底是不是必发"**没有证据**。两边的代价不对称：
/// 多发 4 个埋点请求 = 一点点噪声；少发 = 可能整条抽奖链被服务端静默拒掉
/// （而那时的表现是 `200108`，看起来像"口令抄错了"，会把人引到完全错误的方向）。
/// ⇒ 照抄原项目，并把"未验证"这件事写在这里。
pub const JOURNALING_EVENTS: [&str; 5] = [
    "National_LiveRoomFeedback_pv",
    "National_LiveRoomFeedback_client",
    "National_LiveRoomFeedback_client_isApp",
    "National_LiveRoomFeedback_login",
    "National_LiveRoomFeedback_flowerBtn",
];

/// 构造 `POST /ycloud/visitlog/journaling`。
///
/// body 是 **form-urlencoded**（不是 JSON），逐字：
///
/// ```text
/// module=uservisit&optkeyword=<…>&sourceid=1007&marketName=National_LiveRoomFeedback
/// ```
///
/// ⚠ 参数顺序照抄原项目。form 体的顺序在服务端通常无所谓，但这里没有理由偏离
/// —— 而"照抄"让下一次抓包对拍时能**直接逐字节比**。
///
/// # `optkeyword` 里的 `&` / `=` 要编码
///
/// 埋点事件名本身只有字母数字下划线，但它是**入参**：真实客户端这一格
/// 也可能带上用户侧的关键词。不编码会把一个 `&` 变成"多一个参数"，
/// 而服务端**不会报错**，只是收到一个看不懂的埋点。
/// ⚠ 这里**不是** `application/json`：走 [`HttpRequest::json_body`] 会发出
/// 一个带引号的 JSON 串，与实拍的 form 体不是一回事。
pub fn journaling_request(optkeyword: &str) -> HttpRequest {
    let body = format!(
        "module=uservisit&optkeyword={}&sourceid={}&marketName={}",
        form_encode(optkeyword),
        LIVE_ROOM_JOURNALING_SOURCE_ID,
        LIVE_ROOM_MARKET_NAME
    );
    live_room_headers(
        HttpRequest::post(format!("{BASE_LIVE_ROOM_FEEDBACK}{JOURNALING_PATH}"))
            .text_body(body)
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded;charset=UTF-8",
            ),
    )
}

/// form-urlencoded 的值编码。
///
/// ⚠ 与 query 的百分号编码**不是同一个集合**：form 体里空格要写成 `+`
/// （`application/x-www-form-urlencoded` 的规定），其余"不安全字符"一律 `%XX`。
///
/// 本仓已有 [`crate::app_signin::percent_encode`]，但它的空格是 `%20`
/// —— 在 query 里对，在 form 体里不对（服务端会解成一个字面量 `+` 而不回退）。
/// 这一族只有这一处用 form 体，所以定义在本地。
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// query string 的百分号编码。
///
/// 口令是**中文**，必须编码：不编码的话 `&` / `=` / `+` 会把它截断成另一个口令，
/// 而服务端只会回一个 `200108` —— 看起来像"口令抄错了"。
///
/// ⚠ 这与 [`form_encode`] 的空格处理**刻意不同**（这里是 `%20`）：
/// query 里 `+` 表示空格，而口令本身可能含 `+`，写成 `+` 会把两者混成同一个形状。
///
/// 只编码"会破坏 query 语义"的字符，其余原样保留 —— 与抓包一致
/// （黄金向量 `draw.expectQuery` 里那条中文口令编出来全是 `%XX`）。
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ══════════════════════════ ③ 首次参与惊喜奖 ══════════════════════════

/// `firstParticipate/give` 的三种结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirstParticipateKind {
    /// `code == 0`：拿到了惊喜奖。
    #[default]
    Gained,
    /// [`CODE_FIRST_PARTICIPATE_ALREADY`]（`200110`）：**已经参与过**。
    ///
    /// 它是**静默**的一支 —— 不是错误，是"这次没有"。
    /// 每个账号第二次跑这条链都会拿到它。
    AlreadyJoined,
    /// 别的码：原样带回给调用方，由编排层决定该不该报错。
    Other,
    /// 响应里**没有 `code` 字段**。
    ///
    /// 与 [`Self::Other`] 分开：前者是"上游说了个别的码"，
    /// 后者是"上游这次没按信封说话"—— 两者的排查方向完全不同。
    Unparsable,
}

/// `firstParticipate/give` 的判定结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FirstParticipateOutcome {
    pub kind: FirstParticipateKind,
    /// 只在 [`FirstParticipateKind::Gained`] 下有效。
    pub flower_num: i64,
    /// 原始业务码（`None` = 响应里没有 `code` 字段）。
    pub code: Option<i64>,
    /// 上游原文，**只进日志**。
    pub message: String,
}

impl FirstParticipateOutcome {
    /// 这一次真的拿到了首次参与惊喜奖吗。
    ///
    /// ⚠ 用它的地方是**记账**：只有它为真时才该把 `flower_num` 加进"本次进账"。
    pub fn is_success(&self) -> bool {
        self.kind == FirstParticipateKind::Gained
    }

    /// 这一次是不是"已经参与过"（**正常的没得领**，不是故障）。
    pub fn is_already_joined(&self) -> bool {
        self.kind == FirstParticipateKind::AlreadyJoined
    }
}

/// 构造 `POST /ycloud/liveRoomFeedback/firstParticipate/give`。
///
/// ⚠ body **空**、**无 `content-type`**（实拍 `content-length: 0`）——
/// 别调 [`HttpRequest::json_body`]（那会发出一个 `{}`，
/// 并补上 `content-type: application/json`）。
pub fn first_participate_give_request() -> HttpRequest {
    live_room_headers(HttpRequest::post(format!(
        "{BASE_LIVE_ROOM_FEEDBACK}{FIRST_PARTICIPATE_GIVE_PATH}"
    )))
}

/// 判定 `firstParticipate/give` 的结果。
///
/// # 为什么**不**用 [`RedFlowerEnvelope::ensure_ok`]
///
/// 那个把"非 0 即失败"一律变成错误，而 `200110`（已参与过）是**正常**的一支
/// —— 每个账号第二次跑这条链都会拿到它。用 `ensure_ok` 的话，一次完全正常的运行
/// 会以一句"上游返回 code=200110"结束，读起来像故障。
///
/// # 任务描述里那句"`200110` 之外的非零码只记 info，不当失败"
///
/// 本函数**如实照做**：`Other` 与 `Unparsable` 都**不返回 `Err`**，
/// 只把 code / message 原样带出来。**该不该中断整批由编排层决定** ——
/// 这一层只回答"上游说了什么"。
///
/// ⚠ 但这不等于"永远不报错"：本仓的纪律是**禁止全链吞错**。
/// 把这一支放进报告的 `info` 里是编排层的选择，不是本层的默认。
pub fn classify_first_participate(env: &RedFlowerEnvelope) -> FirstParticipateOutcome {
    let mut out = FirstParticipateOutcome {
        code: env.code,
        message: env.message.clone(),
        ..Default::default()
    };
    let Some(code) = env.code else {
        out.kind = FirstParticipateKind::Unparsable;
        return out;
    };
    out.kind = match code {
        0 => {
            // 成功时也把 `flowerNum` 读出来 —— 它是"这次进账多少"的一部分。
            out.flower_num = env
                .result
                .as_ref()
                .and_then(|r| int_at(r, "flowerNum"))
                .unwrap_or(0);
            FirstParticipateKind::Gained
        }
        CODE_FIRST_PARTICIPATE_ALREADY => FirstParticipateKind::AlreadyJoined,
        _ => FirstParticipateKind::Other,
    };
    out
}

// ══════════════════════════ ④ 抽小红花（**写**）══════════════════════════

/// `draw` 成功时的 `result`。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedFlowerDrawResult {
    /// 奖品 id。
    pub prize_id: i64,
    /// **这一口令换到几朵**（不是余额）。
    pub flower_num: i64,
    /// 奖品名（原项目把它打进日志：`获得 N 朵（<prizeName>）`）。
    pub prize_name: String,
    /// 奖品类型。
    pub prize_type: i64,
}

/// `draw` 的判定结果。
///
/// ⚠ 顺序有语义：`Succeeded` 是 `0` 值那一档，新增的档**只能往后加**
/// （数值一旦被人读去当判据就收不回来）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedFlowerDrawKind {
    /// `code == 0` **且** `result` 非空。
    ///
    /// ⚠ `#[default]` 标在**第一个**变体上是刻意的：0 值 = 成功，
    /// 于是将来新增档位**只能往后加**（数值一旦被人读去当判据就收不回来）。
    #[default]
    Succeeded,
    /// [`CODE_KOULING_ALREADY_DRAWN`]（`200112`）：已兑换。
    ///
    /// ⚠ 它**不是**无效口令：口令本身是**对的**。
    /// 别把它并进 [`RedFlowerDrawOutcome::rejected_kouling`]。
    AlreadyDrawn,
    /// [`CODE_KOULING_INVALID`]（`200108`）：**口令不对**（实拍确认）。
    InvalidKouling,
    /// [`CODE_KOULING_EXPIRED`]（`200118`）：**这条口令认识、但它已经失效了**
    /// （本场直播已结束）。
    ///
    /// ⭐ 2026-09-23 17:09 实拍：同账号、同样"直播已关"，用**真但已过期**的口令去领
    /// ⇒ `200118`；用**乱码 / 不存在的口令** ⇒ `200108`
    /// ⇒ 它是**口令级**的码（"我认识它、它失效了"），**不是**场次级
    /// （"没直播一律挡"）。
    ///
    /// ⚠⚠ 它**不是**无效口令：错的回 `200108`。所以
    /// [`RedFlowerDrawOutcome::rejected_kouling`] 对它必须返回 `false`
    /// —— 否则会触发一次毫无意义的 60 秒重听。
    KoulingExpired,
    /// **别的**业务码（是个数字，但本层没分类）。
    ///
    /// ⚠ 它**不算**无效口令（见模块头"刻意不同的一处"）。
    Other,
    /// 响应里**没有 `code` 字段**，或者 `code == 0` 却没有 `result`。
    ///
    /// 原项目这一支是"warn 之后跳过、**不判无效**"（避免误伤）。
    /// 单独一类而不是并进 [`Self::Other`]：前者是"上游说了个别的码"，
    /// 后者是"上游这次没按信封说话"。
    Unparsable,
}

impl RedFlowerDrawKind {
    /// 稳定的字面名（**给日志与人读**，不是 `Debug` 的替代品）。
    ///
    /// ⚠ 它与黄金向量的 `expectKind` 逐字一致 —— 那是**对拍接口**，
    /// 改名会让向量与实现对不上。`unparsable` 是本层多出来的一档
    /// （原 TS 那一支落在"跳过"上，向量里没有对应用例）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::AlreadyDrawn => "already_drawn",
            Self::InvalidKouling => "invalid_kouling",
            Self::KoulingExpired => "kouling_expired",
            Self::Other => "other",
            Self::Unparsable => "unparsable",
        }
    }
}

/// 一次 `draw` 的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedFlowerDrawOutcome {
    pub kind: RedFlowerDrawKind,
    /// 只在 [`RedFlowerDrawKind::Succeeded`] 下有值。
    pub result: RedFlowerDrawResult,
    /// 原始业务码（`None` = 没有这个字段）。
    pub code: Option<i64>,
    /// 上游原文，**只进日志**。
    pub message: String,
}

impl RedFlowerDrawOutcome {
    /// 这一次真的抽到了吗。
    pub fn is_success(&self) -> bool {
        self.kind == RedFlowerDrawKind::Succeeded
    }

    /// **这一个口令是否应当被判为无效**（决定要不要重听一次 / 重领）。
    ///
    /// ⭐ 只有实拍确认的那一个码算 —— 见模块头"刻意不同的一处"。
    ///
    /// ⚠ `200112`（已兑换）与 `200118`（已失效）都**不是**无效口令：
    /// 口令本身是对的。把它们算进来会把一个**有效**口令从候选里剔掉，
    /// 让同一批账号里后面的那几个白跑一趟。
    ///
    /// ⚠ `Unparsable` 也**不算**：原项目那一支刻意"不判无效"（避免误伤）
    /// —— 响应解不开时我们**不知道**口令对不对，把它当"错"的代价是
    /// 丢掉一个可能是真的口令，而那一侧没有第二次机会。
    pub fn rejected_kouling(&self) -> bool {
        self.kind == RedFlowerDrawKind::InvalidKouling
    }

    /// 这一次是不是"口令对、但已经兑换过"（**不算失败**）。
    pub fn is_already_drawn(&self) -> bool {
        self.kind == RedFlowerDrawKind::AlreadyDrawn
    }

    /// 这一次是不是"口令认识、但本场已结束"。
    pub fn is_kouling_expired(&self) -> bool {
        self.kind == RedFlowerDrawKind::KoulingExpired
    }
}

/// 构造 `POST /ycloud/liveRoomFeedback/redFlower/draw?code=<urlencoded>`。
///
/// # 口令走 **query string**，不是 body
///
/// 实拍（§5.1 第 5 条）：`POST …/redFlower/draw?code=%E4%B8%AD%E7%A7%8B…`，
/// body **空**（`content-length: 0`、**无 content-type**）。原项目也是 `undefined` 作 body。
///
/// ⚠ 口令是**中文**，必须百分号编码（见 [`percent_encode`]）。
pub fn draw_request(code: &str) -> HttpRequest {
    live_room_headers(HttpRequest::post(format!(
        "{BASE_LIVE_ROOM_FEEDBACK}{DRAW_PATH}?code={}",
        percent_encode(code)
    )))
}

/// 判定 `draw` 的结果。
///
/// **不返回错误**：这一层只回答"上游说了什么"。
/// 判定结果里只有 [`RedFlowerDrawKind::Other`] 与 [`RedFlowerDrawKind::Unparsable`]
/// 是"未分类"，而它们该不该中断整批由编排层决定 ——
/// **逐个口令重试是正常路径**，不该让一个口令的异常把其余口令一起带走。
pub fn classify_red_flower_draw(env: &RedFlowerEnvelope) -> RedFlowerDrawOutcome {
    let mut out = RedFlowerDrawOutcome {
        code: env.code,
        message: env.message.clone(),
        ..Default::default()
    };
    let Some(code) = env.code else {
        out.kind = RedFlowerDrawKind::Unparsable;
        return out;
    };
    out.kind = match code {
        0 => {
            // 判据照抄原项目：`code === 0 && result` —— **两个都成立**才算成功。
            // 只有 code 而没 result，说明拿不到"换到几朵"，
            // 那就不能记成一次成功（否则余额对不上却没有任何线索）。
            let Some(r) = env.result.as_ref() else {
                out.kind = RedFlowerDrawKind::Unparsable;
                return out;
            };
            out.result = RedFlowerDrawResult {
                prize_id: int_at(r, "prizeId").unwrap_or(0),
                flower_num: int_at(r, "flowerNum").unwrap_or(0),
                prize_name: str_at(r, "prizeName"),
                prize_type: int_at(r, "prizeType").unwrap_or(0),
            };
            RedFlowerDrawKind::Succeeded
        }
        CODE_KOULING_ALREADY_DRAWN => RedFlowerDrawKind::AlreadyDrawn,
        CODE_KOULING_INVALID => RedFlowerDrawKind::InvalidKouling,
        CODE_KOULING_EXPIRED => RedFlowerDrawKind::KoulingExpired,
        _ => RedFlowerDrawKind::Other,
    };
    out
}

// ══════════════════════════ ⑤ 复活花 ══════════════════════════

/// 构造 `GET /ycloud/liveRoomFeedback/redFlower/queryRevivalFlowers`。
///
/// ⚠ 它只回**复活花**的数量（实拍 `{"code":0,"msg":"success","result":0}`），
/// **不是**小红花余额 —— 别拿它当余额（见模块头）。
/// 它是这条链上**唯一一个 GET**。
pub fn query_revival_flowers_request() -> HttpRequest {
    live_room_headers(HttpRequest::get(format!(
        "{BASE_LIVE_ROOM_FEEDBACK}{QUERY_REVIVAL_FLOWERS_PATH}"
    )))
}

/// 解析 `queryRevivalFlowers`。
///
/// ⚠ `result` 是**裸数字**（不是对象），所以这里不能按"要对象"去解。
pub fn parse_revival_flowers(env: &RedFlowerEnvelope) -> Result<i64> {
    let r = env.ok_result("查询复活花")?;
    match r {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| Error::Parse("queryRevivalFlowers 的 result 不是整数".into())),
        // 上游有的字段是字符串形式的数字 —— 两种都收。
        Value::String(s) => s
            .parse()
            .map_err(|_| Error::Parse("queryRevivalFlowers 的 result 不是整数".into())),
        _ => Err(Error::Parse(
            "queryRevivalFlowers 的 result 不是整数".into(),
        )),
    }
}

// ══════════════════════════ 响应信封 ══════════════════════════

/// 小红花族的响应信封 `{code, msg, result}`。
///
/// # 这一族的 code 是**数字**
///
/// ⚠ 与直播侧 [`live_room`] 那套 `{rtnCode:"0", rtnMsg, object}`（**字符串**码）
/// 不是同一个约定。**两套信封不能互换**：拿这一套去解 `/cmlive` 的响应会永远判失败
/// （它把 `"0"` 当成"没有 code"），反过来也一样。
///
/// # 为什么不复用 [`crate::app_signin::SignInEnvelope`]
///
/// 两者的形状**完全一样**（同域、同 `{code,msg,result}`），但它们的**失败语义不同**：
/// 小红花这边有一批**不该报错**的非零码（`200112` / `200110`），
/// 而 `SignInEnvelope::ensure_ok` 的语义是"非 0 即失败"。
/// 让小红花这一族复用它，等于把"用错语义"这件事藏进 import 里 ——
/// 所以这里单独一个类型，哪怕字段一模一样。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedFlowerEnvelope {
    /// `None` = 响应里**没有** `code` 字段。
    ///
    /// ⚠ 这里刻意用 `Option` 而不是 `unwrap_or(-1)`：`draw` 与 `give` 的判定
    /// 都把"没有 code"当成**独立的一档**（`unparsable`），
    /// 折成一个哨兵值会让它混进"别的业务码"里，而两者的排查方向完全不同。
    pub code: Option<i64>,
    pub message: String,
    /// `result` 原样保留。`None` = 响应里没有这个键。
    pub result: Option<Value>,
}

impl RedFlowerEnvelope {
    /// 解析响应文本。
    ///
    /// ⚠ **不是 JSON 时不报错**，而是返回一个 `code: None` 的信封 ——
    /// 理由见 [`RedFlowerDrawKind::Unparsable`]：原项目那一支刻意
    /// "warn 之后跳过、不判无效"，正是为了**避免误伤**。
    /// 在这里直接 `Err` 会逼调用方写一个 `unwrap_or_else`，
    /// 而那个兜底很容易被写成"当成失败"，就把这一档的语义丢掉了。
    ///
    /// 需要"非要 JSON 不可"的地方自己判 [`Self::code`] 是否为 `None`。
    pub fn parse(body: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(body) else {
            return Self::default();
        };
        Self::from_value(&v)
    }

    /// 从一个已解析的 JSON 值构造。
    pub fn from_value(v: &Value) -> Self {
        // code 既可能是数字也可能是数字字符串 —— 两种都收。
        let code = match v.get("code") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        };
        // 上游用过 `msg` 与 `message` 两个键名。
        let message = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Self {
            code,
            message,
            result: v.get("result").cloned(),
        }
    }

    /// 成功时取 `result`。
    ///
    /// 判据是 `code == Some(0)`（这一族没有别的成功码）。
    pub fn ok_result(&self, what: &str) -> Result<&Value> {
        self.ensure_ok(what)?;
        self.result
            .as_ref()
            .ok_or_else(|| Error::Parse(format!("{what} 响应没有 result")))
    }

    /// 成功判定但**不要求** `result`。
    ///
    /// ⚠ 用它的地方只有"这一步成功与否本身就是结论"的接口
    /// （`user/info` 要 `result`，所以那边走 [`Self::ok_result`]）。
    pub fn ensure_ok(&self, what: &str) -> Result<()> {
        if self.code == Some(0) {
            return Ok(());
        }
        Err(self.error(what))
    }

    /// 把非 0 业务码翻成一条**能指导排查**的错误。
    fn error(&self, what: &str) -> Error {
        let Some(code) = self.code else {
            return Error::Parse(format!("{what} 响应不是 JSON，或缺少 code 字段"));
        };
        // 会话失效要单独说清楚（与 `app_signin` 同一条纪律）：
        // `90001` 不是"任务失败"，是**票的问题** —— 票过期了，或者这张票
        // 不属于当前页面（`referer` 与会话对不上）。混进一句干巴巴的
        // "失败 code=90001"，人会去查账号、查权限、查重试，
        // 而真正该做的是**重建会话**。
        if code == crate::session_source::CODE_SESSION_EXPIRED {
            return Error::Other(format!(
                "{what} 回会话失效（code={code}）—— {}",
                crate::session_source::describe(crate::session_source::CODE_SESSION_EXPIRED)
            ));
        }
        Error::Upstream {
            code: code.to_string(),
            message: format!("{what}：{}", self.message),
        }
    }
}

// ══════════════════════════ 小工具 ══════════════════════════

fn int_at(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        // 上游有的字段是字符串形式的数字
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests;
