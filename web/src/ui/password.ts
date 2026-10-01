/**
 * 口令的**前端那一次哈希**。
 *
 * 用户口径（2026-10-01）：前端 hash 一次、后端再 hash 一次落库。
 *
 * # 为什么是裸 SHA-256
 *
 * 它必须与 Rust 那份（`mcloud-crypto::client_password_digest`）**逐字节一致** ——
 * 不一致的表现是"口令明明对、就是登不上"，而且换个端才复现。所以两边都钉了
 * 对拍向量：`SHA-256("abc") = ba7816…15ad`（Rust 单测 `client_digest_matches_the_web_side`
 * + `lite/regress` 的登录闸直接断言请求体是摘要）。
 *
 * 选裸 SHA-256 而不是慢哈希：抗暴力破解由服务端那次 argon2id 负责，
 * 这里再做一遍慢哈希只会让每次登录多等一百毫秒，而**收益是零** ——
 * 抓到摘要的人直接重放就行，不需要还原明文。
 *
 * # ⚠ 不是所有上下文都算得出来
 *
 * `crypto.subtle` **只存在于安全上下文**：`https://`，或 `localhost` / `127.0.0.1`。
 * 从 LAN 用 `http://192.168.x.x:3000` 打开时它是 `undefined` ⇒ 退回**发明文**，
 * 服务端认那条路（`auth.rs::login` 的兼容口子：收到明文就现场补一次摘要）。
 *
 * 这是**有意的降级**，不是漏：那条路上跟改之前一模一样，而不是"登录失败"。
 * 想要全程"明文不出浏览器"，就给它配 HTTPS（或只在 localhost 上用）。
 */

import type { PasswordScheme } from '../api'

/**
 * 前端那一次哈希：`SHA-256(明文)` 的小写 hex。
 *
 * 算不了（非安全上下文 / 算法不可用）时返回 `null`，调用方退回明文。
 */
export const clientPasswordDigest = async (plain: string): Promise<string | null> => {
  const subtle = globalThis.crypto?.subtle
  if (!subtle) return null
  try {
    const buf = await subtle.digest('SHA-256', new TextEncoder().encode(plain))
    return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('')
  } catch {
    // 理论路径（算法被禁 / 上下文被撤销）。宁可发明文，也别把人挡在登录页外。
    return null
  }
}

/**
 * 按服务端自述的档位，把明文换算成**这一次请求该发的凭据**。
 *
 * - `sha256`：发摘要（目标形态）
 * - `plain`：发**明文** —— 库里是老档（argon2 直接吃明文），发摘要谁也验不过；
 *   服务端验过之后当场升级，**下一次**登录就轮到 `sha256` 了。
 */
export const credentialFor = async (plain: string, scheme: PasswordScheme): Promise<string> => {
  if (scheme === 'plain') return plain
  return (await clientPasswordDigest(plain)) ?? plain
}
