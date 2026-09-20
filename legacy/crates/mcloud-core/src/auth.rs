//! 账号身份解析 —— 与 TS 原版逐字对齐。
//!
//! # 为什么这层必须精确到"怪癖"
//!
//! `auth` 字符串解出来的 phone 决定 [`AccountId`]，而 `AccountId` 是旧
//! `data/state.json` 的唯一键。解析错一位，历史数据就会挂到**错误的账号**上 ——
//! 而且不会报错，只会静默串号。所以这里连"token 里带冒号会被截断"这种怪癖都要保留。
//!
//! 对拍基准：`test/golden/auth-vectors.json`，由 `scripts/gen-auth-vectors.cjs`
//! **直接 import 原版编译产物**（`packages/core/dist/.../auth/utils.js`）产出。
//!
//! # auth 的格式
//!
//! ```text
//! auth    = ["Basic "] + base64("<platform>:<phone>:<token>")
//! token   = "<f0>|<f1>|<f2>|<expire>|..."   // 第 4 段（下标 3）是过期时间
//! ```
//!
//! # `expire` 的单位是**毫秒**（实测，不是推断）
//!
//! 旧项目把它和 `Date.now()` 直接相减（`auth/utils.ts::isNeedRefresh` 里 `expireTime - Date.now() < days * 86400000`），
//! 实测真实 `asign.json` 里也是 **13 位**。**单位不是秒**。
//! （具体数值不写进代码：那是本机账号的数据。测试一律用合成的 13 位值。）
//!
//! 这一点值得单独写一段，因为它错得毫无声息：拿 13 位的毫秒值去和秒级时间戳比，
//! 结果永远是"没过期"，于是 auth 刷新永不触发、直到请求开始 401。
//! 所以本模块的所有访问器**把单位写进名字**（[`AuthInfo::expire_millis`]，
//! 而不是含义随上下文漂移的 `expire_unix`），让"混用单位"在编译期就显形。
//!
//! # 保留的原版怪癖（**不要"顺手修好"**）
//!
//! 1. `replace("Basic ", "")` 在 JS 里**只替换第一处**，所以 `"Basic Basic x"` 只会去掉一个前缀；
//! 2. 解码后按 `:` 切分且**只取前 3 段**，第 4 段起直接丢弃 → token 里含 `:` 会被截断；
//! 3. `Number(...) || 0`：非法/空/NaN/0 全部归零，且**允许小数**（如 `1.5`）；
//!    JS 的 `Number` 会先 trim 再解析 —— 这里由 [`is_plain_decimal`] 对齐，
//!    顺带挡掉 Rust 会认而 JS 不认的 `"inf"`；
//! 4. 解析结果的 `auth` 字段是 `"Basic " + 去掉前缀后的原文`，**不是重新编码**的结果。
//!
//! # 一处刻意的不一致
//!
//! Node 的 `Buffer.from(s, "base64")` 是**宽松**的：非法字符被忽略、且**不报错**。
//! 所以原版遇到 `"!!!not-base64!!!"` 不会失败，而是解码出乱码字节、再被 UTF-8 有损解码成
//! 一堆 `U+FFFD`，最后产出一个**平台标识是乱码但对象非 null** 的结果。
//! 这是实现缺陷而不是设计意图（真实输入的 auth 一定是合法 base64）。
//!
//! 因此本实现在以下三种情况下返回 `None`（即"不可用"）：**长度对不齐 4**、
//! **解不出合法 UTF-8**、**解出零字节**。这是**更严格，且不继承乱码**。
//! 向量里这类用例标记为 `malformed: true`，只断言"被判为不可用"。
//!
//! 为什么不逐字复刻 Node 的宽松解码：实测它并不遵守"滤掉非法字符再按 4 对齐"
//! （`"!!!not-base64!!!"` → 7 字节、`"basic " + 48 字符 base64` → 39 字节，都推不出来），
//! 复刻既无价值又脆弱。功能上真正要保证的只有两条：
//! **合法 auth 解析结果逐字一致**；**垃圾 auth 一律判为不可用**（从而回退到昵称/`"account"`，
//! 而不是把垃圾串映射成某个"看起来像账号"的 id）。

