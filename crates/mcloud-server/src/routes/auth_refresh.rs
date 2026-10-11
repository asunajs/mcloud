//! 「凭据刷新」的四个接口：**手动一键刷新** + **自动定时的开关与预设时间**。
//!
//! # 用户口径（2026-10-11）
//!
//! 原话：「**刷新auth，1. 可手动一键刷新 2. 自动设置定时，明天检查是否到达预设刷新时间，
//! 到了就刷**」✓ ⇒ 本文件正好是那两件事的 HTTP 面：
//!
//! | 路由 | 干什么 |
//! |---|---|
//! | `POST /api/auth/refresh` | **手动一键刷新**（一个账号 / 我名下全部 ✓）|
//! | `GET /api/auth/refresh-schedule` | 那条自动定时现在是什么状态（**只读** ✓）|
//! | `PUT /api/auth/refresh-schedule` | 改开关 / 改预设时间 ⇒ **立刻重算**并回新状态 ✓ |
//!
//! ⚠⚠ **手动 = 强制**（2026-10-11 第二轮用户口径：「**点击后强制刷新，不检测时间**」✓）：
//! 界面上那两颗手动按钮（每账号的「刷新」＋ 工具行的「刷新全部凭据」）**显式传
//! `force: true`** ⇒ 未到提前量也**真换新**（不看还剩多少天 ✓），**不走**
//! "剩余不足 `refresh_token_days` 天"那个检测 ✗。缺省仍是 `false`（见
//! [`RefreshInput::force`] ✓），**自动定时那条路一个字节没动** ✓（它直接调
//! `AuthRefreshService::refresh` ✓，`force = false` ⇒ 到点只是"**检查**" ✓）。
//!
//! # ⚠ 判据与上游调用**一个字都不改**（那是既有正确逻辑 ✓）
//!
//! 刷新走的是 [`AuthRefreshService`]（四个触发面共用同一份 ✓，见它的模块头 ✓）。
//! 本文件只做三件事：**归属校验** → **调它** → **把结局说给人听** ✓。
//! 出现 `expire - now < …` 就是放错了层 ✗。
//!
//! # 鉴权与归属
//!
//! 照 `routes/accounts.rs` 同一口径：登录闸放行后由 [`CurrentUser`] 给出"我是谁" ✓。
//! * 手动刷新：**只刷自己的账号** ✓ —— 别人的账号回 **404**（不泄露存在性 ✓，
//!   与 `accounts::owned_account` 同一条处置 ✓）；
//! * 定时：`owner` 就是当前用户 ✓（别人的那条既看不到、也改不动 ✓）。
//!
//! # ⚠⚠ 预设钟点是**每用户一个**（2026-10-11 用户口径：「**要每用户一个钟点**」✓）
//!
//! 它按 owner 存在 `data/auth-refresh.json` 里（不是 `data/settings.json` 那一格 ✗ ——
//! 那一格是**实例级默认值**，见 `mcloud_core::auth_refresh::effective_at` ✓）。
//! 于是这条路由上的"别人的钟点"：
//!
//! * `GET` 永远只回**自己**的 ✓（连读都读不到别人的 ✓）；
//! * `PUT` 只写**自己**那一格 ✓，且**没有任何"给谁"的字段** ⇒ 想指也指不到别人的 ✗；
//! * 万一有人硬塞一个 `owner`（脚本作者的直觉写法 ✓）：**不是你自己就 404** ✓ ——
//!   静默把它当"改自己"会让人以为改到了别人 ✓（与 `accounts::owned_account`
//!   同一条处置：别人的与不存在的回**同一个** 404，不泄露存在性 ✓）。
//!
//! ⚠ 还有一条**真正的**越界面在这里堵掉了：那条定时的 id 是
//! `auto-auth-refresh-<owner>`，而 `ScheduleId` **全局唯一**、`save_schedule` 又是
//! **按 id 全局覆盖**的 ⇒ 查的时候若按 `(owner, id)` 查，别人名下那条同名定时会被
//! 就地覆盖 ✗。判据现在**按 id 全局查** ✓（`mcloud_app::auth_refresh_auto` ✓），
//! 不是自己那条就一个字节都不碰 ✓（有测试钉住 ✓）。

