# AGENTS.md — 给在这个仓库里干活的 agent

## 这个仓库是什么

`mcloud` 是**中国移动云盘（移动云盘）的自动化工具**。

> 命名：**本仓（Rust）就叫 `mcloud`**（crate 名一律 `mcloud-*`）。
> 下文提到「Go 参考实现」时也写作 `mcloud` —— 那是**另一份** Go 实现，
> 与本仓同名不同物；凡可能歧义的地方一律带上「Go 参考实现」字样。
>
> ⚠ **改名（2026-09-30，用户口径）**：本仓原名 `mcloud-gost`，现已**全部收口** ——
> 源码 / WebUI / 文档 / **发布产物名**（`mcloud-v<版本>-<target>`）都换成了 `mcloud`。
> **规则：再发现残留就顺手改掉**，别攒着做全仓大改。
> 查残留：`grep -rn mcloud-gost --exclude-dir={.git,target,node_modules} .`
> —— 除了**本段说明**必须引用旧名（否则「改了什么」就无从说起），功能面应当 0 命中。
>
> 三处**一次性副作用**（不是 bug，别当回归去查）：
> ① 主题偏好的 localStorage 键 `mcloud-gost-theme` → `mcloud-theme` ⇒ 老用户第一次打开会
> 回落成「跟随系统」，之后重新记住；
> ② 通知 webhook 载荷里的 `source` 字段 `"mcloud-gost"` → `"mcloud"`（`mcloud-app::notify`），
> 收方若按它分流要跟着改；
> ③ `/api/status` 的 `name` 字段同理 —— 侧栏那行「mcloud v0.1.0」要**重编译后**才变。

⇒ **访问云盘本身、读写它的接口、抓包分析它的协议，就是这个项目的正常工作内容。**
不需要为"要动用户的云盘数据了"而反复征求确认 —— 那是任务本身，不是风险动作。

⚠ 曾经发生过一次跑偏：把常规的仓库内文件读写误当成"访问用户的外部卷宗"，
于是反复弹确认、用户在超时后被迫中断。**别再这样。**

## ⚠⚠ 那个「想访问可移除宗卷上的文件」弹窗 —— 别去改 DSH 配置

那个弹窗是 **macOS 的 TCC** 发的（服务名 `kTCCServiceSystemPolicyRemovableVolumes`），
**跟 DSH 的权限配置毫无关系**。改 `~/.dsh/` 里的 sandbox/approval 是白费功夫。

成因：工作区在**外置盘**上（USB 挂载的 APFS 卷这类**可移除宗卷**；
`pwd` 落在挂载点下就是这种情形）。
macOS 对可移除宗卷有隐私保护，可执行文件访问它就要授权。

### 真正的放大器是 launchd 的 `KeepAlive`

仓库的 WebUI 由用户级 launchd 代理托管：

```
~/Library/LaunchAgents/<label>.plist        # 形如 com.<用户名>.mcloud-webui
  ProgramArguments → <仓库>/target/debug/mcloud-server   （外置盘）
  KeepAlive: true                                        ← 挂掉立刻重拉
```

**`kill` 它 = 立刻又起来一个新进程 = 又触发一次 TCC 检查 = 又弹一次框。**

⇒ **验证时绝对不要 `kill` 这个服务，也不要另起一个占 3000 端口的实例。**
后者更糟：launchd 那份绑不上端口就退出，`KeepAlive` 再拉，**形成崩溃重启循环**，
每 20 秒弹一次框（20 秒是 launchd 的重启节流）。

今天的 1000+ 次记录就是这么来的 —— 是 agent 自己反复 kill + 另起实例造成的。

判断有没有掉进这个循环：

```bash
launchctl list | grep mcloud-webui     # 第 2 列非 0 ⇒ 上次退出码非0，正在重试
tail -5 ~/Library/Logs/<label>/webui.err.log       # 找 "Address already in use"
log show --last 2m --predicate 'subsystem == "com.apple.TCC"' --style compact \
  | grep RemovableVolumes | grep -c PROMPTING      # 期望 0
```

### 还有一个残留触发点：重新编译

TCC 按**代码要求**记授权，而未签名的二进制其要求就是**内容哈希**。
⇒ **每次 `cargo build` 都变成"另一个二进制"**，下次启动授权对不上，再弹一次。
系统日志的原话是 `Failed to match existing code requirement`。

这解释了"第一天用没问题"：那天没反复重编译，授权一直匹配。
正常开发（不改代码、不重启服务）**不会弹**；只有改了代码重新编译才会。