use crate::model::AccountId;
use crate::pure::mask_phone;
use base64::Engine as _;

/// 解析后的账号身份。
///
/// **注意 `Debug` 是手写的**：`token` 是可直接复用的凭据，`#[derive(Debug)]` 会让
/// 任何一次 `tracing::debug!(?info)` 把凭据写进日志。项目约定里 auth/token/JWT 一律不得输出。
#[derive(Clone, PartialEq)]
pub struct AuthInfo {
    /// 平台号（如 `"2"` / `"4"`）。原版缺字段时是 `undefined` → `None`。
    pub platform: Option<String>,
    /// 手机号。原版缺字段时是 `undefined` → `None`；**注意空串与缺失是两种情况**。
    pub phone: Option<String>,
    /// 业务 token（凭据）。同上，缺失与空串有别。
    pub token: Option<String>,
    /// 规范化后的 auth：`"Basic " + 原始 payload`（不去重新 base64 编码）。
    pub auth: String,
    /// 过期时间（**epoch 毫秒**，见模块头）。
    ///
    /// 保留 `f64` 是为了逐字对齐原版的 `Number(...) || 0` —— 它**允许小数**
    /// （向量里就有 `1.5` 这一例）。要整数时用 [`AuthInfo::expire_millis`]。
    pub expire: f64,
}

impl std::fmt::Debug for AuthInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthInfo")
            .field("platform", &self.platform)
            .field("phone", &self.phone)
            // 凭据不输出：只暴露"有没有"与长度，足以排查又不泄漏
            .field(
                "token",
                &self
                    .token
                    .as_ref()
                    .map(|t| format!("<redacted {} 字符>", t.chars().count())),
            )
            .field("expire", &self.expire)
            .finish_non_exhaustive()
    }
}

impl AuthInfo {
    /// 过期时间的整数形式（**epoch 毫秒**）。
    ///
    /// 名字里带单位是刻意的：原版这层叫 `expire_unix` 一类的名字，
    /// 读者很容易默认它是秒，然后把毫秒值拿去和秒级时间戳比 —— 那会永远判"没过期"。
    /// 小数向零截断，与 JS 的隐式转换一致（`1.5` → `1`，`-1` → `-1`）。
    pub fn expire_millis(&self) -> i64 {
        if self.expire.is_finite() {
            self.expire as i64
        } else {
            0
        }
    }

    /// 是否已过期（`now_millis` 为 **epoch 毫秒**）。
    ///
    /// `expire == 0` 视为"无过期信息"，判为已过期 —— 与旧版 `isNeedRefresh(0, …)` 同向。
    pub fn is_expired_at(&self, now_millis: i64) -> bool {
        self.expire_millis() <= now_millis
    }

    /// 是否到了该刷新的时候，对齐旧版 `isNeedRefresh(expireTime, days)`：
    /// `expireTime - Date.now() < days * 86400000`。
    ///
    /// **不是** `is_expired_at` 的别名：它提前 `days` 天触发，
    /// 所以"没过期"不代表"不用刷新"。两件事分开命名，避免将来有人拿错一个。
    pub fn is_need_refresh(&self, now_millis: i64, days: f64) -> bool {
        let remaining = self.expire - now_millis as f64;
        remaining < days * 86_400_000.0
    }

    /// 手机号（缺失或空串都返回 `None`），供身份派生使用。
    ///
    /// 之所以把空串也当缺失：原版写的是 `account.phone || parseAuth(...)?.phone || ""`，
    /// JS 里 `""` 是 falsy，会继续往下回退。
    pub fn phone_or_none(&self) -> Option<&str> {
        self.phone.as_deref().filter(|p| !p.is_empty())
    }
}

