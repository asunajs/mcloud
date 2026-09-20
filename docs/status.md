# 实现进度与现状须知

> 状态：**实况**（这里写的是"现在能跑什么、还差什么"，不是计划）
>
> - 最后核对：2026-09-19 收工
> - 规划（**写于动工前**，进度不要以它为准）：[`rust-rewrite-plan.md`](./rust-rewrite-plan.md)
> - 实现顺序（先做哪项能力）：同文档 §7.1
> - 门禁怎么跑：[`dev-gates.md`](./dev-gates.md)

---

## 一句话总览

| 层 | 状态 |
| --- | --- |
| CLI：初始化 / 登录 | ✅ 可用（扫码 + 短信，PC 端与 App 端） |
| CLI：签到（云朵中心） | ✅ 端到端跑通（含领取、复活卡） |
| 上游接口层 | ⚠ **逐块重写中** —— 只有 `login` 与 `app_signin` 是完整的，其余仍是骨架 |
| 服务入口 | ⚠ 能起，但**只有"会话"这一层**，跑任务的那批路由仍在 P5 |
| WebUI | ⚠ 登录与初始化是真接口，**主界面仍是 P0 spike 的占位数据** |
| 运行时层 | ⚠ 事件模型 / 生命周期 / 事件总线 / `run_events` 表已落地，编排与调度接入中 |
| 夹具回放（L2） | ❌ **尚无消费者** —— `test/fixtures/http/` 里的夹具一个断言都没跑 |

---

## 历史流水账（已折叠，只留结论）

测试数一路涨上来的过程本身不构成"当前状态"，故压成下表。当前数见下方「门禁实况」。

| 时间 | 测试数 | 当轮新增了什么 |
| --- | --- | --- |
| 2026-09-17 | 241 | 当时的一帧 |
| —— | 493 | 接完 smid（设备指纹绑账号）那条线，自贡献 46 条（上游算法 25 + 生命周期 13 + 存储 8） |
| —— | 516 | App 签到线：上游接口层 + 编排层 + 存储层（`account_state.phone`，schema v6） |
| —— | 523 | 云朵余额采集（`mcloud-app::cloud` 5 条 + `set_account_cloud` 窄写 2 条） |
| —— | 533 | `receiveV3` 领取（请求构造 / 解析 / 错误码 10 条） |
| —— | 541 | `revival` 复活卡（8 条，含一条读黄金向量的） |
| —— | 542 | 查明 H5 `deviceId` 根因并接线（新增一条把签到页 `_smConf` 与 Rust 常量钉在一起的守卫） |
| —— | 546 | `Authorization` 改为整串存整串用（schema v7），新增 4 条 |
| —— | 554 | CLI 账号选择（下标语义）补 9 条用例 |
| 2026-09-19 | **586** | 把云端那 9 个提交拉进本地（`mcloud-app` 组装层、App 签到全链路、schema v7 等），并修掉两处 clippy |

> 这里曾经把 554 写成"当前数"—— 那是**拉取云端之前**本机的数。
> 云端那 9 个提交带进来的 `mcloud-app` 等新模块另有自己的 lib 测试，落地后实跑是 **586**。

**门禁实况（2026-09-19）**：**586 个测试通过**，七条门禁全绿
（黄金向量那段是 **122 条断言**，见 [`dev-gates.md`](./dev-gates.md)）。

---

## 逐项须知

### 服务入口：能起，但只有"会话"这一层

`cargo run -p mcloud-server` 起 API + 前端产物，路由目前只有
`/api/health`、`/api/setup`、`/api/session`（白名单免登录，其余一律要会话）。

跑任务的那一批（accounts / capabilities / runs / schedules）、SSE 与 `openapi.json`
导出仍在 P5 —— 所以 **WebUI 登录与初始化是真接口，主界面仍是 P0 spike 的占位数据**。

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

### ⚠ 夹具回放（L2）尚无消费者

此前本文件写着"`mcloud-upstream` 的 L2 夹具回放已跑通
（`crates/mcloud-upstream/tests/l2_replay.rs`）"—— **那个文件不存在**，
`crates/mcloud-upstream/tests/` 是个**空目录**。

现状是：`mcloud fixture extract` 往 `test/fixtures/http/` 写，但**没有任何消费者**，
所以那 31 个夹具目前一个断言都没跑。这是规划 §6.3 里要补的一环，
**别按"已经有了"来理解**。

已确认可用的样本（写回放时直接照着接）：

| 接口 | 在哪个夹具 |
| --- | --- |
| `startSignIn` / `infoV3` / `doTaskPost` | `3d.json` |
| `taskListV3` | `每月阅读任务.json` |
| `receiveRevivalReward` | `复活.json` |
| `receiveV3` | **没有任何夹具**（只能靠单测 + 真机） |

### 旧 TS 与线上实际请求本就不一致，重写时要逐条核对

上一轮对拍换来的经验，**别默认"照 TS 抄"就是对的**：

| 接口 | TS 写法 | 真实流量 |
| --- | --- | --- |
| `visitlog/journaling` | 参数放在 **query** | 参数放在**表单体** |
| `signin/task/taskListV2` | `clientVersion=11.2.1`、`group=month` | `clientVersion=11.4.0`、`group=new` |
| `auth-service/auth/tyrzLogin` | body 带 `type`/`channel` | body 无这两个字段，`marketName=newsign_139mail` |
| `market/signin/page/infoV3` | 不带端口的 `/market/...` | `caiyun.feixin.10086.cn:7071/ycloud/...` |
| `noteRefreshToken` | `note.mcloud.139.com/noteServer/...` | `note-njs.yun.139.com/yun-note/...`（另一个域） |

另有：`refreshToken` 的真实流量是**加密载荷**（原实现注释却说是明文 JSON 接口）；
`auth/refreshToken`、`querySpecTokenV2` 这类接口的 `code` 是**字符串**不是数字。

---

## 按任务指路

| 你要做的事 | 读哪篇 |
| --- | --- |
| 跑门禁 / 看分层约束 | [`dev-gates.md`](./dev-gates.md) |
| 敲命令行 / 查参数 | [`cli-usage.md`](./cli-usage.md) |
| 加文件 / 起新目录 | [`repo-layout.md`](./repo-layout.md) |
| 实现某个业务能力 | [`business-feature-inventory.md`](./business-feature-inventory.md) |
| 全部文档的索引 | [`README.md`](./README.md) |
