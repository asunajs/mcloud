# 黄金向量

**权威测试数据，请勿手改。** 本目录的文件是从上游
[`mcloud`](../../../mcloud/) 仓库同步的副本，不是在本仓库生成的。

| 文件 | 上游路径 |
|---|---|
| `crypto-vectors.json` | `mcloud/test/golden/crypto-vectors.json` |

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

文件里还有 `desEcb*` / `rsaPkcs1v15B64` / `getTn` / `gzip` / `smid*` 等段，
**当前未消费**，保留给后续任务（设备指纹、RSA 加密、XML 接口）使用。
不要因为「没用到」就删——删掉后要用时得回上游重新同步。

## 对拍原则

不一致就是 bug。**不允许修改期望值来让测试变绿**；
要么实现错了，要么同步到了错误的向量版本——两种都该查清。
