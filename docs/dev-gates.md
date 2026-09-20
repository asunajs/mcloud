# 门禁与分层铁律

> 状态：**规范**（改代码前必读；跑门禁是完成定义的一部分）
>
> - 入口：`bash scripts/gates.sh`（一条命令跑完全部，任一条失败即非零退出）
> - 权威说明：本文；分层表与 `scripts/check-deps.sh` 的可执行断言**互为镜像**，改一处要改另一处

---

## 一条命令

```bash
bash scripts/gates.sh                    # 全跑
bash scripts/gates.sh test clippy        # 只跑指定的几条
```

工具链定位只有一份实现：`scripts/toolchain.sh`（被 `gates.sh` 与 `.githooks/`
source）。**不要复制那段代码** —— 它已经因为有三份拷贝而分叉过一次，
详见下面「跨环境」。

---

## 七条门禁

| # | 命令 | 管什么 |
| --- | --- | --- |
| 1 | `cargo test` | 单元测试 |
| 2 | `cargo clippy --all-targets -- -D warnings` | 静态检查（warnings 视为错误） |
| 3 | `cargo fmt --check` | 格式 |
| 4 | `bash scripts/check-deps.sh` | 分层铁律（见下） |
| 5 | `bash scripts/check-filenames.sh` | 非 Rust 文件一律 kebab-case |
| 6 | `node scripts/check-identifiers.cjs` | 仓库里不得有真实标识 |
| 7 | `node scripts/check-golden-vectors.cjs` | 黄金向量能算出自己 |

---

## 在哪里跑

三个入口，**跑的是同一组命令**，区别只在"什么时候"与"给谁看"：

| 入口 | 什么时候 | 覆盖面 |
| --- | --- | --- |
| `pre-commit` | 每次提交 | 1/3/4/5/6 里不需要编译的四条（`fmt` `deps` `filenames` `idents`） |
| `pre-push` | 每次推送 | 全七条 |
| `.github/workflows/ci.yml` | push / PR 到 `main` | 全七条，在一个干净的 Ubuntu runner 上 |

前两个装一次即可：`bash scripts/install-hooks.sh`（细节见 `.githooks/README.md`）。

### 三者不是"三道防线"

同一组命令在三个地方执行，**不构成三次独立的检查**。真正独立的第二道防线必须
能发现第一道查不出来的问题；重复执行同一组断言只是赌"环境差异会碰巧暴露什么"。

| 入口 | 真实价值 |
| --- | --- |
| 本地 hook | **日常**：改完就知道，不用等 |
| GitHub Actions | **对外**：公开仓库里任何人可在 Actions 页面核对，是一份可外证的记录 |

所以 CI 不是 hook 的"兜底"——它的检查面并不更宽。**用 `--no-verify` 跳过
hook 之后，不会有任何东西替你补上那次检查。**

---

## 跨环境

七条门禁本身都是**平台无关**的，这是刻意核过的（不是"碰巧能跑"）：

- **Rust 侧**：零个 `cfg(unix)` / `cfg(windows)`；测试用 `tempfile::tempdir()`
  而不是硬编码 `/tmp`；`local_tz()` 是硬编码 `TZ_NAME.parse()`，不读系统时区。
- **TLS**：用 `rustls`（不是 `native-tls`），依赖树里没有 openssl ——
  所以不需要目标平台的头文件。
- **Node 侧**（`idents` / `golden`）：零个 `execSync` / `process.platform`，纯 fs 遍历。
- **bash 侧**（`filenames` / `deps`）：零个 `uname` / `cygpath` / `realpath` 分支。

**唯一有平台差异的是工具链定位**（`scripts/toolchain.sh`）：

- **Windows + Git Bash** —— 需要手工补：`RUSTUP_HOME` 指向非默认位置、
  工具链真实 `bin` 置顶 PATH（绕开会联网同步的 rustup shim）、
  MSVC 的 `Hostx64/x64` 进 PATH（顶掉 Git Bash 自带的 GNU `link`）、
  导出 `INCLUDE` / `LIB`（必须是 `C:/...` 原生路径，MSVC 不认 `/c/...`）。
- **macOS / Linux** —— 装好的 rustup 就在 PATH 上，**不需要补任何东西**。
- **其他平台** —— 打印提示但不直接退出（WSL 里 rustup 装好了就能用），
  成败交给后面的 `rustc` 探测。

⚠ 这段逻辑历史上**有三份拷贝**（`gates.sh` / `.githooks/run-gates.sh` /
`toolchain.sh`），并且已经分叉：hook 那份少了 `INCLUDE` / `LIB`，
于是 Windows 上 `clippy` / `test` **在 hook 里链接失败、在 `gates.sh` 里却能过** ——
同一仓库、同一组命令、两个结论。现在统一成 `scripts/toolchain.sh` 一份。
**再加入口时 source 它，不要抄。**

> **首次在 Linux / macOS 上跑门禁时**，请把实际结果（过了、或卡在哪条）
> 补进本节。CI 配置至今**未在真实 runner 上验证过**，
> 所以它的第一次运行才是它被证明可行的那一次 —— 在那之前，
> "CI 已就位"只是一种期望，不是事实。

