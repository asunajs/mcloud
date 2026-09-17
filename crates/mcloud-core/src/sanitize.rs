//! HAR → fixture 的脱敏规则。**纯函数**，因此脱敏逻辑本身可以单测。
//!
//! # 为什么必须脱敏
//!
//! `har/*.har` 是真实流量，含可直接复用的登录凭据。fixture 要入库，
//! 所以脱敏必须在**生成侧**完成，而不是靠"记得别提交"。
//!
//! # 与日志脱敏的区别（刻意不同）
//!
//! 日志里我们**保留**配置绝对路径、完整收件地址、临时文件 ID —— 它们有实际排障价值；
//! 只剔除可直接复用的凭据。而 fixture 是要进版本库的静态文件，
//! 因此这里对手机号等标识信息也一并遮蔽。
//! 两者目标不同，不要互相套用。

use serde_json::{Value, json};

/// 替换占位符。
pub const PLACEHOLDER: &str = "<REDACTED>";

/// 允许**明文**进版本库的 header 名单（已归一化：小写、去 `-`/`_`）。
///
/// # 为什么 header 这边必须是白名单，而不是黑名单
///
/// 黑名单在真实流量上被证伪了。把 36 份真实 HAR 跑一遍，
/// `cookieToken`、`cookieTokenKey`、`x-yun-tid`、`hecaiyundata2021jssdkcross`
/// 这几个**名字不在名单里**的 cookie / 追踪头带着可直接复用的凭据原样通过，
/// 自检报出三万余处残留 —— 结果是一份 fixture 都写不出来。
///
/// 逐个往黑名单里加名字是打地鼠：header 名由上游与抓包工具决定，我们穷举不完，
/// 而漏掉一个就意味着凭据进了版本库、推上去收不回来。
/// 反过来，"夹具需要哪些 header"是**有限且已知**的：匹配键只看 `method + path`，
/// 回放只读响应体，请求 header 仅作留档。所以未登记的一律打码 ——
/// 代价是偶尔多打一个无害的值，收益是**新的 cookie / 追踪头一出现就被默认挡住**。
const ALLOWED_HEADERS: &[&str] = &[
    // 请求内容协商
    "accept",
    "acceptencoding",
    "acceptlanguage",
    "contentencoding",
    "contentlength",
    "contenttype",
    "origin",
    "useragent",
    "xrequestedwith",
    // 注意：HTTP/2 伪头（`:path` / `:authority` / …）**刻意不在此列**。
    // 它们的值是完整路径与主机，与 `request.url` / `method` 完全重复，
    // 而 `:path` 那种长路径又会被形状规则盯上 —— 留着只会白占注意力，不如一律打码。
    // 传输与缓存
    "cachecontrol",
    "connection",
    "date",
    "expires",
    "host",
    "lastmodified",
    "pragma",
    "server",
    "transferencoding",
    "vary",
    // CORS：调试台看响应时会用到
    "accesscontrolallowcredentials",
    "accesscontrolallowheaders",
    "accesscontrolallowmethods",
    "accesscontrolalloworigin",
    "accesscontrolexposeheaders",
    "accesscontrolmaxage",
    // 抓包时机（无凭据）
    "secfetchdest",
    "secfetchmode",
    "secfetchsite",
    "secfetchuser",
];

/// 敏感 body / query 字段（已归一化）。
const SENSITIVE_FIELD_KEYS: &[&str] = &[
    "auth",
    "token",
    "accesstoken",
    "refreshtoken",
    "idtoken",
    "jwt",
    "cookie",
    "userdomainid",
    "deviceid",
    "phone",
    "mobile",
    "phonenumber",
    "password",
    "pass",
    "secret",
    "corpsecret",
    "apikey",
    "apisecret",
    "sign",
    "signature",
    "sessionid",
    "smscode",
    "verifycode",
    "authcode",
    "wxcode",
];

/// URL 中每次调用都会变的参数 —— 不能进匹配键，否则 fixture 永远命中不了。
const VOLATILE_QUERY_KEYS: &[&str] = &[
    "_",
    "_t",
    "t",
    "ts",
    "timestamp",
    "nonce",
    "rand",
    "random",
    "callback",
    "_r",
];

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// key 是否属于敏感字段（大小写/连字符/下划线无关）。
pub fn is_sensitive_field(key: &str) -> bool {
    let k = normalize_key(key);
    SENSITIVE_FIELD_KEYS.contains(&k.as_str())
}

/// 该 header 是否允许明文进版本库。**未登记的一律不允许**（见 [`ALLOWED_HEADERS`]）。
pub fn is_allowed_header(key: &str) -> bool {
    ALLOWED_HEADERS.contains(&normalize_key(key).as_str())
}

/// 脱敏 header 列表。
///
/// - 不在白名单里 → **整值打码**（名字保留，留痕便于排查）；
/// - 在白名单里 → 保留，但仍要过一遍长串遮蔽。
///
/// 第二条不是多余的：白名单里的 `user-agent` 实测就带着一截 40 字符的构建/会话 token
/// （`Mozilla/5.0 … <40 字符>`），它是"这个头有用"与"这个值里有一段该藏起来"
/// 同时成立 —— 整值保留会漏，整值打掉又会把有用的部分一起丢掉。
pub fn sanitize_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            if is_allowed_header(k) {
                (k.clone(), scrub_text(v))
            } else {
                (k.clone(), PLACEHOLDER.to_string())
            }
        })
        .collect()
}

