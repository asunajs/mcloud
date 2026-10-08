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
> ⚠ **仓库身份（2026-10-03，用户口径：「mcloud-gost 正式改名 mcloud」）**：
> GitHub 上**只有一个** `mcloud`（`asunajs/mcloud`；`catlair/mcloud` 与 `catlair/mcloud-gost`
> 是它改 org 前后的旧路径，org 改名后 `catlair/…` 仍能解析到 `asunajs/…`，但**不要再当权威名用**）。
> 本仓 `main` = Rust 实现；**那份 Go 实现现在是同一个仓的 `go-legacy` 分支**，
> 不是"另一个仓库的上游"。`go-legacy` 已经把抓包素材与通路脚本删掉（`docs/capture/`、
> `docs/upstream/`、四篇规范、八个脚本，共 39 个文件），素材没消失 —— 在删除前的那一个提交里：
> `git show go-legacy^:docs/capture/<文件>`。⚠ **本机那份 Go 侧检出是 `../mcloud-back/`**
> （2026-10-04 查实：`../mcloud/` 是**本仓自己**的一份同 HEAD 副本，不是 Go ✗）⇒
> 全库 `../mcloud-back/…` 那些相对路径才解析得到（`docs/business-feature-inventory.md`、
> `docs/capture/*.txt`、`docs/pending-decisions.md`、`internal/app/live_room*.go`、`har/` 都在那边 ✓）。
> 抓包规范与协议文档的家是**第三个仓** `mcloud-capture`（本机 `../mcloud-capture/`）。
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
  ProgramArguments → <仓库>/target/debug/mcloud         （外置盘）
  KeepAlive: true                                        ← 挂掉立刻重拉
