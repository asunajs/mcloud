//! 「直播自动定时」的两个接口（设置页那张卡的后端）。
//!
//! # 这张卡是什么（用户口径 2026-10-08）
//!
//! 用户原话：「**为直播增加一个自动设置定时的功能，设置为下次直播开始时间的一个小时后**」
//! —— 展开就是：**每个登录用户一条**定时，到点拿他挑的那几个云盘账号跑
//! **直播任务（检测口令 + 领取）**，时刻 = 这批账号里**最早**的那一场
//! 「下一场直播开始时间」+ **1 小时** ✓。
//!
//! 卡片要回答的只有两件事，所以只有两条路由：
//!
//! | 路由 | 干什么 |
//! |---|---|
//! | `GET /api/live-room/auto-schedule` | 现在算出来的是几点、开没开、作用在哪几个账号 ✓ |
//! | `PUT /api/live-room/auto-schedule` | 改开关 / 改账号多选 ⇒ **立刻重算一次**并回新状态 ✓ |
//!
//! 落盘是**那条普通定时**（`data/schedules.json` 里的 `auto-live-<owner>` ✓），
//! 不是新开的一张表 ✓ —— 用户口径：「落盘：新字段进 `data/schedules.json`
//! （既有文件），别新开表」✓。
//!
//! # 鉴权与归属
//!
//! 照 `routes/schedules.rs` 同一口径：登录闸放行后由 [`CurrentUser`] 给出
//! "我是谁" ✓，而**时刻 / 账号 / 意愿三样全都按这个 owner 取**
//! （`reschedule_live_auto(owner)` ✓）—— 别人的那条既看不到、也改不动 ✓
//!（别人的 id 与不存在一样，不泄露存在性 ✓）。
//!
//! # ⚠ 与「定时」页的关系
//!
//! 那条自动定时**同时**会出现在定时页的列表里（它是用户自己的定时 ✓，
//! 用户口径：「它是用户的定时，只是系统会继续维护它」✓），
//! 载荷里带着 `managedBy: "live-auto"` ⇒ 前端能给它加个"系统维护"的标注 ✓。
//! 用户在定时页**改它的时刻** ⇒ 下次重算会被覆盖 ✓（理由见
//! `mcloud_core::Schedule::managed_by` 的注释 ✓）；
//! 在定时页**翻它的开关** ⇒ 与这张卡上的开关是同一格
//!（`managed_wanted` ✓，见 `routes/schedules.rs` 里那段 ✓）。

use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use mcloud_app::live_auto::{read_auto_state, reschedule_live_auto, AutoLivePatch, AutoLiveState};
use mcloud_core::AccountId;

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> Router<ServerState> {
    Router::new().route("/api/live-room/auto-schedule", get(get_auto).put(put_auto))
}

/// `GET /api/live-room/auto-schedule` —— **只读**。
///
/// 它**不重算**（不写盘、不改用户那条定时 ✓），理由见
/// [`mcloud_app::live_auto::read_auto_state`] 的说明 ✓。
/// 想看最新的时刻就 `PUT` 一次，或等下一次直播任务跑完 ✓。
pub async fn get_auto(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<AutoLiveState> {
    Ok(Json(
        read_auto_state(&state.handle.store, &state.handle.clock, &cur.0).await?,
    ))
}

/// `PUT /api/live-room/auto-schedule` 的请求体。
///
/// ⚠ 两格都是 `Option`，而且语义照 [`crate::routes::schedules::absent_or_list`]
/// 那套**双 Option**（缺席 = 保持原样 ✓）：
///
/// * `enabled`：**不给** = 不动用户的意愿 ✓（前端只想改账号列表时不必带上它 ✓）；
/// * `accounts`：**不给** = 不动 ✓、**给了 `[]`（或 `null`）** = 清空
///   ⇒ 回到"全部已启用账号"（`Schedule::accounts` 的既有语义 ✓）。
///
/// 两条都是"没给 ≠ 清空"那件事的另一面：混起来的话，前端只改开关那一次
/// 会把用户挑好的账号**静默清空** ✗（定时页上已经踩过一次，见
/// `ScheduleInput::body` 的注释 ✓）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoLiveInput {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::schedules::absent_or_list")]
    pub accounts: Option<Vec<String>>,
}

/// `PUT /api/live-room/auto-schedule` —— 改开关 / 账号多选，并**立刻重算一次**。
///
/// 重算与落盘都在 [`reschedule_live_auto`] 里（唯一入口 ✓）——
/// 这条路由**不自己**拼那条定时 ✗：拼一份出来的话，"算出来的时刻"与
/// "运行收尾时算出来的时刻"就会有两套写法，迟早分叉 ✓。
///
/// ⚠ 重算会**按账号去上游现取**「下一场」（用户口径 2026-10-08 ✓）⇒ 这条请求会
/// 打几枪上游（每个账号最多 4 枪、并发、每账号一个总超时 ✓，见
/// [`mcloud_app::live_auto::UPSTREAM_FETCH_TIMEOUT`]）。它**不是**"只读一个本地文件"
/// 那种快请求了 —— 用户点一次保存多等一会儿，换来的是**现取**的时刻 ✓。
/// 上游取不到**不会**让这条路由报错：走池里那份兜底 ✓（`reschedule_live_auto` 的说明 ✓）。
pub async fn put_auto(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(input): Json<AutoLiveInput>,
) -> ApiResult<AutoLiveState> {
    // 账号必须是**自己的**。写进别人的 id 其实不会越权执行（调度器只按定时的
    // 主人过滤账号 ✓），但会让用户以为配上了 ⇒ 明确拒绝比静默丢更不容易误判 ✓
    //（与 `routes/schedules.rs` 里那条校验同一口径 ✓）。
    if let Some(accounts) = &input.accounts {
        for account in accounts {
            if state
                .handle
                .store
                .account_for(&cur.0, &AccountId::new(account.clone()))
                .await?
                .is_none()
            {
                return Err(ApiError(mcloud_core::Error::Config(format!(
                    "账号不存在或不属于你：{account}"
                ))));
            }
        }
    }
    let patch = AutoLivePatch {
        wanted: input.enabled,
        accounts: input.accounts,
    };
    Ok(Json(
        reschedule_live_auto(
            &state.handle.store,
            &state.handle.clock,
            // 按账号造客户端的那份工厂（句柄上那份 ✓）—— 重算要用**每个账号自己的票**
            // 去上游现取「下一场」✓。
            Some(&state.handle.http_for),
            &cur.0,
            Some(patch),
        )
        .await?,
    ))
}
