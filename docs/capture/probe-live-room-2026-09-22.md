# 直播口令 / 小红花：开播后核对记录

> 由 `node scripts/probe-live-room.js` 生成，**追加**式。
> ⚠ 清单的权威版本在**脚本里**（`ITEMS`），这里只是**记录**。
> ⚠ 口令**当场有效**，所以每次记录都带时刻 —— 没有时刻的样本事后没法判"是不是过期了"。
> ⚠ **两个退出码别混**：早期几条（19:22 / 19:41 / 19:42）raw 块里那个裸的「退出码 N」是
> **CLI 的**（CLI 的 `0` = "它正常跑完"，"今天没开播"也是 `0`）；**20:06 之后**每条记录
> 开头多了「**脚本退出码**」一行，raw 块里也改写成「CLI 退出码」，两者才分得开。
> ⇒ 早期那几条**看不出脚本自己的档位**，但它们都是同一处境（`liveCount:0`，脚本退 `2`）。
>
> ⚠⚠ **2026-09-23 补记（本文件是留档，正文不改，但下面这些"当时的话"现在是错的）**：
> 每条记录里都有一行 `draw` 的 `200112`，写着"这条要**消耗**一个口令（不可逆），所以不自动跑"。
> **前提错了** —— 口令是**全场通用**的，**不存在"消耗口令"这回事**（谁投过都不影响别人）；
> `200112` 的真正语义是"**同一账号**第二次请求同一条**正确**口令 ⇒ 已兑换"，
> 它说的是**按账号去重**，不是"口令被消耗"。
> ⇒ 该核对项**已从脚本的 `ITEMS` 删掉**（清单 6 条 → 5 条，`--code` 开关一并删）：
> "投两次造样本"这条路本来就不该走。**`200112` 只能等真实触发**。
> 五条事实与这条语义的完整口径见 `docs/status.md`「口令的五条事实」与
> `internal/upstream/live_flower.go` 文件头。

## 运行记录 —— 2026-09-22 19:22:38

参数：`--db data/mcloud.db` `--listen 5`

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 前置：门开着，且有「中国移动云盘」的直播间在播 | ❌ 没拿到 | 门没开：liveCount=0（0 = 谁都没在播；>0 = 别的主播在播） |
| 一条**真口令**（当场从弹幕 / 小红书收到） | ❌ 没拿到 | 听了 5 秒，两个来源一条都没收到（门没开 —— 门开着却没口令才是"主播没喊"） |
| ~~`draw` 的 `200112`（这个口令已兑换过）—— **至今没有样本**~~ ⛔ **该项已于 2026-09-23 移除** | ⏭ 未跑 | 当时的话是"没给 `--code`。这条要消耗一个口令（不可逆），所以不自动跑。" ⚠⚠ **前提错了** —— 口令**全场通用、不存在消耗**；`200112` = **同一账号**第二次请求同一条**正确**口令。见文件头补记。 |
| `visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认 | 🔌 要抓包 | （见 --wire） |
| WS 握手签名头 `systemmd5` / `tokenmd5` 的真实取值 + `member/apply` 的 `version` | 🔌 要抓包 | （见 --wire） |
| WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」 | 🔌 要抓包 | （见 --wire） |

⚠ **前置不成立**（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。

### 前置：门开着，且有「中国移动云盘」的直播间在播

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```

### 一条**真口令**（当场从弹幕 / 小红书收到）

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 5 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```


## 运行记录 —— 2026-09-22 19:41:26

参数：`--db data/mcloud.db` `--listen 1`

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 前置：门开着，且有「中国移动云盘」的直播间在播 | ❌ 没拿到 | 门没开：liveCount=0（0 = 谁都没在播；>0 = 别的主播在播） |
| 一条**真口令**（当场从弹幕 / 小红书收到） | ❌ 没拿到 | 听了 1 秒，两个来源一条都没收到（门没开 —— 门开着却没口令才是"主播没喊"） |
| ~~`draw` 的 `200112`（这个口令已兑换过）—— **至今没有样本**~~ ⛔ **该项已于 2026-09-23 移除** | ⏭ 未跑 | 当时的话是"没给 `--code`。这条要消耗一个口令（不可逆），所以不自动跑。" ⚠⚠ **前提错了** —— 口令**全场通用、不存在消耗**；`200112` = **同一账号**第二次请求同一条**正确**口令。见文件头补记。 |
| `visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认 | 🔌 要抓包 | （见 --wire） |
| WS 握手签名头 `systemmd5` / `tokenmd5` 的真实取值 + `member/apply` 的 `version` | 🔌 要抓包 | （见 --wire） |
| WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」 | 🔌 要抓包 | （见 --wire） |

