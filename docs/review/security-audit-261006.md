# mcloud 安全审计（2026-10-06）

- **基线**：`86dc1bc`（2026-10-06 12:25，工作区有未提交改动，见文末"诚实边界"）
- **范围**：`crates/mcloud-server/`（HTTP 面：鉴权、会话、限速、路由）、`crates/mcloud-store/`（落盘）、
  `crates/mcloud-crypto/`（口令/令牌/加密原语）、`crates/mcloud-upstream/`（出网客户端）、
  `crates/mcloud-app/`（推送/日志中枢）、部署形态（`main.rs` / launchd / `docs/usage.md`）
- **方法**：逐文件通读 + 只读 `grep`/`sed` + **依赖源码交叉核对**
  （`axum 0.8.9` / `matchit 0.8.4` / `hyper 1.11.1` / `hyper-util 0.1.21` / `axum-core 0.5.6` /
  `tower-http 0.6.11` / `url 2.5.8` / `argon2 0.5.3`，版本与 `Cargo.lock` 一致）
  ＋ **一次独立小程序实测**（SSRF 闸绕过，见 SEC-04，不碰工作区 `target/`）
- **纪律**：**没有改任何源码/配置**（本次只新增本报告与 `docs/task-log.md` 的一行）、**未跑 `cargo`/`npm`**、
  **未对 `:3000` 发业务请求**（只做了 `lsof` 看监听地址与一次 `GET /api/health` 确认服务在跑）。
  所有"影响"都是按代码路径推的，**没有构造双用户实例实测**。
- **计数**：高 2 · 中 7 · 低 13 · 已核实"不是问题" 16

---

## 0. 一句话结论

**这个项目的"认证闸本身"做得比同规模项目扎实**（公开清单精确、三态门正确、令牌 256 bit、
只存哈希、口令 argon2id、日志全链路脱敏、CSRF 面基本关死、`sv-4/6/7/8` 都已修）。
**真正的缺口集中在三处**：

1. **抗 DoS 面基本是空的** —— 没有连接/并发上限、没有请求超时，而 hyper 自带的
   30 秒"请求头读超时"因为 `axum::serve` 没装 timer 而**被静默关闭**（SEC-01）；
   唯一的限速器是**全局单键**，既挡不住分布式，又能被未认证者反过来用来**永久锁死管理员登录**（SEC-02）。
2. **登录频率限制的"键"是错的** —— 想按 IP/按用户名限速，但 `serve` 没接 `ConnectInfo`，
   拿不到 peer IP；`X-Forwarded-For` 也完全没被读（SEC-02）。
3. **云盘凭据是明文落盘的**，且没有任何加密/密钥管理层（SEC-03）；
   配套的"推送地址闸"可以被 IPv4 写法绕过，服务端仍可被指向 `127.0.0.1`/内网（SEC-04，**已实测**）。

另有一组**多租户才成立、但代码里已经真实存在**的越权：账号归属可被覆盖（SEC-05）、
扫码会话不绑定发起者（SEC-06）、实例级动作没有角色概念（SEC-07）。
⚠ **今天通过 API 建不出第二个用户**（`POST /api/setup` 只在 0 用户时可建，无用户管理路由，
`remove_user` 全仓零命中）⇒ 这几条现在是**潜在**的；一旦真跑多用户（手改 `users.json` 或将来加功能）就立刻成立。

---

## 1. 威胁模型与部署前提（严重度都按这两档读）

| 部署形态 | 事实依据 | 对本报告的影响 |
|---|---|---|
| **默认：`127.0.0.1:3000`**（当前实况，`lsof` 实测 `TCP 127.0.0.1:3000 (LISTEN)`） | `main.rs:59` 默认值；`scripts/run-local.sh:39` 同 | 未认证面只能被**本机**进程触达 ⇒ 抗 DoS 类发现降一档 |
| **绑 `0.0.0.0`/LAN**（**文档明确教的**） | `docs/usage.md:84` `MCLOUD_ADDR=0.0.0.0:3000 MCLOUD_DATA_DIR=/srv/mcloud ./mcloud`；`README.md:99` 也说明这是可选形态 | SEC-01/02/04/05/06 在这一档下全部升级：**免认证远程**可打 |

其它既定事实（影响判读，先摆出来）：

- **实例实际是单管理员**：`data/users.json` 实测 1 个用户；`POST /api/setup` 只在 0 用户时可建；
  没有用户管理路由、没有删用户 CLI。⇒ 归属模型（`owner` / `visible_to`）今天**几乎不产生跨用户面**。
- **WebUI 是唯一的配置入口**（README:111：定时/编排只能在 WebUI 里建）⇒ 把管理员挡在登录页外
  ＝ 整个产品不可用（这是 SEC-02 的严重性来源）。
- **HTTPS 由反代终止**，仓内没有 TLS 配置；cookie 因此**永不写 `Secure`**（SEC-13）。
- **无 CORS 层**（同源设计，`lib.rs:5-7`）⇒ 浏览器侧跨站面很小。

---

## 2. 发现总表

| 编号 | 严重度 | 面 | 一句话 | 位置 |
|---|---|---|---|---|
| **SEC-01** | 高（LAN）/ 中（本机） | DoS | 请求头读超时被静默关闭 + 无连接/并发上限 + 无请求超时 ⇒ slowloris 可按住连接不放 · **部分修复**（见 §9） | `lib.rs:27-49`、`main.rs:113-124` |
| **SEC-02** | 高（LAN）/ 中（本机） | 登录频率 | 限速器是**全局单键**且被拒不计数 ⇒ 挡不住分布式，反被用来**永久锁死管理员** · **已修复**（见 §9） | `ratelimit.rs:66-87`、`state.rs:61`、`auth.rs:321-336` |
| **SEC-03** | 中 | 凭据落盘 | 云盘 token/手机号/base64 原串**明文**写 `accounts.json`，无加密与密钥管理 | `mcloud-core/src/account.rs:104-125`、`json_store.rs:623-629` |
| **SEC-04** | 中 | 出网/SSRF | 推送地址闸可被 **IPv4-mapped IPv6** 与 **WHATWG 十进制/十六进制/八进制 IPv4** 绕过（**已实测**） · **已修复**（见 §9） | `mcloud-app/src/push.rs:994-1060` |
| **SEC-05** | 中（多用户） | 越权 | 账号归属可被**覆盖式**写入；构造一条假凭据即可接管/清空他人账号行与历史 · **已修复**（见 §10） | `accounts.rs:195-223`、`login.rs:796-804` |
| **SEC-06** | 中（多用户） | 越权 | 扫码会话不与发起者绑定，归属归"轮询者" ⇒ QRLJacking · **已修复**（见 §10） | `login.rs:39-43`、`210-236` |
| **SEC-07** | 中（多用户） | 越权 | 无角色模型：`restart` / `admin/security` / `tasks/*/config` 都是实例级、任何登录用户可改 · **已修复**（实例级动作收归"最早的用户"= 管理员，见 §10） | `system.rs:198-208`、`security.rs:69-90`、`task_config.rs:142-188` |
| **SEC-08** | 中 | 限速 | 昂贵/有副作用的接口无限速：改口令（每次 argon2）、跑任务、**对任意手机号触发上游短信** · **部分修复**（改口令 + 发短信已限速；`POST /api/runs` 仍未限速，见 §10） | `security.rs:116-158`、`runs.rs:201-282`、`login.rs:347-375` |
| **SEC-09** | 中 | 认证 | `POST /api/setup` 的 TOCTOU：并发可建出两个管理员；未初始化实例"先到先得" · **已修复**（见 §10） | `auth.rs:126-184`、`json_store.rs:1204-1211` |
| **SEC-10** | 低 | 越权 | 存在性预言机：404 与 400/200 的差别漏出"这个 id 存不存在"（可拿手机号试成员资格） | `runs.rs:298-303`、`cancel.rs:48-58` 等 5 处 |
| **SEC-11** | 低 | 越权 | `GET /api/logs` 是全实例日志（含别人的账号 id 与任务正文；**不含**凭据/手机号） · **已缓解**（收归管理员；"每人只看自己的"需要日志行带 `user_id`，见 §10） | `server_logs.rs:35-45` |
| **SEC-12** | 低 | 可观测 | 认证失败**零日志**、成功不记来源 IP、无失败计数/锁定 · **已修复**（见 §11） | `auth.rs:216-237`、`258` |
| **SEC-13** | 低 | 会话 | cookie 永不写 `Secure`，且没有任何开关 / `__Host-` 前缀 · **已修复**（`MCLOUD_COOKIE_SECURE` 开关，见 §11） | `auth.rs:447-456` |
| **SEC-14** | 低 | 会话 | 过期会话**永不清理**（`purge_expired_sessions` 无生产调用者） · **已修复**（挂进既有 2 秒循环，见 §11） | `traits.rs:382`、`json_store.rs:1329` |
| **SEC-15** | 低 | 口令策略 | 任何 64 位小写 hex 都被当"前端摘要"跳过长度检查 ⇒ 下限可绕过（非提权） · **已文档化**（刻意接受，理由写在代码里，见 §11） | `auth.rs:146-153`、`password.rs:196-200` |
| **SEC-16** | 低 | 资源 | 账号/定时/编排**条数无上限**，且每次写都整文件重写 + fsync（O(n²)） · **已修复**（200/500/200，见 §11） | `json_store.rs:56,623-654` |
| **SEC-17** | 低 | 资源 | SSE：订阅者数无上限；订阅"内存历史已回收的终态 run"⇒ 流永不结束 · **已修复**（见 §12） | `hub.rs:155-177,332-361`、`runs.rs:384-385` |
| **SEC-18** | 低 | 信息泄露 | `/api/system/{about,storage}` 摊出绝对路径、数据文件清单、git 提交标题 · **部分修复**（`storage` 已收归管理员；`about` 刻意保持开放，见 §10） | `system.rs:75-103,154-192` |
| **SEC-19** | 低 | 会话 | `require_session` 不校验用户是否还在、不校验 `SessionKind`（CLI 会话永不过期） · **已修复**（见 §11） | `auth.rs:409-432`、`user.rs:100-111` |
| **SEC-20** | 低 | 一致性 | `/api/runs/{id}/logs` 与 `/events` 可见性口径不一致（sv-9 仍在，方向是失败关闭） · **已修复**（`/logs` 改用 `run_access`，见 §12） | `logs.rs:38` vs `runs.rs:140-196` |
| **SEC-21** | 低 | 防御纵深 | 闸门"非 `/api/` 直接放行"的安全性完全依赖 matchit 不做归一化 · **已修复**（形状可疑的路径直接 404，见 §11） | `auth_gate.rs:76-86` |
| **SEC-22** | 低 | DoS | 静态资源免认证且无速率/连接限制（单请求 ~155 KB / gz 48 KB） · **已缓解**（第一批的 `ConcurrencyLimitLayer(128)` 罩住包括静态资源在内的一切，见 §9.1） | `auth_gate.rs:79-82`、`static_files.rs:111-124` |

