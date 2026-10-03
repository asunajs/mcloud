//! 端点常量。
//!
//! 集中放置便于版本升级时统一修改，也便于测试断言。

/// 认证域：这些域名接受 `authorization`（basic）。
pub const AUTH_DOMAINS: [&str; 4] = [
    "yun.139.com",
    "mcloud.139.com",
    "feixin.10086.cn",
    "mail.10086.cn",
];

/// 这些主机接受 `jwttoken`。
pub const JWT_HOSTS: [&str; 3] = [
    "caiyun.feixin.10086.cn",
    "mrp.mcloud.139.com",
    "m.mcloud.139.com",
];

/// 查询 ssoToken。
pub const QUERY_SPEC_TOKEN: &str = "https://user-njs.yun.139.com/user/querySpecTokenV2";

/// 新版统一认证换取 jwtToken。
pub const TYRZ_LOGIN_NEW: &str = "https://m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin";

/// 旧版统一认证（带签名头）。
pub const TYRZ_LOGIN_LEGACY: &str =
    "https://caiyun.feixin.10086.cn/portal/auth/v2/tyrzLogin.action";

/// 刷新 basic auth（XML 报文）。
pub const AUTH_TOKEN_REFRESH: &str =
    "https://aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do";

/// 默认 sourceId。
pub const DEFAULT_SOURCE_ID: &str = "001005";

/// 邮件业务用的 sourceId。
pub const MAIL_SOURCE_ID: &str = "001003";

// ── PC 客户端登录链路（`../mcloud-capture/docs/upstream/login-protocol.md` 实测）──────────────
//
// 四条都在**同一个**用户域主机 `user-njs.yun.139.com` 上。
// 注意这个主机**不在** `AUTH_DOMAINS` 里 —— 登录时还没有凭据，
// 本来就不该注入 `authorization`；登录成功后拿到的 auth 串才用于后续请求。

/// 登录总入口：扫码 / 短信 / 账密**共用**这一个接口，靠 `pintype` 区分。
pub const THIRD_LOGIN: &str = "https://user-njs.yun.139.com/user/thirdlogin";

/// 发送短信验证码。
pub const SEND_SMS_CODE: &str = "https://user-njs.yun.139.com/user/sms/getSmsCode";

/// 取滑块拼图。路径是 `verfycode`（**没有 `i`**）—— 上游就这么拼的。
pub const SLIDER_PUZZLE: &str = "https://user-njs.yun.139.com/user/verfycode";

/// 退出登录。实测客户端是"先清本地 token 再调"，服务端常回
/// `01000001 用户token不能为空` —— 那不是失败，是正常顺序的结果。
pub const LOGOUT: &str = "https://user-njs.yun.139.com/user/logout";

// ── H5 签到页那一族（`crates/mcloud-upstream/src/app_signin.rs`）──────────

/// H5 那一族的主机。
///
/// `origin` 请求头要用它（兑换下单实拍里带了 `origin`），
/// 所以单列一个常量而不是让各处以 `"https://m.mcloud.139.com"` 字面量散步。
pub const MCLOUD_HOST: &str = "https://m.mcloud.139.com";

/// 签到页的主机 + 路径前缀。
pub const SIGNIN_PAGE_BASE: &str = "https://m.mcloud.139.com/ycloud/signin/page";

/// 兑换清单（**只读**）。查询串只有 `client` 与 `clientVersion`。
pub const EXCHANGE_LIST: &str = "https://m.mcloud.139.com/ycloud/signin/page/exchangeList";

/// 下单（**花真豆**）。
pub const EXCHANGE_V3: &str = "https://m.mcloud.139.com/ycloud/signin/page/exchangeV3";

/// 取滑块题面（**只读**）。
///
/// ⚠ 主机路径族是 `/ycloud/auth-service/slide/`，与上面两条的
/// `/ycloud/signin/page/` **不同** —— 别顺手复用那一族的请求构造。
pub const GET_SLIDE: &str = "https://m.mcloud.139.com/ycloud/auth-service/slide/getSlide";

// ── 个人网盘那一族（crates/mcloud-upstream/src/cloud_trash.rs）──────────
//
// ⚠ 与上面 H5 那一族**不是同一个主机**：H5 走 `m.mcloud.139.com`（还要
// `activityid` / `jwttoken` 那一套签名头），网盘走 `personal-kd-njs.yun.139.com`
// （用 basic `authorization`，无签名）。两族的头**不能互换** ——
// 换错的表现是「登录是好的、网盘回 90001」，最难查的那类。

/// 个人网盘（文件 / 回收站那一族）的基础域。
///
/// 它落在 `AUTH_DOMAINS` 的 `yun.139.com` 后缀之内 ⇒ 客户端会**自动**注入
/// basic `authorization`。所以请求构造里**不该**再自己设一遍：
/// reqwest 的 `.header()` 是**追加**，设两次就是发出去两根 `authorization`
/// 线，而上游读哪一根**未测试**。
pub const PERSONAL_KD: &str = "https://personal-kd-njs.yun.139.com";

/// 批量移进回收站（**写**，且不可回滚 —— 只是进回收站，不是彻底删除）。
///
/// 路径出处：旧 TS `apis/file.ts:114`；2026-09-26 由活体探针真打闭合。
pub const RECYCLEBIN_BATCH_TRASH: &str =
    "https://personal-kd-njs.yun.139.com/hcy/recyclebin/batchTrash";

/// 读回收站列表（**只读**，可反复跑）。
pub const RECYCLEBIN_LIST: &str = "https://personal-kd-njs.yun.139.com/hcy/recyclebin/list";

/// 判断主机是否应注入 basic authorization。
pub fn accepts_basic(host: &str) -> bool {
    AUTH_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// 判断主机是否应注入 jwttoken。
pub fn accepts_jwt(host: &str) -> bool {
    JWT_HOSTS.contains(&host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_is_domain_scoped() {
        assert!(accepts_basic("yun.139.com"));
        assert!(accepts_basic("user-njs.yun.139.com"));
        assert!(accepts_basic("m.mcloud.139.com"));
        assert!(!accepts_basic("evil.com"));
        // 后缀必须落在域名边界上
        assert!(!accepts_basic("notyun.139.com.evil.com"));
    }

    #[test]
    fn jwt_is_host_exact_only() {
        assert!(accepts_jwt("m.mcloud.139.com"));
        assert!(accepts_jwt("caiyun.feixin.10086.cn"));
        // 子域不算
        assert!(!accepts_jwt("sub.m.mcloud.139.com"));
    }
}
