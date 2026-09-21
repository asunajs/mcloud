# 实现进度与现状须知

> 状态：**实况**（这里写的是"现在能跑什么、还差什么"，不是计划）
>
> ⚠ **2026-09-20：Rust 侧已冻结到 `legacy/`，本仓库正在迁往 Go。**  
> 下面这张表描述的是**冻结那一刻**的 Rust 实现 —— 它仍然有效、也仍然是对拍基准，  
> 但"进度"从此按 Go 侧算。迁移方案与顺序见 [`go-migration-plan.md`](./go-migration-plan.md)。  
> Go 侧进度见下方「Go 侧迁移实况」一节。
>
> - 最后核对：2026-09-19 收工（Rust 侧）；Go 侧 2026-09-20 起
> - 规划（**写于动工前**，进度不要以它为准）：[`rust-rewrite-plan.md`](./rust-rewrite-plan.md)
> - 实现顺序（先做哪项能力）：同文档 §7.1
> - 门禁怎么跑：[`dev-gates.md`](./dev-gates.md)

---


## Go 侧迁移实况（2026-09-20 起）

**当前：步 1（`mcloud-core`）✅ 收口（2026-09-21，判据与一条例外见下方"步 1 收口"）；**&#x6B65; 2（`mcloud-upstream`）已整族落&#x5730;**（`login` / `client` / `app_client` / `app_login` / `app_signin` 全族 / `smid`，150 条测试）；**&#x6B65; 3（`mcloud-io`）已整族落地\*\*（schema / store / store::user / store::config / datetime / **legacy_import** / fixture / HTTP 客户端 / 随机源，150 条测试）；步 4 已落地 `capabilities`（注册表 + `login`）、`exec`、`app::payload` / `smid` / `session` / **`signin/*` 整族**。

⇒ **`app::migration` / `cli` / `server` 的阻塞已解除**（此前全被步 3 挡着）。\**
⇒ **`server` 已整包落地并接上入口**（2026-09-21）：HTTP 层 26 条测试，入口层
   （参数面 / 退出码 / 静态资源与 SPA 回退 / 真监听起服务）再 **17 条**，
   共 **43 条**。`cmd/mcloud-server` 不再打印占位退 2。
⇒ **`app` 层已全部落地**（2026-09-21）：最后两块 `login`（扫码与短信的驱动，14 条）
   与 `app_login`（App 端，13 条）已补上。
⇒ ⭐ **`cli` 全部叶子落地**（2026-09-21，**116 条测试**）—— 那张「缺口表」
   （`internal/cli/unmigrated.go`，登记"哪条叶子还没接后端、缺 Rust 侧哪个文件"）
   **已随最后一条叶子一起退役**：它自己的用例就写着"表空了就该整个删掉"。
   位置由 `TestEveryLeafActuallyRuns` 守着（12 个叶子逐个跑，一个都不许出现占位错误）。

> ✅ **2026-09-21 起并发结束**，全部由本会话接手（此前最多三个会话在同一棵树上）。  
> 已付出的可见代价：`docs/go-migration-plan.md` 的 §6.1 一度出现**两组 `#20`**，  
> 现已统一为**全局连续 + 追加式**（步 1 = 1–27，步 4 = 28–39 与 42–45，步 3 = 40–41 与 46–47）。  
> ⚠ 动 `docs/`、共享脚本、门禁白名单前**先看 `git status`**：  
> 白名单条目若指向**未提交**的文件，干净检出会让 `jsontags` 门禁变红 ——  
> 条目与文件必须**同进同出**（**两个方向都算**：文件先落地而条目没跟上，同样会让 HEAD 变红）。

> 步 4 的落点见下方「步 4 实况」一节 —— 它**没有**按 `capabilities → exec → app → server/cli`  
> 一口气做完：`capabilities::login` 与 `app` 的绝大多数模块**依赖步 2（`mcloud-upstream`）与  
> 步 3（`mcloud-io`）**，那两步还没迁，所以先落了不依赖它们的三块；  
> 之后为了解开 `capabilities::login`，又**回头补了步 2 的 `upstream::login` 那一块**  
> （只补到够用为止，步 2 的 `signin` 协议族仍未迁）。

| `mcloud-core` 的模块 | Go 侧 | 说明                                                                                                                                          |
| ----------------- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `crypto`          | ✅    | 哈希 / AES(CBC,ECB) / DES / RSA / gzip / 签名头 / getTn                                                                                          |
| `error`           | ✅ 前半 | ErrorKind / Error / classify_status / classify_business_code；**后半的 `classify_body` 那台"响应体分类机"未迁** ⇒ 2026-09-21 定案：不补进 core，见下方「步 1 收口」第 2 条 |
| `auth`            | ✅    | 含全部原版怪癖（只去一处 `Basic ` 前缀、token 在 `:` 处截断、`Number() \|\| 0`）                                                                                 |
| `pure`            | ✅    | 日键 / 增量 / 开抢时刻 / 脱敏                                                                                                                         |
| `cron`            | ✅    | 5/6 段分流、日与周 OR、周日 `0`/`7` 归一化、时区为参数、4 年上界返回 false                                                                                           |
| `sanitize`        | ✅    | header 白名单 / JSON 递归脱敏 / token 形状打码 / 199 号段重映射 / `scan_for_leaks` 自检 / `build_fixture_entry`                                               |
| `user`            | ✅    | 口令哈希（argon2id PHC 串）/ 会话 token / 数据隔离判据；**带来了第七份黄金向量**                                                                                      |
| `run`             | ✅    | RunID / 四态结论 / 需人工 / Seq 语义 / 五类事件流；**顺带补上了枚举的 JSON 编解码（见下）**                                                                               |
| `model`           | ✅    | Account / RunRecord / 能力注册表 7 个枚举 / AccountState / SmidState / Schedule / Pipeline / Exchange / Webui                                       |
| `stage`           | ✅    | 三阶段存储生命周期 / 四类迁移错误 / RunExecution；**`RunStage` 是 tag 枚举，形状由 serde 产出后对拍**                                                                   |
| `legacy`          | ✅    | 旧文档 14 个类型 / 规整规则 / 账号视图 / 引用解析；**带来了第九份黄金向量（形状）**                                                                                          |
| `traits`          | ✅    | HTTP 请求/响应形状 / Clock / Scope / ClaimedRows / 三个 store 接口；**带来了第十份黄金向量（形状）**                                                                 |

- 落点 `internal/core`，**270 条测试**；读 `test/golden/*.json` 经 `internal/golden`，  
  读 `test/fixtures/legacy/*.json` 经 `internal/golden.LoadFixture`，都不复制。  
  ⚠ **全项目的条数不在这里维护**（抄一次就 stale 一次）：  
  `grep -rho '^func Test' internal/*/*_test.go | wc -l` 现算。

#### 步 1 收口（2026-09-21 实测）

判据是 §7 那行"黄金向量在 Go 侧逐字段复算通过"。逐条核过：

1. **模块对位：17/17。** Rust 侧 `mcloud-core/src` 的 17 个源文件（含 `lib.rs`、  
   `test_support.rs`）在 `internal/core` 全有落点。
2. **公开 API 对位：164 个 `pub` 项里 156 个按名字对上**（比对要先做  
   `snake_case`→`PascalCase` 归一，否则整表都是假阳性 —— 我第一版就这么误报过一批），  
   剩下 8 个：4 个只是改名（`AES_FIXED_IV` / `DEFAULT_EXCHANGE_WINDOWS` /  
   `SIGN_SALT_MAP` / `WEBUI_PAGES`，Go 侧逐一对到了实现），  
   另 4 个是同一件事 —— **`classify_body` 及其 `Classified` / `FailureClass` / `body_snippet`**。  
   ⇒ **定案：不补进 core。** 依据不是"还没轮到它"，而是**冻结基准里它就没有消费者**：  
   `grep -rn "classify_body\|Classified" legacy/crates` 除 `mcloud-core` 自身  
   （定义处 + 自己的单测 + `lib.rs` 再导出）外**零命中**。  
   而 Go 侧判据的三块已经各在其位：HTTP 状态归 `core.ClassifyStatus`  
   （`internal/io/client.go` 就在用），信封归 `upstream.Envelope`  
   （`Success` 先于 `Code` 那条顺序也在那儿）。  
   **补进去就是一台没人开的分类机**，正是本文件反复警告的"写下来就没人再看"。  
   ⇒ 归属改挂到**第一个真需要"状态 + 响应体一步判完"的调用点**（步 4 的 `exec` / `server`  
   错误映射），届时按 Go 的形状写，而不是照 Rust 的签名平移。
3. **向量：core 读 8 份，且逐节都有正向断言** —— 这次把最后一处补上了：  
   `app-business-crypto-vectors.json` 的 `hcyCoolFlagEnvelope`（发码链路那把 AES-128，  
   3 条）此前**只被当过反例**（`…192IsNotThe128BitKey` 用它证明"128 那把解不开 192 族"），  
   **从没正向解过一次**。新增 `TestAppCoolFlagSmsLinkEnvelope128BothDirections`  
   补两个方向：2 条实拍响应走解密、1 条合成请求走**加密**（固定 IV ⇒ 逐字节可比）。  
   ⚠ **反例不能代替正例**：拿错钥匙有 1/256 的概率碰上合法 PKCS7，  
   那时"解不出正确明文"这条在两边都成立。  
   ⚠ 反向测试已做：把钥匙换成 `BODY_AES_KEY_192`，两个方向**同时变红**。
