# 项目规范（所有 Agent 开工前必读）

本文件只立**横向规矩**：质量口径、多 Agent 协作、语言与风格。
不涉及具体业务，也**不复制**任何专门文档的内容 —— 各领域以下表为准，本文件与其冲突时以它们为准；与用户当前指令冲突时以用户为准，并同步修订本文件。

## 0. 真相源地图（引用，不复制）

| 领域 | 唯一权威 |
| --- | --- |
| 门禁与分层铁律 | [`docs/dev-gates.md`](docs/dev-gates.md) |
| 迁移决策与对拍口径（与其它文档冲突时以它为准） | [`docs/go-migration-plan.md`](docs/go-migration-plan.md) |
| 实测判据登记表 #1–#100（踩过的坑） | [`docs/migration-pitfalls.md`](docs/migration-pitfalls.md) |
| 文档索引与目录约定 | [`docs/README.md`](docs/README.md)、[`docs/repo-layout.md`](docs/repo-layout.md) |
| WebUI 视觉 / 组件 / 代码约定 | [`docs/webui-design-guide.md`](docs/webui-design-guide.md)、[`docs/webui-code-guide.md`](docs/webui-code-guide.md)、[`docs/webui-components.md`](docs/webui-components.md) |
| 进度与实况记录 | [`docs/status.md`](docs/status.md) |

同一条事实只写一处，别处放链接。发现两处成文且不一致 ⇒ 报告，不要静默"对齐"。

## 1. 质量：什么才算"完成"

1. **门禁**：Go 九条 `node scripts/gates-go.js`，web 两条 `node scripts/gates-web.js`（2026-09-22 起整套门禁脚本已从 bash 迁到 zx，入口随仓库根 `package.json` 的 zx 依赖走，克隆后先 `npm install`）；逐条明细看**当轮 stdout**（或每条一份的 `target/gates-*-<name>.log`）。⚠ `target/gates-go-last.log` 只留**最后一次失败**的现场 —— 全绿时脚本末尾自己把它清空，别拿它的空不空当"跑没跑"的证据（web 侧那份不清空，两条故意不一致）。pre-commit 按暂存范围只跑静态子集，**build / test / golden 不在其中** —— 提交前必须手动全量跑过。
2. **全绿才提交**：不提交编译不过的树；不用 `--no-verify` 绕钩子（钩子红了先修根因）。`gates-go.js` 横幅会写明"本次 N/9"，抄进 `status.md` 时**照抄这个 N**，禁止把子集写成"九项全绿"。
3. **每个修复带测试位**：说得出"反向改坏它，哪条测试会红"。造不出反例的分支（如不可测的兜底判断）必须在注释里声明"未被断言"，不得因"没红过"当作不重要。
4. **对拍面冻结，内部风格自由**：本项目的对拍基准是仓库内的**黄金向量与契约文档**（`test/golden/*.json`、[`docs/signin-cli-spec.md`](docs/signin-cli-spec.md)、[`docs/cli-usage.md`](docs/cli-usage.md)），不是任何外部语言实现。线格式、JSON 字段名、CLI 参数面与退出码、黄金向量是**冻结基准** —— 改这些行为（含大小写敏感度、可接受输入集、转义、缩进字节）前，先对上面这些基准核实并跑 golden 门禁；int 宽度、参数写法、文件布局**不是**对拍面，按 Go 惯例即可。
5. **新文件类型 / 无 tag 结构体必须显式登记**：进 `TEXT_EXT` 或 `NOT_SCANNED_EXT`（写理由）；json tag 白名单同理。"加了却不登记"会表现为**扫了并通过**。
6. **web 侧只有 typecheck + build**：没有测试运行器。页面正确性只能真机（或浏览器）核验；核验过才写"已验证"，更不得写成"有测试了"。
7. **结论先核实再落笔**：断言"缺陷 / 必须改"之前查四件事 —— 上游支持吗、是否只是旧实现的偷懒、该行为今天还活动吗、判据的数字出自哪个基准。核实不了就写成疑问，不把推断当缺陷。

