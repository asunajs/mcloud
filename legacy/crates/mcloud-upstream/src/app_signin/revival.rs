use super::*;
use crate::Envelope;
use crate::base;
use mcloud_core::{Error, HttpRequest};
use serde_json::{Value, json};

/// `POST /ycloud/signin/page/receiveRevivalReward` —— **用复活卡 / 回馈卡**。
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// receiveRevivalReward: function(e){
///   return c["a"].post("/ycloud/signin/page/receiveRevivalReward", JSON.stringify(e),
///     {headers:{showLoading:false, "Content-Type":"application/json;charset=UTF-8"}})
/// }
/// ```
///
/// 调用点组体是 `{}`（`z["a"].receiveRevivalReward({})`）—— 所以 body 是 `"{}"`，
/// 这也是老抓包里的样子（`content-length: 2`）。**没有查询参数、没有别的字段。**
///
/// 注意 `showLoading: false`：同页其余接口几乎都是 `true`，只有它是 `false`
/// （H5 要自己画那张卡片的弹窗，不想要全局 loading 遮住）。
///
/// # `device_id`：两个证据不一致，两个分支都留着
///
/// | 证据 | 结论 |
/// | --- | --- |
/// | H5 源码（iOS 12.6.0）| 头里**没有** `isDeviceId` ⇒ 拦截器不注入 |
/// | 老抓包（Android WebView）| 带一个 `deviceid` **头**（值被脱敏，看不出是不是同一个 blob）|
///
/// 所以做成 `Option<&str>`：`None` = 照 H5 定义（缺省，推荐）；
/// `Some(v)` = 照老抓包补一个 `deviceid` 头。**哪个是服务端真正要求的，未验证** ——
/// 这也正是留成可选参数的意义：真出问题时能一次 A/B 试出来（`receiveV3` 就是这么定的）。
///
/// ⚠ 这里**不加** `isdeviceid` 头（老抓包里也没有）—— 别照 `receive_v3_request` 抄。
pub fn receive_revival_reward_request(
    jwt: &str,
    referer: &str,
    device_id: Option<&str>,
) -> Result<HttpRequest, Error> {
    // 空对象体：不是 `None`。`json_body` 会给出 `"{}"`，与抓包的 `content-length: 2` 对齐。
    let request = HttpRequest::post(format!(
        "{}/ycloud/signin/page/receiveRevivalReward",
        base::MCLOUD
    ))
    .json_body(&json!({}))?;
    let request = with_h5_headers(request, referer, Some(jwt), "false");
    Ok(match device_id {
        Some(id) => request.header("deviceid", id),
        None => request,
    })
}

