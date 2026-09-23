# 目录结构与文件命名

> 状态：**规范**（加文件前必读；命名由 `node scripts/check-filenames.js` 强制执行）
>
> - 相关：[`dev-gates.md`](./dev-gates.md)（门禁总表；本文的规则 = Go 侧第 6 条；
>   Rust 剥离后该条在其门禁体系里编号 5，脚本两边共用）

---

## 目录结构

```
cmd/            Go 产物入口（mcloud / mcloud-server）
internal/       Go 实现八层（分层见 dev-gates.md 与 go-migration-plan.md §5）
web/            WebUI：Vue Vapor（无虚拟 DOM）+ TSX + daisyUI
docs/           文档（本目录）—— 索引在 README.md，"谁说了算"也在那篇
  upstream/       上游协议取证，**按端一篇**：login-protocol / logout-sms-login /
                  web-qr-login / app-sms-login / app-business-api
  archive/        归档：**不删、但别当现状读**（实况流水、逐 crate 要点、
                  签到规格的历史段、一次性体检报告）
  capture/        抓包原始素材与上游客户端源码（豁免规则见下面 §豁免第 2 类）
  其余扁平放一层         规范类（dev-gates / repo-layout / credential-boundary /
                  sanitize-guide）、实况类（status / go-migration-plan /
                  migration-pitfalls）、手册类（cli-usage / signin-cli-spec）、
                  盘点与对照（business- / caiyun-go- / client-profiles /
                  device-and-source-codes）、前端三篇（webui-design-guide /
                  webui-components / webui-code-guide）
  ⚠ **一篇拆多篇的写法**：文件名用"主题前缀 + 各篇自己的后缀"
                  （`caiyun-go-exchange-and-runtime.md`、
                  `client-profiles-fingerprint-and-landing.md`），
                  **`§` 编号沿用拆分前那一版并在每篇抬头互链** ——
                  这样"见 §5.2 / §8.4"这类跨篇引用不会因拆分而失效
data/           运行时数据与凭据（**绝不入库**，见 credential-boundary.md）
scripts/        门禁与夹具生成器
  gen-*.cjs       —— 夹具一律从真实产物生成，不手写期望值
  check-*.cjs     —— 门禁（标识 / 黄金向量 / json tag）
  check-filenames.js / check-go-deps.js —— 门禁（文件名 / Go 分层。2026-09-22 起
                    门禁体系整体是 zx（node），旧 `.sh` 版已删，见 dev-gates.md 迁移补记）
  gates-go.js / gates-web.js / gates-scope.js —— 编排 / 分类器；install-hooks.js / shots.js 同属这套
  applog-capture.py / reqable-capture.py / reqable-detail-masked.py
                  —— 抓包侧工具（代理通路 + 打印前脱敏）。**逐条命令与前置条件不在这里**，
                     见 capture-tools.md；本文只登记归属
  probe-live-room.js —— 开播窗口的核对清单（权威那份在脚本的 `ITEMS` 里，见坑表 #116）；
                    `--wire` 是抓包前置自检
  asar-extract.cjs —— 从 PC 端 `app.asar` 里 list / cat / grep / save（自己解析头部，不联网）
  probe-hcy-cool-flag.py / crack-cool-flag-response.py —— 活体探针与已知明文攻击（一次性）
  h5-device-id.py —— **取证用**：从 WebView 的 localStorage 里读官方 App 实发的
                     `deviceId`（只读）。日常**不需要它** —— 那个值本项目自己就能算
                     （`mcloud smid refresh`）。留着是为了"拿真值对拍"。
                     带 `--self-test`（合成库往返 + 形状正反例 + 归属排序），改动后跑一遍
test/
  fixtures/     夹具
    legacy/       旧数据夹具（脱敏产物，进版本库）
    http/         `mcloud fixture extract` 现抽的 HTTP 夹具 —— **本机测试数据，不入库**
                  （来源 HAR 含登录凭据，且每人抓到的流量不同；干净与否由 L2 用例的
                  `audit_leaks` 兜底，而不是靠提交前人眼审）
  golden/       与 TS 原实现对拍的黄金向量（进版本库；同级仓库 mcloud-rust
                另存一份副本，两侧守卫脚本**互为交叉验证**）
har/            原始抓包（含凭据，**不入库**）
proto/          （规划中）
```

> **Rust 实现（8 crate workspace、`gen-*.rs` 向量生成器、`gates.sh` 等）已于
> 2026-09-21 剥离到同级仓库 [`../mcloud-rust/`](../../mcloud-rust)。**
> 本仓库不再有 `.rs` 源文件与 `Cargo.toml`。

