//! 凭据串的解析与拼装（`Authorization` 的载荷）。
//!
//! # 为什么这一层在 `mcloud-upstream` 而不是 `mcloud-core`
//!
//! `Basic base64("<前缀>:<手机号>:<authToken>")` 是**上游的 wire 契约**，
//! 不是本系统的领域概念 —— 领域侧只该看到一个不透明的 [`AuthInfo::raw`]。
//! 放这里的另一个现实理由：解析要 base64，而 `mcloud-core` 刻意不背这个依赖
//! （它只装领域类型与 trait）。
//!
//! # 三个易错点（原实现的坑，逐条保留）
//!
//! 1. `replace("Basic ", "")` **只替换第一处** —— JS 的 `String.replace(串, "")`
//!    就是这个语义。用 `trim_start_matches` 会把中间出现的 `Basic ` 也吃掉。
//! 2. 按 `:` 切分**只取前 3 段**：`authToken` 里含 `:` 时，后面那截会被丢掉。
//!    这是原实现的行为，改成"取全部再拼回去"会让 token 悄悄变长。
//! 3. 过期时间取自 `token` 的第 4 段（下标 3），且 `0` 与非法值都算"没有过期信息"
//!    —— 对应 JS 的 `Number(x) || 0`（`0` / `NaN` / 负数以外的 falsy 都归 0）。
//!    单位是**毫秒**（见 [`crate::login`] 的说明与 `AuthInfo::expire`）。
//!
//! [`AuthInfo::raw`]: mcloud_core::AuthInfo::raw

use mcloud_core::{AuthInfo, Error, Result};

/// PC 端 `Authorization` 载荷的第一段。
///
/// # 它**不区分** Windows 与 macOS
///
/// 客户端源码里是字面量 `pc`（不是平台号）—— 这正是"光看凭据分不出平台"
/// 的原因，也是 `Account::pc_platform` 必须单独落盘的理由。
pub const PC_AUTH_PREFIX: &str = "pc";

/// App 端 `Authorization` 载荷的第一段（实测 1076 处解码后全是 `mobile:<手机号>:<authToken>`）。
pub const APP_AUTH_PREFIX: &str = "mobile";

/// 解析结果：`AuthInfo` 加上从串里还原出的分段。
///
/// 与 [`AuthInfo`] 的关系：`AuthInfo` 是**要落盘**的形态（含 `raw`），
/// 这里只是解析过程中的中间产物，所以不引入新类型，直接构造 `AuthInfo`。
///
/// # 为什么 `platform` 与 `phone` 允许为空
///
/// 上游的串**可能缺段**（实测有只有两段的样本）。缺段时留空串而不是报错：
/// 报错会让一条"能用但信息不全"的凭据变成不可用，代价不对等。
/// 但调用方要判"是不是 PC 端"时，必须**显式比对前缀**，不能靠"非空"。
pub fn parse_auth(auth: &str) -> Option<AuthInfo> {
    if auth.is_empty() {
        return None;
    }

    // 只替换**第一处** `Basic `（对齐 JS 的 `replace(串, "")`），
    // 且它出现在中间时也要替换 —— JS 的 replace 不认位置，只认"第一处"。
    let clean = match auth.find("Basic ") {
        Some(i) => format!("{}{}", &auth[..i], &auth[i + "Basic ".len()..]),
        None => auth.to_string(),
    };

    // ⚠ 用**宽容**的 base64 解码，而不是 `base64` crate 的严格模式。
    //
    // Node 的 `Buffer.from(s, "base64")` 会**忽略字母表以外的字符**，
    // 而严格模式直接报错。差异只出现在畸形输入上，但那正是这一层要处理的东西：
    // 用严格模式，"上游塞了个空白字符"会变成"凭据整条不可用"。
    //
    // 宽容模式的**下界**同样重要：过滤后长度模 4 余 1 是无解的，
    // 必须判为不可用（否则会解出无意义的半个字节）。
    let bytes = node_like_b64_decode(&clean)?;
    let raw = String::from_utf8(bytes).ok()?;

    // 只取前 3 段：第 4 段起被丢弃（token 含 `:` 会被截断）
    let mut parts = raw.splitn(4, ':');
    let platform = parts.next().unwrap_or("").to_string();
    let phone = parts.next().unwrap_or("").to_string();
    let token = parts.next().unwrap_or("").to_string();

    // `Number(token?.split("|")[3]) || 0`
    //
    // 按 **f64** 解析再截断，理由是原实现就是浮点：
    // - `is_plain_decimal` 是"合法十进制数"的判据（拒绝 `notanumber`、`.`、`+.`）；
    // - `|| 0` 把 `0` 与 `-0` 都归零，也把 `NaN` 归零；
    // - 截断对齐 Go 参考的 `ExpireMillis()`（`int64(a.Expire)`）——
    //   小数毫秒没有实际意义，真实 token 都是整数。
    let expire = token
        .split('|')
        .nth(3)
        .map(str::trim)
        .filter(|f| is_plain_decimal(f))
        .and_then(|f| f.parse::<f64>().ok())
        .filter(|v| *v != 0.0)
        .map(|v| v as i64)
        .unwrap_or(0);

    Some(AuthInfo {
        raw: format!("Basic {clean}"),
        platform,
        phone,
        token,
        expire,
    })
}

