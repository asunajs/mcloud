use super::session::open_session;
use super::{SignInCredential, SignInOptions, SignInReport};
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
/// 签到。今天已签过就**不发** `startSignIn`，直接回报当前状态。
///
/// # `device_id`
///
/// 真实客户端会带 H5 那个 `deviceId`（`"B"` + 88 字符 base64），
/// **它就是 [`crate::smid::device_id`] 换回来的那个值** —— 签到页的 `fp.min.js`
/// 与本项目的 `mcloud-upstream::smid` 打的是同一个 `deviceprofile/v4`
/// （同 host / organization / appId / 公钥，逐字比对过），所以**纯 Rust 就能拿到**。
///
/// 但它是 `Option`：实测签到**不带也能签上**（2026-09-19 账号 B），
/// 所以调用点可以给 `None`；给 `Some(..)` 时是"更像真实客户端"的那一版。
/// 别让它变成"忘了传" —— 调用点必须显式写。
///
/// ⚠ 但**领取类接口（`receiveV3` 领云朵 / AI 豆）不带就报错**（回 `615`）——
/// 所以"签到能不带"**不能**外推到同族的别的接口，见 `app_signin` 的模块说明。
///
/// # 判据
///
/// 成败一律以**回查 `infoV3`** 为准，见 [`SignInReport`]。
pub async fn sign_in<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    options: &SignInOptions<'_>,
    clock: &K,
    rng: &mut R,
) -> Result<SignInReport, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;

    let before = upstream.app_sign_in_status(&jwt, &referer).await?;
    if before.signed_today() == Some(true) && !options.force {
        // 已经签过了：**不发** `startSignIn`。少一次写请求，也少一次风控记数。
        return Ok(SignInReport {
            before: before.clone(),
            after: before,
            result: None,
        });
    }

    let result = upstream
        .app_start_sign_in(&jwt, &referer, options.device_id)
        .await?;
    let after = upstream.app_sign_in_status(&jwt, &referer).await?;

    // ⚠ 这里**必须**回查，不能拿 `result.today_sign_in` 当结论。
    // 那个字段是调用**前**的快照 —— 实测 `todaySignIn:false` 的那次恰恰把今天签上了
    // （见 `docs/capture/app-signin-samples.txt` §3.2）。反过来，
    // 只看 `code == 0` 也不行：上游对"今天已经签过"同样回 `success`。
    // 所以唯一可信的判据是重新查一次 `infoV3`。
    if after.signed_today() != Some(true) {
        return Err(Error::Internal(format!(
            "startSignIn 回了成功，但回查 infoV3 显示今天仍未签（signed_today={:?}）。\
             以 infoV3 为准；`todaySignIn={}` 是调用前的快照，不是成败判据",
            after.signed_today(),
            result.today_sign_in
        )));
    }

    Ok(SignInReport {
        before,
        after,
        result: Some(result),
    })
}
