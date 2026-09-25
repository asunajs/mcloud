# mcloud-gost 架构设计

> 移动云盘（中国移动云盘 / 139 / 和彩云）自动化平台。
> 后端 Rust，前端 Vue Vapor（无虚拟 DOM）+ TSX。
>
> 状态：**设计定稿 + 脚手架**。本文件是唯一设计来源。

---

## 1. 目标与非目标

### 目标
- 多账号管理：凭证解析、有效期跟踪、自动刷新、持久化。
- 任务自动化：签到、领云朵、备份、AI 类活动、直播口令、兑换奖品等。
- 可观测：每次运行有结构化日志与事件流，前端实时可见（SSE）。
- 可调度：cron 定时 + 任务组合（pipeline）。
- 单二进制交付：前端产物内嵌，`mcloud-gost serve` 一条命令跑起来。
- 可测试：密码学与协议层用**黄金向量**对拍，不依赖网络。

### 非目标
- 不做批量注册、刷量、商业分发。仅个人账号自用。
- 不追求覆盖上游全部 130+ 接口，按任务需要增量接入。

---

## 2. 分层与依赖铁律

依赖只能**自上而下**，反向依赖一律视为设计错误。

```
                    ┌─────────────┐
                    │ mcloud-cli  │  mcloud-server
                    └──────┬──────┘        │
                           ▼               ▼
                    ┌──────────────────────────┐
                    │        mcloud-app        │  组装层：注入 IO 实现
                    └────────────┬─────────────┘
                                 ▼
              ┌──────────────────────────────────┐
              │           mcloud-tasks           │  任务实现
              └────────────────┬─────────────────┘
                               ▼
              ┌──────────────────────────────────┐
              │          mcloud-engine           │  注册表 / 运行器 / 事件
              └───────┬──────────────────┬───────┘
                      ▼                  ▼
        ┌──────────────────────┐  ┌──────────────┐
        │   mcloud-upstream    │  │ mcloud-store │
        └──────────┬───────────┘  └──────┬───────┘
                   ▼                     ▼
        ┌──────────────────────┐  ┌──────────────┐
        │   mcloud-crypto      │  │  (IO 实现)    │
        └──────────┬───────────┘  └──────────────┘
                   ▼
        ┌──────────────────────────────────────────┐
        │              mcloud-core                 │  领域类型 + trait，零 IO
        └──────────────────────────────────────────┘
```

| crate | 职责 | 允许 IO |
|---|---|---|
| `mcloud-core` | 领域类型、错误、trait 定义（`Store`/`Clock`/`Http`/`Sink`） | ❌ |
| `mcloud-crypto` | 纯函数密码学：MD5 签名、SHA256、AES-CBC、DES | ❌ |
| `mcloud-upstream` | 上游接口封装：端点常量、请求/响应类型、`reqwest` 客户端 | ✅ HTTP |
| `mcloud-engine` | 任务注册表、运行器、并发闸、事件总线 | ❌ |
| `mcloud-tasks` | 各任务的业务编排 | ❌（经 trait） |
| `mcloud-store` | 持久化：账号、运行历史、调度、pipeline | ✅ 磁盘 |
| `mcloud-app` | 组装：把 IO 实现注入 engine | ✅ |
| `mcloud-server` | axum 路由 + SSE + 内嵌前端 | ✅ |
| `mcloud-cli` | 命令行入口 | ✅ |

**铁律**：`core` / `crypto` / `engine` / `tasks` 不得直接依赖 `reqwest`、`sqlx`、`tokio::fs`。
所有 IO 走 trait，便于测试时替换为 fixture 回放。

---

## 3. 领域模型（mcloud-core）

```rust
/// 账号标识：手机号
pub struct AccountId(String);

/// 凭证：base64(platform:phone:token)，token 第 4 段是过期时间
pub struct AuthInfo {
    pub raw: String,       // 原始 base64，写回配置用
    pub platform: String,  // 如 "android"
    pub phone: String,
    pub token: String,
    pub expire: i64,       // 秒级时间戳
}

pub struct Account {
    pub id: AccountId,
    pub nickname: Option<String>,
    pub auth: AuthInfo,
    pub device_id: DeviceId,
    pub settings: AccountSettings,   // 各活动的开关与参数
}

/// 运行上下文：任务通过它访问一切能力
pub struct TaskContext {
    pub account: Account,
    pub http: Arc<dyn HttpClient>,
    pub store: Arc<dyn Store>,
    pub clock: Arc<dyn Clock>,
    pub sink: Arc<dyn EventSink>,   // 日志/进度上报
    pub state: Mutex<AccountState>, // 运行期可变状态
}
```

