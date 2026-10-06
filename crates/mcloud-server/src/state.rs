//! 服务状态。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mcloud_app::AppHandle;
use mcloud_core::{RunId, UserId};

use crate::routes::login::LoginSessions;

#[derive(Clone)]
pub struct ServerState {
    pub handle: Arc<AppHandle>,
    /// 本进程**开始服务**的时刻（epoch 毫秒）。
    ///
    /// 只给界面算「运行时间」用。为什么值得单独留一格：状态点只会说"现在看着正常"，
    /// 而一个**持续增长**的时长是"这个进程真的活着"最难伪造的证据 ——
    /// 顺带也回答了"上一次重启是什么时候"。
    ///
    /// ⚠ 取的是构造这一刻：进程起来到建好 HTTP 层之间只有毫秒级，
    /// 与"进程启动时刻"的差别看不出来。
    pub started_at_ms: i64,
    /// 进行中的扫码会话。
    ///
    /// **刻意不落盘**：一次扫码会话只活两分钟，进程重启后重出一张码
    /// 比"从磁盘恢复一个已经过期的会话"更符合直觉，也少一份要清理的状态。
    pub login_sessions: Arc<LoginSessions>,
    /// 认证面的限速器（登录 / 初始化）。见 [`crate::ratelimit`] 的模块说明。
    ///
    /// ⚠ 与 `login_sessions` 一样**刻意不落盘**：限速状态是"这一小会儿"的事，
    /// 重启后清零正是我们要的（重启本来就该让人重新试）。
    ///
    /// ⚠⚠ 2026-10-06（安全审计 `SEC-02`）：**键是 peer IP**，不再是"整个进程一个
    /// 计数器"。原来的全局单键有两个毛病：挡不住分布式，而且攻击者打满配额就能把
    /// **管理员**锁在登录页外（WebUI 是唯一配置入口）✗。peer IP 由 `serve` 侧的
    /// `ConnectInfo` 提供（`main.rs`）；拿不到时退化成 `"unknown"` 一个桶
    /// （见 `auth.rs` 的 `peer_key`）✓。
    pub auth_limiter: Arc<crate::ratelimit::RateLimiter>,
    /// **已登录用户**触发的昂贵动作的限速器（安全审计 `SEC-08`）。
    ///
    /// # 与 `auth_limiter` 分开，是因为**键和配额都不一样**
    ///
    /// * `auth_limiter`：键 = peer IP，配额 30/分 —— 挡的是"没凭据的人狂刷登录"；
    /// * 这个：键 = **用户 id**，配额 10/分 —— 挡的是"**已登录的人**拿自己的身份
    ///   去狂刷要算 argon2 / 要发短信的路由"：
    ///   - `PUT /api/admin/password`：每次一个 argon2（19 MiB）⇒ 自己就能把自己
    ///     的机器打满，而它还**不**受登录闸限速（那时已经登录了）✗；
    ///   - `POST /api/login/{sms,app/sms}/start`：每次**发一条真短信** ⇒ 这是唯一
    ///     会产生"对外副作用 + 可能被运营商计费/风控"的登录路由 ✗。
    ///
    /// 用**用户 id** 当键而不是 IP：这些动作已经过登录闸，身份是确定的；
    /// 而同一个 IP 后面可能坐着多个人（局域网），按 IP 限会误伤 ✓。
    pub sensitive_limiter: Arc<crate::ratelimit::RateLimiter>,
    /// **登录失败**计数器（安全审计 `SEC-12`）。键 = `用户名@peer IP`，5 次 / 5 分钟。
    ///
    /// # 与 `auth_limiter` 的分工
    ///
    /// `auth_limiter` 数的是**请求**（30/分/IP），挡"狂刷"；这个数的是**猜错**
    /// （5 次/5 分/用户名+IP），挡"慢慢猜" —— 一个攻击者每 10 秒试一次口令，
    /// 永远撞不到 30/分的闸，但会撞到这个 ✓。
    ///
    /// # ⚠ 为什么键里要带 IP
    ///
    /// 只用用户名的话，任何人都能靠"故意输错 5 次"把**管理员**锁在登录页外
    /// （`SEC-02` 那个坑的翻版 ✗）。带上 IP 之后，被锁的只是"攻击者自己那条
    /// 用户名+IP 组合" ✓。
    ///
    /// # ⚠ 为什么"口令对的那一次"不计入
    ///
    /// 调用方**只在失败时** `check`、成功时 `reset`（见 [`crate::ratelimit::RateLimiter::reset`]）
    /// ⇒ 配额只限制"猜错的速度"，**不限制"能不能进来"** ✓。
    pub login_fail_limiter: Arc<crate::ratelimit::RateLimiter>,
    /// 给 `Set-Cookie` 加不加 `Secure`（安全审计 `SEC-13`）。
    ///
    /// 由环境变量 `MCLOUD_COOKIE_SECURE` 决定（`1` / `true` / `yes`）。
    ///
    /// # 为什么是开关，而不是"自动判断"
    ///
    /// 本服务说**明文 HTTP**（TLS 由反代做），所以服务端**看不出**客户端那一段
    /// 是不是 HTTPS —— 拿 `X-Forwarded-Proto` 判断等于信任一个可伪造的头 ✗。
    /// 于是只能由部署者声明：**放在 HTTPS 反代后面就设上它** ✓。
    /// 默认关（本机 `http://127.0.0.1:3000` 直连时，加了 `Secure` 浏览器
    /// 就不会存这张 cookie ⇒ 直接登不上，那是比"缺一个属性"更糟的失效 ✗）。
    pub cookie_secure: bool,
    /// 本进程由**本服务**发起的运行 → 归属。
    ///
    /// # 为什么需要它
    ///
    /// 运行记录**结束时**才落盘（`Runner::run_impl` 收尾那一步），而客户端
    /// 拿到 run id 后会立刻订阅 SSE、或请求中止。那一刻存储层里没有这条记录，
    /// 归属也就无从推导 —— 不登记的话，B 只要猜到 id 就能订阅 / 中止 A 的运行。
    ///
    /// # 为什么只留「还在跑」的
    ///
    /// 表项在 [`ServerState::note_run_owner`] 里顺手按取消登记表清理：
    /// 运行结束后记录已经落盘，归属可由 `RunRecord.account` 推出来，
    /// 留着只会让这张表无限增长。
    pub run_owners: Arc<Mutex<HashMap<RunId, UserId>>>,
}

