//! WebUI 登录闸：首次初始化、登录、登出、当前会话。
//!
//! # 这是**另一层**认证，别与账号登录混
//!
//! | | 谁的凭证 | 存哪 | 谁用 |
//! |---|---|---|---|
//! | 系统用户（本模块） | 你设的用户名/口令 | `users.json` | 浏览器 |
//! | 云盘账号（`/api/accounts`） | 移动云盘的 auth | `accounts.json` | 任务运行时 |
//!
//! 本模块管"谁能进这个 WebUI"，不管"能操作哪个云盘账号"。
//!
//! # 三态门
//!
//! - **未初始化**（0 个用户）⇒ 除 `/api/setup` 与 `/api/health` 外**一律 503**
//! - **已初始化、未登录** ⇒ 除 `/api/session` 外一律 401
//! - **已登录** ⇒ 放行
//!
//! ⚠ 未初始化时返回 503 而不是 401，是因为这两件事的**处置不同**：
//! 401 让前端弹登录框（而这时根本没有账号可登），503 让前端跳到初始化向导。
//! 混成一个码，用户会看到"登录失败"而不是"还没建管理员"。

use axum::extract::{ConnectInfo, Extension, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use mcloud_core::{validate_username, PasswordHash, Session, SessionKind, User, UserId};
// 令牌原语来自 crypto 而不是 core：core 只放**类型与 trait**，
// 密码学原语归 crypto。这条边界让 core 不必依赖任何密码学实现。
use mcloud_crypto::{hash_session_token, new_session_token};

// 面板行为（`loginNew` / `loginFailed` ✓）—— 判据与发送在 `mcloud-app::notify` ✓，
// 这里只负责**在正确的时机调它**（用户口径：「接啊」✓）。
use mcloud_app::notify::PanelNotice;
use mcloud_core::push::{BehaviorKey, BehaviorStatus};

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 会话 cookie 名。
const COOKIE: &str = "mcloud_session";

// ── 请求/响应类型 ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    /// 是否已经建过管理员。
    pub initialized: bool,
    /// 口令长度下限。**必须发给前端** —— 前端硬编一份必然与它漂移，
    /// 而漂移的表现是"前端说可以、后端拒绝"。
    pub min_password_len: usize,
    /// 登录时前端该发什么凭据：`"sha256"` = 发 `SHA-256(明文)` 的 hex；
    /// `"plain"` = 发**明文**（库里还是老档，服务端验过之后会当场升级）。
    ///
    /// 让前端**先问再发**，而不是"发错了再重试一次" —— 后者要多一条错误码、
    /// 一次额外的往返，而两条路的差异只是"这个实例的库是什么时候建的"。
    pub password_scheme: &'static str,
}

/// 凭据档位：库里存的是 `argon2(SHA-256(明文))`。
const SCHEME_DIGEST: &str = "sha256";

/// 凭据档位：库里存的是 `argon2(明文)`（2026-10-01 之前的实例）。
const SCHEME_PLAIN: &str = "plain";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub user_id: String,
    pub name: String,
    pub kind: &'static str,
    /// 是不是**实例管理员**（= 创建最早的那个用户，见 [`crate::auth_gate::is_admin`]）。
    ///
    /// # 为什么要给前端这一格（2026-10-06）
    ///
    /// 界面上有些东西**只对管理员有意义**：日志页的「运维」那一档
    /// （`/api/logs?category=ops` 对普通用户回 **403** ✓）、设置页里那些
    /// 实例级动作。没有这一格的话，普通用户点进去只会撞一个 403 报错 ✗ ——
    /// 而"这一格按钮不该给他看"这件事，前端只能从后端问 ✓。
    ///
    /// ⚠ 它**只是提示**，不是权限本身：真正的闸在后端（`require_admin` ✓）。
    /// 前端藏按钮是为了体验，不是为了安全 —— 别把判据搬到前端 ✗。
    #[serde(rename = "isAdmin")]
    pub is_admin: bool,
}

// ── 初始化 ──────────────────────────────────────────────────────────────

/// 这个实例是否已经初始化过。
///
/// 判据是"有 0 个用户"而不是"有个 initialized 标志位" ——
/// 标志位会与实际状态漂移（标志位 true 但用户表空了），而用户表是**事实**。
pub async fn is_initialized(state: &ServerState) -> Result<bool, ApiError> {
    let n = state.handle.store.user_count().await?;
    Ok(n > 0)
}

/// 这个实例的**登录凭据档位**（见 [`SetupStatus::password_scheme`]）。
///
/// ⚠ 只要**有一个**用户还是老档就报 `plain`：前端一次只能发一种凭据，
/// 而"存在老记录"意味着只有明文那条路能同时喂饱两边。单个管理员是本项目的主线，
/// 多管理员只是理论情形 —— 这时候宁可让新记录也再走一次明文（登录成功后会各自升级）。
pub(crate) async fn login_scheme(state: &ServerState) -> Result<&'static str, ApiError> {
    let users = state.handle.store.users().await?;
    Ok(
        if users
            .iter()
            .any(|u| !mcloud_crypto::is_digest_scheme(u.password_hash.as_str()))
        {
            SCHEME_PLAIN
        } else {
            SCHEME_DIGEST
        },
    )
}

