use super::SignInCredential;
use super::session::open_session;
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::{BackupGift, BackupGiftReceive};
/// 「备份好礼」的门槛判定（纯函数，可单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupGiftPlan {
    /// **可以领**：本月有礼、且还没领过。
    Claim,
    /// 本月**已领过** —— 不再发请求。
    AlreadyReceived,
    /// **未开启自动备份** —— 没有礼可领。
    ///
    /// 与 [`MultiplePlan::NoBackup`] 同源（旧项目这里也是提示"前往 APP 手动开启"），
    /// 同样**不上传文件去开备份** —— 我们没有网盘上传能力。
    NoBackup,
    /// 本月**没有**备份好礼（`curMonth` 为假）。
    ///
    /// 它比 [`Self::NoBackup`] 更靠前：旧项目先判 `if (!curMonth)` 就直接返回，
    /// 连 `state` 都不看。所以这一档**不是**"未开备份"，别把两者说成一回事。
    NothingThisMonth,
}

/// 按 `backupgift/info` 的两个字段判 [`BackupGiftPlan`]。
///
/// ⚠ 顺序有意义：`curMonth` 为假时**先**返回 [`BackupGiftPlan::NothingThisMonth`]，
/// 因为旧项目就是这么写的（`curMonth` 是前置开关，`state` 是它的细分类）。
pub fn plan_backup_gift(gift: &BackupGift) -> BackupGiftPlan {
    if !gift.cur_month {
        return BackupGiftPlan::NothingThisMonth;
    }
    match gift.state {
        // 旧项目的 `switch (state)`：`0` 可领、`1` 已领、`-1` 未开备份。
        0 => BackupGiftPlan::Claim,
        1 => BackupGiftPlan::AlreadyReceived,
        _ => BackupGiftPlan::NoBackup,
    }
}

/// 领「备份好礼」的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupGiftOutcome {
    /// 领到了。`received` 见 [`BackupGiftReceive::received`]（`None` = 上游没说）。
    Claimed { received: Option<i64> },
    /// 本月已领过 —— 没发请求。
    AlreadyReceived,
    /// 未开启自动备份 —— 没发请求。
    NoBackup,
    /// 本月没有备份好礼 —— 没发请求。
    NothingThisMonth,
    /// 门槛成立、请求也发了，但**失败**。不抛 `Err`，理由同
    /// [`MultipleOutcome::Failed`]。
    Failed { error: String },
}

/// 领「备份好礼」（`backupgift/receive`）—— **写请求**，每月一次。
///
/// 先查一次状态判门槛（[`plan_backup_gift`]），不成立就**一个请求都不发**。
///
/// # 它解决什么
///
/// 旧项目 `mcloud-sign` 的 `backupGiftTask`。它与 [`claim_multiple`] 是**两条独立的线**：
/// `multiple` 翻的是**今日签到奖励**，这条是「备份好礼」本身 ——
/// 两者都以「开了自动备份」为前提，所以 `state = -1` 时两条会一起拿不到，
/// 但**别因此把它们合并**：它们的接口、周期、门槛字段都不是同一个。
pub async fn claim_backup_gift<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<BackupGiftOutcome, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    let gift = upstream.app_backup_gift(&jwt, &referer).await?;

    match plan_backup_gift(&gift) {
        BackupGiftPlan::Claim => match upstream.app_receive_backup_gift(&jwt, &referer).await {
            Ok(BackupGiftReceive { received }) => Ok(BackupGiftOutcome::Claimed { received }),
            Err(err) => Ok(BackupGiftOutcome::Failed {
                error: err.to_string(),
            }),
        },
        BackupGiftPlan::AlreadyReceived => Ok(BackupGiftOutcome::AlreadyReceived),
        BackupGiftPlan::NoBackup => Ok(BackupGiftOutcome::NoBackup),
        BackupGiftPlan::NothingThisMonth => Ok(BackupGiftOutcome::NothingThisMonth),
    }
}