/// 递归脱敏 JSON。返回是否发生了修改。
///
/// 两条规则**叠加**，缺一不可：
///
/// 1. 键名命中黑名单 → 整值打码；
/// 2. 其余**字符串值**里的长串再按形状打码。
///
/// 第 2 条不是锦上添花。只按键名判断在真实流量上会大量放行 ——
/// `{"xxx":"<base64 的 mobile:手机号:token>"}`、`{"params":"<JWT>"}`
/// 这类"键名无害、值是凭据"的形态很常见，实测 36 份 HAR 里到处都是。
/// 键名由上游决定，我们无法预先知道哪个字段会装凭据，所以值这一侧必须照形状兜住。
/// 这与 `scan_for_leaks` 的兜底是**同一条思路的两次实现**，阈值刻意不同（见 [`mask_string_value`]）。
pub fn sanitize_json(value: &mut Value) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if is_sensitive_field(key) {
                    if *child != json!(PLACEHOLDER) {
                        *child = json!(PLACEHOLDER);
                        changed = true;
                    }
                } else if sanitize_json(child) {
                    changed = true;
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                if sanitize_json(item) {
                    changed = true;
                }
            }
        }
        Value::String(text) => {
            let masked = mask_string_value(text);
            if masked != *text {
                *text = masked;
                changed = true;
            }
        }
        // 数字也要过一遍：JSON 里 19 位标识常常写成**裸数字**而不是字符串，
        // 只处理字符串会让它带着真实值进版本库（守卫按形状扫原始文本，扫得到）。
        Value::Number(number) => {
            let text = number.to_string();
            if text.bytes().all(|b| b.is_ascii_digit()) {
                let synthesized = synthesize_identifiers(&text);
                if synthesized != text
                    && let Ok(replacement) = synthesized.parse::<u64>()
                {
                    *number = serde_json::Number::from(replacement);
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}

/// 对单个字符串值做脱敏。
///
/// - 整串是 URL → 走 [`redact_url`]：**保留路径**，只处理 query。
///   路径是夹具可读性与匹配的来源，不该被当成 token 打掉；
///   而 query 里的凭据由 [`redact_url`] 负责。
/// - 整串是**内嵌 JSON** → 递归脱敏后再序列化回来。
/// - 其余 → 把长串打码，阈值取 32。
///
/// # 为什么"内嵌 JSON"必须单独处理
///
/// 真实 HAR 里这**不是特例而是常态**：抓包工具会把整份请求体当成 JSON 字符串再包一层，
/// 于是真正有意义的字段在第二层。只看外层会把它当成一段普通文本 ——
/// 键名黑名单在第二层完全失效，而自检会递归进去（见 [`scan_value`]），
/// 两边深度不一致，结果就是"自检报出一堆本该被脱敏的残留"。
/// 实测 `userdomain_id` 就是这么漏的。
///
/// 阈值比 [`scan_for_leaks`] 的 40 **更低**是刻意的：脱敏要覆盖得比自检更宽，
/// 自检才可能对"脱敏漏网的串"报警。若两边同阈值同字符集，自检就会与脱敏一起盲掉。
fn mask_string_value(text: &str) -> String {
    if looks_like_url(text) {
        return redact_url(text);
    }
    if let Ok(mut inner @ (Value::Object(_) | Value::Array(_))) =
        serde_json::from_str::<Value>(text)
        && sanitize_json(&mut inner)
    {
        return serde_json::to_string(&inner).unwrap_or_else(|_| text.to_string());
    }
    scrub_text(text)
}

/// 脱敏请求体：JSON 走结构化脱敏，表单体逐对脱敏，其余退化为"长 token 串"遮蔽。
pub fn sanitize_body(body: &[u8]) -> Vec<u8> {
    if body.is_empty() {
        return Vec::new();
    }
    match std::str::from_utf8(body) {
        Ok(text) => match serde_json::from_str::<Value>(text) {
            Ok(mut value) => {
                sanitize_json(&mut value);
                serde_json::to_vec(&value).unwrap_or_else(|_| text.as_bytes().to_vec())
            }
            Err(_) if is_form_encoded(text) => redact_form_pairs(text).join("&").into_bytes(),
            Err(_) => scrub_text(text).into_bytes(),
        },
        // 二进制体（如图片/压缩流）不解析，整体打码以免误带凭据。
        Err(_) => format!("<BINARY {} bytes>", body.len()).into_bytes(),
    }
}

/// 把形如长 token 的连续串遮蔽。
///
/// 判定标准：`[A-Za-z0-9+/=_.-]` 且长度 ≥ 32，且**不是 URL 形状**（URL 刻意保留）。
///
/// 这是脱敏侧唯一的"形状兜底"，[`redact_url`] 的路径段、header 值、非 JSON 文本体
/// 全都走它 —— 好处是这几处的判定完全一致，不会出现
/// "URL 里放行、header 里打码"这种两侧对不上的缝。
///
/// 阈值 32 比 [`scan_for_leaks`] 的 40 **更低**是刻意的：脱敏要覆盖得比自检更宽，
/// 自检才可能对"脱敏漏网的串"报警。若两边同阈值，自检就会与脱敏一起盲掉。
pub fn redact_token_like_runs(text: &str) -> String {
    const MIN_LEN: usize = 32;
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();

    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= MIN_LEN && !looks_like_url(run) {
            out.push_str(PLACEHOLDER);
        } else {
            out.push_str(run);
        }
        run.clear();
    };

    for c in text.chars() {
        if is_token_char(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);

    out.to_string()
}

/// "token 长相"的字符集。
///
/// 脱敏（[`redact_token_like_runs`]）与自检（[`token_like_runs`]）**必须共用它**。
/// 少一个 `-` 就会让带连字符的 CDN URL 被切成碎片（`/a-<80 字符>-b.png` 断成三段），
/// 碎片的形状已经不像 URL，于是逐个被当成疑似凭据报警 —— 而脱敏侧把整串当 URL
/// 保留、什么都没做。两边定义不一致，门禁就会永远红且无从修复。
/// 两份规则现在只差阈值（32 对 40），这才是刻意留的差异。
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '.' | '-')
}