pub async fn setup_status(State(state): State<ServerState>) -> ApiResult<SetupStatus> {
    Ok(Json(SetupStatus {
        initialized: is_initialized(&state).await?,
        // 下限是**可改的设置**（默认 8 = `MIN_PASSWORD_LEN`）
        min_password_len: state.handle.store.settings().await?.min_password_len,
        password_scheme: login_scheme(&state).await?,
    }))
}

/// 首次建管理员。
///
/// ⚠ **已经初始化过就拒绝**（409）。不这样的话，任何人都能在你的实例上
/// 再建一个管理员账号 —— 那等于没有登录闸。
///
/// # 口令怎么落库（用户口径 2026-10-01：**前端 hash 一次、后端再 hash 一次**）
///
/// 新建的管理员**一律落新档**：`argon2(SHA-256(明文))`，存储串带 `sha256$` 前缀。
/// 前端没做那一次（缓存里的旧 bundle 还在发明文）时，服务端**现场补上**
/// —— 兼容口子开在这里，而不是"拒绝明文"，否则老 bundle 的用户一点登录就 400。
pub async fn setup_create(
    State(state): State<ServerState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(body): Json<Credentials>,
) -> Result<Response, ApiError> {
    // ⚠ 先过闸（复审 sv-5）：这条路同样在 PUBLIC 里，且要算一次 argon2。
    let peer_ip = peer_key(&peer);
    if let Some(err) = auth_rate_limit(&state, &peer_ip) {
        return Err(err);
    }
    if is_initialized(&state).await? {
        // 记一笔：正常操作碰不到这条（向导只走一次），出现它要么是重复提交、
        // 要么是有人在探"这个实例建了没"（审计 SEC-12）✓
        tracing::warn!(peer = %peer_ip, "拒绝重复初始化");
        return Err(ApiError(mcloud_core::Error::Config(
            "已经初始化过了，不能重复创建管理员".into(),
        )));
    }

    validate_username(&body.username).map_err(mcloud_core::Error::Config)?;

    // ⚠ 长度下限只在**明文**这条路上判得出来：摘要恒 64 位，拿它比长度没有意义。
    // 所以"收到明文"时在这里卡一道，报错文案与从前一致（前端已经拿 minPasswordLen 卡过一次，
    // 这是服务端那道，缺了它 curl 直接 POST 就能设一个 1 位的口令）。
    //
    // ⚠⚠ 已知且**刻意接受**的口径（安全审计 `SEC-15`）：`is_client_digest` 的判据是
    // "64 位小写 hex"，所以任何 64 位 hex 串都会被当成"前端摘要"从而跳过长度检查。
    // 这不是提权 —— 那个串仍然要经 argon2 落库、下次登录还得原样再发一遍才进得来
    //（摘要本身**就是**凭据，而它的长度不受"口令长度下限"约束是摘要方案的固有性质）。
    // 真要卡，得改成"摘要必须由服务端下发的一次性挑战派生"，那是另一件事。
    let settings = state.handle.store.settings().await?;
    if !mcloud_crypto::is_client_digest(&body.password)
        && body.password.chars().count() < settings.min_password_len
    {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "口令太短：至少 {} 个字符（下限 {} 位）",
            settings.min_password_len, settings.min_password_len
        ))));
    }

    let digest = mcloud_crypto::to_digest_credential(&body.password);
    let hash = mcloud_crypto::hash_password(&digest).map_err(|e| {
        mcloud_core::Error::Config(format!("{e}（下限 {} 位）", settings.min_password_len))
    })?;

    let now = state.handle.clock.now_ms();
    let user = User {
        id: UserId::new(uuid::Uuid::new_v4().to_string()),
        name: body.username.clone(),
        password_hash: PasswordHash(mcloud_crypto::mark_digest_scheme(&hash)),
        created_at_ms: now,
    };
    // ⚠⚠ 2026-10-06 安全审计 `SEC-09`：**判空与插入必须在存储层同一把锁内**。
    //
    // 上面那句"已经初始化过就拒绝"（TOCTOU）与这里之间隔着一次 argon2
    //（几十毫秒，**不持锁**）⇒ 两个并发的 `POST /api/setup` 都能通过那道检查，
    // 于是各建一个管理员 ✗。`create_first_user` 把判空挪进写锁里，
    // 返回 `false` 就是"你输了这场竞争"，照旧回 409 ✓。
    if !state.handle.store.create_first_user(&user).await? {
        return Err(ApiError(mcloud_core::Error::Config(
            "实例已经初始化过了".into(),
        )));
    }

    // 建完管理员**顺手登录** —— 让用户接着填一遍刚设的口令是多余的
    let (token, session) = make_session(&user, now, settings.session_ttl_ms());
    state.handle.store.save_session(&session).await?;

    tracing::info!(user = %user.name, peer = %peer_ip, "已创建管理员");
    Ok(set_cookie_response(
        StatusCode::CREATED,
        &token,
        session_expires_at(session.expires_at_ms, now),
        state.cookie_secure,
        Json(SessionView {
            // 刚建出来的**就是第一个用户** ⇒ 按 `legacy_owner` 的口径他必然是管理员 ✓
            // （这里不查库：`create_first_user` 刚刚在锁内确认过用户表是空的 ✓）
            is_admin: true,
            user_id: user.id.to_string(),
            name: user.name,
            kind: "web",
        }),
    ))
}