use axum::extract::{Extension, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use mcloud_app::auth_refresh_auto::{
    read_auth_refresh_state, reschedule_auth_refresh, AuthRefreshPatch, AuthRefreshState,
};
use mcloud_app::RefreshOutcome;
use mcloud_core::{AccountId, Error};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> Router<ServerState> {
    Router::new()
        .route("/api/auth/refresh", post(refresh))
        .route(
            "/api/auth/refresh-schedule",
            get(get_schedule).put(put_schedule),
        )
}

// ════════════ ① 手动一键刷新 ════════════

/// `POST /api/auth/refresh` 的请求体。
///
/// ⚠ 目标两个字段**恰好给一个**：
/// * `{"account": "<id>"}` ⇒ 刷这一个 ✓；
/// * `{"all": true}` ⇒ 刷**我名下的全部** ✓（空表 = 没有账号可刷，回空结果 ✓）。
///
/// 两个都不给 / 两个都给 ⇒ **400**（与 CLI 的 `auth refresh` 同一条口径 ✓：
/// "不默认「全部」—— 那是个会改凭据的动作" ✓）。
///
/// # `force`：手动那条路要**强制**（2026-10-11 用户口径）
///
/// 原话：「**点击后强制刷新，不检测时间**」✓ ⇒ 手动点「刷新」/「刷新全部凭据」
/// 要**立刻换新**、**不走**"剩余不足 `refresh_token_days` 天"那个检测 ✗
///（旧行为：天数还够 ⇒ `skipped`、一枪都不打 ⇒ 界面表现成"点了没反应" ✗）。
///
/// ⚠ **缺省 `false`**（显式传 `true` 才强制 ✓）。理由两条：
///
/// 1. 别的调用方（脚本、将来的批量口子）**不传就照旧**走天数判据 ✓ ——
///    缺省 `true` 会让"我只是想刷一下看看"的调用**真打一次上游、换掉本地那一串** ✗；
/// 2. 那条**自动定时**的路根本不经过这里（它直接调 [`AuthRefreshService::refresh`] ✓，
///    见 `mcloud_app::auth_refresh_auto` ✓）⇒ 这一格不会把它带成强制 ✓，
///    但缺省值的取向仍要与它一致：**"不传" = "按判据来"** ✓。
#[derive(Debug, Deserialize)]
pub struct RefreshInput {
    /// 要刷的那个账号 id。
    #[serde(default)]
    pub account: Option<String>,
    /// 刷我名下的全部账号。
    #[serde(default)]
    pub all: Option<bool>,
    /// **强制刷**（越过 `not-yet`）：手动那两颗按钮显式传 `true` ✓，缺省 `false` ✓。
    ///
    /// ⚠ 它**越不过** `ManualOnly`（库里没有有效的过期时刻 ⇒ 打了上游也注定失败 ✓，
    /// 见 [`AuthRefreshService::refresh`] ✓）—— 那一档照旧回 `kind: "manual"` ✓。
    #[serde(default)]
    pub force: Option<bool>,
}

/// 一个账号的刷新结局（**照 [`RefreshOutcome`] 的真实形状** ✓）。
///
/// ⚠ `kind` 是**四档**，不是"成功/失败"两档 —— 与 `RefreshOutcome` 那三档
/// 加"上游失败"那一档一一对应 ✓：
///
/// | `kind` | 含义 | 界面该说什么 |
/// |---|---|---|
/// | `refreshed` | 换了新凭据 ✓ | "已换新凭据（新过期 …）" |
/// | `skipped` | 还没到提前量 ⇒ **一枪都没打** ✓ | "还剩 N 天，未到提前量" |
/// | `manual` | 库里没有有效的过期时刻 ⇒ **要重新登录** ✓ | "请重新登录该账号" |
/// | `failed` | 上游失败（库里**原样不动** ✓）| 错误原文 |
///
/// ⚠ `skipped` 与 `manual` **必须分开**：合成"没刷"的话，用户看到的就是
/// "没刷"而不知道**要不要管** ✓（前者什么都不用做，后者要重新登录 ✓）。
///
/// ⚠ 2026-10-11 起手动那条路**一律 `force`**（用户口径：「点击后强制刷新，
/// 不检测时间」✓）⇒ 从这两颗按钮点出来时 **`skipped` 不会再出现** ✓
///（天数不够也真刷 ⇒ 走 `refreshed` ✓）。这一档留着是为了**接口本身的完整**：
/// 不传 `force` 的调用方（脚本 ✓）照旧会拿到它 ✓。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResultView {
    /// 账号主键。
    pub account: String,
    /// **给界面看的名字**（有手机号就是掩码 ✓，认不出主键那串十六进制 ✓）。
    pub label: String,
    /// 四档之一（见上面那张表 ✓）。
    pub kind: &'static str,
    /// `kind == "skipped"` 时有值：还剩多少天 ✓。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_days: Option<f64>,
    /// `kind == "refreshed"` 时有值：新的过期时刻（epoch 毫秒 ✓）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_expire_ms: Option<i64>,
    /// `kind == "refreshed"` 时有值：这次是不是**越过了 `not-yet`** ✓。
    ///
    /// ⚠ 手动那两颗按钮传 `force: true` ⇒ 未到提前量也真刷 ⇒ 这里就是 `true` ✓
    ///（界面照它把话说成"**已强制换新**"，而不是"提前量生效过" ✓ ——
    /// 同一句"已换新凭据"不区分这两件事，会让人以为判据起过作用 ✓，
    /// 与 CLI 的 `--force` 那一支同一条处置 ✓）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forced: Option<bool>,
    /// `kind == "manual"` 时有值：库里那个（无效的）过期时刻 ✓。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_ms: Option<i64>,
    /// `kind == "failed"` 时有值：错误原文 ✓。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/auth/refresh` 的响应体：**逐账号结局** + 一句汇总 ✓。
///
/// # ⚠ 为什么汇总里 `failed` 要单列
///
/// 界面要能一眼说"3 个里 1 个失败"✓；而**失败那一档**与另外三档的下一步
/// 完全不同（见 [`RefreshResultView::kind`] 那张表 ✓）—— 混成一个"没刷成"的
/// 数字会让用户不知道该不该管 ✓。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    /// 逐账号结局（顺序 = 目标账号的顺序 ✓）。
    pub results: Vec<RefreshResultView>,
    /// 换了新凭据的个数 ✓。
    pub refreshed: usize,
    /// 未到提前量、**没打上游**的个数 ✓。
    pub skipped: usize,
    /// 需人工（重新登录）的个数 ✓。
    pub manual: usize,
    /// 上游失败的个数 ✓。
    pub failed: usize,
}