```

> ⚠⚠ **2026-10-03 用户口径：「合并为一个，以后就叫 mcloud」。**
> CLI 与服务合并成**一个**二进制，名字叫 **`mcloud`**（原来两个：`mcloud-server` +
> `mcloud-cli`，同一个 crate、同一张依赖表）。实测合并只多 **116 KB**，省掉一整个
> 3.12 MB 的二进制（7.17 MB → 4.16 MB）；分发规则是**不带参数 = 起服务、带子命令 = CLI**。
> ⇒ 本文件里凡是出现在**带日期的旧口径**里的 `target/debug/mcloud-server` /
> `target/release/mcloud-server` / `mcloud-cli`，今天一律读作 `target/debug/mcloud` /
> `target/release/mcloud` / `mcloud` ✓（历史原文不再逐句改写：那时候确实叫那个名字）。
> ⚠ **那条「重编 ⇒ SIGKILL ⇒ KeepAlive 重拉 ⇒ TCC 弹框」的链一条都没少**：plist 改指
> `target/debug/mcloud` 之后，`cargo build` 写的**正是托管在跑的那个文件** ⇒ 照旧原地覆写、
> 照旧可能被判签名失效 ✗。合并改的是"几个二进制"，不是这条机制。
> （只有在"改了 bin 名但 plist 还没跟上"的那几分钟里，这条链才短暂断开 —— 别把它当收益 ✓。）

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

> ✅ **2026-10-03：这条路已经堵上（用户点头后做的）。**
> 用**固定身份签名**，要求就从"内容哈希"变成"这条证书签的" ⇒ 换内容照样匹配 ✓。
> 机器可读的对照（本机实测，`codesign -d -r-` 打出的 `designated` 那行）：
>
> ```
> 签之前： identifier 缺省 ⇒ cdhash H"351df1…"                        ← 每次重编都变 ✗
> 签之后： identifier "mcloud" and certificate leaf = H"3d79b3f3…"     ← 重编不变 ✓
> ```
>
> 用法（**日常别手动拼**，`cargo build` 覆写的是未签名那份 ✗）：
>
> ```bash
> scripts/dev-reload.sh            # 重建 + 签名 + 重启（一条命令）
> scripts/dev-reload.sh --no-build # 只重新签名 + 重启
> scripts/sign-local.sh [<二进制>] # 单独签名（身份：MCLOUD_SIGN_IDENTITY，缺省取钥匙串第一个）
> ```
>
> ⚠ `dev-reload.sh` 是**先 `bootout` 停掉托管、再编、再签、最后 `bootstrap`** ✓ ——
> 因为 `cargo build` 原地覆写正在跑的文件时可能被内核 SIGKILL ✗，而 `KeepAlive`
> 会立刻拉起**还没签名**的新文件 ✗ ⇒ 白签。停掉再编，这个窗口就不存在 ✓。
> 代价是编译那 1~2 分钟 WebUI 不可用（**只改前端别用它** ✓：前端 `npm run build`
> + 刷新即可，二进制不用重编 ✓）。
> ⚠ 换机器/换钥匙串后身份可能不存在 ⇒ 那时脚本**只警告不失败** ✓（弹框回到旧行为，
> 不会把上线卡死 ✓）。

这解释了"第一天用没问题"：那天没反复重编译，授权一直匹配。
正常开发（不改代码、不重启服务）**不会弹**；只有改了代码重新编译才会。

### ⚠ 而且「重新编译」不只是换个哈希 —— 它会 **SIGKILL 正在跑的那个进程**

上面那句只说了一半。2026-09-28 用崩溃报告查实了另一半：

`cargo build` **原地覆写** `target/debug/mcloud`，而 launchd 正在跑那个文件。
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
> `lsof -p <PID> | awk '$4=="txt"'` 的 inode 是否等于 `stat target/debug/mcloud`；
> 或看启动日志那句（改名后新代码写 `mcloud 启动`、旧代码写 `mcloud-gost 启动`）。
> 那次显式重启会按上面那条触发一次授权检查：2026-09-30 实测近 3 分钟 **1 次** PROMPTING、
> err.log 没有被写 —— 是**一次**弹框，不是循环。
>
> ⚠⚠⚠ **2026-10-02 用户口径：开发期让托管跑 `target/debug/mcloud`。**
> 上面那条"指 release"是 2026-10-01 的状态，**现在已经改回 debug**（plist 的
> `ProgramArguments`；改之前那份 plist 备份在 `~/.LaunchAgents/…plist.bak-before-debug`）。
>
> 理由很实在：**改动前端不该每次都重编二进制**。release 把 `web/dist` **编译期嵌死** ⇒
> 改一个字也要 release 重编 ⇒ 换内容哈希 ⇒ 又弹一次授权框；而 debug 是**运行时读盘** ⇒
> 改前端只要 `npm run build` + 刷新页面，**一次框都不弹**（2026-10-02 实测：
> 切到 debug 后改好的文案直接就在线上生效了，没重编、没弹框）。
>
> 于是开发期的分工是：**改前端 = `npm run build` + 刷新**；**改 Rust = `cargo build` +
> `launchctl kickstart -k`**（这一步仍然会弹一次框，因为二进制换了哈希 —— 但比"每改一个字
> 前端都弹"少得多）。**上线才编 release**（`npm run build` → `cargo build --release` →
> kickstart → 点一次「允许」）。
>
> 代价照旧要说清：debug 是未优化的，跑得慢一些（本地 WebUI 无所谓）；
> 另外 debug 也**照样**有"重编覆写正在跑的二进制"那个坑（见下）。

> ⚠⚠ **2026-10-01 曾经有一条：托管跑的改成了 `target/release/mcloud-server`**
> （plist 的 `ProgramArguments`；`ps -o command -p <PID>` 一看就知道）。
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
> 5. ⚠ **别用 `grep -a` / `strings` 判断"某段代码在不在二进制里"**：2026-10-01 实测两者
>    在这份二进制上**一律漏报**（连 `mcloud 启动` 这种确定执行了的串都搜成 0）。要查就按
>    字节数：
>    `python3 -c "import sys;print(open(sys.argv[1],'rb').read().count(sys.argv[2].encode()))" <二进制> <中文串>`
>    —— 实测新路由的三个串各命中 1 次，而 grep 全 0。
> 6. ⚠ 重编后**第一次启动会弹一次授权框**（TCC 按内容哈希认程序），进程会**卡在启动阶段**
>    等你点「允许」：表现是 launchctl 有 PID、`lsof` 里进程持有新 inode，但 **3000 没在监听、
>    日志一片空白**。别把它当启动慢或崩了 —— 也别 kill 它（点了允许同一个进程会自己接着跑，
>    PID 不变；实测放开后约 20 秒起来）。

### ✅ 开发期自检用它，代价 0 次重启：`cargo check`

上面那条有一个**反直觉的例外**，实测有效：

```bash
cargo check --workspace --all-targets     # 全工作区类型检查，不链接
```

`cargo check` **只产出 `.rmeta`**，从不写 `target/debug/mcloud`
—— 而 SIGKILL 的触发点正是**链接**把那个文件覆写掉。

2026-09-28 实测（本仓），跑之前与跑之后：

```
inode=2364362 mtime=1790581118 size=38401616    ← 两次完全一致
服务 PID = 42558                                 ← 没被换掉
/api/health → {"status":"ok"}
```

⇒ **改完代码先用它自检**，能回答"整个工作区还编不编得过"，而代价是
0 次重启、0 次弹框。同理 `cargo test -p <某个 crate>`（只要不牵到
`mcloud` 那个二进制的链接）也不碰那个二进制。

⚠ 但它**替代不了** `cargo test --workspace` —— 那是运行期的门。
真要跑全量就得接受一次重启；"要不要为此改服务怎么跑"仍然要用户点头。

若要去掉这最后一种弹窗，两条路：
1. 把服务改成从**内置盘**跑（二进制 + cwd + `data/` 都在内置盘，实测 0 次外置盘访问）
   —— `scripts/run-local.sh` 就是这条 ✓（仍在，按需用）
2. ✅ **用固定身份的证书签名二进制**（2026-10-03 用户点头后**已落地** ✓）
   —— 见本节上面的「这条路已经堵上」；日常用 `scripts/dev-reload.sh` ✓

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

## 用户下达的任务要记账（用户口径，2026-10-02）

用户每下达一件事 ⇒ 在 **`docs/task-log.md`** 记一行：

**下达时刻 / 完成时刻 / 状态 / 落点（`文件:行`，或一条能自己跑的命令）**，以及**没做完的部分** ✗。

状态只用四种，别含糊：

| 状态 | 含义 |
|---|---|
| `已上线` | 跑在 `:3000` 上的那份就是它 ✓（Rust 要 `cargo build` + `launchctl kickstart`；前端 `npm run build` 后**刷新即生效**）|
| `已落盘` | 源码/产物改了，**但没重启** ✗ ⇒ ⚠「已落盘」**不等于**生效 ✗ |
| `待确认` | 需要用户点一次/跑一次才能闭环 ✓（写清卡在哪）|
| `未做` | 没做 ✗（宁可写这个，也不要写"记着要…"✗）|

⚠ 时刻只写**确实知道的**（会话时间戳、日志、`git log -1 --format=%ci`）；不知道就写 `≈` 或 `未记录` ✗。

⚠ 这份台账是给用户**核对**用的，不是给 agent 自述用的 ⇒ 能自己跑的命令优先 ✓。

> 起因（2026-10-02）：用户实测直播「下一场」时间错，而 agent 有过"只说改了、其实没落盘"的记录 ⇒
> 用户原话：「你之前说改怎么就口头修改。我怀疑你还有很多口头修改的地方，但是我没法一一查证」。

## 项目约定（详见 `docs/conventions.md`）

- 中文注释，说明**为什么**这么写（尤其"看着像 bug 但故意的"那些）
- kebab-case 文件名；仅具名导出
- 提交前必须全绿：
  ```
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
  node scripts/gates-web.mjs --push
  ```
  （前端门禁**分两档**（2026-10-04 用户口径：「一种在 push 时再验证，时间长范围广；
  一种提交就验证必要的测试」）—— `--commit`（**默认**）是必要且快的那档：类型 + 构建 +
  交互验收 `ready` + 大体量（分页 + 窗口化）+ 搜索框；`--push`（旧名 `--full` 仍认）在它之上
  再加 320/375 窄屏 + `login`/`setup` 两个认证态 + 泄漏量测。⚠ **2026-10-07 起框架自己的
  三条门（编译期负例 / demo / bench）不在这里了** —— lite 已剥离到 `asunajs/lite`，那三条
  跑在框架仓的 CI 里（见 `docs/web/lite.md`）。**提交时**跑哪档可以交给钩子：`scripts/install-git-hooks.sh` 一装，
  `pre-commit` 自动跑提交档、`pre-push` 自动跑推送档 ✓（跳过一律 `--no-verify`，
  跳了就是没验证过 ✓）。要 Chrome 的那几条在没有浏览器的机器上会明说"跳过 ≠ 通过"。
  ⚠ Rust 侧**只有格式那一档**进了钩子（2026-10-06 用户口径「A」）：暂存区里有 `.rs` 时，
  `pre-commit` 先跑 `cargo fmt --all -- --check`（实测 ≈1.1 s、只读源码、**不碰 `target/`** ✓）；
  `clippy` / `test` **不进钩子** —— 冷编代价 19m13s / 27m14s，且 `cargo test -p mcloud-server`
  会**重链接** launchd 正在跑的那个 `target/debug/mcloud` ⇒ SIGKILL + 一次 TCC 弹框 ✗。
  那两档照旧**手动**跑，或 `scripts/gates-rust.sh --full` ✓。）

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
- **要测试账号 / 多用户场景**（2026-10-06 加）：
  `mcloud user list` / `mcloud user add <名字> <口令>` / `mcloud user passwd <名字> <新口令>`。
  空实例上第一条 `add` 建的就是**管理员**（= 不必再走 WebUI 初始化向导 ✓）；之后加的都是
  **普通用户** —— 管理员按 `legacy_owner`（= 创建最早的那个）**算出来**，不能用命令指定 ✓。
  ⚠⚠ 它写 `data/users.json`，而 `JsonStore` 是**启动时全量读进内存**的 ⇒ **跑完必须重启服务**
  （`scripts/dev-reload.sh --no-build`）：否则用它登录 **401**，而且服务下次写这个文件
  （改口令、老档口令升级）会拿**陈旧快照整份覆盖**，刚加的用户就没了 ✗。
  ⚠ 加人**不会**把已有账号藏起来：账号记录里的 `owner` 是显式写死的，`visible_to` 对
  `Some(owner)` 只认相等 ✓（动手前核过，别被"多用户会丢数据"吓住）。
- ⚠ **无头 Chrome 在某些环境里必须加 `--no-sandbox`**（本机 DSH 沙箱：不加就是
  `Target crashed` / `CDP 调用超时：Page.enable`，2026-10-05 实测）。它是**显式开关，默认关**
  （关沙箱是降级，开发机不需要）：`MCLOUD_CHROME_NO_SANDBOX=1 node scripts/gates-web.mjs --push`。
  公共参数**只有一份来源** `web/scripts/chrome-flags.mjs`（2026-10-07 之前它在
  `web/lite/chrome-flags.mjs`；框架剥离时本仓只留下页面级验收这一处 spawn）。环境变量**会穿过 git 钩子** ✓ —— 过 `pre-commit` /
  `pre-push` 不需要改任何文件。
- ⚠ **编译/检查为什么慢、怎么跑才快 —— 实测归因见 `docs/build-perf.md`**（2026-10-01）。
  三条最省事的：① 日常自检用 **`cargo check -p <crate>`**，别动不动 `--workspace --all-targets`
  （后者要把含 `#[cfg(test)]` 的文件**再编一份 test 版**（别再背数，现算：`grep -rl '#\[cfg(test)\]' crates | wc -l`）+ 链接 8 个集成测试目标）；
  ② **起编译前 `pgrep -fl 'cargo check'`** 看一眼有没有别的写入线在跑 —— cargo 对 `target/`
  上独占锁，硬抢两边都慢；③ `mcloud-core` / `mcloud-tasks` / `mcloud-engine` 是**全仓下游**，
  这类改动**攒成一批再编**（每改一行就编 = 全仓重编一次）。
  ⚠ 已实测**排除**：外置盘吞吐、小文件随机 I/O、内存换页、磁盘空间 —— 别再往那些方向查。