---

## 3. 详细发现（高 / 中）

### SEC-01 [高·LAN｜中·本机] 请求头读超时被静默关闭，且没有连接/并发上限、没有请求超时

**位置**：`crates/mcloud-server/src/lib.rs:27-49`（只有压缩 / 登录闸 / Trace 三层）、`crates/mcloud-server/src/main.rs:113-124`

**证据**（依赖源码交叉核对，逐环）：

```rust
// lib.rs:44-48 —— 没有任何 TimeoutLayer / ConcurrencyLimitLayer / RequestBodyLimitLayer
.layer(CompressionLayer::new())
.layer(middleware::from_fn_with_state(state.clone(), auth_gate::gate))
.layer(TraceLayer::new_for_http())
// main.rs:122-124 —— 也没有 into_make_service_with_connect_info
axum::serve(listener, router).with_graceful_shutdown(...)
```

```rust
// axum-0.8.9/src/serve/mod.rs:391 —— 只 new，从不 .timer(...)
let mut builder = Builder::new(TokioExecutor::new());
// hyper-1.11.1/src/server/conn/http1.rs:246,250 —— timer 为空，而默认值写着 30s
timer: Time::Empty,
h1_header_read_timeout: Dur::Default(Some(Duration::from_secs(30))),
// hyper-1.11.1/src/common/time.rs:70-77 —— 没 timer 时是 warn! + None ⇒ 超时被关掉
Dur::Default(Some(dur)) => match self {
    Time::Empty => { warn!("timeout `{}` has default, but no timer set", name); None }
    Time::Timer(..) => Some(dur),
},
```

**触发**（未执行）：开 N 条 TCP，各发 `POST /api/session HTTP/1.1\r\n` 加约 400 KB 头字节、
**不发最后的空行**，然后挂着。没有 timer ⇒ 不超时；没有连接上限 ⇒ 想开多少开多少；
每条连接常驻一个 tokio 任务 + 最多 ~408 KiB 读缓冲（hyper `DEFAULT_MAX_BUFFER_SIZE = 8192 + 4096*100`）。

**影响**：本机 = 本机任意进程可占满 FD/内存；绑 LAN = 同网段任意主机可做到，几百条连接就是几百 MiB。
**修复**：给 `axum::serve` 换 `hyper_util` 自定义 builder 并装 `TokioTimer`（让默认 30s 生效），
或加 `tower::timeout::TimeoutLayer` + `ConcurrencyLimitLayer`；最省事是在反代层限制连接数与读超时。
**状态**：已交叉核对依赖源码 · 置信度 高

---

### SEC-02 [高·LAN｜中·本机] 认证限速器是全局单键：挡不住分布式，反而能被用来永久锁死管理员

**位置**：`crates/mcloud-server/src/ratelimit.rs:66-87`、`state.rs:61`、`routes/auth.rs:193,321-336`、`main.rs:122`

**证据**：

```rust
// state.rs:59-61 —— 整个进程只有一个计数器，没有任何键
// 60 秒 30 次：正常使用（一个人偶尔登录）碰不到…
auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(60_000, 30)),
// ratelimit.rs:78-86 —— 超限即拒，但**被拒的那几次不占配额**
if hits.len() >= self.max {
    let oldest = *hits.front()...;
    return Err((wait_ms.max(1) + 999) / 1000);
}
hits.push_back(now_ms);   // ← 只有放行的才计入
```

`main.rs:122` 用裸 `axum::serve(listener, router)`，全仓 `into_make_service_with_connect_info` **零命中**
⇒ handler 里拿不到 peer IP，唯一的键就是"整个进程"（`ratelimit.rs:27-32` 自己承认这条边界）。

**触发**（未执行）：攻击者每 60 秒打满 30 次 `POST /api/session`（合法 JSON、随便什么用户名口令）。
因为被拒的不占配额，**窗口永远满** ⇒ 管理员每次登录都得 429 + `Retry-After`，且可无限维持。
更便宜的是打 `POST /api/setup`：已初始化实例上它只做一次 `user_count()` 读就返回 400（`auth.rs:134`），
连 argon2 都不触发 —— 用最便宜的请求占满最贵的配额。

**影响**：本机 = 本机进程可把管理员挡在 WebUI 外（要等攻击者停手）；绑 LAN = **免认证远程无限期锁死**，
而 WebUI 是唯一配置入口。同时它对"多源低速"（每个源只发几次）**完全无效**。
**修复**：`into_make_service_with_connect_info::<SocketAddr>()` + 键换成 peer IP（可再加"用户名"维度，
并对同一用户名的连续失败做指数退避）；"已初始化实例上的 setup 拒绝"与"成功登录"不该占用登录配额；
反代场景要显式定义 `X-Forwarded-For` 的信任边界（现在该头**完全没被读**，所以无法伪造，但也无法按真实客户端限速）。
**状态**：已交叉核对依赖源码（axum `serve` 签名） · 置信度 高

---

### SEC-03 [中] 云盘凭据明文落盘，没有加密与密钥管理

**位置**：`crates/mcloud-core/src/account.rs:104-125`（`AuthInfo`）、`crates/mcloud-store/src/json_store.rs:623-629`（`persist_accounts`）

**证据**（不贴真实值）：

```rust
// account.rs:104-125 —— 原始 base64 原串 + 手机号 + token 都是普通字段
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthInfo { pub raw: String, pub platform: String, pub phone: String, pub token: String, pub expire: i64 }
```

实测 `data/accounts.json` 的一条账号里 `auth` 有 `{expire, phone, platform, raw, token}`，
**值全是明文**（`raw` = `base64(platform:phone:token)`，等于完整可冒充凭据）。
`grep -rn "encrypt|decrypt|aes|cipher" crates/mcloud-store/src/` **零命中** ⇒ 存储层没有任何加密。
权限位实测是好的：`data/` `drwx------`、各文件 `-rw-------`（0600/0700），且 `data/` 从未入库
（`git ls-files data` 为 0；`.gitignore:9` 有 `/data`）。

**威胁模型**：同机其它用户被权限位挡住；**但**备份/同步盘/`cp -r`/云备份/取证镜像都会带走明文凭据；
`GET /api/system/storage`（SEC-18）还会把数据目录绝对路径与文件名清单摊给任何登录用户，
等于指路。凭据本身是"能冒充用户访问其云盘"的完整能力（`mcloud-app/src/builder.rs:225` 的注释也写着
"云盘凭据一个都不能给"）。
**修复**：要么在文档里明说"`data/` 就是凭据本体，别同步/别备份到不可信位置"（最省事、诚实）；
要么引入一层"用 OS keychain / 机器绑定密钥加密 `auth`"的封装（注意：单机工具里密钥与密文同机，
收益主要是防"文件被单独拷走"）。
**状态**：已实测（`ls -l data/`、`data/accounts.json` 结构解析）+ 静态读码 · 置信度 高

---

### SEC-04 [中] 推送地址闸可被绕过 ⇒ 服务端盲 SSRF（**已实测**）

**位置**：`crates/mcloud-app/src/push.rs:994-1030`（`validate_push_url`）、`:1038-1060`（`is_public_ip`）；调用点 `notify.rs:91`

**背景**：`sv-8` 已经加了一道闸（scheme 白名单 + 拒字面量内网/回环 + 推送客户端 `no_redirects: true`，
见 `builder.rs:225-231`）。闸的判据是**自己按字符串切出 host，再 `host.parse::<std::net::IpAddr>()`**。
而真正发请求的 `reqwest` 用的是 `url` crate 的 **WHATWG 解析**——两把尺子不一样。

**实测**（`$HOME/.cargo/bin/rustc -O /tmp/ssrf_check.rs`，链接工作区已编译的 `liburl-*.rlib`，不写工作区）：

```
http://2130706433/                  project_host=2130706433        parse_as_ip=false => host=Some("127.0.0.1")
http://0x7f000001/                  project_host=0x7f000001        parse_as_ip=false => host=Some("127.0.0.1")
http://017700000001/                project_host=017700000001      parse_as_ip=false => host=Some("127.0.0.1")
http://127.0.0.1./                  project_host=127.0.0.1.        parse_as_ip=false => host=Some("127.0.0.1")
http://[::ffff:127.0.0.1]/          project_host=::ffff:127.0.0.1  parse_as_ip=true  => host=Some("[::ffff:7f00:1]")
http://[::ffff:169.254.169.254]/    project_host=::ffff:169.254.169.254 parse_as_ip=true => host=Some("[::ffff:a9fe:a9fe]")
```