4. **§6.1 那四个坑逐条核过**：① `Below()` 是拒绝采样且取值序列被钉住  
   （`crypto_test.go`，注释里写明了"别简化成 `% n`"）；② argon2 的盐走注入的  
   `Random`（`HashPassword(plain, rng)`）；③ DES 的豁免理由在 `crypto.go` 文件头  
   （"协议层必须能用…设备指纹（DES/RSA）"+ 三条坑里的补零口径）；  
   ④ 四态穷尽用 `countKnownStates` 真值表守卫兜住（`run_test.go`，  
   判据是"`String()` 对越界值返回空串"）。
5. **⚠ 步 1 的判据里从来不该含 `login-crypto-vectors.json`**：它覆盖的是  
   `puzzleAnswer` / `aesEcb*` / `sha1Password` / `md5SafeVerify` / `guid32` ——  
   落点在 `internal/upstream/login.go`，**属步 2**。而且它并非"完全无断言"：  
   `puzzleAnswer` / `aesEcb*` / `sha1Password` 三族**行为两侧都已正向钉过**  
   （期望值取自 `pcclient-crypto-vectors.json` 那一份，不是没测，是没接**这一份**）；  
   `md5SafeVerify` 是**两侧都没实现**的 TS 路径（Rust 侧 `login.rs` 记着  
   "`safeVerifyCodeEncrypt` 暂未实现：实测没走到那条分支"）⇒ 先定要不要实现，别补断言；  
   `§guid32` 的规则**向量里给了**（生成器 `getGUID32` = `MD5(UTF8(uuid))` 取大写），  
   但**两侧都刻意不复刻**它（Go 与冻结的 Rust 都是 `hex(16 个随机字节)`，
   上游分辨不出两者）⇒ 钉**形态**即可。逐节表见 `go-migration-plan.md` §6.1 末尾。  
   ⇒ 下面那条"尚未达成"就是这次复核要更正的。  
   ✅ **2026-09-21 已接完**：消费者 = `internal/upstream/login_crypto_vectors_test.go`  
   （7 条）+ `internal/app/device_test.go`（1 条），门禁那行 `⚠ 死向量` 已转 ✓，
   反向测试 6/6 如期变红（`target/revtest-login-vectors.py`）。

- ⚠ **步 1 只剩 `error` 后半的 `classify_body` ~400 行**（那台"响应体分类机"）。  
  它之所以一直排在最后：写出来之后**没有消费者**（用到它的层还没迁），  
  而"写下来就没人再看"的代码正是这个项目反复吃亏的地方。  
  ⇒ **2026-09-21 复核更正**：不只是"用到它的层还没迁"——**冻结基准里它也没有调用点**  
  （见上方收口第 2 条），所以这条判据成立的方式是"两边都没人用"。  
  结论从"步 1 的尾巴"改为"挂到步 4 的第一个真实调用点"。
- 门禁**九条**（新增 `jsontags`）—— 见 `docs/dev-gates.md`。
- `auth-vectors.json` 从"只有 Rust 读"变成**两边都读**。
- 步 1 的退出判据是"黄金向量在 Go 侧逐字段复算通过"，**尚未达成**：  
  `app-signin-vectors` 的消费者在步 2 接线，而 `login-crypto-vectors.json`  
  至今仍是**死向量**（有生成器 `scripts/gen-login-crypto-vectors.cjs`、无消费者），  
  且它的 `§guid32` 连拼接规则都没在向量里给出 —— 详见 `go-migration-plan.md` §6.1 末尾。  
  ⇒ **2026-09-21 复核：这句话把两步的账混在了一个判据里，据此判"步 1 未达成"是错的。**  
  `app-signin-vectors` 与 `login-crypto-vectors` 的消费者都在**步 2**（`internal/upstream`），  
  与 core 无关；core 读的 8 份**逐节都有正向断言**（最后一处 `hcyCoolFlagEnvelope`  
  这次补上，见收口第 3 条）。⇒ **步 1 判据达成**，例外只有收口第 2 条那一台分类机。  
  ✅ 补记（同日）：`login-crypto-vectors.json` 的消费者已落地，**`§guid32` 的规则也在** ——  
  原文"连拼接规则都没在向量里给出"是看漏了生成器。见 `go-migration-plan.md` §6.1 末尾。
- ⚠⚠ **别照门禁那行"✓ 有测试读取"判定进度**：它的消费者探测扫**四个落点**  
  （`crates/` / `legacy/crates/` / `internal/` / `cmd/`），命中任一即算有消费者，  
  **不区分是哪一侧在读**。所以 `app-signin-vectors.json` 至今显示 ✓，  
  而读它的**只有已冻结的 Rust 侧**。  
  Go 侧实际读了 **11 份**（= `test/golden/` 的全部）：`crypto` / `pcclient-crypto` /  
  `app-business-crypto` / `auth` / `password-hash` / `enum-json` / `legacy-shape` /  
  `traits-shape` / `datetime-format` / `app-signin` / `login-crypto`  
  （见 `internal/core/*_test.go`、`internal/io/*_test.go`、`internal/upstream/*_test.go`  
  与 `internal/app/device_test.go` 里的 `golden.Load`）。  
  ⇒ **2026-09-21 起，"✓ 有测试读取"与"Go 侧读了"这两个集合已经重合** ——  
  但这是**当前**的事实，不是这条判据的性质（它仍不区分哪一侧）。
- ⚠ `user` 是第一个引入**第三方依赖**（`golang.org/x/crypto`，只为 argon2）的模块。  
  它只给**裸 KDF**，PHC 串的编解码要自己写；且 `v=16` 与 `$argon2d$` 两条  
  Go 侧**物理上算不出来**，已在向量的 `$crossLanguageGaps` 里登记为  
  "Rust 是 mismatch、Go 是 error"（详见 `go-migration-plan.md` §6.1）。
- ⚠ `cron` 与 `sanitize` **没有黄金向量** —— 它们的规格来自 Rust 侧的单元测试  
  （`sanitize` 的 5 个测试文件共 348 行），迁移时是**逐条平移**的，不是照文档重写。
- ⚠⚠ **枚举的 JSON 形状是 `run` 这一步顺手补上的一个既有洞**，不是新需求：  
  Rust 侧每个进 JSON 的枚举都带 `#[serde(rename_all = …)]`，序列化成**文本**；  
  而 Go 的 `type X int` 默认序列化成**数字**。两边读同一批 `run_events.payload`  
  与同一张 `role` 列 ⇒ 同一个字段有两个形状，且**不报错**。  
  `user` 那一步的 `Role` / `SessionKind` 就已经是这个状态（它的测试只比 `String()`，  
  所以没露出来）。现在收进 `internal/core/enum.go` 一张名字表，  
  并由第八份黄金向量 `enum-json-vectors.json`（**serde 产出**）逐条对拍。
- ⚠ **`stage` 这一步新钉住一条**：tag 枚举（`#[serde(tag = "stage")]`）的  
  **不同变体字段顺序不同** —— `Done` 是 `outcome→manual_steps→summary`，  
  `Failed` 是 `summary→manual_steps`。单个结构体做不到，  
  只能按 Kind 分派 `MarshalJSON`；而**逐字段比对会把这个差异整个放过**，  
  只有整串比对抓得到。
- ⚠⚠ **`legacy` 是最容易"静默读空"的一层**：旧文档的所有字段都带  
  `#[serde(default)]` ⇒ **字段名写错不报错，只静默读到零值** —— 表现是  
  "历史数据读出来是空的"，没有任何线索指向字段名。  
  而 Rust 侧对这批形状**一个断言都没有**。⇒ 新增第九份向量  
  `legacy-shape-vectors.json`，且它的生成器 `scripts/gen-legacy-shape-vectors.rs`  
  是 **path 依赖真实的 mcloud-core**（不是镜像类型定义）—— 镜像会抄错，  
  `ScheduleTarget` 那次已经证明过一次。  
  向量两节：`$serializedShapes`（8 个可序列化类型的空/填充整串）  
  与 `$fixtureKeys`（夹具里实际出现的键名，用反射读 tag 逐条认领）。
- ⚠⚠ **`traits` 是"两侧都跑得通、但写出来的不是同一份东西"最典型的一层**：  
  `HttpRequest` 的注释写着"golden request 对拍依赖它逐字段可比"，  
  而 Rust 侧对它**只断言过一个枚举的一个变体**。第十份向量  
  `traits-shape-vectors.json`（同样 path 依赖真实 mcloud-core）实测出四条  
  Go 侧会**静默接受**的坏形状：`body` 的 base64 串、`headers: null`、  
  缺席的 `method` / `url` / `status`、1 元组的 header。  
  其中 `body` 那条最要命 —— `encoding/json` 对 `[]byte` 有**专门的** base64 分支，  
  于是两侧各自序列化、各自解析都成功，只有两边文件逐字节 diff 才看得见。  
  详见 `go-migration-plan.md` §6.1 #23–#27。
- ⚠ **测试夹具的 RNG 必须逐模块对齐 Rust 侧各自那一份，不能就近复用**：  
  `run.rs` 与 `stage.rs` 各自定义了 `SeededRng`，算法不同  
  （前者是 `s*31+7` 的 u8 LCG，**周期恰好 16 —— 正是 UUID 的字节数**）。  
  `stage.rs` 还专门用一个进程内计数器把种子岔开，并在注释里写明了原因。  
  迁移时我照样踩了（就近复用了 `run_test.go` 那份），详见 §6.1 #19。
- ⚠⚠ **同一个病还有第二个面：结构体字段名**。Rust 的 serde 默认 snake_case，  
  Go 不带 tag 输出字段名原样 ⇒ `password_hash` vs `PasswordHash`。  
  这个版本已修三处：`CloudStats`（pure 带过来的）、`User` + `Session`（user 带过来的）、  
  `model` 那批。⚠ **往返测试救不了它**（用什么字段名都是绿的），  
  所以新增了第九条门禁 `scripts/check-go-jsontags.cjs`：  
  混合 tag 直接失败、全无 tag 必须进白名单并写明理由。

