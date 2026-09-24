# 直播口令 / 小红花：开播后核对记录

> 由 `node scripts/probe-live-room.js` 生成，**追加**式。
> ⚠ 清单的权威版本在**脚本里**（`ITEMS`），这里只是**记录**。
> ⚠ 口令**当场有效**，所以每次记录都带时刻 —— 没有时刻的样本事后没法判"是不是过期了"。

## 运行记录 —— 2026-09-23 15:09:05

参数：`--db data/mcloud.db` `--listen 45`

**脚本退出码：0** —— 这一趟会跑的项都拿到了（⚠ 要抓包的几条本来就不在这一趟）

| 项 | 结果 | 说明 |
| --- | --- | --- |
| 前置：门开着，且有「中国移动云盘」的直播间在播 | ✅ 拿到 | 门开着，在播 3 间 |
| 一条**真口令**（当场从弹幕 / 小红书收到） | ✅ 拿到 | 收到 2 条：一键9图出圈 / 中秋美好存云盘 |
| `visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认 | 🔌 要抓包 | （见 --wire） |
| WS 握手签名头 `systemmd5` / `tokenmd5` 的真实取值 + `member/apply` 的 `version` | 🔌 要抓包 | （见 --wire） |
| WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」 | 🔌 要抓包 | （见 --wire） |


### 前置：门开着，且有「中国移动云盘」的直播间在播

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 1 --json
CLI 退出码 0（**CLI 的**，不是脚本的档位；CLI 的 0 = "正常跑完"）
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****42",
      "failed": false,
      "gateOpen": true,
      "index": 1,
      "kouling": [
        "中秋美好存云盘"
      ],
      "liveCount": 3,
      "retried": false
    },
    {
      "account": "19****16",
      "failed": false,
      "gateOpen": true,
      "index": 2,
      "kouling": [
        "中秋美好存云盘"
      ],
      "liveCount": 3,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15****42 开门中…
[1] 15****42 在播房间 147606，听 1 秒弹幕…
[2] 19****16 开门中…
[2] 19****16 在播房间 147606，听 1 秒弹幕…
```

### 一条**真口令**（当场从弹幕 / 小红书收到）

```
$ mcloud live-room --db data/mcloud.db --kouling-only --listen-seconds 45 --json
CLI 退出码 0（**CLI 的**，不是脚本的档位；CLI 的 0 = "正常跑完"）
--- stdout ---
{
  "liveRoom": [
    {
      "account": "15****42",
      "failed": false,
      "gateOpen": true,
      "index": 1,
      "kouling": [
        "一键9图出圈",
        "中秋美好存云盘"
      ],
      "liveCount": 3,
      "retried": false
    },
    {
      "account": "19****16",
      "failed": false,
      "gateOpen": true,
      "index": 2,
      "kouling": [
        "一键9图出圈",
        "中秋美好存云盘"
      ],
      "liveCount": 3,
      "retried": false
    }
  ]
}
--- stderr ---
[1] 15****42 开门中…
[1] 15****42 在播房间 147606，听 45 秒弹幕…
[2] 19****16 开门中…
[2] 19****16 在播房间 147606，听 45 秒弹幕…
```

