//! 直播口令提取（弹幕 + 小红书主页）—— **纯字符串处理，零 IO**。
//!
//! 对应原项目 `packages/core/src/live-room/kouling.ts` 与 `xiaohongshu.ts`，
//! 与 Go 侧 `internal/upstream/live_kouling.go` 同源同行为。
//!
//! # 为什么要**逐字**复刻原实现，包括它的假阳性
//!
//! 口令是**服务端当场校验**的：解错一个，代价是 `200108`
//! （实拍证明**零消耗** —— 那一枪什么都没扣）外加一次重听。
//! 而"顺手把某个分支写得更严"的代价**不对称** ——
//! 严过头会把**真口令**滤掉，而那一侧没有第二次机会（口令只在直播那一小时有效）。
//!
//! 所以本模块**刻意保留** `口令是相伴什么` 这种会被解成 `相伴什么` 的假阳性
//! （实拍弹幕里真有这一条，黄金向量的 `live-false-positive` 把它钉成行为契约）。
//! ⚠ **不要"改进"它** —— 那是改变行为，不是修 bug。
//!
//! # 权威与对拍
//!
//! 期望值全部出自 `test/golden/live-room-vectors.json` 的 `koulingExtract`（36 条）
//! 与 `xiaohongshuExtract`（5 条），它们由原项目的 TS 源码**真跑**生成
//! （`$evidence.originalSrc` 里记了 sha256），不是手写的。
//! ⚠ 该文件自己也记着：这两段**一度**是手写期望值，结果与真实现不符 ——
//! 所以这里的任何一条不一致都当 **bug** 处理，改实现而不改期望值。
//!
//! # ⚠ 两处"照抄会出错"的静默差异
//!
//! ## 1. `\s` 必须是 **JS 语义**的字符类（[`is_js_space`]）
//!
//! JS 的 `\s` = `[\t\n\v\f\r \u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]`，
//! 含全角空格 `U+3000`；而 Rust `regex` 的 `\s`（RE2 风格）**只有 ASCII 那六个**。
//! 照窄的写会让 `口令1：AAA　口令2：BBB` 变成"只有一个口令 `AAA　口令2：BBB`"。
//! 弹幕里全角空格很常见（手机输入法默认就是全角），
//! 黄金向量的 `live-ideographic-space` 就是为这条准备的。
//!
//! ## 2. "同一行多个口令"必须**显式扫描**，不能用前瞻正则
//!
//! 原式是 `/(?:复制)?口令\s*\d*\s*[：:]\s*(.*?)(?=\s+(?:复制)?口令\s*\d*\s*[：:]|无随机|$)/g`
//! —— 它用了**前瞻**。Rust 的 `regex` crate 出于线性时间保证**不支持前瞻**
//! （Go 的 `regexp` 同理）。这里照 Go 侧的做法改成显式扫描（[`same_line_codes`]），
//! 而不是"改成一个差不多的正则" —— 后者会**静默**改变切分行为。
//! 两个边界条件照抄：下一个口令头**必须以空白开头**；`无随机` **不要**前导空白。
//!
//! ## 3. 扫描推进必须按 JS `g` 的语义（**本模块唯一一处不照抄 Go 的地方**）
//!
//! JS 的 `g` 是**非重叠**扫描：下一次匹配从**上一次匹配的末位**开始。
//! 而 Go 参考实现的 `sameLineCodes` 写的是 `pos = headEnd`（只跨过"头"），
//! 于是同一个头会被重复扫到，内容里的 `口令2:` 还会被当成一个新的头。
//!
//! 两者在 `口令1:A口令2:B` 上给出**不同**答案：
//!
//! ```text
//! 原 TS（权威，真跑过）  → ["A口令2:B"]
//! Go 侧 sameLineCodes    → ["A口令2:B", "B"]   ← 多出一条
//! ```
//!
//! 黄金向量里没有这个形状（两条口令之间**都有**空白），所以这个差别一直没被测出来。
//! 本模块按 TS 推进（见 [`same_line_codes`] 里的说明与
//! [`tests::adjacent_heads_without_space_match_the_ts_global_scan`]）。
//!
//! # 分层的边界
//!
//! 本模块只做**纯函数**（输入文本 → 输出候选口令）。它**不发请求**、
//! 不读文件、不碰时钟。小红书那一侧的"拉主页"是 IO，属于上层；
//! 这里只提供 [`xiaohongshu_profile_url`] / [`xiaohongshu_profile_request`]
//! 两个**构造器**与纯解析函数 [`extract_kouling_from_xiaohongshu_html`]。