### 步 3 实况（2026-09-20 起）

**这是全项目唯一的 IO 层**（8,697 行），也是 `app` / `cli` / `server` 的阻塞点。

| `mcloud-io` 的模块                 | Go 侧 | 说明                                                                                                                                                                                    |
| ------------------------------- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `store::schema`                 | ✅    | schema v1→v8 的 8 条幂等迁移链；**18 条测试**逐级验证                                                                                                                                                |
| `store`                         | ✅    | `StateStore` 全套（账号 / 登录态 / smid / 云朵快照 / 运行记录 / 事件流）                                                                                                                                  |
| `store::user`                   | ✅    | `UserStore` + `IssueSession`（口令 / 会话 / 清理过期）                                                                                                                                          |
| `datetime`（新增）                  | ✅    | 与 chrono **逐字节一致**的时间写法，见下                                                                                                                                                            |
| `store::config`                 | ✅    | 定时任务 / 任务组合 / 抢兑配置 / 界面偏好 / 导入台账（14 个方法）；**JSON 列存原文，只有 `enabled` / `cron` / `last_status` 单独成列**                                                                                     |
| `legacy_import`                 | ✅    | **"老库只读导入结果一致"这条退出判据的载体**（~1,600 行 + 1,163 行测试）：账号清单 → 身份映射 → 6 个数据文件 → 凭据回填；`asign.json` + 6 文件原样归档（**21 条测试**）。五条硬判据见下方「`legacy_import` 的五条硬判据」                                     |
| `random` + `SystemClock`        | ✅    | 生产随机源（`crypto/rand`，取不到熵就 panic 而不是退化成弱随机）与真实时钟（归一化 UTC）；**3 条测试**。放 io 是因为取熵 / 读时钟都是 IO                                                                                              |
| `reqwest_client`（→ `client.go`） | ✅    | 重试策略（指数退避 + 抖动，硬上限）+ 真实 HTTP 客户端；**17 条测试**。⭐ 两处刻意不同于 Rust 侧：**不声明 brotli**（标准库解不开，声明了会把响应变乱码）、**关掉 `net/http` 自动解压**由自己管 `Accept-Encoding`                                           |
| `fixture`                       | ✅    | L2 夹具回放：按 `METHOD host/path`（**丢 query**）匹配、多条按顺序消费且用尽保留最后一条、`body_json` 优先、**未命中即报错不回退真网**、`AuditLeaks` 自检（请求段也要扫）；`BuildEntry` 从 `core.BuildFixtureEntry` 收口，录制与回放同一份形状（**16 条测试**） |

- **驱动 = `modernc.org/sqlite` v1.59.0（纯 Go，免 cgo）**，方案 §8 早已拍板。  
  实测它吃下了带 `BEGIN/COMMIT` 的**多语句**迁移 —— 这一点不显然  
  （`Exec` 多语句是驱动相关的），所以先写 schema 测试再往下走。
- 落点 `internal/io`（**包名是 `mcloudio`**，目录名与包名刻意不同）；**150 条测试**。  
  `Client` 现在是**真的** HTTP 客户端了（此前只是骨架），`FixtureClient` 也已能  
  加载并回放抓包夹具并有消费者（见下方「夹具回放」）。
- ⭐ **`mcloud-io` 至此整族落地**，步 3 完成 ⇒ `app::migration` / `cli` / `server`  
  的阻塞解除。

### `legacy_import` 的五条硬判据（都做了反向测试，实测变红）

| 判据                      | 为什么                                                                                       |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| **掩码冲突 → 中止，一行都不写**     | 两个手机号前 2 后 2 相同时会塌缩成同一个 `AccountID`，历史数据混在一起且**不报错**。代价必须是"迁移没做"，不是"迁移做错了"                |
| **缺文件 → 报出来，其余照导**      | 老库本来就可能少一个文件；整体失败会让"少一个文件"变成"什么都导不进来"                                                     |
| **整份 JSON 坏 → 大声失败**    | 静默当空对象继续，等于把"缺了 12 条历史"报成"导入成功"                                                           |
| **一条记录坏 → 只丢自己**        | 一条 `ok: 1`（类型错）不该带走同一份文件里另外 3 条好的                                                         |
| **凭据回填**不受哈希门控，**按状态判** | 老库的 `state.json` 早就导过了，走哈希门控就永远不会补上 —— 于是同一个版本的程序会产出**两种库形态**（老库 NULL、新库有值），而这两种在别处长得分毫不差 |

⚠ `Authorization` 是**整串**原样落库（`Basic …`，铁律 2）—— 任何"拆开再拼回去"  
的写法都会被那条整串断言抓到。

- ⚠⚠ **时间列必须与 chrono 逐字节一致**（`internal/io/datetime.go`）：  
  `sessions.expires_at` / `users.created_at` / `runs.started_at` 都在 SQL 里做  
  **字符串比较与排序**，而 chrono 写 `+00:00` + AutoSi（0/3/6/9 位）、  
  Go 的 `RFC3339Nano` 写 `Z` + 去尾零。混着的后果是 `+`(0x2B) < `Z`(0x5A)  
  让**同一时刻的 Rust 行永远排在 Go 行之前** —— 不报错，只在比较与排序时显形。  
  规则由第十一份向量 `datetime-format-vectors.json` 钉（**chrono 0.4.45 真跑**：  
  17 条写出 + 19 条读取），详见 `go-migration-plan.md` §6.1 #16 / #41。
- ⚠ 读取侧 Go **严格更严**：实测四种 chrono 收而 Go 拒（空格代替 `T`、  
  小写 `t`/`z`、闰秒 `12:00:60`）。方向安全，且这四种 chrono 自己从不写出来 ——  
  逐条登记在 `chronoAcceptsGoRejects` 里**待复查**（名单里出现不了的条目会判失败）。
- ⚠ **照抄 SQL 时注释里的反引号要全部去掉**：Go 的原始字符串用反引号作定界符，  
  `schema.rs` 里 SQLite 标识符引用（`` `sha256` `` / `` `raw` ``）会把字面量**当场截断**，  
  而编译器报的错指不到"注释里有个反引号"（§6.1 #40）。
- ⚠ **Go 的 `?` 是顺序绑定，Rust 的 rusqlite 支持 `?1`/`?2` 编号**：  
  Rust 侧一条查询里同一个值可以复用（`(?1 IS NULL OR owner = ?1)`），  
  Go 必须**重复传参**（`ownerArg(scope), ownerArg(scope)`）。照抄 SQL 时这里会静默差一个参数 ——  
  好在它**会在运行期报** `missing argument with index 2`（反向测试已确认），不是静默错。
- ⚠⚠ **`PipelineStep` 是唯一"嵌进 JSON 再读回来"的 core 类型**（`pipelines.steps` 列），  
  而它的 `body` 是 `json.RawMessage` —— 这个类型**既不 raw、也不 null 保真**（§6.1 #46）：  
  ① 写的时候 `json.Marshal` 会对内容**再跑一遍 HTML 转义**，必须走 `marshalJSON`；  
  ② 读的时候 `null` 被存成**字面的四个字节**，往返后 `Body == nil` 反转，  
  而**再序列化出来还是 `null`** ⇒ **逐字节比对看不见**，只有 `== nil` 会变。  
  向量 `$pipelineStepEncode` / `$pipelineStepDecode`（serde 真跑）钉住形状与宽严。
- ⚠⚠ **参考实现里"没有"的函数，新加一个就是新契约**（§6.1 #47）：  
  `ParseScheduleStatus` / `ParsePipelineStatus` 是迁 `model` 时自己加的，  
  我按"未知归 `error`"写；而参考实现的做法在 **io 层**（`schedule_status_from_str`）——  
  未知返回 `None`，**刚好相反**。且当时测试是绿的（照着实现写的）。  
  已改成返回 `(状态, bool)`。

### 步 4 实况（2026-09-20 起）

