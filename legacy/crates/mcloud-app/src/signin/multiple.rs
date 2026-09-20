use super::SignInCredential;
use super::session::open_session;
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::{MultipleResult, SignInStatus};
// ─────────────────────────────────────────────────────────────────────────────
// 签到翻倍（`multiple`）—— 旧项目 `mcloud-sign` 的 `executeSignInMultiple`
// ─────────────────────────────────────────────────────────────────────────────
/// 签到翻倍**能不能领** —— 判据逐字来自旧项目 `mcloud-sign`
/// （`packages/core/src/tasks/sign/task.ts`）：
///
/// ```javascript
/// if (curMonthBackup && !curMonthBackupSignAccept) { await executeSignInMultiple(ctx); }
/// ```
///
/// 两个值都在 `infoV3` 里，见 [`SignInStatus::cur_month_backup`] /
/// [`SignInStatus::cur_month_backup_sign_accept`]。
///
/// # 为什么要有这个枚举，而不是在调用点写个 `if`
///
/// **每月只有一次机会** —— 条件不成立还发，是白白消耗掉这个月的名额。
/// 而"条件不成立"有三种完全不同的原因（已领过 / 没开备份 / 上游没说），
/// 它们的**处置相同、说法不同**。做成枚举就能：① 单独测；② 原样报给用户，
/// 而不是笼统一句"跳过"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplePlan {
    /// **可以领**：本月开着备份，且这一次机会还没用掉。
    Claim,
    /// 本月**已经领过** —— 不再发请求。
    AlreadyAccepted,
    /// 本月**没开自动备份** —— 翻倍机会根本没产生。
    ///
    /// ⚠ 旧项目在这种情况下会**上传一个文件把备份打开**再重试
    /// （`backFile` + 等 `backupWaitTime`，默认 20 秒）。我们**没有网盘上传能力**，
    /// 所以这里只如实报告，**不假装做过**。要补的是"上传"那条线，不是这条判定。
    NoBackup,
    /// 上游**没给**这两个字段 —— 判不了。
    ///
    /// 单独成一档是为了**安全**：`None` 既不是"开了"也不是"没开"。
    /// 当成"开了"会白发一个写请求、把名额用掉；当成"没开"会漏领。
    /// 所以这一档的处置是**什么都不发**，并把这个"判不了"如实报出来。
    Unknown,
}

/// 按 `infoV3` 的两个门槛字段判 [`MultiplePlan`]。
///
/// ⚠ 匹配顺序有意义：`(true, true)` 要排在 `(false, _)` 前面 ——
/// 两个都"不领"，但"本月已领过"是更具体的说法，报给用户更有用。
pub fn plan_multiple(status: &SignInStatus) -> MultiplePlan {
    match (status.cur_month_backup, status.cur_month_backup_sign_accept) {
        (Some(true), Some(false)) => MultiplePlan::Claim,
        (Some(true), Some(true)) => MultiplePlan::AlreadyAccepted,
        (Some(false), _) => MultiplePlan::NoBackup,
        _ => MultiplePlan::Unknown,
    }
}

/// 领签到翻倍的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultipleOutcome {
    /// 领到了。`cloud_count` 见 [`MultipleResult::cloud_count`]
    /// （`None` = 上游没说领到多少，**不是 0**）。
    ///
    /// `multiple` 是**倍数**（`N` 倍），与 `cloud_count`（合计）不是一个量 ——
    /// 见 [`MultipleResult::multiple`]。
    Claimed {
        cloud_count: Option<i64>,
        multiple: Option<i64>,
    },
    /// 本月已领过 —— 没发请求。
    AlreadyAccepted,
    /// 本月没开自动备份 —— 没发请求（见 [`MultiplePlan::NoBackup`]）。
    NoBackup,
    /// 门槛字段上游没给 —— 没发请求（见 [`MultiplePlan::Unknown`]）。
    Unknown,
    /// 门槛成立、请求也发了，但**失败**（例如回 `1302`）。
    ///
    /// 不抛 `Err`：这个开关的用途恰恰是"看上游到底回什么码"，
    /// 抛出去就只剩一句错误信息。与 [`receive_all`] 的 [`ReceiveOutcome::Failed`] 同策。
    Failed { error: String },
}

/// 领签到翻倍（`multiple`）—— **写请求**，每月一次。
///
/// 自己先取一次 `infoV3` 判门槛（[`plan_multiple`]），不成立就**一个请求都不发**。
///
/// # 它解决什么
///
/// 「自动备份」每月送一次翻倍机会，把**今日签到奖励**翻 2–50 倍。
/// 旧项目 `mcloud-sign` 把它放在 `signIn` 里顺手做掉（`executeSignInMultiple`），
/// 很容易被忽略 —— 而它是一条**独立的、每月一次的**收入。
pub async fn claim_multiple<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<MultipleOutcome, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    let status = upstream.app_sign_in_status(&jwt, &referer).await?;

    match plan_multiple(&status) {
        MultiplePlan::Claim => match upstream.app_claim_multiple(&jwt, &referer).await {
            Ok(MultipleResult {
                cloud_count,
                multiple,
            }) => Ok(MultipleOutcome::Claimed {
                cloud_count,
                multiple,
            }),
            Err(err) => Ok(MultipleOutcome::Failed {
                error: err.to_string(),
            }),
        },
        MultiplePlan::AlreadyAccepted => Ok(MultipleOutcome::AlreadyAccepted),
        MultiplePlan::NoBackup => Ok(MultipleOutcome::NoBackup),
        MultiplePlan::Unknown => Ok(MultipleOutcome::Unknown),
    }
}