use mcloud_core::HttpRequest;
use regex::Regex;
use std::sync::OnceLock;

// ══════════════════════════ 与 JS 对齐的字符类 ══════════════════════════

/// JS `\s` 的字符类（供拼进正则）。
///
/// ⚠ 与 Rust `regex` 自带的 `\s` **不是一个集合**：那个只有 ASCII 六个。
/// 本模块内**所有**表示"空白"的位置都必须走它，否则全角空格会被漏掉。
///
/// `\x{feff}`（BOM）也在内 —— JS 的 `\s` 含它，看起来反直觉但照抄。
const JS_SPACE_CLASS: &str = r"[\t\n\v\f\r \x{00a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]";

/// 报告一个字符是否落在 JS 的 `\s` 里。
///
/// 它是 [`JS_SPACE_CLASS`] 的手写版，用于[`same_line_codes`] 的逐字符扫描
/// （那个地方不能用正则，否则又要引入前瞻）。
///
/// ⚠ 两份定义**必须保持一致** —— 测试 [`tests::js_space_class_and_predicate_agree`]
/// 用穷举把这件事钉住了。不一致会出现"正则说这里是空白、扫描说不是"这种
/// 只在某一个分支上显形的偏差，最难查。
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | ' '
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    ) || ('\u{2000}'..='\u{200a}').contains(&c)
}

/// 主播口令格式的收尾标记。
///
/// ⚠ 它同时是**格式 4 兜底**的触发条件（文本含它才走那一支）与**格式 2** 的内容
/// 终止符。两个用法不是一回事，别合并。
pub const NO_RANDOM_TOKEN: &str = "无随机";

// ══════════════════════════ 正则 ══════════════════════════

/// 口令格式的正则。每一条都对应原实现里的一个字面量。
///
/// ⚠ 用 `OnceLock` 惰性编译而不是 `lazy_static` / `once_cell`：
/// 标准库自带，不给本 crate 添依赖。编译失败只可能是**常量写错**
/// —— 那是开发期错误，所以 `expect` 在这里是可接受的（会立刻在第一条测试上炸）。
fn kouling_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"小红花口令[：:]{JS_SPACE_CLASS}*(.+)")).expect("格式 1 正则")
    })
}

/// "整行只是一个口令头"（内容在下一行）。
fn kouling_head_only_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"(?:复制)?口令{JS_SPACE_CLASS}*\d*{JS_SPACE_CLASS}*[：:]{JS_SPACE_CLASS}*$"
        ))
        .expect("整行口令头正则")
    })
}

/// "口令头"本身（不带行尾锚点），同一行切分用。
fn kouling_head_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"(?:复制)?口令{JS_SPACE_CLASS}*\d*{JS_SPACE_CLASS}*[：:]{JS_SPACE_CLASS}*"
        ))
        .expect("口令头正则")
    })
}

/// 判断一行是否**以口令开头**（`/^(?:复制)?口令/`）。
///
/// # ⚠ 它与 [`kouling_head_only_re`] **不是一回事**，别合并
///
/// 前者只要求"以口令二字开头"（`口令1：AAA` 也算），
/// 后者要求"整行就是个头"（必须以冒号结尾）。
/// 用途也不同：这一条用在"下一行是不是又是个头"的**排除**里。
fn kouling_head_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?:复制)?口令").expect("口令前缀正则"))
}

/// 格式 3：`口令是XXX`。
fn kouling_is_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"口令是(.+)").expect("格式 3 正则"))
}

/// 格式 4 的单条：`1:XXX`。
fn kouling_digit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"^\d+{JS_SPACE_CLASS}*[：:]{JS_SPACE_CLASS}*(.+)"))
            .expect("格式 4 正则")
    })
}