### ⚠ 而且「重新编译」不只是换个哈希 —— 它会 **SIGKILL 正在跑的那个进程**

上面那句只说了一半。2026-09-28 用崩溃报告查实了另一半：

`cargo build` **原地覆写** `target/debug/mcloud-server`，而 launchd 正在跑那个文件。
覆写瞬间内核把进程判为**代码签名失效**：

```
~/Library/Logs/DiagnosticReports/mcloud-server-2026-09-28-153826.ips
  termination: { namespace: "CODESIGNING", indicator: "Launch Constraint Violation" }
  exception:   { type: "EXC_CRASH", signal: "SIGKILL (Code Signature Invalid)" }
```

⇒ **光"不 kill、不另起实例"是不够的**：一次 `cargo build` 就足以触发
「SIGKILL → `KeepAlive` 重拉 → 旧进程还没放开 3000 ⇒ `Address already in use`
→ 再拉」这个循环。今天的崩溃报告落在 07:29 / 13:33 / 13:48 / 13:55×2 /
14:04 / 14:06 / 14:07 / 14:12 / 14:50 / 15:00 / 15:38×2 —— **全部落在重编译时刻**，
不是"agent 手贱 kill"那一条路径。

> ⚠ **2026-09-30 实测补一条，方向与上面相反**：`cargo build` **不一定**会 SIGKILL
> 正在跑的那个进程 —— 它把新二进制**改名**顶上去（**新 inode**），旧进程继续持有
> 已被 unlink 的旧 inode，于是 **PID 不变、`/api/health` 照样 ok**。
> 实测：编前 `inode=2642011 PID=20565`，编后 `inode=2707431 PID=20565`。
> ⇒ 由此多出一个很有欺骗性的中间态：**二进制是新的、跑着的代码是旧的**。
> 「编完就生效」在 debug 下只对**前端**成立（`web/dist` 运行时读盘），
> Rust 侧要显式重启：`launchctl kickstart -k gui/$(id -u)/<label>`。
> 判断到底跑的是哪一份，两条都能一眼看出：
> `lsof -p <PID> | awk '$4=="txt"'` 的 inode 是否等于 `stat target/debug/mcloud-server`；
> 或看启动日志那句（改名后新代码写 `mcloud 启动`、旧代码写 `mcloud-gost 启动`）。
> 那次显式重启会按上面那条触发一次授权检查：2026-09-30 实测近 3 分钟 **1 次** PROMPTING、
> err.log 没有被写 —— 是**一次**弹框，不是循环。
>
> ⚠⚠ **2026-10-01 再补一条更要紧的：托管跑的已经是 `target/release/mcloud-server`**
> （plist 的 `ProgramArguments` 现在指 release；`ps -o command -p <PID>` 一看就知道）。
> 由此三条：
> 1. `cargo build`（**debug**）与线上**毫无关系** —— 编完、重启完线上还是旧代码。
>    要上线得 `cargo build --release -p mcloud-server --bin mcloud-server`。
> 2. **前端也不再"刷新即生效"**：`rust-embed` 那条"debug 下运行时读盘"只对 debug 成立，
>    release 是**编译期**把 `web/dist` 嵌进二进制（`static_files.rs` 文件头写着这条）
>    ⇒ release 部署下前端改动**同样要重编 release**。
> 3. 判断线上到底是哪一份，最快两条：
>    `curl -s http://127.0.0.1:3000/ | grep -o 'assets/index-[A-Za-z0-9_-]*\.js'`
>    与 `web/dist/index.html` 的 chunk 名对比；以及
>    `lsof -p <PID> | awk '$4=="txt"'` 的 inode 与 `stat target/release/mcloud-server` 对比。
> 4. ⚠ release 重编会把**工作区里所有未提交的改动**一起编进线上（本仓同时有多条写入线时
>    尤其危险）—— 重启上线前先 `git status` 看一眼。

### ✅ 开发期自检用它，代价 0 次重启：`cargo check`

上面那条有一个**反直觉的例外**，实测有效：

```bash
cargo check --workspace --all-targets     # 全工作区类型检查，不链接
```

`cargo check` **只产出 `.rmeta`**，从不写 `target/debug/mcloud-server`
—— 而 SIGKILL 的触发点正是**链接**把那个文件覆写掉。

2026-09-28 实测（本仓），跑之前与跑之后：

```
inode=2364362 mtime=1790581118 size=38401616    ← 两次完全一致
服务 PID = 42558                                 ← 没被换掉
/api/health → {"status":"ok"}
```

⇒ **改完代码先用它自检**，能回答"整个工作区还编不编得过"，而代价是
0 次重启、0 次弹框。同理 `cargo test -p <某个 crate>`（只要不牵到
`mcloud-server` 的链接）也不碰那个二进制。

