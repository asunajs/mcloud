# mcloud 架构设计

> 移动云盘（中国移动云盘 / 139 / 和彩云）自动化平台。
> 后端 Rust，前端自研 **`lite`**（无虚拟 DOM + 编译期优化）+ TSX。
> ⚠ 前端这一侧本文件只留**契约与结论**；现状与实测读数以
> [`lite-framework.md`](lite-framework.md) 为准（原先写的 Vue Vapor 栈已于 2026-09-30 拆掉）。
>
> 状态：**设计定稿 + 脚手架**。本文件是唯一设计来源。

---

## 1. 目标与非目标

### 目标
- 多账号管理：凭证解析、有效期跟踪、自动刷新、持久化。
- 任务自动化：签到、领AI豆、备份、AI 类活动、直播口令、兑换奖品等。
- 可观测：每次运行有结构化日志与事件流，前端实时可见（SSE）。
- 可调度：cron 定时 + 任务组合（pipeline）。
- 单二进制交付：前端产物内嵌，`mcloud serve` 一条命令跑起来。
- 可测试：密码学与协议层用**黄金向量**对拍，不依赖网络。

### 非目标
- 不做批量注册、刷量、商业分发。仅个人账号自用。
- 不追求覆盖上游全部 130+ 接口，按任务需要增量接入。

---

## 2. 分层与依赖铁律

依赖只能**自上而下**，反向依赖一律视为设计错误。

```
                    ┌──────────────────────────┐
                    │   mcloud（唯一二进制）    │  ← 不带参数 = 起服务；带子命令 = CLI
                    └────────────┬─────────────┘
                                 ▼
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
| `mcloud-server` | 库（axum 路由 + SSE + 内嵌前端）**＋ 唯一入口**（`src/main.rs` 按 argv 分流） | ✅ |
| `mcloud`（二进制） | 不带参数 = 服务；带子命令 = CLI（`src/cli/run.rs`）。**2026-10-03 起只有一个 bin** —— 原来 `mcloud-server` 与 `mcloud-cli` 两个，实测合并后只多 116 KB、省掉一整个 3.12 MB 的二进制 | ✅ |

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
    pub expire: i64,       // epoch **毫秒**（不是秒，见 §5.3 的说明）
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

deviceid: <smid>         注入所有请求（值由 smid 系统换取，见 §5.6）
```

### 5.2 客户端装配

⚠ **没有 `create_http(HttpOptions)` / `create_task_http(...)` 这类工厂** ——
全仓零命中（`grep -rn "create_http\|HttpOptions" crates --include="*.rs"`）。
真实的装配在 `mcloud-app/src/builder.rs`：用 `ClientProfile`
（`basic_token` / `jwt_token` / `device_id` / `user_agent` / `timeout_secs` / `pc` 形态）
构造 `mcloud_upstream::ReqwestClient`，再按需要装饰。
活动类接口的 `appversion` / `activityid` / `origin` 等**不是客户端级配置**，
而是各请求自己带头（见 `mcloud-upstream::app_signin::h5()` 一族）。

装饰器栈（`builder.rs` 实际装配的顺序）：

```text
SmidClient        ← 懒加载补 deviceid 头（第一次请求时才去换）
  └ H5Client      ← 建会话、带 jwttoken、90001 刷新重放
      └ ReqwestClient  ← 真正发请求（按域名注入 authorization；UA/超时来自 ClientProfile）
```

`SmidClient` 必须在**最外层**：它补的头要作用于所有出去的请求，
包括 `H5Client` 建会话那三枪。

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

⚠ `expire` 的单位是 **epoch 毫秒**（13 位），不是秒。上游 token 第 4 段
就是毫秒（黄金向量 `1893456000000`），而旧实现曾按秒处理 ——
后果不是报错，而是把"已过期"算成公元 61968 年、永远不刷新，
直到上游回 90001 才发现。详见 `docs/conventions.md` §7.2。

### 5.4 H5 会话层

签到、领取、任务列表这些动作**全部由 H5 发出**，它们要的不是 `Authorization`
而是 `jwttoken`。拿票要打三枪：

```
① POST /user/querySpecTokenV2      （原生 A 族头，带 Authorization）
      {"toSourceId":"001005"}  →  SSO token
② GET  /ycloud/api/timestamp       （无鉴权）→ 13 位毫秒
③ POST /ycloud/auth-service/auth/tyrzLogin  （带 x-signature）
      → jwttoken（905 字符）
```

**这三条是一条链，不是三个接口。** 漏掉第 ③ 步的表现是"每个业务接口都报
`90001 未登录`"—— 看上去像鉴权坏了，实际是会话没建。

#### 票是"每页一套配方"，不是全局一张

mcloud 的实拍结论：**每个页面用自己的参数换自己的票，票与票之间逐字节不等**。
所以键叫 [`SessionRecipe`]，而不是"页面名"：

| 页面 | `marketName` | `sourceId` |
|---|---|---|
| 签到页 | `sign_in_3` | `1002` |
| 备份有礼页 | `backupgift` | `1134` |
| 开启通知页 | `msgpushon` | `1062` |
| 拍拍门户页 | `National_playAI` | `1044` |

目前**只有签到页有完整实拍**，其余页面在拿到自己的实拍前映射到它。
但代码**一次就按配方建模** —— mcloud 自己吃过亏：它先把换票参数写死在
`OpenSignInSession` 里，后来发现各页有自己配方，切的时候"动的是**共享会话层**"。

#### 缓存只在内存，不落地

`jwttoken` 2 小时即废，而库里那一列（`account_state`）装的是**登录凭据** ——
同名不同物。落一张两小时就死的票，换来的是"重启后读到一张过期票"这一整类难查现象。
进程重启就重取，代价是三条请求。

**SSO token 连内存都不缓存**：实测"14:38:54 换到的 token，14:42:09 重放已被拒"。

#### 不加 TTL，只认 `90001`

抓包只给了"这张票 2 小时即废"，**没给**"该在哪个时刻算它过期"的依据。
所以失效判据只有上游回的那一句 `90001`，不自造时钟。

代价说清楚：票自然过期后的第一次请求会先撞一次 `90001`，然后刷新并
**重放那一条**（不是整轮）。用户看到的是"多等一次往返"，不是失败。
反过来写死 2 小时重建，那是拿客户端猜测替换服务端语义。

> ⚠ **重放的是"那一条"，不是"整轮"。** 重放整轮的表现是"某个写请求跑了两次"，
> 而它在余额上看不出、日志里不报错 —— 只有把**请求序列**钉住才抓得到。

#### 透明接入

`H5Client` 是 `HttpClient` 的装饰器，只接管 [`JWT_HOSTS`] 那一族：

- 任务代码**不需要知道有"会话"这回事**，只管发业务请求；
- `90001` 刷新重放**只有一处实现**（注册表里每个任务各写一遍必然分叉，
  而分叉的表现是"某个任务在票过期时静默失败"；任务条数现算：
  `grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs`）；
- 原生族（`user-njs.yun.139.com`）**原样透传** —— 对它建会话是白打三枪，
  而且那三枪的第一条就在这个主机上，会绕回自己。

按**主机**分派而不是"给每个任务传一个 jwt 字符串"：mcloud 踩过的坑是
"把直播侧 JWT 传进了云盘侧的参数位"，根因是**类型把两套凭据降级成了同一个
`string`** —— 摆错顺序在编译器和测试眼里都合法。按主机分派让这个位置不存在。

### 5.5 错误语义

```
code ∈ {"0","0000","200"} 或 resultCode == 0  → 成功
其余 → 业务错误，携带 code + message
```

网络层错误与业务错误分离：前者 `Err`，后者 `Ok(Envelope::Err{..})`。

### 5.6 smid 设备指纹（`deviceid` 从哪来）