### trait 边界

```rust
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse>;
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn accounts(&self) -> Result<Vec<Account>>;
    async fn save_account(&self, a: &Account) -> Result<()>;
    async fn append_run(&self, r: &RunRecord) -> Result<()>;
    async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>>;
    // schedules / pipelines / webui prefs ...
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub trait EventSink: Send + Sync {
    fn emit(&self, ev: RunEvent);
}
```

---

## 4. 密码学（mcloud-crypto）

全部为标准原语，**纯 Rust 实现，无 FFI**。

### 4.1 请求签名（核心）

```
data = salt + id + timestamp + nonce + other + salt
sign = MD5(data)                      // 小写 hex

salt ∈ { 1 => "seedMdYYLIZfbCxg", 2 => "sekaMdYYLIZfbCfm" }
未知 salt_key 回落到 1
```

请求头：

| header | 值 |
|---|---|
| `x-request-id` | 随机 UUID |
| `x-timestamp` | 毫秒时间戳 |
| `x-nonce` | 随机 UUID |
| `x-signature` | 上式 MD5 |

`other` 为可选附加串（例如 `ssoToken=...&openAccount=0`）。

### 4.2 其他原语

| 用途 | 算法 | 参数 |
|---|---|---|
| 通用哈希 | SHA-256 | hex 小写 |
| 刷新令牌加解密 | AES-CBC/PKCS7 | key `2olBaQGYnEKoStYomsd1n7ax`，**动态 IV**，格式 `base64(IV‖CT)` |
| 通用 AES | AES-CBC/PKCS7 | 固定 IV `0102030405060708`，输出 hex |
| 旧协议 | DES-ECB/PKCS7 | 用于个别遗留接口 |
| 密码哈希 | SHA-256 | 前缀 `fetion.com.cn:` |

**动态 IV 约定**：`IV` 为 16 随机字节，与密文拼接后整体 base64。解密时前 16 字节即 IV。

### 4.3 黄金向量对拍

`test/golden/*.json` 存放权威向量，Rust 侧测试必须全部通过。
向量来源是 TS 原实现 + 真实抓包，**不重新发明**。

---

## 5. 上游接口层（mcloud-upstream）

### 5.1 认证域隔离

不同域名注入不同凭证，**不可混用**：

```
authorization: <basic>   仅注入 →  yun.139.com
                                  mcloud.139.com
                                  feixin.10086.cn
                                  mail.10086.cn

jwttoken: <jwt>          仅注入 →  caiyun.feixin.10086.cn
                                  mrp.mcloud.139.com
                                  m.mcloud.139.com

deviceid: <smid>         注入所有请求
```

### 5.2 客户端工厂

```rust
/// 通用客户端
pub fn create_http(opts: HttpOptions) -> impl HttpClient;
//  默认 POST、30s 超时、retry 0
//  UA: Android Chrome 128 移动端串
//  content-type: application/json;charset=UTF-8

/// 任务客户端（活动类接口）
pub fn create_task_http(device_id, jwt_token, referer) -> impl HttpClient;
//  附加: appversion 12.6.0.0 / activityid sign_in_3
//        origin https://m.mcloud.139.com
```

### 5.3 认证生命周期

```
auth(base64) ──parse──> {platform, phone, token, expire}
                              │
                              ├─ querySpecTokenV2 ──> ssoToken
                              │      POST user-njs.yun.139.com/user/querySpecTokenV2
                              │      body {toSourceId}   (默认 001005)
                              │      5 次重试，指数退避 1s·2^i
                              │
                              └─ tyrzLogin ──────────> jwtToken + userDomainId
                                     POST m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin
                                     body {token, openAccount:0, marketName, sourceId}
```

**刷新**：剩余天数 < 阈值（默认 10 天）时触发
`POST aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do`（XML 报文），
成功后重算 basic auth 并**原子写回**配置文件（保留 `.bak`）。

### 5.4 错误语义

```
code ∈ {"0","0000","200"} 或 resultCode == 0  → 成功
其余 → 业务错误，携带 code + message
```