- 前四种：`parse_as_ip=false` ⇒ 被当成"普通域名"**放行**，而 `url` 2.5.8 会把它归一化成 `127.0.0.1`
  （`url-2.5.8/src/host.rs:297-331` 的 `parse_ipv4number` 认 `0x` 十六进制、前导零八进制、单段十进制；
  `:333-361` 的 `parse_ipv4addr` 会去掉末尾空段 ⇒ `127.0.0.1.` 也成立）。
- 第五、六种：`std::net::Ipv6Addr` 的 `is_loopback/is_unique_local/is_unicast_link_local` 对
  `::ffff:127.0.0.1` 全部为 `false` ⇒ `is_public_ip` 返回 `true` **放行**，而 OS 会把它当 IPv4 回环/链路本地连出去。

**触发**（未执行）：任何能改账号设置/推送配置的人，把 `customPost.url`（或企业微信机器人 url / TG `apiHost`）
填成上面任一形状 ⇒ 服务进程向 `127.0.0.1:3000/api/system/restart`、`169.254.169.254`、内网段发请求。
响应体不回给调用方，但**状态码会进推送日志**（`GET /api/logs` 可读，SEC-11）⇒ 盲 SSRF 探测。
**闸自己写明的边界**：不做 DNS 解析 ⇒ 挡不住"域名解析到 127.0.0.1"（DNS rebinding / 内网自有域名），
这条是已知且诚实记录的。
**修复**：不要在字符串上判 —— 先 `url::Url::parse(url)`，取 `url.host()`（它已经是归一化后的
`Host::Ipv4/Ipv6`），再对 `IpAddr` 判公网；IPv6 分支补 `to_ipv4_mapped()`；
最好在**连接时**核对实际解析出的 IP（`reqwest` 的 `resolve`/自定义 connector）。
**状态**：**已实测**（独立小程序 + 依赖源码）+ 静态读码 · 置信度 高

---

### SEC-05 [中·多用户] 账号归属可被覆盖式写入：构造一条假凭据即可接管/清空他人账号行

**位置**：`crates/mcloud-server/src/routes/accounts.rs:195-223`（`import`）、`login.rs:796-804`（`claim_account`）、
`json_store.rs:786-793`（`save_account` 整行替换）、`mcloud-upstream/src/auth.rs:45-90`（`parse_auth` **只解析形状**）

**证据**：

```rust
// accounts.rs:195-200 —— account_for 对"别人的账号"返回 None（visible_to 挡住了）
let existing = state.handle.store.account_for(&cur.0, &account_id).await?;
let created = existing.is_none();          // ← 别人的账号在这里被当成"新建"
let mut account = match existing {
    Some(a) => a,
    None => { /* account_from_auth 造全新的一条：nickname=None, settings=default */ }
};
...
account.owner = Some(cur.0.clone());       // ← 无条件覆盖 owner
state.handle.store.save_account(&account).await?;   // ← 按 id 整行替换
```

```rust
// login.rs:796-802 —— 四条登录路径（QR / SMS / App-SMS / 账密）都走这里
async fn claim_account(state: &ServerState, owner: &mcloud_core::UserId, mut account: mcloud_core::Account) -> ... {
    account.owner = Some(owner.clone());
    state.handle.store.save_account(&account).await?;
```

**关键**：`parse_auth` 只做 `base64 → platform:phone:token` 的形状解析，**不校验 token 是否有效**
（`auth.rs:45-90` 全文只判空、解 base64、取前三段）。而账号主键是 `sha256(手机号)` 前 16 位
（`auth.rs:244-247`）⇒ **A 不需要持有 B 的真实凭据**，只要知道 B 的手机号，构造
`base64("pc:13800000000:whatever")` + `platform=windows` 就能 `POST /api/accounts`：

- `account_for(A, id)` → `None`（`visible_to` 要求 owner 相等，`owner.rs:32-37`）
- 走"新建"分支 → `owner = A` → `save_account` 把 **B 那一行整行覆盖**
- B 的昵称/每账号设置/设备号/停用状态被重置；凭据被换成那条垃圾串
- 运行记录的归属是**从账号 owner 推出来的**（`json_store.rs:471-477` `run_visible`）
  ⇒ B 在这个号上的**全部历史运行记录转手给 A**，B 自己反而看不见了

手机号可以用 SEC-10 那个预言机先确认"这个号在本实例被托管"。
**修复**：写之前用**不带归属过滤**的 `store.account(&id)` 做存在性检查，owner 存在且 ≠ `cur` 就拒（404/409）；
`import` 的"已存在"分支要按 `store.account()` 判而不是 `account_for()`；
登录成功只该换 `auth`/`expire`，保留 nickname/settings/device_id/enabled；
`claim_account` 还要绑定"发起者"（见 SEC-06）。
**状态**：已交叉核对（`visible_to` / `sole_user` / `save_account` / `parse_auth` / `run_visible`） · 置信度 高

---

### SEC-06 [中·多用户] 扫码登录会话不与发起者绑定，归属归"轮询者" ⇒ QRLJacking

**位置**：`crates/mcloud-server/src/routes/login.rs:39-43`（`QrSession`）、`:210-236`（`poll_qr`）、`:294-300`（`cancel_qr`）

**证据**：

```rust
// login.rs:39-43 —— 会话里没有"谁发起的"
struct QrSession { qr: QrLogin, tid: String }
// login.rs:216 —— 只按 session_id 取
let Some((mut qr, tid)) = state.login_sessions.take(&req.session_id) else { ... 404 ... };
...
Ok(QrProgress::LoggedIn { user_info }) => {
    let account = state.handle.login.finish_qr(&qr, &user_info).await?;   // 落库
    let account = claim_account(&state, &cur.0, account).await?;          // ← 归"轮询者"
```

**触发**（未执行）：A 登录后 `POST /api/login/qr` 拿到二维码载荷 → 以话术让 B 用云盘 App 扫 →
B 的云盘凭据被 `finish_qr` 落进实例，`claim_account` 把它归到 **A** 名下。B **不需要**是本实例用户。
**影响**：A 的 WebUI 里出现 B 的云盘账号（完整可冒充凭据），随后可跑任务、读 B 的云盘记录、
用 B 的 AI豆兑换（真扣豆）。
**修复**：`QrSession` 加 `owner: UserId`，`poll_qr`/`cancel_qr` 只认发起者（不是自己的与不存在同 404）；
归属保持"发起者"；扫码成功再加一次显式确认。
**状态**：已交叉核对（`QrSession` 结构 + `claim_account` 调用点） · 置信度 高

---

### SEC-07 [中·多用户] 没有角色模型：实例级动作任何登录用户都能做

**位置**：`system.rs:198-208`（restart）、`security.rs:69-90`（口令策略/会话时长）、`task_config.rs:142-188`（任务参数）、`server_logs.rs:35-45`（全量日志，见 SEC-11）

**证据**（三处都是"过了登录闸就算数"）：

```rust
// system.rs:198 —— 签名里没有 CurrentUser
pub async fn restart(State(state): State<ServerState>) -> Result<StatusCode, ApiError> { ... shutdown.notify_one() ... }
// security.rs:73 —— 同上，改的是 settings.json 里的**全实例**值
pub async fn put_security(State(state): State<ServerState>, Json(body): Json<SecurityPatch>) -> ApiResult<SecurityView>
// task_config.rs:142/171 —— 同上；存储键只有任务名（json_store.rs:91 task_config: HashMap<String, Value>）
pub async fn get_one(State(state): State<ServerState>, Path(name): Path<String>) -> ApiResult<TaskConfigView>
```

`User` 结构**没有角色字段**，所有登录用户等价于管理员。
**影响**（多用户）：B 可重启服务（杀掉 A 正在跑的任务与 SSE，并把限速器/日志环清零）；
可把全实例口令下限改成 1、会话时长改成一年；可读改 A 的任务参数（`live-room` 的 `codes` 就是口令）。
**修复**：加最小角色（`User.role`），或至少把这几条收进"实例唯一管理员"判定
（`users.len()==1` 或最早创建的那个，与 `legacy_owner` 同一判据）；`task_config` 改按 `UserId` 分键。
**状态**：仅静态（三处签名 + store 字段已核对） · 置信度 高

---

### SEC-08 [中] 昂贵/有副作用的接口没有限速

**位置**：`security.rs:116-158`（改口令）、`runs.rs:201-282`（跑任务）、`login.rs:347-375`（PC 发码）、`:548-573`（App 发码）

**证据**：

```rust
// security.rs:136 —— 口令填错也照样跑一次 argon2（19 MiB）
mcloud_crypto::verify_password(candidate, ...).map_err(|_| ApiError(...Auth("当前口令不正确")))?;
// login.rs:358 —— 手机号由请求体给，一次请求就可能让上游真的发一条短信
let sms = state.handle.login.start_sms(platform, req.phone.trim()).await?;
```

`state.auth_limiter`（`state.rs:61`，60s/30 次）全仓**只有 `auth.rs:323` 一个调用点**（实测
`grep -rn auth_limiter crates/` 只有 state.rs×2 + auth.rs×1 + 一处测试）⇒ 上面这些一条都没挂。
`/api/login/*` 的 8 条路由都要登录，但登录之后**没有任何节流**。
**影响**：已登录者（或被盗会话）可对 `PUT /api/admin/password` 无限次跑 argon2 打满 CPU
（并发 30 就是 ~570 MiB 瞬时内存）；可循环对**任意第三方手机号**触发上游短信
（缓解：上游自己有配额与滑块，`remain_get_times` / `NeedSlider`，所以不是无限轰炸）；
`POST /api/runs` 默认同步执行，长任务（`listenSeconds` 是分钟级）会一直占着连接。
**修复**：把 `RateLimiter` 复用到所有跑 argon2 的路由与 `/api/login/*`（键：用户 + 目标手机号）；
给发码单独一条更紧的配额；给同步 `POST /api/runs` 一个上限或强制后台化。
**状态**：已实测（限速器调用点全仓 grep）+ 静态读码 · 置信度 高