/// 合成号段前缀（11 位手机号与 19 位 userDomainId 共用）。
const SYNTHETIC_PREFIX: &str = "199";

/// 文本级脱敏的**唯一入口**：先按形状打码，再把标识形状的数字改成合成号段。
///
/// 两件事必须捆在一起走同一个入口 —— 这一路上踩到的坑几乎都是"某条分支只做了其中一件"：
/// URL 分支只跑 `redact_url`、非 JSON 文本只跑形状打码、JSON 字符串没递归……
/// 分散调用迟早会再裂开一条缝。
pub fn scrub_text(text: &str) -> String {
    synthesize_identifiers(&redact_token_like_runs(text))
}

/// 把"标识形状"的数字重映射到合成号段，其余原样保留。
///
/// # 为什么必须做，而且判据要和仓库守卫逐条对齐
///
/// `scripts/check-identifiers.cjs` 是一条**形状**判定：11 位或 19 位、以 `1` 开头、
/// 两侧不是字母数字。它分不清"这是 userDomainId 还是 taskId"，也不该去分 ——
/// 它要的是一条可机检的不变量：**进版本库的这个形状的值必须 199 开头**。
///
/// 而真实流量里撞上这个形状的远不止号码（实测 `taskId` 就是 19 位 `1…`）。
/// 于是"反正不是标识"这条退路并不存在，生成侧必须把这类值改掉。
/// 这里是**重映射**而不是打码：`taskId` 这类字段要参与响应解析，掩码会让它失去形状。
///
/// # 映射是确定性的
///
/// 用原值派生新数字（sha256），同输入永远同输出。
/// 夹具要进版本库，随机重映射会让每次重新生成的 diff 全是噪声，
/// 也让人没法用"两次生成的差异"判断上游是否真的变了。
pub fn synthesize_identifiers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;

    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            out.push(chars[index]);
            index += 1;
            continue;
        }

        let start = index;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }

        let run: String = chars[start..index].iter().collect();
        let before = start.checked_sub(1).map(|i| chars[i]);
        let after = chars.get(index).copied();
        out.push_str(&remap_identifier(&run, before, after));
    }

    out
}

/// 单个数字串的重映射。判据与守卫的 `SHAPES` 一致，包括"两侧不能是字母数字"。
fn remap_identifier(run: &str, before: Option<char>, after: Option<char>) -> String {
    let is_identifier_shape = matches!(run.len(), 11 | 19)
        && run.starts_with('1')
        && !before.is_some_and(|c| c.is_ascii_alphanumeric())
        && !after.is_some_and(|c| c.is_ascii_alphanumeric());

    if !is_identifier_shape {
        return run.to_string();
    }

    let tail_len = run.len() - SYNTHETIC_PREFIX.len();
    format!("{SYNTHETIC_PREFIX}{}", synthetic_digits(run, tail_len))
}

/// 由原值确定性地派生 `count` 个十进制数字。
fn synthetic_digits(original: &str, count: usize) -> String {
    crate::crypto::sha256_hex(original.as_bytes())
        .bytes()
        .filter_map(|b| (b as char).to_digit(16))
        .map(|nibble| char::from(b'0' + (nibble % 10) as u8))
        .take(count)
        .collect()
}

/// URL 的匹配键：`METHOD host/path`，丢弃全部 query。
///
/// 这是 fixture 查找的键 —— 只看"打到哪个接口"，
/// 不看易变的 query，否则每次抓包都生成一批命不中的 fixture。
pub fn match_key(method: &str, url: &str) -> String {
    let no_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let no_query = no_scheme.split(['?', '#']).next().unwrap_or(no_scheme);
    format!("{} {}", method.to_ascii_uppercase(), no_query)
}