- ⚠⚠ **同时有多条写入线（多会话 / 子代理）时：每线一个 `CARGO_TARGET_DIR`**（2026-10-04）。
  `target/` 上的独占锁是**串行**的：两条线同时编，谁都没变快，只是排成一队。
  2026-10-04 实测：本应 **3m01s** 的 `cargo build -p mcloud-server --bin mcloud`，
  因为另一条线（两个 cargo 进程）在编，变成 **34m15s** ✗ —— 这 31 分钟里只有排队。

  ⇒ 约定（**起手就设**，别等卡住才设）：

  ```bash
  export CARGO_TARGET_DIR="$PWD/target/lines/<本线名字>"   # 例如 target/lines/lead
  ```

  代价：**第一次**是冷编（全仓一遍），之后这个目录就是本线独占的暖目录 ✓，
  再也不会被别的线锁住 ✓。磁盘换时间，而 `target/` 本就在 `.gitignore` 里 ✓。
  只有"确实要共用同一份产物"时才回到默认 `target/`（例如要给托管部署的那份 debug 二进制 ✓）。
  ⚠ 判据别只看 "Finished"：先 `pgrep -fl 'cargo (build|check|test)'` 数一下有几条线 ✓。
- **前端工具链（2026-10-01 起，两条都换过）**：
  * 编译：TSX 由 **`oxc-parser`** 解析（框架仓 `asunajs/lite` 的 `ast.ts` 是适配层，
    `typescript` 包**已不在依赖里**）。⚠ 它是**原生**包 ⇒ 凡把它打进 Node 侧产物的构建
    都要标 `external`（见框架仓的 `regress/check.vite.config.ts`）。⚠ 2026-10-07 起
    它由 `@asunajs/lite` 直接依赖，本仓 `web/package.json` 里**不再**单独声明。
  * 类型检查：`npm run typecheck` = **`tsgo --noEmit`**（`@typescript/native-preview`，原生实现）。
    原来那个 `@ts-macro/tsc` fork 已删（全仓没有 ts-macro 宏用法，且它对 TS 大版本最敏感）。
    ⚠ **用户口径（2026-10-01）：只要 tsgo** —— 不要把 `typescript`/`tsc` 加回来做"互校兜底"。
    已验证它真在检查：故意写错时与 `typescript@5.9.3` 各报 7 条、错误码/行列/文案逐条相同，
    有错时退出码 1（门禁会红）；两边 `--listFiles` 项目文件清单逐行相同。
  * 换解析器的验收方式（以后动这一层照做）：**先存黄金样本**（18 个 tsx 的旧产物），
    换完逐字节对比 + 12 条编译期负例 + `dist` 的 chunk hash 不变。见框架仓 `asunajs/lite` 的
  `docs/design.md` §4.6（⚠ 2026-10-07 起框架已剥离出本仓，见 `docs/web/lite.md`）。