---

### SEC-09 [中] `POST /api/setup` 的 TOCTOU 竞态与"先到先得"

**位置**：`auth.rs:126-184`（检查在 `:134`、argon2 在 `:156`、落库在 `:167`）、`json_store.rs:1188-1190,1204-1211`

**证据**：

```rust
if is_initialized(&state).await? { return Err(...) }   // :134 读锁一瞬即释放
...
let hash = mcloud_crypto::hash_password(&digest)...?;  // :156 中间隔着几十毫秒的 argon2，全程不持锁
state.handle.store.save_user(&user).await?;            // :167 写
// json_store.rs:1204-1211 —— 按 id upsert，对 name 无唯一约束
match db.users.iter_mut().find(|u| u.id == user.id) { Some(slot) => *slot = user.clone(), None => db.users.push(...) }
```

限速是 30 次/分钟，远大于 2 ⇒ 两个并发请求都能过检查、各生成一个 UUID 后 push ⇒ **两个管理员**。
同名时 `user_by_name` 取 `iter().find()` 的第一条（`json_store.rs:1196-1198`），先拿到写锁者可劫持登录。
**触发**：实例未初始化时并发（≤30 个）`POST /api/setup`；绑 LAN 且未初始化时＝初始化窗口内免认证夺取管理员。
**修复**：在 store 里加 `create_first_user`，**持写锁内**判 `users.is_empty()` 后 push，并给 `name` 加唯一性；
更稳的是把"首次初始化"限制成只能从 loopback 发起的请求（`ConnectInfo` 已经有了 SEC-02 的改造前提）。
**状态**：仅静态（读锁/写锁路径逐行核对） · 置信度 高

---

## 4. 低危发现（合并陈述）

| 编号 | 位置 | 要点与修复 |
|---|---|---|
| SEC-10 | `runs.rs:298-303`、`cancel.rs:48-58`、`accounts.rs:157-162`、`schedules.rs:164-168`、`pipelines.rs:62-66` | "别人的"回 404、"压根不存在"回 400/200 ⇒ 可用手机号算出的账号 id 试出"这个号在本实例被托管"。`cancel.rs:43` 的注释（"与不存在区分不开"）与 8 行后的 `Unknown => 200` 直接矛盾。**修**：这几处统一成"不存在与不是自己的回同一个码"。 |
| SEC-11 | `server_logs.rs:35-45`、`logbuf.rs:119-128`、`run_labels.rs:99-110` | `/api/logs` 取 `CurrentUser` 只为过闸、取到就丢；环里每条任务日志都带**账号 id**（=手机号哈希）与任务正文。**已核实不含** token/authorization/完整手机号（`http_log.rs:185-192` 全链路 `redact`，账号显示走掩码）。**修**：给环里每行打 `user_id` 并按 `cur` 过滤。 |
| SEC-12 | `auth.rs:216-237,258` | 全文件只有三处 `tracing`（建号/升级/登录成功），**失败一条不落**，成功也不带来源 IP（拿不到 IP，见 SEC-02）。30 次/分钟 ≈ 4.3 万次/天静默尝试。**修**：失败路径 `warn!` + 用户名 + IP；加失败计数与退避。 |
| SEC-13 | `auth.rs:447-456` | 只有 `HttpOnly; SameSite=Strict; Path=/; Max-Age`，**永不写 `Secure`**（理由：默认跑 `http://127.0.0.1`，加了浏览器不存）。项目里**没有任何开关**能打开它，也没有 `__Host-` 前缀。**修**：`MCLOUD_COOKIE_SECURE`（或按 `X-Forwarded-Proto`）条件追加。 |
| SEC-14 | `traits.rs:382-383`、`json_store.rs:1329-1338` | `purge_expired_sessions` 有声明、有实现、**生产路径零调用**（只被测试与 engine 的测试替身调）。实测 `data/sessions.json` 已积 15 条 web 会话。**修**：在 `main.rs` 那个 2 秒攒批循环旁挂低频清扫。 |
| SEC-15 | `auth.rs:146-153`、`password.rs:196-200` | `is_client_digest` 只判"64 位小写 hex" ⇒ `SHA-256("a")` 直通跳过长度检查，可设 1 字符口令（前端 `auth-page.tsx` 那道只是 UI）。**不构成提权**（能走这条路的要么是初始化者、要么已持会话）。**修**：文档化这条边界，或在安全页明示。 |
| SEC-16 | `json_store.rs:56,623-654` | 只有 `runs` 有 `HISTORY_LIMIT=200`；`accounts`/`schedules`/`pipelines` **无条数上限**，且每次 `save_*` 都整文件序列化 + 原子写 + fsync ⇒ 写入总量 O(n²)。**修**：加条数上限，或改增量写。 |
| SEC-17 | `hub.rs:155-177,332-361`、`runs.rs:384-385,447-456` | `subscribe` 用 `entry().or_default()` 建**非终态**空条目（`evict_terminal_runs` 只回收 `terminal` 的）⇒ 订阅一个"内存历史已被回收的终态 run"时流**永不结束**（`run_access` 会因存储里有记录判 `Allowed`）。订阅者数量全仓无上限。另：`waiter.recv()` 把 `broadcast::RecvError::Lagged`（落后 256 条）当成"发送端没了"⇒ 流静默中断。**修**：内存无历史时改从存储层回放并立即发终局；订阅数设上限；区分 `Closed`/`Lagged`。 |
| SEC-18 | `system.rs:75-103,154-192` | `/api/system/storage` 回**绝对路径 + 数据目录每个文件名与字节数 + 可执行文件路径**；`/api/system/about` 回 commit/dirty/build_time + 私有仓库最近若干条提交标题。**修**：只给管理员，或路径换占位符。 |
| SEC-19 | `auth.rs:409-432`、`user.rs:100-111` | `require_session` 只查会话记录与到期时间，**不查 `users` 表**（`current_session` 是查的，两条路口径不一致），也不拒 `SessionKind::Cli`；而 `is_valid_at` 把 `expires_at_ms == 0` 当永不过期。今天没有可利用路径（生产只构造 `Web` 会话，也没有删用户的入口）。**修**：顺手 `store.user()` 确认用户还在，并显式拒 `Cli` 走 HTTP。 |
| SEC-20 | `logs.rs:38` vs `runs.rs:140-196` | 运行**结束**才落盘 ⇒ 运行中的 run 走 `/api/runs/{id}/logs` 一律 404，而同一时刻 `/events` 是 200（口径不一致；方向是失败关闭，不泄露）。`web/src/api.ts` 的 `getRunLogs` 仍**零调用点**。**修**：`/logs` 复用 `run_access`。 |
| SEC-21 | `auth_gate.rs:76-86` | `!path.starts_with("/api/")` 直接放行。`//api/accounts`、`/API/accounts`、`/api%2faccounts` 确实绕过了这一行，但 **matchit 不做归一化**（`tree.rs:476-479` 纯字节匹配）⇒ 同样匹配不到任何 handler，最终落到 SPA 回落，**没有业务 handler 被触达**（已交叉核对 axum 0.8.9 `path_router.rs:388` / http 1.5.0 `uri/mod.rs:332-338`）。安全性完全依赖"没有 handler 挂在 `/api/` 之外 + 框架不归一化"。**修（防御纵深）**：默认必须登录，只对显式白名单放行。 |
| SEC-22 | `auth_gate.rs:79-82`、`static_files.rs:111-124` | 静态资源免认证、无速率/连接限制；debug 构建下 rust-embed 每请求读盘；单请求最大 ~155 KB（gz 48 KB）。与 SEC-01 叠加。**修**：反代层限制。 |

---

## 5. 已核实为「不是问题」（专门查过并排除）

1. **静态文件路径穿越**：`WebAssets::get(path)` 是**精确查表**（rust-embed 8.12 生成代码先 canonicalize 再校验前缀），
   `/../…` 拿不到 `web/dist` 之外的东西；`assets/` 未命中与"末段带点"的路径都回 404（`static_files.rs:137-140`）。
2. **请求体大小有上限**：axum 默认 2 MiB（`axum-core-0.5.6/src/ext_traits/request.rs:319` `DEFAULT_LIMIT = 2_097_152`，
   `:326` 无 `DefaultBodyLimit` 扩展时套 `Limited`），全仓 `DefaultBodyLimit|RequestBodyLimit` **零命中** ⇒ 没被关掉。
3. **请求头/URL 有硬上限**：hyper `DEFAULT_MAX_BUFFER_SIZE = 8192 + 4096*100`、`DEFAULT_MAX_HEADERS = 100`
   ⇒ 不存在"无限长的解析"。
4. **CSRF 面基本关死**：cookie `HttpOnly; SameSite=Strict`，无 CORS 层，写接口全在 `/api/` 且方法非 GET；
   `DELETE /api/session` 虽公开，但跨站请求不带 cookie ⇒ 现代浏览器下不构成可用 CSRF。
