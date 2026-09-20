use super::SignInCredential;
use super::session::open_session;
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::SignInStatus;
use mcloud_upstream::app_signin::{CloudRecords, PrizeRecords};
use mcloud_upstream::app_signin::{PRIZE_MARKET_NAME, PRIZE_STATUS_PENDING};
/// 只查状态，不签到（CLI 的 `--status-only`）。
pub async fn status<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<SignInStatus, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream.app_sign_in_status(&jwt, &referer).await
}

/// 同上，但返回 **`infoV3` 的原始响应体**（不解析）。
///
/// 排障用：见 [`mcloud_upstream::app_signin::Upstream::app_sign_in_status_raw`]。
pub async fn status_raw<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<serde_json::Value, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream.app_sign_in_status_raw(&jwt, &referer).await
}

/// `cloudRecordV2` 的查询条件。
///
/// 做成结构而不是四个裸参数，理由与 [`SignInOptions`] 同源：`ai_dou` 与 `filter`
/// 都是 `i64`，`page_number` 与 `page_size` 也都是 —— 四个裸 `i64` 连排时，
/// 传反了编译期一点提示都没有（实测这种错会静默变成"查错账本、取错页"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordsQuery {
    /// 账本：取 [`mcloud_upstream::app_signin::RECORD_BOOK_CLOUD`]（改名前的云朵）
    /// 或 [`mcloud_upstream::app_signin::RECORD_BOOK_BEAN`]（改名后的 AI 豆）。
    pub ai_dou: i64,
    /// 收支方向过滤。全部用 [`RECORD_FILTER_ALL`]。
    pub filter: i64,
    /// 页码，**从 1 开始**。
    pub page_number: i64,
    /// 每页条数。
    pub page_size: i64,
}

/// 查一档**豆的收支流水**（`cloudRecordV2`）。**只读**，审计 / 排障用。
///
/// # 它跟 `--status-only` 的分工
///
/// `--status-only` 回答"**现在有多少**"（`infoV3.total`）；这一条回答"**这些是怎么来的**"。
/// 余额对不上时，只有流水能指出差在哪一笔 —— 光看两个余额是永远对不出来的。
///
/// 查询条件走 [`RecordsQuery`]，见那里的说明。会话在这里建（见 [`open_session`]），用完即弃。
pub async fn cloud_records<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    query: &RecordsQuery,
    clock: &K,
    rng: &mut R,
) -> Result<CloudRecords, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_cloud_records(
            &jwt,
            &referer,
            query.ai_dou,
            query.filter,
            query.page_number,
            query.page_size,
        )
        .await
}

/// 查**待领取奖品**（`record` 的「待领取」页签）。只读。
///
/// # 它解决什么
///
/// 旧项目 `mcloud-sign` 的 `printPendingPrizes`：把"中了但还没领"的奖品列出来，
/// 连过期时间一起 —— 免得奖品躺着过期。
///
/// ⚠ 当前版 H5 的列表项**只显示「中奖时间」**（`insertTime`），
/// **没有 `expireTime`** —— 旧项目那个字段名在新接口上不存在，别照抄。
///
/// `page_size` 只取一页：这条的用途是"看有没有待领的"，不是把历史全拉下来。
pub async fn pending_prizes<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
    page_size: i64,
) -> Result<PrizeRecords, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_prize_records(
            &jwt,
            &referer,
            PRIZE_MARKET_NAME,
            PRIZE_STATUS_PENDING,
            1,
            page_size,
        )
        .await
}