/// `GET /ycloud/signin/page/receiveTaskExpansion?acceptDate=<acceptDate>` —— **延期领取**。
///
/// # 它在"领取"里的位置（这是 H5 的逻辑，不是可有可无的一支）
///
/// `infoV3` 的 `receiveList[]` 按 `cloudType` 分流，逐字见 [`ReceiveItem::cloud_type`]：
///
/// | `cloudType` | 含义 | 动作 |
/// | --- | --- | --- |
/// | `1` | 延期领取 | **本函数**（要 `acceptDate`） |
/// | `2` | 下月可领取 | 什么都不发 |
/// | 其它 | 可领取 | [`receive_v3_request`] |
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// receiveTaskExpansion: function(e){
///   return c["a"].get("/ycloud/signin/page/receiveTaskExpansion",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：A("2", t.acceptDate, e) → z["a"].receiveTaskExpansion({acceptDate: t})
/// ```
///
/// 即**查询参数只有一个 `acceptDate`**、**没有 body**、**没有 `isDeviceId`**
/// （H5 没标 ⇒ 拦截器不注入）。别照 [`receive_v3_request`] 抄那套。
///
/// # 错误码（H5 的处理分支，留着备查）
///
/// `0` 成功 / `1302` 「开启自动备份按钮才可领取」/ `500` 「AI豆已过领取有效期」。
///
/// ⚠ `acceptDate` 来自 `receiveList[]` 里**同一条**的 `acceptDate` —— 不是自己编的日期。
///
/// # 为什么是 `Option<&str>`（而不是必填 `&str`，也不是空串）
///
/// 这条是**被实拍推翻过两次**的地方，写清楚免得再改回去：
///
/// - H5 的调用点逐字是 `A("2", t.acceptDate, e)` → `receiveTaskExpansion({acceptDate: t})`，
///   参数取自**同一条条目**。
/// - 而**实测两条账号的 `receiveList` 里都没有 `acceptDate` 这个字段**（`--raw` 的原始
///   响应里只有 `cloudNum` / `cloudType`）⇒ 那个值是 `undefined`。
/// - axios 的默认 `paramsSerializer` 对 `null` / `undefined` 是**整个键丢掉**
///   （不是发 `acceptDate=`）⇒ 真实客户端发出去的是**裸 GET，一个查询参数都没有**。
///
/// 所以：
/// `None` → 不带查询串（**与真实客户端一致，是缺省路径**）；
/// `Some(v)` → `?acceptDate=<百分号编码后的 v>`。
///
/// ⚠ 别把 `None` 写成 `Some("")` —— 那会发出 `?acceptDate=`，是**另一个请求**
/// （多一个空参数）。服务端怎么处理没验过，没必要去踩。
pub fn receive_task_expansion_request(
    jwt: &str,
    referer: &str,
    accept_date: Option<&str>,
) -> HttpRequest {
    let url = match accept_date {
        Some(value) => format!(
            "{}/ycloud/signin/page/receiveTaskExpansion?acceptDate={}",
            base::MCLOUD,
            percent_encode(value)
        ),
        None => format!("{}/ycloud/signin/page/receiveTaskExpansion", base::MCLOUD),
    };
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `GET /ycloud/signin/page/multiple` —— **签到翻倍**（旧项目叫 `singInMultiple`）。
///
/// # 它领什么
///
/// 「自动备份」每月给**一次**翻倍机会：本月开着备份、且这次机会还没用掉时，
/// 领一次能把**今日签到奖励**翻 2–50 倍。H5 原话逐字：「备份成功，获得1次AI豆
/// 翻倍机会今日签到奖励可翻2-50倍！」，领到手后弹「额外获得 {cloudCount} 豆，
/// 自动备份保持开启，任务奖励翻倍!」。
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// cloudMultiple: function(e){
///   return c["a"].get("/ycloud/signin/page/multiple",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：z["a"].cloudMultiple({})        ← **空对象**
/// ```
///
/// ⇒ 与 [`receive_task_expansion_request`] 同一个坑：`params` 是空对象时，
/// axios 的默认序列化器**一个查询参数都不产生**。所以这里发的是**裸 GET**，
/// 不是 `?`。
///
/// # 门槛**必须自己判**，别指望上游替你把关
///
/// 旧项目 `mcloud-sign` 的判据逐字（`tasks/sign/task.ts`）：
///
/// ```javascript
/// if (curMonthBackup && !curMonthBackupSignAccept) { await executeSignInMultiple(ctx); }
/// ```
///
/// 两个值都在 `infoV3` 里（[`SignInStatus::cur_month_backup`] /
/// [`SignInStatus::cur_month_backup_sign_accept`]）。**两个都成立才去领** ——
/// 每月只有一次机会，条件不成立还发，是**白白消耗**。
/// 判据的可执行版本在 `mcloud_app::signin::plan_multiple`。
pub fn multiple_request(jwt: &str, referer: &str) -> HttpRequest {
    let url = format!("{}/ycloud/signin/page/multiple", base::MCLOUD);
    // `showLoading: true`（H5 逐字）—— 与 `receiveTaskExpansion` 同档。
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `receiveRevivalReward` 的解析结果 —— **用掉一张卡，换回一批云朵**。
///
/// 实拍响应（`test/fixtures/http/复活.json`）：
///
/// ```json
/// {"code":0,"msg":"success",
///  "result":{"cardType":1,"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevivalReward {
    /// `1` = **复活卡**（随机复活上一年失效的云朵），`2` = **回馈卡**。
    ///
    /// H5 用 `1 == cardType ? "复活卡弹窗" : "回馈卡弹窗"` 分流，
    /// 两张卡连累计数的**文案单位都不同**（复活卡叫"累计复活AI豆"，回馈卡叫"累计获得奖励"）。
    /// 所以它是判据不是装饰 —— 缺了就没法说清"这次用掉的是什么卡"。
    pub card_type: i64,
    /// 本次随机复活到的云朵数。**这就是"这次进账多少"**（与 [`ReceiveResult::receive`] 同义）。
    pub reward_clouds: i64,
    /// 累计数（复活卡口径下是"累计复活AI豆"）。
    pub total_clouds: i64,
    /// 只影响一句文案（"下月再来可获得AI豆回馈卡！" vs "下月再来可继续获得AI豆复活卡！"）。
    ///
    /// 所以它是**附属字段**：服务端不给就给 `0`，不因此判整条响应失败 ——
    /// 判据是"缺了它还能不能回答本函数的问题"，它答不了任何问题，只是换个措辞。
    /// （与"主判据缺字段必须报错"成对，见 [`parse_sign_in_status`] 里 `receiveList` 的处理。）
    pub little_first_time: i64,
}

/// 解析 `receiveRevivalReward` 的响应。
///
/// ⚠ 这里**不**把 `total_clouds` 接进 `record_cloud` —— 它到底是不是"云朵余额"口径
/// **没有验证**：H5 文案说的是"复活卡把失效云朵**转换成 AI豆**"，
/// 累计文案也是"累计复活**AI豆**"。规划 §3.4 的来源③当初假设它是云朵余额，
/// 两者可能不是同一个池子。**等有 `hc1t` / `cloudStatus` 的实拍再定**，
/// 现在接进去只会把两个口径混进同一条曲线。
pub fn parse_revival_reward(envelope: &Envelope) -> Result<RevivalReward, Error> {
    let payload = ensure_ok(envelope)?;
    let payload =
        payload.ok_or_else(|| Error::Parse("receiveRevivalReward 响应没有 result".to_string()))?;
    Ok(RevivalReward {
        card_type: require_i64(payload, "cardType")?,
        reward_clouds: require_i64(payload, "rewardClouds")?,
        total_clouds: require_i64(payload, "totalClouds")?,
        // 附属字段：缺了给 0（见字段文档）。
        little_first_time: payload
            .get("littleFirstTime")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

/// `receiveTaskExpansion` 的解析结果 —— **月度任务奖励**（`cloudType == 1` 那条线）。
///
/// # 这条线是什么（两个来源对上之后的说法）
///
/// 「本月签到/做任务攒记录 → **下个月**结算 → 下个月来领」，且**要开着自动备份**。
/// 所以它在 `infoV3` 的 `receiveList[]` 里有两个状态，同一个特性的两个时点：
///
/// | `cloudType` | 时点 | 老路径的字段 | 动作 |
/// | --- | --- | --- | --- |
/// | `1` | **已到期、现在能领**（上月攒的那批）| `curMonthTaskRecordCount > 0` | **本接口** |
/// | `2` | 还没到期（本月攒的，下月才能领）| `nextMonthTaskRecordCount` | 只提示，不发请求 |
///
/// 旧项目 `mcloud-sign` 的 `_taskExpansion` 逐字是这么写的：
/// `if (curMonthTaskRecordCount > 0) { … receiveTaskExpansion(acceptDate) }`，
/// 然后 `if (nextMonthTaskRecordCount) { logger.info("下月可领取…") }` —— **一领一报**，
/// 与新 H5 的 `1 → 领` / `2 → toast("下月可领取")` 完全同构。
///
/// # 为什么字段是 `Option`
///
/// ⚠ **本仓库没有任何 `receiveTaskExpansion` 的实拍响应。** 唯一的字段线索来自旧项目
/// `mcloud-sign` 的调用点：`receiveData?.result?.cloudCount`，以及 H5 的错误分支
/// （`1302` 未开启备份、`500` 已过领取有效期）。
///
/// 所以这里**不**用 [`require_i64`]：那个约定是"缺了主判据就判整条响应失败"，
/// 而这条接口的主判据是**成败**（由 `code` 决定，`ensure_ok` 已经管了），
/// `cloudCount` 只回答"领到多少"。没有实拍还硬要求字段，等于把猜测变成断言 ——
/// 第一次真跑就会以一个看不懂的解析错误告终。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveTaskExpansion {
    /// `result.cloudCount` —— 本次领到多少云朵（豆）。
    ///
    /// `None` = 上游没给（**不是 0**）：0 是"领到 0 朵"，`None` 是"没说"。
    pub cloud_count: Option<i64>,
}

/// 解析 `receiveTaskExpansion` 的响应。
///
/// ⚠ **没有 `result` 不算失败。** 两个客户端都只拿 `code` 判成败：
/// H5 的 `.then` 里只比 `e.data.code`（`0` / `1302` / `500`），
/// 旧项目 `mcloud-sign` 写的是 `receiveData?.result?.cloudCount` —— `?.` 就是"没有也无所谓"。
///
/// 这里曾经把"没有 result"报成 [`Error::Parse`]，那是**比客户端更严**：
/// 真跑起来会以一句解析错误盖掉上游真正想说的话（比如 `1302` 未开备份）。
/// 现在只在拿不到 `cloudCount` 时给 `None`。
pub fn parse_receive_task_expansion(envelope: &Envelope) -> Result<ReceiveTaskExpansion, Error> {
    let payload = ensure_ok(envelope)?;
    Ok(ReceiveTaskExpansion {
        cloud_count: payload
            .as_ref()
            .and_then(|payload| payload.get("cloudCount"))
            .and_then(Value::as_i64),
    })
}

/// [`multiple_request`] 的解析结果 —— **签到翻倍**这一次额外领到多少豆。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleResult {
    /// `result.cloudCount` —— 本次额外获得的豆数。
    ///
    /// H5 逐字读它：`e.data.result.cloudCount` → 「额外获得 {cloudCount} 豆」。
    ///
    /// # 为什么是 `Option`（这条**没有实拍响应**）
    ///
    /// 字段名来自**当前版 H5 的调用点**（比旧项目的转述强），但**没有实拍**：
    /// 两个账号的 `curMonthBackupSignAccept` 都已经是 `true`（本月已领过），
    /// 复现不出来。所以按 [`ReceiveTaskExpansion::cloud_count`] 的同一约定 ——
    /// **缺了不判整条失败**。领没领到由 `code` 决定（`ensure_ok` 已经管了），
    /// 这个数只回答"领到多少"；硬要求它会把一次**成功**的领取变成解析错误。
    pub cloud_count: Option<i64>,
    /// `result.multiple` —— **倍数**（`N` 倍）。
    ///
    /// 旧项目逐字读了它并拼成一句日志：
    /// `ctx.logger.success("成功获得", multiple, "倍云朵，共计", cloudCount)`
    /// —— 即 **`multiple` 是倍数、`cloudCount` 是这次的合计**，两者不是一个量。
    /// 只报 `cloudCount` 会让"翻了几倍"这个信息丢掉。
    ///
    /// 与 [`Self::cloud_count`] 同档：**没有实拍**、**缺了不判失败**。
    pub multiple: Option<i64>,
}

/// 解析 `multiple` 的响应。
///
/// `result` 缺失**不判失败** —— 理由同 [`parse_receive_task_expansion`]：
/// 两个客户端都只拿 `code` 判成败，`result` 只是"领到多少"的补充。
pub fn parse_multiple(envelope: &Envelope) -> Result<MultipleResult, Error> {
    let payload = ensure_ok(envelope)?;
    let field = |key: &str| {
        payload
            .as_ref()
            .and_then(|payload| payload.get(key))
            .and_then(Value::as_i64)
    };
    Ok(MultipleResult {
        cloud_count: field("cloudCount"),
        multiple: field("multiple"),
    })
}
