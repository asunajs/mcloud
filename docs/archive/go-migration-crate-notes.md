# 逐 crate 迁移要点（归档）

> 状态：**归档**。Rust → Go 时**按 crate 逐个**写下的迁移要点。
> 2026-09-22 从 [`../go-migration-plan.md`](../go-migration-plan.md) §6 拆出，**内容逐字未改**。
> 阶段已走完（Rust 侧 2026-09-21 冻结并剥离为同级仓库 mcloud-rust），
> 所以这部分从「方案」降级成「当时怎么想的」记录。
>
> ⚠ **它旁边那份不是归档**：同一节里的编号登记表（`#1 … #100`）是**活判据**，在
> [`../migration-pitfalls.md`](../migration-pitfalls.md)。本文里凡引用 `§6.1 #N` 处，
> 说的都是那张表；相对链接已按归档位置改成 `../`。

---

## 6. 逐 crate 迁移要点

### 6.0 先说清楚：**用 Go 写这个项目，不是用 Go 把 Rust 重写一遍**

这是一次**移植**，不是一次**转写**。两者的差别在于：移植的目标是把**行为**搬过来，
转写的目标是把**代码**搬过来。后者写出来的东西能编译、能过测试，但会带着一整套
在原语言里才成立的形状 —— 而那些形状在 Go 里没有理由存在，却会一直被当成约束。

> **判据：对齐只发生在边界上。**
>
> **必须**逐字节对齐的（这些才是"移植"的标的）：
> 线上字节、错误码、JSON 键名与枚举的文本形式、退出码、终端输出的形状、
> 时间戳的写法（连 `+00:00` / `Z` 这种都要一致，见 #16/#41）。
>
> **不许**为了"Rust 那边就是长这样"而保留的（这些是转写的产物）：
> 内部字段的整型宽度、结构体是值语义还是指针、有没有为某个 trait 造一个接口、
> 文件怎么切、一个函数是 9 个位置参数还是一张选项表。
> 这些一律按 **Go 的习惯**来 —— 只要边界上的行为不变。

⭐ **已经犯过一次的**（`smsSendResult` / `widenedRemain`，2026-09-21 已删）：
Rust 侧 PC 与 App 两条发码路径的余量字段分别是 `u32` 与 `i64` —— 那只是 serde
字段类型的**历史偶然**，没有任何语义。我把它当成必须保留的约束搬了过来，
然后在 CLI 层造了一个 `smsSendResult` 去抹平它，还配一个 `widenedRemain` 做转换。
一个计数在 Go 里就是带符号整型，两边长一样，那一整层桥接根本不该存在。

⚠ 这条同样管住**注释与提交说明**：写"Rust 侧是这样，所以这里也这样"是**危险信号**，
不是理由。要写就写"边界上必须一致的是 X"，或者"Go 里按 Y 写更好，因为 Z"。
溯源（"移植自 `legacy/…/login.rs`"）**可以**留一行，它是有用的考古信息；
但别让它承担论证。

### 6.1 `mcloud-core`（10,521 行）—— 风险最低，先做

密码学几乎全部落在 Go 标准库：

| Rust | Go |
| --- | --- |
| `md-5` `sha1` `sha2` `hmac` `aes` `cbc` `des` `rsa` | `crypto/*` **全覆盖** |
| `argon2` | `golang.org/x/crypto/argon2` |
| `base64` `hex` `flate2` | `encoding/{base64,hex}`、`compress/gzip` |

⚠ 四个坑：

1. **`Random` 的注入要照搬**。smid 构造混用了 `randomUUID` 与 `Math.random`，
   不注入则夹具回放与黄金向量对拍**无法复现**。`below()` 的**无偏采样**
   （拒绝采样）要逐位对拍，不能用 `% n`。
2. **argon2 的盐由 `Random` 注入**（为了钉住"同盐同哈希"），**别用库自带的随机源**。
3. `crypto/des` 是弱算法，安全类 linter 会报 —— 要**写明豁免理由**（它是上游协议的
   既有事实，不是我们的选择），而不是关掉 linter。
4. `run` 的四态结果 / 三类失败 / 需人工，Rust 用 enum 钉住；Go 只能
   `const iota` + 约定，`switch` 漏一个态**不再有编译错误**。
   ⇒ 这是 §9 里最该补装 linter 的一条。

**退出判据**：6 份黄金向量在 Go 侧逐字段复算通过。

⚠ **这句"6 份"写于动工前**（那时全仓只有 6 份向量）。收口时的实测口径是：
仓库里共 11 份，**core 读 8 份**（`crypto` / `pcclient-crypto` / `app-business-crypto` /
`password-hash` / `enum-json` / `auth` / `legacy-shape` / `traits-shape`），
其余 3 份的消费者在别的步（`datetime-format` → 步 3，`app-signin` 与
`login-crypto` → 步 2）。⇒ **别照"6"这个数验收**，照上面那串文件名验收。
（2026-09-21 步 1 收口时按"逐节都被正向引用"核过，见 §6.1 #56。）