`deviceid` **不是本地算出来的串**，是中国移动统一认证（cmpassport）的风控服务
发下来的：客户端把一份浏览器特征加密上报，服务端回一个 `deviceId`，
客户端在它前面拼一个字母 `B` 当自己的设备 id 用。

```
uid = randomUUID()
  ├─ ep     = RSA(uid)              公钥 SKLAND_PUBLIC_KEY_B64
  └─ priId  = md5(uid)[0..16]       下面那份特征的 AES 密钥
特征体（约 30 个字段，含一个当场随机出来的 smid）
  → tn = md5(getTn(特征体))          ← 先算摘要，再写回 tn 字段
  → 按 25 条 DES 规则逐字段加密 + 改名（appId→um、ua→pu、…）
  → JSON → gzip（OS 字节写 19）→ base64
  → AES-CBC/PKCS7 固定 IV，密钥 priId → hex   = data
POST https://slw.h5cmpassport.com:9090/deviceprofile/v4
  → code == 1100 → deviceId = "B" + detail.deviceId
```

#### ⚠ 绑定账号的是**特征**，不是 smid，也不是 deviceId

这三样生命周期完全不同，混起来就会把该变的东西钉死：

| | 会不会变 | 本项目的处理 |
|---|---|---|
| **设备特征**（屏幕、dpr、platform、canvas、cpu 数、ua…） | 真机上**不变** | **绑定到账号**：首次生成一份落库，之后一直复用 |
| **smid 字段** | **每次都变**（内嵌 `yyyyMMddHHmmss` 与现生成的 uuid） | 每次上报现算，不落库 |
| **deviceId**（`B…`） | 服务端发的，会变 | 每次上报重新取；库里那个只是"最近拿到的那一个" |

旧实现每跑一次任务就重新随机一份特征，等于同一台机器每次上报都长成另一台机器的
样子 —— 那正是最容易触发风控核验的姿态。本项目不跟：特征生成一次、按账号存下来
复用，**只在主动更换指纹时**才重生成。

#### 落点

| 层 | 职责 |
|---|---|
| `mcloud-crypto` | 纯原语：DES/AES/gzip(OS=19)/RSA/getTn/`Random` |
| `mcloud-upstream::smid` | 算法：`SmFingerprint`、`smid_string`、`private_id`、`ep`、`build_sm_data`、DES 规则表、`parse_device_id` |
| `mcloud-core::SmidStore` | 存储契约（**窄写**：换指纹连带清 deviceId） |
| `mcloud-store` | `smid.json`，**与 `state.json` 分开**（见下） |
| `mcloud-app::smid` | 生命周期：ensure / rotate / report / device_id |
| `mcloud-app::SmidClient` | 请求路径上懒加载补 `deviceid` 头 |

#### 为什么 `smid.json` 与 `state.json` 分开

`save_account_state` 是**整块覆盖**写入。若指纹也放在里面，登录路径（它根本不知道
指纹的存在）会顺手把它清掉 —— 而"换凭据不该换设备长相"正是这里要保证的。
分开存之后两边的写互不可见，窄写才有意义。

#### 取 id 失败时不阻断业务请求

`SmidClient` 取 id 要打第三方风控服务，它可能超时。这时**原样发出去、只是不带那个头**，
而不是让业务请求失败 —— 一个风控服务的抖动不该让签到跑不起来。缺头的后果由上游按
自己的规则回错，那才是权威判据。

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
    Started   { run_id, task, account, at_ms },
    Log       { run_id, level, message, at_ms },
    Progress  { run_id, step, total, label },
    Finished  { run_id, outcome, duration_ms },
}