| 模块                     | Go 侧       | 说明                                                                                                                                                                                                                                                                                                                                                                                                 |
| ---------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `capabilities` 注册表     | ✅          | `Capability` / `REGISTRY` / `get` / `list_visible` / `list_batchable`（**10 条测试**）。对应 `capabilities/src/lib.rs`                                                                                                                                                                                                                                                                                     |
| `capabilities::login`  | ✅          | 扫码状态机（推进 / 终态不可逆 / 自身 TTL / 码的映射 / 限流非终态）+ 短信链路（滑块降级 / 配额 / 登录）+ 登录结果 → `auth` 与领域对象（**22 条测试**）。前置的 `upstream::login` 已先落地                                                                                                                                                                                                                                                                        |
| `exec`                 | ✅          | 并发闸 / 互斥键 / 参数摘要 / 准入（幂等 + 互斥一起判）/ 事件总线（**35 条测试**）。对应 `mcloud-exec/src/lib.rs`                                                                                                                                                                                                                                                                                                                    |
| `app::payload`         | ✅          | `/api/capabilities` 与 `/api/version` 的响应体（**7 条测试**）。纯：只依赖 `capabilities` + `core`                                                                                                                                                                                                                                                                                                                 |
| `app::smid`            | ✅          | 生命周期那一层：东八区 14 位时间（跨 UTC 午夜那条已钉）/ `UploadContext` / 绑定与复用（复用路径**一次写都不发**）/ 主动换特征 = 旧 id 作废 / `DeviceID`（库里有就**不打网络**）与 `ReportDeviceID`（每次都真上报）/ 落库失败就丢掉 id 不返回（**12 条测试**）。库里那串读不出来是**报错**不是重掷                                                                                                                                                                                                    |
| `app::session`         | ✅          | **网络路径**的会话（`332` 行）：cookie 里是明文 token、库里只有 `sha256(token)`；用户不存在与口令错误给**同一个**结论（否则登录页变成用户名枚举器）；没带 cookie / 已过期 / 已撤销三种都导出"去登录"；撤销**立刻生效**且幂等；空 token 决不能被当成一次登录（**8 条测试**）。不碰 HTTP（cookie 与状态码在 `server`）                                                                                                                                                                                         |
| `app::signin/*`        | ✅ **整族落地** | 签到业务层（9 个文件）：`UserTimestamp`（**微秒**不是毫秒）/ 建会话**三步不夹别的 IO** / **已签就不发** `startSignIn` / 成败只看**回查 `infoV3`**（`startSignIn` 回的是调用前快照）/ `PlanReceive` 按 `cloudType` 三路分流 / `ReceiveAll` 一条失败不拖垮其余 / 翻倍与备份好礼的**门槛判定**（门槛不成立一个请求都不发）（**18 条测试**）                                                                                                                                                         |
| `app::device`          | ✅          | 设备指纹的本地持久化：PC 端 32 位大写 HEX + `-ENDIN` / App 端纯 32 位 / `deviceShort` 12 位**不落盘** / 空白文件算"没有" / 原子写（**9 条测试**）                                                                                                                                                                                                                                                                                       |
| `app::user`            | ✅          | 首个管理员的初始化：只有一条路径（CLI 与 WebUI 共用一次 bootstrap）、**只负责从零到一**、口令下限在写库**之前**判、认领历史数据失败**不能当什么都没发生**（**7 条测试**）                                                                                                                                                                                                                                                                                           |
| `app::cloud`           | ✅          | 云朵余额采集：历史窗口取**全量**（近 7 天会静默少算）、同一天重复采集**不以今天为基准**、先写日快照再写当前值（**9 条测试**）                                                                                                                                                                                                                                                                                                                            |
| `app::migration`       | ✅          | 迁移的**执行策略**（`483` 行）：先归档 **再建库** 后导入、预演走 `DiagnoseLegacy`（内存库上同一条路径）、**库里一个用户都没有时落到 `Scope::Local` 而不是报错**（CLI 那条"先 migrate 再 init"全靠它）、`has_legacy_data` 只要求**任一**文件存在、渲染放在这一层不放在 CLI（**11 条测试**，全用真实 SQLite + `test/fixtures/legacy/` 的真实脱敏夹具）                                                                                                                                                   |
| `app::run`             | ✅          | 跨账号编排（`748` 行）：受理 → 记 `Running` 行 → 跑 → 写终态，**锁在 defer 里释放**（任务失败也放）；单账号失败**不整批停**（只记自己那一行）；**需人工 > 普通失败**（且它在历史里是 `Done` 不是 `Failed` —— 它确实跑完了，只是没达成目标）；被挡下的账号记下**是谁占着**；进度是**整批计数**（第 `index` 个账号的流里写 `index+1 / N`）（**10 条测试**）。⭐ "先记 Running"这条顺序只有**在任务进行中往里看**才测得出来（跑完之后与"直接写终态"长得一模一样，见方案 §6.1 #63）                                                                                         |
| `app::login`           | ✅          | 扫码与短信的**驱动**（Rust 侧 828 行，**14 条测试**）：出码 → 轮询 → 成功后落库；**限频（200059545）不是终态**（退避 30 秒后继续问，把用户赶回去重开一张码只会更深地撞进风控）；超时单独归 `Timeout`；摘要**不得**带出完整手机号。⭐ 等待做成注入项 `Waiter`（Rust 靠 tokio `start_paused` 虚拟化定时器，Go 没有对应物，见方案 §6.1 #72） |
| `app::app_login`       | ✅          | App 端验证码登录（Rust 侧 910 行，**13 条测试**）：发码**全程不开浏览器**（取第一方题图 → 用公开常量解出坐标 → 带答案重发，解回来必须是 370）、用尽才把 H5 地址交给用户；登录体是**密文**（能解回原样）、`9103` 归 `Internal` 而不是 `BadRequest`（那是**我们**的加密参数不对）；落库前缀是 `mobile` 不是 `pc`。⭐ 用户域回填**不得**改调用方那份档案（Rust 的值语义在 Go 里变成原地修改，见方案 §6.1 #73）                                                                                                                  |
| `upstream::login`      | ✅          | 为使 `capabilities::login` 能落地而**提前做的前置**：信封 / 端档案 / 请求体 / 滑块 / 口令哈希 / 判定（**33 条测试**）。步 2 的其余部分（`signin` 协议族）仍未迁                                                                                                                                                                                                                                                                                     |
| `upstream::app_client` | ✅          | App 端（iOS）档案：16 段 `x-yun-client-info` 与 14 段 `x-DeviceInfo` **不同值**、三种 `os_version` 写法、`mobile` 前缀的 `Authorization`、未登录占位头（**10 条测试**）。它是整个签到链路的前置                                                                                                                                                                                                                                                 |
| `upstream::app_login`  | ✅          | App 端验证码登录（`632` 行）：三层封装（明文 JSON → AES-128-ECB hex → **末 10 位错位**）、`hcy-cool-flag` **主动不发**、滑块 H5 的 `data` 只有两个头、`9441`/`9103`/`9999` 三态判定（**18 条测试**）。中心断言 `appLoginWire` 取自 Rust 侧 `crypto/tests/app.rs`（抓包侧独立脚本算出），Go 复算同一串 ⇒ 真跨实现对拍                                                                                                                                                            |
| `upstream::smid`       | ✅          | 设备指纹（`733` 行）：`smid` 拼接 / `priId` / `tn`（**`tn` 还是空串时算摘要**）/ 25 条 DES 规则整表对拍 TS 侧向量 / 特征体绑定账号（`SmFingerprint`）与每次现算的一半（`SmUpload`）/ 上报与 `deviceId`（拼 `B` 前缀）（**18 条测试**）。⭐ `ep` **复现不了**是算法性质（PKCS#1 v1.5 随机填充 + 盲化），不是随机源没注入到位 —— 断言显式排除它并另开一条钉住                                                                                                                                                  |
| `upstream::app_signin` | ✅ **整族落地** | 常量（由 `app-signin-vectors` 的 `constants` 钉住）/ `Diagnose` / H5 与原生头 / 百分号编码 / 取值 helper + 建会话三段 + 签到与状态（`infoV3` / `startSignIn` / `taskListV3` / `doTaskPost`）+ 领取族（`receiveV3` / `receiveRevivalReward` / `multiple` / `receiveTaskExpansion`）+ 记录族（`cloudRecordV2` / `record` / `backupgift/*`）+ **发送层**（`Upstream` 上的 15 个方法）（**113 条测试，整包合计**）。`app-signin-vectors.json` 从"只有 Rust 读"变成**两边都读** |
| `server`               | ✅          | HTTP 层（`1,481` 行 Rust）：**领域错误 → HTTP 的映射**（`statusAndCode` 用兜底而不是逐个列，新增 `ErrorKind` 默认落 500 而不是让人挑一个"看起来合适"的 4xx；5xx 走类别文案**不回吐原文** —— 那些原文里实测带过 SQL 片段与上游响应体）、免登录**白名单**（漏登记 = 默认要登录）、未初始化的闸门（查库失败时**不给结论**）、会话 cookie（`HttpOnly` + `SameSite=Lax`，`Secure` 按部署）、账号/运行/定时任务三组路由、`SSE` 事件流（`Last-Event-ID` 与 `?after=` 同语义，先订阅再读库，队列满时重订阅**不静默跳过**）（**26 条测试**，用 `httptest` 起真服务器验状态码 / cookie 属性 / SSE 帧 / 错误体形状）。⭐ 见下方「服务端」两节 |
| `cli`                  | ✅ **全部叶子** | 参数面 / 退码 / 输出约定 / 口令边界 + **12 个叶子全部接上真后端**（**116 条测试**）：`version` / `capabilities` / `user init·list` / `migrate` / `login qr·sms` / `smid show·reset·refresh` / `fixture extract` / **`signin`（编排 1,444 行 + 渲染 584 行，42 条用例）**。⭕ 缺口表 `unmigrated.go` **已退役**（表空了就该整个删掉）；位置由 `TestEveryLeafActuallyRuns` 守着。⭐ 见下方「Go 侧 `cli` 落地」 |

- 落点 `internal/capabilities`、`internal/exec`（包名 `mcloudexec`）、`internal/app/payload.go`。
- ⚠ **`app` 现在仍只是"组装根接上了 IO"那个骨架**（`internal/app/app.go` 只有一个 `Deps`）。  
  payload 是第一个真正有内容的模块。
- ⚠ `core.MarshalJSON` / `core.ParseJSON` 这次**导出**了：产物层要拼对外 payload，  
  而"不转义 HTML + 不补尾换行"这两条规则**必须只有一份实现**（详见方案 §6.1 #22）。
- ⚠ `core.Version` 是新增常量 —— Go 没有 `env!("CARGO_PKG_VERSION")`，  
  它得与 `legacy/Cargo.toml` 的 `version` **手工同步**（不同步的表现只是两个产物报的版本号不一样）。

### 步 4 实况补充：Go 侧 `cli` 落地（2026-09-21，追加，不改上表那一行）

