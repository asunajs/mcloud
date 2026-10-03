//! 兑换（抢兑）—— 读清单、取滑块题、下单。
//!
//! 对应参考实现的 `internal/upstream/app_exchange*.go`，消费
//! `test/golden/app-exchange-vectors.json`（实拍请求/响应）。
//!
//! # 四枪，三条链路
//!
//! | 枪 | 方法 | 路径 | 作用 |
//! |---|---|---|---|
//! | `exchangeList` | GET | `/ycloud/signin/page/exchangeList` | 读奖品清单（**只读**）|
//! | `prizeGroup.json` | GET | `/ycloud/signin/public/text/<活动>/prizeGroup.json` | 读**组字典**：组名与"每月多选一限兑"规则（**静态、只读、免鉴权**）|
//! | `getSlide` | POST | `/ycloud/auth-service/slide/getSlide` | 取滑块题面（**只读**）|
//! | `exchangeV3` | POST | `/ycloud/signin/page/exchangeV3` | **下单（花真豆）** |
//!
//! 注意 `getSlide` 的路径族是 `/ycloud/auth-service/slide/`，
//! 与 `exchangeList` 的 `/ycloud/signin/page/`、组字典的 `/ycloud/signin/public/text/`
//! **三条都不同** —— 别顺手复用哪一族的请求构造。
//!
//! # `code` 的两种语义，**不能一刀切**
//!
//! - `exchangeList` / `getSlide`：非 0 码**就是错误**（没有"非 0 但仍是数据"的档）。
//! - `exchangeV3`：**码是数据不是错误**。`514` 可以重取滑块再来（代价为零），
//!   `2301` 再怎么试也不会成交，`415` 说明这件今天别再试、明天还有 ——
//!   三档处置完全不同，而它们都是**正常业务响应**。
//!
//! 所以下单那条**刻意不走** [`SignInEnvelope::ensure_ok`]。
//!
//! # 证据
//!
//! 逐字实拍样本在**上游参考仓**：`../mcloud-capture/docs/capture/app-exchange-samples.txt`
//! （`exchangeList` / `exchangeV3` / `getSlide` 的请求与响应逐字，含 4 轮兑换、3 错 1 成）；
//! 组字典那一枪是本仓 2026-10-01 抓的，落在
//! `../mcloud-capture/docs/capture/app-exchange-samples.txt`（含响应逐字与四个坑）。
//!
//! ⚠ 这里以前写的是 `test/golden/app-exchange-vectors.json`。2026-10-01 核实：
//! **该文件在四个仓（本仓 / `mcloud` / `mcloud-rust` / `mcloud-sign` / `mcloud-capture`）
//! 里都不存在**，是一条指不到的引用 —— 已改成上面那两份真实样本。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};

use crate::app_signin::{h5, percent_encode, SignInEnvelope};
use crate::endpoints::{EXCHANGE_LIST, EXCHANGE_V3, GET_SLIDE, MCLOUD_HOST};

// ══════════════════════════════ 常量 ══════════════════════════════

/// 下单体里那个 `clientVersion`。
///
/// ⚠ **不等于**请求头 `appversion` 的 `12.6.0.0`（四段）——两个值在**同一个请求里**
/// 同时出现且不一样（实拍如此），别互抄。
/// `POST /ycloud/visitlog/journaling` —— 访问日志埋点（兑换前预热用，见
/// [`report_visit_log_request`]）。取自 `mcloud-sign` 的 `apis/exchange.ts:17`。
pub const VISIT_LOG_JOURNALING: &str = "https://m.mcloud.139.com/ycloud/visitlog/journaling";

/// 兑换链在 TS 里用的那个 `optkeyword`（`tasks/exchange/task.ts:86`）。
pub const VISIT_LOG_KEYWORD_EXCHANGE: &str = "newsignin_index_to_exchange";

/// 预热的**第二枪**用的 `optkeyword`（`tasks/exchange/task.ts:88`）：
/// 两枪之间夹着 `sleep(PREHEAT_DELAY)`，顺序是契约。
pub const VISIT_LOG_KEYWORD_EXCHANGE_TIMES: &str = "newsignin_index_exchange_times";

/// TS 的 `marketName` 默认值（`apis/exchange.ts:15` 的默认参数）。
pub const VISIT_LOG_MARKET_NAME_DEFAULT: &str = "sign_in_3";

pub const EXCHANGE_CLIENT_VERSION: &str = "12.6.0";

/// `getSlide` 实拍的 `content-type`。
///
/// ⚠⚠ 它**不等于**这一族别的请求用的 `application/json;charset=UTF-8`。
/// 差别不是理论上的：探针把这条发成 JSON **也照样回 `code 0`**，
/// 于是"请求头与真机不一致"这件事在探针那一轮完全没露头。
/// 按**实拍**构造，而不是按"能过的那一行"：上游今天不校验的头，
/// 哪天开始校验时错的那一方是**静默失效**，不会报错。
pub const SLIDE_FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded;charset=UTF-8";