pub struct Runner { registry, store, clock, sink, http_for, next_id, inflight, cancel }
impl Runner {
    /// 同步执行，返回最终记录
    pub async fn run(&self, task, account, body) -> Result<RunRecord>;
    /// 后台执行，**立刻**返回真实 run id
    pub fn start(self: &Arc<Self>, task, account, body) -> RunId;
    /// 带链令牌的版本，供编排把"中止整条链"传进每一步
    pub async fn run_with_chain_cancel(..) -> Result<RunRecord>;
}
```

**并发闸**：以 `task:account` 为键的互斥。到点仍在跑 ⇒ **跳过，不排队** ——
排队会让定时任务在慢上游面前无限堆积，掩盖真实问题。

**id 分配只有一个来源**：`start()` 先分配 id 再 spawn，因此调用方拿到的是
**真实 id**。这不是细节 —— 界面要能点"中止"，就必须先有可指认的 id；
同步执行时请求还没返回，用户根本没有可中止的时机。

#### 生命周期与业务结论是两件事

```rust
pub enum RunStatus { Running, Done, Cancelled }   // 存储层事实
pub enum TaskOutcome { Success, Failed, .. }      // 业务结论
```

刻意**不合并**：`Done` 里可能是"全成/部分成/需人工"，拿它去染红绿
等于替后端编一个它没给过的结论。中止记 `Cancelled` 而不是 `Failed` ——
用户自己点的中止不该计入错误率、不该告警、推送里也不该报红。

#### 中止是协作式的

`CancelToken` 只置标志，不打断正在进行的请求。任务在**安全点**
（每轮循环、每个请求之间）调 `ctx.check_cancelled()?` 主动退出。

直接 drop 一个正在发请求的 future 会留下**已发出但结果没人处理**的请求：
签到真的打到上游了，本地却记成"已中止"，用户再点一次就是重复签到。
代价是中止不是瞬时的；换来的是"中止"不会造成半个副作用。

任务若不检查，取消就只是"不再开始新步骤"—— 这是刻意的默认，
比留下不一致的状态好。

`TaskContext` 同时持**两个**令牌：本次运行的（`cancel`）与整条链的
（`chain_cancel`）。只看一个的话，另一种中止方式对它就是无效的。

#### 取消登记表

按 **run id** 而不是 `task:account` 登记：用户点中止时给的是 run id，
用 `task:account` 当键会连带停掉同一账号另一个用户没想停的运行。

`is_running` 与 `is_cancelled` 是两件事：已请求中止的运行在任务退到
安全点之前**仍在运行**。混为一谈会让界面在任务还在跑时就显示"已停止"。

### 6.4 事件中枢与日志

`mcloud-app` 的 `EventHub` 负责序号分配、内存重放、日志落盘与 SSE 扇出。

**为什么不用 `tokio::sync::broadcast`**：

1. **迟到的订阅者拿不到历史** —— 而"运行已开始、用户才打开页面"正是最常见的场景
2. **订阅者互相影响** —— 环形缓冲按**最慢**的订阅者推进，一个卡住的客户端
   会让其他人一起丢事件；而卡住的 SSE 客户端是常态（手机切后台）
3. **序号语义得自己维护** —— 而序号正是断线续传的锚点

所以是"**每个 run 一份内存历史 + 每订阅者一条独立队列**"。

**双源**：内存历史 = 在线那一半（不必等落盘）；存储层 = 持久那一半
（重启、迟到者、断线重连）。缺一不可：只发内存则刷新页面就丢历史，
只落库则前端得轮询。

**顺序：先订阅，再读库。** 订阅会把 `since` 之后的内存历史补进队列，
于是"读库"与"订阅"之间那个窗口里发布的事件不会两头都漏掉。反过来必漏。

**队列满 ⇒ 丢最新的，并报出丢了多少。** 丢最老的会让前端从中间开始，
进度条与日志都对不上；丢最新的只是"后面几条没看到"，且下次续传能接上。
丢弃计数**累计且不自动清零** —— 静默少了半截日志是最难查的形态。

#### `Seq`：两个命名构造器

"已看到的序号"与"下一个要取的序号"都是整数、**只差 1**。混淆会**静默跳过一条**。

```rust
Seq::seen(3)         // 客户端已经看到 3，下次要 4
Seq::resume_after(3) // 从 3 之后继续（内部值相同，意图不同）
```

数值相同、语义不同，所以调用点必须**显式选一个**。`next()` 饱和而不是回绕 ——
回绕会让序号变小，于是"大于 after"这个续传判据永远为假，那次运行从此
再也发不出任何事件，而且是个到不了却彻底静默的分支。

**SSE 契约**：终局事件（`finished`）后服务端**立即收尾**，客户端**必须
自己 `close()`**。`EventSource` 按规范会自动重连，不关的话一次已结束的
运行会被无限重连，每个打开的页面挂着一条永久连接。

> 这类"X 之后要**及时**结束"的性质，不能用"最终读到的内容"当判据 ——
> 内容只证明**发对了**，不证明**收尾了**。实测 0.06 秒关闭。

### 6.5 调度器

**三条硬规则**，都是踩过坑才立下的：

1. **先推水位，再打上游。** 水位（`last_fire_ms`）必须在触发上游**之前**落盘。
   反过来的话，进程在跑任务时被杀 ⇒ 重启后同一个点会再跑一次，而签到、
   用口令领小红花在上游那侧**不是幂等的**。结论：**宁可漏跑一次，不可重跑一次。**

2. **错过的整串一次弃掉，绝不逐点补跑。** 停机 8 小时后再启动，若把错过的
   每个点都补一遍，等于"连打 8 趟上游"。

3. **迟到一点不算错过。** 一轮 tick 被写盘顶住是常态，用 `GRACE_MS`（60 秒，
   即 2 个 tick）把"被顶了一下"和"进程真的停了"分开。真停机迟到的是
   分钟到小时级，一定越过这条线。

`last_fire_ms`（水位线）与 `last_run_ms`（上次运行时间）**必须分开**：
错过的点被弃掉时不产生运行记录，用后者当水位会让下一次 tick 把同一个点
重新捞出来 —— 表现成"每次刷新多一条 skipped"。

**首个 tick 立刻跑，不等一个间隔** —— 定时器的第一次触发是在一个间隔之后，
那样每次重启都要白等 30 秒，而重启恰恰是"错过今天那一趟"最常见的原因。

**5 段 cron 补 `0` 秒，不补 `*`**：补 `*` 会把"每天 7:30"变成
"7:30 那一分钟里每秒跑一次"—— 60 倍请求量，而且完全不报错。

**不支持的时区明确报错**，不静默按 UTC 算 —— 静默按 UTC 会让任务在
错误的钟点跑，且没有任何提示。

### 6.6 任务编排

```rust
pub struct Pipeline { id, name, accounts, account_mode, stop_on_error, steps, enabled }
pub struct PipelineStep { task, body, continue_on_error }
```

**执行顺序：步骤在外，账号在内**。差别在于上游看到的请求交错方式；
每步内部仍是**逐账号串行** —— 防风控的节奏没有被放宽。

**一次性预检**：在发出**任何**真实请求之前，先校验整条链的步骤是否都存在。
否则"第 3 步任务名拼错"的表现是：前两步已经把真上游打了一遍（签到是写操作），
然后整条链返回一个错误。

**失败语义是两层**（都有才够用）：

| 层级 | 字段 | 含义 |
|---|---|---|
| 步骤级 | `continue_on_error` | 这一步失败算不算数（比编排级更具体，优先） |
| 编排级 | `stop_on_error` | 整个流程还要不要走 |

只有一层时，要么无法跳过非关键步骤，要么无法在关键步骤失败时止损。

中止后**仍然把剩余步骤记为 `Skipped`**，让结果数组形状稳定 ——
前端不必处理"长度会变"。`stopped_after` 必须记下来：否则"第 2 步失败、
按设定继续"与"第 2 步失败、后面 8 步没跑"在界面上长得一模一样。

**整体状态单调**：error > partial > ok，后面成功不能把前面的失败翻绿。

**`Parallel` 明确报错**而不是静默按串行跑：静默接受等于在界面上留一个
后端做不到的开关，用户以为并发了、实际没有，只会觉得"怎么这么慢"。

## 7. 任务清单（mcloud-tasks）

注册表里的任务（**条数别背数**，现算：`grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs`），
按域分组。**名单以 `crates/mcloud-tasks/src/lib.rs` 的 `register_all` 为唯一权威**：

| 组 | 任务 | 说明 |
|---|---|---|
| 签到族 | `sign` | 网盘签到 + AI豆奖励 |
| | `receive` | 领取待领AI豆 |
| | `multiple` | 签到翻倍 |
| | `backup-gift` | 备份好礼 |
| | `revival` | 复活奖励（每月 2 次） |
| | `prizes` | 待领取奖品列表 |
| | `cloud-snapshot` | AI豆余额快照 |
| | `task-list` | 任务清单 |
| 直播 | `live-room` | 直播口令 → 小红花（WebSocket + 小红书） |
| | `red-flower` | 小红花领取 |
| 活动 | `exchange` | 兑换奖品（**真扣AI豆、不可回滚**） |
| | `msg-push` | 消息推送开关 + 奖励 |
| | `fun-ai` | 趣玩 AI 抽奖 |
| | `notice` | 开启通知领AI豆 |
| | `play-ai-special` | 拍拍盘点（只读那两口） |
| | `ai-avatar` | AI 新头像 + 抽奖（**默认关**） |
| | `cloud-phone-redpack` | 红包派对 |
| App 任务 | `app-task-matrix` | App 任务矩阵（**只读那一半**） |
| | `app-task` | App 任务做任务（写侧：逐条 `task/click` + 传文件） |
| 任务拓展 | `task-expansion` | 任务拓展（读 → 凑备份 → 领 → 删自己传的文件） |

活动类任务由账号级开关控制（`AccountSettings`），默认关闭的需显式开启。

---

## 8. 持久化（mcloud-store）

**接口按可替换设计，当前只提供 JSON 后端。**

业务代码只依赖 `mcloud-core` 的五个子 trait
（`AccountStore` / `RunStore` / `LogStore` / `ScheduleStore` / `PipelineStore`），
组合成的 `Store` 是唯一的注入点。换后端 = 新写一个实现了这五个 trait 的类型，
**不动任何业务代码**。

> 拆分依据是**消费者**而不是表：任务只需要 `LogStore`，不该被迫依赖账号读写；
> 合一个胖 trait 会让"只想给任务写个内存假实现"的人被迫实现十几个
> 用不到的方法 —— 而假实现正是保证函数级可测的手段，不能让它的成本
> 反过来逼人不写测试。

```
data/
  accounts.json    账号（含 .bak 备份）
  state.json       每账号运行期状态
  history.json     运行历史（保留最近 200 条）
  schedules.json   定时任务
  pipelines.json   任务编排
  logs/<run_id>.json  每次运行的日志
