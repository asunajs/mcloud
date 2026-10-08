//! 推送规则路由（2026-10-08 推翻重做后的形状，见 `mcloud_core::push` 模块头）。
//!
//! # 三层收成两层（用户口径）
//!
//! > 「我设置一个推送规则，**这个规则配置推送的 token，包括推送哪几个平台**」
//! > 「**不是规则去命中行为，而是行为选择规则，一个行为可以选择多个规则**」
//!
//! ⇒
//!
//! | 层 | 回答什么 | 落在哪 |
//! |---|---|---|
//! | ① **规则**（本文件 `/api/push/rules` ✓） | **怎么发**（平台 + 凭据 + 标题 + 等级 + 汇总） | [`PushRule`]（自带一份 [`PushConfig`] ✓） |
//! | ② **行为** | **什么时候发** | `Schedule::push_rules` / `Pipeline::push_rules`（在 `schedules.rs` / `pipelines.rs` ✓）＋ [`PanelBindings`]（本文件 `/api/push/panel` ✓） |
//!
//! ⚠ **用户级"渠道配置"那一层整个没了** ✗：`GET/PUT /api/push/config` 与
//! `get_config` / `put_config` 两个 handler 一起删掉 ✓ —— 凭据在规则里 ✓，
//! 再留一层"用户级渠道"就一定会出现"两份 token 对不上"✓。
//!
//! 这一层只管**规则的增删改查**与三张清单（事件 / 渠道 / 面板绑定）。真正的
//! 过滤与投递在 `mcloud-app::notify` 那条线上 —— 本文件一行 IO 都不做，
//! 与 `pipelines.rs` 同一个形状 ✓。

use axum::extract::{Extension, Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use mcloud_core::push::{
    BehaviorKey, DigestMode, PanelBindings, PushChannelKey, PushConfig, PushEvent, PushRule,
    PushRuleId,
};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> Router<ServerState> {
    Router::new()
        .route("/api/push/rules", get(list))
        .route("/api/push/rules/{id}", put(upsert).delete(remove))
        // 面板行为（登录 / 性能）**没有定时/编排可绑** ✓ ⇒ 单独一份绑定 ✓
        // （用户口径：「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的入口」✓）。
        .route("/api/push/panel", get(get_panel).put(put_panel))
        // 两张清单**服务端算**，不让前端各写一份字符串：事件键写错一个字母
        // 不会报错，只会「静默地永不触发」✗（`PushEvent` 的类型说明里写着这条）。
        .route("/api/push/events", get(events))
        .route("/api/push/channels", get(channels))
        .route("/api/push/test", post(test_send))
}

/// `GET /api/push/rules` —— 当前用户的规则列表。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<Vec<PushRule>> {
    // 只列自己的：规则是"我想什么情况下被叫醒"，不是实例级资源 ✓。
    Ok(Json(state.handle.store.push_rules(&cur.0).await?))
}

/// `PUT /api/push/rules/{id}` 的入参。
///
/// ⚠ 字段与 [`PushRule`] 对齐，但**不含 `id` / `owner`**：`id` 在路径里 ✓，
/// `owner` 由服务端定（沿用老行的、否则写当前用户 ✓）—— 让请求体带 owner
/// 就等于让任何一个登录用户把规则塞给别人 ✗。
///
/// ⚠⚠ **没有** `events` / `channels` / `slowFactor` / `failStreak` ✗（2026-10-08
/// 推翻重做时删掉 ✓）—— 前两个是"什么时候发"（归行为 ✓），后两个是"怎么算异常"
/// （归 `PanelBindings` 的 `slowFactor` / `failStreak` ✓）。留在这里的话，
/// 请求体里就存在两个**永远不生效**的字段 ✗（用户配了、没人读 ✓）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRuleInput {
    pub name: String,
    /// ⭐ 这条规则自己的**平台与凭据**（`PushConfig` 的 10 个渠道 ✓）。
    ///
    /// ⚠ `#[serde(default)]` 而不是必填：缺这一格时落到**空配置** ✓，
    /// 再被 `validate()` 以"没有任何渠道"拒掉 ⇒ 错误信息说得出**为什么**
    /// （"你一个渠道都没配" ✓），而不是 serde 那句"missing field `config`" ✓。
    #[serde(default)]
    pub config: PushConfig,
    /// 汇总方式：一次运行一条 / 一步一条 ✓。
    #[serde(default)]
    pub digest: DigestMode,
    // ⚠ 这里原先有 `quiet: Option<QuietHours>` —— 2026-10-08 用户口径「不要静默时间」
    // ⇒ **整块删掉** ✗（字段、类型、投递层那一句、界面两格时间一起删 ✓）。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// `PUT /api/push/rules/{id}` —— 建或改（id 在路径里，幂等）。