> 上表 `cli` 那一行（🟡 "参数面 / 退码 / 输出约定 / 口令边界已在（24 条测试）"）
> 说的是同一件事，这里补的是**它没写下来的判据与边界**。两者不冲突，别只看一行就下结论。
>
> ⭐ 本轮反向测试又挖出两个真缺口，已补断言并记进方案 §6.1（#67 / #68）：
> · 把 `capabilities.QRTTL` 改成 121，**一条测试都没变红** —— 现有断言比的是
>   顶层与 `signin` 的**相对**一致，两边都走同一个函数，恒等；于是"CLI 又硬编码了一份
>   120"完全查不出来 ⇒ 补 `TestQrTTLDefaultComesFromTheRegistryNotFromASecondLiteral`
>   （读 flag 默认值文本并与注册表常量比，这样改源头会让期望值跟着走）
> · 把 `notMigrated` 的 unknown 分支改成静默占位，也**一条都没变红** —— 现有用例走的
>   都是表里**已有**的那几条，那个分支从来没被执行过。而它存在的全部理由就是挡住
>   "顺手加了个子命令却忘了登记" ⇒ 它是纯函数，直接调即可（`TestAnUnregisteredNotMigratedCommandSaysSo`）

- **参数面**：`mcloud-cli` 那套 clap 定义逐条搬齐 —— 签到 21 个开关、`login qr` /
  `login sms`、`smid`、`user`、`migrate`、`fixture`、`capabilities`、`version`
  （12 条叶子路径，由 `TestCommandTreeMatchesTheDocumentedSurface` 钉住清单）。
  顶层与 `signin` 子命令**调同一个 `registerSigninFlags`**（= clap 的 `flatten`），
  并有一条断言**逐项比对两侧的每个 flag 的 `DefValue` / `Usage` / `Type`** ——
  因为"后来有人把参数抄成两份"在 Go 里**不报错**，只会静默漂移。
- **互斥表照搬 Rust 的语义，不重推**：写开关之间**可以**叠加
  （`--receive --backup-gift` 合法），只读开关与写开关**必须**互斥。
  成对表在 `internal/cli/args.go`，测试直接跑 Rust 侧 `unit_tests.rs` 里那批
  接受/拒绝清单（顶层与子命令两种写法各跑一遍）。
- **退码 = 0 成功 / 1 运行错 / 2 用法错**（clap 的语义）。
  ⚠ 这条踩到一个真缺陷：cobra 的 `execute()` 顺序是
  ParseFlags → `PersistentPreRunE` → `ValidateRequiredFlags` → `ValidateFlagGroups` → RunE，
  所以"是否已进入执行"这个标记**不能**在 `PersistentPreRunE` 里设 ——
  那样 `--list --account 1` 会退 1（Rust 退 2）。现在的做法是包一层 `RunE`。
  ⚠ 另一条：**只构造命令树看不到 `completion`**（cobra 在 `ExecuteC` 里才注册），
  所以"有没有多出用户不该看到的子命令"必须**实跑**才问得出来。
- **`--json` 约定**：stdout 只有一份干净 JSON，进度与提示**一律**走 stderr；
  错误走一行紧凑"用法"而不是 cobra 那整屏 help。
- **凭据边界**：口令**只从 stdin 读**（TTY 走 `term.ReadPassword` 不回显，
  非 TTY 退回读一行 —— 管道里 `echo pw | mcloud user init` 要能跑），
  **任何情况下都不存在 `--password` 这个参数**（参数进 shell 历史与 `ps`）。
  有断言钉住"两次输入不一致要拒"，以及"`$argon2id$` 与口令明文不得出现在任何输出里"；
  `user list` 逐字段打印而**不用 `%+v`**（后者会展开 `PasswordHash`）。
  ⚠ 本轮**没有新建任何凭据、也没有打印过任何真实标识**，测试数据全是 `199` 开头的合成号。
- **接了真后端的**：`version` / `capabilities` / `user init·list` / `migrate`
  （后者走 `app.MigrationPlanFromRepoRoot` + `app.RunMigration`，`--dry-run` 一条写都不发）、
  `login qr·sms`（**12 条**）、`smid show·reset·refresh`（**11 条**）、
  `fixture extract`（**15 条**）。
- ⭐ **`signin` 落地（最后一条叶子，2026-09-21）**：拆成**两条零编译依赖的线**并行做
  —— 渲染层 `signin_render.go`（584 行，纯函数：结果 → 终端行 / `--json`）与编排层
  `run_signin.go`（1,444 行），规格由第三条只读线产出（`docs/signin-cli-spec.md`，366 行）。
  切法是"渲染层不调编排、编排层不写渲染"，两边各自只写自己的文件 ⇒ 同包也不会撞编译。
  - 编排层覆盖 21 个 flag；**分支优先级照抄 Rust**（`--records > --prizes > --raw >
    --status-only > 签到主路`），否则行为静默分叉。
  - ⚠ **下标是"完整列表里的位置"，含停用账号** —— 一旦随启用状态变动，
    同一个数字在不同时刻会指到不同账号上。
  - ⚠ 三个**刻意**与 Rust 分叉的地方（都写在代码注释里）：进度行一律走 **stderr**；
    `--prizes --json` 用 typed 结构、**不带** `raw`（Rust 那份会把 `userDomainId`
    带出来）；"有账号但全停用"**不**再打"库里还没有账号"那句误导文案。
  - 反向测试 **42 条全部实测变红**。
- ⭐ **缺口表已退役**：`internal/cli/unmigrated.go`（连同 `notMigrated`、两条对价用例）
  随最后一条叶子一起删掉 —— 表自己的用例就写着"表空了就该整个删掉"。
  ⚠ 退役之后"某个叶子又变回没实现"就**没有守卫**了，所以补了
  `TestEveryLeafActuallyRuns`（12 个叶子逐个跑，一个都不许出现占位错误）。
- ⭐ **`fixture extract` 的核心判据是 fail closed**：写盘**之前**把整份产物再扫一遍，
  发现残留就**一个字节都不写**并报错退出。⭐ 这条分支**真实数据触发不了** ——
  脱敏规则刻意铺得比自检规则更宽（阈值 32 vs 40），否则每次抽取都会报警；
  于是留了一个注入缝 `leakScan`，否则没有任何办法证明它不是死代码。
- ⚠⚠ **判据账要算对（2026-09-21 更正，这一条之前记错了）**：步 4 那句
  "19 项能力逐项打勾"里，**19 这个数取自旧 TS 项目**（`packages/api/src/tasks/*`
  的 19 个 `TaskModule`，见 `rust-rewrite-plan.md` §1.2），**不是冻结基准的条目数**。
  实测两侧注册表：**Rust 3 项、Go 3 项，code 集合逐项相同**
  （`sign` / `exchange` / `cloud-snapshot`；两侧注释都写着"P4 会补齐全部 19 项"）。
  ⇒ **注册表这一项在迁移上已对位**，"还差 16 项"是把**功能开发**记成了迁移缺口。
  ⚠ 同理 `POST /api/runs`：`legacy/crates/mcloud-server/src/routes/mod.rs` 自己写着
  "写的那一半要等 P4"，**基准里从未实现** ⇒ 也不是缺口。
  ⇒ 结论：**CLI 与 app 都无关，`capabilities` 也不是** —— 这两个数当初就被误当成了判据。
- ⭐ **并发做法的结论**：这一块（`signin`，Rust 侧 1,264 行）用**三条线并行**做完 ——
  一条只读调研出规格、两条各写一个文件（渲染 / 编排）。
  ⚠ 能并行的前提是**先钉契约**：`signinView` 的字段形状先定死，两条线之间零编译依赖，
  否则 Go 按包编译会让"一方改到一半"直接卡住另一方。
  期间撞到过一次 `jsontags` 报"白名单里有对不上的条目"，**那是并行会话在飞编辑**
  `scripts/check-go-jsontags.cjs` 与 `internal/server/{auth,error,routes}.go` 的瞬时状态 ——
  他们的 `012c55a`（server 整包，26 条测试）已在 2026-09-21 落地，之后 `gates-go.sh` → 九项门禁全绿。
  ⇒ 这正好复现了上面那条铁律：**白名单条目与文件必须同进同出**。

### 步 4 收口：`server` 入口接线（2026-09-21）

`internal/server` 那 26 条测试测的是**处理器**，而 `cmd/mcloud-server` 一直是
"打印一句占位就退 2" —— 也就是说**没接线**（不是后端没写）。本轮把它接上，
新增三个文件 + 17 条测试（包内共 **43 条**）：

| 文件 | 职责 |
| --- | --- |
| `internal/server/server.go`（改写） | `Deps`（外部世界）/ `Options` / `ParseArgs` / 退出码常量 |
| `internal/server/static.go`（新） | `App(webDir)`：静态资源 + SPA 回退 |
| `internal/server/run.go`（新） | `Main`（解析 + 退出码）/ `Run`（开库 → 监听 → 横幅 → 阻塞） |

⭐ **三个承重判断，都做了反向测试（13/13 如期变红）**：

1. **静态资源必须挂在会话中间件的外层。** `Server.requireSession` 的默认是
   "除白名单一律要会话"，把静态挂进 mux 会让**首屏 401**（现象是白屏）。
   Rust 侧靠 `router.fallback_service(...)` 挂在中间件外；Go 的 `ServeMux`
   没有 fallback 这个概念 ⇒ 改成**先按路径分流**（`/api` 走 API，其余走静态）。
   ⚠ 这条断言是 `App` 存在的唯一理由，别处一条测试都不会红。
2. **`webDir` 不存在时只挂 API，非 `/api` 路径拿到的是 401（不是 404）。**
   这是**对的**：没有静态层时那个请求落在会话闸的默认分支上，
   与 Rust 侧 `web_dir.filter(is_dir)` 给出 `None` 时同一条路。
