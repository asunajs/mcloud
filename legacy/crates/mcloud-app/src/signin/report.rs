use mcloud_upstream::app_signin::{SignInResult, SignInStatus};
/// 一次签到调用的结果。
///
/// `before` / `after` 都是**服务端给的**状态，不是从 `startSignIn` 的响应推的 ——
/// 见 [`SignInReport::signed_now`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInReport {
    /// 签到**前**的状态（`infoV3`）。
    pub before: SignInStatus,
    /// 签到**后**的状态。今天本来就签过时**不会**再查一次，两者相同。
    pub after: SignInStatus,
    /// `startSignIn` 的响应。`None` = 这次没发（查的时候今天已经签过了）。
    pub result: Option<SignInResult>,
}

impl SignInReport {
    /// 今天（调用之后）是否已签。**这是唯一的成败判据。**
    pub fn signed_today(&self) -> bool {
        self.after.signed_today() == Some(true)
    }
    /// 来的时候今天**就已经**签过了。
    ///
    /// 与 [`Self::signed_now`] 是互斥的两件事，而它们在日志里长得很像：
    /// "已签"既可能是"这次刚签上"，也可能是"本来就已经签过"。
    /// 定时任务连跑一个月，只有分开报才看得出它到底有没有在干活。
    pub fn was_already_signed(&self) -> bool {
        self.before.signed_today() == Some(true)
    }

    /// 这次调用**是否真的把今天从「未签」变成了「已签」**。
    ///
    /// 注意 `before` 那一项：`--force`（协议自检）会在已签的情况下也发一次
    /// `startSignIn`，那次的 `result` 是 `Some`、`after` 也是已签，
    /// 但**不是**"这次签上的"。
    pub fn signed_now(&self) -> bool {
        self.result.is_some() && !self.was_already_signed() && self.signed_today()
    }

    /// 本次签到可得云朵数（`startSignIn` 里那个）。没发请求时为 `None`。
    pub fn sign_in_points(&self) -> Option<i64> {
        self.result.as_ref().map(|r| r.sign_in_points)
    }

    /// 服务端报的本月已签天数。
    pub fn month_days(&self) -> i64 {
        self.after.month_days
    }
}

/// [`sign_in`] 的两个可选开关。
///
/// 做成结构而不是两个裸参数：两个都是 `Option`/`bool`，位置一错就是
/// "把 deviceId 当成了 force"，而编译器不会吭声。
#[derive(Debug, Clone, Copy, Default)]
pub struct SignInOptions<'a> {
    /// H5 那个 `deviceId`（`"B"` + 88 字符 base64 → 64 字节）。见 [`sign_in`] 的说明。
    pub device_id: Option<&'a str>,
    /// 今天已签也发一次 `startSignIn`。
    ///
    /// **唯一的用途是协议自检**：确认我们拼出来的请求被上游接受
    /// （而不是回 `401` / `90001`）。服务端对同一天的重复签到是幂等的 ——
    /// 实拍里真实客户端在已签状态下也调了它，回 `success` + `todaySignIn:true`。
    ///
    /// 日常签到**不要**开它：那会让"今天已经签过"这条捷径失效，多打一次写请求。
    pub force: bool,
}