### 6.2 `mcloud-upstream`（8,064 行）—— 向量兜底最厚

⚠ **本层的铁律是"跨端常量不许互搬"**（这条在 Rust 侧已经踩过两次）：

PC 与 App 是**两套客户端**，同名同形的常量值可能不同，而"两边实测都能过"
只说明服务端在这条路径上不校验，**不等于可以互换** —— 错了是**静默失效**，不报错。

已确认按「谁发的」×「哪个环境」分的常量（**Go 侧的常量表必须保留这两个维度，
不许扁平化**）：

| 位置 | 取值 |
| --- | --- |
| `Authorization` 第 1 段 | PC `pc` / App `mobile` |
| 业务签名密钥 | PC `6lXUhuJGkkAr5HXr` / App `BOFlEUINUdSugD6u` |
| 签名 `version` | PC 写死 `1.1` / App **必须用请求头里那个** |
| `hcy-cool-flag` 整包信封钥匙 | 原生 AES-192 / H5 默认 AES-256 / H5 非 `dev2Prod` AES-128 |

**判据（一句话）：这个码能在同一端的两个接口上取不同值吗？能，就不许钉进端档案。**
（`toSourceId` 就是这种 —— 同一个 PC 文件里有三个值。）

⚠ 另一个坑：`serde_json::Value` 的手取字段（`require_str` / `require_i64` /
`ensure_ok`）在 Go 里是 `map[string]any` 的链式取值，**漏判直接 panic**，
而 Rust 侧漏判是编译错误。
⇒ 必须先把这几个 helper 写成一等 API 并配对拍测试，**禁止在业务里裸取**。

**退出判据**：同一条抓包请求 Rust/Go **双跑**，header 顺序 / percent-encode /
签名串逐字节一致。

### 6.3 `mcloud-io`（8,697 行）—— 唯一 IO 层

| Rust | Go | 风险 |
| --- | --- | --- |
| `reqwest`（rustls + gzip/brotli/deflate + cookies） | `net/http` | 中 |
| `rusqlite`（`bundled`） | `modernc.org/sqlite`（纯 Go，免 cgo） | 中 |
| `rand`（`OsRng`） | `crypto/rand` | 低 |

⚠ 三个坑：

1. **Go 的 `net/http` 只自动解 gzip**，brotli 要自己接 transport。
2. **换 sqlite 驱动后要专门验一次 schema v1→v7 迁移**，以及"老库只读导入结果一致"。
   `chrono` 列类型要改成 `time.Time`。
3. **TLS 指纹**：rustls 与 Go `crypto/tls` 的握手行为不同（扩展顺序、JA3 形状）。
   若上游有指纹风控，这里会**静默**失败。⇒ 迁移后要拿真实流量验一次。

**退出判据**：纯 Go 驱动下 schema v1→v7 迁移 + 老库只读导入结果一致。

### 6.4 `mcloud-capabilities` / `mcloud-exec`

- `capabilities`（1,362 行）：能力注册表，CLI 与 WebUI 的**唯一契约来源**。
  两个产物各自描述一遍同一批能力，就会出现"CLI 有、WebUI 没有"这种只有人肉比对
  才发现的漂移 —— 所以它是**唯一**来源这条要保持。
- `exec`（1,074 行）：事件模型 / 生命周期 / 事件总线 / `run_events` 表。
  ⚠ 只有 1 个文件、1,074 行，但它是**编排的中枢**，别按行数低估。

### 6.5 `mcloud-app`（7,108 行）—— 组装层

签到全链路编排、登录驱动、账号档案、迁移策略。

⚠ 两条已经踩过的坑，迁 Go 时**要原样带过去**：

1. **`Authorization` 整串存、整串用**。形状是
   `Basic base64("<前缀>:<手机号>:<authToken>")`，登录时一次拿到整串。
   **别再"拆开存、用的时候拼回去"** —— 本地合成的串一律不许落库，
   理由不是洁癖：读侧优先采信整串，而整串路径**没有前缀可校验**，
   一串拼错的会被永久当原文用，现象只是"忽然回 `90001`"。
2. **成败只看回查 `infoV3`**。`startSignIn` 的 `todaySignIn` 是**调用前**快照；
   `code == 0` 也不够（"今天已签"同样回 `success`）。

### 6.6 `mcloud-cli`（3,817 行）

`clap` derive → `cobra` + `pflag`。

- ⚠ **零参数 + 下标**（用户明确要求）：不带子命令直接跑 = 对**全部启用**账号各签一次；
  账号用**下标**（`--account 2`），不是掩码 id。
- ⚠ 顶层必须收全套签到参数（Rust 侧用 `#[command(flatten)]` 解决）。
- ⚠ `--json` 时进度行**一律走 stderr**，否则 `| jq` 直接语法错。
  单账号给**对象**、多账号给**数组**。
