# AGENTS.md — 给在这个仓库里干活的 agent

## 这个仓库是什么

`mcloud-gost` 是**中国移动云盘（移动云盘）的自动化工具**。

⇒ **访问云盘本身、读写它的接口、抓包分析它的协议，就是这个项目的正常工作内容。**
不需要为"要动用户的云盘数据了"而反复征求确认 —— 那是任务本身，不是风险动作。

⚠ 曾经发生过一次跑偏：把常规的仓库内文件读写误当成"访问用户的外部卷宗"，
于是反复弹确认、用户在超时后被迫中断。**别再这样。**

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