5. **会话令牌**：`OsRng` 取 32 字节（256 bit）hex；库里只存 `sha256(token)`；原文只出现一次（`Set-Cookie`）。
6. **无会话固定**：登录与建号都调 `make_session` 生成全新令牌，从不采纳请求里的旧 cookie。
7. **改口令踢全部旧会话**：`security.rs:166` → `remove_user_sessions`（按 `user_id` 一刀切）后才发新令牌。
8. **改口令要当前口令**，且该路由在闸后 + `SameSite=Strict` ⇒ 手滑防护成立（会话被盗时不拦，语义如此）。
9. **TTL 不可能 0/无限**：`settings.rs:27-30` 钉死 `min_password_len ∈ [4,64]`、`session_ttl_hours ∈ [1,2160]`，
   `put_security` 与 `save_settings` 都校验。
10. **`sv-4` 用户名存在性时序 —— 已修复**：`auth.rs:211-219` 用户不存在时先对固定假 PHC 跑一次真 argon2；
    交叉核对 `argon2-0.5.3` 默认 `m=19456,t=2,p=1` 与 `data/users.json` 实际 PHC **逐项相同**。
    （残留：老档记录若参数非默认会有弱差异，量级远小于 argon2 主体。）
11. **`sv-6` 登出清 cookie —— 已修复**：`auth.rs:276-296` + `:341-344` 回 `Max-Age=0` 且属性逐项对齐，
    集成测试 `login_routes.rs:686-731` 钉着。
12. **`sv-7` 扫码会话 TTL 清扫 —— 已修复**：`login.rs:69-73` 的 `insert` 先 `retain(|_, s| !s.qr.is_expired(now))`，
    判据用上游给的 `started_at_ms + ttl_seconds`（残余：只在下次 insert/put_back 时清，无定时任务）。
13. **日志里的秘密**：`http_log.rs:185-192` 的 `safe()` **先脱敏再截断**（顺序有注释与单测钉着），
    连 `trace` 档的请求头/正文也走它；账号显示走 `mask_phone`/`account_label`（掩码 + 哈希）。
    ⇒ `/api/logs` 与 `/api/runs/{id}` 里**没有** token / authorization / 完整手机号。
14. **`gzip` 解压无上限但不在生产路径**：`gunzip`（`gzip.rs:53-61`）只有 `decode_target`（`smid.rs:386`）
    在调，而后者**只被测试用**（`smid/tests/payload.rs`）；固定 IV / 硬编码密钥是**上游协议复刻**
    （`aes.rs:20-27` 写明"复现 rt.crypto.aesEncrypt"），不是本仓自造。
15. **仓库卫生**：`data/` 从未入库（`git log --all --diff-filter=A -- data/` 空、`git ls-files data` 为 0），
    权限 0600/0700；`target/`、`.cargo-home/` 在 `.gitignore` 里。
16. **闸门公开清单无漏网**：`PUBLIC` 只有 6 条（setup GET/POST、session POST/DELETE、health、version），
    逐条对得上"没有它就没法登录/存活探测"；`is_public` 是**精确相等**（不是前缀），方法也参与匹配。

---

## 6. 对 `docs/review/findings-261004-server.md` 的核对（今天的状态）

| 条目 | 结论 | 证据 |
|---|---|---|
| `sv-4` 登录时序侧信道 | **已修复** | `auth.rs:211-219` + `:303-311`（dummy argon2，参数逐项一致） |
| `sv-5` 认证面无限速 | **部分修复** | 限速已加且在 argon2 之前（`auth.rs:193`、`ratelimit.rs`），但键是全局的、被拒不计数 ⇒ **变成 SEC-02** |
| `sv-6` 登出不清 cookie | **已修复** | `auth.rs:276-296`、`:341-344` |
| `sv-7` 扫码会话无 TTL 清扫 | **已修复** | `login.rs:69-73` |
| `sv-8` 推送 URL 盲 SSRF | **已加闸但可绕过** | 闸在 `push.rs:994`，绕过见 **SEC-04**（IPv4-mapped / WHATWG 写法） |
| `sv-9` `/logs` 与 `/events` 口径不一致 | **仍在** | `logs.rs:38` vs `runs.rs:140-196` ⇒ SEC-20 |
| `sv-10` 订阅已回收终态 run 会建永不回收条目 | **仍在** | `hub.rs:155-177` 的 `or_default()` + `:332-361` 只回收 `terminal` ⇒ 并入 SEC-17 |
| `sv-1`（并发闸早退泄漏取消登记） | 已修（旁证） | `runner.rs:410` 有 `cancel.release`，且 `run_owners` 有 `retain` 清理（`state.rs:75`） |

---

## 7. 修复优先级建议

**第一批（一次改动就能显著改变攻击面，建议先做）**

1. `main.rs`：把 `axum::serve(listener, router)` 换成
   `into_make_service_with_connect_info::<SocketAddr>()`，并给 hyper 装 `TokioTimer`
   （顺带让 30 秒请求头读超时生效）—— **同时解决 SEC-01 与 SEC-02 的前提**。
2. `ratelimit.rs`：键换成 `IpAddr`（+ 用户名维度），"已初始化实例的 setup 拒绝"与"成功登录"不占配额；
   对同一用户名连续失败做指数退避。
3. 加 `tower::timeout::TimeoutLayer` + `ConcurrencyLimitLayer`（或反代层等价物）。
4. `push.rs`：先 `url::Url::parse` 取归一化 host 再判公网，补 `to_ipv4_mapped()`。

**第二批（多租户/正确性，改动小但需要想清楚语义）**

5. `accounts.rs` / `login.rs`：写归属前用不带过滤的 `store.account()` 做存在性检查（SEC-05）；
   `QrSession` 加 owner（SEC-06）；`task_config` 按 `UserId` 分键（SEC-07）。
6. `store`：加 `create_first_user`（写锁内判空）与 `name` 唯一性（SEC-09）。
7. 把所有跑 argon2 的路由与 `/api/login/*` 纳入限速（SEC-08）。
8. `/api/logs` 按 owner 过滤；`/api/system/{about,storage}` 与 `restart` 收进管理员（SEC-07/11/18）。

**第三批（加固与一致性）**

9. 认证失败日志 + 失败计数（SEC-12）；cookie `Secure` 开关（SEC-13）；
   会话清扫挂到既有 2 秒循环（SEC-14）；`require_session` 补用户存在性与 `SessionKind` 校验（SEC-19）。
10. SEC-10 的存在性口径统一、SEC-15 的文档化、SEC-16 的条数上限、SEC-17 的 SSE 收尾与订阅上限、
    SEC-21 的"默认拒绝"闸门写法。

---

## 8. 诚实边界（本报告**没有**做的事）

- **没有跑任何编译/测试**（`cargo check` / `cargo test` 都没跑）⇒ 本报告不构成"代码能编译/测试全绿"的断言。
- **没有对 `:3000` 发任何业务请求**，没有构造双用户实例 ⇒ SEC-05/06/07/09 的**可利用性**是逐行读出来的，
  没有实测复现；SEC-01 的"408 KiB/连接"、SEC-08 的放大倍数也没有实测量化。
- **SEC-04 是唯一实测项**：用独立小程序（`/tmp`，只读链接工作区 `target/debug/deps` 的 `liburl`）
  验证了 `url` 2.5.8 的归一化结果与"闸看到的主机串"；**没有**真的让服务发一次请求。
- **多用户今天不可达**：`POST /api/setup` 只在 0 用户时可建，无用户管理路由、无删用户 CLI
  ⇒ SEC-05/06/07/11/18 的危害取决于"真的会跑多用户"（手改 `users.json` 或将来加功能），
  因此按"中/低"记而不是"高"。
- **未覆盖**：`crates/mcloud-server/src/cli/*`（5 个文件）、前端交互与状态机、
  `mcloud-tasks` 各任务体自身的资源消耗、上游（移动云盘）的限流阈值与风控行为、
  HTTP/2（h2c）路径的独立核查、`smid.rs` 指纹计算的正确性。
- **基线是工作区**（HEAD `86dc1bc` + 未提交改动）：审计读的是磁盘上的当前文件，
  与 `git show HEAD:` 的内容可能有差异。

---

## 9. 附录：第一批修复记录（2026-10-06，用户点头后落地）

> ⚠ 这一节是**审计之后**追加的 —— 上面 §2~§8 的"现状"描述写的是**修复前**的代码。
> 修复状态以本节为准。

### 9.1 落了什么

| 编号 | 状态 | 改动 | 验收读数 |
|---|---|---|---|
| **SEC-02** | **已修复** | 限速器从"整个进程一个计数器"改成**按 peer IP 分桶**（`ratelimit.rs`：`HashMap<String, VecDeque<i64>>`，键数上界 `DEFAULT_MAX_KEYS = 4096`）；`main.rs` 的 `axum::serve` 接 `into_make_service_with_connect_info::<SocketAddr>()`；`auth.rs` 用 `Option<Extension<ConnectInfo<SocketAddr>>>` 取键，拿不到时退化成 `"unknown"` 一个桶（**退化成粗键，不是退化成不限速**） | 单测 5 条（含"一个 IP 打满不影响另一个"）；集成测试 `the_auth_rate_limit_is_per_peer_ip`；**真机冒烟**：新二进制起在 `127.0.0.1:3999`，连发 31 次 `POST /api/session` ⇒ **前 30 条 400、第 31 条 429 + `retry-after: 60`** ✓ |
| **SEC-04** | **已修复** | `validate_push_url` 不再自己切字符串，改成 `url::Url::parse` → `Url::host()`（与 reqwest **同一把尺子**）；`is_public_ip` 补 `to_ipv4_mapped()`；域名判据先 `trim_end_matches('.')` | 新增回归测试 `whatwg_ipv4_spellings_cannot_smuggle_a_private_host_past_the_gate`（15 条绕过写法全拒 + 3 条公网地址不误伤）；既有闸测试 22 条照旧全绿 |
| **SEC-01** | **部分修复** | 加了**并发上限**（`ConcurrencyLimitLayer`，128，罩在最外层 ⇒ 30 并发 argon2 的 ~570 MiB 尖峰变成有界）与**认证面 30 秒硬超时**（`MethodRouter::layer(TimeoutLayer::with_status_code(..))`，只给 `/api/setup` 与 `/api/session`，**不**给 SSE / 同步跑任务那两条长跑路由） | 冒烟里 31 连发 + 静态请求全部正常返回（并发层不会把请求挂住）✓ |

