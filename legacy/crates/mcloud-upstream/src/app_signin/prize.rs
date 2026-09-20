use super::*;
use crate::Envelope;
use crate::base;
use mcloud_core::{Error, HttpRequest};
use serde_json::Value;

/// `record` 的 `marketname` —— H5 两个奖品记录组件**都传这个值**（逐字核对过两处调用点）。
///
/// ⚠ 名字里的 `_ex` **不是"兑换"**：这两个组件的页签是「待领取 / 已领取 / 已过期」，
/// 是**奖品记录**不是兑换流水（兑换走的是 `exchangeV3` / `exchangeList`）。
/// 别看到 `_ex` 就往"exchange"上想。
pub const PRIZE_MARKET_NAME: &str = "sign_in_3_ex";
/// `record` 的 `type` —— 页签。见 [`PRIZE_MARKET_NAME`] 上那句提醒。
///
/// 这三个值是 H5 里**写死的字面量**（`[{id:1,btnText:"待领取"},{id:2,btnText:"已领取"},
/// {id:10,btnText:"已过期"}]`），不是从响应里读出来的 —— 所以它们是常量不是枚举探测。
pub const PRIZE_STATUS_PENDING: i64 = 1;
/// 「已领取」。
pub const PRIZE_STATUS_RECEIVED: i64 = 2;
/// 「已过期」。
pub const PRIZE_STATUS_EXPIRED: i64 = 10;

/// `GET /ycloud/signin/public/record` —— **奖品记录**（默认页签是「待领取」）。
///
/// 这是旧项目 `printPendingPrizes`（`getUserPrizeLogPageV2`）在新接口上的对应物。
///
/// # 形状（H5 源码逐字，两处调用点）
///
/// ```javascript
/// getPrizeRecordData: function(e){
///   return c["a"].get("/ycloud/signin/public/record",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：{marketname: "sign_in_3_ex", type: <页签 id>, pageNumber: F.value, pageSize: 10}
/// ```
///
/// 即 **GET + 四个查询参数**、**没有 body**、**没有 `isDeviceId`**（H5 没标）。
/// 分页判据是 H5 逐字写的：**`records.length < pageSize` ⇒ 到底了**
/// （`t.length<10&&(X.value=!0)`）—— 它**不读 `total`**。
///
/// # 与旧接口的关系
///
/// 旧项目打的 `.../checkPrize/getUserPrizeLogPageV2?currPage=&pageSize=` 在
/// `m.mcloud.139.com` 上**仍然存在**，但**裸请求回 `501 参数校验异常`**
/// （换 `pageNumber` 直接 `404`）—— 参数口径已经变了。新接口是这条。
pub fn prize_record_request(
    jwt: &str,
    referer: &str,
    marketname: &str,
    status: i64,
    page_number: i64,
    page_size: i64,
) -> HttpRequest {
    let url = format!(
        "{}/ycloud/signin/public/record?marketname={}&type={status}\
         &pageNumber={page_number}&pageSize={page_size}",
        base::MCLOUD,
        percent_encode(marketname)
    );
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `record` 的解析结果 —— 一页**奖品记录**（见 [`prize_record_request`]）。
///
/// # 字段依据（**有实拍**，2026-09-19，账号 159）
///
/// 「待领取」页签回了一条。实拍条目长这样（**已去掉账号标识字段**）：
///
/// ```json
/// {"flag":1,"groupId":0,"type":1057,"source":0,"belonger":0,"device":0,
///  "prizeId":260630014,"prizeName":"移动云盘1000万tokens叠加包",
///  "insertTime":"2026-09-16T11:12:54.000+00:00",
///  "expireTime":"2026-09-30T15:59:59.000+00:00",
///  "updateTime":"2026-09-16T11:12:54.000+00:00",
///  "market":"sign_in_3_ex","marketName":"sign_in_3_ex",
///  "oid":1270932714,"orderType":0,"prizeSectionStatus":1,
///  "reasons":"260630014","uorder":400,
///  "linkUrl":"{\"saleType\":103,\"days\":\"0\",\"serviceId\":\"…\"}"}
/// ```
///
/// ⇒ 所以这里**声明**这几个字段。但仍保持 `Option`：
/// 缺一个就少一行字，不该把整条查询打成解析错误
/// （**空页才是最常见的答案** —— 三个页签里总有两个是空的）。
///
/// ⚠ 条目里**带账号标识**（`userDomainId`，那个 19 位值）。所以本仓库
/// **不留原样条目**：黄金向量只保留上面这几个字段，见
/// `test/golden/app-signin-vectors.json`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrizeRecordItem {
    /// `prizeName` —— 奖品名。列表主文案。
    pub prize_name: Option<String>,
    /// `insertTime` —— **中奖时间**。H5 用 dayjs 格式化成 `YYYY-MM-DD`。
    pub insert_time: Option<String>,
    /// `expireTime` —— **过期时间**。
    ///
    /// ⚠ 这里曾经写着"当前版 H5 没有这个字段"——**错了，已按实拍更正**。
    /// H5 的**列表项模板**确实只显示「中奖时间」（`insertTime`），
    /// 但响应里 `expireTime` **一直在**：实拍是 `2026-09-30T15:59:59.000+00:00`
    /// （中奖后 14 天）。「模板没渲染它」**不等于**「响应没有它」——
    /// 旧项目 `printPendingPrizes` 读这个字段是对的，当时是我把两件事混成了一件。
    pub expire_time: Option<String>,
    /// `flag` —— **`1` = 待领取**。旧项目 `printPendingPrizes` 就是按它过滤的。
    ///
    /// 页签（请求参数 `type`）已经筛过一遍，所以它多半恒为 `1`；
    /// 留着是因为它**是旧项目那条过滤的判据**，丢了就没人能解释"为什么这么筛"。
    pub flag: Option<i64>,
    /// `prizeId` —— 奖品 id。点「去领取」时要回传给奖品详情接口。
    pub prize_id: Option<i64>,
    /// `oid` —— 中奖记录 id（H5 里叫 `drawRecode`，是**同一个值换了个名字**）。
    ///
    /// 实拍是**数字**（`1270932714`），而 H5 把它拼进 URL 查询串 ——
    /// 两种形态都能用，所以这里统一成字符串。
    pub oid: Option<String>,
    /// `marketName` —— 该奖品属于哪个活动（H5 里叫 `marketId`）。实拍 `sign_in_3_ex`。
    pub market_name: Option<String>,
    /// `type` —— 奖品类型号（实拍 `1057`）。
    ///
    /// ⚠ 与请求参数那个"页签 `type`"（`1/2/10`）**同名不同物**，别混。
    pub prize_type: Option<i64>,
    /// 本条**原样**保留 —— 上面那几个字段是"能认出来的"，其余不替上游编名字。
    ///
    /// ⚠ 它含账号标识，**别直接落进文档或黄金向量**。
    pub raw: Value,
}

