//! 趣玩 AI（「全网趣玩AI赢好礼」）抽奖族 —— **协议层**。
//!
//! 两条请求，就这两条在链上：
//!
//! ```text
//! GET  /ycloud/funai/api/page/getmodule   读判据（drawTimes / drawsUsed / 活动起止）
//! POST /ycloud/funai/api/page/lottery     body 只有 {"moduleId":"…"}，值取自上一枪
//! ```
//!
//! 判定（该不该抽）在 `mcloud_tasks::fun_ai`。
//!
//! # 只要这两条 —— 其余十口为什么**不**补
//!
//! 实拍（`mcloud` 的 `../mcloud/test/fixtures/http/玩-ai-抽支付宝红包.json`，按文件顺序 0…11）
//! 里出现过六类请求，门户页的接口名单一共有十二口。没实现的那些**不是忘了**：
//!
//! * `timed/getmodule`：`result` 是模块数组，每一项的 `drawTimes` / `drawsUsed` /
//!   `todayGetLotteryCount` **全是 `null`** ⇒ 它回答不了"还剩几次"。
//!   给它编一个解析函数，下一步就是有人拿 `null` 当 `0` 去做判据。
//!   抽次数一律问 `page/getmodule`。
//! * 规则文本（`commonMarketconfig/getByMarketName`）：里面那句「每日最多可通过
//!   **完成任务**获得 5 次」说的是**做任务能挣到几次**，不是"每天最多能抽几次"。
//! * `invite/getcode`、广告位与动画素材：本链不需要。
//! * ⭐ **"刮刮乐"不是 `page/lottery`**，它是另一口 `POST timed/lottery`。
//!   本文件抽的是**转盘**那一口；两者**互不改写账本**（实拍：真刮一次之后
//!   页面三个计数逐字不动）⇒ 别把刮奖接到 [`funai_lottery_request`] 上。
//!
//! # 票：走**公共来源**（临时态）
//!
//! 这一页的票按签到页那条建会话配方走。实拍里**没有**换出该页 jwttoken 的那一步
//! （也没有该页的预热）⇒ 该页的特有取法无从得知，按退路走。
//! 抓到特有取法时**主动更正**调用点并改掉这段说明，别留"注释说公共、代码走特有"。
//!
//! # 请求头与实拍的三处已知差异（都不带判断作用，但要说清）
//!
//! * `x-tingyun`：实拍每条都带（前端拨测 SDK 注入）。本仓**所有** H5 接口都不发它，
//!   上游是否据此判什么 ⇒ **未测试**。
//! * `activityid`：实拍这一页带的是 `sign_in_3` —— 因为壳层把
//!   `localStorage.marketName` **原样**写进这个头，它是**会话侧残留**，
//!   不是这一页自己选的档位 ⇒ 本仓按公共值发是**与前端行为一致**的写法。
//!   ⚠ 别把它按页面名硬编成 `National_playAI`，前端也不带。
//! * `deviceid`：实拍带，但值已脱敏（连形态都核对不了）⇒ 做成**可选参数**，
//!   缺省不发。服务端到底要不要它**未验证**，留着可选才能一次 A/B 试出来。

use serde_json::Value;

use mcloud_core::{parse_beijing_time, HttpRequest, Result};

use crate::app_signin::{h5, SignInEnvelope};
use crate::endpoints::MCLOUD_HOST;

/// 这一活动的市场名 —— 实拍 `page/lottery` 响应里的 `marketName`。
///
/// ⚠ 它**不进**本文件两条请求的参数：`page/lottery` 的 body 只有 `moduleId` 一个键
/// （实拍 `content-length: 23`）。留这个常量是为了核对响应、以及将来接规则文本时有出处。
pub const FUNAI_MARKET_NAME: &str = "National_playAI";

/// `startTime` / `endTime` 的**实拍形态**（逐字）：
///
/// ```text
/// startTime "2025-05-28T00:00:00"
/// endTime   "2026-12-31T23:59:59"
/// ```
///
/// ⇒ **不带时区、不带偏移**的本地墙钟串（既不是 RFC 3339，也不是 epoch）。
/// 按 [`mcloud_core::parse_beijing_time`] 以东八区解释 —— 详见那里的说明
/// （按 UTC 解会把边界整体推后 8 小时）。
pub const FUNAI_ACTIVITY_TIME_LAYOUT: &str = "%Y-%m-%dT%H:%M:%S";

