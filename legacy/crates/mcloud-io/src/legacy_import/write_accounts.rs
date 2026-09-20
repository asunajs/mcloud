// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
use crate::store::SqliteStore;
use mcloud_core::legacy::{LegacyAccountConfig, LegacyAccountView};
use mcloud_core::{Account, AccountId, AccountState, Scope, StateStore};
use serde_json::Value;
use std::collections::BTreeMap;

// ═══════════════════════════════════════════════════════════════════════════
// 账号
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn write_accounts(
    store: &SqliteStore,
    scope: Scope<'_>,
    views: &[LegacyAccountView],
) -> Result<i64> {
    for view in views {
        let account = Account {
            id: view.account_id.clone(),
            // 导入即归属：旧数据里的账号交给执行导入的这个用户
            owner: scope.owner().cloned(),
            nickname: view.nickname.clone(),
            // 手机号非空时 AccountId 本身就是掩码，直接复用，避免两处掩码逻辑跑偏
            phone_masked: (!view.phone.is_empty()).then(|| view.account_id.as_str().to_string()),
            platform: view.platform.clone(),
            enabled: true,
            // 保持旧配置里的顺序
            sort_order: view.index as i64 - 1,
            // `0` 在旧语义里是"没有过期信息"，不是"1970 年过期"
            auth_expire: (view.auth_expire != 0).then_some(view.auth_expire),
            ..Account::new(view.account_id.as_str())
        };
        store.upsert_account(scope, &account).await?;
    }
    Ok(views.len() as i64)
}

/// 把旧 `asign.json` 里的整串 `auth`（与手机号）回填进 `account_state`。
///
/// # 为什么单独一步，而不是塞进 `write_accounts` / `write_state`
///
/// 那两处写的是**两张不同的表**（`accounts` 与 `account_state`），而且各自被
/// "源文件哈希没变就跳过"的门控着。凭据回填需要跨过那道门控 —— 理由见下。
///
/// # 为什么**不受 `source_changed` 门控**
///
/// `write_state` 里那段注释把问题说得很清楚：一条被哈希门控着的回填，对**已经导入过的
/// 库**永远不会再跑 —— 于是"同一个版本的程序"会产出两种库形态（老库 `NULL`、新库有值），
/// 而这两种形态在别处长得分毫不差，排障时根本想不到差别在这里。
///
/// 所以这里换一条判据：**按状态判**（"这一列空着就填"），不按哈希判。
/// [`mcloud_core::StateStore::fill_account_credentials`] 只填空缺、不覆盖已有值，
/// 于是它天然幂等：每次 `migrate` 都跑也无害，而"已经重登过、库里存着更新凭据"的账号
/// 不会被旧配置里的串盖回去。
///
/// # 为什么按**下标**对上号
///
/// `describe_legacy_accounts` 是 `configs.iter().enumerate().map(...)`，第 `i` 个视图的
/// `index` 恒为 `i + 1`。这里断言钉住这个不变量 —— 将来有人往那里面加过滤，应当**当场炸**，
/// 而不是悄悄把 A 账号的凭据回填到 B 账号头上。
///
/// # 什么算"能填的 `auth`"
///
/// 只有 [`mcloud_core::auth::parse_auth`] 认得的才算。旧配置里可能留着占位串或半截
/// 手改过的值，把那种东西存进去，下一次请求会带着一个格式不对的头出去，
/// **失败长得像"未登录"** —— 那正是这个项目最不想要的错误方向。
/// 存进去的是 `parse_auth` 规范化后的整串（它会补上可能缺失的 `Basic ` 前缀），
/// 而不是配置里的原文。
pub(crate) async fn fill_credentials(
    store: &SqliteStore,
    scope: Scope<'_>,
    configs: &[LegacyAccountConfig],
    views: &[LegacyAccountView],
) -> Result<i64> {
    let mut filled = 0i64;
    for (position, (config, view)) in configs.iter().zip(views).enumerate() {
        debug_assert_eq!(
            view.index,
            position + 1,
            "视图顺序必须与配置顺序一致，否则回填会张冠李戴"
        );

        let authorization = mcloud_core::auth::parse_auth(&config.auth).map(|info| info.auth);
        let phone = (!view.phone.is_empty()).then_some(view.phone.as_str());
        if authorization.is_none() && phone.is_none() {
            continue;
        }

        // 先看一眼有没有缺口。回填本身是幂等的（重复调用无害），但**报告要诚实** ——
        // 每次都报"回填了 2 个账号"，这个数字从第二次运行起就彻底失去意义了，
        // 而它恰恰是用户用来确认"我那个老库补上了吗"的唯一依据。
        //
        // 这里读一次再写没有 TOCTOU 风险：写入是"只填空缺"，结果与读到什么无关。
        let existing = store.get_account_state(scope, &view.account_id).await?;
        let has_auth = existing
            .as_ref()
            .and_then(|s| s.authorization.as_deref())
            .is_some();
        let has_phone = existing.as_ref().and_then(|s| s.phone.as_deref()).is_some();
        if (authorization.is_none() || has_auth) && (phone.is_none() || has_phone) {
            continue;
        }

        store
            .fill_account_credentials(scope, &view.account_id, authorization.as_deref(), phone)
            .await?;
        filled += 1;
    }
    Ok(filled)
}