网络层错误与业务错误分离：前者 `Err`，后者 `Ok(Envelope::Err{..})`。

---

## 6. 引擎（mcloud-engine）

### 6.1 任务抽象

```rust
#[async_trait]
pub trait Task: Send + Sync {
    fn name(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// 是否在 UI 中隐藏（如兑换）
    fn hidden(&self) -> bool { false }
    async fn run(&self, ctx: &TaskContext, body: Value) -> Result<TaskOutcome>;
}
```

`TaskOutcome` 携带结构化结果（成功/跳过/失败 + 摘要 + 明细），
前端可直接渲染，而不是让 UI 去解析日志文本。

### 6.2 注册表

```rust
pub struct Registry { tasks: BTreeMap<&'static str, Arc<dyn Task>> }
impl Registry {
    pub fn register(&mut self, t: Arc<dyn Task>);
    pub fn get(&self, name: &str) -> Option<Arc<dyn Task>>;
    pub fn list(&self) -> Vec<TaskMeta>;   // 供 /api/capabilities
}
```

### 6.3 运行器与事件

```rust
pub enum RunEvent {
    Started   { run_id, task, account },
    Log       { run_id, level, message, at },
    Progress  { run_id, step, total, label },
    Finished  { run_id, outcome, duration_ms },
}

pub struct Runner { registry, store, clock, bus }
impl Runner {
    /// 同步执行
    pub async fn run(&self, task, account, body) -> Result<RunRecord>;
    /// 异步执行，返回 run_id，事件经 bus 推给 SSE
    pub async fn spawn(&self, task, account, body) -> RunId;
}
```

**并发闸**：以 `task:account` 为键的互斥，避免同一账号同一任务并发。

### 6.4 任务组合（pipeline）

```rust
pub struct Pipeline {
    pub id: String,
    pub name: String,
    pub steps: Vec<PipelineStep>,     // 有序
    pub accounts: Vec<AccountId>,     // 共享账号集
    pub account_mode: AccountMode,    // Sequential | Parallel
    pub stop_on_error: bool,
}
pub struct PipelineStep {
    pub task: String,
    pub body: Value,
    pub continue_on_error: bool,
}
```

---

## 7. 任务清单（mcloud-tasks）

19 个任务，按域分组：

| 组 | 任务 | 说明 |
|---|---|---|
| 基础 | `sign` | 网盘签到 + 云朵奖励 |
| | `receive` | 领取待领云朵 |
| | `revival` | 复活奖励（每月 2 次） |
| | `task-expansion` | 任务扩展（含上传） |
| | `msg-push` | 消息推送开关 + 奖励 |
| | `notice` | 短信/邮件通知开关 + 奖励 |
| | `backup-gift` | 备份好礼 |
| | `app` | App 内任务 |
| 活动 | `ai-avatar` | AI 新头像 + 抽奖 |
| | `play-ai-special` | 拍拍大作战 |
| | `play-ai-lottery` | 趣玩 AI 抽奖 |
| | `cloud-phone-redpack` | 红包派对 |
| | `hc1t` | 云朵大作战 |
| 直播 | `kouling` | 直播口令（WebSocket + 小红书） |
| | `red-flower` | 小红花领取 |
| 奖品 | `prizes` | 待领取奖品列表 |
| | `exchange` | 兑换奖品（UI 隐藏） |
| 维护 | `auth-refresh` | 凭证刷新 |
| | `after` | 清理临时文件 |

活动类任务由账号级开关控制（`AccountSettings`），默认关闭的需显式开启。

---

## 8. 持久化（mcloud-store）

**脚手架阶段用 JSON + 原子写（tmp + rename）**，接口按可替换设计。

```
data/
  config.json     账号 + 全局设置（含 .bak 备份）
  state.json      每账号运行期状态
  history.json    运行历史（保留最近 200 条）
  schedules.json  定时任务
  pipelines.json  任务组合
  webui.json      前端偏好（记住上次页面等）
```

**演进路径**：`Store` trait 已抽象，后续可加 `SqliteStore`（sqlx）替换，
用于历史查询与并发安全。当前不引入 sqlx 以避免编译负担。

---

## 9. HTTP 服务（mcloud-server）

### 9.1 路由契约

