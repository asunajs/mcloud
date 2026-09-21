//! 测试辅助（仅 `cfg(test)`）。
//!
//! 这里放的是**跨测试文件共享的断言**，而不是测试数据本身。
//! 只收录那些"逻辑微妙、抄一份就会跑偏"的东西 —— 典型例子是下面的标识扫描：
//! 它的边界判定有讲究（见函数注释），两份实现迟早只有一份是对的。

use base64::Engine as _;

/// 合成标识前缀。两套生成器统一把手机号与 `userDomainId` 重映射到 `199` 号段，
/// 于是"只允许 199 开头"成了一条可机检的不变量。
const SYNTHETIC_PREFIX: &str = "199";

/// 断言文本里的标识值**全部**是合成值（`199` 开头）。
///
/// 覆盖两类形状：
/// - 11 位 → 手机号
/// - 19 位 → `userDomainId`
///
/// 裸文本与其中的 base64 载荷都会扫一遍（[`decode_base64_payloads`]）。
///
/// # 为什么用"199 开头"当判据
///
/// `test/fixtures/legacy/` 与 `test/golden/` 都是**要进版本库**的生成物。
/// 它们由脚本从本机真实数据产出，脚本负责脱敏；这里断言的是"脱敏真的生效了"。
/// 一旦有人拿真实值重新生成或手改夹具，测试立刻红，而不是等它被推到远端。
///
/// # 为什么必须卡"两侧不是字母数字"
///
/// 不卡边界会全是误报：`$sourceSha256` 的十六进制串里就有 `2a18531489942b83…`
/// 这种恰好 11 位、又恰好以 1 开头的一段。真实标识在 JSON 里一定被引号、
/// 空白或标点包着，所以"两侧非字母数字"能精确区分两者。
///
/// # 已知盲区（剩下的那个）
///
/// **掩码**（如 `19****16`）不受本断言约束 —— 它含 `*`，不是连续数字串。
/// 但掩码同样是从号码派生的：由真实号 `19…16` 生成的掩码，等于泄漏前 2 后 2 位。
/// 这一类只能靠人工审 — 约定是所有示例掩码都用合成号（`19900000001` → `19****01`）派生。
///
/// （原先这里还写着一个"base64 不看"的盲区，并断言"那里装的必然是重映射后的号"。
/// 那个假定是错的：`mcloud-io`/`mcloud-server` 的测试字面量里真就藏过真实手机号，
/// 而当时的两道守卫都只扫字面、一个都没报。现已封堵。）
pub(crate) fn assert_only_synthetic_identifiers(label: &str, text: &str) {
    scan_identifiers(label, text);
    for decoded in decode_base64_payloads(text) {
        scan_identifiers(&format!("{label}（base64 解码后）"), &decoded);
    }
}

/// 在单段文本里扫 11 位与 19 位两种形状。
fn scan_identifiers(label: &str, text: &str) {
    for (width, kind) in [(11usize, "手机号"), (19usize, "userDomainId")] {
        scan_width(label, text, width, kind);
    }
}

fn scan_width(label: &str, text: &str, width: usize, kind: &str) {
    let bytes = text.as_bytes();

    for (start, _) in text.match_indices('1') {
        let end = start + width;
        if end > bytes.len() || !bytes[start..end].iter().all(u8::is_ascii_digit) {
            continue;
        }
        // 串首/串尾也算"边界"，故用 `is_none_or`。
        //
        // 这条边界同时挡住了"19 位号码被 11 位规则切一段"的误判：
        // `1990000000000000001` 的第 12 位是数字，右边界不成立，于是跳过。
        let bounded = |i: Option<usize>| i.is_none_or(|i| !bytes[i].is_ascii_alphanumeric());
        if !bounded(start.checked_sub(1)) || !bounded(Some(end)) {
            continue;
        }
        let candidate = &text[start..end];
        assert!(
            candidate.starts_with(SYNTHETIC_PREFIX),
            "{label} 里出现了疑似真实{kind} {candidate}（只有 {SYNTHETIC_PREFIX} 开头的合成值才允许出现）"
        );
    }
}

/// 取出文本里所有"看起来是 base64"的片段并解码，返回其中能当文本读的部分。
///
/// 存在的理由是上面说的那个踩过的坑：base64 把号码从"字面"里藏起来，
/// 任何基于 `grep` 或字面正则的审计都会整段漏过。
///
/// 只收"解码后没有控制字符"的结果，用来滤掉十六进制串（sha256 之类）
/// 被当成 base64 硬解出来的二进制垃圾。
fn decode_base64_payloads(text: &str) -> Vec<String> {
    /// 太短的串不具备判别力，且会大量误入（真 auth 有 40+ 字符）。
    const MIN_CHUNK: usize = 16;

    let bytes = text.as_bytes();
    let is_b64 = |b: u8| b.is_ascii_alphanumeric() || b == b'+' || b == b'/';

    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_b64(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_b64(bytes[i]) {
            i += 1;
        }
        // 尾部填充也吃进来，否则 STANDARD 引擎会因长度不对而失败。
        let mut end = i;
        while end < bytes.len() && bytes[end] == b'=' {
            end += 1;
        }
        let chunk = &text[start..end];
        if chunk.len() < MIN_CHUNK {
            continue;
        }

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(chunk)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(chunk));
        if let Ok(raw) = decoded
            && let Ok(text) = String::from_utf8(raw)
            && !text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            out.push(text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 守卫自己也要被守卫：先证明它对**裸文本**和 **base64** 都能失败。
    ///
    /// 一个不会失败的守卫等于没有守卫 —— 这道守卫历史上就漏过一次
    /// （见 `assert_only_synthetic_identifiers` 的盲区说明），所以补上这组自测。
    ///
    /// 这里刻意用**非真实的**非 199 号码：判据是形状，不需要真值。
    /// 把真实号码写进本文件，等于让守卫自己制造一次它要防的那件事。
    #[test]
    #[should_panic(expected = "疑似真实")]
    fn guard_rejects_non_synthetic_phone() {
        assert_only_synthetic_identifiers("t", "phone=13800000000;");
    }

    #[test]
    #[should_panic(expected = "疑似真实")]
    fn guard_rejects_non_synthetic_domain_id() {
        assert_only_synthetic_identifiers("t", r#""userDomainId":"1000000000000000001""#);
    }

    #[test]
    #[should_panic(expected = "base64 解码后")]
    fn guard_rejects_phone_hidden_in_base64() {
        let hidden = base64::engine::general_purpose::STANDARD.encode("2:13800000000:t");
        assert_only_synthetic_identifiers("t", &format!(r#""auth":"Basic {hidden}""#));
    }

    #[test]
    fn guard_accepts_synthetic_values() {
        let ok = base64::engine::general_purpose::STANDARD.encode("2:19900000001:ak|bk");
        assert_only_synthetic_identifiers("t", &format!(r#""auth":"Basic {ok}""#));
        assert_only_synthetic_identifiers("t", r#""userDomainId":"1990000000000000001""#);
    }

    /// 十六进制串不能被误当成标识（也顺手证明 hex 不会被 base64 解码后误报）。
    #[test]
    fn guard_ignores_sha256_hex_and_long_digit_runs_inside_hex() {
        assert_only_synthetic_identifiers(
            "t",
            r#""$sourceSha256":{"a":"2a18531489942b83aa2a18531489942b83"}"#,
        );
    }
}