pub(crate) async fn write_state(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    identity: &BTreeMap<String, AccountId>,
    report: &mut MigrationReport,
) -> Result<i64> {
    let Some(accounts) = value.get("accounts").and_then(Value::as_object) else {
        return Ok(0);
    };

    let mut written = 0i64;
    for (key, raw) in accounts {
        let state: mcloud_core::legacy::LegacyAccountState =
            match serde_json::from_value(raw.clone()) {
                Ok(state) => state,
                Err(err) => {
                    report.issues.push(MigrationIssue::UnparsableAccountState {
                        key: key.clone(),
                        error: err.to_string(),
                    });
                    continue;
                }
            };

        // 未知字段如实报出，而不是静默丢弃 —— 旧版本写过的东西凭什么当它不存在
        let unknown = state.unknown_fields();
        if !unknown.is_empty() {
            report.issues.push(MigrationIssue::UnknownStateFields {
                key: key.clone(),
                fields: unknown.iter().map(|f| (*f).to_string()).collect(),
            });
        }

        if state.is_empty() {
            report
                .issues
                .push(MigrationIssue::EmptyAccountState { key: key.clone() });
            continue;
        }

        // 键是旧 `accountKey`（手机号原文/昵称/"account"）。映射不到就不导入：
        // 凭空造一个账号会把"这是谁的数据"这件事变成永久的谜
        let Some(account_id) = identity.get(key) else {
            report
                .issues
                .push(MigrationIssue::OrphanAccountState { key: key.clone() });
            continue;
        };

        // `state.json` 对 `authorization` / `phone` **没有主张** —— 它本来就没有这两个字段
        //（键才是手机号原文/昵称）。而 `upsert_account_state` 是**整体覆盖**（`None` = 清空），
        // 直接传 `None` 会先把库里已有的凭据抹掉，再由 [`fill_credentials`] 从 `asign.json`
        // 补回来 —— 补回来的却是**旧配置里的串**：如果这中间用户重登过一次，
        // 新鲜凭据就被换成了旧的，而且事后完全看不出发生过什么。
        //
        // 所以这里把库里现有的值**原样带过去**：语义上就是"这次快照对这两列没有新信息"。
        // 于是回填只剩"从无到有"这一种情况，反复跑都安全。
        let existing = store.get_account_state(scope, account_id).await?;

        store
            .upsert_account_state(
                scope,
                &AccountState {
                    account_id: account_id.clone(),
                    jwt_token: state.jwt_token,
                    device_id: state.device_id,
                    user_domain_id: state.user_domain_id,
                    cloud_phone_redpack_token: state.cloud_phone_redpack_token,
                    // 从 `asign.json` 反填凭据这件事**不在这里做**：这条写入被
                    // `source_changed(state.json)` 门控着 —— 已经导入过的库不会重跑，
                    // 于是"同一版程序、两种库形态"（老库 `NULL`、新库有值），
                    // 而两种形态在别处长得一模一样。
                    //
                    // 改由 [`fill_credentials`] 在**所有文件之后**单独跑一遍：那条回填
                    // 按状态判（"这一列空着就填"）而不是按哈希判，所以老库也会被补上。
                    authorization: existing.as_ref().and_then(|s| s.authorization.clone()),
                    phone: existing.as_ref().and_then(|s| s.phone.clone()),
                    // 旧 state.json 没有时间戳字段；"什么时候导入的"看台账
                    updated_at: None,
                },
            )
            .await?;
        written += 1;
    }

    report.account_states = written as usize;
    Ok(written)
}
