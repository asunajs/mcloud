use super::SignInCredential;
use super::session::open_session;
use mcloud_core::{Clock, Error, Random};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::{ReceiveResult, RevivalReward};
/// [`receive_cloud`] 的输入：领哪一笔、以及拿什么设备标识去领。
///
/// 做成结构而不是三个裸参数，理由与 [`SignInOptions`] 同源：
/// 它们全是字符串 / `Option`，位置一错就是"把 cloudType 当成了 deviceId"，
/// 而编译器不会吭声。
#[derive(Debug, Clone, Copy, Default)]
pub struct ReceiveTarget<'a> {
    /// `receiveV3` 的 `cloudId` —— 来自 `infoV3` 的 `receiveList[].recordId`。
    ///
    /// 实拍里那个数组**没有 `recordId`**，所以 H5 的 `e.recordId ?? ""` 给的就是空串。
    pub cloud_id: &'a str,
    /// `receiveV3` 的 `cloudType` —— 同样来自 `receiveList[]`。
    ///
    /// `None` 表示"上游没给"，body 里会写成空串（照 H5 的 `?? ""`）。
    pub cloud_type: Option<i64>,
    /// H5 的 `deviceId` —— **就是 [`crate::smid::device_id`] 换回来的那个值**
    /// （`'B'` + 64 字节 base64）。
    ///
    /// 取值走 [`crate::smid::device_id`]（纯 Rust，不用浏览器）。
    ///
    /// ⚠ 上游**校验它的内容**（不带回 `615`、内容错回 `614`），所以不能给编的值 ——
    /// 但也不需要"用官方 App 打开一次签到页"：那个值我们自己就能换。
    pub device_id: &'a str,
}

/// 领云朵（`receiveV3`）。**这是写请求** —— 上游认为有可领的就会真的领走。
///
/// # 为什么要单独一个函数，而不是塞进 [`sign_in`]
///
/// 两者**不是一回事**：签到只是打卡，云朵要另外领（所以 `infoV3.beforeTotal`
/// 在签到前后不变）。日常流程里它们通常连着跑，但也有人只想签到、不想领。
/// 分开之后"领不领"由调用方决定，而不是被签到顺手决定掉。
///
/// # `device_id`
///
/// 这是**已知唯一一个不带就失败**的接口（H5 标了 `isDeviceId: true`），
/// 而且上游**校验它的内容** —— 见 [`ReceiveTarget::device_id`]。
///
/// ⚠ 会话在这里**又建了一次**（[`open_session`] 没暴露给外部）。
/// 自检/探测场景下多两次请求无所谓；真要做"签到 + 领取"一条龙时，
/// 应该把会话提出来复用，而不是让它建两遍。
pub async fn receive_cloud<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    target: &ReceiveTarget<'_>,
    clock: &K,
    rng: &mut R,
) -> Result<ReceiveResult, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_receive_v3(
            &jwt,
            &referer,
            target.cloud_id,
            target.cloud_type,
            target.device_id,
        )
        .await
}

/// 用复活卡 / 回馈卡（`receiveRevivalReward`）。**这是写请求** —— 卡用掉就没了。
///
/// # 它跟 [`receive_cloud`] 不是同一件事
///
/// `receiveV3` 领的是**当期可领的云朵**；这一条用的是**卡**：
/// H5 弹窗写得很清楚 ——「关闭弹窗视为放弃机会，**无法补发**」。
/// 所以调用点必须是有意为之，不能"顺手也跑一下"。
///
/// # `device_id` 为什么是 `Option`
///
/// 两个证据不一致：H5 定义里**没有** `isDeviceId`（⇒ 不发），
/// 老抓包里却带了一个 `deviceid` **头**。哪个是服务端真正要求的**未验证**，
/// 所以留成可选参数 —— 真出问题时能一次 A/B 试出来（`receiveV3` 就是这么定的）。
/// 要显式给的话，取值走 [`crate::smid::device_id`]（纯 Rust，见那里）。
///
/// ⚠ 与会话的取舍同 [`receive_cloud`]：这里**又建了一次会话**。
pub async fn receive_revival_reward<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    device_id: Option<&str>,
    clock: &K,
    rng: &mut R,
) -> Result<RevivalReward, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_receive_revival_reward(&jwt, &referer, device_id)
        .await
}