/// 复刻 `padBase64`：补 `=` 到 4 的倍数（已经是倍数则原样返回）。
///
/// 用**字符数**而非字节数对齐 JS 的 `String.length`：对纯 ASCII 的 base64 输入两者相同，
/// 只有在含星体平面字符时才与 JS 的 UTF-16 码元数不同（真实 auth 不会出现）。
pub fn pad_base64(s: &str) -> String {
    let len = s.chars().count();
    let rem = len % 4;
    if rem == 0 {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (4 - rem));
    out.push_str(s);
    for _ in 0..(4 - rem) {
        out.push('=');
    }
    out
}

/// 宽松解码：容忍 `=` 之后的垃圾与不属于字母表的字符，但**拒绝空洞结果**。
///
/// 规则：
/// - 只取第一个 `=` 之前的部分（`=` 即结束）；
/// - 丢弃所有不属于**标准** base64 字母表的字符（刻意**不**接受 base64url 的 `-`/`_`：
///   真实 auth 是标准编码，把它们当合法字符只会让垃圾输入更容易"解码成功"）；
/// - 剩余长度 `% 4 == 1` → `None`（末尾一个孤立字符凑不出完整字节，属于编码错误）；
/// - 解码失败，或解出**零字节** → `None`。
///
/// 这里的宽松是**照抄输入习惯**，不是为了复刻 Node 的字节级行为。实测 Node 的
/// `Buffer.from(s, "base64")` 并不遵守"滤非法字符后按 4 对齐"：`"!!!not-base64!!!"`
/// 解出 7 字节、`"basic " + 48 字符 base64` 解出 39 字节，都不是按 4 对齐能推出来的数。
/// 复刻它既无价值（真实 auth 一定规整）又极其脆弱，所以这里只保证：
/// **合法 auth 逐字一致，垃圾 auth 一律判为不可用**。
fn node_like_b64_decode(s: &str) -> Option<Vec<u8>> {
    let payload: String = s
        .chars()
        .take_while(|c| *c != '=')
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/')
        .collect();

    if payload.is_empty() || payload.len() % 4 == 1 {
        return None;
    }
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(payload)
        .ok()
        .filter(|b| !b.is_empty())
}

/// 解析 `auth` 字符串。对齐 `parseAuth` / `doParseAuth`。
///
/// - 入参为空串或全空白（falsy 语义）→ `None`；
/// - 解不出合法 UTF-8 → `None`（见模块头"一处刻意的不一致"）；
/// - 其余情况一律返回 `Some`，**即使 phone/token 缺失**（原版也只在这时返回非 null）。
pub fn parse_auth(auth: &str) -> Option<AuthInfo> {
    if auth.is_empty() {
        return None;
    }

    // JS 的 String.replace(字符串, "") 只替换第一处
    let clean = auth.replacen("Basic ", "", 1);
    let padded = pad_base64(&clean);
    let bytes = node_like_b64_decode(&padded)?;
    let raw = String::from_utf8(bytes).ok()?;

    // 只取前 3 段：第 4 段起被丢弃（token 含 ":" 会被截断）
    let mut parts = raw.split(':');
    let platform = parts.next().map(str::to_string);
    let phone = parts.next().map(str::to_string);
    let token = parts.next().map(str::to_string);

    // Number(token?.split("|")[3]) || 0
    let expire = token
        .as_deref()
        .and_then(|t| t.split('|').nth(3))
        .map(str::trim)
        .filter(|field| is_plain_decimal(field))
        .and_then(|field| field.parse::<f64>().ok())
        .filter(|v| *v != 0.0) // JS 的 `|| 0`：0 与 -0 都是 falsy
        .unwrap_or(0.0);

    Some(AuthInfo {
        platform,
        phone,
        token,
        auth: format!("Basic {clean}"),
        expire,
    })
}