/// `limit` 的五档 —— 本账号对**这一件**奖品的可兑状态。
///
/// ⚠ 每一档的证据强度**不一样**，别当成一张同级字典：
///
/// - `0` / `2` / `4` **有字段依据**：`0` 是可兑那件的实拍取值；`2` 由"成功兑掉
///   一件之后它的 `limit` 由 `0` 变 `2`"钉住；`4` 由 `probability2` 里的
///   `monthGroupLimit`（数组）+ `limitMonthQuantity` 钉住（同组另外三件全是 `4`，
///   而只兑过其中一件）。
/// - `1` **只有 H5 那句文案**（「明日再来」）。本轮样本里一件都没有。
/// - `3` 两处文案**互相冲突**（H5 说「已兑换」、旧实现说「暂不可用」）⇒ 存疑。
pub const LIMIT_AVAILABLE: i64 = 0;
/// `1`：文案「明日再来」。**零样本**。
pub const LIMIT_TOMORROW: i64 = 1;
/// `2`：**这一件本身**本月已兑过。
pub const LIMIT_EXCHANGED: i64 = 2;
/// `3`：两处文案冲突那一档（存疑）。
pub const LIMIT_UNKNOWN3: i64 = 3;
/// `4`：**同组别的件**本月已兑（组名额用掉了）。
pub const LIMIT_GROUP_EXCHANGED: i64 = 4;

/// `501 滑块验证参数不能为空`（实拍）。**第 1 道闸**。
pub const CODE_SLIDER_PARAM_MISSING: i64 = 501;
/// `514 验证失败`（实拍 3 条）：`puzzleOffset` 非空但算错了。
///
/// 代价为零（三条 `514` 期间余额始终没动）⇒ **只有这一档**可以重取滑块再来一次。
pub const CODE_SLIDER_FAILED: i64 = 514;
/// `407 非移动用户不可领奖`（实拍）。**账号级**闸门，两件不同奖品回同一个码。
pub const CODE_NOT_TARGET_USER: i64 = 407;
/// `415 奖品单日已耗尽`（实拍，**不在 H5 那张表里**）。
///
/// 说的是**奖品的池子**空了，不是"我这个账号的份额用完"（那是 `420`）。
pub const CODE_PRIZE_EXHAUSTED_TODAY: i64 = 415;
/// `2301 重复兑奖`（实拍，**四位**，其余全是三位）。
///
/// ⭐ 它是这条链上"服务端有幂等语义"的唯一实证：同账号 + 同 `prizeId` 不重复发奖。
pub const CODE_DUPLICATED: i64 = 2301;
/// `420`：H5 文案「该奖品每日限兑，您今日份额已用完」。**零样本**。
pub const CODE_DAILY_QUOTA: i64 = 420;
/// `421`：H5 文案「该奖品每月限兑，您本月份额已用完」。**零样本**。
pub const CODE_MONTHLY_QUOTA: i64 = 421;
/// `610`：H5 文案「兑换失败，请稍后再试或联系客服~」。
///
/// ⚠ **2026-10-02 已观测**（线上真发单那次）：上游回的是
/// 「活动异常，请稍后重试！Error Code：GK」—— 与 H5 表里那句**不是同一句**，
/// 而且 `GK` 在抓包仓与 `mcloud-sign` 里**都查不到样本** ⇒ 不许猜它是什么。
/// 已知的只有：**它不是 deviceId 那一档**（按抓包，缺 id 回 `615`、内容不对回 `614`）。
pub const CODE_GENERIC: i64 = 610;
/// `614` / `615`：H5 给这两档**同一句文案**。
///
/// ⚠⚠ 别照文案分类，必须读 `code`。同两个数在**签到族**里另有已实证的含义
/// （那条线是 deviceId 对不对）；在**这一族**里究竟是不是同一回事，**没有样本**。
pub const CODE_BUSY_A: i64 = 614;
/// 见 [`CODE_BUSY_A`]。
pub const CODE_BUSY_B: i64 = 615;

/// 成功码。
pub const CODE_OK: i64 = 0;

// ══════════════════════════ 请求构造（纯函数） ══════════════════════════

/// `GET /ycloud/signin/page/exchangeList` —— 兑换清单。**只读**。
///
/// 查询串**只有** `client` 与 `clientVersion` 两个参数，**不带 `deviceId`**
/// （H5 那条只标了 `showLoading`，没标 `isDeviceId`）。
pub fn exchange_list_request() -> HttpRequest {
    let url = format!(
        "{EXCHANGE_LIST}?client=app&clientVersion={}",
        percent_encode(EXCHANGE_CLIENT_VERSION)
    );
    h5(false, HttpRequest::get(url), "true")
}