/// 匹配 `无随机` 及其**同一行**的剩余部分（JS 的 `.` 不跨行）。
///
/// ⚠ Rust `regex` 的 `.` **默认也不匹配 `\n`**，与 JS 一致
/// —— 所以这条不需要额外加 `(?s)`，加了反而会跨行删掉别的行。
fn kouling_no_random_rest_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"{NO_RANDOM_TOKEN}.*")).expect("无随机正则"))
}

/// 疑问句末尾的语气词。
fn trailing_particle_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[呀啊吧呢嘛哦]+$").expect("语气词正则"))
}

/// JS 的 `/\s+/`。
fn js_space_run_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"{JS_SPACE_CLASS}+")).expect("空白串正则"))
}

/// "整条就是一句提问"的那几个词。
///
/// ⚠ 判据是**去语气词后整条相等**，不是"包含"：`口令是相伴什么` 不在表里，
/// 所以它会被解成口令（原实现如此，见模块头）。
const QUESTION_WORDS: [&str; 6] = ["什么", "啥", "多少", "几", "哪个", "哪些"];

// ══════════════════════════ 公开判据 ══════════════════════════

/// 这条弹幕是否**只是一个口令头**（内容在下一帧）。
///
/// 用它的地方是**跨帧拼接**：主播先发 `复制口令1：`、再发 `短漫上新免费看`。
/// 分两条弹幕时，第一条单独解不出任何东西，必须记下"下一条整条就是口令"
/// —— 否则那一条真口令就丢了，而丢掉的那一侧没有第二次机会。
///
/// ⚠ 与 [`extract_kouling`] 里那个 `IsKoulingHead(line)` 分支是**同一判据**，
/// 两边都 trim 过输入：`is_kouling_head("  口令：  ")` 为真。
pub fn is_kouling_head(text: &str) -> bool {
    kouling_head_only_re().is_match(text.trim())
}

/// 这条文本**是否值得送进提取器**。
///
/// 它的用处是**过滤**（小红书那边只对含关键词的候选文本做提取），
/// 所以它比 [`extract_kouling`] 宽松得多 —— 宽了只是多算几次，窄了会漏。
pub fn has_kouling(text: &str) -> bool {
    text.contains("口令") || text.contains("小红花")
}

/// 解出来的"口令"其实是不是观众的一句提问。
///
/// 弹幕里大量出现 `口令是什么` / `口令是啥` / `第2个口令是什么？` ——
/// 它们是**观众在问**，不是主播在发。不滤掉的话每一次都会换回一个 `200108`。
///
/// ⚠ 判据只有两条：含 `?` / `？`，**或**去语气词后整条等于表里的疑问词。
/// `口令在哪里` 靠的是**整条不等**（"哪里"根本不在表里），
/// 而不是"包含疑问词"—— 后者会把 `口令是带着云盘去上学` 误伤掉。
pub fn is_question(code: &str) -> bool {
    if code.contains('?') || code.contains('？') {
        return true;
    }
    let trimmed = trailing_particle_re().replace(code, "");
    QUESTION_WORDS.iter().any(|w| trimmed == *w)
}

// ══════════════════════════ 主提取器 ══════════════════════════