/// 该字段是否是 JS `Number()` 会认的**普通十进制数**（可带正负号、小数、指数）。
///
/// 直接 `f64::parse` 会有两处与 JS 不同，都必须挡住：
/// 1. Rust **不接受**首尾空白（`" 12 "` → `Err`），JS 的 `Number` 会 trim 后取 12；
/// 2. Rust **接受** `"inf"` / `"infinity"` / `"nan"`（大小写不敏感），而
///    `Number("inf")` 是 `NaN`。
///
/// 非有限值在这里统一归零，含义是"无过期信息 → 判为已过期 → 触发刷新"。
/// 这是**故意偏安全的一侧**：宁可多刷一次，也不要把一个来路不明的 token 当成永不过期。
pub(crate) fn is_plain_decimal(s: &str) -> bool {
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
    // `.` 两侧至少要有数字，否则 `.`、`+.` 之类会被当成 0
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

/// 解析账号手机号。对齐 `resolvePhone`：配置里的 `phone` 优先，其次从 `auth` 现场解析。
///
/// 空串按缺失处理（JS falsy 语义）。
pub fn resolve_phone(config_phone: Option<&str>, auth: &str) -> String {
    if let Some(p) = config_phone.filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    parse_auth(auth)
        .and_then(|info| info.phone_or_none().map(str::to_string))
        .unwrap_or_default()
}

/// 账号展示名。对齐 `accountLabel`：昵称优先，否则手机号掩码，都没有则 `"账号"`。
pub fn account_label(nickname: Option<&str>, phone: &str) -> String {
    if let Some(n) = nickname.filter(|n| !n.is_empty()) {
        return n.to_string();
    }
    if phone.is_empty() {
        "账号".to_string()
    } else {
        mask_phone(phone)
    }
}

/// 旧项目的存储键。对齐 `accountKey`：手机号 → 昵称 → `"account"`。
///
/// **这就是旧 `data/state.json` 的键**，迁移时靠它把历史数据对到账号上。
pub fn legacy_account_key(phone: &str, nickname: Option<&str>) -> String {
    if !phone.is_empty() {
        return phone.to_string();
    }
    match nickname.filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => "account".to_string(),
    }
}