/// `POST /api/auth/refresh` —— **手动一键刷新**。
///
/// # ⚠ 手动 = **强制**（2026-10-11 用户口径：「点击后强制刷新，不检测时间」✓）
///
/// 前端那两颗手动按钮（每账号的「刷新」＋ 工具行的「刷新全部凭据」）**显式传
/// `force: true`** ✓ ⇒ 未到提前量也**真打上游、真换新串** ✓（不看还剩多少天 ✓）。
///
/// ⚠ **只改手动这一条** ✗：那条自动定时（`mcloud_app::auth_refresh_auto`）与
/// CLI 的默认行为都**不经过这里** ✓，它们仍是"到点**检查**、该刷才刷" ✓
/// （`force = false` ✓）。这一格缺省也是 `false` ✓（见 [`RefreshInput::force`] ✓）。
///
/// # 返回 200 还是 502
///
/// * 有账号**刷新失败**（上游那档）⇒ **502**（`Error::Network` 的既有映射 ✓）——
///   界面上那是一条"该去看一眼"的提示，不该混在"都好了"里 ✓；
/// * 其余（含"未到提前量跳过"与"需人工"）⇒ **200** ✓：
///   前者是**正常结局**，后者要用户去重新登录（不是这次请求的失败 ✓）。
///
/// ⚠ "需人工"（`ManualOnly`）**不算这次请求失败**：它的下一步是"去重新登录"，
/// 而 502 会让界面说"服务器出错了"，把人引到错的方向 ✓。
///
/// # ⚠ 它可能**慢**（每个账号最慢约 28 秒，实测 ✓）
///
/// 上游实测最慢一次 28.4 秒（见 `mcloud_app::auth_refresh` 模块头 ✓）。
/// 界面那边必须给"进行中"状态 ✓；服务端这边**不设**额外超时 ——
/// 掐掉它不会让上游那边少换一串新的（那一枪已经打出去了 ✓），只会让用户看到"失败"
/// 而凭据其实已经换了 ✓。
pub async fn refresh(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(input): Json<RefreshInput>,
) -> ApiResult<RefreshResponse> {
    let all = input.all.unwrap_or(false);
    // ⚠ 缺省 `false`（**显式传才强制** ✓）—— 见 `RefreshInput::force` 那两条理由 ✓。
    let force = input.force.unwrap_or(false);
    match (&input.account, all) {
        (None, false) => {
            return Err(ApiError(Error::Config(
                "要给 `account`（刷哪一个）或 `all: true`（刷全部）\
                 —— 不默认「全部」，那是个会改凭据的动作"
                    .into(),
            )))
        }
        (Some(_), true) => {
            return Err(ApiError(Error::Config(
                "`account` 与 `all` 不能同时给".into(),
            )))
        }
        _ => {}
    }

    // ── 归属校验（**硬边界** ✓）────────────────────────────────────
    //
    // ⚠ 用 `account_for`（**不看**不带过滤的 `account()` ✓）：
    // 别人的账号与不存在的账号回**同一个** 404（不写"不属于你" ✓）——
    // 差别本身就是"这个 id 存不存在"的信息 ✓（与 `accounts::owned_account` 同口径 ✓）。
    let targets = match &input.account {
        Some(id) => {
            let account_id = AccountId::new(id.clone());
            match state.handle.store.account_for(&cur.0, &account_id).await? {
                Some(a) => vec![a],
                None => {
                    return Err(ApiError(Error::NotFound(format!("账号不存在：{id}"))));
                }
            }
        }
        None => state.handle.store.accounts_for(&cur.0).await?,
    };

    let mut views = Vec::with_capacity(targets.len());
    let (mut n_refreshed, mut n_skipped, mut n_manual, mut n_failed) =
        (0usize, 0usize, 0usize, 0usize);
    for account in &targets {
        let label =
            mcloud_upstream::account_label(account.nickname.as_deref(), &account.auth.phone);
        // 提前量走**账号配置**（`refresh_token_days` ✓）；`force` 来自请求体
        //（手动那两颗按钮显式传 `true` ⇒ 未到提前量也真刷 ✓，见 `refresh` 的说明 ✓）。
        let view = match state
            .handle
            .auth_refresh
            .refresh(account, None, force)
            .await
        {
            Ok(RefreshOutcome::Rotated {
                new_expire_ms,
                forced,
            }) => {
                n_refreshed += 1;
                RefreshResultView {
                    account: account.id.to_string(),
                    label,
                    kind: "refreshed",
                    remaining_days: None,
                    new_expire_ms: Some(new_expire_ms),
                    forced: Some(forced),
                    expire_ms: None,
                    error: None,
                }
            }
            Ok(RefreshOutcome::Skipped { remaining_days }) => {
                n_skipped += 1;
                RefreshResultView {
                    account: account.id.to_string(),
                    label,
                    kind: "skipped",
                    remaining_days: Some(remaining_days),
                    new_expire_ms: None,
                    forced: None,
                    expire_ms: None,
                    error: None,
                }
            }
            Ok(RefreshOutcome::ManualOnly { expire_ms }) => {
                n_manual += 1;
                RefreshResultView {
                    account: account.id.to_string(),
                    label,
                    kind: "manual",
                    remaining_days: None,
                    new_expire_ms: None,
                    forced: None,
                    expire_ms: Some(expire_ms),
                    error: None,
                }
            }
            Err(e) => {
                n_failed += 1;
                // 上游失败时库里**原样不动**（窄写只在拿到新串之后跑 ✓）。
                tracing::warn!(
                    account = %account.id,
                    user = %cur.0,
                    error = %e,
                    "手动刷新凭据失败"
                );
                RefreshResultView {
                    account: account.id.to_string(),
                    label,
                    kind: "failed",
                    remaining_days: None,
                    new_expire_ms: None,
                    forced: None,
                    expire_ms: None,
                    error: Some(e.to_string()),
                }
            }
        };
        views.push(view);
    }

    tracing::info!(
        user = %cur.0,
        all,
        count = views.len(),
        refreshed = n_refreshed,
        skipped = n_skipped,
        manual = n_manual,
        failed = n_failed,
        "手动刷新凭据完成"
    );

    let body = RefreshResponse {
        results: views,
        refreshed: n_refreshed,
        skipped: n_skipped,
        manual: n_manual,
        failed: n_failed,
    };

    if n_failed > 0 {
        // ⚠ **502**（而不是 200）：有账号**真的没刷成**，界面该把它当成"要去看一眼" ✓。
        // 逐条结局仍然带在 body 里 —— 于是"哪一个失败、为什么"不会因为一个状态码丢掉 ✓。
        //
        // ⚠ 用 `Error::Network` 的既有映射（502 ✓），不新造一档：
        // "上游没成"就是它 ✓（与 `mcloud_upstream` 那侧报错的形状一致 ✓）。
        return Err(ApiError(Error::Network(
            serde_json::to_string(&body).unwrap_or_else(|_| "刷新失败".into()),
        )));
    }
    Ok(Json(body))
}