⚠ **前置不成立**（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。

### 前置：门开着，且有「中国移动云盘」的直播间在播

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```

### 一条**真口令**（当场从弹幕 / 小红书收到）

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```


## 运行记录 —— 2026-09-22 19:41:45

参数：`--db data/mcloud.db` `--listen 1`

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 前置：门开着，且有「中国移动云盘」的直播间在播 | ❌ 没拿到 | 门没开：liveCount=0（0 = 谁都没在播；>0 = 别的主播在播） |
| 一条**真口令**（当场从弹幕 / 小红书收到） | ❌ 没拿到 | 听了 1 秒，两个来源一条都没收到（门没开 —— 门开着却没口令才是"主播没喊"） |
| ~~`draw` 的 `200112`（这个口令已兑换过）—— **至今没有样本**~~ ⛔ **该项已于 2026-09-23 移除** | ⏭ 未跑 | 当时的话是"没给 `--code`。这条要消耗一个口令（不可逆），所以不自动跑。" ⚠⚠ **前提错了** —— 口令**全场通用、不存在消耗**；`200112` = **同一账号**第二次请求同一条**正确**口令。见文件头补记。 |
| `visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认 | 🔌 要抓包 | （见 --wire） |
| WS 握手签名头 `systemmd5` / `tokenmd5` 的真实取值 + `member/apply` 的 `version` | 🔌 要抓包 | （见 --wire） |
| WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」 | 🔌 要抓包 | （见 --wire） |

⚠ **前置不成立**（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。

### 前置：门开着，且有「中国移动云盘」的直播间在播

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```

### 一条**真口令**（当场从弹幕 / 小红书收到）

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```


## 运行记录 —— 2026-09-22 19:42:14

参数：`--db data/mcloud.db` `--listen 1`

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 前置：门开着，且有「中国移动云盘」的直播间在播 | ❌ 没拿到 | 门没开：liveCount=0（0 = 谁都没在播；>0 = 别的主播在播） |
| 一条**真口令**（当场从弹幕 / 小红书收到） | ❌ 没拿到 | 听了 1 秒，两个来源一条都没收到（门没开 —— 门开着却没口令才是"主播没喊"） |
| ~~`draw` 的 `200112`（这个口令已兑换过）—— **至今没有样本**~~ ⛔ **该项已于 2026-09-23 移除** | ⏭ 未跑 | 当时的话是"没给 `--code`。这条要消耗一个口令（不可逆），所以不自动跑。" ⚠⚠ **前提错了** —— 口令**全场通用、不存在消耗**；`200112` = **同一账号**第二次请求同一条**正确**口令。见文件头补记。 |
| `visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认 | 🔌 要抓包 | （见 --wire） |
| WS 握手签名头 `systemmd5` / `tokenmd5` 的真实取值 + `member/apply` 的 `version` | 🔌 要抓包 | （见 --wire） |
| WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」 | 🔌 要抓包 | （见 --wire） |

⚠ **前置不成立**（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。

### 前置：门开着，且有「中国移动云盘」的直播间在播

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```

### 一条**真口令**（当场从弹幕 / 小红书收到）

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
退出码 0
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```


## 运行记录 —— 2026-09-22 20:06:07

参数：`--db data/mcloud.db` `--listen 1`