// ── 登录 / 登出 ─────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<ServerState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> Result<Response, ApiError> {
    // ⚠ 先过闸（复审 sv-5）——**必须在"是否已初始化"之前**，理由见助手函数的注释。
    let peer_ip = peer_key(&peer);
    if let Some(err) = auth_rate_limit(&state, &peer_ip) {
        return Err(err);
    }
    // 失败计数的键：**用户名 + peer IP**（审计 `SEC-12`，理由见 `login_fail_limiter`）。
    // ⚠ 带 IP 是刻意的：只用用户名的话，谁都能靠"故意输错 5 次"把管理员锁在门外 ✗。
    let fail_key = format!("{}@{peer_ip}", body.username);

    // 未初始化时明确告知，而不是"用户名或口令错误" ——
    // 后者会让人反复试一个根本不存在的账号
    if !is_initialized(&state).await? {
        return Err(ApiError(mcloud_core::Error::Config(
            "实例尚未初始化，请先创建管理员".into(),
        )));
    }

    // ⚠⚠ 计时抹平（2026-10-04 复审 `sv-4`）：**用户不存在时也要跑一次 argon2**。
    //
    // 直接 `ok_or_else` 早退的话，这条分支**一步 argon2 都不跑**，而 argon2id 是本
    // 项目里最慢的一步（故意慢）⇒ 两条分支的响应时间差**一个数量级**，等于把
    // "这个用户名存不存在"写在秒表上 ✗ —— 与下面那句共用的"用户名或口令错误"
    //（注释里明写"区分开来就是一个探测器"）自相矛盾。
    let Some(user) = state.handle.store.user_by_name(&body.username).await? else {
        let _ = mcloud_crypto::verify_password(
            &mcloud_crypto::to_digest_credential(&body.password),
            mcloud_crypto::strip_digest_scheme(dummy_password_hash()),
        );
        return Err(note_login_failure(
            &state,
            &fail_key,
            &body.username,
            &peer_ip,
            // ⚠ 用户名**根本不存在** ⇒ 没有 owner ⇒ `loginFailed` **不发** ✓
            // （不许猜一个用户出来 —— 那等于把别人的规则当成他的 ✓）。
            None,
        ));
    };

    // ── 口令校验：两种档位，按**库里那条记录**决定怎么验 ──────────────────
    //
    // 老档（2026-10-01 之前建的实例）：库里是对**明文**做的 argon2 ⇒ 只能用明文验。
    // 新档：库里是对**前端摘要**做的 argon2 ⇒ 请求里是摘要就直接用；
    //       是明文（浏览器缓存里的旧 bundle）就现场补一次摘要。
    // 两条兼容口子都指向同一个目的：**任何一侧还没更新，都不许把人锁在门外**。
    let stored = user.password_hash.as_str();
    let legacy = !mcloud_crypto::is_digest_scheme(stored);
    let digest = mcloud_crypto::to_digest_credential(&body.password);
    let candidate = if legacy {
        body.password.as_str()
    } else {
        digest.as_str()
    };

    if mcloud_crypto::verify_password(candidate, mcloud_crypto::strip_digest_scheme(stored))
        .is_err()
    {
        return Err(note_login_failure(
            &state,
            &fail_key,
            &body.username,
            &peer_ip,
            // ⭐ 用户**在库里** ⇒ owner 就是他自己 ✓ ⇒ `loginFailed` 该报 ✓。
            Some(&user.id),
        ));
    }

    // 老档验过 ⇒ **当场升级**。不升级的话这条记录永远只能靠明文登录，
    // "明文不出浏览器"就永远只是新装实例的事。
    // ⚠ 升级用的一定是**摘要**（`digest`），不是刚验过的明文 —— 后者等于白做。
    if legacy {
        let hash = mcloud_crypto::hash_password(&digest)
            .map_err(|e| mcloud_core::Error::Config(format!("口令升级失败：{e}")))?;
        let upgraded = User {
            password_hash: PasswordHash(mcloud_crypto::mark_digest_scheme(&hash)),
            ..user.clone()
        };
        state.handle.store.save_user(&upgraded).await?;
        tracing::info!(user = %upgraded.name, "管理员口令已升级为「前端摘要」档");
    }

    // ⚠ 成功即清零失败计数（审计 `SEC-12`）：**口令对的那一次永远不该被算进配额**
    // ⇒ 配额只限制"猜错的速度"，不限制"能不能进来" ✓（`SEC-02` 的教训）。
    state.login_fail_limiter.reset(&fail_key);

    let now = state.handle.clock.now_ms();
    let ttl_ms = state.handle.store.settings().await?.session_ttl_ms();
    let (token, session) = make_session(&user, now, ttl_ms);
    state.handle.store.save_session(&session).await?;

    // ⭐ 「**不能登录时給好建一个？**」（用户口径 2026-10-07 ✓）—— 就是这里 ✓。
    //
    // 给这个用户补上**他自己的**默认编排 + 默认定时（`daily-<他的id>` ✓，幂等 ✓）。
    // ⚠ 放在登录而不是只在启动时：`mcloud user add` 加进来的新用户**不必等下一次重启** ✓。
    // ⚠ 失败**不挡登录**：默认编排是附加品，建不出来不该让人进不了门 ✗（只记一条 warn ✓）。
    if let Err(e) = state.handle.seed_defaults_for(&user.id).await {
        tracing::warn!(user = %user.name, error = %e, "补建默认编排失败（不影响登录）");
    }

    tracing::info!(user = %user.name, peer = %peer_ip, "登录成功");

    // ⭐ 面板行为 `loginNew`（「登录新设备」✓）—— 判据是**这个账号 + 这台设备/
    //    这个地点**没见过 ✓（用户口径：「登录新设备」✓）。
    //
    //    ⚠ 本仓**没有**真设备这个概念 ✗（登录请求里只有口令 ✓）⇒ 标识退化成
    //    `UA|peer IP` ✓ —— 这是**明说的近似**（见 `login_device_key` 的说明 ✓），
    //    不是"任意一次成功登录都算新设备" ✗（那是骗人 ✓）。
    //
    //    ⚠ 顺序：放在"口令验过、会话已建"之后 ⇒ 只有**真的登进来**才算一次 ✓；
    //    失败那条路走 `note_login_failure` ✓，两条互不重叠 ✓。
    //    ⚠ 失败**不挡登录**：新设备判定是附加品，判不出来不该让人进不了门 ✗。
    let device = login_device_key(&headers, &peer_ip);
    match state
        .handle
        .store
        .remember_login_device(&user.id, &device)
        .await
    {
        Ok(true) => {
            state.handle.pushes.spawn_panel(
                user.id.clone(),
                PanelNotice {
                    behavior: BehaviorKey::LoginNew,
                    status: BehaviorStatus::Abnormal,
                    title: format!("mcloud 登录告警 · {}", BehaviorKey::LoginNew.label()),
                    text: format!(
                        "账号 {} 在 {} 从没见过的一台设备/地点登录成功（来自 {}，客户端 {}）—— 不是你本人的话，去改口令",
                        user.name,
                        mcloud_core::push::local_time_hms(now),
                        peer_ip,
                        short_ua(&headers),
                    ),
                },
            );
        }
        Ok(false) => {}
        Err(e) => {
            // 判不出来 ⇒ **不发**并留痕 ✓（宁可少报一次新登录，
            // 也不要因为一次读盘失败就把每一次登录都报成"新设备" ✗）。
            tracing::warn!(user = %user.name, error = %e, "登录设备判据读取失败，本次不报新登录");
        }
    }
    // ⚠ 这里要**真查**（不能照 `setup_create` 那样写死 true）：`mcloud user add`
    // 可以往库里加普通用户（2026-10-06 ✓）⇒ "登录的人一定是管理员"不再成立 ✓。
    let is_admin = crate::auth_gate::is_admin(&state, &user.id).await?;
    Ok(set_cookie_response(
        StatusCode::OK,
        &token,
        session_expires_at(session.expires_at_ms, now),
        state.cookie_secure,
        Json(SessionView {
            is_admin,
            user_id: user.id.to_string(),
            name: user.name,
            kind: "web",
        }),
    ))
}