/// 一页奖品记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrizeRecords {
    /// `result.records` —— 本页条目。
    pub records: Vec<PrizeRecordItem>,
}

/// 解析 `record` 的响应。
///
/// ⚠ 没有 `records` 时给**空数组**而不是报错：三个页签里总有两个是空的
/// （待领取/已领取/已过期），"这一档没有记录"正是最常见的**正常**答案。
pub fn parse_prize_records(envelope: &Envelope) -> Result<PrizeRecords, Error> {
    let payload = ensure_ok(envelope)?;
    let items = payload
        .as_ref()
        .and_then(|payload| payload.get("records"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let str_field =
        |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
    let records = items
        .into_iter()
        .map(|raw| PrizeRecordItem {
            prize_name: str_field(&raw, "prizeName"),
            insert_time: str_field(&raw, "insertTime"),
            expire_time: str_field(&raw, "expireTime"),
            flag: raw.get("flag").and_then(Value::as_i64),
            prize_id: raw.get("prizeId").and_then(Value::as_i64),
            // `oid` 可能是数字也可能是字符串 —— H5 直接把它塞进 URL 参数，
            // 两种形态都能用，所以这里两种都收，统一成字符串。
            oid: raw.get("oid").and_then(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            }),
            market_name: str_field(&raw, "marketName"),
            prize_type: raw.get("type").and_then(Value::as_i64),
            raw,
        })
        .collect();
    Ok(PrizeRecords { records })
}