/// URL 的可存档形式：去掉易变参数、遮蔽敏感参数值、遮蔽没名分的长串。
///
/// 与 [`match_key`] 的分工：`match_key` 用于查表，本函数用于**存档展示**。
///
/// # 为什么还要管"没名分的长串"
///
/// 只按参数名判断是不够的（同 [`sanitize_json`] 的道理）：实测有 URL 把整段
/// base64 塞在 query 里（`?e=<1280 字符>`），参数名 `e` 无害，值才是要藏的东西。
/// 这类参数没有任何业务价值 —— 匹配键本来就丢弃整个 query —— 所以直接打码。
pub fn redact_url(url: &str) -> String {
    let (raw_base, query) = match url.split_once('?') {
        Some((b, q)) => (b, q),
        None => (url, ""),
    };
    // 路径段也要过一遍长串遮蔽：一条 URL 里出现 `:` 或 `?` 之后，剩下的片段
    // 已经不带主机名，形状上就和凭据无法区分。整条是 URL 形状的部分照旧原样保留
    // （`https://host/a/b/c` 是一个 URL 形状的长串，判定会放过它），可读性不受影响。
    let base = scrub_text(raw_base);
    if query.is_empty() {
        return base;
    }

    let kept = redact_form_pairs(query);

    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// `k=v&k=v` 形状的逐对脱敏。**query 与表单体共用**。
///
/// # 为什么必须逐对处理，而不是整串按形状打码
///
/// 形状打码的字符集含 `=`，于是 `optkeyword=<长值>` 会被整段替换掉 ——
/// **参数名一起没了**。实测真实流量里这个接口就是把参数放在表单体里的，
/// 结果夹具里只剩 `<REDACTED>`，既不看不出调的是哪个字段，也没法用来验证请求构造。
/// 逐对处理只打码值，键名留下；同时 `VOLATILE_QUERY_KEYS` 与敏感键名判断照旧生效。
fn redact_form_pairs(pairs: &str) -> Vec<String> {
    pairs
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            // 键名也要过一遍脱敏：判据之外还留着一条口子 ——
            // 整串就是一段 base64 载荷、里面恰好有 `=` 时，`k` 就是那一段载荷。
            let key = scrub_text(k);
            if VOLATILE_QUERY_KEYS.contains(&normalize_key(k).as_str()) {
                return None;
            }
            if is_sensitive_field(k) || redact_token_like_runs(v) != v {
                Some(format!("{key}={PLACEHOLDER}"))
            } else {
                Some(format!("{key}={}", scrub_text(v)))
            }
        })
        .collect()
}

/// 表单键名的最大长度。超了就不是字段名了。
const MAX_FORM_KEY_LEN: usize = 64;

/// 这段文本是否是 `k=v&k=v` 形状的表单体。
///
/// 判据刻意保守，两条都要：
/// 1. 每个非空段都必须含 `=`；
/// 2. **键名要像字段名**（非空且不超过 [`MAX_FORM_KEY_LEN`]）。
///
/// 第 2 条是补上来的：只判"含 `=`"会把**一整段 base64 载荷**（末尾带 `=` 填充）
/// 误判成表单，于是键名位置留了一整段载荷 —— 实测正是这样漏出 60 处残留。
/// 宁可漏判（退回形状打码，照样安全），也不要把一段载荷当成表单拆开。
fn is_form_encoded(text: &str) -> bool {
    !text.is_empty()
        && !text.contains(['{', '}', '\n'])
        && text.split('&').filter(|p| !p.is_empty()).all(|p| {
            matches!(p.split_once('='), Some((k, _)) if !k.is_empty() && k.len() <= MAX_FORM_KEY_LEN)
        })
}

/// 自检：扫描一段文本（通常是已脱敏的 fixture 序列化结果），列出疑似残留。
///
/// **这个函数本身也要被测** —— 否则"CI 通过"可能只是检查器失灵。
///
/// # 为什么报告里必须带位置
///
/// 只报"长度 89 的疑似 token"是**没法据以修复**的：修复者不知道是哪个接口、
/// 哪个字段、哪一侧（请求还是响应），只能靠猜。实测在 36 份真实 HAR 上跑本自检时，
/// 报出的三万余条残留全是这种没有位置的信息 —— 门禁是红的，但没人能把它转绿。
/// 所以 JSON 输入一律**按叶子逐条定位**，报出 `$` 路径。
///
/// # 为什么只按叶子扫，而不是扫整段序列化文本
///
/// JSON 里一个"token 长相的长串"必然落在某个字符串叶子里；按叶子扫描之后，
/// `$.request.headers[3].value` 这样的路径才既指得出位置、又对得上修复动作。
/// 顺带消掉一类噪音：整段文本扫描会把键名与长数字也算进"疑似 token"。
///
/// # 报告里不出现值本身
///
/// 只给 JSON 路径、形状分类与长度，**连前缀都不给**。这份报告是给修复者定位用的，
/// 不该反过来成为一条新的泄漏通道。
pub fn scan_for_leaks(text: &str) -> Vec<String> {
    let mut leaks = Vec::new();

    match serde_json::from_str::<Value>(text) {
        Ok(value) => scan_value(&value, "$", 0, &mut leaks),
        // 非 JSON（表单体、日志片段）没有结构可依，退回整段形状扫描
        Err(_) => collect_run_leaks(text, "$（非 JSON 文本）", &mut leaks),
    }

    leaks
}

/// 内嵌 JSON 的最大解析层数。
///
/// "JSON 里套 JSON" 在本项目里是常态而非特例：`build_fixture_entry` 会把整份请求体
/// 序列化成**一个字符串**存进 `request.body`。解析出来再按结构扫，才能区分"键名"与"值" ——
/// 否则 telemetry 类请求体里那些 URL 形状的**键名**会被当成疑似凭据：
/// 它们既不该报，也没法靠脱敏规则消掉（打码键名会毁掉结构）。
const MAX_EMBEDDED_JSON_DEPTH: usize = 4;