/// 从一条弹幕里提取口令。
///
/// # 四个分支**按顺序累加**（不是"命中一个就返回"）
///
/// ```text
/// ① `小红花口令：XXX`
/// ② 逐行扫口令头：整行是头 ⇒ 取**下一行**；行内有头 ⇒ 用 same_line_codes 切
/// ③ `口令是XXX`
/// ④ **仅当 ①②③ 全空**且文本含 `无随机` ⇒ 按空白切、找 `N:XXX`
/// ```
///
/// 顺序不是风格：③ 的 `口令是(.+)` 会命中 ② 里那些行的内容，
/// ④ 是"上面都没认出来时的最后兜底"，条件里那句 `result.length === 0` **不能省**
/// （省了它，多行格式会额外多出一批垃圾）。
///
/// 最后**保序去重**（`[...new Set()]`）再过一遍疑问句过滤 ——
/// 顺序对"先拿到哪个口令"有影响（先领哪一条），去重则防止同一个口令被领两次。
///
/// # 一个刻意保留的"重复"
///
/// `小红花口令：AAA 无随机` 会同时命中 ① 与 ②：
/// ① 给出 `AAA 无随机`，② 给出 `AAA`。黄金向量的 `live-no-random-with-head`
/// 把**两条**都钉住了。看起来像 bug，但原实现如此 ——
/// 且这一个"多出来的候选"是**更全**的那一条（含收尾标记），
/// 收紧它反而会把某些真实形状切掉。
pub fn extract_kouling(msg: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();

    // ① `小红花口令：XXX`
    if let Some(m) = kouling_prefix_re().captures(msg) {
        result.push(m[1].trim().to_string());
    }

    // ② 逐行
    //
    // ⚠ 两个分支是**互斥**的：整行只是口令头 ⇒ 取下一行，然后 `continue`
    // （**不再**做同一行切分）。原实现那个 `continue` 就是这个意思，
    // 漏掉它会让 `复制口令1：` 这一行既取到下一行、又被同一行切分算一遍。
    let normalized = normalize_newlines(msg);
    let lines: Vec<&str> = normalized.split('\n').collect();
    for (i, raw_line) in lines.iter().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if is_kouling_head(line) {
            if let Some(next) = lines.get(i + 1) {
                let content = next.trim();
                // 两道排除：下一行自己也是个口令头、或含 `无随机`（那是收尾标记）。
                if !content.is_empty()
                    && !kouling_head_prefix_re().is_match(content)
                    && !content.contains(NO_RANDOM_TOKEN)
                {
                    result.push(content.to_string());
                }
            }
            continue;
        }
        result.extend(same_line_codes(line));
    }

    // ③ `口令是XXX`
    if let Some(m) = kouling_is_re().captures(msg) {
        result.push(m[1].trim().to_string());
    }

    // ④ 纯数字编号兜底：**只在前面全空**时才走。
    if result.is_empty() && msg.contains(NO_RANDOM_TOKEN) {
        // JS 的 `msg.replace(/无随机.*/, "")` 是**非全局**替换：只去掉第一处
        // 以及它**本行**剩下的部分（`.` 不跨行）。用 `replace_all` 会多去掉后面几行,
        // 而那正好是"多行格式"的形状。
        let stripped = match kouling_no_random_rest_re().find(msg) {
            Some(m) => format!("{}{}", &msg[..m.start()], &msg[m.end()..]),
            None => msg.to_string(),
        };
        for part in js_space_run_re().split(stripped.trim()) {
            if let Some(m) = kouling_digit_re().captures(part) {
                let code = m[1].trim();
                if !code.is_empty() {
                    result.push(code.to_string());
                }
            }
        }
    }

    filter_codes(result)
}