/// `POST /ycloud/auth-service/slide/getSlide` —— 取滑块题面。**只读**。
///
/// # 为什么 body 要手写 `{}` 而不是空 body
///
/// 实拍的 `Content-Length: 2` 是抓包的一部分（H5 源码就是 `getSlidePuzzle({})`）。
/// 空 body 的 POST 在 HTTP 层合法，但那是**另一种**客户端形态。
///
/// # content-type 必须按实拍那一行
///
/// 见 [`SLIDE_FORM_CONTENT_TYPE`]。这里先删掉 `h5_headers` 可能补的
/// `application/json`，再补上 form 那一个 —— 结果**恰好一个**。
pub fn get_slide_request() -> HttpRequest {
    let req = HttpRequest::post(GET_SLIDE).text_body("{}");
    let mut req = h5(false, req, "true");
    req.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
    req.header("Content-Type", SLIDE_FORM_CONTENT_TYPE)
}

/// `POST /ycloud/visitlog/journaling` —— **兑换前的"预热"埋点**。**只读、无副作用**。
///
/// # 为什么要有这一枪（证据：`mcloud-sign`，能跑的 TS）
///
/// `mcloud-sign/packages/core/src/tasks/exchange/task.ts:86-87`：
///
/// ```text
/// await reportVisitLog(http, "newsignin_index_to_exchange");
/// await sleep(PREHEAT_DELAY);          // ⚠ 500ms，定义在 tasks/exchange/utils.ts:7
/// ```
///
/// 也就是**先报一次访问日志、再等 500ms、然后才下单** —— 顺序本身是这段链条的一部分，
/// 不是可有可无的日志。本仓原先**完全没有**这一枪。
///
/// # 请求形状逐字照 TS（`packages/core/src/apis/exchange.ts:15-23`）
///
/// | 项 | 值 |
/// |---|---|
/// | 方法 | `POST` |
/// | 查询串 | `module=uservisit&optkeyword=<keyword>&sourceid=1002&marketName=<marketName>` |
/// | body | **空**（TS 传的是 `""`，Content-Length: 0）|
/// | content-type | `application/x-www-form-urlencoded;charset=UTF-8` ＝ [`SLIDE_FORM_CONTENT_TYPE`] |
/// | `marketName` 缺省 | `sign_in_3`（TS 的默认参数；兑换链传的是 `newsignin_index_to_exchange` 那个 keyword，marketName 走默认）|
///
/// ⚠ 与 [`get_slide_request`] 同样要**先把 `h5()` 可能补上的 `content-type` 删掉**再
/// 设成 form 那一个 —— 否则会留下两个 `content-type`，那就不是"同一个客户端"了。
pub fn report_visit_log_request(keyword: &str, market_name: &str) -> HttpRequest {
    let url = format!(
        "{VISIT_LOG_JOURNALING}?module=uservisit&optkeyword={}&sourceid=1002&marketName={}",
        percent_encode(keyword),
        percent_encode(market_name)
    );
    let req = HttpRequest::post(url).text_body("");
    let mut req = h5(false, req, "true");
    req.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
    req.header("Content-Type", SLIDE_FORM_CONTENT_TYPE)
}

/// `POST /ycloud/signin/page/exchangeV3` —— **下单（花真豆）**。
///
/// # 请求体是手拼的，键序是契约
///
/// `prizeId → client → clientVersion → puzzleOffset → smsCode → deviceId`。
/// 用 `serde_json` 序列化会是字母序 —— 那与实拍不同（虽然服务端不校验键序，
/// 但这里没有理由引入一处"看起来一样、字节不同"的差异）。
///
/// # `puzzleOffset` 的三种形态
///
/// 整数（正确值）、`null`（`offset` 为 `None` ⇒ 回 `501`）。
/// 实拍里**空串与 `null` 逐字相同**地触发 `501`，所以这里只暴露 `None` 这一种。
///
/// # 两个额外的头
///
/// `isdeviceid: true` 与 `origin: https://m.mcloud.139.com` 是实拍里额外带的，
/// 少一个就不是"同一个客户端"了。
pub fn exchange_v3_request(device_id: &str, prize_id: i64, offset: Option<i64>) -> HttpRequest {
    let offset_raw = match offset {
        Some(v) => v.to_string(),
        None => "null".to_string(),
    };
    let body = format!(
        r#"{{"prizeId":{prize_id},"client":"app","clientVersion":"{EXCHANGE_CLIENT_VERSION}","puzzleOffset":{offset_raw},"smsCode":"","deviceId":"{device_id}"}}"#
    );

    // `is_device_id = true` ⇒ 补 `isdeviceid: true`（实拍里这条**有**它，
    // 而取滑块那条**没有** —— 逐接口的入参，不是一刀切）。
    h5(true, HttpRequest::post(EXCHANGE_V3).text_body(body), "true").header("origin", MCLOUD_HOST)
}