impl ServerState {
    pub fn new(handle: Arc<AppHandle>) -> Self {
        let started_at_ms = handle.clock.now_ms();
        Self {
            handle,
            started_at_ms,
            login_sessions: Arc::new(LoginSessions::new()),
            // 60 秒 30 次：正常使用（一个人偶尔登录）碰不到，而"100 个并发"
            // 会被截到 30 以内 ⇒ argon2 最多跑 30 次而不是无限次 ✓。
            auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(60_000, 30)),
            // 60 秒 10 次：改口令 / 发短信都是"人手动做、一分钟最多几次"的动作，
            // 10 次足够正常使用（连点、改错了再来），又让"脚本狂刷"立刻撞墙 ✓。
            sensitive_limiter: Arc::new(crate::ratelimit::RateLimiter::new(60_000, 10)),
            // 5 分钟 5 次：正常人手滑最多错一两次；而"每 10 秒猜一次"的脚本
            // 会在第 6 次撞墙（它撞不到 auth_limiter 那个 30/分的闸）✓。
            login_fail_limiter: Arc::new(crate::ratelimit::RateLimiter::new(300_000, 5)),
            cookie_secure: cookie_secure_from_env(),
            run_owners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 登记一次「本服务发起的运行」属于谁。
    ///
    /// 顺手把已经跑完的条目清掉（见 `run_owners` 的说明）。锁不跨 `await`。
    pub fn note_run_owner(&self, run_id: RunId, owner: &UserId) {
        let Ok(mut map) = self.run_owners.lock() else {
            // 锁中毒说明别的线程在这段同步代码里 panic 过。归属登记丢了
            // 只会让**这次**运行在落盘前不可订阅，不该让请求失败。
            return;
        };
        map.retain(|id, _| self.handle.cancel.is_running(*id));
        map.insert(run_id, owner.clone());
    }
}

/// `MCLOUD_COOKIE_SECURE` 的解析（安全审计 `SEC-13`）。
///
/// ⚠ 单独一个函数是为了**可测**：真值只在进程启动时取一次（见
/// [`ServerState::cookie_secure`]），而解析规则本身要能逐条钉住 ✓。
pub fn cookie_secure_from_env() -> bool {
    matches!(
        std::env::var("MCLOUD_COOKIE_SECURE").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}
