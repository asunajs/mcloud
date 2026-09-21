use mcloud_core::{AccountId, Error, Scope, StateStore, account_label};
use mcloud_upstream::app_client::AppProfile;
/// 一次签到的凭据。
///
/// # 为什么单独成结构，而不是两个 `&str` 参数
///
/// 与 [`crate::app_login::AppLoginContext`] 同一个理由：这三样凑齐才是"以谁的身份签到"，
/// 分开传容易在调用点被摆错顺序，而它们都是字符串，摆错了编译器不会吭声。
///
/// # 手机号还需要吗（**v7 起不需要了**）
///
/// `Authorization` 的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`。
/// 早先的写法是把它**拆开存**（只留第三段 token），用的时候再拼回去 —— 于是必须凑齐
/// 前缀与手机号，而这两样都推不出来（`accounts.id` 是掩码、`jwt_token` 里没有号码），
/// 只能由调用方每次在命令行上补（`--phone`）。那是缺口不是设计。
///
/// **v7 起登录把整串原文存进 `account_state.authorization`**，用的时候原样发出去 ——
/// 一段都不用凑，`phone` 列从此只是**标识**（打印、排障），不参与拼头。
/// 旧项目 `mcloud-sign` 一直就是这么做的（它的 `asign.json` 里 `auth` 存的就是整串），
/// 没有重新发明的必要。
///
/// 老行（v7 之前登录、库里没有整串的）退回"用 `phone` + `jwt_token` 现拼"，
/// 那条路的前缀由 [`AppProfile::authorization`] 硬编码成 `mobile` —— 见 [`Self::load`]。
///
/// ⚠ 无论走哪条路，**都不要**拿掩码值去凑这个头 —— 服务端认不认掩码是个未知数，
/// 而这类"能过但不该过"的东西一旦过了一次，就会变成一条没人敢动的隐性依赖。
pub struct SignInCredential {
    /// 完整手机号。**不参与拼头**（v7 起），只用于日志里认出"签的是谁"。
    phone: String,
    /// `Authorization` 头。含凭据，[`Debug`] 里隐去。
    authorization: String,
    /// 账号 id（掩码），用于定位库里的行。
    account_id: AccountId,
}

impl SignInCredential {
    /// 从库里已存的登录态取签到凭据（**推荐路径**，也是唯一不需要用户补参数的路）。
    ///
    /// v7 起库里存的是整串 `Authorization`，所以只要一个掩码 id 就够了 ——
    /// 手机号、前缀、token 全在里面。老行（v7 之前登录的）会退回现拼，
    /// 那时才需要 `phone` 列；该列也是 `NULL` 就报错并给出两条出路，
    /// **不会**退化成用掩码去凑 —— 见本结构的文档。
    pub async fn from_account<S: StateStore>(
        store: &S,
        scope: Scope<'_>,
        account_id: &AccountId,
    ) -> Result<Self, Error> {
        Self::load(store, scope, account_id, None).await
    }

    /// 同上，但**强制**用调用方给的手机号现拼 —— 给 `phone` 列还是 `NULL` 的老账号用。
    ///
    /// `phone` 必须是**完整手机号**。账号定位方式与登录落库时一致
    /// （同一个 [`account_label`]），所以"登录时存进去的"和"签到时要读的"必然是同一行。
    ///
    /// ⚠ 这条会**跳过**整串优先（哪怕库里存着整串），拼出来的前缀固定是 `mobile`。
    /// 所以它只是老行的逃生口，新调用方一律走 [`Self::from_account`]。
    ///
    /// ⚠⚠ **它的 `authorization()` 是拼出来的，不是拿到的** —— 调用方可以把
    /// `phone` 落库（那是用户给的事实），但**绝不要**把这个串写进
    /// `account_state.authorization`。那一列的口径是"服务端给的原文"，写进去等于
    /// 给一个本地合成物伪造来源：它会走整串优先路径、**绕过上面那道前缀闸**，
    /// 于是此后每个请求都带着一个我们自己安上去的前缀，且再也没人核对。
    /// 只记 `phone` 就够了 —— 下次照旧现拼，闸照旧生效。
    pub async fn from_phone<S: StateStore>(
        store: &S,
        scope: Scope<'_>,
        phone: &str,
    ) -> Result<Self, Error> {
        let account_id = AccountId::new(account_label(None, phone));
        Self::load(store, scope, &account_id, Some(phone)).await
    }

