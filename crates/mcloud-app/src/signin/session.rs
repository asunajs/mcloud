use super::SignInCredential;
use crate::app_login::user_timestamp;
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::{TO_SOURCE_ID, sign_h5_request, signin_page_url};
/// 建会话：换 SSO token → 取服务端时间戳 → 签名 → `tyrzLogin`。
///
/// 返回 `(referer, jwtToken)`。`referer` 是签到页地址（带本次的 SSO token），
/// 后面每个请求都要带同一个 —— 它同时也是"这条会话属于哪个页面"的凭据。
///
/// 抽出来是因为 [`status`] 与 [`sign_in`] 的前四步**逐字相同**；
/// 复制一份的话，"签名少了一个头"这种错只会修在其中一条路径上。
pub(super) async fn open_session<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<(String, String), Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    // ① 换 SSO token。`x-yun-user-timestamp` 是**微秒**，由 `user_timestamp` 统一换算。
    let sso_token = upstream
        .app_query_spec_token(
            profile,
            user_timestamp(clock),
            credential.authorization(),
            TO_SOURCE_ID,
        )
        .await?;

    let referer = signin_page_url(&sso_token);

    // ② 服务端毫秒时间戳。**不能**用本地时钟替代：签名里用的就是它。
    let timestamp = upstream.app_h5_timestamp().await?;
    let sign = sign_h5_request(rng, &timestamp);

    // ③ 建会话。SSO token 很短命（换完几十秒内就要用），所以它到这一步之间没有别的 IO。
    let login = upstream.app_h5_login(&sign, &referer, &sso_token).await?;

    Ok((referer, login.jwt_token))
}