/// 按 JSON 结构逐叶子定位残留。
fn scan_value(value: &Value, path: &str, depth: usize, leaks: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if is_sensitive_field(key) {
                    let redacted = child.as_str() == Some(PLACEHOLDER)
                        || child.is_null()
                        || child.as_str() == Some("");
                    if !redacted {
                        leaks.push(format!("敏感字段未脱敏: {child_path}"));
                    }
                }
                // `match.key` 是**查表键**，不是载荷，因此不按载荷来查。
                // 两个理由都是结构性的，不是"为了让它变绿"：
                // 1. 它一旦被打码，`FixtureHttpClient` 用真实请求算出的键就永远命不中它；
                // 2. 它的形状天然不像 URL —— 主机带端口时（`host:7071/…`），
                //    `:` 会把主机名切掉，只剩 `7071/<长路径>` 这种"端口 + 路径"碎片，
                //    任何按形状的判定都必然误报。真正该查的是它派生自的 `request.url`。
                if key == "key" && path.ends_with(".match") {
                    continue;
                }
                // `headers` 是 `[[名字, 值], …]`。名字是元数据（`sanitize_headers` 也不动它），
                // 按载荷查会误报：实测响应头 `cross-origin-embedder-policy-report-only`
                // 本身就是 41 个字符的连字符串，形状上和一个 token 没有区别。
                if key == "headers"
                    && let Value::Array(pairs) = child
                {
                    for (index, pair) in pairs.iter().enumerate() {
                        if let Some(value) = pair.get(1) {
                            scan_value(value, &format!("{child_path}[{index}][1]"), depth, leaks);
                        }
                    }
                    continue;
                }
                scan_value(child, &child_path, depth, leaks);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                scan_value(item, &format!("{path}[{index}]"), depth, leaks);
            }
        }
        Value::String(text) => {
            // 只有**结构化**的内嵌内容才递归：纯数字/纯文本串按 JSON 解析也会成功
            // （`123` 是合法 JSON），若不加限制，一条 40 位纯数字的串会被当成
            // "已解析、无残留"而静默放过。
            if depth < MAX_EMBEDDED_JSON_DEPTH
                && let Ok(inner @ (Value::Object(_) | Value::Array(_))) =
                    serde_json::from_str::<Value>(text)
            {
                scan_value(&inner, path, depth + 1, leaks);
                return;
            }
            collect_run_leaks(text, path, leaks);
        }
        _ => {}
    }
}

/// 收集一段文本里"token 长相"的长串残留。
fn collect_run_leaks(text: &str, path: &str, leaks: &mut Vec<String>) {
    for run in token_like_runs(text) {
        leaks.push(format!(
            "疑似 token 明文于 {path}（{}，长度 {}）",
            describe_run(&run),
            run.len()
        ));
    }
}

/// 把"token 长相"分类。修复者据此判断该往哪条规则里补 —— 是补键名黑名单，
/// 还是补形状规则。
fn describe_run(run: &str) -> &'static str {
    if run.starts_with("eyJ") && run.matches('.').count() >= 2 {
        return "JWT";
    }
    if run.chars().all(|c| c.is_ascii_digit()) {
        return "纯数字串";
    }
    if run.chars().all(|c| c.is_ascii_hexdigit()) {
        return "十六进制串";
    }
    "base64/编码串"
}

/// 判断一个"token 长相"的长串其实是 URL / 路径 —— 这类内容我们**故意保留**
/// （fixture 回放必须靠它匹配请求），不能当成泄漏误报。
///
/// 只按形状判断，不依赖上下文：
/// - 以 `//` 开头：URL 被 `:` 截断后就是这个样子（`https://x` → `https` + `//x`）；
/// - 或 `host/path` 形状：第一段像域名。
///
/// # 主机名里允许 `:` 与 `-`
///
/// 少了 `:` 会把带端口的主机名判成凭据（`data.example.com:8080/newapp/action`
/// 在实测里就是这么被误报的），少了 `-` 会漏掉连字符域名。
/// 放宽这两个字符是安全的：真正的凭据走 base64/hex，**base64 字母表里没有 `.`**，
/// 而这里的判定要求第一段含 `.` —— 这个条件本身就把 base64 挡在外面了。
///
/// 之所以要放宽到"只看形状"：误报会让门禁永远红，比漏报更有害
/// —— 真正的第一道防线是 [`is_sensitive_field`] 的字段名黑名单。
fn looks_like_url(run: &str) -> bool {
    if run.starts_with("//") || run.starts_with("http") {
        return true;
    }
    // 只取第一段当主机名判断。取整串会在路径含 `:` 时误判 ——
    // 而长串里一旦出现 `:`，后面的片段就已经不带主机名了。
    let host = run.split_once('/').map_or(run, |(h, _)| h);
    !host.is_empty()
        && host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | ':'))
}

fn token_like_runs(text: &str) -> Vec<String> {
    const MIN_LEN: usize = 40;
    let mut result = Vec::new();
    let mut run = String::new();

    for c in text.chars() {
        if is_token_char(c) {
            run.push(c);
        } else {
            if run.len() >= MIN_LEN && !looks_like_url(&run) {
                result.push(run.clone());
            }
            run.clear();
        }
    }
    if run.len() >= MIN_LEN && !looks_like_url(&run) {
        result.push(run);
    }
    result
}