3. **先开库再监听。** 反过来的话，库打不开时端口已经被占过一下，
   而"端口被占用"是最容易被误读的失败（人第一反应是查谁占了端口）。
   ⚠ 这条**光断言"没调 Serve"是钉不住的**（把监听提到开库之前，`Serve` 照样不会被调，
   断言照样绿 —— 反向测试实测）。所以测试**先把端口占住**：顺序对了报的是**库**的错，
   顺序反了报的是**端口**的错，两个分支的文案不同，断言才有判别力。

⭐ **顺带修掉两处**：

- `internal/server/server.go:Deps` 里的 `HTTP *mcloudio.Client` 字段**是死代码**
  （除注释外零引用：跑任务那条路没落地，其余路由全是读库）。删掉并在注释里写明
  "等真有调用点再加" —— 摆一个没人读的字段只会让"接线的缺口"看起来像已经填上了。
- `/api`（**没有尾斜杠**）原先会吃 `ServeMux` 的一个 301 到 `/api/`，
  那是 Go 的产物不是我们的契约（Rust 侧那里就是 404）⇒ 补一条精确匹配，
  两种写法给出**同一个**错误形状。

⚠ **方案 §6.7 写的 `net/http + chi` 与实际不符**：Go 侧**没有引 `chi`**
（`go.mod` 里没有），用的是标准库 `ServeMux` —— Go 1.22 起它自带方法 + 通配路由，
`chi` 那一层在这里没有换来任何东西。方案那一行已改。

⇒ 上面这三条 + 两条顺带修掉的，已记进方案 **§6.1 #81–#84**
（⚠ 那两条**先发生**的 `cli::signin` 教训占用 #79 / #80，本轮顺延，别重编）。

⚠⚠ **收尾时还揪出一条"偶发、且指向错误方向"的门禁红**：`deps` 偶尔报
「`internal/app` / `internal/server` 未依赖 `internal/io`」，而它们**明明依赖** ——
根因是运行环境有时往 `go` 的 stdout 塞一句**没有换行**的
`Brokered program policy check unavailable`，它粘到 `go list` 输出的行首，
让「整行相等」那条判据**凭空找不到依赖**。
⇒ 判据改成「**定界符 + 子串**」（`-f '|{{.ImportPath}}|'` + `grep -qF "|X|"`），
精确性不丢；两条验证（模拟污染 + 注入真违规）都实测过。
详见 `docs/dev-gates.md`「偶发红」一节 + 方案 §6.1 **#85**。

### 与 `origin/main` 的分叉已合并（2026-09-21）

⚠ 收尾时发现本地分支**不只是"领先 48 个提交"，而是已经分叉**：
`origin/main` 上有 **2 个我没有的提交** —— `37ab29e`（docs：耗时由"改了哪一层"决定）
与 `cba917b`（fix(gates-go)：门禁名按表校验 + `-race` 缺 C 编译器时显式降级），
时间戳 2026-09-20 19:41 / 20:23，来自昨天那批并行会话。
它们改的正是 `docs/dev-gates.md` 与 `scripts/gates-go.sh`
⇒ **也就是说上面那轮"九项全绿"跑的是合并前的脚本**，而远端改的恰好就是它。

合并提交 **`47e2e0c`**（父 = `c052249` + `cba917b`）。冲突**只有一处**：
`gates-go.sh` 的末尾横幅 —— 我这边新增第九条门禁 `jsontags`（写"九项"），
远端把 `gate_test` 从**八条**基线上重构（加了 `is_known_gate` + `RACE_SKIPPED`）。
两侧都要 ⇒ 保留降级分支 + 说"九项"。

⭐ **这次合并暴露的是"文档里的条数没人维护"这一类腐坏**：`八条` 散在**四处**
（`gates-go.sh` 头注释、`dev-gates.md` 三处、`docs/README.md` 一处），
第 9 条门禁加了两天、没有一处跟着改 —— 而**门禁名是硬编码在 `GATE_NAMES` 里的**，
文档与它对不上时，读文档的人会以为自己少跑了一条。
⇒ 已全部改为九条；步 0 与「冻结」两行按**当时的条数**保留并加注（不篡改历史）。

⚠ **`jsontags` 原先没进 `pre-commit`**：hook 里写死的是四条
（`fmt deps filenames idents`），而 `jsontags` 只 `require` 了 `fs` / `path`、
**不需要编译**，正落在 pre-commit 的判据上（"秒级、问题一旦产生就立刻可修且必然要修"）。
⇒ 已加入，现在 pre-commit **五条**、pre-push 仍是全部九条。

门禁：**合并后在合并树上重跑，九项全绿**（那轮跑的是合并前的脚本，见上）。

### 收尾整理：这一轮找出来的"不对的地方"（2026-09-21）

用户原话是"先提交，然后整理那些不对的地方"。逐条如下，**前四条都已修**：

| # | 不对的地方 | 症状（为什么一直没人发现） | 处置 |
| --- | --- | --- | --- |
| 1 | **两个 git hook 从未生效**（三层，各自都足以让它静默失效） | ① `core.hooksPath` 根本没设过；② 更要命的是**它们以 `100644` 入库**（`git ls-files -s .githooks/` 可验），而 git 对**没有 x 位**的 hook 是**直接跳过**、不报错；③ 装好之后立刻暴露第三层：hook 里写的是 `exec scripts/gates-go.sh`，而 `scripts/*.sh` 也是 `100644` ⇒ `Permission denied`，**每一次提交都被拦住**。⇒ 门禁**一次都没自动跑过** | `bash scripts/install-hooks.sh`（它本来就 chmod +x 并写明了理由）；**把 mode 变更（644→755）提交进库**；hook 改成 `exec bash …/gates-go.sh`（不依赖 x 位，与文档里的 `bash scripts/gates-go.sh` 一致）。⚠ 判据：**路径上任何一环缺 x 位，"跳过"与"拦住"都不报出真正的原因** |
| 2 | `scripts/install-hooks.sh` 打印的是**过期的指引** | 原文"pre-push 跑七条全量，含 clippy"是 **Rust 侧**的说法（Go 侧既没有 clippy，条数也不是七）。⚠ 它是**第二份真相**，而真相处（`GATE_NAMES`）改了它不会跟着改 | 删掉具体条数，改成指向 `.githooks/README.md` 与 `gates-go.sh` 头注释 |
| 3 | 步 4 判据里的 **"19 项能力 / `POST /api/runs` 未实现"** | 19 取自**旧 TS 项目**，冻结基准的注册表只有 3 项（code 集合与 Go **完全相同**）；`POST /api/runs` 在 `legacy/.../routes/mod.rs` 里自己写着"要等 P4"。⇒ 一个**已对位**的迁移被记成没做完 | 见 §6.1 **#86**；`docs/go-migration-plan.md` 与本文档同步更正 |
| 4 | **`login-crypto-vectors.json` 是死向量** | 有生成器、无消费者，golden 门禁每轮打 `⚠ 无任何测试读取（死向量）` 但**不失败** ⇒ 可以无限期挂着 | 接上消费者：`internal/upstream/login_crypto_vectors_test.go`（7 条）+ `internal/app/device_test.go`（1 条）。⚠ 与预判不同：**不是"改已有断言"，是新写一份** —— 两份向量文件的段名**完全不相交**（见 §6.1 末尾） |
| 5 | `check-golden-vectors.cjs` 里 `guid32` 的**豁免理由过期** | 写着"uuid→md5 的具体拼接未在向量里给出"，而生成器里 `getGUID32` **逐字给了**。⇒ 2 条本来能算的向量长期挂在"未覆盖"清单里 | 删该条 + 补独立实现，断言 131 → **133**。见 §6.1 **#88** |

**新长出来的两条通则**（已进 §6.1）：**#88** 借口表会过期且过期时不报错；
**#89** 用向量的 `name` / 键当循环源的断言，键一改名就**静默少测一条**（判据必须双向）。

**反向测试**：新断言 **6 个变异全部如期变红**。
⚠ 脚本在 `target/`（**gitignored**），所以把变异逐条记在这里 ——
否则下一个人只看到"做过反向测试"，复现不了：

| 变异打在哪 | 改什么 | 应变红的目标 |
| --- | --- | --- |
| 实现 | 口令前缀少一个冒号（`"fetion.com.cn:"` → 无冒号） | `TestLoginVectorSha1PasswordHashesWithTheSourcePrefix` |
| 实现 | 滑块 IV 改一个字符（`PuzzleAESIV` 末位 `'4'` → `'5'`） | `TestLoginVectorPuzzleAnswerMatchesTheRealCaptures` |
| 实现 | 设备 id 后缀多一个字符（`DeviceIDSuffix` → `"-ENDINN"`） | `TestDeviceFingerprintMatchesTheGoldenVectorShape` |
| **数据** | 向量里**加**一个没人接的常量 | `TestLoginVectorConstantsAllHaveAGoCounterpart`（正向） |
| **数据** | 向量里把一个常量**改名** | 同上（反向：Go 侧在断言一个已不存在的名字） |
| **数据** | 向量里**改一位密文** | `…AesEcbEncryptsToTheSameBytes` + `…TwoSectionsDescribeTheSameCiphertext` |

⭐ 后三条是这一轮的新东西：**变异打在"数据"那一侧**。
只变异实现会漏掉"数据的键名漂移"这类失效（见 §6.1 #89）。

### 第二轮：把整张「借口表」审一遍（2026-09-21）