/// 登出。
///
/// 返回类型是 `Result<StatusCode, ApiError>` 而**不是** `ApiResult<StatusCode>`：
/// 后者展开成 `Result<Json<StatusCode>, _>`，会把 `204` 包成 `Json(204)` ——
/// 于是响应变成 `200` 加一个 `204` 的 JSON 体。
pub async fn logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = cookie_token(&headers) {
        state
            .handle
            .store
            .remove_session(&hash_session_token(&token))
            .await?;
    }
    // ⚠⚠ 服务端那条会话删了，**浏览器里那枚 cookie 也得让浏览器扔掉**
    //（2026-10-04 复审 `sv-6`）：只删库、不 `Set-Cookie` 的话，cookie 会一直留在
    // 浏览器里 —— 登出后它仍会被每一次请求带上（虽然服务端已经认不出它），
    // 于是"登出"看起来只是"下次一定 401"，那枚凭据本体还躺在磁盘上 ✗。
    // 属性与登录时**逐项对齐**（同 Path / HttpOnly / SameSite）＋ `Max-Age=0`，
    // 否则浏览器会留下另一枚同名的 cookie，表现成"登出了还在"。
    // ⚠ 即使没有 cookie 也返回 204：登出是**幂等**的，
    // "本来就没登录"不该报错（照旧清一遍 cookie，对没登录的人也无害 ✓）。
    Ok(clear_cookie_response(
        StatusCode::NO_CONTENT,
        state.cookie_secure,
    ))
}

