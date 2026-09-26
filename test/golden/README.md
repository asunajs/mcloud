# 黄金向量

**权威测试数据，请勿手改。** 本目录的文件是从上游
[`mcloud`](../../../mcloud/) 仓库同步的副本，不是在本仓库生成的。

| 文件 | 上游路径 |
|---|---|
| `crypto-vectors.json` | `mcloud/test/golden/crypto-vectors.json` |
| `app-signin-vectors.json` | `mcloud/test/golden/app-signin-vectors.json` |
| `auth-vectors.json` | `mcloud/test/golden/auth-vectors.json` |
| `password-hash-vectors.json` | `mcloud/test/golden/password-hash-vectors.json` |
| `live-room-vectors.json` | `mcloud/test/golden/live-room-vectors.json` |
| `live-redflower-vectors.json` | `mcloud/test/golden/live-redflower-vectors.json` |

## 为什么同步副本而不是直接引用

上游路径在本仓库之外，CI 与单机测试都不该依赖「旁边恰好有这个仓库」。
复制一份进来让测试自洽；代价是可能漂移，因此**改动前先回上游核对**。

同步方式：

```bash
cp ../mcloud/test/golden/crypto-vectors.json test/golden/crypto-vectors.json
cargo test -p mcloud-crypto --test golden
```

## 目前消费了哪些段

`crates/mcloud-crypto/tests/golden.rs` 用到：

- `constants` —— 盐值、刷新令牌密钥、固定 IV
- `md5` / `sha256` —— 哈希原语
- `aes128CbcFixedIvHex` —— 固定 IV 路径（密钥截断/补零语义）
- `aesDynamicIv` —— 动态 IV 路径（`byteExact` 条字节级对拍，其余靠解密断言）
- `signHeader` —— 请求签名
- `desEcbNoPaddingB64` / `desEcbPkcs7Decrypt` —— 两种 DES 填充语义
- `getTn` —— 设备指纹 `tn` 摘要的拼接输入
- `gzip` —— 只验"能解回原文 + OS 字节 == 19"（deflate 输出因实现而异）
- `rsaPkcs1v15B64` —— 只验结构（定长 128 字节、两次不同）
- `rsaSelfKeyPair` —— 用自持私钥把密文**解回明文**，这是 RSA 唯一的硬证据

`crates/mcloud-upstream/src/smid/tests/` 用到 smid 四段（设备指纹的纯计算部分）：

- `smidString` —— `smid` 字段的拼接（63 字符）
- `smidPriId` —— AES 密钥（`md5(uid)` 前 16 位）
- `smidTn` —— 摘要必须在 `tn` 还是空串时算
- `smidDesRules` —— 25 条改名 + 加密规则表的**整份**字节级对拍

> 这四段之所以落在 `mcloud-upstream` 而不是 `mcloud-crypto`：它们依赖 DES 改名表
> 与上报体形态，那是协议层的事。纯原语（DES/AES/gzip/RSA/getTn）在 crypto 侧对拍。

文件里还有 `hmacSha256B64` 等段**当前未消费**，保留给后续任务使用。
不要因为「没用到」就删——删掉后要用时得回上游重新同步。

## 对拍原则

不一致就是 bug。**不允许修改期望值来让测试变绿**；
要么实现错了，要么同步到了错误的向量版本——两种都该查清。