const FUNAI_MODULE_PATH: &str = "/ycloud/funai/api/page/getmodule";
const FUNAI_LOTTERY_PATH: &str = "/ycloud/funai/api/page/lottery";

// ══════════════════════════ 请求 ══════════════════════════

/// `GET /ycloud/funai/api/page/getmodule` —— 读判据。**只读**。
///
/// 实拍（顺序 2、7）：**没有查询串**、**请求体是空的**、**没有 `isdeviceid`**、
/// **没有 `content-type`** —— 裸 GET 出去就是一个 body 都没有、一个
/// content-type 都没有（`json_body` 才会带上那两样）。
pub fn funai_module_request(device_id: Option<&str>) -> HttpRequest {
    let req = h5(
        false,
        HttpRequest::get(format!("{MCLOUD_HOST}{FUNAI_MODULE_PATH}")),
        "true",
    );
    with_optional_device_id(req, device_id)
}

/// `POST /ycloud/funai/api/page/lottery` —— **抽一次**（写请求）。
///
/// # 请求体逐字（实拍顺序 11，`content-length: 23`）
///
/// ```text
/// {"moduleId":"1-250730"}
/// ```
///
/// 只有 `moduleId` **一个键**：没有 `marketName`、没有 `client`、没有查询串。
///
/// # ⚠ 门槛**不在这里判**
///
/// 该不该发由调用方看 [`FunaiModule`] 的计数决定（`mcloud_tasks::fun_ai::plan_funai`）。
/// 本层只保证"发出去、解析回来"。
///
/// `module_id` **必须**来自**同一次** `page/getmodule`：写死了就会在换期时
/// 打到过期模块上（同一份抓包的 `timed/getmodule` 里同期另挂着三个别的 id）。
pub fn funai_lottery_request(module_id: &str, device_id: Option<&str>) -> HttpRequest {
    let req = h5(
        false,
        HttpRequest::post(format!("{MCLOUD_HOST}{FUNAI_LOTTERY_PATH}"))
            .json_body(&serde_json::json!({ "moduleId": module_id })),
        "true",
    );
    with_optional_device_id(req, device_id)
}

/// 把可选的 `deviceid` 头接上。`None` ⇒ **不发**（理由见文件头第三条）。
fn with_optional_device_id(req: HttpRequest, device_id: Option<&str>) -> HttpRequest {
    match device_id {
        Some(id) => req.header("deviceid", id),
        None => req,
    }
}

// ══════════════════════════ 解析 ══════════════════════════

/// `page/getmodule` 里本链要用到的那几个字段。
///
/// # 为什么只声明这几个
///
/// 实拍的 `result` 有 15 个键（另有标题、图片、分享链接与 16 项任务列表）。
/// 本层的问题是"该不该抽、拿什么 id 去抽"，任务列表与图片素材答不了它 ——
/// 声明出来只会多一处会漂移的字段表。
///
/// # ⚠ 为什么不读 `isDraw` / `drawTime` / `drawResult`
///
/// 页面族里这三个**都是 `null`**（归档顺序 2 与 7 两次都一样），而且 2026-09-25
/// 在**同一页上真抽过四次转盘**之后回读，三个键**仍然是 `null`** ——
/// ⇒ "这一页今天抽过没有"**不能**由它答。拿"望文生义的名字"当判据，
/// 等于把猜测变成断言（且它错的时候不报错，只是抽或不抽）。
///
/// 判据因此只用三个计数。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunaiModule {
    /// `result.moduleId` —— 当期模块 id，实拍 `1-250730`。
    ///
    /// ⚠ **必填**（缺了或为空就发不出抽奖请求），且抽奖请求体里的值必须来自这里。
    pub module_id: String,
    /// `result.drawTimes` —— 实拍 `0`~`3` 各档都见过。
    ///
    /// ⭐ 语义**同日已收口**（2026-09-25，11 个请求时刻）：恒等式
    /// `todayGetLotteryCount == drawTimes + drawsUsed` 一次没破 ⇒
    /// 这一位是**今天还剩几次可抽**，不是"总共能抽几次"。
    ///
    /// ⚠ **没收口的两件事**：① 跨天清不清零（只有同日读数）；
    /// ② 恒等式只在 `page/getmodule` 那一族成立。
    pub draw_times: Option<i64>,
    /// `result.drawsUsed` —— 归档 `null`；同日读到 `0`→`4`。
    pub draws_used: Option<i64>,
    /// `result.todayGetLotteryCount` —— 归档 `null`；同日 `0 → 5`。
    ///
    /// 是上面那个恒等式的第三项 ⇒ 它是"今天一共挣到几次"（= 已花 + 还剩）。
    ///
    /// ⚠ 本链的判定**不用它** —— 用了等于把判据建在一个只做**加法**的字段上：
    /// 它抽了不动，所以它**不是**"还剩几次"。
    pub today_get_lottery_count: Option<i64>,
    /// `result.startTime` / `endTime` —— 活动期，**epoch 毫秒**。
    ///
    /// `None` = 上游没给**或**形态不认识 ⇒ 判不了（**不是**"没有边界"）。
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
}