/// 把 `\r\n` 归一成 `\n`（对齐 JS 的 `split(/\r?\n/)`）。
///
/// ⚠ JS 那个正则其实是 `/\r?\n/`：**孤立的 `\r`**（老 Mac 换行）不会被当换行。
/// 这里跟着归一化，因为弹幕是 JSON 里来的、`\r` 只会出现在 `\r\n` 里；
/// 而把孤立 `\r` 当换行只会多切一刀（多切一刀最多多一个候选，**不会漏**）。
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// 处理一行里的口令头，返回这一行能解出的口令。
///
/// # 它替代的是原实现那个带前瞻的正则
///
/// 原式：`/(?:复制)?口令\s*\d*\s*[：:]\s*(.*?)(?=\s+(?:复制)?口令\s*\d*\s*[：:]|无随机|$)/g`
///
/// 逐条照抄它的语义：
///
/// - 每个口令头的**内容**从头的末尾开始，到下面三者中**最先**出现的一个为止：
///   ① 一个**以空白开头**的下一个口令头（`\s+口令…`）；
///   ② `无随机`（**没有**前导空白要求）；
///   ③ 行尾。
/// - 所以 `口令1:A口令2:B`（中间没空格）**不会**被切成两条 ——
///   内容是 `A口令2:B`。这不是 bug，是那个前瞻要求 `\s+` 的直接后果。
///   ⚠ **不要"顺手修"**：原实现如此，改了就与原实现解出不同的口令集。
/// - 下一个匹配从上一个头结束处继续扫（`g` 的语义），所以中间那段空白
///   会被跳过，不会变成一个"没有头的内容"。
///
/// 返回值已 trim；空内容与含 `无随机` 的内容都丢弃（对齐原式里那两处判断）。
fn same_line_codes(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < line.len() {
        // ⚠ `line[pos..]` 的安全性：`pos` 只会被推进到某个匹配的**结束位置**
        // 或某个字符的边界上（见下），两者都在 UTF-8 边界上。
        let Some(loc) = kouling_head_re().find(&line[pos..]) else {
            break;
        };
        let head_end = pos + loc.end();
        let mut content_end = line.len();

        // 从头的末尾往后找第一个边界。
        let mut j = head_end;
        while j < line.len() {
            if line[j..].starts_with(NO_RANDOM_TOKEN) {
                content_end = j;
                break;
            }
            let Some(c) = line[j..].chars().next() else {
                break;
            };
            if is_js_space(c) {
                // 跳过整段空白，看紧跟着的是不是一个口令头。
                let mut k = j;
                for next in line[k..].chars() {
                    if !is_js_space(next) {
                        break;
                    }
                    k += next.len_utf8();
                }
                if k > j && kouling_head_re().find(&line[k..]).map(|m| m.start()) == Some(0) {
                    content_end = j;
                    break;
                }
                j = k;
                continue;
            }
            j += c.len_utf8();
        }

        let code = line[head_end..content_end].trim();
        if !code.is_empty() && !code.contains(NO_RANDOM_TOKEN) {
            out.push(code.to_string());
        }
        // ⚠ 推进到**整个匹配的末位**（= `content_end`），而不是 `head_end`。
        //
        // 这是与 Go 参考实现**刻意**的一处偏离，也是本模块唯一一处"不照抄 Go"。
        // 原 TS 用的是 `String.prototype.matchAll` + `g` 标志，而 `g` 的语义是
        // **非重叠**：下一次匹配从**上一次匹配的末位**开始。原式整体
        // `(?:复制)?口令…[：:]\s*(.*?)(?=…)` 的末位就是内容结束的位置。
        //
        // Go 侧写的是 `pos = headEnd`（只跳过"头"），于是同一个头会被
        // 重新扫到，还会把头**之后**的内容里那段 `口令2:` 当成新的头。
        // 两者在 `口令1:A口令2:B` 上答案不同：
        //
        //   TS（权威，真跑过）      → `["A口令2:B"]`
        //   Go 侧 sameLineCodes     → `["A口令2:B", "B"]`  ← 多出一条
        //
        // 黄金向量里没有这个形状（`live-real-2` / `ts-digit-format` 两条口令
        // 之间**都有**空白），所以这个差别一直没被测出来。
        // 任务口径是"行为与原 TS 实现逐字一致"，且**原 TS 是权威**
        // ⇒ 这里按 TS 的 `g` 语义推进。用户看得见的影响：多出来的那条 `B`
        // 会被拿去领一次，而它必然回 `200108`（白跑一枪）。
        pos = if content_end > head_end {
            content_end
        } else {
            head_end
        };
    }
    out
}

/// 保序去重 + 疑问句过滤（对齐 `[...new Set(r)].filter(c => !isQuestion(c))`）。
fn filter_codes(codes: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::with_capacity(codes.len());
    let mut out = Vec::with_capacity(codes.len());
    for code in codes {
        if !seen.insert(code.clone()) {
            continue;
        }
        if is_question(&code) {
            continue;
        }
        out.push(code);
    }
    out
}

// ══════════════════════════ 源 B：小红书主页 ══════════════════════════

/// 口令发布方的小红书主页。
///
/// ⚠ 它是**硬编码的公开主页**（没有凭据）—— 口令除了直播弹幕之外还会在这里
/// 以笔记形式发一遍，所以它是 WS 之外唯一的口令来源。
pub const XIAOHONGSHU_PROFILE_URL: &str =
    "https://www.xiaohongshu.com/user/profile/568caf13aed7585949773b30";

