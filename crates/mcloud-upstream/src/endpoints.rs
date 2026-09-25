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