---

## 文件命名

| 范围 | 规则 | 例 |
| --- | --- | --- |
| 非 Go 文件（文档 / 前端 / 脚本 / 夹具 / JSON） | **kebab-case**：全小写、`-` 分隔，不用空格与下划线 | `dev-gates.md`、`app.tsx`、`gen-auth-vectors.cjs` |
| Go 源文件 | **snake_case**，且 `_test.go` / `_GOOS.go` / `_GOARCH.go` 后缀有工具链语义 | `golden_vectors_test.go`、`store_config.go` |
| 生态固定名 | 不改 | `README.md`、`LICENSE`、`package.json`、`THIRD-PARTY-NOTICES.md` |

Go 那条不是风格偏好而是**工具链强制**：`go test` 只认 `_test.go` 后缀，
构建约束按字面匹配 `_linux.go` / `_amd64.go`，改名即失效。
（Rust 的同款约束 —— 文件名就是模块名 —— 见 `../mcloud-rust/docs/dev-gates.md`「第 5 条」，
那边的 `check-filenames.sh` 副本管着它。）

夹具名由 `mcloud fixture extract` 的 `slugify` 统一
（HAR 名实测带中文 / 空格 / 下划线 / 大写；中文保留，其余折成 `-`）。

### kebab-case 只约束**文件名**，不约束文件里的标识符

这两件事正交，混起来会改错东西：

| 文件 | 文件名 | 里面导出的东西 |
| --- | --- | --- |
| WebUI 页面 | `auth-page.tsx` | `export const AuthPage = …` —— **保持 PascalCase** |
| 脚本 | `check-filenames.js` | 函数名随意（脚本内部按语言惯例，shell 一般 snake_case、js 一般 camelCase） |
| 文档 | `dev-gates.md` | 中文标题随意 |

React 要求**组件名首字母大写**，那是语言 / 框架的硬要求，不受本仓库的文件名约定影响。
所以 `AuthPage.tsx` 要改成 `auth-page.tsx`，但 import 进来的 `AuthPage` 保持不动：

```tsx
import { AuthPage } from './pages/auth-page'   // 路径 kebab，标识符 PascalCase
```

### 这条规则以前只写在文档里，于是被违反了

`web/src/pages/` 下三个 `AuthPage.tsx` / `DashboardPage.tsx` / `PlaceholderPage.tsx`
一路活到被用户点名 —— 违规的表现极其安静：**编译能过、测试全绿、界面正常**，
只有下一个人加文件时"照着旁边那个写"，错误便自我复制。

⇒ 现在由 `node scripts/check-filenames.js` 强制执行 —— 它是**两套门禁共用**的一条
（Go 侧第 6 条；Rust 剥离后其第 5 条归 `../mcloud-rust`，脚本各仓一份），
所以别记成"第几条"，记成"文件名那条"。

### 判据

按 `.` 把文件名拆成**段**，每段必须匹配 `^[a-z0-9]+(-[a-z0-9]+)*$`。

按段判而不是按整名判，是为了让扩展名天然通过：
`gen-auth-vectors.cjs` 拆成 `gen-auth-vectors` / `cjs`，两点名 `foo.d.ts` 拆成 `foo` / `d` / `ts`，
都不必为扩展名写特例。

### 豁免分三类（**不允许因为"改名麻烦"而豁免**）

1. **生态固定名** —— 工具按字面找它们：
   `Cargo.toml` / `Cargo.lock` / `README.md` / `LICENSE` / `THIRD-PARTY-NOTICES.md` /
   `rust-toolchain.toml` / `index.html` / `package.json` / `package-lock.json` /
   `tsconfig.json` / `.gitignore` / `.gitattributes` / `.npmrc` / `SKILL.md` /
   `.github/workflows/*`。
2. **上游原样取出的证据** —— 改名会破坏"文件里能搜到的符号 ↔ 文件名"的对应，
   而这份素材的价值恰恰在于可逐字对照：
   `docs/capture/apiServer.js`、`docs/capture/pcclient-api.js`、
   `test/fixtures/legacy/$meta.json`（被 `../mcloud-rust` 的 `include_str!` 按字面引用，
   也由 `scripts/gen-legacy-fixtures.cjs` 按名生成）。
3. **生成物 / 依赖目录** —— `node_modules/` / `target/` / `dist/`。

新增豁免要同时满足"它属于上述三类之一"与"在 `scripts/check-filenames.js` 的注释里写清属于哪一类"。