⚠ 但它**替代不了** `cargo test --workspace` —— 那是运行期的门。
真要跑全量就得接受一次重启；"要不要为此改服务怎么跑"仍然要用户点头。

若要去掉这最后一种弹窗，两条路（都需要用户点头，别自作主张）：
1. 把服务改成从**内置盘**跑（二进制 + cwd + `data/` 都在内置盘，实测 0 次外置盘访问）
2. 用**固定身份的证书签名**二进制，让授权在重编译后依然匹配

## 写到这里：**依据的优先级**（用户口径，2026-09-28）

写任何上游协议时，**依据的优先级**是：

1. **`mcloud-sign`**（旧 TS 实现，单独一个工作区，**不在本仓内**）
   —— 它**能跑**，照它写**完全可以**。
2. 真机抓包 / 参考实现的 `docs/capture`
3. 推断

⚠ **不要拿"等实拍"把工作挂起。** 照着 `mcloud-sign` 把能写的都写完；
只有**真写不了**的分支才留空，而留空处**必须注明**"为什么写不了"。
"等实拍"不是不做的理由 —— 它是**注明的理由**。

⚠ 反过来也成立：`mcloud-sign` 里**没有**的步骤（例如某条链它压根不建会话），
要当成"我们可能多做了"的线索去查，而不是"我们有、它没有 ⇒ 我们对"。

⚠⚠ **不要把 Go 参考实现（`mcloud`，另一份 Go 代码）的形状搬过来。**
用户口径（2026-09-28）：「**不需要按照 go 的语法写**」。

Go 那边是**同一件事的第二次实现**，它的价值在于"**它踩过哪些坑、哪些格子有读数**"——
拿它当**线索**，不拿它当**结构**。具体地说：

* 别把它的类型名/表结构/分层照搬成 Rust（例如它那几张 spec 数据表 + 标志位，
  在本仓未必是该有的形状）。**按本仓的写法写**。
* 别拿它的证据门槛卡自己。行为以 **`mcloud-sign`** 为准——那份 TS **能跑**。
* 注释里**可以**引它的坑与读数，但**论证的根据**应当是 TS 的行为或实拍，
  而不是"Go 是这么分的"。

## 什么**才**需要先问

只有这几类真的需要先问用户（其余默认直接做）：

| 情形 | 为什么 |
|---|---|
| **对上游产生副作用且不可逆** | 例如兑换下单（真扣豆）、删账号、删网盘文件 |
| **改动会覆盖用户未提交的工作** | 例如 `git checkout -- <file>`、强制 push |
| **要动仓库与工作区之外的东西** | 例如改 `~/.dsh/`、装全局软件 |

判别方法：**这件事做错了能不能撤？** 能撤就直接做，不能撤才问一次。

⚠ "问一次"就是**一次**。同一个问题不要反复确认；用户已经答过的，照做。

## 项目约定（详见 `docs/conventions.md`）

- 中文注释，说明**为什么**这么写（尤其"看着像 bug 但故意的"那些）
- kebab-case 文件名；仅具名导出
- 提交前必须全绿：
  ```
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
  node scripts/gates-web.mjs --full
  ```
  （`--full` = 三个认证态的交互验收全跑，≈ 77 s；不带它的默认只跑 `ready`，
  是**改脚本时的快回路**，不能替代提交前那次。四条里最后一条要无头 Chrome，
  没有的机器上会明说"跳过 ≠ 通过"。）

## ⚠ 构建顺序（踩过坑）

**改完前端必须 `npm run build` 再 `cargo build`。**

`rust-embed` 在**编译期**把 `web/dist` 内嵌进二进制，而 `cargo build`
**不跟踪** `web/dist` 的变化。顺序反了会内嵌旧资源，
表现是页面白屏 / chunk 404 —— 而且 `cargo build` 还会说"Finished"，不报错。

`web/dist` 不入库，所以干净检出后**必须**先构建前端。

## 环境

- `export CARGO_HOME="$PWD/.cargo-home"`；`cargo` 在 `$HOME/.cargo/bin`（不在默认 PATH）
- `npm --cache /tmp/npmcache-dsh`
- 服务端口用环境变量 `MCLOUD_ADDR`（默认 `127.0.0.1:3000`），数据目录 `MCLOUD_DATA_DIR`
  （⚠ 不是 `MCLOUD_DATA`）；`PORT` 环境变量**被忽略**
- macOS 没有 `timeout` 命令；SSH 用 `GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=20"`