pub async fn upsert(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(input): Json<PushRuleInput>,
) -> ApiResult<PushRule> {
    if input.name.trim().is_empty() {
        return Err(ApiError(mcloud_core::Error::Config("名称不能为空".into())));
    }
    let id = PushRuleId::new(id);
    // 归属：只认「自己的」那一行；别人的 id 与不存在一样回 404。
    let existing = state.handle.store.push_rule(&cur.0, &id).await?;
    if existing.is_none()
        && state
            .handle
            .store
            .all_push_rules()
            .await?
            .iter()
            .any(|r| r.id == id)
    {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "推送规则不存在：{id}"
        ))));
    }
    // 老行沿用它的 owner（本来就等于当前用户；单用户下可能是刚回填的 None，
    // 这时补成当前用户，顺手把归属钉死）；全新的一行归当前用户。
    let owner = existing
        .as_ref()
        .and_then(|r| r.owner.clone())
        .or_else(|| Some(cur.0.clone()));
    let rule = PushRule {
        id,
        name: input.name,
        config: input.config,
        digest: input.digest,
        enabled: input.enabled,
        owner,
    };
    // ⚠ 校验在**存之前**：`validate()` 拦的是"渠道里少 token / 一个渠道都没有"
    // 这类错 ✓ —— 存进去之后投递层只会 warn 一句"发不出去" ✗，
    // 而用户看到的是"我明明配了" ✗（错误信息只说路径、不带 token ✓）。
    rule.validate()?;
    // ⚠ 存进的是**当前用户那一格**（`push_rules` 按 owner 分表 ✓）——
    // 不能按 `rule.owner` 存：那样"老行 owner 是别人"时会把别人的行改掉 ✗。
    state.handle.store.save_push_rule(&cur.0, &rule).await?;
    Ok(Json(rule))
}

