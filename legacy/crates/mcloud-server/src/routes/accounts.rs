//! 云盘账号：列表与云朵趋势。
//!
//! # 只读
//!
//! 这一批只做"看"。增删改（加账号 = 走一条登录链路、改昵称/启用状态、调顺序）
//! 都还没做，因为"加账号"其实是 P2.5 之后的上游登录流程从 HTTP 侧再走一遍 ——
//! 那是独立的一件事，不要混在"把已有数据端出去"里做半截。
//!
//! # 出网字段是**手写**的
//!
//! [`Account`] 上不加 `Serialize`（理由与 `auth.rs` 里 `user_payload` 相同）：
//! 结构体多一个 derive，就多一个"某天有人直接 `Json(account)` 把它吐出去"的机会，
//! 而那一刻的字段是随手加进来的、没人审过。这里逐字段列出**允许出网**的名字。

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::Days;
use mcloud_core::{Account, AccountId, Error, Scope, StateStore};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AppState, AuthUser};
use crate::error::{ApiError, ApiQuery};
use crate::routes::clamp_limit;

/// 云朵趋势的默认与最大天数（也就是前端那张 30 天趋势图的取数窗口）。
const DEFAULT_DAYS: usize = 30;
const MAX_DAYS: usize = 365;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/accounts", get(list_accounts))
        .route("/api/accounts/{id}/cloud-stats", get(cloud_stats))
}

/// `GET /api/accounts`
///
/// 只返回**当前会话用户**的账号（`Scope::User`）。未归属的历史账号对谁都不可见 ——
/// 那是存储层的契约，不是这里判断的（见 `mcloud_core::StateStore` 的说明）。
async fn list_accounts(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let accounts = state
        .store()
        .list_accounts(Scope::user(&auth.user.id))
        .await?;
    // "过期了没有"要用**注入的时钟**判定：直接用系统时间会让这条断言在测试里没法拨
    let now_millis = state.now().timestamp_millis();

    Ok(Json(json!({
        "accounts": accounts
            .iter()
            .map(|account| account_payload(account, now_millis))
            .collect::<Vec<_>>(),
    })))
}

/// `GET /api/accounts/:id/cloud-stats?days=30`
///
/// 云朵余额的日快照，`date` 升序（前端画趋势图直接照着走）。
async fn cloud_stats(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<DaysQuery>,
) -> Result<Json<Value>, ApiError> {
    let id = AccountId::new(id);
    let scope = Scope::user(&auth.user.id);

    // 账号不存在**或不属于你** → 404，而不是"空数组"。
    // 空数组会让三种情况长得一样：账号不属于你、账号没有历史、`days` 筛掉了全部。
    // 三者该导出完全不同的动作（换账号 / 等采集 / 调窗口），而前端无从区分。
    if state.store().get_account(scope, &id).await?.is_none() {
        return Err(ApiError::Domain(Error::NotFound(format!("云盘账号 {id}"))));
    }

    let days = clamp_limit(query.days, DEFAULT_DAYS, MAX_DAYS);
    // "今天"必须按 `Asia/Shanghai` 切分：日快照的 date 就是这个时区下的日期，
    // 用 UTC 算会让每天的窗口在早上 8 点之前整体错一天。
    let today = state
        .now()
        .with_timezone(&mcloud_core::local_tz())
        .date_naive();
    let since = (today - Days::new(days as u64))
        .format("%Y-%m-%d")
        .to_string();

    let stats = state.store().cloud_stats_since(scope, &id, &since).await?;

    Ok(Json(json!({
        "accountId": id.as_str(),
        "days": days,
        "since": since,
        // 差值（`cloudDiff` / `cloudDiffWeek`）由采集侧按 `pure::next_cloud_stats` 算好落库，
        // 这里**不重算** —— 重算一遍等于让"图上显示的数"与"库里存的数"有两个来源。
        "stats": stats
            .iter()
            .map(|s| json!({
                "date": s.date,
                "cloudCount": s.cloud_count,
                "cloudDiff": s.cloud_diff,
                "cloudDiffWeek": s.cloud_diff_week,
            }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct DaysQuery {
    days: Option<usize>,
}

/// 账号的对外形状。
///
/// 刻意**不含**：
/// - `owner` —— 那是本系统的用户 id，前端一个都用不上，出网只是多一处信息泄漏面；
/// - 任何凭据 —— 它们本来就不在 [`Account`] 里（在 `AccountState`，只有能力层读得到），
///   这里是"连结构体都不往外递"的那道双保险。
fn account_payload(account: &Account, now_millis: i64) -> Value {
    json!({
        "id": account.id.as_str(),
        // 展示名（昵称 → 脱敏手机号 → id）由领域层决定，前端不再自己挑
        "displayName": account.display_name(),
        "nickname": account.nickname,
        "phoneMasked": account.phone_masked,
        "platform": account.platform,
        "enabled": account.enabled,
        "sortOrder": account.sort_order,
        "cloudCurrent": account.cloud_current,
        "cloudUpdatedAt": account.cloud_updated_at.map(|t| t.to_rfc3339()),
        "authExpire": account.auth_expire,
        // 给结论，也给依据：前端要能显示"已过期"而不用自己再做一次毫秒比较
        "authExpired": account.is_auth_expired_at(now_millis),
        "lastOkAt": account.last_ok_at.map(|t| t.to_rfc3339()),
        "failStreak": account.fail_streak,
        "note": account.note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Clock, FixedClock};

    fn account() -> Account {
        let mut account = Account::new("acc-1");
        account.nickname = Some("昵称".to_string());
        account.phone_masked = Some("19****01".to_string());
        account.cloud_current = Some(1234);
        account.auth_expire = Some(1_000);
        account.fail_streak = 2;
        account
    }

    #[test]
    fn payload_never_carries_the_owner_or_credentials() {
        let now = FixedClock::parse("2026-09-20T00:00:00Z")
            .now()
            .timestamp_millis();
        let payload = account_payload(&account(), now);
        let object = payload.as_object().unwrap();

        for forbidden in ["owner", "jwtToken", "authorization", "phone"] {
            assert!(
                !object.contains_key(forbidden),
                "对外形状里不该有 {forbidden}: {payload}"
            );
        }
        assert_eq!(object["id"], "acc-1");
        assert_eq!(object["displayName"], "昵称");
    }

    #[test]
    fn expiry_is_reported_as_a_conclusion_not_only_a_timestamp() {
        let now = FixedClock::parse("2026-09-20T00:00:00Z")
            .now()
            .timestamp_millis();

        // auth_expire 是 epoch 毫秒（见 mcloud_core::auth）
        let expired = account_payload(&account(), 2_000);
        assert_eq!(expired["authExpired"], true);

        let alive = account_payload(&account(), 500);
        assert_eq!(alive["authExpired"], false);

        // 上面两组数字只差 4 倍，**区分不出"绝对时间戳"与"相对秒数"** ——
        // 而 `account()` 的 auth_expire 是 1_000（epoch 毫秒，1970 年）。
        // 换成真实量级的"现在"（2026）再钉一次：它必须是绝对时间戳。
        let still_expired = account_payload(&account(), now);
        assert_eq!(
            still_expired["authExpired"], true,
            "auth_expire 是绝对时间戳，不是相对秒数"
        );
    }
}