/// 解析 `page/getmodule`。
///
/// 只有 `module_id` 是必填，其余全是指针 —— 计数缺了要能落成"判不了"那一档，
/// 而**不是**被默认值伪造成"0 次 / 已用过"。
pub fn parse_funai_module(env: &SignInEnvelope) -> Result<FunaiModule> {
    let payload = env.ok_result("读取趣玩AI活动信息")?;
    let module_id = match payload.get("moduleId").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        Some(_) => {
            // 空串单独拦一句：带着空 id 发出去的抽奖请求**一定**是另一个请求
            // （实拍那个键值从不为空）。
            return Err(mcloud_core::Error::Parse(
                "趣玩AI 响应字段 `moduleId` 为空，无法构造抽奖请求".into(),
            ));
        }
        None => {
            return Err(mcloud_core::Error::Parse(
                "趣玩AI 响应缺少 `result.moduleId`（构造抽奖请求的必填项）".into(),
            ));
        }
    };
    Ok(FunaiModule {
        module_id,
        draw_times: opt_int_at(payload, "drawTimes"),
        draws_used: opt_int_at(payload, "drawsUsed"),
        today_get_lottery_count: opt_int_at(payload, "todayGetLotteryCount"),
        start_time: time_at(payload, "startTime"),
        end_time: time_at(payload, "endTime"),
    })
}