**SEC-01 仍然缺的那一半**（这条要诚实说）：**slowloris / 请求头读超时**没修。
根因是 `axum::serve` **不暴露** hyper 的 builder（`axum-0.8.9/src/serve/mod.rs:391` 内部只
`Builder::new(TokioExecutor::new())`、从不 `.timer(...)`），要让它生效得**手写 accept 循环**
（自己 `hyper_util::server::conn::auto::Builder` + `TokioTimer` + 优雅收尾），
而那会重写"服务怎么起、怎么收"这条**目前只有 launchd 在跑、我没法在真机上反复验**的路径。
⇒ 按"宁可不改，也不要拿没验过的启动路径去赌"处理，留作下一批；
在那之前，绑 LAN 的部署应当由**反代**限制连接数与读超时（或退回 `127.0.0.1`）。

### 9.2 顺手发现的一条**构建纪律**（比修复本身更该记住）

> `cargo test -p mcloud-server --lib --test login_routes --test routes`
> **照样会重链接 `target/debug/mcloud`**（也就是 launchd 正在跑的那个文件）。

- **成因**：cargo 在编译**集成测试**时必须为它准备 `CARGO_BIN_EXE_<name>` 环境变量
  ⇒ 于是把该 package 的 **bin 目标一起编出来**。所以 `--lib --test X` 这种"我只挑测试目标"
  的写法**挡不住**它 —— AGENTS.md 里"`cargo test -p mcloud-server` 会重链接"那条，
  准确说法是"**只要选了集成测试目标就会**"。
- **本次实测**：`cargo check --all-targets` 之后 inode 纹丝不动（`4039060`）；
  `cargo test ... --test login_routes --test routes` 跑到 14:38:40 时 inode 变成 `4054533`。
- **代价（这次是轻的那一档）**：进程**没有**被 SIGKILL、PID 没变、`/api/health` 照旧 ok
  ⇒ **没有弹 TCC 授权框**、没有重启循环。只是变成 AGENTS.md 说的那个中间态：
  **盘上是新的、跑着的是旧的**（实测：跑着的进程持有 `inode=4039060`，盘上那份是 `4054533`，
  且新那份里能按字节数数到这次新增的错误串）。
- **想跑集成测试又不碰线上那份**：`CARGO_TARGET_DIR=target/lines/<名字>`（代价是全量冷编）；
  只跑**单元测试**（`--lib`）则安全 ✓。

---

## 10. 附录：第二批修复记录（2026-10-06，用户点头后落地）

> 对应 §7 的**第 5~8 条**（多租户 / 正确性）。上面 §2~§9 的"现状"写的是**修复前**的代码，
> 修复状态以 §9 与本节为准。

### 10.1 落了什么

| 编号 | 状态 | 改动 | 验收读数 |
|---|---|---|---|
| **SEC-05** | **已修复** | 三处一起改：① `accounts::import` 写归属前先用**不带过滤**的 `store.account()` 查一次，属于别人 ⇒ 404；② `persist_login` 从"整行覆盖"改成**保留式合并**（只换凭据/设备身份，保留 `nickname` / `settings` / `enabled` / `owner`）；③ `claim_account` 以**库里那一行**为准、只改 `owner`，归属冲突 ⇒ 404 | 新增 `persist_login_keeps_nickname_settings_enabled_and_owner`（tasks）；新增 `importing_an_account_owned_by_someone_else_is_404`（含"主人自己照样能更新"的反向断言）✓ |
| **SEC-06** | **已修复** | `QrSession` 加 `owner: UserId`（出码时记发起者）；`poll` / `cancel` 都要求"你得出示同一个登录会话"，不匹配 ⇒ 404（**且把会话放回去** —— 否则猜一个 id 就成了一次对别人扫码流程的拒绝服务）；`LoginSessions::remove` → `remove_for(id, owner)` | 表级单测 `a_qr_session_belongs_to_the_user_who_started_it`；路由级 `a_qr_session_can_only_be_cancelled_by_its_starter`；真机冒烟：取消自己的码 200、再取消 404 ✓ |
| **SEC-07** | **已修复** | 新增 `mcloud_core::Error::Forbidden`（→ **403**，不是 401 —— 401 会让前端弹登录框，把"权限不够"显示成"你掉线了"）与 `auth_gate::{is_admin, require_admin}`；管理员判据 = **`users` 里创建最早的那个**（与 `legacy_owner` 同一判据）。收归管理员的：`POST /api/system/restart`、`GET /api/system/storage`、`GET`/`PUT /api/tasks/{name}/config`、`PUT /api/admin/security`、`GET /api/logs` | 新增 `instance_level_actions_are_admin_only`（六个动作 × 两个身份：非管理员 403、管理员不被挡）✓ |
| **SEC-08** | **部分修复** | 新增 `ServerState::sensitive_limiter`（**键 = 用户 id**，10/分）与 `Error::RateLimited`（→ 429 + `Retry-After`）；接上 `PUT /api/admin/password`（每次 argon2）与 `POST /api/login/{sms,app/sms}/start`（**真发短信**那两条）。**仍未限速**：`POST /api/runs`（见 10.3） | 新增 `changing_password_is_rate_limited_per_user`（10 发 401 + 第 11 发 429）；真机冒烟：12 连发 ⇒ `401×10 + 429×2`，且 429 带 `retry-after: 46` ✓ |
| **SEC-09** | **已修复** | `UserStore::create_first_user`（默认实现**报错**，与 `save_settings` 同一口径）+ `JsonStore` 里**在同一把写锁内**判空并插入；`setup_create` 改用它，"竞争失败"回 400 | 新增 `create_first_user_only_works_on_an_empty_instance`（含重启后仍 1 个用户）；真机冒烟：第二次 `POST /api/setup` ⇒ 400 ✓ |
| **SEC-11** | **已缓解**（不是"按归属过滤"） | `GET /api/logs` 收归管理员。`logbuf` 里没有"这条属于谁"这个字段 ⇒ **按归属过滤在数据结构上做不到**，硬做要么给每行加 `user_id`（改动面很大）、要么按当前用户猜（更糟 ✗） | `instance_level_actions_are_admin_only` 里那条 ✓；单管理员部署读数**完全不变** ✓ |
| **SEC-18** | **部分修复** | `GET /api/system/storage` 收归管理员；**`GET /api/system/about` 刻意保持开放** —— 它是版本/构建信息（前端"关于"卡片要用），没有路径与文件清单，藏它只会让界面坏掉 ✗ | 同上 ✓ |

### 10.2 与 §7 建议的**两处刻意偏离**（都写在这里，免得以后被当成"漏做"）

1. **`tasks/*/config` 收归管理员，而不是"按 `UserId` 分键"**。按用户分键要把存储键从
   `任务名` 改成 `(用户, 任务名)`（含存量迁移 + 前端语义），而多用户今天**不可达**
   ⇒ 那是给一条走不通的路修护栏 ✗。收归管理员在单管理员实例上**零行为变化**，
   而越权面（"B 能改 A 的任务参数"）直接消失 ✓。真要分用户时，改动点只有那一处存储键。
2. **`/api/logs` 收归管理员，而不是"按 owner 过滤"**：理由同上（`logbuf` 没有归属字段）。
   副作用要说明白：**第二个用户从此看不到任何日志**（包括自己的）——
   在多用户不可达的今天这是可接受的取舍，但它确实是"功能损失换安全" ✓。

### 10.3 这一批**没做**的（留给第三批）

- **SEC-08 的 `POST /api/runs`**：它会真跑任务（分钟级、有上游副作用），限速要考虑
  "多账号一次跑 N 个"是正常用法 ⇒ 阈值语义得先想清楚，没在这一批里拍脑袋定 ✗。
- **SEC-03（凭据明文落盘）**：需要密钥管理（Keychain / 口令派生密钥）与存量迁移，
  是独立一件事。
- **SEC-01 的另一半（slowloris / 请求头读超时）**：见 §9.1 末尾 —— `axum::serve`
  不暴露 hyper builder，要修得手写 accept 循环。
- **§7 的第三批**：SEC-10 / SEC-12 / SEC-13 / SEC-14 / SEC-15 / SEC-16 / SEC-17 /
  SEC-19 / SEC-20 / SEC-21 / SEC-22。

### 10.4 顺手发现的一条**测试**纪律（与 §9.2 的构建纪律并列）

> `login_routes.rs::cancelling_qr_session_removes_it` 以前是**因为错误的原因**通过的。

它三次调用 `authed_app(handle.clone())`，而那个辅助函数每次都会 `ServerState::new`
⇒ **扫码会话表每次都是空的**。于是"DELETE 返回 200"其实是因为
"表本来就是空的、`remove` 无条件成功"，这条测试**从来没有验证过"取消真的把会话摘掉了"** ✗。
绑定发起者（SEC-06）之后它立刻变红 —— 现在改成**路由只建一次**（同一份 `ServerState`），
它才真的在测那件事 ✓。

⚠ 教训：**跨请求的状态断言必须共用同一个 app 实例**。`authed_app` / `two_user_app`
这类辅助函数每调一次就是一个新实例，"第二次调用还看得见第一次的状态"是**错的**。
（本仓已经有两处踩过：这一处，以及 `login_routes.rs` 里那条写明"路由只建一次"的用例。）