// ══════════════════════════════ 清单模型 ══════════════════════════════

/// `probability2` 那个 JSON 串里**与限兑有关**的两个字段。
///
/// # 为什么单独一层（而不是把 `probability2` 当字符串交出去）
///
/// `limit == 4` 的**字段依据**就在这里：`monthGroupLimit` 列的是"每月只能兑
/// 其中一件"的那组 prizeId，配合 `limitMonthQuantity` 给出组额度。
///
/// ⚠⚠ **两个字段类型不一样**：`limitMonthQuantity` 是**数字**、
/// `monthGroupLimit` 是**字符串数组**。按"都是数字"读会让后者**静默归零**且不报错
/// —— 于是"同组三件全是 4"这件事当时解释不了。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MonthQuota {
    /// 这个组每月能兑几件。`None` = 上游没给。
    pub month_quantity: Option<i64>,
    /// 同组互斥的 prizeId（**字符串**形态）。
    ///
    /// `None` = 没这个键；`Some(vec![])` = 给了空数组。**两者不同**。
    pub group_prize_ids: Option<Vec<String>>,
}

/// 清单里的一件商品。
///
/// # 为什么只声明这些字段
///
/// 实拍一条有 **28** 个键，其中 `imageUrl` / `memo` / `oid` / `canEdit` /
/// `allowInside` / `areaCode` / `sort` / `updateTime` 是渲染与投放用的，本能力一个都不读。
/// 留下的每一条要么是**判据**，要么是判据的**字段依据**。
///
/// ⚠ `count` / `dailyRemainderCount` / `minRemainderCount` 是**时刻量**：
/// 会因**别的用户**在同一分钟里兑掉而变化（实拍 `839 → 837`）。
/// ⇒ 只能用来判"现在还有没有"，**不能**当个人流水。真正只反映"我这个账号"的是 `limit`。
///
/// 判据字段一律 `Option`："上游没给"与"给了 0"在**这一族**是两种答案。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExchangePrize {
    /// 下单要回传的那个数。⚠ 实拍是**数字**，不是字符串。
    pub prize_id: i64,
    pub name: Option<String>,
    pub group_id: Option<i64>,
    pub kind: Option<i64>,
    /// 兑换所需AI豆数。见 [`parse_price`] 的双键说明。
    pub price: Option<i64>,
    /// 本账号对**这一件**的可兑状态，见 [`LIMIT_AVAILABLE`] 那五档。
    pub limit: Option<i64>,
    pub count: Option<i64>,
    pub total_count: Option<i64>,
    pub daily_count: Option<i64>,
    pub daily_remainder_count: Option<i64>,
    pub min_remainder_count: Option<i64>,
    pub on_line: Option<i64>,
    pub next_time_status: Option<i64>,
    pub prize_section_status: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub market_name: Option<String>,
    pub quota: Option<MonthQuota>,
}

impl ExchangePrize {
    /// 报告"按上游给的状态，这一件现在是不是可兑"。
    ///
    /// 判据是 H5 那条按钮逻辑：`limit == 0` **且** `minRemainderCount != 0`。
    ///
    /// ⚠ 两个判据**都要有值**才敢返回 `true` —— 缺任一返回 `false`（"判不了"），
    /// 因为这一族 `false` 的代价只是"不发"，而 `true` 的代价是**真扣豆**。
    pub fn can_exchange_now(&self) -> bool {
        match (self.limit, self.min_remainder_count) {
            (Some(l), Some(m)) => l == LIMIT_AVAILABLE && m != 0,
            _ => false,
        }
    }
}

/// `exchangeList` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExchangeList {
    /// 键是 `groupId` 的**字符串**形态（顶层键在 JSON 里就是串）。
    ///
    /// ⚠⚠ 键**不连续**：实拍九个是 `0,1,2,5,7,8,9,11,15`。
    /// 按 `0..n` 遍历会静默漏掉一半且不报错 —— 所以遍历只能靠 [`Self::all`]。
    pub groups: Vec<(String, Vec<ExchangePrize>)>,
}

impl ExchangeList {
    /// 按 `groupId` **数值**升序（`"10"` 排在 `"2"` 后面，不是字典序）摊平。
    ///
    /// 存在的意义只有一个：界面与小结要**确定序**输出。JSON 对象没有顺序承诺，
    /// 不给这一份，同一份响应两次渲染会给出两份不同清单。
    pub fn all(&self) -> Vec<&ExchangePrize> {
        self.groups.iter().flat_map(|(_, v)| v.iter()).collect()
    }