/// `page/lottery` 的解析结果 —— 这一次抽出的是什么。
///
/// # 实拍（顺序 11，`code: 0`）
///
/// ```text
/// {"id":3228298,"isWin":1,"lotteryDate":"20260629",
///  "lotteryTime":"2026-06-29T20:13:47.890625379","marketName":"National_playAI",
///  "moduleId":"1-250730","prizeId":260107009,"prizeName":"30云朵",
///  "prizeType":2,"prizeUrl":"…png","type":"1"}
/// ```
///
/// **全部字段是可选**：这条**成功样本只有两档**（中奖 `isWin:1`、未中奖 `isWin:0`，
/// 两档都是 `code:0`），没有任何"非 0 码"的实拍 ⇒ 没有一个字段是"缺了就说明响应坏了"
/// 的依据。硬要求某个字段，表现是把一次真成功的抽奖报成解析错误。
///
/// ⚠ 实拍里还有一个 `phone` 字段（归档时已脱敏）：它**不带进来**，
/// 既不进结构也不进夹具 —— 凭据/账号标识纪律。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunaiLottery {
    /// `result.prizeName` —— 抽出的是什么，实拍「30云朵」。
    ///
    /// `None` 的情况：上游没给这一格，**或者**就是没中奖那一档（`prizeName` 也是 `null`）。
    /// ⇒ 读到 `None` **优先**按"没中奖"理解；但**反过来不成立**（"中了却没名"零样本）。
    /// 判"中没中"要看 [`Self::is_win`]。
    ///
    /// ⚠ **别拿它做长期比对的键**：`prizeName` **不是快照** —— 同一条中奖记录
    /// （同 `prizeId`）在改名前后回的是「30云朵」与「30云盘专属AI豆」
    /// ⇒ 要长期比对只能认 [`Self::prize_id`]。
    pub prize_name: Option<String>,
    /// `result.prizeId`（实拍 `260107009`）—— 这一族里**唯一适合长期比对**的键。
    pub prize_id: Option<i64>,
    /// `result.prizeType`（实拍 `2`）。
    ///
    /// ⚠ 这套类型号的取值表**没有成文**，别拿它当"现金/云朵"的分类用。
    /// ⚠⚠ 而且**没中奖那一条也带 `prizeType`**（实拍 `1`）⇒ 它**不是**"有没有奖品"的信号。
    pub prize_type: Option<i64>,
    /// `result.isWin` —— 实拍**两档都有**：`1`（中，带 `prize*`）与 `0`（没中，`prize*` 全 `null`）。
    ///
    /// ⇒ "0 = 没中奖"这一读法**不再是望文生义**。
    ///
    /// ⚠ 但本链**仍不据此分支**：不是"没样本"，而是**那一档不算失败** ——
    /// `code:0` + `isWin:0` 是一次正常抽完，拿它走失败支会把成功写红。
    pub is_win: Option<i64>,
    /// `result.moduleId` —— 上游认下的那个 id，只用于**排障对账**。
    ///
    /// 本链**不**据此分支 —— "上游回了另一个 moduleId"这种情况一个样本都没有。
    pub module_id: Option<String>,
    /// `result.marketName` —— 实拍与 [`FUNAI_MARKET_NAME`] 一致，只作核对用。
    pub market_name: Option<String>,
}

/// 解析 `page/lottery`。
///
/// `code` 非 0 一律按失败交回（**包括**"没有次数"那一类）——
/// 本口 `page/lottery` 的非 0 码**一个样本都没有**，所以这里不猜具体码值。
///
/// ⚠ 别与**姊妹口**搞混：`timed/lottery`（刮刮乐，本仓零实现）已有三档 `code:500`
/// 原文 —— 那是**那一口**的话，**不许**照搬进本口的失败文案。
///
/// 没有 `result` **不判失败**：成败已由 `code` 说过一次，这里再判一次会把
/// 一句上游的原话换成一句我们编的解析错误。
pub fn parse_funai_lottery(env: &SignInEnvelope) -> Result<FunaiLottery> {
    env.ensure_ok("趣玩AI抽奖")?;
    let obj = match env.result.as_ref() {
        Some(v @ Value::Object(_)) => v,
        _ => return Ok(FunaiLottery::default()),
    };
    Ok(FunaiLottery {
        prize_name: obj
            .get("prizeName")
            .and_then(Value::as_str)
            .map(str::to_string),
        prize_id: opt_int_at(obj, "prizeId"),
        prize_type: opt_int_at(obj, "prizeType"),
        is_win: opt_int_at(obj, "isWin"),
        module_id: obj
            .get("moduleId")
            .and_then(Value::as_str)
            .map(str::to_string),
        market_name: obj
            .get("marketName")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

// ══════════════════════════ 小工具 ══════════════════════════

/// 取一个**可选**整数：缺键与"给了非数字"都回 `None`（不伪造成 0）。
fn opt_int_at(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        // 上游有的字段是字符串形式的数字
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// 取活动起止时刻（**epoch 毫秒**）。
///
/// ⚠ **形态不认识一律给 `None`，不报错**：这里报错会把一次**能抽**的机会
/// 变成解析失败，而"上游换了时间格式"真正该表现为"判不了 ⇒ 不发请求"
/// （那句结论是人能看懂的）。非字符串（含 epoch 数字）也算形态不认识。
fn time_at(v: &Value, key: &str) -> Option<i64> {
    let raw = v.get(key)?.as_str()?;
    parse_beijing_time(raw, FUNAI_ACTIVITY_TIME_LAYOUT)
}

#[cfg(test)]
mod tests;