    /// 两条公开路径的共同实现。`phone_override` 为 `None` 时从库里读。
    async fn load<S: StateStore>(
        store: &S,
        scope: Scope<'_>,
        account_id: &AccountId,
        phone_override: Option<&str>,
    ) -> Result<Self, Error> {
        let state = store
            .get_account_state(scope, account_id)
            .await?
            .ok_or_else(|| {
                Error::BadRequest(format!(
                    "本地库里没有账号 {}。先跑一次 App 端登录：\
                     `mcloud login sms --client app`",
                    account_id.as_str()
                ))
            })?;

        // ── 首选：整串 auth（v7 起）────────────────────────────────────────
        //
        // 库里存的就是 `Basic base64("<前缀>:<手机号>:<authToken>")` 的**原文**，
        // 前缀在里面、手机号在里面、token 在里面 —— **一段都不用凑**。
        // 所以走这条路时既不需要 `phone` 列，也不需要猜前缀。
        //
        // 这也是旧项目 `mcloud-sign` 一直以来的做法（它的 `asign.json` 里
        // `auth` 存的就是这个整串），照搬即可，没有重新发明的必要。
        //
        // # 为什么它排在下面那两道闸**前面**
        //
        // 那两道闸的前提在整串这条路上**都不成立**：
        //   - **前缀闸**拦的是"我们硬编码 `mobile`、配上一个 PC 发的 token"。整串自带
        //     前缀，不存在配错的可能 —— 发出去的就是登录时拿到的那个字符串。旧项目
        //     同样不做这个区分（`initAccount` 里是 `setAuth({type:"basic", token})`，
        //     整串原样用），`pc:` 前缀的账号与 `mobile:` 的走同一条签到路径。
        //   - **token 闸**拦的是"拼不出凭据"。整串里就带着 token，不依赖 `jwt_token` 列。
        // 把它们留在前面，就会出现"库里明明有完整凭据、却被判成没有"的假阴性 ——
        // 而 v7 的迁移回填恰恰会造出这种"只有整串"的行。
        if phone_override.is_none()
            && let Some(raw) = state
                .authorization
                .as_deref()
                .filter(|value| !value.is_empty())
        {
            return Ok(Self {
                // 手机号只用于打印与排障，**不参与拼头**。整串里就带着，
                // 顺手解出来 —— 免得"库里 `phone` 是 NULL"这种纯展示问题
                // 又变成一条要用户去补数据的报错。
                phone: state
                    .phone
                    .filter(|phone| !phone.is_empty())
                    .or_else(|| {
                        mcloud_core::auth::parse_auth(raw)
                            .and_then(|info| info.phone)
                            .filter(|phone| !phone.is_empty())
                    })
                    .unwrap_or_default(),
                authorization: raw.to_string(),
                account_id: account_id.clone(),
            });
        }

        // ── 退路：老行（v7 之前）没有整串，用 `phone` + `jwt_token` 现拼 ──
        //
        // 这条路的两个代价都写在下面，**不要再往里加新调用方**：
        //   ① 必须凑齐手机号，缺了就只能让用户在命令行上补（`--phone`）；
        //   ② 前缀由 [`AppProfile::authorization`] **硬编码**成 `mobile`，
        //      它只对 App 端的 token 成立 —— 下面那道 `platform` 闸拦的就是这个。
        //
        // 新登录一律写整串，所以这条路只会随老行自然消亡。

        // 空串与 `None` 同等对待：`Some("")` 拼出来的头服务端只会回 90001，
        // 那时错误信息指向"未登录"，而真正的原因是"库里存了个空值"。
        let auth_token = state
            .jwt_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                Error::BadRequest(format!(
                    "账号 {} 没有可用的登录凭据（authToken 为空），重新登录一次",
                    account_id.as_str()
                ))
            })?;

        // 这道闸**只会新增拒绝，不会改变既有行为**：
        // 只有"能证明是另一个端登录的"才拦；`platform` 缺失（老数据）时照旧放行 ——
        // 那正是 `--phone` 这条老路径一直以来的行为，不在这里悄悄收紧。
        //
        // 拦的理由与 `app_client` 不提供 `Android` 变体同源：**这是我们没有实测过的组合**，
        // 宁可让调用方显式去重登一次，也不要拿一个"PC 发的 token 配 App 的前缀"去试探上游 ——
        // 那种失败长得像"未登录"，会把人带到凭据坏了的错误方向上。
        let platform = store
            .get_account(scope, account_id)
            .await?
            .and_then(|account| account.platform)
            .filter(|p| !p.is_empty());
        if let Some(platform) = platform
            && platform != mcloud_upstream::app_client::ios::AUTH_PREFIX
        {
            return Err(Error::BadRequest(format!(
                "账号 {} 是用 `--client {platform}` 登录的，而签到走的是 App 端凭据\
                 （`Authorization` 前缀 `{}`）。重新登录一次：`mcloud login sms --client app`",
                account_id.as_str(),
                mcloud_upstream::app_client::ios::AUTH_PREFIX
            )));
        }

        let phone = match phone_override {
            Some(phone) => phone.to_string(),
            None => state
                .phone
                .filter(|phone| !phone.is_empty())
                .ok_or_else(|| {
                    Error::BadRequest(format!(
                        "账号 {} 库里既没有整串 `Authorization`，也没有完整手机号，\
                         拼不出签到要用的凭据。两条出路：\
                         重新登录一次 `mcloud login sms --client app`（推荐，会存下整串）；\
                         或者用 `--phone <完整手机号>` 补一次（补完会记进库里，不必再输）。\
                         ⚠ 不要拿掩码 id 去凑 —— 服务端认不认是未知数。",
                        account_id.as_str()
                    ))
                })?,
        };

        Ok(Self {
            authorization: AppProfile::authorization(&phone, &auth_token),
            account_id: account_id.clone(),
            phone,
        })
    }

    /// 账号 id（掩码），可直接打印。
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 完整手机号。**含隐私**，打印前先掩码（[`account_label`]）。
    ///
    /// v7 起它不参与拼头，只是"签的是谁"的标识 —— 日志与排障用。
    pub fn phone(&self) -> &str {
        &self.phone
    }

    /// `Authorization` 头。**含凭据**，只在发请求时用。
    pub fn authorization(&self) -> &str {
        &self.authorization
    }
}

/// 手写 [`Debug`]：这个结构里有一整条可冒充登录的凭据，
/// 而 `{:?}` 是最容易在排障时顺手打出来的东西。手机号也一并掩码 ——
/// 它虽然不算凭据，但打日志时没有任何理由用完整的。
impl std::fmt::Debug for SignInCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInCredential")
            .field("account_id", &self.account_id)
            .field("phone", &account_label(None, &self.phone))
            .field("authorization", &"<已隐去>")
            .finish()
    }
}