/// 用户不存在时拿它跑一次 argon2（见 [`login`] 里的计时抹平，复审 `sv-4`）。
///
/// 第一次用时算一次（`LazyLock`），之后复用 —— 串的内容**是什么不重要**，
/// 重要的是"这一步必须和真校验**一样贵**" ✗（图省事换成一个便宜的哈希，
/// 那个计时差当场就回来了）。
fn dummy_password_hash() -> &'static str {
    static DUMMY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        mcloud_crypto::mark_digest_scheme(
            &mcloud_crypto::hash_password("mcloud-timing-equalizer")
                .expect("argon2 应当能算出这个固定串"),
        )
    });
    DUMMY.as_str()
}

/// 限速用的键：**peer IP**（2026-10-06 安全审计 `SEC-02`）。
///
/// # 为什么是 `Option<Extension<..>>` 而不是 `ConnectInfo<SocketAddr>`
///
/// `ConnectInfo<T>` 作为提取器**拿不到就直接拒绝请求**（500）—— 而"拿不到"这件事
/// 在测试里是常态（`oneshot` 直接打 Router，没有连接信息），在将来换了 `serve`
/// 装配时也可能发生。这里要的是"拿不到就退化成粗一点的键"，不是"整个登录接口 500" ✗。
///
/// # 退化为什么是 `"unknown"` 一个桶
///
/// 退化成全局键 = **退回旧行为**（限速仍然生效，但不同客户端会互相影响）。
/// 反过来"拿不到键 ⇒ 干脆不限"是最糟的失效方向：一个装配失误就静默把这道闸摘掉 ✗。
/// 所以宁可粗，不可无。
///
/// ⚠ `X-Forwarded-For` **不读**：那是一个可伪造的头，读它就等于让攻击者自选桶。
/// 要按真实客户端限速必须先定义反代的信任边界（见 `ratelimit.rs` 模块头的边界说明）。
fn peer_key(peer: &Option<Extension<ConnectInfo<SocketAddr>>>) -> String {
    peer.as_ref()
        .map(|Extension(ci)| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 认证面的限速闸（2026-10-04 复审 `sv-5`）：超限 ⇒ `Some(429 响应)`。
///
/// 为什么这两个路由要单独设闸：它们在 `auth_gate::PUBLIC` 里 ⇒ **不需要凭据**，
/// 而每一次都跑一遍 argon2id（19 MiB × 2 轮）⇒ 一个 `for` 循环就能占满单核。
/// 判据与边界写在 [`crate::ratelimit`] 的模块说明上 ✓。
///
/// ⚠ 闸放在**所有其他判断之前**（包括"实例是否已初始化"）：否则未初始化的实例上
/// 那条 400 分支就成了一个不花 argon2 的探针，而闸也拦不住它 ✗。
fn auth_rate_limit(state: &ServerState, key: &str) -> Option<ApiError> {
    rate_limited(
        state.auth_limiter.check(key, state.handle.clock.now_ms()),
        // ⚠ 措辞固定成"尝试"：`tests/login_routes.rs` 断言了这句（用户看得懂的
        // 那句提示，不该在重构里被顺手改掉）
        "尝试",
    )
}

/// **已登录用户**触发的昂贵动作的闸（2026-10-06 安全审计 `SEC-08`）。
///
/// 与 [`auth_rate_limit`] 分开的两个理由写在 [`crate::ServerState::sensitive_limiter`] 上：
/// 键是**用户 id**（不是 IP），配额是 10/分（不是 30/分）。
///
/// ⚠ 顺序：**先过闸再干活**。放后面就等于"活干完了再说你太快了"，
/// 而这里要挡的正是那件"活"（一次 argon2 / 一条真短信）✗。
pub(crate) fn sensitive_rate_limit(
    state: &ServerState,
    user: &mcloud_core::UserId,
) -> Option<ApiError> {
    rate_limited(
        state
            .sensitive_limiter
            .check(user.as_str(), state.handle.clock.now_ms()),
        "操作",
    )
}

/// 记一次登录失败，并给出该回给客户端的错误（2026-10-06 安全审计 `SEC-12`）。
///
/// * 配额内 ⇒ **401**"用户名或口令错误"（措辞与成功无关、与"用户不存在"同一句）；
/// * 超配额 ⇒ **429**（`登录失败过于频繁`）。
///
/// ⚠ 两种码都**不透露口令对不对**（它们只在"口令已经错了"之后才可能返回），
/// 所以 429 不构成新的探测器 ✓。
///
/// ⚠ 日志里**只记用户名与 peer IP，绝不记口令**（明文/摘要都不记）：
/// 那条日志会被 `logbuf` 收进环、经 `/api/logs` 给管理员看，凭据进去就等于
/// 把口令写进了一个能读回来的地方 ✗。
///
/// # 面板行为 `loginFailed`（2026-10-08 接上 ✓）
///
/// 用户口径：**每一次失败都算一次触发** ✓ —— 频次交给 `deliver_panel` 的节流
/// （冷却 + 每日上限 ✓），这里不做任何取舍 ✓。
/// ⚠ `owner` 拿不到（用户名根本不存在 ✓）⇒ **不发** ✓ —— 猜一个用户出来，
/// 等于把别人的规则当成他的用 ✗（与 `notify.rs` 模块头同一条 ✓）。
fn note_login_failure(
    state: &ServerState,
    key: &str,
    username: &str,
    peer_ip: &str,
    owner: Option<&UserId>,
) -> ApiError {
    tracing::warn!(user = %username, peer = %peer_ip, "登录失败");
    if let Some(owner) = owner {
        // ⚠ 走 `spawn_panel`（**不 await**）：`deliver_panel` 会同步走完 HTTP
        // （最多 10s ✓）—— 在这条路上 await 它，等于让"能不能进门"取决于
        // "推送服务器快不快" ✗。⚠ 也**不记口令**：正文里只有用户名与 IP ✓。
        state.handle.pushes.spawn_panel(
            owner.clone(),
            PanelNotice {
                behavior: BehaviorKey::LoginFailed,
                status: BehaviorStatus::Abnormal,
                title: format!("mcloud 登录告警 · {}", BehaviorKey::LoginFailed.label()),
                text: format!(
                    "账号 {username} 在 {} 登录失败（来自 {peer_ip}）—— 不是你本人的话，去改口令",
                    mcloud_core::push::local_time_hms(state.handle.clock.now_ms()),
                ),
            },
        );
    }
    match state
        .login_fail_limiter
        .check(key, state.handle.clock.now_ms())
    {
        Ok(()) => ApiError(mcloud_core::Error::Auth("用户名或口令错误".into())),
        Err(wait_secs) => ApiError(mcloud_core::Error::RateLimited {
            what: "登录失败".into(),
            retry_after_secs: wait_secs,
        }),
    }
}

/// 登录设备的判据串：**`UA|peer IP`** ✓（2026-10-08 接 `loginNew` 时新增）。
///
/// # 为什么只能这样（这条要说清楚，别把它当"真设备"）
///
/// 本仓**没有**设备这个概念 ✗ —— 登录请求里只有用户名与口令（外加 HTTP 自带的
/// 头 ✓）。所以"新设备"只能退化成「**这个账号 + 这个 UA + 这个 peer IP** 的组合
/// 没见过」✓：
///
/// * 换一台设备（或换个浏览器）⇒ UA 变 ⇒ 报 ✓；
/// * 换一个网络（家里 → 公司 → 手机热点）⇒ IP 变 ⇒ 报 ✓；
/// * 浏览器自动升级改了 UA 版本号 ⇒ **也会报** ✗ —— 这是这套近似的已知代价 ✓
///   （节流会把它压到每 30 分钟最多一条 ✓，见 `BehaviorKey::default_throttle` ✓）。
///
/// ⚠ 它是**明说的近似**，不是"任何一次成功登录都算新设备" ✗（那是骗人 ✓）。
/// 要做得更准，得先让登录带上一个真设备标识 —— 那是另一件事 ✓。
///
/// ⚠ UA 是**请求方自己给**的任意长度串 ⇒ 这里截到 200 字符：判据够用，
/// 而落盘那格（`data/login-devices.json` ✓）不会被一个超长头撑大 ✓。
fn login_device_key(headers: &HeaderMap, peer_ip: &str) -> String {
    format!("{}|{peer_ip}", short_ua(headers))
}

/// 请求里的 `User-Agent`（**截断**、拿不到就是 `"unknown"` ✓）。
///
/// 抽出来单独一个函数，是为了让"判据串"与"告警正文"用的是**同一个**值 ✓
/// —— 两处各拼一次，早晚会漂移成"报了新设备、正文里却是另一台" ✗。
fn short_ua(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .chars()
        .take(200)
        .collect()
}

/// 把限速结果变成错误：放行 ⇒ `None`，超限 ⇒ `Some(429 + Retry-After)`。
///
/// ⚠ 做成 `ApiError` 而不是直接拼响应：handler 的返回类型有两种
/// （`Result<Response, _>` 与 `ApiResult<T> = Result<Json<T>, _>`），
/// 只有错误能同时装进两种（详见 [`mcloud_core::Error::RateLimited`]）。
fn rate_limited(result: Result<(), i64>, what: &str) -> Option<ApiError> {
    result.err().map(|wait_secs| {
        ApiError(mcloud_core::Error::RateLimited {
            what: what.to_string(),
            retry_after_secs: wait_secs,
        })
    })
}

/// 让浏览器**扔掉**登录 cookie 的那一发（登出用）。
///
/// `Max-Age=0` 是标准的失效写法；值留空（不是 `token=;` —— 空值仍是"一枚 cookie"）。
///
/// ⚠ 属性必须与登录时**逐项一致**（含 `Secure`）：浏览器按"名字 + Path + Domain +
/// Secure"匹配已有 cookie，属性对不上时这一发清掉的可能是**另一枚** ——
/// 表现就是"点了登出、刷新还在"。所以这里也收 `secure` ✓。
fn clear_cookie_response(status: StatusCode, secure: bool) -> Response {
    let mut cookie = format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    (status, [(header::SET_COOKIE, cookie)]).into_response()
}

/// 当前会话。未登录时 401（由中间件拦，这里只处理已登录的情况）。
pub async fn current_session(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> ApiResult<SessionView> {
    let session = require_session(&state, &headers).await?;
    let user = state
        .handle
        .store
        .user(&session.user_id)
        .await?
        .ok_or_else(|| ApiError(mcloud_core::Error::Auth("会话对应的用户已不存在".into())))?;

    Ok(Json(SessionView {
        is_admin: crate::auth_gate::is_admin(&state, &user.id).await?,
        user_id: user.id.to_string(),
        name: user.name,
        kind: match session.kind {
            SessionKind::Web => "web",
            SessionKind::Cli => "cli",
        },
    }))
}

// ── 会话读写 ────────────────────────────────────────────────────────────

/// 建会话。
///
/// `ttl_ms` 由调用方从 `AppSettings` 取（默认值 = 原来的 `WEB_SESSION_TTL_MS`，
/// 见 `mcloud-core::settings`）—— **改设置只影响之后新建的会话**，
/// 已经发出去的 cookie 不会被动缩短有效期。
pub(crate) fn make_session(user: &User, now_ms: i64, ttl_ms: i64) -> (String, Session) {
    let token = new_session_token();
    let session = Session {
        token_hash: hash_session_token(&token),
        user_id: user.id.clone(),
        kind: SessionKind::Web,
        created_at_ms: now_ms,
        expires_at_ms: now_ms + ttl_ms,
    };
    // 令牌原文只在这一次响应里出现（写进 cookie），之后库里只有哈希
    (token, session)
}

pub(crate) fn session_expires_at(expires_at_ms: i64, now_ms: i64) -> i64 {
    // cookie 的 Max-Age 用秒；会话表用毫秒
    ((expires_at_ms - now_ms) / 1000).max(0)
}

/// 从 `Cookie` 头里取我们的令牌。
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{COOKIE}=")) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 取当前有效会话；没有/过期/用户已删都返回 401。
pub async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Session, ApiError> {
    let token =
        cookie_token(headers).ok_or_else(|| ApiError(mcloud_core::Error::Auth("未登录".into())))?;
    let hash = hash_session_token(&token);

    let session = state
        .handle
        .store
        .session(&hash)
        .await?
        .ok_or_else(|| ApiError(mcloud_core::Error::Auth("会话已失效".into())))?;

    let now = state.handle.clock.now_ms();
    if !session.is_valid_at(now) {
        // 顺手清掉：过期会话留着既占地方，又会在下次被误读
        state.handle.store.remove_session(&hash).await?;
        tracing::debug!("会话已过期，已清除");
        return Err(ApiError(mcloud_core::Error::Auth("会话已过期".into())));
    }

    // ⚠⚠ 2026-10-06 安全审计 `SEC-19`：**只查会话表是不够的**。
    //
    // ① **用户还在不在**：删掉一个用户（今天没有这条路，但 `remove_user` 已经在
    //    `UserStore` 里了）之后，他那张还没到期的会话会继续放行 —— 会话表与用户表
    //    之间没有外键，谁也不会替你把它们对齐 ✗。这里顺手对一次，
    //    发现用户没了就**当场删掉这张会话**（失败关闭 ✓）。
    // ② **会话种类**：`SessionKind::Cli` 是给命令行用的**长期**会话
    //    （`expires_at_ms` 可以是 0 = 永不过期）。它要是被塞进 cookie 就能
    //    当 Web 会话用 —— 而 WebUI 的闸、登出、改口令踢会话那几条全都按
    //    "Web 会话有到期时间"设计 ✗。所以这里显式只认 `Web` ✓。
    //
    // 两条都是"今天没有可利用路径、但口径必须一致"的加固（`current_session`
    // 早就查了用户表，这里原先没查 ⇒ 两条路口径不一致）。
    if session.kind != mcloud_core::SessionKind::Web {
        state.handle.store.remove_session(&hash).await?;
        tracing::warn!("拒绝把非 Web 会话当 cookie 用（已清除该会话）");
        return Err(ApiError(mcloud_core::Error::Auth("会话已失效".into())));
    }
    if state.handle.store.user(&session.user_id).await?.is_none() {
        state.handle.store.remove_session(&hash).await?;
        tracing::warn!(user = %session.user_id, "会话对应的用户已不存在，已清除该会话");
        return Err(ApiError(mcloud_core::Error::Auth("会话已失效".into())));
    }

    Ok(session)
}

/// 组装带 `Set-Cookie` 的响应。
///
/// cookie 属性逐条都有理由：
///
/// - `HttpOnly` —— JS 读不到，XSS 偷不走令牌
/// - `SameSite=Strict` —— 跨站请求不带它，挡住 CSRF
/// - `Path=/` —— 全站可用
/// - `Max-Age` —— 与库里的到期时间**一致**；只靠 cookie 过期的话，
///   客户端时间被改就绕过了
///
/// **`Secure` 由 `secure` 参数决定**（2026-10-06 安全审计 `SEC-13`）：
/// 本工具默认跑在 `http://127.0.0.1`，无条件加 `Secure` 会让浏览器**不存**这枚
/// cookie，表现是"登录成功但立刻又未登录" ✗ ⇒ 默认不加；部署在 HTTPS 反代后面时
/// 由 `MCLOUD_COOKIE_SECURE=1` 打开（见 [`crate::ServerState::cookie_secure`]）。
pub(crate) fn set_cookie_response<T: Serialize>(
    status: StatusCode,
    token: &str,
    max_age_secs: i64,
    secure: bool,
    body: Json<T>,
) -> Response {
    let mut cookie =
        format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}");
    if secure {
        cookie.push_str("; Secure");
    }
    (status, [(header::SET_COOKIE, cookie)], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    // 这两个只在测试里当"默认值"用：生产路径改从 `AppSettings` 读（默认值正是它们）。
    use axum::http::HeaderMap;
    use mcloud_core::WEB_SESSION_TTL_MS;

    fn headers_with_cookie(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, v.parse().unwrap());
        h
    }

    #[test]
    fn reads_our_cookie_from_a_crowded_header() {
        let h = headers_with_cookie("other=1; mcloud_session=abc123; another=2");
        assert_eq!(cookie_token(&h).as_deref(), Some("abc123"));
    }

    #[test]
    fn ignores_foreign_cookies() {
        let h = headers_with_cookie("other=1; another=2");
        assert_eq!(cookie_token(&h), None);
    }

    #[test]
    fn empty_cookie_value_is_treated_as_absent() {
        let h = headers_with_cookie("mcloud_session=");
        assert_eq!(cookie_token(&h), None, "空值等于没有，不该去查库");
    }

    #[test]
    fn missing_cookie_header_is_none() {
        assert_eq!(cookie_token(&HeaderMap::new()), None);
    }

    /// ⭐ 限速的键：有 peer IP 就用它，没有就退化成 `"unknown"`（**不是"不限"**）。
    ///
    /// 红测：把 `peer_key` 改成"拿不到就返回一个每次都不同的串"（等于每人一桶 ⇒
    /// 实际不限速）⇒ 第一条断言红；改成 `unwrap_or_default()`（空串）虽然也能限，
    /// 但"拿不到"和"IP 恰好是空"就混成一个桶了 —— 所以这里钉的是**具体的退化值**。
    #[test]
    fn the_rate_limit_key_is_the_peer_ip_or_a_named_fallback() {
        assert_eq!(peer_key(&None), "unknown");

        let addr: SocketAddr = "203.0.113.7:54321".parse().unwrap();
        let with_peer = Some(Extension(ConnectInfo(addr)));
        assert_eq!(
            peer_key(&with_peer),
            "203.0.113.7",
            "键只取 IP（端口每次都变，带上它就等于每人一桶 ⇒ 不限速）"
        );
    }

    /// cookie 里存的是**原文**，库里存的是哈希 —— 两者不能混。
    #[test]
    fn session_creation_stores_only_the_hash() {
        let user = User {
            id: UserId::new("u1"),
            name: "admin".into(),
            password_hash: PasswordHash("$argon2id$...".into()),
            created_at_ms: 0,
        };
        let (token, session) = make_session(&user, 1_000, WEB_SESSION_TTL_MS);
        assert_ne!(session.token_hash, token, "库里不能是原文");
        assert_eq!(
            session.token_hash,
            hash_session_token(&token),
            "存的必须是这个令牌的哈希"
        );
        assert_eq!(session.user_id, user.id);
        assert_eq!(session.kind, SessionKind::Web);
        assert_eq!(session.expires_at_ms, 1_000 + WEB_SESSION_TTL_MS);
    }

    /// `Max-Age` 是**秒**，会话表是**毫秒** —— 换算错就是"cookie 活得久"
    /// 或"立刻过期"。
    #[test]
    fn max_age_converts_ms_to_secs() {
        let now = 1_000_000;
        assert_eq!(session_expires_at(now + 60_000, now), 60);
        assert_eq!(session_expires_at(now, now), 0, "不能是负数");
        assert_eq!(
            session_expires_at(now + WEB_SESSION_TTL_MS, now),
            WEB_SESSION_TTL_MS / 1000
        );
    }
}
