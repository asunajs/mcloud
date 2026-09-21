use super::*;
use crate::Envelope;
use crate::base;
use mcloud_core::{Error, HttpRequest};
use serde_json::Value;

/// `cloudRecordV2` 的 `aiDou` —— **账本**。两个值，不是两种货币。
///
/// ⚠ 云朵与「云盘专属 AI豆」是**同一个东西的两个名字**（2026-07-31 按 1:1 改名）。
/// 这两档分开只是因为**记录跨了那次改名**：旧记录留在旧账本里，新记录进新账本。
/// 别把它理解成"两个池子"或"两种货币" —— 那种理解会让人以为可以互相换算。
pub const RECORD_BOOK_CLOUD: i64 = 0;
/// 见 [`RECORD_BOOK_CLOUD`]。`1` = 改名之后的账本。
pub const RECORD_BOOK_BEAN: i64 = 1;
/// `cloudRecordV2` 的 `type` —— H5 那三个筛选页签。
pub const RECORD_FILTER_ALL: i64 = 0;
/// 「获得记录」。
pub const RECORD_FILTER_EARNED: i64 = 1;
/// 「使用记录」。
pub const RECORD_FILTER_SPENT: i64 = -1;

/// `GET /ycloud/signin/public/cloudRecordV2` —— **豆的收支流水**（审计线）。
///
/// # 它解决什么问题
///
/// `infoV3.total` 给的是**当前余额**，但它不告诉你这个数是怎么来的 ——
/// 于是"余额对不上"这类问题只能靠猜。这条给的是**流水**。
///
/// # 形状（H5 源码逐字，`139newsignin.cc383f1b.js`）
///
/// ```javascript
/// getAIBeanRecordData: function(e){
///   return c["a"].get("/ycloud/signin/public/cloudRecordV2",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：{aiDou: 0|1, type: p.value, pageNumber: F.value, pageSize: 10}
/// ```
///
/// 即 **GET + 四个查询参数**、**没有 body**、**没有 `isDeviceId`**（H5 没标 ⇒ 拦截器不注入）。
/// `showLoading: true`。
///
/// ⚠ 函数名里那个 `AIBean` 是**误导**：同一个函数被两个页面共用，
/// 靠 `aiDou` 切换账本（`pointRecord` 传 `0`、`AIBeanRecord` 传 `1`）。
pub fn cloud_record_v2_request(
    jwt: &str,
    referer: &str,
    ai_dou: i64,
    filter: i64,
    page_number: i64,
    page_size: i64,
) -> HttpRequest {
    let url = format!(
        "{}/ycloud/signin/public/cloudRecordV2?aiDou={ai_dou}&type={filter}\
         &pageNumber={page_number}&pageSize={page_size}",
        base::MCLOUD
    );
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `cloudRecordV2` 的解析结果 —— 一页流水。
///
/// # 为什么条目是**原样的 `Value`**
///
/// H5 拿到 `records` 之后**直接丢给模板渲染**，从没读过条目里的任何具体字段名。
/// ⇒ 本仓库**没有任何 `records[]` 的实拍**（也没人抓过）。
///
/// 所以这里**不声明字段**：编一个 `amount` / `createTime` 出来，等于把猜测变成断言 ——
/// 第一次真跑就会以一个看不懂的解析错误收场，而原因只是"名字猜错了"。
/// 原样交出去，让调用方看着办。
///
/// 真正**有依据**的只有下面两个字段（H5 逐字读了它们）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRecords {
    /// `result.records` —— 本页流水条目，**原样**。
    pub records: Vec<Value>,
    /// `result.total` —— ⚠ **条目总数（分页用），不是金额**。
    ///
    /// 判据是 H5 的翻页逻辑：`records.length + … >= total` 就判"加载完了" ——
    /// 能这么比的只有条数。
    pub total: Option<i64>,
}

/// 解析 `cloudRecordV2` 的响应。
///
/// ⚠ 没有 `records` 时给**空数组**而不是报错：这是审计接口，
/// "这一档没有记录"是正常答案（例如改名前的账本本来就只到 2026-07-30）。
pub fn parse_cloud_records(envelope: &Envelope) -> Result<CloudRecords, Error> {
    let payload = ensure_ok(envelope)?;
    let records = payload
        .as_ref()
        .and_then(|payload| payload.get("records"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(CloudRecords {
        records,
        total: payload
            .as_ref()
            .and_then(|payload| payload.get("total"))
            .and_then(Value::as_i64),
    })
}
