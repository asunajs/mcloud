# findings-261004-docs

> ⚠ **2026-10-04 追记**：本文件 dc-1…dc-19 **已逐条处理**（含 dc-14/dc-17/dc-19 这三条脚本/CI 修复）——
> 处理清单与逐条落点见 `docs/task-log.md` 第 88 行 ✓。本文件正文**保持当时的样子**（那是快照，
> 不是待办表）；要复核"现在还是不是这样"，照每条的「复现」命令重跑一遍 ✓。

- 基线: HEAD `2ca9cf7`
- 范围: docs/, README.md, AGENTS.md, scripts/, .github/
- 方法:
  - 通读: `docs/usage.md`、`docs/task-log.md`、`README.md`、`AGENTS.md`、`scripts/*`（dev-reload / run-local / sign-local / gates-rust / gates-web.mjs / install-git-hooks / hooks/*）、`.github/workflows/{ci,release}.yml`、`docs/app-task-status.md` 头部与主表、`docs/task-inventory.md` 头部、`docs/conventions.md` §0/§9.2/速查、`docs/web/lite-framework.md` 头部、`.gitignore`。
  - 机器核查（全部只读）:
    - 版本: `grep -n '^version' Cargo.toml web/package.json web/package-lock.json` + `Cargo.lock`（**四处一致 `3.0.0-rc.2`** ✓，但 `:3000` 上跑的是 `3.0.0-rc.1`，见 dc-3 备注）。
    - 全库文件引用存在性: 自写脚本抽出 docs/README/AGENTS/scripts/.github/workflows 里所有 `path[:line]` 形状引用，逐个 `os.path.exists` + 按 basename 全仓回查（`crates/mcloud-server/src/bin/cli.rs` 等真死引用即由此确认）。
    - `../` 相对引用: 抽出所有 `../...` 逐个验存在性（发现了 `../mcloud` 的真实身份，见 dc-4）。
    - task-log 结构: 用脚本按"未转义 `|`"切列，统计每行字段数与 `#` 编号（结论：**列数全一致、编号 1..82 连续无重复**），并抽出状态列与 `第 N 行` 指针。
    - 代码反证: `grep`/`sed -n` 对 `crates/mcloud-tasks/src/{lib.rs,app_task.rs}`、`crates/mcloud-server/src/{main.rs,cli.rs,cli/run.rs}`、`crates/mcloud-core/src/pipeline.rs`、`crates/mcloud-upstream/src/live_consts.rs`、`crates/mcloud-tasks/src/live_room.rs`、`web/package.json`、`web/vite.config.ts`、`web/scripts/*`、`web/lite/regress/*`、`.git/hooks/`、`~/Library/LaunchAgents/*.plist`。
    - 复现实验（只读/无副作用）: `bash -n scripts/*.sh`；`bash -c 'set -euo pipefail; LAUNCHD_LABEL="${MCLOUD_LAUNCHD_LABEL:-$(launchctl list …)}"'`；`bash -c 'set -euo pipefail; X="${UNSET:-$(false)}"'`；`node -e "require('fs').readdirSync('web/node_modules/daisyui/components')"`；`curl -s :3000/api/version`。
    - **未跑**（遵纪律）: 任何 `cargo`/`npm`、`scripts/dev-reload.sh`、`scripts/install-git-hooks.sh`、`scripts/run-local.sh`；未 `kill` 任何进程、未动 `:3000`。

## 摘要（≤10 行）

1. 最要紧的是 **`docs/usage.md` 整篇还在叫 `mcloud-server` / `mcloud-cli`**，而发布包里只有 `mcloud` —— 它是 Release 说明正文里**链接过去**的那份文档（`release.yml:423`），下载用户照着敲直接 `No such file`。
2. `README.md` 的 **exchange 说明是反的**（写着"已从 `register_all` 摘掉"，实际 `lib.rs:86` 已注册且哨兵用例钉着），同处"19 条在注册表里"也已变 20。
3. `AGENTS.md:23-24` 断言"本机 `../mcloud/` 仍是 Go 侧检出 ⇒ 那些 `../mcloud/…` 路径照旧可解析"**不成立**：`../mcloud` 现在是**本仓自己的一份同 HEAD 副本**，Go 侧在 `../mcloud-back`。⇒ 全库 7 处 `../mcloud/docs|capture|internal|har` 证据链全断。
4. `docs/app-task-status.md` 的 **"落点 path:line" 整列已系统性失效**（`step_for` 写 `:251`，实际 `:621`；`SKIP_TASK_LIST` 写 `:200`，实际 `:570`；`entry_do_task` 写 `:866`，实际 `:1363`）。
5. "现算"命令也有跑不动的：`web/scripts/daisyui-size-probe.mjs` 依赖已被删掉的 `node_modules/daisyui`，而 `lite-framework.md:16/:22` 正是拿它当 CSS 体积的现算入口。
6. 两条"照抄就报错"的命令行: `--bin mcloud-cli`（`README.md:186-190`、`conventions.md:743`）、`grep … crates/mcloud-server/src/bin/cli.rs`（`usage.md:129`、`architecture.md:1328`）。
7. 门禁口径有两处自相矛盾: `ci.yml` 说"五条前端闸门里 CI 只覆盖这三条"（现为 10 条、CI 覆盖 3 条、**7 条本地**），`lite-framework.md:19` 还停在"默认只跑 ready"的单档时代。
8. `conventions.md` §9.2（自称硬约束）的 clippy 命令**缺 `-D warnings`**，而仓里 `[workspace.lints]` 只把 clippy 设成 `warn` ⇒ 这条门实际上拦不住新告警。
9. `scripts/run-local.sh:72` 与 `dev-reload.sh:37` 是**同一个"`set -e` + `launchctl list` 静默死"**，前者没修（本沙箱实测：无输出、exit 1）。
10. `docs/task-log.md` 结构本身是干净的（编号 1–82 连续唯一、表格列数一致），但**状态列有 9 行越出自己定的四种**，另有 3 处 `mcloud-gost` 残留。

## 发现

### dc-1 [高] `docs/usage.md` 通篇的二进制名是废弃的 `mcloud-server` / `mcloud-cli`，而发布包里只有 `mcloud`
- 位置: `docs/usage.md:7`（另见 `:9` `:10` `:50` `:51` `:79` `:92` `:108` `:119` `:122` `:148` `:167` `:176`）
- 证据:
  - 文档原文（`:3` / `:7` / `:50`）:「整包里**只有一个可执行文件**」→ 表格却写 `` | `mcloud-server` | 服务本体… | ``；「`./mcloud-server            # Linux / macOS`」。
  - 代码反证: `crates/mcloud-server/Cargo.toml:17-19` 只有一个 bin，`name = "mcloud"`；`release.yml:234/238` 编的是 `--bin mcloud`，`:343` `cp "$BIN_DIR/mcloud"`，`:300` 冒烟跑的也是 `mcloud`；`:411` Release 正文写「包内**只有一个可执行文件 `mcloud`**」，而 `:423` 的"完整说明"链接**就是这份 `docs/usage.md`**。
  - 文档内部互斥: `:132` 已经写着「**只有一个二进制 `mcloud`**（2026-10-03 起…合并了）」，与 `:148`「实际二进制叫 `mcloud-server` / `mcloud-cli`」直接对打。
- 复现: `grep -n 'mcloud-server\|mcloud-cli' docs/usage.md`
- 影响: 下载 Release 的人按 `./mcloud-server` / `xattr … ./mcloud-server` / systemd `ExecStart=/srv/mcloud/mcloud-server` 全部失败；launchd/systemd 两段配置粘出去就是起不来的单元。后续 agent 若"以 usage.md 为准"也会把名字写回错的一边。
- 建议: 把 `mcloud-server` → `mcloud`（含 `.exe`、`ExecStart`、`ProgramArguments`、`xattr`、`MCLOUD_ADDR=… ./mcloud`）；`:9`/`:10`/`:119`/`:122` 那段"CLI 不进发布包"整体改写为"CLI 就在同一个 `mcloud` 里，带子命令即是"；`:122` 注释 `# 产物：target/release/mcloud-cli` → `target/release/mcloud`。
- 状态: 已实测 · 置信度 高

### dc-2 [高] `docs/usage.md:147-150` 那段"帮助文本是旧的"已是**反向错误**（帮助文本 2026-10-03 就修好了）
- 位置: `docs/usage.md:147-150`
- 证据:
  - 文档原文:「⚠ **帮助文本里的名字与清单是旧的**：它写的是 `mcloud <命令>` 与 `mcloud serve`，而且 login 那一行**漏了 `device`**。实际二进制叫 `mcloud-server` / `mcloud-cli`，服务**没有 `serve` 子命令**…（这些是代码侧注释/文案的欠账，不在这份文档里改。）」
  - 代码反证: `crates/mcloud-server/src/cli/run.rs:197` 打的是「（不带参数 = 起 WebUI 服务…）」；`:196` 是「用法：mcloud <命令> [参数]」；`:184` 的 login 行**含 `device`**（`login device show|reset`，`:183`）；`:231-233` 的注释明写「这里原先教用户跑 `mcloud serve` —— 那时**没有这个子命令**（2026-10-03 review §1.15）… 仍然**没有** `serve` 子命令」。
  - 交叉: `docs/task-log.md:146`（#30）把"CLI 命令数/`login device`/不存在的 `mcloud serve`"列为**已修并已上线**的那批之一。
- 复现: `sed -n '180,200p' crates/mcloud-server/src/cli/run.rs`
- 影响: 读者会以为 CLI 帮助在说谎、并据此认定二进制名是 `mcloud-server`/`mcloud-cli`（与 dc-1 互相加固）；"欠账"其实已还，这一段会把已经正确的帮助文本判成错的。
- 建议: 整段删掉或改写成"帮助文本已与 `mcloud <命令>` 对齐（`:196`）"。
- 状态: 已实测 · 置信度 高

### dc-3 [高] `README.md` 的 exchange 说明是反的；"19 条在注册表里"也已变 20
- 位置: `README.md:26-31`、`README.md:9`
- 证据:
  - 文档原文 `:26-28`:「⚠ **`exchange`（兑换）的实现还在，但已从 `register_all` 摘掉，不算「对外可用」**：它**不在** `/api/capabilities`、也**不能**经 `POST /api/runs` 跑；专用入口 … 会如实回一句「任务池里没有 exchange」。」`:31`:「兑换页今天**读得到奖品清单、点不下单**」。`:9`:「**已落地 20 个任务，其中 19 条在注册表里**」。
  - 代码反证: `crates/mcloud-tasks/src/lib.rs:86` `Arc::new(exchange::ExchangeTask),`（其上方 `:83-85` 注释写「`exchange` 2026-10-02 起**在这里**：三条缺口已补齐… 用户点「兑换」能真的走完整条链」）；哨兵用例 `:126 register_all_exposes_exchange` 断言它必须注册；`grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs` = **20**（全部在 `register_all` 的 vec 里，`:97-99` 逐个 register）。
  - 同仓已改对的地方: `docs/task-inventory.md:32-35` 有「**改口（当前事实，2026-10-02）**：`exchange` **已回到 `register_all`**」；`docs/task-log.md:59` 的 rc.1 现场日志是 `tasks=20`。
  - 历史: `git log -S'已从 `register_all` 摘掉' --oneline -- README.md` ⇒ 只有 `99d10d1`（2026-10-01，早于 `625a1f9` 把它加回来）⇒ 从未更新。
  - 备注（另一件独立事实）: `curl -s http://127.0.0.1:3000/api/version` ⇒ `{"name":"mcloud","version":"3.0.0-rc.1"}`，而 `Cargo.toml:15` / `web/package.json:3` 已是 `3.0.0-rc.2` ⇒ **`:3000` 上跑的不是 rc.2**（"已发布"≠"已上线"）。
- 复现: `sed -n '83,86p;126,134p' crates/mcloud-tasks/src/lib.rs && grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs && git log -S'摘掉' --oneline -- README.md`
- 影响: 读者被明确告知兑换"点不下单"，而任务页/兑换页/`POST /api/runs` 其实都能跑它（`task-log` #22/#23/#30 的实机读数也以此为背景）；台账里 rc2 与线上 rc1 的差别也会被"已上线"口径糊过去。
- 建议: 删掉 `:26-31` 那段（或改写成"2026-10-02 起已回到注册表"），`:9` 改成 20/20；顺带在台账/AGENTS 里点明 `:3000` 仍是 rc.1（重编后才变）。
- 状态: 已实测 · 置信度 高

### dc-4 [高] `AGENTS.md` 断言 `../mcloud/` 仍是 Go 侧检出 —— 它其实是本仓自己的副本；全库 `../mcloud/…` 证据链因此全断
- 位置: `AGENTS.md:23-24`；断链清单: `docs/task-inventory.md:9` `:170` `:239` `:349` `:399`、`docs/conventions.md:671`、`README.md:134`、`docs/t5-orchestration-notes.md:3`
- 证据:
  - 文档原文 `AGENTS.md:23-24`:「本机 `../mcloud/` 目录仍是 Go 侧的检出，所以文档里那些 `../mcloud/…` 相对路径照旧可解析。」
  - 反证: `git -C ../mcloud rev-parse HEAD` = `2ca9cf7fe505f77f56f5b8b80eb2b78e22024653`（**与本仓 HEAD 逐字相同**）、`git -C ../mcloud remote -v` = `asunajs/mcloud`、`ls ../mcloud` = `Cargo.toml crates web rustfmt.toml …` ⇒ 那是**本仓的 Rust 检出**，不是 Go。Go 侧在 `../mcloud-back`（`go.mod` / `internal/` / `har/` 都在）。
  - 目标文件实测: `../mcloud/docs/business-feature-inventory.md` **不存在**、`../mcloud/docs/capture/` **不存在**、`../mcloud/har/` **不存在**、`../mcloud/internal/app/` **不存在**；而这些在 `../mcloud-back/` 下**都存在**（`ls ../mcloud-back/docs/` 有 `business-feature-inventory.md` / `capture/` / `pending-decisions.md`，`ls -d ../mcloud-back/har` 有）。
  - 全库 `mcloud-back` 命中数 = **0**（`grep -rn 'mcloud-back' docs/ README.md AGENTS.md` 无输出）。
- 复现: `git -C ../mcloud rev-parse HEAD; ls ../mcloud-back/go.mod; ls ../mcloud/docs/business-feature-inventory.md`
- 影响: 这是**给 agent 的工作手册**，它把"路径可解析"写成事实 ⇒ 后续 agent 会照着断链去找抓包素材/规格（`business-feature-inventory.md`、`docs/capture/*.txt`、`pending-decisions.md`、`internal/app/live_room*.go`），白跑或编造来源；`docs/t5-orchestration-notes.md` 整篇的立论（"我读 `../mcloud/internal/app/live_room*.go` 后提炼"）也失去可核对的底本。
- 建议: `AGENTS.md:23-24` 改成「本机 Go 侧检出是 `../mcloud-back/`（`../mcloud/` 是本仓的一份同 HEAD 副本）」；再把上述 7 处 `../mcloud/` 换成 `../mcloud-back/`（逐条确认存在再改），或统一改成 `git show go-legacy^:<路径>` 这种不依赖本机目录的形状。
- 状态: 已实测 · 置信度 高

### dc-5 [高] `docs/app-task-status.md` 的"落点 path:line"整列已系统性失效
- 位置: `docs/app-task-status.md:24`（§1 表头）、`:26-30`（五道关）、`:40`（106 行）等主表各行的"落点"格
- 证据（文档 : 实际）:
  - §1 关 5 `step_for` 写 `:251` → 实际 `grep -n 'pub fn step_for' crates/mcloud-tasks/src/app_task.rs` ⇒ **621**（`:251` 处是 `///` 注释）。
  - §1 关 2 `SKIP_TASK_LIST` 写 `:200` → 实际 **570**；`:200` 是注释「我们的 `TaskV2Item` 里没有 `button` 那一格」。
  - §1 关 3 `skip_tasks` 写 `:222` → 实际 `is_configured_skip` 在 **592**；`:222` 是 `fn clean_task_name`。
  - §1 关 1 `state == "FINISH"` 写 `:290` → `:290` 是 `if beiyong1_button_gate(…)`；FINISH 判据在 `should_click`（**697**）。
  - §1 关 4 月门写 `:409` → `:409` 是 `.and_then(|it| it.name.as_deref())`；月门函数是 `month_gate_skips`（**787**）。
  - §2 106 行 `:148`（`upload_shape_for` → `UPLOAD_ONE`）→ `UPLOAD_ONE` 在 **92**、`upload_shape_for` 在 **262**、`:148` 是文档注释 `/// }`。
  - §2 615 行 `UPLOAD_TEN :113` → 实际 **133**；`entry_do_task :866` → 实际 **1363**；`share_first_uploaded` → 实际 **1257**；`should_click :325-331` → 实际 **697**。
  - 只有 `beiyong1_button_gate :157` 是对的（`app_task.rs:157`）。
  - 同一批旧行号也出现在 `docs/task-log.md:181`（`app_task.rs:245` 的 `step_for`）。
- 复现: `grep -n 'pub const UPLOAD_ONE\|pub fn upload_shape_for\|pub const SKIP_TASK_LIST\|pub fn is_configured_skip\|pub fn step_for\|pub fn should_click\|async fn entry_do_task\|async fn share_first_uploaded' crates/mcloud-tasks/src/app_task.rs`
- 影响: 这份文档的**唯一价值**就是"落点 `path:line` 证据"（文件头与 `task-inventory.md:3-6` 都这么分工）。现在每条都指错地方 —— 读者点过去看到的是一段无关注释，会得出"实现根本没写"的相反结论。
- 建议: 要么把整列换成"符号名 + 现算命令"（`grep -n 'pub fn step_for' …`，与本仓其它文档"别背数"的取向一致），要么按上面这组实际行号重钉一遍，并把"行号会漂"写进文件头。
- 状态: 已实测 · 置信度 高

### dc-6 [中] `--bin mcloud-cli` / `src/bin/cli.rs` 这两条"照抄就报错"的路径与命令
- 位置: `README.md:186-190`、`docs/conventions.md:743`；`docs/usage.md:125` `:129`、`docs/architecture.md:1327-1328`、`docs/design/cli-webui-design.md:525`
- 证据:
  - bin: `crates/mcloud-server/Cargo.toml:17-19` 只有 `[[bin]] name = "mcloud"`；`crates/mcloud-server/src/bin/` **目录不存在**（`ls crates/mcloud-server/src/bin` ⇒ No such file or directory）。⇒ `cargo run -p mcloud-server --bin mcloud-cli -- tasks` 必报 no bin target。
  - 文件: `grep -n '^        Some("' crates/mcloud-server/src/bin/cli.rs` ⇒ `grep: crates/mcloud-server/src/bin/cli.rs: No such file or directory`（exit 2）；把路径换成 `crates/mcloud-server/src/cli/run.rs` ⇒ **7 行**（`docs/design/cli-webui-design.md:29-30` 用的就是这条正确路径）。
  - 相关: `docs/conventions.md:237` 已经写明"（合并前的 `src/bin/cli.rs`）"✓，只有上面几处没跟上。
- 复现: `grep -n 'name = "mcloud"' crates/mcloud-server/Cargo.toml; grep -n '^        Some("' crates/mcloud-server/src/bin/cli.rs; echo "exit=$?"`
- 影响: README 的"构建与运行"是新手第一条路，5 条 CLI 示例全废；`usage.md:129` 与 `architecture.md:1328` 的"现算命令"返回的是 grep 的错误而不是条数（`architecture.md:1325` 标题也还叫 `mcloud-cli`）。
- 建议: `--bin mcloud-cli` → `--bin mcloud`（CLI 现在是同一个 bin 加子命令）；两处 `src/bin/cli.rs` → `src/cli/run.rs`（或 `src/cli.rs`）；`architecture.md:1325` 标题去掉 `mcloud-cli`。
- 状态: 已实测 · 置信度 高

### dc-7 [中] 被当作"现算入口"的 `web/scripts/daisyui-size-probe.mjs` 已经跑不动（依赖已删的 `node_modules/daisyui`）
- 位置: `docs/web/lite-framework.md:16`、`:21-23`；`docs/daisyui-removal-analysis.md:25`、`:133`
- 证据:
  - 文档原文 `lite-framework.md:16`:「| 主产物 CSS（Tailwind + daisyUI） | **别背数** | `node web/scripts/daisyui-size-probe.mjs`（跑一次 ≈6 s，自带逐字节自检） |」；`:22`「跑 `node web/scripts/daisyui-size-probe.mjs` 现算」。
  - 脚本反证: `daisyui-size-probe.mjs:353` `readdirSync(join(WEB, 'node_modules/daisyui/components'))`；而 `ls web/node_modules/daisyui` ⇒ No such file；`grep -c daisyui web/package-lock.json` ⇒ **0**；`web/package.json` 的 devDependencies 只剩 tailwindcss / @tailwindcss/vite / oxc-parser / ts-preview / vite；`web/src/app.css` 里 `@plugin "daisyui"` 也只剩历史注释（`:13` `:18` `:367`）。
  - 实际复现: `node -e "require('fs').readdirSync('web/node_modules/daisyui/components')"` ⇒ `Error: ENOENT: no such file or directory, scandir 'web/node_modules/daisyui/components'`。
- 影响: 谁按"别背数、现算"去量 CSS 体积都会立刻撞 ENOENT；`lite-framework.md` 那张表里"主产物 CSS"一行因此**没有可用的量法**，读者会误以为自己在量一个还存在的东西（daisyUI）。
- 建议: `lite-framework.md:16` 的量法换成 `cd web && npm run build` 末行读数（`web/scripts/gzip-sidecars.mjs` 已经在打）；`:16` 标题去掉 `+ daisyUI`；`daisyui-removal-analysis.md` 是**历史分析**，在 `:25` 明确标注"探针依赖当时的 daisyui 依赖，撤依赖后已不可跑"。
- 状态: 已实测 · 置信度 高

### dc-8 [中] `ci.yml` 的前端闸门口径与计数过期（"五条…只覆盖这三条"，现为 10 条 / 覆盖 3 / 本地 7）
- 位置: `.github/workflows/ci.yml:5-6`、`:97-100`
- 证据:
  - 文档原文 `:97-100`:「⚠ 五条前端闸门里 CI 只覆盖这三条（typecheck / build / 负例）：**窄屏布局验收**与**真 DOM 验收**都要无头 Chrome… ⇒ 那两条是**本地提交前**的门」；`:5-6` 同样说法。
  - 代码反证: `scripts/gates-web.mjs:82-115` 的 `GATES` 共 **10** 条 —— commit 档 6 条（typecheck / build / 编译期负例 / lite 真 DOM 验收 / 编译产物行为+耗时护栏 / 交互验收 ready），push 档再加 4 条（narrow-320-375 / login / setup / 泄漏量测）。CI 的 web job（`:82-102`）只跑 typecheck、build、`lite/regress/compiler.mjs` ⇒ **漏的本地门是 7 条**，而文档只点了 2 条（"真 DOM 验收"与"窄屏"），漏掉 demo/bench/ready 之外的 `bench`（编译产物行为）、`ready`、`login`、`setup`、`泄漏量测`。
  - 同文件内 `scripts/gates-web.mjs:34-35` 的说法是对的（"CI 只覆盖不依赖浏览器的三条"）⇒ 是 ci.yml 的"五条"这一层旧。
  - 顺带同类计数漂移: `AGENTS.md:329`「94 个含 `#[cfg(test)]` 的文件」→ `grep -rl '#\[cfg(test)\]' crates | wc -l` ⇒ **97**（同句的"8 个集成测试目标" ✓ 正确）。
- 复现: `grep -n 'tier:' scripts/gates-web.mjs | wc -l; sed -n '82,115p' scripts/gates-web.mjs; grep -rl '#\[cfg(test)\]' crates | wc -l`
- 影响: 读者会以为"CI 绿了 ⇒ 前端只有 2 条没验"，实际另有 5 条（含泄漏与 login/setup 两条认证态）从没在 CI 跑过；这正是"上线后才发现"的温床。
- 建议: 把"五条"改成"十条（提交档 6 + 推送档 4）"，把未覆盖的 7 条逐个列出（现在只列 2 条）；`AGENTS.md:329` 的 94 改成现算命令或 97。
- 状态: 已实测 · 置信度 高

### dc-9 [中] `lite-framework.md` 的门禁一行还停在"单档 + 只跑 ready"的旧口径
- 位置: `docs/web/lite-framework.md:19`
- 证据:
  - 文档原文:「| 门禁（默认 / `--full`） | 默认只跑 `ready` 一个认证态（改脚本时的快回路）；`--full` 跑满三个认证态 + 一条前端泄漏量测。… | `node scripts/gates-web.mjs` |
  - 代码反证: `scripts/gates-web.mjs:53` `MODE = --push|--full ? 'push' : 'commit'`；`:118` commit **6 条**（含 typecheck/build/负例/demo/bench/ready，不是"只跑 ready"）；push 档再加 narrow + login + setup + leak（共 10 条）。`docs/conventions.md:582-596` 与 `AGENTS.md:302-308` 都已是"两档"的新口径 ⇒ 只有这一处旧。
- 复现: `sed -n '17,20p' docs/web/lite-framework.md; sed -n '117,119p' scripts/gates-web.mjs`
- 影响: 想"改运行时或编译器"的人（本文件正是给他们看的）会以为默认门禁不含 demo/bench/类型/构建 —— 于是少跑 5 条自查，最容易漏的恰好是它自己的框架断言。
- 建议: 该行改成"`--commit`（默认，6 条：类型/构建/负例/demo/bench/ready）；`--push` 10 条（再加 320/375、login、setup、泄漏量测）"。
- 状态: 已实测 · 置信度 高

### dc-10 [中] `conventions.md` 三处 clippy 命令都缺 `-D warnings`，而仓里 clippy 只设成 `warn`（§9.2 还自称【硬约束】）
- 位置: `docs/conventions.md:578`（§9.2 命令块）、`:209`（§3.3）、`:741`（速查）
- 证据:
  - 文档原文 `:578`「`cargo clippy --workspace --all-targets`」（**无** `-- -D warnings`），其上是「### 9.2 提交前自检【硬约束】」、其下是「四条全绿才能提交」；`:209-211`「`cargo clippy --workspace --all-targets`」+「**当前 0 告警**（含测试代码），因此可以直接当硬门禁」；`:741` 速查同形。
  - 反证: `Cargo.toml:49-56` 的 `[workspace.lints.clippy]` 是 `all/style/suspicious/complexity/perf = "warn"`（**不是 deny**）⇒ 少写 `-D warnings` 就是"只打印、不失败"。而同一道门在别处都写了: `AGENTS.md:298`、`.github/workflows/ci.yml:174`、`release.yml:169`、`scripts/gates-rust.sh:93` 全是 `-- -D warnings`。
- 复现: `grep -n 'cargo clippy' docs/conventions.md; sed -n '49,56p' Cargo.toml; grep -rn 'clippy --workspace --all-targets' AGENTS.md .github/workflows/*.yml scripts/gates-rust.sh`
- 影响: 照着 §9.2 逐条敲的人会得到"clippy 有输出但退出码 0"的假绿；CI 与本地口径不一致时，"本地绿、CI 红"就回来了（`ci.yml:141-157` 那段长注释防的正是这个）。§3.3 那句"因此可以直接当硬门禁"也建立在同一个漏掉的 flag 上。
- 建议: 三处都补 `-- -D warnings`（与另外四处统一），并在 §9.2 点明 `[workspace.lints]` 是 `warn` 级、必须靠 flag 提级才拦得住新告警。
- 状态: 已实测 · 置信度 高

### dc-11 [中] `conventions.md` 说"上面四条即为合并门禁"，但 CI 里没跑 `gates-web.mjs --push`
- 位置: `docs/conventions.md:611`
- 证据:
  - 文档原文:「CI 已配置（`.github/workflows/ci.yml`），上面四条即为合并门禁，另加 MSRV 编译。」（"上面四条" = fmt / clippy / `cargo test --workspace` / `node scripts/gates-web.mjs --push`）
  - 反证: `ci.yml` 里**没有任何一步**调用 `gates-web.mjs` —— `grep -nE '^[[:space:]]+run:.*gates-web' .github/workflows/ci.yml` 无命中（exit 1），全部 `run:` 只有 9 条: `:83 npm ci`、`:86 npm run typecheck`、`:91 npm run build`、`:102 node lite/regress/compiler.mjs`、`:171 fmt`、`:174 clippy`、`:177 test`、`:205 check`（另 `:67` 是 `defaults.run`）。文件里那三处 `gates-web` 字样（`:6` `:38` `:99`）**全在注释里**（`:38` 是"本地交付前你自己要跑"的叮嘱）⇒ 第 4 条**不在**合并门禁里。
- 复现: `grep -nE '^[[:space:]]+run:.*gates-web' .github/workflows/ci.yml; echo "exit=$?"; grep -nE '^[[:space:]]+run:' .github/workflows/ci.yml`
- 影响: PR 评审者会以为 CI 绿即等于"前端推送档跑过"，于是放行只在本机跑过的改动；对"跳过 ≠ 通过"的本仓口径也是直接违背。
- 建议: 改成「CI 跑 fmt / clippy / test / MSRV + 前端三条（typecheck / build / 负例）；`gates-web.mjs --push` 是**本地**推送前门（CI 无无头 Chrome）」。
- 状态: 已实测 · 置信度 高

### dc-12 [中] `README.md` 的前端栈还写着 Vue Vapor 与 daisyUI（两者都已从仓里移除）
- 位置: `README.md:3`、`README.md:152`（另 `:273` 路线图 P3「Vue Vapor WebUI」）
- 证据:
  - 文档原文 `:3`「前端 **Vue Vapor + TSX**」；`:152`「web/                 Vue Vapor + TSX + daisyUI」。
  - 反证: `web/package.json` 的 devDependencies 只有 `@tailwindcss/vite` / `@typescript/native-preview` / `oxc-parser` / `tailwindcss` / `vite`（**无 vue，无 daisyui**）；`web/vite.config.ts:14` 明写"曾经这里还有 `vue` / `vue-jsx-vapor` 两条 alias"；`AGENTS.md:319-320`/`349-359` 说框架是本仓自研 lite、TSX 由 `oxc-parser` 解析；`docs/architecture.md:6`「原先写的 Vue Vapor 栈已于 2026-09-30 拆掉」、`:944`「**零 Vue 依赖**」；`docs/task-log.md:157`（#41）记录 daisyUI 已在 10-02 移除（`package.json` + lock 删掉 daisyui）。
- 复现: `grep -n 'vue\|daisyui' web/package.json web/vite.config.ts | head; sed -n '3p;152p' README.md`
- 影响: README 是入口文档；"Vue Vapor"会让人去找不存在的依赖，"daisyUI"会让人按已删的 `@plugin` 机制加样式（`app.css` 现在是 `@theme` 令牌层，`AGENTS.md:367-372` 专门警告过没注册就静默不生成）。
- 建议: `:3` 改成"自研 lite（无虚拟 DOM，编译期优化）+ TSX"；`:152` 去掉 `+ daisyUI`（`web/package.json:6` 的 description 里同样有 `+ daisyUI`，可一并处理）；`:273` 标注为历史阶段。
- 状态: 已实测 · 置信度 高

### dc-13 [中] `README.md:260`「本机**不能**跑 `cargo test --workspace`」与 `AGENTS.md` / 台账现状矛盾
- 位置: `README.md:260`
- 证据:
  - 文档原文:「`cargo test --workspace` —— 全量门。⚠ 本机**不能**跑它：它会重链接 `target/debug/mcloud-server`，把 launchd 正在跑的那个进程判成代码签名失效而 SIGKILL（见 AGENTS.md）。」
  - 反证: `AGENTS.md:295-301` 把 `cargo test --workspace` 列为"**提交前必须全绿**"的第一条；`docs/conventions.md:574-578` 同（【硬约束】）；`scripts/gates-rust.sh:53-54` 的 `--full` 就是跑它；`docs/task-log.md:206`（#82）记录 2026-10-04 实测 `--full` ⇒ **1849 passed / 0 failed**。AGENTS.md 里那条机制的真实口径是"能跑，但要接受一次重启/可能被 SIGKILL"，不是"不能跑"（`AGENTS.md:215`：`cargo test -p <某个 crate>`（只要不牵到 `mcloud` 那个二进制的链接）也不碰它；`:218`：「⚠ 但它**替代不了** `cargo test --workspace` —— 那是运行期的门」）。
  - 同处还用了旧二进制名 `target/debug/mcloud-server`。
- 复现: `sed -n '260p' README.md; sed -n '295,301p' AGENTS.md; grep -n 'cargo test --workspace' scripts/gates-rust.sh docs/conventions.md`
- 影响: 与"提交前必须全绿"的第一条冲突 ⇒ 交付时可能出现"按 README 跳过全量测试"的辩解（而 §9.2 明说"『本来就有』这个借口不存在"）。
- 建议: 改成「本机会重链接 `target/debug/mcloud`（launchd 那份）⇒ 要么接受一次重启/授权框，要么先用 `cargo test -p <crate>` 分批；**提交前仍必须跑一次全量**（`scripts/gates-rust.sh --full`）」。
- 状态: 已实测 · 置信度 高

### dc-14 [中] `scripts/run-local.sh:72` 是 `dev-reload.sh:37` 修过的那个"`set -e` 静默死"，这处没修
- 位置: `scripts/run-local.sh:72`（对照 `scripts/dev-reload.sh:37-45`）
- 证据:
  - `run-local.sh:33` `set -euo pipefail`；`:72` `LAUNCHD_LABEL="${MCLOUD_LAUNCHD_LABEL:-$(launchctl list 2>/dev/null | awk '$3 ~ /mcloud-webui/ { print $3; exit }')}"` —— 命令替换**没有** `|| true`。
  - `dev-reload.sh:37-41` 的注释把这条坑写得很清楚:「`launchctl list` 在**被沙箱包住的 shell** 里会 exit 1（空输出）… `set -e` 下 **`VAR=$(失败的命令)` 这行赋值本身就会终止脚本** ✗ —— 位置恰好在 trap 安装之前 ⇒ 表现是「无任何输出、exit 1」」，`:44` 已加 `|| true`。
  - 本会话实测: `launchctl list >/dev/null 2>&1; echo $?` ⇒ **1**；逐字复现 `:72` 那一行 ⇒ 没有任何输出、`EXIT=1`（用 `bash -c` 跑同一行，`echo reached` 从未执行）。语义最小复现: `bash -c 'set -euo pipefail; X="${UNSET:-$(false)}"; echo survived'` ⇒ exit 1。
- 复现: `bash -c 'set -euo pipefail; LAUNCHD_LABEL="${MCLOUD_LAUNCHD_LABEL:-$(launchctl list 2>/dev/null | awk '"'"'$3 ~ /mcloud-webui/ { print $3; exit }'"'"')}"; echo reached'; echo "EXIT=$?"`
- 影响: 这个脚本的用途是"从内置盘跑、避开 TCC 弹框"，而它偏偏在最需要它的沙箱里**无提示死掉**（用户只看到没反应），与 `dev-reload.sh` 那次"WebUI 停了 30 分钟"的教训同源。注意它在 `:91` 之前是关键守卫（防两个实例抢 3000），静默死等于守卫也没生效。
- 建议: `:72` 补 `|| true`（与 `dev-reload.sh:44` 同形）—— 它落在 `:91` 那道"绝不与托管抢 3000"的守卫之前，静默死等于守卫也没生效；另建议与 `dev-reload.sh` 那样给"找不到 label"留一条有出口的提示路径。
- 状态: 已实测 · 置信度 高

### dc-15 [中] `docs/app-task-status.md` 的"未实现条数"自相矛盾（§0 说 15，§1/附录说 17）
- 位置: `docs/app-task-status.md:19`、`:31`、`:85`
- 证据:
  - `:19`「⇒ **项目口径完成度：10 / 25**（有明确实现且行为可判），其余 **15 条未实现**」；§0 表 `:16` 也是「❌ **未实现** | **15**」。
  - `:31`（五道关表第 5 行）「⇒ **这条是 17 条未实现的直接原因**（见 §5-1）」；`:85`「**B. 17 条未实现的**：本仓**一个请求都不发**」。
  - 实测: §2 主表 25 行里 `❌ 未实现` = **15** 行（另 1 行 `⛔ 刻意不做`、1 行 `⛔ 刻意不做（与参考不一致）`、7 行 `✅ 已实现`、1 行 `✅门 判定门`）⇒ 15 才是自洽的数；17 把两条黑名单也算进了"未实现"，与 §0 自己的分类（黑名单单列）冲突。
- 复现: `awk -F'|' '/^\| [0-9]{3,4} \|/ {print $4}' docs/app-task-status.md | sort | uniq -c`
- 影响: 读者据此估工时会多算/少算两条；"哪些是刻意不做、哪些是待实现"正是这份文档的分类价值所在。
- 建议: `:31`/`:85` 统一成 15（或明写"15 条未实现 + 2 条刻意不做 = 17"）。
- 状态: 已实测 · 置信度 高

### dc-16 [中] `docs/task-log.md` 状态列有 9 行越出自己规定的四种状态
- 位置: `docs/task-log.md:9-10`（规矩）vs 状态列越界的 9 行: `:43`(#12 `已改`)、`:44`(#13 `已完成`)、`:45`(#14 `已跑，口令无效`)、`:52`(#21 `已定位`)、`:59`(#28 `已发布`)、`:147`(#31 `已诊断`)、`:148`(#32 `已废弃`)、`:180`(#54 `已闭环`)、`:206`(#82 `已发布`)
- 证据:
  - 规矩原文 `:9-10`「**状态**只用四种：`已上线`／`已落盘`／`待确认`／`未做`」；`AGENTS.md:275-282` 同表（"状态只用四种，别含糊"）。
  - 实测（按未转义 `|` 切第 5 列）: #12 `已改`、#13 `已完成`、#14 `已跑，口令无效`、#21 `已定位`、#28 `已发布`、#31 `已诊断`、#32 `已废弃`、#54 `已闭环`、#82 `已发布` —— **9 行**四种全不含。
  - 结构其它项**是干净的**（本次核查结论）: `#` 编号 1..82 **连续、无重复**；主表每行 6 列、附录两表 3 列，**未转义 `|` 计数逐行一致**（没有裸 `|` 拆列）；所有 `第 N 行` 指针都落在 1..82 内。
- 复现: `python3 -c "import re;rows=[l for l in open('docs/task-log.md') if re.match(r'^\|\s*\d+\s*\|',l)];print([re.split(r'(?<!\\\\)\|',r)[5].strip()[:12] for r in rows if not re.search('已上线|已落盘|待确认|未做',re.split(r'(?<!\\\\)\|',r)[5])])"`
- 影响: 台账是给用户**核对**用的（`:13`"能自己跑的命令优先"）。用四种状态去筛（正是它自己建议的用法）会漏掉这 9 条 —— 其中 #28/#82 的"已发布"、#31 的"已诊断"是有实质结论的条目。
- 建议: 逐条折算成四种（#28/#82 发布类 ⇒ `已上线` 或明确写"发布链已落盘"、#31 `已诊断` ⇒ `未做`/`待确认` 并写卡点），或把规矩改成"四种主状态 + 允许括注补充"。**不要**改历史原话（该文件规矩 5），只把状态格里那个词换掉 + 补括注。
- 状态: 已实测 · 置信度 高

### dc-17 [中] `scripts/dev-reload.sh` 的 EARLY-EXIT 兜底会在"别的 cargo 在跑"时去签名 + bootstrap（与它自己的提示相反）
- 位置: `scripts/dev-reload.sh:67-76`（trap）与 `:87-95`（早退路径）
- 证据:
  - `:76` `trap restore_on_exit EXIT` 在 `:88-94` 的锁检查**之前**安装；`:93` 那句 `exit 1` 会触发 `restore_on_exit`（`:67-75`）⇒ 执行 `scripts/sign-local.sh`（`:71`，**重新签名 `target/debug/mcloud`**）与 `launchctl bootstrap …`（`:72`，对**仍加载着**的托管再 bootstrap 一次，错误被 `|| true` 吞掉）。
  - 而同一路径打印给用户的话是 `:90`「⚠ 检测到别的 cargo 在跑 ⇒ **不动服务**（它还在正常提供 WebUI ✓）」—— 实际**动了**（写了二进制、又发了 bootstrap）。
  - 该分支的设计意图是"等它结束再跑"（`:92`），也就是说：正因为**此刻有别人在编**，才最不该去覆写那个二进制。
- 复现: `sed -n '67,95p' scripts/dev-reload.sh`（**仅静态阅读**；脚本本身按纪律未执行）
- 影响: 与"多写入线并行"的真实工况直接冲突（`AGENTS.md:334-348` 明确说多线是常态）：在另一条线 `cargo build` 写 `target/debug/mcloud` 的同时 `codesign --force` 同一个文件，可能落一个半写状态；`codesign` 失败会让这次 `exit 1` 的语义变成"两次失败"。至少也是白做一次签名 + 一次多余的 bootstrap。
- 建议: 把锁检查挪到 `trap` 安装**之前**（或在 `restore_on_exit` 里加一个 `STOPPED=0/1` 标志，未 `bootout` 过就只 `exit`，不签名、不 bootstrap）。
- 状态: 仅静态 · 置信度 高

### dc-18 [低] `mcloud-gost` 在功能面还有 3 处残留（与 `AGENTS.md:13-15` 的"0 命中"口径冲突）
- 位置: `docs/build-perf.md:4`、`docs/task-log.md:163`、`docs/task-log.md:180`（另 `docs/task-log.md:193` 是"现场事实"性质，可留）
- 证据:
  - `build-perf.md:4`「> `/Users/catlair/mobile/workspace/mcloud-gost`（外置宗卷 `/dev/disk5s1`，APFS）」—— `ls -d ../mcloud-gost` 不存在，当前工作区是 `/Users/catlair/mobile/workspace/mcloud` ⇒ 这份实测数据的"在哪台/哪个目录测的"已不可解析。
  - `task-log.md:163` 给出的**可执行命令**:「`tail -f ~/Library/Logs/mcloud-gost/webui.log | grep 上游`」；`:180` 同形（`webui.err.log`）。而当前 plist 写的是 `~/Library/Logs/mcloud/webui.log`（`grep -o 'Library/Logs/[^<]*' ~/Library/LaunchAgents/com.catlair.mcloud-webui.plist`），`dev-reload.sh:134` 用的也是 `~/Library/Logs/mcloud/`。旧目录**仍存在**（`ls ~/Library/Logs` 两个都有）⇒ 照敲不报错、只是**永远看不到新日志**。
  - `AGENTS.md:15` 的规则:「除了**本段说明**必须引用旧名…功能面应当 0 命中」。
- 复现: `grep -rn mcloud-gost --exclude-dir={.git,target,node_modules} . | grep -v '^./AGENTS.md'`
- 影响: `task-log:163` 是"想立刻看 http logger 效果"的验证入口，指到废弃目录 ⇒ 用户/agent 会得出"日志没打"的错误结论（与 `task-log:163` 自己"还没发生过第三方请求"的说明叠在一起尤其容易误判）。
- 建议: 三处改名（`build-perf.md:4` 的路径、`task-log:163`/`:180` 的日志目录）。台账规矩 6 只豁免"旧二进制名"，不含日志路径。
- 状态: 已实测 · 置信度 高

### dc-19 [低] `ci.yml` 没有 `permissions:` 块，而同仓 `release.yml` 专门按最小权限写过
- 位置: `.github/workflows/ci.yml:40-52`（`on:` / `concurrency:` / `env:` 之间无 `permissions:`）
- 证据:
  - `release.yml:79-83` 有一整段注释 + `permissions: contents: read`，并在 `:389-390` 只给 release job `contents: write`，理由是"跑 `npm ci`、下载依赖、执行编译的 job 不该白拿写权限"。
  - `ci.yml` 全文 `grep -n 'permissions' .github/workflows/ci.yml` ⇒ **无命中**；它的三个 job 只读代码 + 上传/下载 artifact，同样只需要 `contents: read`。
- 复现: `grep -c permissions .github/workflows/ci.yml .github/workflows/release.yml`
- 影响: 实际风险取决于仓库默认 token 权限（`ci.yml:41` 的 `pull_request` 事件本身已降权），但同一仓库两份工作流口径不一致，会让后续新增 job（尤其带 secret/写入的）默认拿到更宽的权限。
- 建议: 与 `release.yml` 对齐，在 `ci.yml` 顶层加 `permissions: contents: read`。
- 状态: 已实测 · 置信度 中

## task-log 未闭环清单

来源: `docs/task-log.md:82-91`（"未闭环 / 未做"表，2026-10-04 清理时复核）+ 各行末尾的 `✗ 没做完`。逐条只读核对如下。

| # | 事项 | 台账口径 | 本次只读核对 | 结论 |
|---|---|---|---|---|
| ② | 编排页两个账号清单换成 `ui/account-picker.tsx` | **仍未做** ✗（`:89`） | `grep -n 'toggleRunAccount\|toggleSchedAccount\|account-picker' web/src/pages/pipelines-page.tsx` ⇒ 手搓 `toggleRunAccount:396` / `toggleSchedAccount:436`（用在 `:940` `:1039`），**无** `account-picker` 命中 | **仍开着** ✓ 台账准确 |
| ③ | `Pipeline.accounts` / `stopOnError` 仍是死字段 | **仍是死字段** ✓（`:90`） | `crates/mcloud-core/src/pipeline.rs:67` = `pub accounts: Vec<String>`、`:77` = `pub stop_on_error: bool` ✓ 行号准确；`web/src/pages/pipelines-page.tsx:312-323` 的 `toInput()` 确实不含 `accounts`/`stopOnError`（`:316` 恒 `stopOnError: false`） | **仍开着**（有意保留作老数据兼容）✓ |
| ④ | 任务页「编排」tab 只读总览 | 待用户定（`:91`） | 未复核（属产品取舍，不是文档/代码矛盾） | **等你定** |
| ① ⑤ | 直播「下一场」/ 移除 daisyUI | 已闭环（`:84-85`） | daisyUI 已从 `web/package.json`/lock 移除 ✓ | 已闭环 ✓ |

台账里**另有三处"没做完"，仍有效且本次未做**（不在上面那张表里，但同一份文件里明写）:

1. `:206`（#82）⚠ `generate_release_notes: true` 在该仓没有上一个 release 兜底 ⇒ Release body **没有"这次改了什么"的清单**（自动部分只落一行 `Full Changelog`）。
2. `:206`（#82）⚠ `cross` 那条 **Linux arm64 本地验不了**（本机无 Docker）⇒ 只算 CI 那次 success；Release 包**未签名/未公证**（macOS 首次要 `xattr -dr com.apple.quarantine`）。
3. `:163`（#68）✗ 全量门禁当时未跑（`scripts/gates-rust.sh --full` / `gates-web.mjs --full`）—— 这一条**已被 `:206`（#82）的 1849 passed + 10/10 覆盖**，可视为闭环，但**:3000 仍在跑 rc.1**（`curl -s :3000/api/version`），所以"版本号改成 rc.2"这件事**没有上线**（另见 dc-1 与其备注）。

> 说明: `docs/task-log.md:197`（#66）里那条"Rust 侧两档还没进钩子（`scripts/gates-rust.sh` 仍手敲，等用户点头）"也仍然成立 —— 当前 `.git/hooks/` 里只有 `pre-commit` / `pre-push` 两个指向前端门禁的软链（`ls -la .git/hooks/` 可见），没有 Rust 侧钩子。