/// `DELETE /api/push/rules/{id}` —— 删。
///
/// 别人的 id 与不存在**一样回 404**：既不泄露"存在这么一条"，
/// 也不会让"删别人的"这件事看起来成功过 ✗（幂等 200 会骗人）。
pub async fn remove(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let id = PushRuleId::new(id);
    if state.handle.store.push_rule(&cur.0, &id).await?.is_none() {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "推送规则不存在：{id}"
        ))));
    }
    state.handle.store.remove_push_rule(&cur.0, &id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// `GET /api/push/panel` —— 当前用户的**面板行为绑定**。
///
/// # ⚠ 没配过 ⇒ **回内置保守默认**，不是 `null` ✗
///
/// 四个行为各自有一份保守默认（登录 `every/1800/10`、性能 `onChange/1800/10` ✓，
/// 见 `BehaviorKey::default_throttle` ✓）—— 那是**用户口径**定的：
/// 「登录，性能，这些可能**频繁触发**，注意」✓。
///
/// 回 `null` 的话前端就得自己造一份默认 ✗ ⇒ 两处默认一旦漂移，
/// 表现是"界面显示的节流值不是真正生效的那个" ✗（不报错、也查不出来 ✓）。
/// `UserStore::push_panel` 的默认实现本来就回 `PanelBindings::default()` ✓，
/// 这里原样透出即可 ✓。
pub async fn get_panel(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<PanelBindings> {
    Ok(Json(state.handle.store.push_panel(&cur.0).await?))
}

/// `PUT /api/push/panel` —— **整份替换**当前用户的面板绑定 ✓，回同一份 ✓。
///
/// # 为什么要校验规则 id（用户口径）
///
/// 用户说的是「行为选择规则」✓ ⇒ 请求体里点名的每条规则都必须是
/// **当前用户自己的** ✓。不校验的后果：任何一个登录用户能把**别人的规则 id**
/// 绑到自己的行为上 ✗ —— 投递侧按 owner 查不到那条规则，表现是"配了但永远不发" ✗
/// （不报错 ✓，正是最难查的一类 ✓）。
pub async fn put_panel(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(input): Json<PanelBindings>,
) -> ApiResult<PanelBindings> {
    // 四个具名格走 `BehaviorKey::ALL` 取（不手抄字段名：抄漏一格的表现是
    // 那一格不校验 ✗）。
    let mut wanted: Vec<&PushRuleId> = Vec::new();
    for key in BehaviorKey::ALL {
        for id in &input.get(key).rules {
            // 去重只为少查几次库（同一批 id 在四个格里各出现一次是常态 ✓）。
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
    }
    for id in wanted {
        if state.handle.store.push_rule(&cur.0, id).await?.is_none() {
            return Err(ApiError(mcloud_core::Error::Config(format!(
                "推送规则不存在或不属于你：{id}"
            ))));
        }
    }
    // ⚠ 阈值校验在**存之前**（`PanelBindings::validate` ✓，与 CLI 共用同一份 ✓）：
    // `slowFactor: 0` 会让「耗时异常」的判据**永远成立** ✗ ⇒ 用户设一次 0，
    // 从此每次运行都推一条"耗时异常"（而界面上那一格看起来完全正常 ✓）。
    // 拦在入口而不是判据那边：判据读的是**已经存下来的**值 ✓，
    // 到那儿再挡就等于"存了一个永远不生效的值" ✗（用户以为生效了 ✓）。
    input.validate()?;
    state.handle.store.save_push_panel(&cur.0, &input).await?;
    Ok(Json(input))
}

/// 事件清单的一项（界面多选用）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushEventOption {
    /// serde 的 camelCase 名 —— 落进 `Schedule::push_events` 里就是它。
    pub key: String,
    pub label: &'static str,
    /// ⭐ **分组用**：`"run"`（运行类：失败 / 部分成功 / 成功 / 取消 ✓）或
    /// `"panel"`（面板类：登录新设备 / 登录失败 / 耗时异常 / 连续失败 ✓）。
    ///
    /// # 为什么服务端给而不是前端按 key 硬编
    ///
    /// 前端要靠它分组 ✓；让前端自己维护一张"哪些 key 属于面板"的表，
    /// 加事件时两边一定分叉 ✗（表现是某个事件落到错误的分组里，不报错 ✓）。
    /// 这里由**枚举本身**分（`match` 穷尽 ✓ —— 加变体时编译器会点名 ✓）。
    ///
    /// ⚠ 面板类那四个**不进** `Schedule::push_events` ✓（它们的选择落在
    /// `/api/push/panel` 的四个具名格 ✓）—— 列出来是给界面"设置里的面板行为"
    /// 那一块对名字用的 ✓。
    pub kind: &'static str,
    /// 新建定时/编排时默认选上（[`PushEvent::default_selected`]）。
    pub default_selected: bool,
}

/// `GET /api/push/events`。
pub async fn events() -> ApiResult<Vec<PushEventOption>> {
    let defaults = PushEvent::default_selected();
    Ok(Json(
        PushEvent::ALL
            .iter()
            .map(|e| PushEventOption {
                key: event_key(*e),
                label: e.label(),
                kind: event_kind(*e),
                default_selected: defaults.contains(e),
            })
            .collect(),
    ))
}

/// 事件属于哪一组（`"run"` / `"panel"` ✓）。
///
/// ⚠ 用 `match` 穷尽而不是"看 key 的前缀"：前缀法在改名/加变体时**不报错** ✗，
/// 只会让某个事件悄悄落到错误的分组里 ✓（本仓最忌讳的那种形态 ✓）。
fn event_kind(event: PushEvent) -> &'static str {
    match event {
        PushEvent::RunFailed
        | PushEvent::RunPartial
        | PushEvent::RunOk
        | PushEvent::RunCancelled => "run",
        PushEvent::LoginNew
        | PushEvent::LoginFailed
        | PushEvent::PerfSlow
        | PushEvent::PerfFailStreak => "panel",
    }
}

/// 渠道清单的一项。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushChannelOption {
    pub key: String,
    pub label: &'static str,
}

/// `GET /api/push/channels`。
pub async fn channels() -> ApiResult<Vec<PushChannelOption>> {
    Ok(Json(
        PushChannelKey::ALL
            .iter()
            .map(|c| PushChannelOption {
                key: channel_key(*c),
                label: c.label(),
            })
            .collect(),
    ))
}

/// 取枚举的**落盘名**（serde 的 camelCase）。
///
/// ⚠ 不手写字符串：手写的那份在改名/加变体时不会报错，只会让界面选的键与
/// 规则里存的键对不上 ⇒ 永不触发 ✗。
fn event_key(event: PushEvent) -> String {
    match serde_json::to_value(event) {
        Ok(serde_json::Value::String(s)) => s,
        // 到不了这儿（unit 变体 + camelCase）。真到了也只可能是 serde 实现变了 ——
        // 记一条 warn 而不是 panic：一个清单项少了，好过整个路由 500 ✗。
        other => {
            tracing::warn!(?other, "PushEvent 序列化结果不是字符串");
            String::new()
        }
    }
}