- ✅ 好消息：参数互斥在 cobra 里是 `MarkFlagsMutuallyExclusive`，比 Rust 侧手写
  `conflicts_with_all` **更省事**。但"只读开关与写开关互斥"这条语义要照搬。

#### 2026-09-21 落地时的实测补充（追加）

- ⚠ **clap 的 `requires` 在 cobra 里没有对应物**：`MarkFlagsRequiredTogether` 是
  **对称**的（"两个都得给"），照它写会把 `--records` 也变成必填。
  只能手写 `validate()`，并且**必须** `fmt.Errorf("…: %w", ErrUsage)` ——
  否则退码从 2 掉到 1（用法错被报成运行错）。
- ⚠ **`ValueEnum` 的等价做法是把校验放进 `Set`**（自定义 `pflag.Value`）：
  这样失败发生在**解析期**，退 2；若顺手改成 `RunE` 里判，同一条错误退 1。
- `visible_alias` → 第二个 flag 绑**同一个变量**（`--h5-device-id` = `--device-id`），
  并配一条"给其一等于给另一个"的断言 —— 绑错变量不报错，只是别名静默失效。
- ⚠ **`--version` 的输出要整串比**：`HasPrefix("mcloud ")` 对 cobra 默认的
  `mcloud version 0.1.0` **同样成立**（反向测试实测照绿），所以只能钉 `== "mcloud 0.1.0\n"`。
- ⚠ **"还缺哪一层"要写成表而不是注释**：迁移期间 `internal/cli/unmigrated.go` 是一张
  叶子命令 → "缺 Rust 侧哪个文件、约多少行"的表，由 `UnmigratedBackends()` 导出、
  被测试要求"已注册的叶子要么有实现、要么在表里"。注释会被绕过，表不会。
  ⭐ **这张表已于 2026-09-21 退役**（`cli` 全部叶子落地，表自己的用例就写着
  "表空了就该整个删掉"）—— 连同 `notMigrated` 与它的两条对价用例一起删。
  ⚠ **退役时要补一条新的守卫**：表在的时候，"某个叶子又变回没实现"会被它挡住；
  表一走就没人管了。补的是 `TestEveryLeafActuallyRuns`（12 个叶子逐个跑，
  一个都不许出现占位错误）。**拆掉守卫的同一轮里要把替代品放上**，否则是净损失。
- ⚠ 引依赖这件事本身有一条判据：**别用 `@latest`**，见 §8 的补充。

### 6.7 `mcloud-server`（2,152 行）

`axum` + `tower-http`(fs) → **标准库 `net/http`**，静态用 `http.FileServer`。

> ⚠ **原文这里写的是 `net/http` + `chi`，与实际不符**（2026-09-21 更正）：
> Go 侧**没有引 `chi`**（`go.mod` 里没有），用的是标准库 `ServeMux` ——
> Go 1.22 起它自带「方法 + 通配」路由（`GET /api/runs/{id}/events`），
> `chi` 那一层在这里没有换来任何东西。⚠ 但**路由之外还有一处不能照搬**：
> axum 的 `fallback_service` 天生在中间件**外**，`ServeMux` 没有这个概念
> ⇒ 静态产物必须**先按路径分流**，否则首屏 401（见 §6.1 **#79**）。

> 2026-09-21 追加：**裸 `http.FileServer` 只是移植的第一步，今天在这条线上又加了一层**。
> ⚠ 先说清楚：这**不是**"基准有而我们漏了" —— 实测同一份 CSS 打在冻结基准的
> `mcloud-server`（8787）上：`content-length: 80213`，**没有** `Content-Encoding`，
> 也没有 `Cache-Control`。基准自己就是原样发出去的。改这里的理由是产物**实际**这么大：
> 80,213 字节 gzip 后 12,771（-84%），而差的**不是 tree shaking**（逐类名核过，181 个都在被引用）。
> ⇒ 现在 `internal/server/static.go` 是
> "分流 → 缓存头 → 按扩展名 gzip → `FileServer`"，仍只用标准库。
> ⚠ 两条约束记在这里免得被改回去：压缩**不能**提到整个 handler 外面
> （SSE 会被块缓冲攒住），而缓存/压缩的判据要按**实际送出的文件**而非请求路径
> （`/assets/` 走回退送的是首页）。实测数字与 10 条判据见 [`status.md`](../status.md) 服务端那一节。

⚠ `mcloud-server/tests/api_contract.rs` 的**契约测试要平移** —— 它是前端
（1,265 行 TS，**不动**）与后端之间的接口约定。

> ✅ **已平移**（2026-09-21）：`internal/server/server_test.go` 的 26 条用例逐项覆盖了
> 那份契约（初始化闸门 / 坏请求体的错误形状 / cookie 三个属性 / 口令错与用户不存在
> 给同一个答案 / 会话必须带 cookie / 登出立刻生效，含 Rust 侧那两个**状态码**
> `POST /api/setup → 201`、`DELETE /api/session → 204`）。入口层另加 17 条
> （`static_test.go` + `run_test.go`），包内共 **43 条**。

---