    /// 按 `prizeId` 定位一件。
    pub fn find(&self, prize_id: i64) -> Option<&ExchangePrize> {
        self.all().into_iter().find(|p| p.prize_id == prize_id)
    }

    /// 按名字找（用于命令行/界面按名字挑）。
    ///
    /// 返回**全部**同名项：同名多件时不能凭猜挑一件下单。
    pub fn find_by_name(&self, name: &str) -> Vec<&ExchangePrize> {
        self.all()
            .into_iter()
            .filter(|p| p.name.as_deref() == Some(name))
            .collect()
    }
}

// ══════════════════════════════ 解析 ══════════════════════════════

fn opt_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
    })
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// 取价格，**两个键都试**。
///
/// ⚠⚠ 实拍里同一件商品有**两种拼法**：同一份响应里 `"POrder":50` 与
/// `"pOrder":50` 同时存在。只读一个键的写法会**静默**把一部分商品价格判成"没给"，
/// 于是"余额够不够"那道判断对那几件恒假。
///
/// 小写先试（H5 的消费点是 `t.pOrder`），`POrder` 兜底。
fn parse_price(obj: &Value) -> Option<i64> {
    opt_i64(obj, "pOrder").or_else(|| opt_i64(obj, "POrder"))
}

/// 解析 `probability2`（它是一个**JSON 串**，不是对象）。
///
/// # 键缺失 vs 空数组
///
/// `monthGroupLimit` 没这个键 ⇒ `None`；给了空数组 ⇒ `Some(vec![])`。
/// 这两种是**不同**的答案，所以只在键存在时才建 `Vec`。
///
/// # 为什么解不出来要**报错**（而不是当"没给"）
///
/// 这一层是 `limit == 4` 的**字段依据**：把"解不出"读成"上游没给"，
/// 会让"组名额用完了"看起来可兑。宁可在解析层响，也不要静默放过。
fn parse_month_quota(raw: &Value) -> Result<Option<MonthQuota>> {
    let Some(text) = raw.as_str() else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(text).map_err(|e| {
        Error::Parse(format!(
            "probability2 不是合法 JSON（实拍它是一个 JSON 串，形态变了）：{e}"
        ))
    })?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Parse("probability2 解出来不是对象".into()))?;

    let month_quantity = obj.get("limitMonthQuantity").and_then(Value::as_i64);

    let group_prize_ids = match obj.get("monthGroupLimit") {
        // ⚠ 实拍元素是**字符串**（`"251230051"`）。数字也容忍（上游换形态时不至于静默丢）。
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|it| {
                    it.as_str()
                        .map(str::to_string)
                        .or_else(|| it.as_i64().map(|n| n.to_string()))
                })
                .collect(),
        ),
        _ => None,
    };

    Ok(Some(MonthQuota {
        month_quantity,
        group_prize_ids,
    }))
}

fn parse_prize(raw: &Value) -> Result<ExchangePrize> {
    let prize_id = opt_i64(raw, "prizeId")
        .ok_or_else(|| Error::Parse("清单里有一件没有 prizeId（或它不是数字）".into()))?;
    Ok(ExchangePrize {
        prize_id,
        name: opt_str(raw, "prizeName"),
        group_id: opt_i64(raw, "groupId"),
        kind: opt_i64(raw, "type"),
        price: parse_price(raw),
        limit: opt_i64(raw, "limit"),
        count: opt_i64(raw, "count"),
        total_count: opt_i64(raw, "totalCount"),
        daily_count: opt_i64(raw, "dailyCount"),
        daily_remainder_count: opt_i64(raw, "dailyRemainderCount"),
        min_remainder_count: opt_i64(raw, "minRemainderCount"),
        on_line: opt_i64(raw, "onLine"),
        next_time_status: opt_i64(raw, "nextTimeStatus"),
        prize_section_status: opt_i64(raw, "prizeSectionStatus"),
        start_time: opt_str(raw, "startTime"),
        end_time: opt_str(raw, "endTime"),
        market_name: opt_str(raw, "marketName"),
        quota: parse_month_quota(raw.get("probability2").unwrap_or(&Value::Null))?,
    })
}