---

## 11. 附录：第三批修复记录（2026-10-06，用户点头后落地）

> 对应 §7 的**第 9~10 条**（加固与一致性）。上面 §2~§10 的"现状"写的是**修复前**的代码。

### 11.1 落了什么

| 编号 | 状态 | 改动 | 验收读数 |
|---|---|---|---|
| **SEC-12** | **已修复** | ① 认证失败/成功都记日志，**带 peer IP**（`登录失败` / `登录成功` / `拒绝重复初始化`），**绝不记口令**（日志会进 `logbuf`、经 `/api/logs` 读回来 ✗）；② 新增 `login_fail_limiter`（键 = **`用户名@peer IP`**，5 次 / 5 分钟）：失败时 `check`、成功时 `reset` | 新增 `repeated_wrong_passwords_are_limited_but_the_right_one_still_works`；真机冒烟：6 连发错口令 ⇒ **`401×5 + 429×1`**，紧接着**口令正确的那一次 → 200** ✓ |
| **SEC-13** | **已修复** | `ServerState::cookie_secure`（由 `MCLOUD_COOKIE_SECURE=1/true/yes` 打开）＋ `set_cookie_response` / `clear_cookie_response` 收 `secure` 参数 | 新增 `cookie_can_carry_secure_when_the_deployment_asks_for_it`；真机冒烟：默认那份 `set-cookie` **无** `Secure`，设了开关那份**有** `Secure` ✓ |
| **SEC-14** | **已修复** | 把 `purge_expired_sessions` 挂进**既有的 2 秒循环**（每 30 个 tick ≈ 60 秒一次）—— 不新起定时器，就不多一处要跟着 shutdown 收尾的东西 | `purge_expired_sessions` 本身早有 `json_store` 单测；接线是"每 60 秒调一次"这一句 ✓ |
| **SEC-15** | **已文档化**（刻意接受） | 在 `setup_create` 的长度检查处写明：任何 64 位 hex 都会被当"前端摘要"从而跳过长度检查，**这不是提权**（那个串仍要经 argon2 落库、下次登录还得原样再发一遍才进得来；摘要本身就是凭据，其长度不受"口令下限"约束是摘要方案的固有性质）。真要卡得改成"服务端下发的一次性挑战" | — |
| **SEC-16** | **已修复** | `routes/mod.rs` 加 `MAX_ACCOUNTS = 200` / `MAX_SCHEDULES = 500` / `MAX_PIPELINES = 200`，三处 **只在新建时**判（更新已有条目永不被"满了"挡住 —— 那比灌爆更难受 ✗） | 上限常量 + 三处判断 ✓（⚠ 这三条**没有**集成测试：造 200 条账号太慢，判断本身是一行 `len() >= MAX`） |
| **SEC-19** | **已修复** | `require_session` 补两条：① 只认 `SessionKind::Web`（`Cli` 会话是"永不过期"的，被塞进 cookie 就是一条永不失效的登录态 ✗），② `store.user()` 确认用户还在（会话表与用户表之间没有外键）。两条都**当场删掉那张会话**（失败关闭） | 新增 `cli_sessions_are_not_accepted_as_web_cookies`（`expires_at_ms = 0` 的 CLI 会话 ⇒ 401）与 `a_session_of_a_user_that_no_longer_exists_is_rejected` ✓ |
| **SEC-21** | **已修复**（防御纵深） | `gate` 先拒"本应用不可能产生的路径形状"：`//`、`.`/`..` 段、以及它们的百分号编码（`%2f` / `%2e`，不分大小写）⇒ 404。这样"闸的判定"与"路由的判定"之间不再有可利用的差额 | 新增 `suspicious_path_shapes_are_rejected`（9 条可疑 + 7 条正常不误伤）；真机 `--path-as-is` 复核：`/api/./accounts`、`/../api/accounts`、`/api/../api/accounts`、`/%2e%2e/api/accounts` 全 **404**，而 `/` 仍 200 ✓ |
| **SEC-22** | **已缓解** | 第一批的 `ConcurrencyLimitLayer(128)` 罩在最外层 ⇒ 静态资源也在并发上限之内 | 见 §9.1 ✓ |

### 11.2 ⚠ 一条**测试**教训（这已经是第三次同类）

真机冒烟第一版里 `//api/accounts` 之外那两条（`/api/./accounts`、`/../api/accounts`）拿到的是
**401 而不是 404** —— 不是闸没生效，而是 **`curl` 自己先把路径归一化了**（`/api/./accounts` → `/api/accounts`），
服务端根本没看到那个形状 ✗。换 `curl --path-as-is` 之后四条全是 404 ✓。

⇒ 教训：**验证"原始路径/原始请求行"这类东西，必须确认客户端没有先替你做归一化**。
同类还有 `--path-as-is`、`--no-buffer`、以及 `curl` 默认会合并的重复 header ✓。

（另外第一版冒烟脚本把 `rm -rf "$DIR"` 写在 `cp "$BIN" "$DIR/mcloud"` **之后** ⇒ 把刚拷进去的
二进制删了，全部请求 `000`。这种"脚本自己错了、却看起来像被测对象错了"的形态，
排查时**先看日志再看断言**最省时间 ✓。）

### 11.3 这一批**没做**的（连同理由）

- **SEC-17（SSE）**：`hub.rs` 的 `or_default()` 建非终态空条目 ⇒ 订阅"内存历史已被回收的终态 run"
  会得到一条**永不结束**的流；订阅者数无上限；`waiter.recv()` 把 `Lagged` 当 `Closed`。
  修它要动 `hub.rs` 的订阅/回放路径 + `runs.rs` 的事件流，**改错的表现是"事件流静默少事件"**
  （比现在这种"流不结束"更难发现）⇒ 单独一批做，且要有"订阅已回收终态 run 会立刻收尾"的测试 ✓。
- **SEC-20（口径不一致）**：`/api/runs/{id}/logs` 对"运行中"的 run 回 404，而 `/events` 回 200。
  方向是**失败关闭**（不泄露），且 `web/src/api.ts` 的 `getRunLogs` **零调用点** ⇒
  改它只影响一个没人用的端点，收益是"两条路口径一致"。留到最后一批（要顺带核对 `run_access` 的语义）。
- **SEC-10（存在性预言机）**：**刻意不改**。它要求把"账号不存在"从 400 改成 404 ——
  而那个 400 带着"账号不存在：xxx"的**可用提示**（前端直接展示给用户），
  今天唯一的部署形态是**单管理员**，此时"成员资格"这件事不存在（能走到那些路由的人就是唯一用户）
  ⇒ 改了只有 UX 损失、没有安全收益 ✗。**触发条件**：多用户可达（真加了用户管理）时再改，
  那时连同 SEC-07 的"按用户分键"一起做 ✓。

### 11.4 三批下来的总账（⚠ 截至**第三批**；第四批见 §12.4）

| 状态 | 编号 |
|---|---|
| **已修复** | SEC-02、SEC-04、SEC-05、SEC-06、SEC-07、SEC-09、SEC-12、SEC-13、SEC-14、SEC-16、SEC-19、SEC-21 |
| **部分修复 / 已缓解** | SEC-01（并发上限 + 认证面超时；**slowloris 那半仍未做**）、SEC-08（改口令/发短信已限速；`POST /api/runs` 未限速）、SEC-11（收归管理员，未按归属过滤）、SEC-18（`storage` 收归管理员；`about` 刻意开放）、SEC-22（并发上限覆盖） |
| **刻意不改（有理由）** | SEC-10（见 11.3）、SEC-15（已文档化） |
| **未做** | SEC-03（凭据明文落盘，要密钥管理 + 存量迁移）、SEC-17（SSE）、SEC-20（口径） |

⚠ **上线状态**：**四批全部已上线**（2026-10-06 17:13，`scripts/dev-reload.sh`）——
落地记录见 `docs/task-log.md` 第 121~126 行，本次部署的读数见下面的 §13。

---

## 12. 附录：第四批修复记录（2026-10-06，用户「接着做」之后）

> 对应 §7 收尾剩下的两项：**SEC-17（SSE）** 与 **SEC-20（口径不一致）**。

### 12.1 落了什么