```

### 8.1 并发模型（JSON 后端）

**所有数据常驻内存，用 `RwLock` 保护；写操作先改内存再写透磁盘。**

为什么不是"每次操作都读文件、改、写回"：那种写法在两个请求同时落盘时
会**互相覆盖** —— A 读到 v1、B 也读到 v1，A 写 v2、B 写 v2（丢掉 A 的改动）。
读-改-写三步之间只要没有锁，就一定会在并发下丢更新。

三个刻意的取舍：

1. **用 `std::sync::RwLock` 而不是 `tokio::sync::RwLock`。** 后者要求
   `.await` 持锁，于是"持锁期间做文件 IO"就变成合法写法，而文件 IO 在
   异步上下文里阻塞执行器。用 std 锁 + 全程不 await，编译期就杜绝了
   "跨 await 持锁"。
2. **锁内做阻塞文件 IO。** 这些文件都在 KB 量级，一次写盘是微秒级。
   日志是例外，按 run 攒批。
3. **写盘是"写临时文件 + rename"**。rename 在同一文件系统内原子 ⇒
   进程被杀不会留下半截 JSON。覆盖前留 `.bak`，但**不**自动恢复 ——
   静默回滚到旧数据比报错更难排查。

**只保证进程内并发安全。** 两个进程同开一个数据目录会互相覆盖 ——
这不是缺陷而是范围声明：本工具是单实例的。

### 8.2 窄写

凡是"可能与用户编辑冲突"的写入都用**只改该改的列**的方法：

| 方法 | 只改 | 为什么不整行覆盖 |
|---|---|---|
| `mark_schedule_fired` | 水位线 + 计数 | 调度器手里那行是几秒前读的 |
| `record_schedule_run` | 运行时间/状态/错误 | 同上，且一次跑可能几十秒 |
| `mark_pipeline_ran` | 上次执行时间 | 一次编排要跑几分钟 |

`record_schedule_run` 的两个可选参数**语义刻意不对称**：

- `run_ms = None` ⇒ **不动**这一列（错过的触发点被弃掉时不产生运行，
  抹掉真实的上次运行时间会显示成"从未执行"）
- `error = None` ⇒ **写入空**（清掉旧错误本身就是结论；否则"成功"旁边
  挂着一条旧错误，正是查不出原因的形态）

### 8.3 启动恢复

进程被杀时会留下永远 `running` 的记录（来不及回填状态）。启动时把
它们标成 `Cancelled` 而不是 `Failed`：它不是任务失败，是进程没了。
不处理的话前端会一直显示"进行中"，用户等一个永远不会来的结果。

### 8.4 演进路径

SQLite 后端尚未实现，但 `Store` 的形状已按"能换"设计好，
不是"以后要拆"的占位。判定标准：
**有没有可能存在一个不是本机跑出来的库** —— 有，就只能加列、不能改列。

---

## 9. HTTP 服务（mcloud-server）

### 9.1 路由契约

```
── 登录闸（公开部分）────────────────────────────────────────────
GET    /api/setup                   是否已初始化 + 口令长度下限
POST   /api/setup                   首次建管理员（已初始化则拒绝）
POST   /api/session                 登录
DELETE /api/session                 登出
GET    /api/session                 当前会话（**需登录**）
GET    /api/health
GET    /api/version

── 以下全部需要登录 ─────────────────────────────────────────────
GET    /api/version
GET    /api/status                  后端自述（含调度器运行时实况）
GET    /api/health
GET    /api/capabilities            任务清单

── 云盘账号（与登录闸是两件事）────────────────────────────────
GET    /api/accounts                账号列表（**不含凭据**）
POST   /api/accounts                用一条凭据建账号（**手机号已存在则只换凭据**）
PATCH  /api/accounts/{id}           改昵称 / 设备号 / 凭据（**部分更新**）
DELETE /api/accounts/{id}           移除账号及其本地凭据

⚠ `POST` / `PATCH` 的取舍（2026-10-01，账号页重构时定的）：

* **`POST` 是"建或换凭据"，不是"只在不存在时建"**。用户手上那条凭据会过期，
  过期后他拿到的还是一条 auth、想做的还是把这个号的凭据换掉；回 409 会逼他
  先知道账号主键（一串哈希）再猜该用哪个端点。响应里用 `created` 说清是哪种。
  已存在的账号**只换 `auth`（+ `pc_platform`）**：昵称、设备号、每账号设置都保留
  —— 整行覆盖会让一次"换凭据"顺手抹掉用户自己配的东西，且悄无声息。
* **`PATCH` 是部分更新**，与 `/settings` 那条"整体替换"**刻意不同**：这里有一个
  字段是凭据，而凭据**从不发给客户端**（列表 DTO 里没有它）⇒"读出来、改一处、
  写回去"这条路不存在，整体替换会逼用户在每次改昵称时重新粘贴一遍凭据。
  缺席 = 不改；`nickname: ""` = 清空（回落成掩码手机号）。
* 两条都**先把所有字段校验完再落笔**：半套更新（昵称改了、凭据却是坏的）会让
  "哪部分生效了"变成要读代码才知道的事。
* 前缀决定了要什么：`pc` 凭据**必须**带 `platform`（它决定后续每一次请求的
  UA 与设备头，而凭据本身区分不出 win/mac）；`mobile` 凭据不接受形态（归 `null`）。
  ⚠ 从 auth 串导入的账号**没有 `user_domain_id`**（那个只有登录响应里才有）
  ⇒ 它的"刷新凭据"会明确报缺用户域。这是如实的能力边界：要能刷新就走一次真登录。

POST   /api/login/qr                扫码登录：出码
POST   /api/login/qr/poll           扫码登录：轮询
DELETE /api/login/qr/{sid}          取消扫码
POST   /api/login/sms/start         PC 端短信登录：发码
POST   /api/login/sms               PC 端短信登录：校验
POST   /api/login/app/sms/start     App 端短信登录：发码（另一套协议，无 platform）
POST   /api/login/app/sms           App 端短信登录：校验
POST   /api/login/password          账密登录
GET    /api/login/device            查看本机 PC 设备指纹
POST   /api/login/device/rotate     更换 PC 设备指纹

── 兑换（抢兑）─────────────────────────────────────────────────
GET    /api/accounts/{id}/exchange-prizes  奖品清单 + 每件判据档位（**只读**）
POST   /api/accounts/{id}/exchange         兑一件（**真扣AI豆**，走 task:account 并发闸）

⚠ 两条路由的**请求口径**不一样，别当成同一个：

只读那条**发两枪只读**：`exchangeList`（清单）+ `infoV3`（**余额**），
用 `plan_exchange(prize, Some(balance), now)` ⇒「余额不足」这一档判得出来，
界面上的"可兑"与下单前的真判据**一致**。

> ⚠ 2026-10-02 改过口径：之前这里用 `balance = None`（"不查余额，省一枪"），
> 于是判不出「余额不足」，界面的"可兑"比真判据宽，还要在每条清单头顶挂一句说明。
> 用户口径：「**这是为啥这么说，无余额你就后端请求啊**」——对的，
> 一次**只读**请求换判据准确（这条路本来就不碰滑块、不下单），值。
> `infoV3` 读失败**不**让整份清单失败：退回 `None` 口径，
> 响应里 `balanceAware: false` + `balanceNote` 如实说明，清单照常能看。
> `balance` 字段也一并回给前端（界面上直接显示余额，用户能自己判断要不要先攒豆）。

写那条走 `runner.run(...)`：并发闸以 `task:account` 为键，是"连点两下"与
"同一件扣两次豆"之间唯一的防线（这条链**零幂等键**）。

### 只读那条其实是**两枪**（2026-10-01 起）

`exchange-prizes` 除了清单那一枪，还会读一次**组字典** —— 它是同族的另一条
**静态、只读、免鉴权**接口：

```
GET /ycloud/signin/public/text/sign_in_3/prizeGroup.json      （上游，跨主机同族）
→ [{"groupId":1,"title":"视频类会员","subTitle":"(月卡每月多选一限兑)",…}, …]
```

* **`title` 是组名**（音乐类会员 / 视频类会员 / 外卖美食权益…）—— 界面分组标题直接念它，
  我们**不编**品类名；`subTitle` 是上游原话的**限兑规则**（"每月多选一限兑"），
  与商品上的 `limit == 4`（`exchanged_this_group`）是同一件事的两种表达。
* 路径里的 `sign_in_3` 由清单的 `marketName`（实拍 `sign_in_3_ex`）去 `_ex` 推出来 ——
  **只有一个观测点，是推导不是实拍** ⇒ 推不出来就**不发这一枪**，
  并让响应里的 `groupsNote` 把原因说出来（清单照常返回，组名退回"第 N 组"）。
* ⚠ **HTTP 200 + 0 字节是真实存在的**（实拍把路径换成 `sign_in_3_ex` 就是这样）：
  只看状态码会把"这个路径没内容"读成"上游说没有组"，所以解析层对空体**报错**。
* 逐字样本与四个坑：`../mcloud-capture/docs/capture/app-exchange-samples.txt` §1~§3。