/// 生成一条 fixture 记录（已脱敏）。
pub fn build_fixture_entry(
    method: &str,
    url: &str,
    request_headers: &[(String, String)],
    request_body: &[u8],
    status: u16,
    response_headers: &[(String, String)],
    response_body: &[u8],
) -> Value {
    // JSON 响应只留结构化那一份。
    //
    // `body` 与 `body_json` 是同一份内容的两种写法，两份都存等于把夹具体积翻倍 ——
    // 实测 36 份 HAR 转出来 70 MB，其中一半是这份重复。`FixtureHttpClient`
    // 本来就优先用 `body_json`，回放行为不受影响；非 JSON 响应没有结构化写法，
    // 仍然存 `body`（`body_json` 为 null 是判据）。
    let response_json = response_body_json(response_body);
    let response_text = if response_json.is_null() {
        String::from_utf8_lossy(&sanitize_body(response_body)).to_string()
    } else {
        String::new()
    };

    json!({
        "match": {
            "key": match_key(method, url),
            "method": method.to_ascii_uppercase(),
        },
        "request": {
            "url": redact_url(url),
            "headers": sanitize_headers(request_headers),
            "body": String::from_utf8_lossy(&sanitize_body(request_body)).to_string(),
        },
        "response": {
            "status": status,
            "headers": sanitize_headers(response_headers),
            "body": response_text,
            // 结构化响应体（已脱敏），回放时优先用它
            "body_json": response_json,
        },
    })
}

fn response_body_json(body: &[u8]) -> Value {
    // 用 let-chain 把"能解析成 UTF-8"与"能解析成 JSON"两步合成一个条件：
    // 任一步失败都落到 Value::Null（非 JSON 响应本来也没有结构化内容可回放）。
    if let Ok(text) = std::str::from_utf8(body)
        && let Ok(mut value) = serde_json::from_str::<Value>(text)
    {
        sanitize_json(&mut value);
        return value;
    }
    Value::Null
}