/// 解析 `exchangeList` 的响应。
///
/// 非 0 码走错误（只读查询，没有"非 0 但仍是数据"的档）。
///
/// ⚠ 顶层 `result` 必须是**对象、按 groupId 分组**。上游哪天改成数组，
/// 这里要**立刻响** —— 顺手兼容两种形状会把形状变化藏起来，
/// 表现是"清单忽然空了但每一枪都成功"。
pub fn parse_exchange_list(env: &SignInEnvelope) -> Result<ExchangeList> {
    let payload = env.ok_result("读取兑换清单")?;
    let obj = payload.as_object().ok_or_else(|| {
        Error::Parse("兑换清单的 result 不是按 groupId 分组的对象（上游改了形状）".into())
    })?;

    let mut groups: Vec<(String, Vec<ExchangePrize>)> = Vec::with_capacity(obj.len());
    for (key, value) in obj {
        let items = value.as_array().ok_or_else(|| {
            Error::Parse(format!(
                "兑换清单里 groupId={key} 那一组不是数组（上游改了形状）"
            ))
        })?;
        let mut parsed = Vec::with_capacity(items.len());
        for item in items {
            parsed.push(parse_prize(item)?);
        }
        groups.push((key.clone(), parsed));
    }

    // 按 groupId **数值**升序；非数字键排最后（字典序）。
    groups.sort_by(|a, b| match (a.0.parse::<i64>(), b.0.parse::<i64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.0.cmp(&b.0),
    });

    Ok(ExchangeList { groups })
}

// ══════════════════════════ 滑块题面 ══════════════════════════

/// 一次取题的**题面**：两张 PNG 原文，外加三个尺寸读数。
///
/// # 尺寸为什么留着（解算器并不需要它们）
///
/// 解算器吃 PNG 原文，尺寸从 PNG 自己的 IHDR 里读。这三个数**只**用来交叉自检：
/// 上游说底图 680 而图是别的尺寸时，说明题面与读数不同源。
///
/// ⚠ 三个尺寸在实拍里是**字符串**（`"680"` / `"400"` / `"96"`），不是数字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlidePuzzle {
    /// 底图 PNG 原文。
    pub picture_png: Vec<u8>,
    /// 拼图 PNG 原文。
    pub puzzle_png: Vec<u8>,
    /// 上游报的底图宽（字符串形态解析而来）。
    pub pic_width: Option<i64>,
    /// 上游报的底图高。
    pub pic_height: Option<i64>,
    /// 上游报的拼图宽。
    pub puzzle_width: Option<i64>,
}

/// 解析 `getSlide` 的响应。
///
/// # 刻意**不读** `puzzleLeft`
///
/// 实拍它**恒为 `null`**（4 条样本一致）—— 字段存在，但服务端不给答案。
/// 读出一个永远为 `None` 的字段，下一个读者会以为"有值的时候可以用"，
/// 而那一天没有样本支撑。所以这里连字段都不解析。
///
/// # 非 0 码在这里**是**错误
///
/// 与下单那条相反：取题失败没有可分流的档 —— 拿不到图就是解不了，只能不发下一枪。
pub fn parse_slide_puzzle(env: &SignInEnvelope) -> Result<SlidePuzzle> {
    let payload = env.ok_result("取滑块题面")?;

    let decode = |key: &str| -> Result<Vec<u8>> {
        let text = payload
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Parse(format!("滑块题面缺少 {key}（或它不是字符串）")))?;
        mcloud_crypto::b64_decode(text)
            .map_err(|e| Error::Parse(format!("滑块题面 {key} 不是合法 base64：{e}")))
    };

    Ok(SlidePuzzle {
        picture_png: decode("picture")?,
        puzzle_png: decode("puzzle")?,
        pic_width: opt_i64(payload, "picWidth"),
        pic_height: opt_i64(payload, "picHeight"),
        puzzle_width: opt_i64(payload, "puzzleWidth"),
    })
}

// ══════════════════════════════ 下单结果 ══════════════════════════════

/// `code == 0` 那一份 `result`（实拍 18 个键里取这八个）。
///
/// ⚠ 成功响应里**没有余额字段** —— "扣了多少豆"要靠两次 `infoV3` 的余额差。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExchangeOrder {
    /// 服务端新生成的订单号。
    ///
    /// ⚠ 它**不是幂等键**：每次下单都新产生。真正的幂等依据在服务端侧
    /// （`limit` 状态与 `2301` 那一档），不是请求里的任何字段。
    pub order_id: i64,
    pub prize_id: i64,
    /// 奖品名。
    ///
    /// ⚠ 参考实现把"`code == 0` 但缺 `prizeName`"**记为失败**。所以它是 `Option`，
    /// **不在这里补空串** —— 补了就把那一档抹平了。
    pub prize_name: Option<String>,
    /// `uorder` —— 这一笔**上游说**扣掉的AI豆数（实拍 `1800`）。
    ///
    /// ⚠ 只有它**不等于**"余额真的扣了这么多"：判据是两次 `infoV3` 的差。
    pub cost: Option<i64>,
    pub insert_time: Option<String>,
    pub expire_time: Option<String>,
    pub market_name: Option<String>,
    /// 实拍里它是 prizeId 的**字符串形态**，不是 URL。别按 URL 解析。
    pub link_url: Option<String>,
}