视图新增字段：`totalCount`（活动总投放）/ `dailyCount`（今日投放）/
`groupTitle` / `groupSubTitle` / `monthQuantity`（上游 `quota`，**只用于显示**，
判据仍读 `limit`）/ `groupPrizeIds`（实拍**含自己**，算"同组另外几件"要减一）。
响应新增 `groupsNote`（`null` = 组名拿到了）。

⚠ `count` 是**池子剩余**，不是"今天还能不能抢"：实拍 `251230032` 是
`count = 43,739` 而 `minRemainderCount = 0`（H5 判抢光）⇒ 池子有货、**今日份额没了**。
界面必须分开写这两件事。

⚠ **份额是按自然日复位的**（2026-10-02 00:15 实测确认，样本 §7）：

* 跨 21:00、22:00 两个整点复拉：`min=0` 的件**一件都没恢复**
  （22:00 那次还从 26 涨到 33 ⇒ 只增不减，没有任何回补）；
* 跨过 **00:00** 再拉：样板件 `251230032` 从 `min=0 daily=0 count=43739`（抢光）
  变成 **`min=356 daily=356 count=43728`**，判据 `min_remainder_zero` ⇒ **`go`**。

⇒ 界面不许写"整点开抢"。

⚠⚠ **但也不能反过来断言"份额都在 0 点复位"** —— 用户口径（2026-10-02）：
「也不一定是 0 点，有些也不是」。上面那次实测只证明**那一款**按自然日复位，
复位时刻**因活动而异**（有的照活动自己的投放时刻，例如窗口写 `08:00` 的）。
所以兑换页「定时抢兑」的默认时刻是**按这件露出的口径猜的**
（有「今日份额」⇒ 下一个 0 点；只有「本组每月名额」⇒ 下月 1 日 0 点），
弹窗里写明"是猜的、不保证对"。

⚠ **复位 ≠ 清空**：当晚 `min=0` 的件数只从 33 变 34，`plan=go` 反而 18 → 17
—— 复位是**逐件**的（每件自己的每日投放量），不是整份清单重置；
`count`（池子）也不随复位变化（43739 → 43728，只少了别人兑走的 11 份）。

`mcloud-tasks::exchange::plan_exchange_wait`（`skipWait=false` ⇒ "等到下一个整点"）
在这条链上**只有目标本身就是整点**（比如 0 点）时才等于等到那一刻；
把它当成"等放量"是空等。见抓包样本 §5~§7。

POST   /api/runs                    启动运行 {task, account, body, background}
GET    /api/runs                    运行列表
GET    /api/runs/{id}
GET    /api/runs/{id}/events        ★ SSE 事件流
GET    /api/runs/{id}/logs          运行日志（?after= 断线续传）
POST   /api/runs/{id}/cancel        请求中止（202 语义）

GET    /api/schedules               定时列表（含服务端算的 nextFireAt）
PUT    /api/schedules/{id}          创建或更新（幂等）
DELETE /api/schedules/{id}

GET    /api/pipelines
PUT    /api/pipelines/{id}
DELETE /api/pipelines/{id}          被定时引用时拒绝并列出引用者
POST   /api/pipelines/{id}/run      后台执行，立即返回
```

**`POST /api/runs` 的 `background`**：默认 `false`（同步返回最终结果）。
`true` 时**立刻**返回真实 run id，任务在后台跑 —— 这是"界面上能点中止"
的前提：同步执行的话请求还没返回，用户根本没有可中止的时机。

**归属错误一律 404，不报 403** —— 那等于确认了"这个 id 确实存在"。

### 9.2 SSE 事件流

`GET /api/runs/{id}/events` 返回 `text/event-stream`，每条事件带 `id:`
（序号）供浏览器自动续传：

```
id: 1
event: started
data: {"type":"started","run_id":1,"task":"sign","account":"138...","at_ms":...}