/// 便捷构造：给单测用的 header 元组。
#[allow(dead_code)]
fn h(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_field_matching_is_case_and_separator_agnostic() {
        for key in [
            "auth",
            "AUTH",
            "Auth",
            "userDomainId",
            "user_domain_id",
            "accessToken",
            "access-token",
            "AccessToken",
        ] {
            assert!(is_sensitive_field(key), "{key} 应被判定为敏感字段");
        }
        for key in ["prizeId", "count", "date", "code", "nickname"] {
            assert!(!is_sensitive_field(key), "{key} 不应被判定为敏感字段");
        }
    }

    #[test]
    fn headers_get_redacted_but_harmless_ones_survive() {
        let headers = vec![
            h("auth", "eyJhbGciOiJIUzI1NiJ9.SECRET"),
            h("Cookie", "sid=abc123"),
            h("user-agent", "mcloud/12.6.0"),
            h("content-type", "application/json"),
        ];
        let out = sanitize_headers(&headers);
        assert_eq!(out[0].1, PLACEHOLDER);
        assert_eq!(out[1].1, PLACEHOLDER, "Cookie 大小写不敏感");
        assert_eq!(out[2].1, "mcloud/12.6.0", "普通 header 应保留");
        assert_eq!(out[3].1, "application/json");
    }

    #[test]
    fn nested_json_is_redacted_including_arrays() {
        let mut value = json!({
            "data": {
                "auth": "SECRET_AUTH",
                "profile": { "phone": "19900000001", "nickname": "账号2" },
                "accounts": [
                    { "id": "a1", "accessToken": "SECRET_TOKEN" },
                    { "id": "a2", "jwt": "SECRET_JWT" }
                ]
            },
            "code": 0
        });
        assert!(sanitize_json(&mut value));
        assert_eq!(value["data"]["auth"], PLACEHOLDER);
        assert_eq!(value["data"]["profile"]["phone"], PLACEHOLDER);
        assert_eq!(value["data"]["profile"]["nickname"], "账号2");
        assert_eq!(value["data"]["accounts"][0]["accessToken"], PLACEHOLDER);
        assert_eq!(value["data"]["accounts"][1]["jwt"], PLACEHOLDER);
        assert_eq!(value["code"], 0, "业务码不应被改");
        assert!(scan_for_leaks(&value.to_string()).is_empty());
    }

    #[test]
    fn non_json_body_gets_token_runs_redacted() {
        let body = "session=abcdefghijklmnopqrstuvwxyz0123456789ABCDEF&x=1";
        let out = String::from_utf8(sanitize_body(body.as_bytes())).unwrap();
        assert!(out.contains(PLACEHOLDER), "长 token 串应被遮蔽: {out}");
        assert!(out.contains("&x=1"), "短参数应保留: {out}");
    }

    #[test]
    fn binary_body_is_not_stored_verbatim() {
        let bytes = vec![0x00u8, 0xff, 0xfe, 0x01, 0x02];
        let out = String::from_utf8(sanitize_body(&bytes)).unwrap();
        assert_eq!(out, "<BINARY 5 bytes>");
    }

    #[test]
    fn match_key_drops_query_entirely() {
        assert_eq!(
            match_key(
                "post",
                "https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0&_t=1699999999"
            ),
            "POST m.mcloud.139.com/ycloud/signin/page/exchangeList"
        );
        assert_eq!(
            match_key("GET", "https://a.example.com/x"),
            "GET a.example.com/x"
        );
    }

    #[test]
    fn redact_url_strips_volatile_and_masks_sensitive() {
        let url =
            "https://a.example.com/api?clientVersion=12.6.0&_t=1699999999&nonce=abc&auth=SECRET";
        let out = redact_url(url);
        assert!(out.contains("clientVersion=12.6.0"), "业务参数保留: {out}");
        assert!(!out.contains("_t="), "时间戳应剔除: {out}");
        assert!(!out.contains("nonce="), "随机数应剔除: {out}");
        assert!(!out.contains("SECRET"), "凭据不得残留: {out}");
        assert!(out.contains(&format!("auth={PLACEHOLDER}")));
    }

    #[test]
    fn leak_scanner_actually_detects_a_leak() {
        // 这个用例是"检查器自身"的测试：如果扫描器失灵，
        // 后面所有"无泄漏"断言就都成了空话。
        let dirty = json!({ "auth": "REAL_SECRET_TOKEN" }).to_string();
        assert!(
            !scan_for_leaks(&dirty).is_empty(),
            "扫描器必须能发现未脱敏的敏感字段"
        );

        let dirty_text =
            "here is a very long secret token: AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ";
        assert!(
            !scan_for_leaks(dirty_text).is_empty(),
            "扫描器必须能发现非结构化的长 token"
        );
    }

    /// 扫描器不能把"故意保留的 URL"误报成泄漏 —— 否则门禁永远红，比漏报更有害。
    #[test]
    fn leak_scanner_ignores_urls_but_keeps_tokens() {
        // URL 形状：不该报
        assert!(
            scan_for_leaks("https://m.mcloud.139.com/ycloud/prize/exchange/apply/v3").is_empty(),
            "带 scheme 的 URL 不应误报"
        );
        assert!(
            scan_for_leaks("//cdn.example.com/a/very/long/path/that/definitely/exceeds/40")
                .is_empty(),
            "被截断的 URL 不应误报"
        );
        assert!(
            scan_for_leaks("m.mcloud.139.com/ycloud/prize/exchange/apply/v2").is_empty(),
            "host/path 形状不应误报"
        );

        // 真正的 token：必须报
        assert!(
            !scan_for_leaks("eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.PAYLOAD.SIGNATUREPAD123456")
                .is_empty(),
            "JWT 必须被识别"
        );
        assert!(
            !scan_for_leaks("AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ").is_empty(),
            "纯大写长串必须被识别"
        );
    }

    /// 真实 HAR 上暴露出来的四类形态，逐条钉住。
    ///
    /// 这些不是假想用例，是把 36 份真实 HAR 跑一遍之后自检报出来的形状：
    /// 它们都在"键名黑名单"之外，靠按名判断一条都拦不住。
    #[test]
    fn real_world_shapes_are_cleaned_end_to_end() {
        // ① 内嵌 JSON 里的 `userdomain_id`（下划线形态，靠归一化命中黑名单）
        let req_body = r#"{"header":{"custom":{"userdomain_id":"1990000000000000001"}}}"#;
        let entry = build_fixture_entry(
            "POST",
            "https://data.example.com:8080/newapp/action",
            &[
                // ② 名字不在白名单里的 cookie 类头：实测最常见的漏点
                h("cookieToken", "eyJhbGciOiJIUzI1NiJ9.PAYLOAD.SIGNATURE"),
                h("hecaiyundata2021jssdkcross", "YZsidssolgbmPq1234567890abcdefghij"),
                h("user-agent", "mcloud/12.6.0"),
            ],
            req_body.as_bytes(),
            200,
            &[h("content-type", "application/json")],
            // ③ 无害键名 + 长 base64 值（实测形态：base64 的 `mobile:手机号:token`）
            // ④ 响应里的 CDN 长路径段
            r#"{"data":{"blob":"bW9iaWxlOjE5OTAwMDAwMDAxOmFiY2RlZmdoaWprbG1ub3A=","records":[{"contentImageUrl":"https://cdn3.cmread.com/a/b/cccccccccccccccccccccccccccccccccccccccccccccccc.png"}]}}"#
                .as_bytes(),
        );

        let text = entry.to_string();
        let leaks = scan_for_leaks(&text);
        assert!(leaks.is_empty(), "仍有残留: {leaks:?}");

        for secret in ["1990000000000000001", "YZsidssolgbmPq", "PAYLOAD.SIGNATURE"] {
            assert!(!text.contains(secret), "{secret} 不得出现在 fixture 中");
        }
        // 白名单内的 header 与业务结构必须留下，否则夹具没法用
        assert!(text.contains("mcloud/12.6.0"), "user-agent 应保留");
        assert!(text.contains("contentImageUrl"), "业务键名应保留");
        assert_eq!(
            entry["match"]["key"],
            "POST data.example.com:8080/newapp/action"
        );
    }

    /// URL 形状的值：路径留下，但里面没名分的长串仍要打码。
    ///
    /// 这条钉的是"按 key 判断不够"——实测有接口把整段 base64 塞进 query 参数 `e`，
    /// 参数名毫无提示性。
    #[test]
    fn url_values_keep_path_but_lose_unnamed_payloads() {
        let url = format!(
            "https://data.example.com/newapp/action?e={}&m=1&clientVersion=12.6.0",
            "a".repeat(300)
        );
        let entry = build_fixture_entry("POST", &url, &[], b"", 200, &[], b"{}");
        let text = entry.to_string();
        assert!(scan_for_leaks(&text).is_empty(), "仍有残留");
        assert!(text.contains("m=1"), "短参数应保留");
        assert!(
            text.contains("clientVersion=12.6.0"),
            "业务参数应保留: {text}"
        );
        assert!(text.contains("newapp/action"), "路径应保留");
    }

    /// 标识形状的数字必须被改成合成号段 —— 判据与仓库守卫逐条对齐。
    #[test]
    fn identifier_shaped_numbers_are_remapped_to_synthetic_range() {
        // 两个"一眼假"的顺序占位值（已在 scripts/check-identifiers.cjs 的 ALLOWED 表登记）：
        // 它们与真实值同为 11 / 19 位且以 1 开头，因此能真正走到重映射分支。
        const FAKE_PHONE: &str = "12345678901";
        const FAKE_DOMAIN_ID: &str = "1000000000000000001";
        const FAKE_DOMAIN_ID_2: &str = "1234567890123456789";

        // 11 位：手机号形状。改写后长度与 199 前缀都要保持
        let phone = synthesize_identifiers(&format!("phone={FAKE_PHONE}"));
        assert_eq!(phone.len(), format!("phone={FAKE_PHONE}").len());
        assert!(phone.starts_with("phone=199"), "{phone}");

        // 19 位：userDomainId 形状。实测 `taskId` 也是这个形状，
        // 所以"反正不是标识"这条退路并不存在
        let task_id = format!("\"taskId\":\"{FAKE_DOMAIN_ID}\"");
        let remapped = synthesize_identifiers(&task_id);
        assert_eq!(remapped.len(), task_id.len(), "长度要保持，否则解析会变形");
        assert!(!remapped.contains(FAKE_DOMAIN_ID));
        assert!(remapped.contains("199"), "{remapped}");

        // 确定性：同输入同输出，否则夹具每次重新生成 diff 全是噪声
        assert_eq!(
            synthesize_identifiers(FAKE_DOMAIN_ID),
            synthesize_identifiers(FAKE_DOMAIN_ID)
        );
        assert_ne!(
            synthesize_identifiers(FAKE_DOMAIN_ID),
            synthesize_identifiers(FAKE_DOMAIN_ID_2),
            "不同原值应映射到不同结果"
        );

        // 形状不符的一律不动：位数不对、或两侧挨着字母数字
        for untouched in [
            "1699999999",
            "1234567890",
            "a12345678901",
            "12345678901a",
            "123",
        ] {
            assert_eq!(
                synthesize_identifiers(untouched),
                untouched,
                "{untouched} 不该被改"
            );
        }
    }

    /// 标识重映射必须覆盖所有文本入口，而不是只在某一条分支里生效。
    ///
    /// 这一路踩到的坑几乎都是"某条分支只做了一半"（URL 分支、非 JSON 分支、
    /// header 分支、裸数字…），所以这里逐个入口点名。
    #[test]
    fn identifier_remap_reaches_every_text_path() {
        // 顺序占位值（已登记），与真实值同形状，所以能真正走到重映射分支
        const REAL_ID: &str = "1000000000000000001";

        let entry = build_fixture_entry(
            "POST",
            // URL query 里的标识
            &format!("https://m.mcloud.139.com/a?taskId={REAL_ID}"),
            // 白名单 header 的值
            &[h("user-agent", &format!("mcloud {REAL_ID}"))],
            // 非 JSON 文本体
            format!("taskId={REAL_ID}").as_bytes(),
            200,
            &[],
            // JSON 体，且写成裸数字
            format!("{{\"taskId\":{REAL_ID}}}").as_bytes(),
        );

        let text = entry.to_string();
        assert!(!text.contains(REAL_ID), "标识在某个入口漏了出去: {text}");
    }

    #[test]
    fn end_to_end_har_like_entry_is_clean() {
        let req_headers = vec![
            h("auth", "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.PAYLOAD.SIG"),
            h("Cookie", "jsessionid=ABCDEF123456"),
            h("user-agent", "mcloud/12.6.0"),
        ];
        let req_body = r#"{"prizeId":123,"phone":"19900000001"}"#.as_bytes();
        // 注意：响应体里带中文昵称。`br#"..."#` 是字节串字面量，**不允许非 ASCII 字符**，
        // 所以含 CJK 的样本必须写成 `r#"..."#.as_bytes()`。
        let resp_body =
            r#"{"code":0,"data":{"nickname":"账号2","token":"SECRET_IN_RESPONSE","cloudCount":1286}}"#
                .as_bytes();

        let entry = build_fixture_entry(
            "POST",
            "https://m.mcloud.139.com/ycloud/prize/exchange",
            &req_headers,
            req_body,
            200,
            &[h("content-type", "application/json")],
            resp_body,
        );

        let text = entry.to_string();
        let leaks = scan_for_leaks(&text);
        assert!(leaks.is_empty(), "脱敏后仍有残留: {leaks:?}");

        for secret in [
            "PAYLOAD.SIG",
            "ABCDEF123456",
            "SECRET_IN_RESPONSE",
            "19900000001",
        ] {
            assert!(!text.contains(secret), "{secret} 不得出现在 fixture 中");
        }

        // 结构必须保留，否则 fixture 无法用于回放解析
        assert_eq!(entry["response"]["body_json"]["code"], 0);
        assert_eq!(entry["response"]["body_json"]["data"]["cloudCount"], 1286);
        assert_eq!(entry["response"]["body_json"]["data"]["nickname"], "账号2");
        assert_eq!(
            entry["match"]["key"],
            "POST m.mcloud.139.com/ycloud/prize/exchange"
        );
    }
}