**脚本退出码：2** —— 前置不成立（现在没在播）⇒ 后几条**不适用**，不是"核过了没拿到"

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 前置：门开着，且有「中国移动云盘」的直播间在播 | ❌ 没拿到 | 门没开：liveCount=0（0 = 谁都没在播；>0 = 别的主播在播） |
| 一条**真口令**（当场从弹幕 / 小红书收到） | ❌ 没拿到 | 听了 1 秒，两个来源一条都没收到（门没开 —— 门开着却没口令才是"主播没喊"） |
| ~~`draw` 的 `200112`（这个口令已兑换过）—— **至今没有样本**~~ ⛔ **该项已于 2026-09-23 移除** | ⏭ 未跑 | 当时的话是"没给 `--code`。这条要消耗一个口令（不可逆），所以不自动跑。" ⚠⚠ **前提错了** —— 口令**全场通用、不存在消耗**；`200112` = **同一账号**第二次请求同一条**正确**口令。见文件头补记。 |
| `visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认 | 🔌 要抓包 | （见 --wire） |
| WS 握手签名头 `systemmd5` / `tokenmd5` 的真实取值 + `member/apply` 的 `version` | 🔌 要抓包 | （见 --wire） |
| WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」 | 🔌 要抓包 | （见 --wire） |

⚠ **前置不成立**（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。

### 前置：门开着，且有「中国移动云盘」的直播间在播

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
CLI 退出码 0（**CLI 的**，不是脚本的档位；CLI 的 0 = "正常跑完"）
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```

### 一条**真口令**（当场从弹幕 / 小红书收到）

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
CLI 退出码 0（**CLI 的**，不是脚本的档位；CLI 的 0 = "正常跑完"）
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****",
      "failed": false,
      "gateOpen": false,
      "index": 1,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    },
    {
      "account": "19****",
      "failed": false,
      "gateOpen": false,
      "index": 2,
      "kouling": null,
      "liveCount": 0,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15**** 开门中…
[1] 今天没有直播（中国移动云盘 一间都没在播）
[2] 19**** 开门中…
[2] 今天没有直播（中国移动云盘 一间都没在播）
```


## 补记 —— 2026-09-22 23:16：走 WebUI（`POST /api/runs`）跑的那一批

> 上面那些记录都是 **CLI 直跑**。这一条记的是**同一支链经 WebUI 触发**的实况，
> 因为这一支新加的坑**只在 WebUI 侧才露**（CLI 是"自己跑自己看"，没有事件流这一层）。
> ⚠ 库是**副本**（`target/verify.db`）、服务是**另起的**（127.0.0.1:8794，二进制 `target/mcloud-server-live`），
> 没碰 `data/mcloud.db`，也没碰别人在 8788 上那个进程。

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 202 先回、结果靠事件流（`runs:[{accountId,runId}]`） | ✅ 成立 | 两个账号当场各得一个 `runId`，界面按它订阅两条 SSE |
| 一条链跑完，两卡都出结论 | ✅ 成立 | 结论 `失败`（口令无效 ⇒ 正常），日志 `开门中…` → `投 1 个口令…` |
| 投**不存在**的口令之后余额变没变 | ✅ 没变 | `15****`：`235 → 235`；`19****`：`110 → 110`（进账 0，成功 0/1）。与 `app-live-redflower-samples.txt` §5.1 那两格同向 ⇒ "投无效口令"可当核验手段 |
| 第二个账号的事件流能不能连上 | ❌ **第一跑连不上** | 第二卡停在"连接中断 / 没连上事件流"，可库里那一趟**跑完了** —— 判据与修法见 `docs/migration-pitfalls.md` **#129**（202 里的 `runId` 是预分配的，而 `runs` 行要等后台 goroutine 轮到那个账号才写 ⇒ 第 2..N 个必然有一段 404 窗口，`EventSource` 对 404 不重连） |
| 同上，改完（pending 期间退避重订阅） | ✅ 两卡都出日志 | ⚠ web 侧**没有测试运行器**，这一条只有真机判据；窄屏 375 与桌面 1280、深浅两套主题都各自量过 |