fn channel_key(channel: PushChannelKey) -> String {
    match serde_json::to_value(channel) {
        Ok(serde_json::Value::String(s)) => s,
        other => {
            tracing::warn!(?other, "PushChannelKey 序列化结果不是字符串");
            String::new()
        }
    }
}

/// `POST /api/push/test` 的入参。
///
/// ⚠ 形状是 `{ "ruleId": "…" }` ✓ —— 旧形状 `{ "account": … }` 是**用户级渠道配置**
/// 那个时代的 ✗（那一层整个删了 ✓，见模块头）。语义：**用那条规则自己的平台与
/// 凭据**发一条测试消息 ✓。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestPushInput {
    /// 试发哪一条规则。
    ///
    /// ⚠ `Option` + 自己回 400，而不是让它必填：缺这一格时 axum 的 `Json` 提取会
    /// 甩一句 serde 的英文（`missing field ruleId` ✗），而用户要的是"该给哪个字段" ✓
    ///（与 [`PushRuleInput::config`] 用 `#[serde(default)]` 是同一条口径 ✓）。
    #[serde(default)]
    pub rule_id: Option<String>,
}

/// `POST /api/push/test` —— 用**某条规则自己的**平台与凭据发一条测试消息 ✓。
///
/// # 语义（2026-10-08 新模型）
///
/// 请求体只点名一条规则 ✓，发送走 `mcloud-app::notify` 的
/// [`Notifier::send_test_rule`] ✓ —— 与运行类/面板类**同一份** `send_notify` ✓，
/// 于是"试发通了"与"真发送会通"是同一件事 ✓（两套实现迟早分叉 ✗）。
///
/// # ⚠ 这是**对外发消息**的动作
///
/// 只在**用户显式调这个接口**时发 ✓ —— 任何自动路径（运行收尾 / 面板触发）
/// 都不许借它发 ✗。用户点"测试"要的就是"现在就发一条给我看看" ✓。
///
/// # 三种回法
///
/// | 情况 | 码 |
/// |---|---|
/// | 规则不存在 / **不属于当前用户** | **404**（别人的 id 与不存在一样，不泄露存在性 ✓） |
/// | 规则一个渠道都没配 | **400**（错误原话来自 [`PushConfig::validate`] ✓） |
/// | 发了 | **200** + 每个渠道这一次的结果（`logs` ✓） |
///
/// ⚠ 200 **不等于"发成功了"**：每个渠道的成败都在 `logs` 里（与运行类推送同一套
/// 判据 ✓）。回一个笼统的 `ok: true` 会让"我配的渠道到底通不通"永远答不出来 ✗
/// —— 本仓最忌讳的「不发错、不报错，只是东西不对」正是那种形态 ✓。
///
/// [`Notifier::send_test_rule`]: mcloud_app::notify::Notifier::send_test_rule
pub async fn test_send(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(input): Json<TestPushInput>,
) -> ApiResult<serde_json::Value> {
    let Some(raw) = input.rule_id.filter(|s| !s.trim().is_empty()) else {
        return Err(ApiError(mcloud_core::Error::Config(
            "要给规则 id：{ \"ruleId\": \"…\" }".into(),
        )));
    };
    let id = PushRuleId::new(raw);
    // 按**当前用户**取：别人的 id 与不存在一样回 404 ✓（与 [`remove`] 同一条口径 ✓）。
    let Some(rule) = state.handle.store.push_rule(&cur.0, &id).await? else {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "推送规则不存在：{id}"
        ))));
    };
    // ⚠ 校验在**发之前**：一个渠道都没配 ⇒ 一个包都发不出去 ⇒ 400（错误原话照
    // `validate()` 透出 ✓，里面有「推送配置里没有任何渠道」这句可操作的话 ✓）。
    rule.config.validate()?;

    // 标题以「[测试] 」开头 ✓（用户一眼能认出它不是某次运行的结果 ✓），
    // 正文说清**来自哪条规则** ✓（同一个人可能配了好几条 ✓）。
    let title = format!("[测试] {}", rule.name);
    let text = format!(
        "这是一条测试推送，来自规则「{}」（id {id}）。\n\
         \n\
         收到它就说明这条规则的平台与凭据是通的；它不是任何一次运行的结果，可以忽略。",
        rule.name
    );
    let logs = state
        .handle
        .pushes
        .send_test_rule(&rule, &title, &text)
        .await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "ruleId": id.to_string(),
        "title": title,
        "text": text,
        "logs": logs,
    })))
}