第一轮那 5 条是**撞见**的，但它们的根因是同一个：**"第二份真相"** ——
一份没有东西负责同步、漂移了也不会变红的副本。顺着根因去找最可疑的那份，
就是 `check-golden-vectors.cjs` 的 `NOT_VERIFIED_REASONS`（"这一段为什么没验"）。

**14 条里 6 条被实测推翻** —— 不是"条件变了"，是**当时就没验过**：

| # | 豁免理由（原文） | 实测结论 | 处置 |
| --- | --- | --- | --- |
| 6 | `desEcbNoPaddingB64`：**"Node 的 des-ecb 需 legacy provider"** | 前半句**真**（`error:0308010C`），但由它推出"所以验不了"**错** —— `des-ede3-ecb` 三把子密钥相同时 `E(D(E(x)))==E(x)`，**退化成单 DES** | 补独立实现，**18 条全过** |
| 7 | 同上：**"补位规则（零/空格）未确认"** | 向量名 `key-short-zero-padded` / `key-long-truncated` **就是专门锁这个的**；同一文件 `normalizeKey16` 的注释早讲过这条 ⇒ **一个文件里两处自相矛盾** | 同上 |
| 8 | `desEcbPkcs7Decrypt`：**"同上"** | 同上（手工剥 PKCS#7 即可） | 补实现，**3 条全过** |
| 9 | `getTn`：**"字符串变换，规则未在向量里给出"** | 向量里 4 条 input→expect **齐全**；且 `gen-crypto-vectors.cjs:142` 里**就躺着一份 JS 实现** | 补独立实现，**4 条全过** |
| 10 | `smidTn`：**"未在本脚本复刻（同 §getTn）"** | 同上（它引用的 §getTn 那条理由本身就是错的） | 补实现，**2 条全过** |
| 11 | `rsaPkcs1v15B64` / `rsaSelfKeyPair`：**"DER 包装收益低"** | 包装 15 行、**也已经在生成器里**；RSA 虽无字节级期望值，但**不变量**能锁 | 补实现（形状 + 私钥解密回原文），**7 条全过** |

**顺带查出一条真错**：`gen-crypto-vectors.cjs` 里 `rsaEncrypt` 的注释写着
"**确定性填充，可字节级对拍**" —— 与同仓库向量自己的 `byteExact: false` 和
§6.1 **#49** 直接矛盾（PKCS#1 v1.5 的 `PS` 是**随机非零字节**）。已改成正确说法，
并把"**两次采样必须不同**"做成断言 —— 两个说法只能活一个，现在由门禁裁决。

**断言 133 → 167**（+34），"未覆盖"清单 **14 → 8 条**。
剩下的 8 条理由也都重写过了：不再说"由 Rust 侧断言"（Rust 已冻结），
改成实际消费者（Go 侧 `ParseSignInStatus` / `ParseStartSignIn` / `ParsePrizeRecords` /
`ParseRevivalReward` / `internal/core/auth.go`）。

**反向测试**：**13 个变异全部变红**（7 个打实现、6 个打**数据**）：

| 变异打在哪 | 改什么 | 目标 |
| --- | --- | --- |
| 实现 | `normalizeKey8` 缓冲区 8→7 | `desEcbNoPaddingB64` 密钥归一化 |
| 实现 | `zeroPad8` 总是多补一整块 | 明文补位量 |
| 实现 | `pkcs7Unpad8` 不剥补位 | `desEcbPkcs7Decrypt` |
| 实现 | `getTn` 的 `floor` → `Math.round` | `getTn` 小数分支 |
| 实现 | `getTn` 不排序（改逆序） | `getTn` + `smidTn` 排序 |
| 实现 | `desKey24` 只喂 8 字节 | 3DES 退化技巧（直接抛错） |
| 实现 | `rsaSelfKeyPair` 拿 `modulusHex` 当 `e` 的期望值 | DER 解出的 n / e |
| 数据 | `desEcbNoPaddingB64[0]` 期望密文改一个字符 | 向量确被比较 |
| 数据 | `getTn[0]` 期望值加一个字符 | 同上 |
| 数据 | `rsaPkcs1v15B64` 第二次采样换成第一次 | **"两次必须不同"** 这条判据 |
| 数据 | `rsaSelfKeyPair.cases[0]` 密文改一个字符 | 私钥解密回原文 |
| 数据 | `smidTn[0]` 期望摘要清零 | `smidTn` |

⚠ **其中一个变异先"假绿"过一次**：把 `zeroPad8` 的提前返回删掉
（`if (false) return buf;`）**是语义空操作** —— 已对齐时 `Math.ceil(len/8)*8 === len`，
算出来的字节一模一样。差一点记成"这条断言不承重"（见 §6.1 **#91**）。
判据：**"仍然绿"先怀疑变异本身有没有真的改变输出**。

### 第三轮：门禁脚本**自己** —— 清单的第二份副本 + 横幅报大（2026-09-21）

前两轮的根因都是"第二份真相"。顺着它去看**门禁脚本自己**（它是判断别人对不对的东西，
它错了没人管），`scripts/gates-go.sh` 里一次找出两处：

**① `ORDER` 是 `GATE_NAMES` 的逐字重抄。** 同一个文件里两份一模一样的清单
（第 45 行与第 131 行）。加了门禁却忘了加进 `ORDER` ⇒ 它**默认永远不跑**，
而且什么都不报 —— `is_known_gate` 只保证"名字合法"，不保证"它会被跑到"。
⇒ 改成 `ORDER=($GATE_NAMES)` **派生**：两份副本直接消失，
比给它加个守卫更彻底（已经没有可漂移的东西了）。

**② 末尾横幅无条件报"全绿"。** 写死 `printf '九项门禁全绿'`，不看这次跑了哪几条：

```
$ bash scripts/gates-go.sh fmt          # 只跑一条
✓ fmt 通过
九项门禁全绿                              ← 把 1/9 报成了 9/9
```

而 **pre-commit 跑的正是 5 条子集** ⇒ **每一次提交**都打印一句"九项门禁全绿"。
⚠ 要命的是这条横幅**正是被抄进本文档的**（"门禁实况：九项门禁全绿"）——
抄进去之后**没人能再复核它量的是 9 条还是 5 条**。与"绿只对**跑的那个版本**成立"
是同一条判据，只是这次错在**输出**而不是判定。

⇒ 现在：

```
$ bash scripts/gates-go.sh fmt deps filenames idents jsontags     # pre-commit 那 5 条
门禁通过：fmt deps filenames idents jsontags（本次 5/9 条）
⚠ 未跑：build vet test golden
  ⇒ 别把这当成"全绿"：上面这几条**这次没验**。

$ bash scripts/gates-go.sh                                        # 全量
九项门禁全绿
```

条数取 `${#ALL[@]}`、中文数字查表（保住文档里"九项"这个说法），
**都从那份清单派生** ⇒ 下次加门禁时横幅自动跟上。

⚠ **这条修复没有断言守着**（它改的是输出形状，不是判定）——
守着它的是"**派生而非重抄**"这个写法本身。谁把它改回手写清单，就回到原状。

**顺带核过**（本来是对的，没动）：`bash scripts/gates-go.sh fmt nosuchgate` 退 **2**
（不存在的名字**响亮失败**，不会静默跳过）；`gate_run` 的 `case` 分支与 `GATE_NAMES`
逐条对得上，且漏实现时会打印"登记在 GATE_NAMES 里但没有实现分支"而不是含糊的"失败"。

**验收**：`922 个测试`（914 → +8）、`九项门禁全绿`、golden **167 条断言全绿**、
`test/golden/` **11 份向量全部有消费者**（此前 10/11）。

---

## 一句话总览

| 层            | 状态                                                                                                                                           |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| CLI：初始化 / 登录 | ✅ 可用（扫码 + 短信，PC 端与 App 端）                                                                                                                    |
| CLI：签到（云朵中心） | ✅ 端到端跑通（含领取、复活卡）                                                                                                                             |
| 上游接口层        | ⚠ **逐块重写中** —— 只有 `login` 与 `app_signin` 是完整的，其余仍是骨架                                                                                         |
| 服务入口         | ✅ **Go 侧已接线**：API 四组业务路由（含 SSE）+ 前端产物与 SPA 回退；**15 条路由逐条对位冻结基准**（有断言盯着）。⚠ `POST /api/runs` **基准里也没有**（P4 功能，非迁移缺口） |
| WebUI        | ⚠ 登录与初始化是真接口，**主界面仍是 P0 spike 的占位数据**                                                                                                        |
| 运行时层         | ⚠ 事件模型 / 生命周期 / 事件总线 / `run_events` 表已落地，编排与调度接入中                                                                                            |
| 夹具回放（L2）     | 🟡 **回放器已就绪**（`internal/io/fixture.go`，17 条测试）；已有消费者 `TestEveryArchivedFixtureIsLeakFree`（31 份抓包夹具全量自检），⭕ 但上游协议层**还没真的加载这批夹具跑一遍**（见下方「夹具回放」） |

---

## 历史流水账（已折叠，只留结论）

测试数一路涨上来的过程本身不构成"当前状态"，故压成下表。当前数见下方「门禁实况」。