/// 复刻 Node `Buffer.from(s, "base64")` 的宽容解码。
///
/// 规则（对齐参考实现，逐条都有向量钉住）：
/// 1. 只取 `=` **之前**的内容（填充由我们自己补）；
/// 2. 丢掉字母表以外的字符（空白、`!`、`-` 等）；
/// 3. 过滤后为空、或长度模 4 余 1 ⇒ **不可解**（余 1 无法构成任何字节）；
/// 4. 解出来是空字节串也算不可解 —— 避免"解出了空凭据"这种假成功。
fn node_like_b64_decode(s: &str) -> Option<Vec<u8>> {
    let payload: String = s
        .chars()
        .take_while(|c| *c != '=')
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/')
        .collect();

    if payload.is_empty() || payload.len() % 4 == 1 {
        return None;
    }

    let padded = pad_base64(&payload);
    mcloud_crypto::b64_decode(&padded)
        .ok()
        .filter(|b| !b.is_empty())
}

/// 是否是"朴素的十进制数"（对齐参考实现 `is_plain_decimal`）。
///
/// 比 `f64::from_str` 更严：`f64` 接受 `inf` / `NaN` / `1e5` 之类，
/// 而原实现用正则式的逐字符判断。这里手工走一遍，保证 `notanumber`、
/// 单独一个 `.`、`+.` 都被判为非法（它们的 `Number()` 是 `NaN` ⇒ 归 0）。
fn is_plain_decimal(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    let mut i = 0;
    if b[i] == b'+' || b[i] == b'-' {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;

    let mut frac_digits = 0usize;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - frac_start;
    }
    // `.` 两侧至少要有数字，否则 `.`、`+.` 会被当成 0
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i == b.len() {
        return true;
    }

    // 指数部分
    if b[i] != b'e' && b[i] != b'E' {
        return false;
    }
    i += 1;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let exp_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    i > exp_start && i == b.len()
}

/// 按 `base64(前缀:账号:authToken)` 拼出整串。
///
/// # 前缀是**必填参数**，不给默认值
///
/// 默认值会让"给 App 的登录结果套了 PC 的前缀"变成一个静默通过的调用，
/// 而那正是最难在事后发现的一类错误（现象只是"忽然回 90001 未登录"）。
pub fn format_auth(prefix: &str, account: &str, auth_token: &str) -> Result<String> {
    if account.is_empty() || auth_token.is_empty() {
        return Err(Error::Parse("拼 auth 缺少账号或 authToken".into()));
    }
    Ok(format!(
        "Basic {}",
        mcloud_crypto::b64_encode(format!("{prefix}:{account}:{auth_token}").as_bytes())
    ))
}

/// 手机号掩码：前 2 + `****` + 后 2（对齐原实现，如 `19900000001` → `19****01`）。
///
/// 长度不足 4 位时原样返回：掩码成 `****` 会让"这是个短号"这件事消失，
/// 而短号只可能来自脏数据，原样显示更利于排查。
pub fn mask_phone(phone: &str) -> String {
    let chars: Vec<char> = phone.chars().collect();
    if chars.len() < 4 {
        return phone.to_string();
    }
    let head: String = chars[..2].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}****{tail}")
}

/// 账号显示名 / 主键：有手机号就用掩码，否则回落到昵称，再否则用占位符。
pub fn account_label(nickname: Option<&str>, phone: &str) -> String {
    if !phone.is_empty() {
        return mask_phone(phone);
    }
    match nickname {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => "account".to_string(),
    }
}

/// base64 补齐到 4 的倍数（对齐 JS 的 `padBase64`）。
///
/// 上游给的串**经常缺 `=` 填充**（实测样本里就有），直接解会失败。
fn pad_base64(s: &str) -> String {
    let rem = s.len() % 4;
    if rem == 0 {
        s.to_string()
    } else {
        format!("{s}{}", "=".repeat(4 - rem))
    }
}

#[cfg(test)]
mod tests;