```
GET    /api/version
GET    /api/capabilities            任务清单（替代 /api/tasks）
GET    /api/accounts
GET    /api/accounts/{id}/cloud-stats
GET    /api/config
PUT    /api/config

POST   /api/runs                    启动运行 {task, account, body, mode}
GET    /api/runs                    运行列表
GET    /api/runs/{id}
GET    /api/runs/{id}/events        ★ SSE 事件流
GET    /api/history?limit=

GET    /api/schedules
POST   /api/schedules
PUT    /api/schedules/{id}
DELETE /api/schedules/{id}
POST   /api/schedules/{id}/run

GET    /api/pipelines
POST   /api/pipelines
PUT    /api/pipelines/{id}
DELETE /api/pipelines/{id}
POST   /api/pipelines/{id}/run

GET    /api/prizes                  待领取奖品
```

### 9.2 SSE 事件流

`GET /api/runs/{id}/events` 返回 `text/event-stream`：

```
event: log
data: {"level":"info","message":"签到成功","at":1737000000000}

event: progress
data: {"step":2,"total":5,"label":"领取云朵"}

event: finished
data: {"outcome":"success","durationMs":1832}
```

前端用 `EventSource` 订阅，实现**运行过程实时可见**。

### 9.3 单二进制

`rust-embed` 内嵌 `web/dist`，SPA fallback 到 `index.html`。
开发态走 Vite dev server + `/api` 代理。

---

## 10. 前端（web/）

### 10.1 技术栈

```json
{
  "vue": "3.6.0-rc.9",          // Vapor 运行时
  "vue-jsx-vapor": "^3.2.24",   // TSX → Vapor 编译
  "tailwindcss": "^4", "daisyui": "^5",
  "vite": "^8"
}
```

Vapor 模式**无虚拟 DOM**：组件编译为直接 DOM 操作，运行时开销更低。
TSX 写法保留类型安全与组合能力。

### 10.2 目录

```
web/src/
  main.ts           入口，createApp
  app.tsx           壳：侧边栏 + 路由
  api.ts            REST 封装 + SSE 订阅
  stores/           响应式状态（Vue reactive）
  components/       通用组件
  pages/            页面
```

### 10.3 页面

| 页面 | 职责 |
|---|---|
| `dashboard` | 账号概览、云朵统计、最近运行 |
| `tasks` | 任务列表 + 参数表单 + 运行 + 实时日志 |
| `accounts` | 账号增删改、凭证状态 |
| `schedules` | 定时任务 CRUD |
| `pipelines` | 任务组合编排 |
| `prizes` | 待领取奖品 |
| `history` | 运行历史 |
| `settings` | 全局配置、推送渠道 |

### 10.4 与后端契约

统一经 `api.ts`，不散落 `fetch`。
SSE 封装为 `subscribeRun(runId, handlers)`，返回取消函数。

---

## 11. 测试策略

| 层 | 方式 |
|---|---|
| `mcloud-crypto` | 黄金向量对拍（`test/golden/*.json`），必须逐条相等 |
| `mcloud-upstream` | fixture 回放：录制真实响应，离线断言解析 |
| `mcloud-engine` | 假 Task + 假 Store，断言事件序列与并发闸 |
| `mcloud-server` | `tower::ServiceExt::oneshot` 路由级测试 |
| web | 类型检查 `tsmc --noEmit` + 关键组件测试 |

**随机性路径**（动态 IV、RSA 填充）不做字节级对拍，改为往返/自持密钥验证。

---

## 12. 安全与合规

- 凭证仅存本地，配置目录权限 0600，写盘保留 `.bak`。
- 日志**脱敏**：`auth`/`token`/`jwttoken` 一律掩码，禁止整串落盘。
- 不内置任何他人账号或批量注册能力。
- 请求频率保守，遵循上游限流；失败指数退避而非重试轰炸。

---

## 13. 里程碑

| 阶段 | 交付 |
|---|---|
| **P0（本阶段）** | 设计定稿 + 可编译脚手架 + 前端骨架 |
| P1 | `mcloud-crypto` 全向量通过 + 签名头可用 |
| P2 | `mcloud-upstream` 认证链路跑通（真实登录） |
| P3 | `sign` / `receive` / `after` 三个任务端到端 |
| P4 | `mcloud-server` 路由 + SSE + 前端任务页联通 |
| P5 | 调度 + pipeline + 推送渠道 |
| P6 | 其余任务增量接入 |
| P7 | 单二进制收口（rust-embed） |
