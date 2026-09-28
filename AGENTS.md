# AGENTS.md — 给在这个仓库里干活的 agent

## 这个仓库是什么

`mcloud-gost` 是**中国移动云盘（移动云盘）的自动化工具**。

⇒ **访问云盘本身、读写它的接口、抓包分析它的协议，就是这个项目的正常工作内容。**
不需要为"要动用户的云盘数据了"而反复征求确认 —— 那是任务本身，不是风险动作。

⚠ 曾经发生过一次跑偏：把常规的仓库内文件读写误当成"访问用户的外部卷宗"，
于是反复弹确认、用户在超时后被迫中断。**别再这样。**

## ⚠⚠ 那个「想访问可移除宗卷上的文件」弹窗 —— 别去改 DSH 配置

那个弹窗是 **macOS 的 TCC** 发的（服务名 `kTCCServiceSystemPolicyRemovableVolumes`），
**跟 DSH 的权限配置毫无关系**。改 `~/.dsh/` 里的 sandbox/approval 是白费功夫。

成因：工作区在**外置硬盘**上（`/Users/catlair/mobile`，USB 挂载的 APFS 卷）。
macOS 对可移除宗卷有隐私保护，可执行文件访问它就要授权。

### 真正的放大器是 launchd 的 `KeepAlive`

仓库的 WebUI 由用户级 launchd 代理托管：

```
~/Library/LaunchAgents/com.catlair.mcloud-webui.plist
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
tail -5 ~/Library/Logs/mcloud-gost/webui.err.log   # 找 "Address already in use"
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

1. **`mcloud-sign`**（`/Users/catlair/Documents/workspace/mcloud-sign`，旧 TS 实现）
   —— 它**能跑**，照它写**完全可以**。
2. 真机抓包 / 参考实现的 `docs/capture`
3. 推断

⚠ **不要拿"等实拍"把工作挂起。** 照着 `mcloud-sign` 把能写的都写完；
只有**真写不了**的分支才留空，而留空处**必须注明**"为什么写不了"。
"等实拍"不是不做的理由 —— 它是**注明的理由**。

⚠ 反过来也成立：`mcloud-sign` 里**没有**的步骤（例如某条链它压根不建会话），
要当成"我们可能多做了"的线索去查，而不是"我们有、它没有 ⇒ 我们对"。

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
  node scripts/gates-web.mjs
  ```

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