## 2. 协作：多个 Agent 共享同一个工作树

**前提认知：worktree 与 git 索引都是共享的。** 别人未提交的改动随时在你的 `git status` 里。

1. **开工先看现场**：`git status` 有他人改动（尤其已暂存）⇒ 你的成果**留在工作树**，等对方落定再谈提交；不替在飞的改动定格 commit，不"顺手"把别人的文件 stage 或 commit。
2. **提交只点名自己的文件**：`git add <具体路径>`，禁 `git add -A` / `git add .`。提交信息格式沿用 `git log` 风格（`类型(范围)：中文描述`）。
3. **禁损人操作**：裸 `git stash`（如必须，用唯一 tag、精确恢复自己那一份）、`git reset --hard` / `checkout .` / `git clean` 扫别人的改动、kill 别人的进程（如 8788 上的 server）、删别人的未跟踪文件（新文件可能是别人在写的测试）。
4. **端口与共享数据**：起服务前 `lsof -i :<port>` 核实**二进制与库归属**再占（8787 常被 smoke 版占用，Go 版惯例 8788）；不覆写他人正在使用的 `data/*.db`。
5. **撞车处理**：同一处被并发改 ⇒ 当天提交 + rebase；冲突时保留**已推送到远端**的 API 形状。
6. **记录只增改不删**：`status.md`、门禁注释、坑登记是**追加**性质的历史。修订前先核对代码与抓包；不因"判旧记录写错了"而重写，用**补记**修正。
7. **并行时验证要指名范围**：跑测试 / 门禁说清测的是哪些包，不把"我改的没红"表述成"全仓没红"（别人的在飞改动可能自己就是红的）。

## 3. 语言与风格

1. **文字一律简体中文**：注释、文档、commit message、用户可见的错误串与帮助文本。代码标识符用英文（Go 惯例的导出/非导出 + camelCase）。
2. **注释写 WHY 不写 WHAT**；本仓库惯用长注释承载"为什么这么裁"与"删掉会怎样"。新注释与代码矛盾 ⇒ 先修矛盾再加码，不留"注释说 A 代码做 B"。
3. **文件名 kebab-case** 由门禁强制；新增一类文件后缀要先过 §1.5。
4. **依赖策略**：Go 侧保持零第三方新增（`go.mod` 现外者除外）；`go vet` 只覆盖"几乎肯定是错"的一小撮检查，**不等于**完整静态检查，不引入 `golangci-lint`。要加依赖先经用户拍板。
5. **工具脚本一律用 zx（JavaScript）**：仓库有 bash / python / javascript 三种脚本并存，**新写的脚本统一用 zx**（根 `package.json` 已钉 zx 依赖，克隆后先 `npm install`），不再新增 bash / python 脚本以免三头扩散。2026-09-22 已完成收口：**整套门禁体系（gates-go / gates-web / gates-scope / check-filenames / check-go-deps / install-hooks / shots）都是 zx**；剩下的 bash 只有 `.githooks/pre-commit`、`pre-push` 两个薄壳 —— git 只认固定文件名且要 x 位，壳内**只准**做"调 .js + 汇总退出码"，逻辑写进 .js。
6. **仅存的 bash 只写 POSIX + bash 3.2 可跑的语法**（macOS 自带 3.2.57；`declare -A` / `mapfile` 会假绿）。
7. **web**：Vue 3 TSX + daisyUI/Tailwind；类名必须**字面量**成文（拼接式 Tailwind 静默失效）；断点、单列形态、空态文案按 `webui-design-guide.md`，不改先方案后动手。
8. **术语沿用既成**：对拍、门禁、组装根、工作树、档位等，不生造新词、不回译英文。
9. **长文件主动拆**：文档约 300+ 行、源文件上千行 ⇒ 先提拆分方案再动；交付时主动报告偏大文件。

## 4. 本文件自身

发现规范与现实不符（规则没人执行、门禁已改档、术语变了）⇒ **提出修订**，不静默绕开。新增规则须带"从哪次事故来"，否则只进 §0 表格指回原权威文档。