/// 新项目的账号标识 `AccountId`。
///
/// 与 [`legacy_account_key`] 的唯一差别：**手机号走掩码**（`19****01`），
/// 免得把完整号码当成数据库主键四处引用；昵称/兜底值则原样沿用。
///
/// 代价是掩码可能撞车（两个账号前 2 后 2 相同）。因此迁移时**必须做冲突检测并报错**，
/// 绝不能静默合并 —— 见 `mcloud-io` 的导入实现。
pub fn account_id(phone: &str, nickname: Option<&str>) -> AccountId {
    if phone.is_empty() {
        AccountId::new(legacy_account_key(phone, nickname))
    } else {
        AccountId::new(mask_phone(phone))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试：以原版编译产物产出的黄金向量为准
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    const VECTORS: &str = include_str!("../../../test/golden/auth-vectors.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ParseCase {
        name: String,
        input: String,
        /// 原版的返回值：对象或 null。缺 phone/token 时字段直接不存在。
        expect: Value,
        #[serde(default)]
        malformed: bool,
    }

    #[derive(Deserialize)]
    struct PadCase {
        name: String,
        input: String,
        expect: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct IdentityCase {
        name: String,
        account: Value,
        /// `parseAuth(account.auth)?.phone ?? null` —— 只反映"从 auth 里解出来的"号码，
        /// 与最终生效的号码（配置里的 `phone` 优先）是两回事。
        parsed_phone: Option<String>,
        legacy_key: String,
        account_id: String,
        label: String,
    }

    #[derive(Deserialize)]
    struct Vectors {
        #[serde(rename = "parseAuth")]
        parse_auth: Vec<ParseCase>,
        #[serde(rename = "padBase64")]
        pad_base64: Vec<PadCase>,
        identity: Vec<IdentityCase>,
    }

    fn vectors() -> Vectors {
        serde_json::from_str(VECTORS).expect("auth 黄金向量应能解析")
    }

    #[test]
    fn parse_auth_matches_ts_vectors() {
        let v = vectors();
        assert!(!v.parse_auth.is_empty());

        let mut checked = 0;
        for c in &v.parse_auth {
            let got = parse_auth(&c.input);

            if c.expect.is_null() {
                assert!(
                    got.is_none(),
                    "用例 {} 原版返回 null，我们应返回 None",
                    c.name
                );
                continue;
            }

            if c.malformed {
                // 原版在编码病态输入下产出乱码 platform —— 我们刻意判为不可用
                assert!(
                    got.is_none(),
                    "用例 {} 是编码病态输入，应判为不可用（原版产出乱码 platform）",
                    c.name
                );
                continue;
            }

            let got = got.unwrap_or_else(|| panic!("用例 {} 应能解析", c.name));
            let expect = &c.expect;

            // 关键：字段"存在但为空串"与"字段缺失"必须区分
            assert_eq!(
                got.platform.as_deref(),
                expect.get("platform").and_then(Value::as_str),
                "platform 不一致（用例 {}）",
                c.name
            );
            assert_eq!(
                got.phone.as_deref(),
                expect.get("phone").and_then(Value::as_str),
                "phone 不一致（用例 {}）",
                c.name
            );
            assert_eq!(
                got.token.as_deref(),
                expect.get("token").and_then(Value::as_str),
                "token 不一致（用例 {}）",
                c.name
            );
            assert_eq!(
                got.auth,
                expect.get("auth").and_then(Value::as_str).unwrap_or(""),
                "规范化 auth 不一致（用例 {}）",
                c.name
            );
            let expect_expire = expect.get("expire").and_then(Value::as_f64).unwrap_or(0.0);
            assert!(
                (got.expire - expect_expire).abs() < f64::EPSILON,
                "expire 不一致（用例 {}）: {} vs {}",
                c.name,
                got.expire,
                expect_expire
            );
            checked += 1;
        }
        assert!(checked >= 15, "至少应校验 15 个有效用例，实际 {checked}");
    }

    #[test]
    fn pad_base64_matches_ts_vectors() {
        let v = vectors();
        for c in &v.pad_base64 {
            assert_eq!(pad_base64(&c.input), c.expect, "padBase64 用例 {}", c.name);
        }
    }

    /// 黄金向量里不得出现真实手机号。
    ///
    /// `test/golden/*.json` 会进版本库，而它是脚本从**本机真实数据**产出的 ——
    /// 早先 `gen-auth-vectors.cjs` 就把本机账号的真实号码当字面量写进了向量。
    /// 现在两套生成器统一用 `199` 号段，这条断言负责在它再次发生时立刻报错。
    #[test]
    fn golden_vectors_contain_no_real_phone_numbers() {
        crate::test_support::assert_only_synthetic_identifiers("auth-vectors.json", VECTORS);
    }

    /// 空输入必须判为不可用（原版 `if (!auth) return null`）。
    #[test]
    fn empty_auth_is_none() {
        assert!(parse_auth("").is_none());
    }

    /// "Basic " 前缀只替换**第一处**。
    ///
    /// 证明方式：写两遍前缀 → 只去掉一个 → 剩下的 `Basic ` 会被当成 base64 正文，
    /// 掺进 5 个额外字母后总长度变成 53，对不齐 4 → 判为不可用。
    /// 若误写成全局替换，这条会解析成功 —— 所以它确实钉住了语义。
    #[test]
    fn basic_prefix_only_first_occurrence_is_stripped() {
        let payload = base64::engine::general_purpose::STANDARD.encode("2:19900000001:t");
        assert!(parse_auth(&payload).is_some(), "无前缀应能解析");
        assert!(
            parse_auth(&format!("Basic {payload}")).is_some(),
            "一个 Basic 前缀应能解析"
        );
        assert!(
            parse_auth(&format!("Basic Basic {payload}")).is_none(),
            "两个 Basic 前缀：只去第一处，其余不再对得齐 4（证明不是全局替换）"
        );
    }

    /// token 里含 `:` 会被截断 —— 保留这个怪癖，别"顺手修好"。
    #[test]
    fn colon_in_token_is_truncated_and_expire_becomes_zero() {
        let auth = base64::engine::general_purpose::STANDARD.encode("2:19900000001:a:b:c");
        let info = parse_auth(&auth).expect("应能解析");
        assert_eq!(info.token.as_deref(), Some("a"), "token 应被截断为第一段");
        assert_eq!(info.expire, 0.0, "截断后取不到第 4 段，expire 应为 0");
    }

    /// `Number(...) || 0` 的取值规则：非法/空/NaN/0 全部归零，且**允许小数**。
    ///
    /// 注意构造输入时必须是 `平台:手机号:token` **三段**。只写两段会让冒号切分后
    /// token 为 `undefined`，expire 永远算成 0 —— 那样这几条断言会全部"通过"，
    /// 却什么也没测到（本测试的第一版就踩过这个坑）。
    #[test]
    fn expire_allows_fractional_and_rejects_junk() {
        let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let expire = |token: &str| {
            parse_auth(&enc(&format!("2:19900000001:{token}")))
                .unwrap_or_else(|| panic!("应能解析: {token}"))
                .expire
        };
        assert_eq!(expire("ak|bk|ck|1.5|fk"), 1.5, "小数应保留");
        assert_eq!(expire("ak|bk|ck|-1|fk"), -1.0, "负数应保留");
        assert_eq!(expire("ak|bk|ck|1893456000000|fk"), 1893456000000.0);
        assert_eq!(expire("ak|bk|ck|notanumber|fk"), 0.0, "非数字归零");
        assert_eq!(expire("ak|bk|ck||fk"), 0.0, "空串归零");
        assert_eq!(expire("ak|bk|ck"), 0.0, "段数不足归零");
        // `|| 0` 让 0 与 NaN 都归零
        assert_eq!(expire("ak|bk|ck|0|fk"), 0.0);
        assert_eq!(expire("ak|bk|ck|NaN|fk"), 0.0, "NaN 归零");
    }

    /// `Number()` 的形状判定：Rust 的 `f64::parse` 有三处与 JS 不同，必须逐条挡住。
    ///
    /// 非有限值与 JS 独有写法（十六/八/二进制字面量）在本实现里统一归零，
    /// 方向都是"当成无过期信息 → 判过期 → 去刷新"，属**偏安全的一侧**。
    #[test]
    fn expire_field_follows_js_number_shape() {
        let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let expire = |field: &str| {
            parse_auth(&enc(&format!("2:19900000001:ak|bk|ck|{field}|fk")))
                .expect("应能解析")
                .expire
        };

        // JS 会先 trim 再解析；Rust 不会 → 必须自己 trim
        assert_eq!(
            expire(" 1893456000000 "),
            1893456000000.0,
            "首尾空白应被忽略"
        );
        // 指数写法两边都认
        assert_eq!(expire("1e3"), 1000.0);
        assert_eq!(expire("-2.5e-1"), -0.25);
        // Rust 认而 JS 不认（Number("inf") 是 NaN）→ 挡住
        assert_eq!(expire("inf"), 0.0);
        assert_eq!(expire("-inf"), 0.0);
        assert_eq!(expire("nan"), 0.0);
        // 形状不完整 → 挡住
        assert_eq!(expire("."), 0.0);
        assert_eq!(expire("1.2.3"), 0.0);
        assert_eq!(expire("1e"), 0.0);
        assert_eq!(expire("1e+"), 0.0);
        assert_eq!(expire("1 2"), 0.0);
        // 刻意不支持：JS 的 Number("0x10") 是 16。这种写法不会出现在真实 token 里，
        // 而"认了它"只会让垃圾 token 多出一个被当成有效期的机会，所以判 0。
        assert_eq!(expire("0x10"), 0.0);
    }

    /// 空字节载荷没有身份可言，判为不可用（而不是产出一个各字段皆空的"账号"）。
    #[test]
    fn empty_payload_is_rejected() {
        for input in ["====", "="] {
            assert!(parse_auth(input).is_none(), "空载荷应判为不可用: {input:?}");
        }
    }

    #[test]
    fn expire_millis_truncates_toward_zero() {
        let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let millis = |token: &str| {
            parse_auth(&enc(&format!("2:19900000001:{token}")))
                .expect("应能解析")
                .expire_millis()
        };
        assert_eq!(millis("ak|bk|ck|1.5|fk"), 1, "向零截断");
        assert_eq!(millis("ak|bk|ck|-1|fk"), -1);
        assert_eq!(millis("ak|bk|ck|1893456000000|fk"), 1893456000000);
        // 真实数据是 13 位毫秒；i64 装得下，不该溢出或被截断成别的量级
        assert_eq!(millis("ak|bk|ck|1893456000000|fk"), 1893456000000);
    }

    /// `expire` 的单位是**毫秒**，且"过期"与"该刷新"是两件事。
    ///
    /// 这条测试的存在理由：单位错误在这里是**静默**的 ——
    /// 把 13 位毫秒值当成秒去比，永远得到"没过期"，auth 刷新永不触发，
    /// 直到上游开始回 401 才会被发现。所以用真实量级的数值把语义钉死。
    #[test]
    fn expire_is_milliseconds_and_refresh_is_ahead_of_expiry() {
        let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let info = parse_auth(&enc("2:19900000001:ak|bk|ck|1893456000000|fk")).expect("应能解析");

        // expire 是 2030-01-01（13 位毫秒）；now 取 2026 年中，距到期约 3.5 年
        let now = 1_789_000_000_000_i64;
        assert!(!info.is_expired_at(now), "还没到期");
        // 用秒级时间戳去比会得到错误的答案 —— 这正是要防的用法
        assert!(
            !info.is_expired_at(now / 1000),
            "拿秒级 now 去比是**错误用法**，它会说'没过期'，所以单位必须靠名字挡住"
        );

        // 过期：now 刚好越过 expire
        assert!(info.is_expired_at(1_893_456_000_001));

        // 刷新阈值比过期更早：阈值 5 年时，现在就该刷新（虽然还远未到期）
        assert!(
            info.is_need_refresh(now, 5.0 * 365.0),
            "离到期还有约 3.5 年，阈值 5 年 → 该刷新（虽然还没过期）"
        );
        assert!(!info.is_need_refresh(now, 30.0), "阈值 30 天 → 还不用刷");

        // `expire = 0` = 无过期信息 → 两个判断都视为"该处理了"
        let unknown = parse_auth(&enc("2:19900000001:ak|bk|ck|0|fk")).expect("应能解析");
        assert!(unknown.is_expired_at(now));
        assert!(unknown.is_need_refresh(now, 5.0));
    }

    /// 当前真实配置的 `auth` 一定是 13 位毫秒 —— 单位假设变了，这条会先红。
    ///
    /// 实测旧 `asign.json`（`packages/api/data/` 同期数据）的 expire 就是 13 位
    /// （2026 年 10 月前后），但**具体数值不写进代码**：那是本机账号的数据。
    /// 这里用同量级的合成值 `1893456000000`（2030-01-01）把"数量级"钉住即可。
    #[test]
    fn real_world_expire_magnitude_is_milliseconds() {
        let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let info = parse_auth(&enc("2:19900000001:ak|bk|ck|1893456000000|fk")).expect("应能解析");
        let digits = info.expire_millis().to_string().len();
        assert_eq!(
            digits, 13,
            "真实 auth 的 expire 是 13 位毫秒；若这里变成 10，说明上游换了单位，必须重查"
        );
    }

    /// 三种"编码病态"各有不同的拒绝理由，逐条钉住 —— 否则很容易只测到其中一种。
    ///
    /// 原版对这三种都会返回一个 platform 是乱码/空串的对象（**绝不返回 null**），
    /// 我们刻意更严格；差异与理由见模块头"一处刻意的不一致"。
    #[test]
    fn malformed_base64_is_rejected_instead_of_producing_mojibake() {
        // ① 滤掉非法字符后长度对不齐 4
        assert!(
            parse_auth("!!!not-base64!!!").is_none(),
            "① 剩 9 个有效字符，对不齐 4"
        );
        assert!(parse_auth("plain text").is_none(), "① 剩 9 个有效字符");
        // ② 长度对得齐，但解出的字节不是合法 UTF-8（0xFF 在 UTF-8 里永不合法）
        let invalid_utf8 =
            base64::engine::general_purpose::STANDARD.encode([0xffu8, 0xfe, 0xfd, 0xfc]);
        assert!(
            parse_auth(&invalid_utf8).is_none(),
            "② 解出非法 UTF-8 应判为不可用"
        );
        // ③ 解出零字节 —— 没有身份可言，见 empty_payload_is_rejected
    }

    #[test]
    fn identity_derivation_matches_ts_vectors() {
        let v = vectors();
        assert!(!v.identity.is_empty());
        for c in &v.identity {
            let auth = c.account.get("auth").and_then(Value::as_str).unwrap_or("");
            let nickname = c.account.get("nickname").and_then(Value::as_str);
            let config_phone = c.account.get("phone").and_then(Value::as_str);

            let phone = resolve_phone(config_phone, auth);

            // 只反映 auth 里的号码，用来钉住"配置 phone 优先"这个回退链的前半段
            assert_eq!(
                parse_auth(auth).and_then(|info| info.phone_or_none().map(str::to_string)),
                c.parsed_phone,
                "parsedPhone 不一致（用例 {}）",
                c.name
            );
            assert_eq!(
                legacy_account_key(&phone, nickname),
                c.legacy_key,
                "legacyKey 不一致（用例 {}）",
                c.name
            );
            assert_eq!(
                account_id(&phone, nickname).as_str(),
                c.account_id,
                "accountId 不一致（用例 {}）",
                c.name
            );
            assert_eq!(
                account_label(nickname, &phone),
                c.label,
                "label 不一致（用例 {}）",
                c.name
            );
        }
    }

    /// 迁移用：旧键（手机号原文）→ 新 AccountId（掩码）。这条是历史数据能对上号的根据。
    #[test]
    fn legacy_key_maps_to_account_id_by_masking() {
        assert_eq!(account_id("19900000001", None).as_str(), "19****01");
        assert_eq!(account_id("19900000002", None).as_str(), "19****02");
        // 非手机号键（昵称）原样沿用
        assert_eq!(account_id("", Some("账号B")).as_str(), "账号B");
        assert_eq!(account_id("", None).as_str(), "account");
    }

    /// 凭据不得出现在 Debug 输出里，但手机号应保留（它用于排查，且不是可直接复用的凭据）。
    #[test]
    fn debug_output_redacts_token() {
        let raw = "2:19900000001:SECRETTOKEN|1|1893456000000|2";
        let info =
            parse_auth(&base64::engine::general_purpose::STANDARD.encode(raw)).expect("应能解析");
        assert_eq!(info.token.as_deref(), Some("SECRETTOKEN|1|1893456000000|2"));

        let shown = format!("{info:?}");
        assert!(
            !shown.contains("SECRETTOKEN"),
            "Debug 不得泄漏 token: {shown}"
        );
        assert!(shown.contains("redacted"), "应标明被脱敏: {shown}");
        assert!(
            shown.contains("19900000001"),
            "手机号应保留在 Debug 里以便排查: {shown}"
        );
    }
}