| 时间         | 测试数     | 当轮新增了什么                                                             |
| ---------- | ------- | ------------------------------------------------------------------- |
| 2026-09-17 | 241     | 当时的一帧                                                               |
| ——         | 493     | 接完 smid（设备指纹绑账号）那条线，自贡献 46 条（上游算法 25 + 生命周期 13 + 存储 8）              |
| ——         | 516     | App 签到线：上游接口层 + 编排层 + 存储层（`account_state.phone`，schema v6）          |
| ——         | 523     | 云朵余额采集（`mcloud-app::cloud` 5 条 + `set_account_cloud` 窄写 2 条）        |
| ——         | 533     | `receiveV3` 领取（请求构造 / 解析 / 错误码 10 条）                                |
| ——         | 541     | `revival` 复活卡（8 条，含一条读黄金向量的）                                        |
| ——         | 542     | 查明 H5 `deviceId` 根因并接线（新增一条把签到页 `_smConf` 与 Rust 常量钉在一起的守卫）         |
| ——         | 546     | `Authorization` 改为整串存整串用（schema v7），新增 4 条                          |
| ——         | 554     | CLI 账号选择（下标语义）补 9 条用例                                               |
| 2026-09-19 | **586** | 把云端那 9 个提交拉进本地（`mcloud-app` 组装层、App 签到全链路、schema v7 等），并修掉两处 clippy |

> 这里曾经把 554 写成"当前数"—— 那是**拉取云端之前**本机的数。  
> 云端那 9 个提交带进来的 `mcloud-app` 等新模块另有自己的 lib 测试，落地后实跑是 **586**。

**门禁实况（2026-09-21，Go 侧）**：**922 个测试通过**，**九条门禁全绿**  
（黄金向量那段是 **167 条断言**，见 [`dev-gates.md`](./dev-gates.md)）。  
⚠ 上面「当前数见下方门禁实况」指的就是这一段 —— 它早先写的是 **Rust 侧**
（2026-09-19：586 个 / 七条），那是**冻结前**的数，别拿它当现状。
Go 侧逐包：core 270 / upstream 157 / io 150 / app 118 / cli 116 / server 44 /
exec 35 / capabilities 32。

---

## 逐项须知

### 服务入口：Go 侧已接线，前端产物与 API 都在

> ⚠ 这一节早先写的是 Rust 侧（冻结那一刻）的样子：`cargo run -p mcloud-server`
> 起 API + 前端产物，但路由只有 `/api/health` / `/api/setup` / `/api/session`。
> 那是**旧**状态 —— 下面按 **Go 侧**重写。

`go run ./cmd/mcloud-server`（默认 `127.0.0.1:8787`）现在能起来，并同时提供：

- **API**：健康 / 初始化 / 会话，加上 accounts、capabilities、runs（含 SSE 事件流）、
  schedules 四组业务路由 —— 白名单免登录（只有 `/api/health`、`/api/setup`
  与 `POST /api/session`），其余一律要会话；
- **前端产物**：`web/dist` 的静态文件 + SPA 回退（非 `/api` 的未知路径交给
  `index.html`，前端路由自己处理）。目录不存在时**只挂 API**，并在启动横幅里说明 ——
  开发期先 `cd web && npm run dev` 是常态。

命令行五个选项与 Rust 侧逐项对齐（`--db` / `--host` / `--port` / `--web-dir` /
`--secure-cookie`），退出码同一套口径：0 正常、1 运行错误、2 用法错误。

⭕ **迁移侧没有缺口**：15 条路由逐条对位冻结基准（`internal/server/routes_test.go` 盯着），
`POST /api/runs` **基准里也没有**（`routes/mod.rs` 写明"要等 P4"）。
⚠ 所以"WebUI 主界面仍是占位数据"的**原因不是缺接口**，而是**跑任务那条路本身还没建**
（P4/P5 的功能开发）—— 别把它记成迁移没做完。`openapi.json` 导出同理，是新功能。

> 更早（2026-09-17）时 `mcloud-server` 只有 lib、没有 `serve` 命令，CLI 能跑而 WebUI  
> 没有后端可连，前端只能靠 `vite dev` 单独起。这一条已经过去了。

### CLI：已能完成"从零到登录"

`mcloud user init` → `mcloud login qr`（终端二维码）/ `mcloud login sms`。

短信登录有两个端：`--client pc`（默认）与 `--client app`（iOS 客户端形态）。  
两端**不是同一件事的两种参数** —— 请求体加解密方式、`Authorization` 前缀、  
设备指纹形状都不同（差异见 [`client-profiles.md`](./client-profiles.md)）。

登录态落在本地库的 `account_state`（凭据与展示信息**分表**）；  
设备指纹写在 `data/device.txt`（**换掉它等于换一台设备**，服务端会定位不到会话）。

### 签到（云朵中心）已端到端跑通

`mcloud signin --account <掩码id>`。这是第一个按新抓包写完整的**业务**接口块 ——  
`mcloud-upstream::app_signin`（请求构造 + 响应解析）+ `mcloud-app::signin`（编排）。

协议、六个要点与坑见 [`cli-usage.md`](./cli-usage.md) §签到。

### 上游接口层按新抓包逐块重写

目前 `login`（扫码 / 短信 / 账密 + 退出登录）与 `app_signin`（签到）是完整的，  
它们是"按新流量写接口"的头两块；其余云盘业务接口仍是骨架（`Upstream` + `Envelope` + `base`）。

旧 TS 的 12 个接口模块**一律不从旧代码搬迁** —— 旧 HAR 与旧 TS 都会作废，  
搬了只是把旧形态再抄一遍。新 HAR 抓好后：`mcloud fixture extract` → 按新流量写接口  
→ 用夹具离线回放并断言请求逐字段正确。

### ⚠ 夹具回放（L2）仍只有一个消费者

此前本文件写着"`mcloud-upstream` 的 L2 夹具回放已跑通  
（`legacy/crates/mcloud-upstream/tests/l2_replay.rs`）"—— **那个文件不存在**，  
`legacy/crates/mcloud-upstream/tests/` 是个**空目录**。

现状是：`mcloud fixture extract` 往 `test/fixtures/http/` 写，消费者目前**只有一条**  
—— `io` 包的 `TestEveryArchivedFixtureIsLeakFree`（把 31 份夹具全量加载跑脱敏自检）。  
上游协议层**还没有**真的加载这批夹具把响应解析一遍，所以"回放器已跑通"这句话  
仍然**不能**按"L2 回放已接上"来理解。这是规划 §6.3 里要补的一环。

已确认可用的样本（写回放时直接照着接）：

| 接口                                      | 在哪个夹具                  |
| --------------------------------------- | ---------------------- |
| `startSignIn` / `infoV3` / `doTaskPost` | `3d.json`              |
| `taskListV3`                            | `每月阅读任务.json`          |
| `receiveRevivalReward`                  | `复活.json`              |
| `receiveV3`                             | **没有任何夹具**（只能靠单测 + 真机） |

### 旧 TS 与线上实际请求本就不一致，重写时要逐条核对

上一轮对拍换来的经验，**别默认"照 TS 抄"就是对的**：

| 接口                            | TS 写法                                | 真实流量                                      |
| ----------------------------- | ------------------------------------ | ----------------------------------------- |
| `visitlog/journaling`         | 参数放在 **query**                       | 参数放在**表单体**                               |
| `signin/task/taskListV2`      | `clientVersion=11.2.1`、`group=month` | `clientVersion=11.4.0`、`group=new`        |
| `auth-service/auth/tyrzLogin` | body 带 `type`/`channel`              | body 无这两个字段，`marketName=newsign_139mail`  |
| `market/signin/page/infoV3`   | 不带端口的 `/market/...`                  | `caiyun.feixin.10086.cn:7071/ycloud/...`  |
| `noteRefreshToken`            | `note.mcloud.139.com/noteServer/...` | `note-njs.yun.139.com/yun-note/...`（另一个域） |

另有：`refreshToken` 的真实流量是**加密载荷**（原实现注释却说是明文 JSON 接口）；  
`auth/refreshToken`、`querySpecTokenV2` 这类接口的 `code` 是**字符串**不是数字。

> ✅ 上面"加密载荷"这条 **2026-09-20 已定案**（此前只有结论、没有钥匙）：  
> 带 `hcy-cool-flag` 的请求与响应都是  
> `base64(IV(16 字节二进制) ‖ AES-192-CBC/PKCS7("2olBaQGYnEKoStYomsd1n7ax", IV, 明文))`，  
> 双向；**396 条实拍零例外**。钥匙就藏在旧项目  
> `packages/core/src/utils/encrypt.ts:3` 的 `REFRESH_TOKEN_KEY`，*而它导出的解码函数    
> **全仓无人调用**\** —— 见 `docs/upst`*`ream-app-business-api.md` §8.0。  
> ⚠ 它与发码链路的 `uDjGSA8auFlbwDkY`（AES-128）**不是同一把**，别互搬。
>
> ✅ **同日再更正：这类信封其实有不止一把钥匙，按「谁发的」分** ——  
> 上面那把 AES-192 是 **App 原生**（`FXHttpBaseAccessor`）用的；  
> **H5 / WebView**（`coolFlagPivotal`）用另一把 **AES-256**  
> `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1`，格式完全同形。  
> 判据：`x-yun-uni` + `x-yun-api-version` ⇒ 原生；`origin: mcloudlocal://` + `sec-fetch-*` ⇒ WebView。  
> ⚠ **钥匙是「谁发的」的函数，不是版本号** —— 别按"最新版就该用最新那把"去猜。  
> 见 `docs/upstream-app-business-api.md` §8.1。

---

## 按任务指路

| 你要做的事       | 读哪篇                                                                |
| ----------- | ------------------------------------------------------------------ |
| 跑门禁 / 看分层约束 | [`dev-gates.md`](./dev-gates.md)                                   |
| 敲命令行 / 查参数  | [`cli-usage.md`](./cli-usage.md)                                   |
| 加文件 / 起新目录  | [`repo-layout.md`](./repo-layout.md)                               |
| 实现某个业务能力    | [`business-feature-inventory.md`](./business-feature-inventory.md) |
| 全部文档的索引     | [`README.md`](./README.md)                                         |