// ════════════ ② 自动定时的开关与预设时间 ════════════

/// `GET /api/auth/refresh-schedule` —— **只读**（不重算、不写盘 ✓）。
///
/// 回的是**当前登录用户**的钟点：他自己设过就是他的 ✓，没设过就是实例级那一格
///（升级前那个值 ✓，见 `mcloud_core::auth_refresh::effective_at` ✓）。
pub async fn get_schedule(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<AuthRefreshState> {
    Ok(Json(
        read_auth_refresh_state(&state.handle.store, &cur.0).await?,
    ))
}

/// `PUT /api/auth/refresh-schedule` 的请求体。
///
/// ⚠ 三格都是 `Option`，语义照 `routes/schedules.rs` 那套**双 Option**
///（缺席 = 保持原样 ✓）：
///
/// * `enabled`：**不给** = 不动用户的意愿 ✓；
/// * `at`：**不给** = 不动预设时间 ✓；
/// * `owner`：**不给** = 改自己（正常路径 ✓，前端从不发它 ✓）。
///
/// 前两条都是"没给 ≠ 清空"那件事的另一面：混起来的话，前端只改时间那一次
/// 会把用户关掉的开关**静默打开** ✗（那正是用户口径里点名不许的事 ✓）。
///
/// # `owner` 是**守一道**，不是功能
///
/// 这条路由**没有**"改别人的"这条路（钟点是每用户一份、归属只来自会话 ✓）。
/// 留着这一格是为了**把误用变成明确的拒绝**：脚本作者很容易照 CLI 的
/// `--user` 直觉写一个 `{"owner": "<别人>"}` —— 不收这一格的话，那个字段会被
/// serde 忽略、请求**成功**、改的却是自己的 ✓ ⇒ 他会以为改到了别人 ✗。
/// 现在给了别人就回 **404**（与 `accounts::owned_account` 同口径：
/// 别人的与不存在的回**同一个** 404，不泄露存在性 ✓）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshScheduleInput {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub at: Option<String>,
    /// ⚠ **仅供守一道**（见上 ✓）：给了且不是你自己 ⇒ 404 ✓。前端不发它 ✓。
    #[serde(default)]
    pub owner: Option<String>,
}

/// `PUT /api/auth/refresh-schedule` —— 改开关 / 预设时间，并**立刻重算** ✓。
///
/// 重算与落盘都在 [`reschedule_auth_refresh`] 里（唯一入口 ✓）——
/// 这条路由**不自己**拼那条定时 ✗。
///
/// ⚠ 写的是**当前用户自己那一格**（`data/auth-refresh.json` ✓）与**他自己那条定时**
///（`auto-auth-refresh-<他自己>` ✓）—— 别人的钟点与定时一个字节都碰不到 ✓。
pub async fn put_schedule(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(input): Json<RefreshScheduleInput>,
) -> ApiResult<AuthRefreshState> {
    // ── 守一道：指名了别人 ⇒ 404（**不是**静默改自己 ✓，见 `RefreshScheduleInput` ✓）──
    if let Some(owner) = &input.owner {
        if owner != cur.0.as_str() {
            return Err(ApiError(Error::NotFound(format!("用户不存在：{owner}"))));
        }
    }
    let patch = AuthRefreshPatch {
        wanted: input.enabled,
        at: input.at,
    };
    Ok(Json(
        reschedule_auth_refresh(&state.handle.store, &cur.0, Some(patch)).await?,
    ))
}
