# 目录结构与文件命名

> 状态：**规范**（加文件前必读；命名由 `bash scripts/check-filenames.sh` 强制执行）
>
> - 相关：[`dev-gates.md`](./dev-gates.md)（门禁总表；本文的规则 = Rust 侧第 5 条 / Go 侧第 6 条）

---

## 目录结构

```
crates/         8 个 crate（分层见 dev-gates.md）
scripts/        门禁与夹具生成器
  gen-*.cjs       —— 夹具一律从真实产物生成，不手写期望值
  check-*.cjs     —— 门禁（分层 / 标识 / 黄金向量）
  check-*.sh      —— 门禁（分层 bash 版 / 文件名）
  applog-capture.py / reqable-capture.py —— 抓包侧工具
  h5-device-id.py —— **取证用**：从 WebView 的 localStorage 里读官方 App 实发的
                     `deviceId`（只读）。日常**不需要它** —— 那个值 Rust 自己就能算
                     （`mcloud smid refresh`）。留着是为了"拿真值对拍"。
                     带 `--self-test`（合成库往返 + 形状正反例 + 归属排序），改动后跑一遍
test/
  fixtures/     夹具
    legacy/       旧数据夹具（脱敏产物，进版本库）
    http/         `mcloud fixture extract` 现抽的 HTTP 夹具 —— **本机测试数据，不入库**
                  （来源 HAR 含登录凭据，且每人抓到的流量不同；干净与否由 L2 用例的
                  `audit_leaks` 兜底，而不是靠提交前人眼审）
  golden/       与 TS 原实现对拍的黄金向量（进版本库）
web/            WebUI：Vue Vapor（无虚拟 DOM）+ TSX + daisyUI
proto/          （规划中）
```

---

## 文件命名

| 范围 | 规则 | 例 |
| --- | --- | --- |
| 非 Rust 文件（文档 / 前端 / 脚本 / 夹具 / JSON） | **kebab-case**：全小写、`-` 分隔，不用空格与下划线 | `rust-rewrite-plan.md`、`app.tsx`、`gen-auth-vectors.cjs` |
| Rust 源文件 | **snake_case** | `test_support.rs`、`reqwest_client.rs` |
| 生态固定名 | 不改 | `README.md`、`LICENSE`、`Cargo.toml`、`THIRD-PARTY-NOTICES.md` |

Rust 那条不是风格偏好而是**语言强制**：文件名就是模块名，而 `mod` 只接受标识符，
`mod my-mod;` 不合法。

夹具名由 `mcloud fixture extract` 的 `slugify` 统一
（HAR 名实测带中文 / 空格 / 下划线 / 大写；中文保留，其余折成 `-`）。

### kebab-case 只约束**文件名**，不约束文件里的标识符

这两件事正交，混起来会改错东西：

| 文件 | 文件名 | 里面导出的东西 |
| --- | --- | --- |
| WebUI 页面 | `auth-page.tsx` | `export const AuthPage = …` —— **保持 PascalCase** |
| 脚本 | `check-filenames.sh` | 函数名随意（shell 一般 snake_case / 小写） |
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

⇒ 现在由 `bash scripts/check-filenames.sh` 强制执行 —— 它是**两套门禁共用**的一条
（Rust 侧第 5 条 / Go 侧第 6 条），所以别记成"第几条"，记成"文件名那条"。

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
   `test/fixtures/legacy/$meta.json`（被 `include_str!` 按字面引用，
   也由 `scripts/gen-legacy-fixtures.cjs` 按名生成）。
3. **生成物 / 依赖目录** —— `node_modules/` / `target/` / `dist/`。

新增豁免要同时满足"它属于上述三类之一"与"在 `scripts/check-filenames.sh` 的注释里写清属于哪一类"。