/// 拉主页 HTML 的请求。
///
/// UA/referer 伪装成桌面 Chrome：小红书对非常规 UA 会回一个**空壳页面**，
/// 而"空壳页面"与"这期没有口令"在解析层看起来**一模一样**（都返回 `[]`）
/// —— 这正是这里必须逐字照抄伪装头的原因（真实客户端就是这么发的）。
pub fn xiaohongshu_profile_request() -> HttpRequest {
    HttpRequest::get(XIAOHONGSHU_PROFILE_URL)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .header("Referer", "https://www.xiaohongshu.com/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
        )
}

/// 原实现认的那**六个**具名实体。
///
/// ⚠ 只有六个：多认几个**看起来**更对，但那是**行为变化**
/// （原实现会把 `&copy;` 原样留在文本里）。口令里出现实体的概率极低，
/// 而"两边解出不同的文本"会让黄金向量失去意义。
const NAMED_ENTITIES: [(&str, &str); 6] = [
    ("amp", "&"),
    ("apos", "'"),
    ("gt", ">"),
    ("lt", "<"),
    ("nbsp", " "),
    ("quot", "\""),
];

fn xs_json_field_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r#""(?:displayTitle|title|desc)"{JS_SPACE_CLASS}*:{JS_SPACE_CLASS}*"((?:\\.|[^"\\])*)""#
        ))
        .expect("内嵌 JSON 字段正则")
    })
}

fn xs_element_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)<(?:a|article|div|h\d|p)[^>]*>([\s\S]*?)</(?:a|article|div|h\d|p)>")
            .expect("元素正则")
    })
}

fn xs_meta_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta\b[^>]*(?:content=["']([^"']+)["'])[^>]*>"#).expect("meta 正则")
    })
}

fn xs_entity_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)&(?:#(\d+)|#x([\da-f]+)|(\w+));").expect("实体正则"))
}

fn xs_br_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<br\s*/?\s*>").expect("br 正则"))
}

fn xs_block_end_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)</(?:a|article|div|h\d|li|p|section)>").expect("块结束正则"))
}

fn xs_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("标签正则"))
}

fn xs_run_spaces_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[ \t]+").expect("连续空格正则"))
}

fn xs_run_breaks_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"\n{JS_SPACE_CLASS}+")).expect("换行后空白正则"))
}

fn xs_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\r?\n").expect("行分隔正则"))
}

/// 解 HTML 实体（数字、hex、六个具名）。
///
/// 认不出的具名实体**原样保留**（含 `&` 与 `;`），对齐原实现的
/// `namedEntities[name] ?? entity`。
///
/// ⚠ 超过 `U+10FFFF` 的码点在这里**保留原文**，而 JS 的 `String.fromCodePoint`
/// 会**抛异常**（整个提取失败）。这一处刻意更宽：失败的那一侧是"少一个候选"，
/// 而"保留原文"最多多一个解不出口令的候选 —— 两者都不影响正常路径。
pub fn decode_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for caps in xs_entity_re().captures_iter(text) {
        let whole = caps.get(0).expect("group 0");
        out.push_str(&text[last..whole.start()]);
        let replacement = if let Some(d) = caps.get(1) {
            code_point_or_raw(d.as_str(), 10, whole.as_str())
        } else if let Some(h) = caps.get(2) {
            code_point_or_raw(h.as_str(), 16, whole.as_str())
        } else if let Some(n) = caps.get(3) {
            NAMED_ENTITIES
                .iter()
                .find(|(name, _)| *name == n.as_str().to_ascii_lowercase())
                .map(|(_, v)| (*v).to_string())
                .unwrap_or_else(|| whole.as_str().to_string())
        } else {
            whole.as_str().to_string()
        };
        out.push_str(&replacement);
        last = whole.end();
    }
    out.push_str(&text[last..]);
    out
}

/// 把一串数字按给定进制解成一个字符；越界或非法就返回 [`fallback`]。
fn code_point_or_raw(digits: &str, base: u32, fallback: &str) -> String {
    let mut value: u32 = 0;
    for c in digits.chars() {
        let Some(digit) = c.to_digit(base) else {
            return fallback.to_string();
        };
        value = match value.checked_mul(base).and_then(|v| v.checked_add(digit)) {
            Some(v) => v,
            None => return fallback.to_string(),
        };
    }
    match char::from_u32(value) {
        Some(c) => c.to_string(),
        None => fallback.to_string(),
    }
}