/// `exchangeV3` 的解析结果 —— **码是数据不是错误**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeResult {
    /// 业务码，见 [`CODE_SLIDER_FAILED`] 那一族。
    pub code: i64,
    /// 上游那句 `msg`（「验证失败」「重复兑奖」…）。
    ///
    /// 它**参与判定**：`420`/`421`/`415` 这几档要靠文案分开。
    /// ⚠ 只进小结与日志，**不**进 API 响应。
    pub message: String,
    /// 只在 `code == 0` 时非 `None`。
    pub order: Option<ExchangeOrder>,
}

impl ExchangeResult {
    pub fn is_ok(&self) -> bool {
        self.code == CODE_OK
    }
}

/// 解析 `exchangeV3` 的响应。**不走** [`SignInEnvelope::ensure_ok`]。
///
/// # 为什么这里不能"非 0 即错误"
///
/// `514` 可以重取滑块再来（代价为零），`2301` 再怎么试也不会成交，
/// `415` 说明这件今天别再试、明天还有 —— 三档的处置完全不同，
/// 而它们都是**正常业务响应**。
///
/// # `msg` 缺失怎么办
///
/// 实拍失败响应只有 `{"code":514,"msg":"验证失败"}`（**没有 result 键**）。
/// `msg` 缺了不报错 —— 码才是判据，文案只是给人看的那一句。
pub fn parse_exchange_result(env: &SignInEnvelope) -> Result<ExchangeResult> {
    let code = env.code;

    // `SignInEnvelope::parse` 把"没有 code / code 不是数字"落成 `-1`。
    // 在这条链上必须**当成错误**：判不了成交没成交，而这一步花的是真豆。
    // 静默往下走会让一个畸形响应看起来像"业务码 -1 的失败"，
    // 于是调用方报"兑换失败"，而它真正该说的是"我们读不懂这个响应"。
    if code < 0 {
        return Err(Error::Parse(format!(
            "兑换响应没有可解析的 code（读成 {code}）—— 判不了是否成交，需人工核对"
        )));
    }

    // 成功时 `result` 必须存在且能取出 `oid` / `prizeId`。
    let order = if code == CODE_OK {
        let result = env
            .result
            .as_ref()
            .ok_or_else(|| Error::Parse("兑换成功但响应里没有 result".into()))?;
        let order_id = opt_i64(result, "oid")
            .ok_or_else(|| Error::Parse("兑换成功的 result 里没有 oid".into()))?;
        let prize_id = opt_i64(result, "prizeId")
            .ok_or_else(|| Error::Parse("兑换成功的 result 里没有 prizeId".into()))?;
        Some(ExchangeOrder {
            order_id,
            prize_id,
            prize_name: opt_str(result, "prizeName"),
            cost: opt_i64(result, "uorder"),
            insert_time: opt_str(result, "insertTime"),
            expire_time: opt_str(result, "expireTime"),
            market_name: opt_str(result, "marketName"),
            link_url: opt_str(result, "linkUrl"),
        })
    } else {
        None
    };

    Ok(ExchangeResult {
        code,
        message: env.message.clone(),
        order,
    })
}

// ══════════════════════════ 组字典（`prizeGroup.json`）══════════════════════════

/// 组字典里的一组。**只取界面要用的四样**（上游还给 `media` / `list`，本能力一个不读）。
///
/// # 为什么值得单独读一枪
///
/// `exchangeList` **只有商品**，而"一组商品共享状态"这件事是**组**的属性：
/// 同组每月共用一个名额（见 [`MonthQuota`] 与 [`LIMIT_GROUP_EXCHANGED`]）、
/// 同组共享同一个活动时间窗。组名与那条限兑规则都在这里，
/// 实拍证据见 `../mcloud-capture/docs/capture/app-exchange-samples.txt`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeGroup {
    /// 组键。⚠ 上游**类型混用**：2026-10-01 实拍同一份里既有 `"groupId":1`（数字）
    /// 也有 `"groupId":"0"`（字符串）⇒ 这里一律归一成字符串，
    /// 与清单那边"顶层键在 JSON 里就是串"的口径对齐（两边要能对上）。
    pub group_id: String,
    /// 组名（实拍 `"音乐类会员"`）—— 界面分区标题就念它，**别自己编品类名**。
    pub title: Option<String>,
    /// 该组的限兑规则，**上游原话**（实拍 `"(月卡每月多选一限兑)"`、`"（每月多选一限兑）"`）。
    ///
    /// ⚠ 全角/半角括号混用是上游的写法，别"顺手规范化" —— 那是它给用户看的文案。
    pub sub_title: Option<String>,
    pub prize_type: Option<i64>,
    pub icon: Option<String>,
}

