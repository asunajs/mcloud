//! 云盘账号管理（列表 / 删除）。
//!
//! # ⚠ 绝不能把 `Account` 直接序列化出去
//!
//! `mcloud_core::Account` 派生 `Serialize`，而它含 `auth` ——
//! 那是**能冒充用户的完整凭据**（`Basic base64("pc:<手机号>:<token>")`）。
//! 直接 `Json(account)` 就等于把账号交出去；而这条路的鉴权只是
//! "本实例管理员"，一旦浏览器插件/日志/录屏截到，凭据就漏了。
//!
//! 所以这里用 [`AccountSummary`] 这一层**显式**的 DTO：
//! 只挑列表页真正要用的字段（掩码账号名、平台、设备号、到期时间）。
//! 新增字段时必须**主动**加进 DTO —— 这正是我们要的默认：
//! 漏掉一个字段的代价是功能少一点，而反射式序列化的代价是漏凭据。

use axum::extract::{Path, State};
use axum::Json;
use mcloud_core::AccountId;
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 列表页要展示的账号信息。
///
/// 刻意**不含** `auth` / `token` / `phone`：
///
/// | 字段 | 为什么不给 |
/// | --- | --- |
/// | `auth` / `token` | 是凭据本身 |
/// | `phone`（完整） | 页面只需要掩码；完整号码没有展示场景 |
#[derive(Serialize)]
pub struct AccountSummary {
    /// 账号主键：手机号的 SHA-256 前 16 位。
    ///
    /// ⚠⚠ **它只当键用，不该拿去显示** —— 那是一串不透明十六进制
    /// （`0f6050105ab111f4`），用户认不出是哪个号。要显示请用 [`Self::label`]。
    ///
    /// 它从前是**掩码手机号**，那是错的：掩码只留前 2 后 2 位
    /// ⇒ `19??????16` 形状的两个号会撞成同一个主键、数据串号。
    pub id: String,
    pub nickname: Option<String>,
    /// **给界面看的名字**：有手机号就是掩码（`19****01`），否则回落昵称。
    ///
    /// ⚠ 这一格**必须**给：主键改成哈希之后，前端拿 `id` 当兜底会显示成
    /// `0f6050105ab111f4` —— 那是"能跑但用户认不出账号"的典型长相。
    pub label: String,
    /// 登录时冒充的 PC 客户端形态（`windows` / `macos`）。
    ///
    /// `null` = 非 PC 端登录的账号（App / 历史数据）。
    /// 前端据此提示"后续请求会按这个平台发"。
    pub pc_platform: Option<mcloud_core::PcPlatform>,
    pub device_id: String,
    /// 凭据到期时间（**epoch 毫秒**）。`0` = 解析不出。
    pub expire: i64,
}

impl From<mcloud_core::Account> for AccountSummary {
    fn from(a: mcloud_core::Account) -> Self {
        Self {
            id: a.id.as_str().to_string(),
            label: mcloud_upstream::account_label(a.nickname.as_deref(), &a.auth.phone),
            nickname: a.nickname,
            pc_platform: a.pc_platform,
            device_id: a.device_id.as_str().to_string(),
            expire: a.auth.expire,
        }
    }
}

/// `GET /api/accounts` —— 列出已登录的云盘账号。
pub async fn list(State(state): State<ServerState>) -> ApiResult<Vec<AccountSummary>> {
    let accounts = state
        .handle
        .store
        .accounts()
        .await
        .map_err(ApiError::from)?;
    Ok(Json(accounts.into_iter().map(Into::into).collect()))
}

/// `DELETE /api/accounts/{id}` —— 移除一个账号（本地凭据）。
///
/// # 它是**幂等**的
///
/// 删一个不存在的账号返回 200 而不是 404：用户的意图是"让这个账号消失"，
/// 而它已经不在了 —— 意图已经达成。返回 404 只会让前端弹一个
/// "删除失败"的红框，而用户再点一次就好了，体验上说不通。
///
/// 与 `/api/runs/{id}` 那边刻意不同：那里查的是一个**要读的资源**，
/// "不存在"必须让调用方知道；这里是**收敛到一个目标状态**。
pub async fn remove(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let account_id = AccountId::new(id);
    state
        .handle
        .store
        .remove_account(&account_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "removed": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Account, AccountSettings, AuthInfo, DeviceId, PcPlatform};

    fn sample() -> Account {
        Account {
            id: AccountId::new("0f6050105ab111f4"),
            nickname: Some("小明".into()),
            auth: AuthInfo {
                raw: "Basic U0VDUkVULUNSRURFTlRJQUw=".into(),
                platform: "pc".into(),
                phone: "19900000001".into(),
                token: "SECRET-TOKEN".into(),
                expire: 1_893_456_000_000,
            },
            device_id: DeviceId("DEV-1".into()),
            pc_platform: Some(PcPlatform::MacOs),
            user_domain_id: Some("1990000000000000001".into()),
            settings: AccountSettings::default(),
        }
    }

    /// ⭐ 凭据**绝不能**出现在 DTO 的 JSON 里。
    ///
    /// 这条是防回归的关键：将来有人给 `AccountSummary` 加字段时，
    /// 如果顺手加了 `auth`，这条测试会立刻红。
    #[test]
    fn summary_never_leaks_credentials() {
        let json = serde_json::to_string(&AccountSummary::from(sample())).unwrap();

        for forbidden in [
            "SECRET-TOKEN",             // token
            "U0VDUkVULUNSRURFTlRJQUw=", // auth 原始串
            "19900000001",              // 完整手机号
            "Basic ",                   // 凭据前缀
        ] {
            assert!(
                !json.contains(forbidden),
                "凭据片段 {forbidden:?} 出现在响应里：{json}"
            );
        }
    }

    /// 但该给的字段一个都不能少。
    #[test]
    fn summary_carries_the_fields_the_page_needs() {
        let v = serde_json::to_value(AccountSummary::from(sample())).unwrap();
        assert_eq!(v["id"], "0f6050105ab111f4");
        // ⭐ 显示用 `label`（掩码），不是 `id`（哈希）—— 见下面那条测试
        assert_eq!(v["label"], "19****01");
        assert_eq!(v["nickname"], "小明");
        // ⭐ 平台必须给出：前端要显示"这个账号按 macOS 发请求"
        assert_eq!(v["pc_platform"], "macos");
        assert_eq!(v["device_id"], "DEV-1");
        assert_eq!(v["expire"], 1_893_456_000_000i64);
    }

    /// 非 PC 账号的平台是 `null`，不是 `"windows"`。
    #[test]
    fn non_pc_account_reports_null_platform() {
        let mut a = sample();
        a.pc_platform = None;
        let v = serde_json::to_value(AccountSummary::from(a)).unwrap();
        assert!(v["pc_platform"].is_null(), "实得：{v}");
    }

    /// ⭐ `label` **必须**给，而且**不是**主键。
    ///
    /// 主键改成哈希之后，前端拿 `id` 当兜底会显示成 `0f6050105ab111f4`
    /// —— 用户认不出是哪个号。那是一类"能跑但没法用"的长相。
    #[test]
    fn the_label_is_the_mask_and_never_the_primary_key() {
        let sum = AccountSummary::from(sample());
        assert_eq!(sum.label, "19****01", "界面看的是掩码");
        assert_eq!(sum.id, "0f6050105ab111f4", "主键是哈希");
        assert_ne!(sum.label, sum.id, "两者不许再混成一个");
        // 昵称在时也仍然给掩码（前端自己决定优先用哪个）
        assert_eq!(sum.nickname.as_deref(), Some("小明"));
    }
}
