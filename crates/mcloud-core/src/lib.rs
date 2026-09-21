//! mcloud-core —— 纯业务内核。
//!
//! # 硬约束
//!
//! 本 crate **禁止** 依赖 `reqwest` / `rusqlite` / `tokio` / `std::fs` / `std::net`，
//! 也禁止直接调用 `SystemTime::now()`。一切外部世界通过 [`traits`] 与 [`Random`] 注入：
//! 网络走 `HttpClient`、时间走 `Clock`、存储走 `StateStore`、随机走 `Random`。
//! 纯密码学（哈希/对称/非对称/压缩）是**无 IO 的纯计算**，因此也在这里（[`crypto`]），
//! 这样 [`crate::upstream`] 之外的上游模块无需引入 `reqwest` 就能签名。
//!
//! 这条约束由 `scripts/check-deps.sh` 断言（门禁第 4 条，见 [`docs/dev-gates.md`]），
//! 它同时也是"能对某个函数单独测试"的成立前提。
//!
//! [`docs/dev-gates.md`]: ../../../docs/dev-gates.md

pub mod auth;
pub mod cron; // cron 表达式：解析与下次触发
pub mod crypto;
pub mod error;
pub mod legacy;
pub mod model;
pub mod pure;
pub mod run; // 运行语义：RunId、四态判定、事件模型
pub mod sanitize;
pub mod stage; // 运行生命周期状态机
pub mod traits;
pub mod user;

/// 跨测试文件共享的断言（仅 `cfg(test)`）。见模块头说明为什么不放在各自文件里。
#[cfg(test)]
pub(crate) mod test_support;

pub use auth::{
    AuthInfo, account_id, account_label, legacy_account_key, pad_base64, parse_auth, resolve_phone,
};
pub use crypto::{
    AES_FIXED_IV, REFRESH_TOKEN_KEY, Random, SIGN_SALT_MAP, SKLAND_PUBLIC_KEY_B64,
    aes_cbc_fixed_iv_decrypt_hex, aes_cbc_fixed_iv_encrypt_hex, aes_dynamic_iv_decrypt,
    aes_dynamic_iv_encrypt, aes_dynamic_iv_encrypt_random, aes_ecb_pkcs7_decrypt_hex,
    aes_ecb_pkcs7_encrypt_hex, b64_decode, b64_encode, des_ecb_no_padding_encrypt_base64,
    des_ecb_pkcs7_decrypt, generate_signature, get_tn, get_tn_md5, gunzip, gzip_with_os19,
    gzip_with_os19_base64, hmac_sha256_base64, md5_hex, rsa_pkcs1v15_encrypt_base64,
    rsa_pkcs1v15_encrypt_base64_with_b64_key, sha1_hex_upper, sha256_hex, shift_head10_to_back,
    shift_tail10_to_front, sign_salt,
};
pub use error::{
    Classified, Error, ErrorKind, FailureClass, body_snippet, classify_body,
    classify_business_code, classify_status,
};
pub use model::{
    Account, AccountId, AccountMode, AccountState, CapabilityKind, CloudStats,
    DEFAULT_EXCHANGE_WINDOWS, DEFAULT_WEBUI_PAGE, ExchangeBinding, ExchangeConfig, FieldKind,
    FieldSpec, Group, Pipeline, PipelineStatus, PipelineStep, RiskLevel, RunRecord, RunStatus,
    Schedule, ScheduleStatus, ScheduleTarget, SelectOption, SmidState, WEBUI_PAGES, WebuiConfig,
};
pub use run::{
    LogLevel, ManualStep, ManualTarget, RunEvent, RunId, RunOutcome, RunReport, Seq, Sequenced,
};
pub use stage::{RunExecution, RunStage, StageError};
pub use traits::{
    ClaimedRows, Clock, ConfigStore, FixedClock, HttpClient, HttpMethod, HttpRequest, HttpResponse,
    Scope, StateStore, UserStore,
};
pub use user::{
    MIN_PASSWORD_LEN, Role, SESSION_TOKEN_BYTES, Session, SessionKind, User, UserId, hash_password,
    hash_session_token, is_visible_to, new_session_token, verify_password,
};

/// 产品名。
///
/// 单独定义而不是用 `env!("CARGO_PKG_NAME")`：`/api/version` 与 `--version` 对外暴露的
/// 必须是**产品名**（`mcloud`），而不是"碰巧实现它的那个 crate"的名字。
/// 否则哪天把 `version_payload` 从 server 挪到 cli，对外响应就会悄悄改名。
pub const PRODUCT_NAME: &str = "mcloud";

/// 日快照与时间窗口统一使用的时区（项目面向国内用户）。
pub const TZ_NAME: &str = "Asia/Shanghai";

/// 返回 `Asia/Shanghai` 时区。
///
/// 月份/日快照的"今天"必须按此时区切分，否则跨零点会算错增量。
pub fn local_tz() -> chrono_tz::Tz {
    TZ_NAME.parse().expect("TZ_NAME 必须是合法时区名")
}