/// 从 `marketName` 推 `prizeGroup.json` 里的**活动文件夹名**。
///
/// # 只有一个观测点，所以这是推导、不是实拍
///
/// 2026-10-01 实拍：清单里 `marketName = "sign_in_3_ex"`，而 H5 取组字典的路径是
/// `/ycloud/signin/public/text/sign_in_3/prizeGroup.json` ⇒ 去掉尾部 `_ex` 正好对上。
///
/// ⚠ 活动换代（`sign_in_4`）时这条推导对不对**没人证过**（`sign_in_4` 现在回 404，
/// 只说明"现在没有"）。所以推不出来就回 `None`，由调用方**降级**（干脆不发这一枪），
/// 而不是硬编一个文件夹名 —— 那会在换代之后静默给出**另一场活动的组名**。
pub fn prize_group_folder(market_name: &str) -> Option<String> {
    let name = market_name.trim();
    let base = name.strip_suffix("_ex").unwrap_or(name);
    (!base.is_empty()).then(|| base.to_string())
}

/// `GET /ycloud/signin/public/text/<活动>/prizeGroup.json` —— 组字典。**静态、只读、免鉴权。**
///
/// 回 `None` = 从 `market_name` 推不出文件夹名（见 [`prize_group_folder`]）。
pub fn prize_group_request(market_name: &str) -> Option<HttpRequest> {
    let folder = prize_group_folder(market_name)?;
    let url = format!("{MCLOUD_HOST}/ycloud/signin/public/text/{folder}/prizeGroup.json");
    // 与 `exchangeList` 同形态（`h5(false, ..)`、`"true"`）：这一条上游是静态文案，
    // 2026-10-01 实测**不带任何凭据**也回 200 —— 走同一条装饰链只为省一条分支。
    Some(h5(false, HttpRequest::get(url), "true"))
}

/// 解析组字典。
///
/// # 为什么**空体必须报错**，不能读成"没有组"
///
/// 2026-10-01 实拍：同族路径换成 `sign_in_3_ex` 时，上游回 **HTTP 200 但 0 字节**
/// （`sign_in_4` 回 404）。只看状态码的写法会把"这个路径没内容"读成
/// "上游说没有组" ⇒ 界面表现为组名全空，而且**一声不响**。宁可让调用方拿到 `Err`，
/// 由它降级并把原因说出来。
///
/// # `title` / `subTitle` 的空串归 `None`
///
/// 实拍里好几组的 `subTitle` 是 `""`（给了键、值是空串）。界面上一句空规则没有意义，
/// 所以在这里归一。⚠ 这一族**不是判据字段**，归一不改变任何"能不能兑"的结论 ——
/// 判据那一族的"没给 vs 给了 0"仍然严格区分（见 [`parse_month_quota`]）。
pub fn parse_prize_groups(text: &str) -> Result<Vec<ExchangeGroup>> {
    if text.trim().is_empty() {
        return Err(Error::Parse(
            "prizeGroup.json 是空体（实拍：路径不对时上游回 200 + 0 字节）".into(),
        ));
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|e| Error::Parse(format!("prizeGroup.json 不是合法 JSON（实拍是数组）：{e}")))?;
    let items = value
        .as_array()
        .ok_or_else(|| Error::Parse("prizeGroup.json 顶层不是数组（实拍是数组）".into()))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        // 没有组键的一组对界面毫无用处（而且它一定不是实拍形态）⇒ 响，别静默跳过。
        let group_id = group_id_text(item).ok_or_else(|| {
            Error::Parse("prizeGroup.json 里有一组没有 groupId（实拍每组都有）".into())
        })?;
        out.push(ExchangeGroup {
            group_id,
            title: nonempty(opt_str(item, "title")),
            sub_title: nonempty(opt_str(item, "subTitle")),
            prize_type: opt_i64(item, "prizeType"),
            icon: nonempty(opt_str(item, "icon")),
        });
    }
    Ok(out)
}

/// 取组键：**字符串原样、数字转字符串**（上游类型混用，见 [`ExchangeGroup::group_id`]）。
fn group_id_text(item: &Value) -> Option<String> {
    match item.get("groupId") {
        Some(Value::String(s)) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        // ⚠ 走 [`opt_i64`] 而不是自己 `as_i64`：它也吃字符串形态的数字，
        // 而上面那一支已经把字符串留下来了，所以这里只会拿到数字。
        _ => opt_i64(item, "groupId").map(|n| n.to_string()),
    }
}

/// 空串 / 纯空白归 `None`（见 [`parse_prize_groups`] 的第二段）。
fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

#[cfg(test)]
mod tests;
