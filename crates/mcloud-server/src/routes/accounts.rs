//! 云盘账号管理（列表 / 删除 / 每账号设置）。
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

use axum::extract::{Extension, Path, State};
use axum::Json;
use mcloud_core::account::AccountSettingsDto;
use mcloud_core::{Account, AccountId};
use serde::Serialize;

use crate::auth_gate::CurrentUser;
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

/// `GET /api/accounts` —— 列出**当前用户**已登录的云盘账号。
///
/// ⚠ 用 `accounts_for` 而不是 `accounts()`：后者是「库里所有账号」，
/// 在多用户实例里会把别人的账号列出来（审计 P1-6 要修的就是这个）。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<Vec<AccountSummary>> {
    let accounts = state
        .handle
        .store
        .accounts_for(&cur.0)
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
///
/// # 但"别人的账号"不是"不存在"
///
/// 归属判定插在幂等前面：账号**在、但不是自己的** ⇒ **404**
///（`docs/architecture.md`：归属错误一律 404，不泄露存在性）。
/// 只有"真的不存在"才走上面那条幂等 200 —— 否则 B 删 A 的账号会得到 200，
/// 看起来像删成功了，实际什么都没发生。
pub async fn remove(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let account_id = AccountId::new(id);

    // 自己的账号：删。
    if state
        .handle
        .store
        .account_for(&cur.0, &account_id)
        .await?
        .is_some()
    {
        state
            .handle
            .store
            .remove_account(&account_id)
            .await
            .map_err(ApiError::from)?;
        return Ok(Json(serde_json::json!({ "removed": true })));
    }

    // 账号在、但不是自己的 ⇒ **404**（不泄露存在性）；
    // 真不存在 ⇒ 仍然是上面那条幂等语义（200）—— 删除是「收敛到目标状态」，
    // 而「本来就没有」已经达成，不该让前端弹一个红色失败框。
    if state.handle.store.account(&account_id).await?.is_some() {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "账号不存在：{account_id}"
        ))));
    }
    Ok(Json(serde_json::json!({ "removed": true })))
}

/// 取**当前用户**的那个账号；拿不到（账号不存在 **或** 不是自己的）⇒ 404。
///
/// 这是本文件里"归属"的**唯一**入口：两个设置端点都走它，免得哪天有人顺手
/// 改成 `account()` —— 那会把"别人的"变成"存在"，泄露存在性。
///
/// 判据本身不在这里，也不许在这里另写一套：直接调 store 的 `account_for`，
/// 它内部用的是 `mcloud_core::owner::visible_to`（与列表 / 删除同一条）。
async fn owned_account(
    state: &ServerState,
    cur: &CurrentUser,
    id: &AccountId,
) -> Result<Account, ApiError> {
    match state.handle.store.account_for(&cur.0, id).await? {
        Some(account) => Ok(account),
        None => Err(ApiError(mcloud_core::Error::NotFound(format!(
            "账号不存在：{id}"
        )))),
    }
}

/// `GET /api/accounts/{id}/settings` —— 读**当前用户**那个账号的设置。
///
/// 别人的账号与不存在的账号回**同一个** 404（不写"不属于你"）：差别本身就是
/// "这个 id 存不存在"的信息。
///
/// 响应里**有**推送渠道的 token/secret —— 取舍见 [`put_settings`] 的
/// 「为什么 GET 回显 token/secret」。
pub async fn get_settings(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<AccountSettingsDto> {
    let account_id = AccountId::new(id);
    let account = owned_account(&state, &cur, &account_id).await?;
    Ok(Json(AccountSettingsDto::from(account.settings)))
}

/// `PUT /api/accounts/{id}/settings` —— **整体替换**该账号的设置。
///
/// # 语义：整体替换，不是合并
///
/// 请求体就是"替换成什么"，每个字段都必须显式给出（`notify: null` = 不配推送）。
/// 不做"缺席即保持"的浅合并，理由三条：
///
/// 1. **缺席兜默认值 = 静默重置**。用户只改一个开关、推送渠道却没了，两边都没有
///    任何提示 —— 而"不报错、只是东西不对"正是本仓最忌讳的一类失效。
///    必填 + 400 是**吵**的失败，比静默重置好查得多。
/// 2. **GET 回显完整对象 ⇒ 客户端本来就拿得到整份**，"读出来、改一处、写回去"
///    这条最常用的路是通的（core 的 `the_wire_format_round_trips` 钉住往返闭合）。
///    若 GET 打码，还得再发明一条"打码占位符 = 保持不变"的哨兵规则，而哨兵一旦
///    漏处理就是把占位符存成真 token。
/// 3. 与既有的写接口同形：`PUT /api/schedules/{id}` 也是"请求体即全量"，
///    它只额外保留水位线这类**服务端自己维护**的列。
///
/// # 为什么 GET 回显 token/secret（而不是打码）
///
/// 1. **写权限天然大于读**：能把设置 `PUT` 进来的会话，本就可以把 token 换成
///    自己的收件地址，从而收到这个账号的**全部运行结果**。打码读接口挡不住
///    这种会话，只挡"拿到了响应但不会发请求"的被动泄露 —— 收益很小。
/// 2. **打码在这份模型上必然漏**：`PushConfig` 的渠道配置到处是
///    `#[serde(flatten)] extra`（TS 语义：多余字段原样展开进请求体），
///    `customPost` 还带 `headers` 与任意模板体。按字段名猜敏感键只能盖住
///    几个具名字段，其余照样原样回显 —— 那是**看起来安全**，比不打码更坏。
/// 3. **往返闭合**：整体替换要求客户端能把 GET 的结果原样 PUT 回来（见上）。
/// 4. 真正被"收割"来的凭据 `auth` 依然一个字节都不出 —— 见本文件头的
///    `AccountSummary` 说明，那条边界没有变化。
///
/// 代价说清楚：**设置响应里带着第三方渠道的 token**。它只发给过了登录闸、且被
/// 归属判定认作主人的那个会话，而且`请求体不进日志`（见文件尾的日志一行）。
///
/// # 顺序：先归属、后校验
///
/// 别人的账号不该因为"请求体合法与否"得到不同回包 —— 否则拿 400/404 的差别就能
/// 探测某个 id 存不存在。所以 `account_for` 在 `from_wire` **之前**。
///
/// # ⚠ 读-改-写不是原子的（如实记录）
///
/// `save_account` 是整行覆盖，而 store 没有"只写设置"的窄写方法
///（`json_store.rs` 本批不可动）⇒ 并发的另一次整行写（例如 `auth_refresh`
/// 刷新凭据）理论上会被这次覆盖掉。窗口是"读完到写回"这两次 await 之间
///（其中没有任何别的 await）。这是"没有窄写方法"的既有代价，不是本次新增的。
pub async fn put_settings(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<AccountSettingsDto> {
    let account_id = AccountId::new(id);
    // 归属先于校验：别人的账号与不存在的账号一样 404，且与请求体是否合法无关。
    let mut account = owned_account(&state, &cur, &account_id).await?;
    // 严格解析 + 范围 / 渠道结构校验；不合法一律 `Error::Config` ⇒ 400。
    account.settings = AccountSettingsDto::from_wire(body)?;
    state.handle.store.save_account(&account).await?;
    // 只记"谁改了哪个账号"，**不记设置内容**（里面有 token/secret）。
    tracing::info!(account = %account_id, user = %cur.0, "账号设置已更新");
    Ok(Json(AccountSettingsDto::from(account.settings)))
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
            owner: None,
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