| 编号 | 状态 | 改动 | 验收读数 |
|---|---|---|---|
| **SEC-17①** | **已修复** | `EventHub::subscribe` **不再** `entry().or_default()` 凭空建条目，改成 `get_mut(..).ok_or(NotTracked)?` —— 判存在与建订阅在**同一把锁内**完成（没有 TOCTOU 窗口）。旧行为会建一个**非终态**空条目：`drain` 永远空、通知永远等不到 ⇒ 那条 SSE **永不结束**；而且它**永远不会被回收**（`evict_terminal_runs` 只收 `terminal` 的）⇒ 反复订阅不存在的 run 就是一条内存泄漏 ✗ | `subscribing_to_an_untracked_run_is_refused_and_leaves_nothing_behind`；真机：见 12.2 的 0.07 秒 |
| **SEC-17②** | **已修复** | 同一条 run 的订阅者上限 `MAX_SUBSCRIBERS_PER_RUN = 32`，超了回 **429 + `Retry-After`**（不是默默挂上去）。理由：一条 SSE 订阅 = 一个常驻任务 + 一份队列，而**它不受 HTTP 层并发上限约束**（`ConcurrencyLimitLayer` 的名额在 handler 返回响应那一刻就还回去了，事件流在那之后还要活很久）⇒ 一个已登录用户开一堆连接就能把内存吃光 ✗ | `subscriber_count_is_capped_and_frees_up_after_unsubscribe` |
| **SEC-17③** | **已修复** | `waiter.recv()` 区分 `Lagged` 与 `Closed`：落后**不是**"发送端没了"，旧代码一律 `return None` ⇒ 那条流**静默中断**（前端看到"日志停在半截"而没有任何提示）✗。通知频道只是"醒一醒"的信号，事件本体在每个订阅者**自己的队列**里 ⇒ 漏掉几次唤醒不会丢事件，回到循环顶部 `drain` 一次就补上了 ✓ | 代码路径 + 注释；`Lagged` 那条分支没有专门的用例（要造出"落后 256 条通知"才能触发，成本高于收益 —— ⚠ **这是一处已知的测试缺口**，写在 12.4） |
| **SEC-17④** | **已修复** | "已结束、内存历史已回收"时改走 `finished_stream`：从**存储层**取终局，发**一条** `finished` 事件（带**真实**的 `outcome` 与 `duration_ms`）然后立刻收尾 | `subscribing_to_a_finished_run_replays_the_terminal_event_then_ends`；真机见 12.2 |
| **SEC-20** | **已修复** | `/api/runs/{id}/logs` 改用 `run_access`（与 `/events` 同一个判据）：它三处都看（本服务发起登记 / 存储层 / 内存中枢），并且"别人的"与"压根不存在"回**同一个 404**。旧代码只查存储层 ⇒ 一条**正在跑**的 run（记录是结束时才落盘的）一律 404，而同一时刻 `/events` 是 200 —— 同一个 run、两个端点给出相反的答案 ✗ | `logs_of_a_running_run_are_200_just_like_its_event_stream`；真机：存在 → 200、不存在 → 404，两个端点一致 ✓ |

### 12.2 真机验证（`:3993`，**没碰 :3000**）

造一条"只在 `history.json` 里、内存中枢里没有"的已结束记录（`run_id=4242`，
`duration_ms=4321`，从线上那份真实 `history.json` 借的记录**形状**，保证格式逐字对得上），
然后：

```
① 订阅它：curl -N --max-time 8 … /api/runs/4242/events
   耗时 0.07 秒（远小于 8 秒超时 ⇒ 流自己收尾了 ✓）
   event: finished
   data: {"type":"finished","run_id":4242,"outcome":{"status":"success",…},"duration_ms":4321}
   ⇒ 终局事件带的是**真实**结论与真实时长（不是现编的 0）✓
② 同一时刻 /api/runs/4242/logs → 200；/api/runs/9999/logs 与 /api/runs/9999/events → 都 404 ✓
```

⚠ **修复前的表现**是这条 curl 一直挂着、直到 `--max-time 8` 把它掐掉（退出码 28）——
"永不结束"这条 bug 的判据就是**耗时**，不是状态码 ✓。

### 12.3 ⚠ 三处**测试**教训（这一批尤其多）

1. **`sse_disconnect_unsubscribes_from_the_hub` 真的逮到了新口径**。它先断言
   `subscriber_count == 1`（注释里写着"只断言断开后是 0 的话，订阅根本没发生也能绿 —— 那是摆设"），
   而新口径下"存储层有记录、内存里没有"= 已结束 ⇒ handler 直接回放终局、**不建订阅者**
   ⇒ 那条"摆设断言"当场变红 ✓。**这就是先写"前提断言"的价值**：它这次不是形式主义。
   修法不是放宽断言，而是**把前提造对**：让这条 run 真的在内存中枢里（= 真在跑）。
2. **"两条返回路径必须是同一个具体类型"**。这条 handler 现在有两条返回路径
   （正常订阅 / 终局直发），`impl Trait` 在返回位置只允许**一个**具体类型 ⇒ 必须装箱；
   而 `keep_alive` 会**再包一层**（`KeepAliveStream<S>`）⇒ 装箱必须在 `keep_alive`
   **之前**做，否则两条路的类型仍然不同（实测先写成 `Sse<EventStream>` 直接 E0308）。
   现在两条路都经过同一个 `sse_response()`，keep-alive 的配置也只有那一份 ✓。
3. **测试自己绕过 `authed_app` 就丢了"建会话"这一步**。`authed_app` 顺带建了测试用户与会话，
   而 SEC-20 的用例要往 `ServerState` 里登记归属（`note_run_owner`）⇒ 必须自己造 state
   ⇒ 绕过了 `authed_app` ⇒ 请求直接 **401**，而那个报错看起来像"鉴权坏了"✗。
   已把建会话抽成 `seed_test_user_and_session`（两个入口共用），并留了注释说明为什么要有它 ✓。

### 12.4 收尾总账（截至第四批）

| 状态 | 编号 |
|---|---|
| **已修复** | SEC-02、SEC-04、SEC-05、SEC-06、SEC-07、SEC-09、SEC-12、SEC-13、SEC-14、SEC-16、SEC-17、SEC-19、SEC-20、SEC-21 |
| **部分修复 / 已缓解** | SEC-01（并发上限 + 认证面超时；**slowloris 那半仍未做**）、SEC-08（改口令/发短信已限速；`POST /api/runs` 未限速）、SEC-11（收归管理员，未按归属过滤）、SEC-18（`storage` 收归管理员；`about` 刻意开放）、SEC-22（并发上限覆盖） |
| **刻意不改（有理由）** | SEC-10（改成 404 只有 UX 损失，触发条件是"多用户可达"）、SEC-15（已文档化：摘要长度不受口令下限约束是摘要方案的固有性质） |
| **刻意不做（用户口径）** | **SEC-03**（凭据明文落盘）—— 2026-10-06 用户看过三套密钥方案后选**「先不做」**：保持现状，作为**已知风险**留在本报告里（理由与三套方案见下） |

#### ⚠ 已知测试缺口（不藏着）

- **SEC-17③ 的 `Lagged` 分支没有用例**：要触发它得让某个订阅者落后 256 条通知，
  成本高于收益。判据靠代码路径 + 注释；**如果要补**，思路是"订阅后连发 300 条事件再 drain"。
- **SEC-16 的三处上限没有用例**：造 200 条账号太慢；判断本身是一行 `len() >= MAX`。
- **`MAX_SUBSCRIBERS_PER_RUN` 只在 hub 单测里验过**，没有端到端（要开 33 条 SSE 连接）。

#### SEC-03 为什么必须单独一批（⚠ 用户已定：**先不做**，作为已知风险保留）

它**不是**"加个加密调用"就完事的：密钥放哪决定了三件事 ——
① 服务是**无人在场**启动的（调度器要解密凭据去跑任务）⇒ 密钥不能靠"登录时输入口令"派生；
② 换密钥/迁移存量（`accounts.json` 里现在是明文）要**可回滚**，写错了用户就再也登不上自己的云盘；
③ macOS Keychain / Linux Secret Service / 文件密钥三套后端的能力不一样，
选哪套要用户点头（**这件事做错了撤不回来**，属于"必须先问"那一类 ✓）。

---

## 13. 上线记录（2026-10-06 17:13）

用户点头「现在上线第三+四批」之后走的 `scripts/dev-reload.sh`（**带构建**那条路）。

| 项 | 读数 |
|---|---|
| 走的哪条路 | `scripts/dev-reload.sh`（**不是** `--no-build`）—— 因为它**先 `bootout` 再编**，既保证盘上那份与源码一致，又关掉"覆写正在跑的二进制 ⇒ SIGKILL ⇒ `KeepAlive` 拉起未签名的新文件"那个窗口 ✓ |
| 构建 | `cargo build -p mcloud-server --bin mcloud` 实测 **3m02s**，七个 crate 全部**重编** ⇒ ⚠ 说明盘上那份**确实是旧的**（16:56 之后跑过 `cargo fmt`，格式化改动了源码 ⇒ 只签名不重建会上一份与源码有细微差别的产物）。**这条是"上线必须带构建"的实证** ✓ |
| 签名 | 固定身份 `MacDev`，要求 `identifier mcloud and certificate leaf = H"3d79b3f3…"`，输出 `replacing existing signature` ✓ |
| 重启 | 新 **PID 91588**（旧的 72880）；跑着的 `inode=4099289` == 盘上 `inode=4099289` ⇒ **生效的确实是新代码** ✓ |
| **TCC** | 这次是**重编 + 重启**（最容易弹框的组合），`PROMPTING` 计数 **0** ✓✓ —— 固定身份签名"重编后授权依然匹配"这条得到了最强的一次实证 |
| launchd | 上次退出码 **0**，无 `Address already in use` ✓ |
| 免认证面 | `/api/health` → `{"status":"ok"}`、`/api/setup` → `{"initialized":true,…}`、`/api/version` → `3.0.0-rc.2`；`/api/logs` 与 `/api/system/storage` 无 cookie 仍 **401** ✓ |
| SEC-21 真机 | `--path-as-is` 下 `//api/accounts`、`/api//accounts`、`/api%2faccounts`、`/api/./accounts` 全 **404**；`/api/accounts` **401**、`/` **200** ✓ |
| SEC-12 真机 | 已初始化实例上 `POST /api/setup` → **400**，且日志里出现 `WARN mcloud_server::routes::auth: 拒绝重复初始化 peer=127.0.0.1` ✓ —— **修复前认证失败是零日志的**，这条是"可观测性真的补上了"的实证 |
| 二进制自检 | 四批修复的串各命中 ≥1 次（按**字节数**统计；`grep -a` 在这份二进制上会漏报，见 AGENTS.md）✓ |

⚠ 上线后**没有**往 `:3000` 发任何会消耗限速预算的请求（没有登录、没有连发 `/api/session`）——
上表里那条 `POST /api/setup` 是**唯一**一次写请求，它走的是"已初始化 ⇒ 早退"那条路，
不跑 argon2、也不占登录预算 ✓。
