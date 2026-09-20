use super::*;
use crate::Envelope;
use crate::base;
use mcloud_core::{Error, HttpRequest};
use serde_json::Value;

/// `GET /market/backupgift/info` —— **备份好礼**的状态。
///
/// # 为什么主机是 `caiyun.feixin.10086.cn` 而不是 `m.mcloud.139.com`
///
/// 这个接口**不在新 H5 的签到页里** —— 新 H5 里 `backupGift` 只出现一次，
/// 是一个指向**独立活动页**的 `popLink`
/// （`/portal/caiyunEventPage/index.html?path=backupGifts`）。
/// 所以它没有"新接口"可换，沿用的是旧项目那条。
///
/// ⚠ 换主机是**有实测依据**的，不是照抄旧项目：裸请求
/// `https://caiyun.feixin.10086.cn/market/backupgift/info` 回
/// `{"code":90001,"msg":"未登录"}` —— 与 `m.mcloud.139.com` 上的
/// `record` / `liveRoomFeedback` **回同一个信封**，说明两者在**同一道鉴权网关**后面。
/// （若哪天它回 `404` 而不是 `90001`，就是换了主机，别以为"接口下线了"。）
pub fn backup_gift_info_request(jwt: &str, referer: &str) -> HttpRequest {
    let url = format!("{}/market/backupgift/info", base::CAIYUN);
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `GET /market/backupgift/receive` —— **领备份好礼**。写请求。
///
/// 门槛**不在这里判**：旧项目先读 [`backup_gift_info_request`] 的 `state`，
/// 只有 `state == 0`（可领）才发这条。可执行版本在
/// `mcloud_app::signin::plan_backup_gift`。
pub fn backup_gift_receive_request(jwt: &str, referer: &str) -> HttpRequest {
    let url = format!("{}/market/backupgift/receive", base::CAIYUN);
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `backupgift/info` 的解析结果 —— **备份好礼**。
///
/// # 三态（旧项目 `backupGiftTask` 的 `switch (state)` 逐字）
///
/// | `state` | 含义 | 动作 |
/// | --- | --- | --- |
/// | `-1` | **未开启备份** | 不领，提示去 App 开 |
/// | `1` | 本月**已领过** | 不领 |
/// | `0` | **可领** | 发 [`backup_gift_receive_request`] |
///
/// ⚠ `state` 是**必填**（用 [`require_i64`]）：它就是这条接口的主判据 ——
/// 缺了它就无法回答"该不该领"，而这正是本结构存在的唯一理由。
/// （与 [`MultipleResult::cloud_count`] 那种"只回答领到多少"的附属字段成对，
/// 见 [`parse_sign_in_status`] 里对两者的说明。）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupGift {
    /// `result.state` —— 见上表。
    pub state: i64,
    /// `result.curMonth` —— 本月**有没有**备份好礼。
    ///
    /// 旧项目先判它：`if (!curMonth) { "无备份礼物可领"; return; }` ——
    /// 即它比 `state` **更靠前**，`false` 时连 `state` 都不用看。
    pub cur_month: bool,
}

/// 解析 `backupgift/info` 的响应。
pub fn parse_backup_gift(envelope: &Envelope) -> Result<BackupGift, Error> {
    let payload = ensure_ok(envelope)?;
    let payload =
        payload.ok_or_else(|| Error::Parse("backupgift/info 响应没有 result".to_string()))?;
    Ok(BackupGift {
        state: require_i64(payload, "state")?,
        // `curMonth` 旧项目当布尔用（`if (!curMonth)`），所以这里按"真值"收：
        // 布尔、数字、字符串三种形态都认，缺了当 `false`。
        cur_month: match payload.get("curMonth") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_i64().is_some_and(|value| value != 0),
            Some(Value::String(text)) => !text.is_empty() && text != "0" && text != "false",
            _ => false,
        },
    })
}

/// `backupgift/receive` 的解析结果 —— 这次领到多少豆。
///
/// # 为什么 `result` 直接是个数字
///
/// 旧项目的类型就写着 `result: number`，调用点读 `typeof receiveData?.result === "number"`
/// 再把它当豆数打印。⇒ 这条的 `result` **不是对象**，别按对象去取字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupGiftReceive {
    /// `result` —— 领到的豆数。上游没给数字时是 `None`（≠ 0）。
    pub received: Option<i64>,
}

/// 解析 `backupgift/receive` 的响应。
///
/// `result` 缺失**不判失败** —— 同 [`parse_multiple`]：成败由 `code` 决定。
pub fn parse_backup_gift_receive(envelope: &Envelope) -> Result<BackupGiftReceive, Error> {
    let payload = ensure_ok(envelope)?;
    Ok(BackupGiftReceive {
        received: payload.and_then(|value| value.as_i64()),
    })
}