id: 4
event: finished
data: {"type":"finished","run_id":1,"outcome":{...},"duration_ms":156}
```

**前端契约：收到终局事件（`finished`）后必须自己 `close()`。**
`EventSource` 按规范会自动重连，不关的话一次已结束的运行会被无限重连
（每次带 `Last-Event-ID`，服务端补发 0 条再关），每个打开的页面挂着一条
永久连接。`subscribeRun()` 已经内置了这一步。

**存在性判定要看三处**（且必须在 `subscribe` **之前**判，因为订阅会给
内存中枢建一个空条目）：

1. 存储层 —— 已结束的运行（记录在结束时才落盘）
2. 取消登记表 —— **正在跑**的运行
3. 内存中枢 —— 刚结束、记录还没落盘完的运行

只查存储层会漏掉第 2 类，而 `EventSource` 遇到 404 **不会重连**，
界面就永远停在"执行中"（实测卡过 60 秒以上）。

### 9.3 单二进制与同源

`rust-embed` 在**编译期**把 `web/dist` 内嵌进二进制：

- **同源**：一个进程同时发 API 与前端，因此**不需要 CORS**。
  开发态由 Vite dev server + `/api` 代理抹平差异，避免"开发能登录、打包上去就登录不了"。
- **构建顺序有依赖**：先 `npm run build` 再 `cargo build`。
- **SPA 回落**：未知路径给 `index.html`，但 `/api/*` 必须返回 404 而不是 HTML ——
  否则前端 `JSON.parse` 会报一个与真实原因无关的错。
- 资产缺失时（例如只跑 `cargo test`）不 panic，只是没有页面可发。

#### 传输层：gzip 与缓存

前端产物是大段重复文本，压缩比极高。实测口径（gzip -9）：CSS 约省 **85%**、
JS 约 **69%**、`index.html` 约 **35%**，合计约 **76%**。
⚠ **绝对字节别背**（产物每轮都在变，且现在已按页拆 chunk）——
要复现就对自己构建出的 `web/dist` 跑 `gzip -9`；运行时体积的现算命令见
[`lite-framework.md`](lite-framework.md) §5（`node web/lite/size.mjs`）。

不做压缩时首屏要多传几百 KB —— 这不是"优化项"而是缺陷，
`Content-Encoding` 是服务端的责任，浏览器不会替你压。

两条实现要点：

1. **`CompressionLayer`**（`lib.rs`）按响应头里的 `Content-Type` 判断可压性，
   所以 `static_files.rs` 必须发准确的类型，否则退化成
   `application/octet-stream` 会被静默跳过。
   它挂在 `TraceLayer` **之内**，日志记的才是客户端实际收到的字节数。
   SSE 不受影响：`tower-http` 的 `DefaultPredicate` **明确排除**
   `text/event-stream`、gRPC、图片与 <32 字节的响应。
2. **缓存策略按路径一分为二，且必须相反**：

   | 路径 | 策略 | 理由 |
   |---|---|---|
   | `assets/*` | `max-age=31536000, immutable` | Vite 文件名带内容哈希 ⇒ 同 URL 字节永不变 |
   | `index.html`（含 SPA 回落） | `no-cache` | 它是"指向当前哈希"的那张表 |

   `index.html` **绝不能**跟着长缓存：否则浏览器会拿旧 HTML 去要
   **已被删除**的旧哈希 JS ⇒ 发版后白屏、硬刷新才好。
   SPA 回落路径也必须 `no-cache`，否则就是"首页不缓存、深层路由缓存"，
   问题只会藏得更深。

回归测试锁在 `crates/mcloud-server/tests/routes.rs` 与
`src/static_files.rs` 的 `mod tests`（4 条：压缩生效、缓存分流、
`Content-Type` 准确、入口 HTML 永不 `immutable`）。
这些测试**不写死哈希**，且在前端未构建时**跳过而非失败** ——
保持"`cargo test` 不依赖 npm"这条既有约定。

---

## 10. 前端（web/）

### 10.1 技术栈

```jsonc
{
  // 运行时就是仓库里的 web/lite/（编译器 + 运行时），**不是** npm 包
  "lite": "web/lite/",
  "oxc-parser": "^0.152",                  // TSX 解析（web/lite/ast.ts 是适配层）
  "@typescript/native-preview": "^7.0.0-dev.20260707.2",  // 类型检查：npm run typecheck = tsgo --noEmit
  "tailwindcss": "^4", "daisyui": "^5",
  "vite": "^8"
}
```

⚠ `web/package.json` 里**没有** `dependencies` —— 运行时是仓库里的 `lite/`，
CSS 侧是构建期插件，其余全在 `devDependencies`。
**零 Vue 依赖**（原先那套 `vue` / `vue-jsx-vapor` 已于 2026-09-30 拆掉）。

`lite` 模式**无虚拟 DOM**：TSX 在构建期被折成"模板串 + 每个动态位置一条 effect"，
运行时只留本项目真用到的那几个原语。写法保留类型安全与组合能力。
详见 [`lite-framework.md`](lite-framework.md)。

### 10.2 目录

```
web/src/
  main.ts           入口，mount(App, '#app')
  app.tsx           壳：侧边栏 + 路由（**每页一个动态 import()**）
  api.ts            REST 封装 + SSE 订阅
  ui/               通用控件（log-panel / param-field / dialog / toast / theme …）
  pages/            页面
```

（**没有** `stores/` 目录：响应式状态用 lite 的 `ref` / `watch`，没有 Vue `reactive`。）

### 10.3 页面

**9 个路由页**（侧栏上限 9 条）：

| 页面 | 职责 |
|---|---|
| `dashboard` | 账号概览、AI豆统计、最近运行 |
| `accounts` | 账号列表、添加/编辑/移除、二维码与短信登录 |
| `tasks` | 任务列表 + 参数表单 + 运行 + 实时日志 + **中止** |
| `exchange` | 兑换奖品（抢兑窗口、分组清单） |
| `live-room` | 直播口令 → 小红花 |
| `schedules` | 定时任务 CRUD、启停、下次执行时间 |
| `pipelines` | 任务编排：步骤增删改序、两层失败语义 |
| `history` | 运行历史与日志 |
| `settings` | 全局配置 |

另有 `auth-page`（登录/初始化闸，**刻意不懒加载** —— 它就是未登录时的首屏）
与 `task-config-page`（任务参数子视图，不是顶层路由）。
路由表以 `web/src/app.tsx` 的 `ROUTES` 为准，别背数。

**移动端 dock 放 5 格**（规范上限）：总览、任务、定时、历史、设置。
**编排不进 dock** —— 它是"配置一次、之后自动跑"的东西，日常打开频率低，
从侧栏进完全够用。dock 每一格都要对得起它占的位置。

### 10.4 与后端契约

统一经 `api.ts` 的 `request<T>`（**已导出**，页面不再各自复刻一份 ——
复刻的代价是两处语义迟早分叉，尤其是"后端错误原文透出"这条）。

`subscribeRun(runId, handlers)` 封装 SSE，**收到终局事件自动关闭**，
返回取消函数供组件卸载时调用。

**错误文本一律原样透出**，不翻译、不改写：后端把"哪一步的任务不存在"
"哪些定时任务在引用它"写进错误里了，前端改写等于把这些信息丢掉。

### 10.5 代码分割 —— **已做**（按页 `import()`）

**当前事实（2026-10-03 复核）**：`web/src/app.tsx` 的每个路由页一个动态 `import()`
（另有 hover/focus 预取），Vite 据此切出独立 chunk，切页时才取那一块。
chunk 条数**别背数**，现算：`cd web && npm run build && ls web/dist/assets/*.js | wc -l`。

> ⚠ **本节下面那三大段是 Vue 时代的记录，已经整段不适用** ——
> 当时卡在 `defineVaporAsyncComponent`（Vapor 的异步组件）上，而 Vue 栈已于
> 2026-09-30 全部拆掉、换成自研 `lite`；`lite` 没有 Suspense/异步组件这一层，
> 页面本身就是"异步工厂 + `createComponent`"的形状，所以那三条坑**一条都不再成立**。
> 保留原文只为"别再按它规划工作量"；要问现状看上面这句与
> [`lite-framework.md`](lite-framework.md)。

<details>
<summary>历史记录（Vue 时代，已不适用）——为什么当时拆不动</summary>

**当时的结论：在 Vue 3.6.0-rc.9 上，Vapor 的异步组件跑不通，因此不做按路由拆包。**
以下是把三条坑逐个踩过之后的记录。

#### 现象

把 `app.tsx` 里的页面改成动态 `import()` 后：
chunk **确实**按预期拆开了、也**确实**按需下载了（Network 面板 200），
**但页面一片空白** —— 不报错、不抛异常、`typecheck` 过、`build` 成功。

#### 三个独立的坑，缺一不可

**① `suspensible` 默认是 `true`，而 Vapor 没有 Suspense**

`@vue/runtime-vapor` 源码：

```js
const { ..., suspensible = true } = createAsyncComponentContext(source)
...
if (suspensible && instance.suspense) {      // ← 这条分支
  return load().then(...)                    //    只调 load()，**不建立** renderEffect
}
```

没有 Suspense 宿主时走这条分支 ⇒ `load()` 照跑、chunk 照下，
但那个负责把 loading 换成真组件的 `renderEffect` **从未创建**。
必须显式 `suspensible: false` 才会走下面那条能工作的分支。

**② 命名导出会被 `comp.default` 解包成 `undefined`**

`@vue/runtime-core` 的 `createAsyncComponentContext`：

```js
if (comp && (comp.__esModule || comp[Symbol.toStringTag] === 'Module'))
  comp = comp.default
```

动态 `import()` 返回的**必然是** Module 命名空间对象（`toStringTag === 'Module'`），
所以它一定会去取 `.default`。而本仓 9 个页面**全是命名导出**
（`export const AccountsPage = ...`），模块里没有 `default` ⇒ 解出 `undefined`。

实测：`import('/assets/accounts-page-*.js')` 返回
`MODULE KEYS: AccountsPage`、`has default: false`。

**③ 页面是裸箭头函数，没有 `defineVaporComponent` 的 block**

这一条**无法在调用点绕开**，也是最终放弃的原因。

Vapor 组件必须返回一个 **block**。`defineVaporComponent(fn)` 做的事是
`extend({ name, setup: fn, __vapor: true })` —— 也就是给函数打上 `__vapor`。
本仓页面写的是裸箭头函数（`export const AccountsPage = () => { ... }`），
**没有 `__vapor` 标记**。

静态导入时能跑，是因为 `app.tsx` 编译后是 `createComponent(AccountsPage)`，
而 `createComponent` 接受裸函数并把其返回值当 block。
但经 `createInnerComp` 进 async wrapper 后，Vue 给出明确警告：

```
[Vue warn]: Vapor component setup() returned non-block value, and has no render function.
  at <Anonymous>
  at <VaporAsyncComponentWrapper>
  at <App>
```

→ 界面什么都不渲染。

#### 隔离复现（最小用例，与本项目无关）

用 `vue.runtime-with-vapor.esm-browser.js` 直接跑一个最小页面，
两个变体并排渲染（A 默认 `suspensible`、B 显式 `false`）：

```
t+10ms   #a=""  #b=""
t+200ms  #a=""  #b=""
t+1000ms #a=""  #b=""
[Vue warn]: Vapor component setup() returned non-block value, and has no render function.
  at <Anonymous> at <VaporAsyncComponentWrapper> at <App>
```

**两个变体都渲染不出来** —— 说明问题不在 `suspensible` 这个开关上，
而在 Vapor 的 async wrapper 与"裸函数组件"之间根本不兼容。

顺带确认：`defineVaporAsyncComponent` **确实存在**且从 `vue` 公开导出
（`@vue/runtime-vapor` 里有这个 API），所以这**不是**"API 不存在"，
而是"API 存在但在这套用法下不工作"—— 更容易误判，所以记下来。

#### 因此：要真正做代码分割，需要先满足的前提

1. **Vue 官方修好 Vapor 的 async 组件**（首要，否则后面都是白做）；
2. 若官方不修而仍要做，则必须把**所有页面**改成 `defineVaporComponent(...)`
   —— 那是 9 个业务文件一起改写法，且要逐页回归，代价远大于收益；
3. 收益本身也有限：曾实测拆包后首屏 JS `46,676 → 32,184 B`（gzip 少 31%），
   而首屏真正的瓶颈是 46 KB(gzip) 的 JS 与 `#app` 空壳，
   15 KB 的改善换不回"改 9 个文件 + 等上游修复"的成本。

#### 结论（当时）

**当时不做前端代码分割。** 首屏成本已由服务端解决（见 §9.3 的 gzip + 缓存）。
本条在 Vue 官方支持 Vapor async 组件之前**保持关闭** —— 后来 Vue 栈整个被拆掉，
这条也就随之作废。

</details>

### 10.6 JS 体积：研究结论（2026-09-28，**Vue 时代的历史读数**）

> ⚠ **整节是 Vue 时代（迁移到 `lite` 之前）的字节分析，数字与模块名都已过期** ——
> 现在没有 `@vue/*` 这些模块，产物也**按页拆 chunk**、不再是单 chunk。
> 现状与现算命令看 [`lite-framework.md`](lite-framework.md) §5（`node web/lite/size.mjs`）。
> 保留本节只为"当时为什么这么压"的来龙去脉。

当时的产物现状：**单 chunk 154,441 B raw / 48,718 B gzip**。要压它，先得知道字节在哪。

#### 构成（Rolldown 模块图，压缩前渲染长度）

| 模块 | 渲染长度 | 占比 |
|---|---|---|
| `@vue/runtime-vapor` | 95,823 | 28.8% |
| **我们的页面**（§10.3 那张表停在 6 个，实际已 10 个） | ~133,000 | 40.0% |
| `@vue/reactivity` | 35,493 | 10.7% |
| `@vue/runtime-core` | 22,448 | 6.7% |
| `src/api.ts` | 10,095 | 3.0% |
| `@vue/shared` | 6,769 | 2.0% |
| `@vue/runtime-dom` | 4,560 | 1.4% |
| `ui/*`（含图标） | ~14,000 | 4.2% |
| `vite/modulepreload-polyfill` | 1,205 | 0.4% |

渲染长度合计 332,970 B，最终 chunk 144,894 B ⇒ 压缩比约 43%。

#### 地板：空应用也要 43,656 B（16,344 B gzip）

用**同一套工具链**构建一个"只有一个 `ref` 和一个 div"的 Vapor 应用实测，
得到上面这个地板 ⇒ 反过来，**我们自己的代码是 110.8 kB raw / 32.4 kB gzip（占 71%）**。

顺带钉两条，别再查第二遍：

* 那 4,560 B 的 `@vue/runtime-dom` 在**空应用里也在** ⇒ 是 `vue` 入口自带的，
  **不是我们引错了入口**；`vue/vapor` 这个子路径在 3.6.0-rc.9 的 `exports` 里**不存在**。
* `runtime-vapor` 全程占三到五成，且**没有** VDOM 运行时混进来
  （`createVNode` / `createElementBlock` 等一个都没有）—— Vapor 这条是真的走通了。

#### 试过、无效或更差的（别重复投入）

| 实验 | 结果 |
|---|---|
| `build.target: 'esnext'` | 与 `es2022` **字节级完全相同** |
| oxc `compress.passes: 2` | 产物哈希不变（该选项在这条路径下不起作用） |
| `minify: 'terser'`（临时 `--no-save` 装完即卸） | **更差**：171.71 kB / 54.78 kB gzip（默认 oxc：154.44 / 49.40） |
| 别名到 `vue/vapor` 入口 | 该子路径不存在 ⇒ 构建失败 |
| 去掉 `console.*` / `debugger` | `src/` 里 **0 个**，无处可省 |
| 逐页拆包 | 见 §10.5，当时上游未修、保持关闭；**现已做**（换 `lite` 后按页 `import()`） |

#### 唯一落地的一项

`build.modulePreload: { polyfill: false }` ⇒ **−0.67 kB raw / −0.32 kB gzip**（当时 154,441 → 153,771）。
当时的判据：产物 HTML 里 `<link rel="modulepreload">` 数量是 **0**（单 chunk），
那条 polyfill 没有任何东西可垫。

⚠ **按需拆包已经做了，这条该回来复核** —— 现在产物里有 chunk，
`web/vite.config.ts` 那句"本产物是单 chunk、数量为 0"的注释已过期
（polyfill 本身仍关着；要不要开是另一件事）。

#### 复测更新：§10.5 的收益数字（当时）

§10.5 里记的"拆包后首屏 46,676 → 32,184 B"是**当时的体量**。
按当时现状复量：把 10 个页面全替成空壳后初始包 **154.44 → 68.36 kB**
（gzip 49.40 → 26.35，首屏 JS **−56%**）。

**当时的结论**：门槛是"9 个业务文件得先全改成 `defineVaporComponent`"，
再加上要等上游修 `suspensible` 与命名导出那两条 ⇒ 收益换不回成本。
（这条随 Vue 栈被拆掉而作废：`lite` 侧没有这个门槛。）

#### 怎么复现这些数字

* **模块级构成**：临时给 `vite build` 加一个插件，在 `generateBundle` 里打印
  `chunk.modules[].renderedLength` 并按大小排序；产物写 `dist-analyze`，**别碰 `dist`**。
* **地板**：同一个配置把 `build.rollupOptions.input` 指到一个最小 HTML + 一个最小 TSX。
* **拆包收益**：用 `resolve.alias` 把 `/pages/*-page` 指到一个"导出同名空组件"的文件再构建，
  与正常构建的差值就是初始包能减掉的部分。
* 用完删掉：临时配置、`tmp-min/`、`dist-analyze/`（`dist` 只由 `npm run build` 写）。

#### 附：验前端**不用**重编译（debug 构建运行时读盘）

2026-09-28 核实：`rust-embed` 8 在 `debug_assertions` 且未开 `debug-embed`
（本仓只开了 `interpolate-folder-path`）时，`get()` **每次都去 `web/dist` 读文件**。

实测证据：服务进程 18:36:37 启动后**再没重启**，而**次日 08:06** 新构建出的 JS
与服务端返回的字节 `cmp` **逐字节一致**（CSS、`index.html` 同）。

⇒ **开发期改完前端只需 `npm run build`**，正在跑的服务下一次请求就是新产物；
别为它跑 `cargo build` —— 那条链会覆写二进制 ⇒ SIGKILL ⇒ launchd 重拉 ⇒ 可能弹 TCC。
（⚠ **交付构建**仍必须「先 `npm run build` 再 `cargo build`」：release 下才是编译期内嵌。）

---

## 11. 测试策略

| 层 | 方式 |
|---|---|
| `mcloud-crypto` | 黄金向量对拍（`test/golden/*.json`），必须逐条相等 |
| `mcloud-upstream` | fixture 回放：录制真实响应，离线断言解析 |
| `mcloud-engine` | 假 Task + 假 Store，断言事件序列与并发闸 |
| `mcloud-server` | `tower::ServiceExt::oneshot` 路由级测试 |
| web | 类型检查 `npm run typecheck`（`tsgo --noEmit`）+ 关键组件测试 |

**随机性路径**（动态 IV、RSA 填充）不做字节级对拍，改为往返/自持密钥验证。

---

## 12. 安全与合规

### 12.1 凭证与脱敏

- 凭证仅存本地，配置目录权限 0600，写盘保留 `.bak`。
- 日志**脱敏**：`auth`/`token`/`jwttoken` 一律掩码，禁止整串落盘。
- 不内置任何他人账号或批量注册能力。
- 请求频率保守，遵循上游限流；失败指数退避而非重试轰炸。

### 12.1.1 两层登录：PC 端与 App 端（`auth` 前缀不同）

`Authorization` 的载荷是 `Basic base64("<前缀>:<账号>:<authToken>")`，
**前缀区分客户端身份**：

| 端 | 前缀 | 登录入口 | 请求特征 |
|---|---|---|---|
| PC | `pc` | `login qr` / `login sms` / `login password` | 明文 JSON 体；`pintype` 随方式变（21/5/9）|
| App | **`mobile`** | `login app` / WebUI「手机端」 | AES-128-ECB 加密体 + 末 10 位错位；`pintype` 恒为 `8` |

两端打的是**同一个接口** `POST /user/thirdlogin`，但**没有一处可以复用**：
密钥不同（`qPqDw263XgFgL3u8` vs `lYzvvBMUjucw+h^R`）、`clienttype` 不同
（656/710 vs 412）、`cpid` 不同（277/332 vs 55）、请求体形态不同
（明文 vs 加密+错位）、头集合不同。

所以代码里是**两个模块、两个类型**，刻意不做平台开关 —— 见
`mcloud_upstream::app_login` 的模块头。

> **`Account::pc_platform` 为 `None` 即表示 App 端登录。** 这是
> `AccountSummary` 里"非 PC 端"那一档的判据，WebUI 据此显示「手机端」徽标。
> 给它随便填一个 PC 平台会让后续每个请求都用错的 UA 发出去，
> 而那种错误在上游表现为一个含义模糊的业务码。

**实测判据（`9103` / `9441` 二值判别器）**：调这条链路的加密参数时，
不需要真的登录成功，只看回哪个码：

| 送出的体 | 响应 | 结论 |
|---|---|---|
| 加密 + 末 10 位错位 + 生产密钥 | `9441 验证码不正确` | ✅ 服务端**解开了**体 |
| 同一密文但**不旋转** | `9103 输入参数不合法` | 错位这一步不能省 |
| 明文 JSON（根本没加密） | `9103 输入参数不合法` | 必须加密 |

本仓三种都实测过（2026-09-26），结果与上表一致。

**滑块是同一个第一方接口**：`POST /user/verfycode {"account":…,"type":2}`，
取回的 `puzzleLeft` 里带着动态 IV，因此**本地就能算出坐标**，不需要图像识别、
也不需要用户去拖。App 与 PC 共用这条取题路径（编码方式也相同）。

### 12.2 登录闸（WebUI）

**两层认证，不要混**：

| | 谁的凭证 | 存哪 | 谁用 |
|---|---|---|---|
| 系统用户 | 你设的用户名/口令 | `users.json`（argon2id PHC） | 浏览器 |
| 云盘账号 | 移动云盘的 auth | `accounts.json` | 任务运行时 |

三态门：

| 状态 | `/api/` 下（除公开路由） |
|---|---|
| 未初始化（0 个用户） | **503** `setup_required` |
| 已初始化、未登录 | **401** |
| 已登录 | 放行 |

**为什么 503 与 401 必须分开**：401 让前端弹登录框，而这时根本没有账号可登；
503 让前端跳初始化向导。混成一个码，用户看到的是"登录失败"
而不是"还没建管理员" —— 而后者才是他该做的事。

几条硬规则：

- **口令**：argon2id（`m=19456, t=2, p=1, out=32`），与 mcloud 同参数 ⇒
  两边金标向量可对拍。长度按 **Unicode 字符**数（8 位），
  按字节算会让中文口令的下限变成 3 个字。
- **会话令牌**：库里存 `sha256(token)`，**明文不落盘**。
  令牌是 32 字节随机数（不是"用户名+时间戳"的派生 —— 派生值可预测）。
- **cookie**：`HttpOnly; SameSite=Strict; Path=/; Max-Age=<与库内到期一致>`。
  **不加 `Secure`** —— 默认跑在 `http://127.0.0.1`，加了浏览器会直接不存，
  表现是"登录成功但立刻又未登录"；上 HTTPS 时应由反代加。
- **用户名枚举**：用户不存在与口令错**回同一句话**。区分开来就是一个
  "这个用户名存不存在"的探测器。
- **改口令**：必须清掉该用户的**所有**会话 —— 用户改口令的动机通常正是
  "怀疑别人知道我的口令"，旧会话还在等于改了也没用。
- **静态资源放行**：否则登录页自己都加载不出来，用户看到的是永远转圈的
  空白页而不是登录框。这不泄漏任何东西 —— 前端包里没有数据。
- **读不出用户数时失败关闭**：放行等于"存储坏了就没人需要登录"，
  那是最糟的失效方向。

---

## 13. 里程碑

| 阶段 | 交付 | 状态 |
|---|---|---|
| P0 | 工作区骨架、领域类型、IO trait | ✅ |
| P1 | 密码学（签名/AES）+ 黄金向量对拍 | ✅ |
| P2 | 上游客户端、认证域隔离 | ✅ |
| P3 | 引擎（注册表/运行器/并发闸/事件） | ✅ |
| P4 | HTTP 服务 + 前端（同源单二进制） | ✅ |
| **P5** | **通用存储、定时调度、任务编排、日志收集、任务中止** | **✅** |
| **P6** | **H5 会话层（JWT）+ 签到端点校正** | **✅** |
| **P7** | **WebUI 登录闸（argon2id + 三态门 + 会话）** | **✅** |
| **P8** | **认证闸 + 登出 + CLI 收敛 + 账号登录（扫码/短信）** | **✅** |
| P9 | 推送渠道、其余任务 | ⏳ |

### CLI（`mcloud-cli`）—— 只管"无头也能跑"的那几条

**命令数以 `bin/cli.rs` 的顶层分派为准，别背数** —— 现算（8 空格缩进那层就是顶层）：
`grep -n '^        Some("' crates/mcloud-server/src/bin/cli.rs`。今天的长相：

```
tasks                          列出可用任务
run <任务名> <账号>             跑一次任务
pending                        查看本进程里还没清理的网盘产物
cron <定时id>                   跑一条已配置的定时（给系统 crontab 调用）
pipeline run <编排id>           跑一条已配置的编排
auth refresh <--account|--all>  刷新凭据
login <方式>                    登录账号（qr / sms / app / password / device）
```

**`login app` 与 `login sms` 是两条链路**，不是"同一条加个参数"：
前者落盘 `mobile:` 前缀、走 App 的设备头与加密体（见 §12.1.1）。

**定时与编排的「创建」不在命令行** —— 改配置文件或用 WebUI。
少一个入口就少一处会与 WebUI 漂移的语义。

`cron` **不常驻**：时间由系统 crontab 管，它只跑一条然后退出。
判据、水位推进、并发闸全走 `Scheduler::tick`，不在这里重写 ——
重写就是"两处语义"，而它们的差异（一个补跑、一个不补）极难发现。

退出码约定：`0` 成功（**含"这次不该跑"**）、`1` 任务失败、`2` 配置错误。
"不该跑"退 0 是刻意的：crontab 会把非零当"执行出错"，
而"今天这次不用跑"是正常的 —— 退 1 会让监控天天报假警。

设计取向见 [`cli-webui-design.md`](cli-webui-design.md)：**WebUI 为主，
CLI 只管"无头也能跑"的那几条**（`login` / `run` / `pending` / `cron` /
`auth refresh` / `pipeline`）。编排与定时的**创建**不在 CLI —— 改配置文件或走 WebUI。

### 当前的关键风险

密码学已被黄金向量锁死，**接口存活与签名服务端一致性才是真正的风险点**。
已经吃过一次这个亏：签到端点最初是凭印象写的 `/ycloud/api/sign/query`，
**实测回 405**；换成实拍的 `/ycloud/signin/page/infoV3` 后回 **200**。
这类错误的产物是"能编译、能过测试、但打到一个不存在的地址"。

**规则：端点必须来自抓包，不能凭印象写。**