/// 把一个 JSON 字符串**体**（不含引号）解成原文。
///
/// 先按 JSON 解（正确路径），解不开再手工替换 —— 顺序照抄原实现
/// （`\\n` → `\\r` → `\\t` → `\\"` → `\\\\`）。顺序有意义：
/// 先替换 `\\"` 会把 `\\\\\\"` 这种转义序列切错。
pub fn decode_json_string(value: &str) -> String {
    if let Ok(decoded) = serde_json::from_str::<String>(&format!("\"{value}\"")) {
        return decoded;
    }
    value
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

/// 把一段 HTML 片段压成纯文本。
pub fn html_to_text(html: &str) -> String {
    let text = xs_br_re().replace_all(html, "\n");
    let text = xs_block_end_re().replace_all(&text, "\n");
    let text = xs_tag_re().replace_all(&text, " ");
    let text = decode_html_entities(&text);
    let text = xs_run_spaces_re().replace_all(&text, " ");
    let text = xs_run_breaks_re().replace_all(&text, "\n");
    text.trim().to_string()
}

/// 按四级顺序收集**含关键词**的候选文本。
///
/// ```text
/// ① 内嵌 JSON 的 `displayTitle` / `title` / `desc`（服务端注水的那份状态）
/// ② 服务端渲染的笔记卡片（`<a|article|div|h\d|p>` 里的文本）
/// ③ `<meta content="…">`（SEO 元数据）
/// ④ 整页纯文本逐行兜底
/// ```
///
/// 顺序就是**页面顺序**，而主页是按时间倒序的 ⇒ 第一条命中即最新笔记
/// （见 [`extract_kouling_from_xiaohongshu_html`]）。
fn collect_candidate_texts(html: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |value: &str| {
        // ⚠ 这里是 JS 的 `/\s+/g`，**不是** [`html_to_text`] 里那个 `[ \t]+`。
        // 差别在换行：内嵌 JSON 的 `"口令是A\nB"` 解出来是**带换行**的两行，
        // 折成空格才是原实现喂给提取器的形状。用 `[ \t]+` 的话换行留着，
        // 而 [`extract_kouling`] 的 `口令是(.+)` 遇换行就停 ⇒ 解出半个口令。
        let text = js_space_run_re()
            .replace_all(&decode_html_entities(value), " ")
            .trim()
            .to_string();
        if text.is_empty() || !has_kouling(&text) {
            return;
        }
        if !seen.insert(text.clone()) {
            return;
        }
        candidates.push(text);
    };

    for caps in xs_json_field_re().captures_iter(html) {
        add(&decode_json_string(&caps[1]));
    }
    for caps in xs_element_re().captures_iter(html) {
        add(&html_to_text(&caps[1]));
    }
    for caps in xs_meta_re().captures_iter(html) {
        add(&caps[1]);
    }
    for line in xs_line_re().split(&html_to_text(html)) {
        add(line);
    }
    candidates
}

/// 从主页 HTML 里提取**最新一条**口令；没有就是**空 `Vec`**（不是 `None`）。
///
/// # 为什么"第一条命中即返回"
///
/// 主页按时间倒序 ⇒ 页面顺序第一条可解析的口令笔记就是最新的那一期。
/// 继续往下找会把**历史口令**也交出去，而那些一律回 `200108`
/// —— 除了白跑一趟，还会让"无效口令 ⇒ 重听一次"这条逻辑被自己触发的噪声带偏。
///
/// ⚠ 返回**空 `Vec` 而不是 `None`**：黄金向量 `no-kouling-at-all` 的 `why` 里
/// 明确写着"空结果的形状（返回 `[]` 而不是 nil）"。两者在调用侧几乎没差别，
/// 但这一族的调用方会把结果直接累进一个列表，`None` 会逼出一堆 `unwrap_or_default`。
pub fn extract_kouling_from_xiaohongshu_html(html: &str) -> Vec<String> {
    for candidate in collect_candidate_texts(html) {
        let codes = extract_kouling(&candidate);
        if !codes.is_empty() {
            return codes;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests;