### 第 5 条为什么值得单独存在

这条规则以前**只写在文档里**，于是 `web/src/pages/` 下三个 `AuthPage.tsx`
一路活到被用户点名。规则写在文档里没人执行，就会反复被违反 ——
而违反的表现极其安静：编译能过、测试全绿、界面正常，
只有下一个人加文件时"照着旁边那个写"，于是错误自我复制。

判据：按 `.` 把文件名拆成段，每段必须匹配 `^[a-z0-9]+(-[a-z0-9]+)*$`。
Rust 源文件不在管辖范围 —— 文件名就是模块名，`mod my-mod;` 不合法，编译器已经强制了。

豁免分三类（**不允许因为"改名麻烦"而豁免**）：

1. **生态固定名**：`Cargo.toml` / `README.md` / `SKILL.md` / `index.html` —— 工具按字面找它们；
2. **上游原样取出的证据**：`docs/capture/apiServer.js`、`test/fixtures/legacy/$meta.json`
   —— 改名会破坏"文件里能搜到的符号 ↔ 文件名"的对应，而这份素材的价值恰恰在于可逐字对照；
3. **生成物 / 依赖目录**：`node_modules` / `target` / `dist`。

新增豁免要同时满足"它属于上述三类之一"与"在脚本注释里写清属于哪一类"。

### 第 7 条是抓包侧的自检

`test/golden/*.json` 是要进版本库的产物，但"有人手工改坏一个字符"只有真算一遍才发现。
它用 Node `crypto` 独立实现算**122 条**向量，与 Rust 侧**互为交叉验证** ——
一边过一边不过，说明其中一边错了。

它同时做两件 Rust 侧做不到的事：

- 对**每个** golden 文件跑脱敏扫描（Rust 只扫 `include_str!` 进来的那些）；
- 列出**没有任何 Rust 测试读取**的向量文件
  （`app-business-crypto-vectors.json` 与 `login-crypto-vectors.json` 目前是这种状态）。

覆盖范围是**显式**的：没有校验器的段会被**打印出来**，不假装通过。

---

## 分层铁律

依赖方向（箭头左侧依赖于右侧，与各 crate 的 `Cargo.toml` 一致）：

```
mcloud-core          （无内部依赖）
mcloud-upstream      → core
mcloud-capabilities  → core, upstream
mcloud-exec          → core, capabilities
mcloud-io            → core        （+ reqwest / rusqlite / rand：全项目唯一实现 IO 的地方）
mcloud-app           → core, upstream, capabilities, io          组装层（两个产物共用）
mcloud-server        → core, capabilities, exec, io, app         产物之一（HTTP/WebUI）
mcloud-cli           → core, upstream, capabilities, io, app     产物之一（命令行）
```

| crate | 角色 | 约束 |
| --- | --- | --- |
| `mcloud-core` | 领域类型、纯函数、注入 trait 定义 | **禁止任何 IO**：不得依赖 reqwest、rusqlite、mcloud-io |
| `mcloud-upstream` | 上游接口（请求构造、响应解析、签名） | 同上。因此密码学放在 core 而非 io |
| `mcloud-capabilities` | 能力注册表（CLI 与 WebUI 的唯一契约来源） | 同上 |
| `mcloud-exec` | 任务编排 | 同上 |
| `mcloud-io` | reqwest / rusqlite / 系统随机 | 唯一实现 IO 的地方 |
| `mcloud-app` | 组装层 | **必须**依赖 mcloud-io；迁移策略、能力注册表 JSON、登录驱动都在这里 |
| `mcloud-server` | 产物：HTTP/WebUI | **必须**依赖 mcloud-io，否则是"没人接上 IO 实现"的静默死配置 |
| `mcloud-cli` | 产物：命令行 | **不得**依赖 `mcloud-server`（否则会把 axum 拖进命令行产物） |

外部世界一律走注入 trait，共 4 个：`HttpClient`、`Clock`、`StateStore`、`Random`。
`Random` 是必需的 —— smid 构造混用了 `randomUUID` 与 `Math.random`，
不注入则 fixture 回放与黄金向量对拍无法复现。

### 两条"不写下来就会悄悄失效"的约束

**`mcloud-server` 必须依赖 `mcloud-io`。** 反过来（没人接上 IO 实现）是一个静默的死配置：
编译能过、跑起来什么都没发生。

**`mcloud-cli` 不得依赖 `mcloud-server`**（`check-deps.sh` 的 `FORBIDDEN_EDGES` 断言这条）。
两个产物只共用 `mcloud-app`：CLI 一旦拿到 `mcloud-server`，就会连带把 axum / 路由 / 静态资源
拖进命令行产物，而这件事**不会让任何测试变红**，只会让"CLI 能在没有 HTTP 环境的地方跑"
这条性质消失。

### 为什么是脚本而不是文档约定

上表不是文档约定，是**可执行断言**：`scripts/check-deps.sh` 会真的读每个 `Cargo.toml`
并断言依赖方向，违反即非零退出 —— 而不是只在文档里写着"请勿依赖"。