- ⚠ **框架不够用时：把"改框架"和"绕开"摆上秤比一比再选**（用户口径 2026-10-01）：
  「有些时候前端框架不够用，这时候可以在优化前端框架和避免这个问题上对比择优」。
  **别默认绕，也别默认改** —— lite 存在的理由就是体积，框架侧 +200 B 是**全站每个用户**都付，
  页面侧绕一下只在那一页付；**同一个"绕"在第 3 处出现**才算"该改框架"的证据。
  两个要一并检查的静默坑：① 框架仓 `asunajs/lite` 的 `compiler.ts` 的语法边界（如 `.map` 的 `key`
  必须挂在 JSX 元素的子表达式位置、三元分支只认 JSX / null）；
  ② **`web/src/app.css` 顶部的 `@theme` 令牌层**（2026-10-03 起取代了 daisyUI 的
  `@plugin` + `exclude` 清单）—— **没在 `@theme` 里注册的颜色/圆角，对应的 utility 会
  "静默不生成"**：类名写在页面上、`npm run build` 也不报错，只是那条样式根本不存在 ✗
  （同一类事故的上一版是"类名在 daisyUI 的 `exclude` 里"，实测 `.tab` 的 padding/边框
  `/::after` 全为 0/none，界面退化成"一串连在一起的纯文字"）。要加新颜色就先在 `@theme`
  注册 `--color-*`，**深色那份 `[data-theme="dark"]` 要成对写** ✓。
  页面多数时候**更划算的是用本仓已有的自建控件**（如分组 tab 用 `join` + `btn join-item`，
  见 `tasks-page.tsx`）。完整判据与两个真例：框架仓 `asunajs/lite` 的 `docs/design.md` §8.0。
  ⚠ **写 lite 页面前先读框架仓的「一屏契约」**：`asunajs/lite` 的 `docs/agent.md`
  （硬约束 + 静默编错速查 + 归属/销毁，一屏；比翻 `design.md` 便宜得多）。
  （那份文件在 `web/node_modules/@asunajs/lite/` 里没有 —— 包只发 `src/`+`dist/`，
  要看去框架仓或 <https://github.com/asunajs/lite/blob/main/docs/agent.md>。）
