//! 直播口令提取的协议测试。
//!
//! 判据来源优先级（本项目一贯口径）：**实拍黄金向量 > 原实现源码 > 直觉**。
//!
//! 本文件的两组黄金向量取自 `test/golden/live-room-vectors.json`
//! 的 `koulingExtract`（36 条）与 `xiaohongshuExtract`（5 条）。
//! ⚠ 它们由原项目的 TS 源码**真跑**生成（`$evidence.originalSrc` 记了 sha256），
//! 不是手写期望值 —— 所以这里的任何一条不一致都当 **bug** 处理，
//! **改实现而不改期望值**。

use super::*;

fn vectors() -> serde_json::Value {
    // 用 `CARGO_MANIFEST_DIR` 而不是相对 `cwd`：测试运行的 cwd 不保证是
    // crate 根，相对路径会让这条测试在不同调用方式下时灵时不灵。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/live-room-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

/// 把 JSON 的字符串数组取成 `Vec<String>`。
fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .expect("应为数组")
        .iter()
        .map(|x| x.as_str().expect("元素应为字符串").to_string())
        .collect()
}

// ══════════════════════════ 黄金向量对拍 ══════════════════════════

/// ⭐ 36 条 `koulingExtract` 黄金向量**逐条**对拍。
///
/// # 为什么是一个循环而不是 36 个 `#[test]`
///
/// 每条用例的 `name` 会打进取证文案里（`assert_eq!` 的第三个参数），
/// 所以失败时能直接定位到是哪一条、以及它的 `why`。
/// 拆成 36 个测试函数只能得到"某一条挂了"，还得回去对名字。
///
/// ⚠ 条数**写死**在断言里：向量文件被删条目时必须炸，
/// 而不是"少测了 30 条但全绿"。
#[test]
fn golden_kouling_extract_vectors_all_pass() {
    let v = vectors();
    let cases = v["koulingExtract"]
        .as_array()
        .expect("koulingExtract 应为数组");
    assert_eq!(cases.len(), 36, "黄金向量条数变了 —— 先确认是不是被改动过");

    for case in cases {
        let name = case["name"].as_str().expect("用例应有 name");
        let why = case["why"].as_str().unwrap_or("");
        let text = case["text"].as_str().expect("用例应有 text");
        let expect = strings(&case["expect"]);

        assert_eq!(
            extract_kouling(text),
            expect,
            "黄金向量 `{name}` 不通过（{why}）；输入 = {text:?}"
        );
    }
}

/// ⭐ 5 条 `xiaohongshuExtract` 黄金向量**逐条**对拍。
#[test]
fn golden_xiaohongshu_extract_vectors_all_pass() {
    let v = vectors();
    let cases = v["xiaohongshuExtract"]
        .as_array()
        .expect("xiaohongshuExtract 应为数组");
    assert_eq!(cases.len(), 5, "黄金向量条数变了 —— 先确认是不是被改动过");

    for case in cases {
        let name = case["name"].as_str().expect("用例应有 name");
        let why = case["why"].as_str().unwrap_or("");
        let html = case["html"].as_str().expect("用例应有 html");
        let expect = strings(&case["expect"]);

        assert_eq!(
            extract_kouling_from_xiaohongshu_html(html),
            expect,
            "黄金向量 `{name}` 不通过（{why}）"
        );
    }
}

/// ⚠ `no-kouling-at-all` 要求的是**空 `Vec`**，不是 `None`
/// —— 黄金向量的 `why` 逐字写着"返回 `[]` 而不是 nil"。
#[test]
fn empty_result_is_an_empty_vec_not_none() {
    let codes = extract_kouling_from_xiaohongshu_html("<html><body>今天天气不错</body></html>");
    assert!(codes.is_empty());
    assert_eq!(codes.len(), 0);
    assert!(extract_kouling("").is_empty());
}

// ══════════════════════════ 两条静默差异（本项目踩过的坑）══════════════════════════

/// ⭐ JS 的 `\s` 含全角空格 `U+3000` —— 这一条是**照抄窄字符类**的探针。
///
/// 用 Rust `regex` 自带的 `\s`（只有 ASCII 六个）写，
/// `口令1：AAA　口令2：BBB` 会变成**一个**口令 `AAA　口令2：BBB`。
///
/// 这里除了黄金向量里那条，再补一条**同形状但换字符**的：
/// `U+00A0`（不换行空格）也在 JS 的 `\s` 里，而它比全角空格更隐蔽
/// —— 从弹幕里看不出来，复制粘贴时也不显形。
#[test]
fn js_space_covers_ideographic_and_nbsp() {
    assert_eq!(extract_kouling("口令1：AAA　口令2：BBB"), ["AAA", "BBB"]);
    assert_eq!(
        extract_kouling("口令1：AAA\u{00a0}口令2：BBB"),
        ["AAA", "BBB"]
    );
    // 段分隔符 `U+2028` 也在 JS 的 `\s` 里（第 1 条硬约束点名的那个）
    assert_eq!(
        extract_kouling("口令1：AAA\u{2028}口令2：BBB"),
        ["AAA", "BBB"]
    );
    // 对照：ASCII 空格**本来**就该切（不能因为改宽而改坏）
    assert_eq!(extract_kouling("口令1：AAA 口令2：BBB"), ["AAA", "BBB"]);

    // ⚠ 逐字符钉住 `is_js_space`：它必须与拼进正则的那个字符类**完全一致**，
    // 否则会出现"正则说是空白、扫描说不是"这种只在某一个分支上显形的偏差。
    for c in [
        '\t', '\n', '\u{0b}', '\u{0c}', '\r', ' ', '\u{00a0}', '\u{1680}', '\u{2000}', '\u{2005}',
        '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
    ] {
        assert!(is_js_space(c), "U+{:04X} 应当在 JS 的 \\s 里", c as u32);
    }
    // 三个"看像空白但不在表里"的对照，防 `char::is_whitespace` 式的误宽
    for c in ['\u{200b}', '\u{180e}', '\u{3001}'] {
        assert!(!is_js_space(c), "U+{:04X} 不该在 JS 的 \\s 里", c as u32);
    }
}

/// ⭐ `is_js_space` 与 `JS_SPACE_CLASS` 必须**同集**。
///
/// 两份定义分散在两个地方（一份是字符串、一份是 `matches!`），
/// 靠人眼看不出它们是同一个集合。这里用那个正则逐个字符反查。
#[test]
fn js_space_class_and_predicate_agree() {
    let re = Regex::new(&format!("^{JS_SPACE_CLASS}$")).expect("字符类应可编译");
    // 取一段覆盖所有候选的字符：ASCII + 几个典型 Unicode 块
    let probe: Vec<char> = ('\u{00}'..='\u{3000}')
        .chain(['\u{3001}', '\u{feff}', '\u{1680}', '\u{200b}'])
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect();
    let mut checked = 0;
    for c in probe {
        let in_re = re.is_match(&c.to_string());
        assert_eq!(
            in_re,
            is_js_space(c),
            "U+{:04X} 在两份定义里不一致（正则 {in_re} / 谓词 {}）",
            c as u32,
            is_js_space(c)
        );
        checked += 1;
    }
    assert!(checked > 100, "探针太少（{checked}），别让它变成空测试");
}

/// ⭐ 相邻口令头**中间没有空白**时不切第二条 —— 前瞻里那个 `\s+` 的直接后果。
///
/// 原式要求下一个口令头前面**有** `\s+`，所以 `口令1:A口令2:B` 的内容就是
/// `A口令2:B`（一整条），**只解出一条**。这不是 bug：
/// 真实主播格式里两条口令之间总有分隔符。
///
/// # ⚠ 这条同时钉住扫描的**推进语义**（本模块唯一一处不照抄 Go）
///
/// JS 的 `g` 是**非重叠**扫描，下一次匹配从**上一次匹配的末位**开始。
/// Go 参考实现写的是 `pos = headEnd`，于是会把内容里的 `口令2:` 再当成一个头，
/// 多解出一条 `B`。**原 TS 是权威**（它真跑过），所以这里按 TS 断。
/// 用户看得见的差别：Go 那版会白投一枪（必然回 `200108`）。
#[test]
fn adjacent_heads_without_space_match_the_ts_global_scan() {
    assert_eq!(
        extract_kouling("口令1:A口令2:B"),
        ["A口令2:B"],
        "没有空白分隔 ⇒ 只有一条；不许像 Go 那样多解出一条 `B`"
    );
    // 有空白就切（对照）
    assert_eq!(extract_kouling("口令1:A 口令2:B"), ["A", "B"]);
    // 三个头连在一起：整行仍是一条
    assert_eq!(
        extract_kouling("口令1:A口令2:B口令3:C"),
        ["A口令2:B口令3:C"]
    );
}

/// `无随机` 与下一个口令头的边界要求**不同**：
/// 前者**不要**前导空白，后者**要**。
#[test]
fn no_random_stops_content_without_leading_space() {
    // `无随机` 紧贴着内容 ⇒ 在它之前截断
    assert_eq!(extract_kouling("口令1：AAA无随机"), ["AAA"]);
    // 不带 `无随机` 时整行都是内容
    assert_eq!(extract_kouling("口令1：AAA"), ["AAA"]);
}

/// ⭐ 下一行必须是**真口令**的两道排除（都照抄原实现）。
#[test]
fn next_line_exclusions_match_the_original() {
    // 下一行自己也是口令**头** ⇒ 排除（否则会把 `复制口令2：` 当口令）
    assert_eq!(extract_kouling("复制口令1：\n复制口令2：\nAAA"), ["AAA"]);
    // 下一行含 `无随机` ⇒ 排除（它是收尾标记，不是口令）
    assert!(extract_kouling("复制口令1：\n无随机口令").is_empty());
    // ⚠ 排除用的是"以口令开头"，不是"整行是个头"：
    // `口令1：真正的口令` 不是头（没有行尾冒号），但**以口令开头** ⇒ 下一行被排除。
    // 而它自己那一行**不是**头 ⇒ 走同一行切分，于是 `真正的口令` 从那一行解出来。
    // （原 TS 真跑确认：结果是 `["真正的口令"]`，不是空。）
    assert_eq!(
        extract_kouling("复制口令：\n口令1：真正的口令"),
        ["真正的口令"]
    );
    // 最后一行后面没有下一行 ⇒ 什么都不取（不能越界）
    assert!(extract_kouling("复制口令1：").is_empty());
}

/// ⭐ 整行是口令头时，**同一行切分不参与** —— 那个 `continue` 不能漏。
///
/// 漏了它，`复制口令1：` 这一行会先取下一行、再被同一行切分算一遍，
/// 于是同一条口令出现两次（去重能盖住这一次，但顺序会被打乱）。
#[test]
fn head_only_line_does_not_also_go_through_same_line_split() {
    let codes =
        extract_kouling("复制口令1：\n短漫上新免费看\n复制口令2：\n云盘AI高效复盘\n无随机口令");
    assert_eq!(codes, ["短漫上新免费看", "云盘AI高效复盘"]);
}

/// 格式 4 兜底**只在 ①②③ 全空时**才走 —— 那个 `result.length === 0` 不能省。
#[test]
fn digit_fallback_only_when_everything_else_is_empty() {
    // ①②③ 全空 ⇒ 走兜底
    assert_eq!(
        extract_kouling("1:短漫上新免费看 2:云盘AI高效复盘 无随机"),
        ["短漫上新免费看", "云盘AI高效复盘"]
    );
    // ① 已经有结果 ⇒ **不走**兜底（否则多出一批垃圾）。
    //    ⚠ 这里**两条**都来自格式 1 与同一行切分**同时**命中
    //    （`live-no-random-with-head` 那个契约），而格式 4 一条都没参与。
    assert_eq!(
        extract_kouling("小红花口令：AAA 2:BBB 无随机"),
        ["AAA 2:BBB 无随机", "AAA 2:BBB"]
    );
    // 没有 `无随机` ⇒ 不走兜底
    assert!(extract_kouling("1:AAA 2:BBB").is_empty());
    // ⚠ 兜底**只在** result 为空时走：把同一段文本前面加一条解得出的口令，
    //    兜底那部分就必须**完全消失**（这才是 `result.length === 0` 的判据）
    assert_eq!(
        extract_kouling("口令是ZZZ\n1:AAA 2:BBB 无随机"),
        ["ZZZ"],
        "已有结果 ⇒ 格式 4 整个不参与"
    );
}

/// ⭐ `无随机` 的删除是**非全局**、且**不跨行**的。
///
/// JS 是 `msg.replace(/无随机.*/, "")`：只去掉**第一处**以及它**本行**剩下的部分。
/// 用 `replace_all` / `(?s)` 会多删后面几行 —— 那正好是多行格式的形状。
#[test]
fn no_random_removal_is_non_global_and_single_line() {
    // 第一处 `无随机` 之后的**同一行**被删掉，第二行的内容必须留下
    let codes = extract_kouling("1:AAA 无随机 2:BBB\n3:CCC");
    // `无随机 2:BBB` 整段被删 ⇒ 只剩 `1:AAA` 与 `\n3:CCC` 两段
    assert!(
        codes.contains(&"AAA".to_string()),
        "第一段该留下：{codes:?}"
    );
    assert!(
        !codes.contains(&"BBB".to_string()),
        "`无随机` 同一行后面的那一段必须被删掉：{codes:?}"
    );
}

/// 格式 1 与同一行切分**同时命中**时两条都要（`live-no-random-with-head` 的契约）。
#[test]
fn format_one_and_same_line_split_both_contribute() {
    assert_eq!(
        extract_kouling("小红花口令：AAA 无随机"),
        ["AAA 无随机", "AAA"]
    );
}

// ══════════════════════════ 疑问句过滤 ══════════════════════════

/// ⭐ 疑问句过滤的两个方向都要钉：该滤的滤掉，**不该滤的不能误伤**。
#[test]
fn question_filter_both_directions() {
    // 含问号（半角与全角）
    assert!(is_question("什么?"));
    assert!(is_question("什么？"));
    // 去语气词后整条等于疑问词
    assert!(is_question("多少"));
    assert!(is_question("多少啊"));
    assert!(is_question("啥呢"));
    assert!(is_question("几"));
    // ⚠ 反向边界：真口令不能误伤
    assert!(!is_question("带着云盘去上学"));
    assert!(!is_question("相伴什么"));
    assert!(!is_question("云盘AI高效复盘"));
    // 空串不是疑问句（不是"什么"）
    assert!(!is_question(""));
}

/// 疑问句过滤发生在**去重之后**，且不改变剩余项的顺序。
///
/// # ⚠ 一个反直觉的结果：多行时 `口令是(.+)` **只吃本行**
///
/// JS 的 `.` 不跨行，所以 `口令是啥\n口令是云盘AI高效复盘` 里的格式 3 分支
/// 捕获到的是 **`啥`**（第一行冒号后那一个字符），而不是第二行那句真口令。
/// `啥` 是疑问词 ⇒ 被滤掉 ⇒ **整条结果是空的**。
///
/// 原 TS 真跑确认过就是 `[]`。这条同时钉住两件事：
/// ① 疑问句过滤真的在生效；② 格式 3 **不是**"跨行取下一句"。
#[test]
fn filter_preserves_order_and_filters_questions() {
    assert_eq!(
        extract_kouling("口令是啥\n口令是云盘AI高效复盘"),
        Vec::<String>::new(),
        "格式 3 只吃本行（`.` 不跨行）⇒ 捕到的是 `啥` ⇒ 被疑问句过滤掉"
    );
    // 同一行里"先疑问、后真口令"：疑问的那条被滤掉，真的留下
    let codes = extract_kouling("口令是啥\n小红花口令：云盘AI高效复盘");
    assert_eq!(codes, ["云盘AI高效复盘"], "疑问句丢了，真口令必须在");
    // 顺序：多条真口令时保持出现顺序
    assert_eq!(
        extract_kouling("口令是B\n口令是A"),
        ["B"],
        "`口令是(.+)` 只取第一处匹配（非全局）⇒ 只有 B"
    );
}

// ══════════════════════════ 去重 ══════════════════════════

/// ⭐ 保序去重：同一个口令只留**第一次**出现的位置。
///
/// 顺序有实际影响：调用方按顺序逐个投口令，重复项会让同一条口令被投两次
/// （第二次回 `200112`，是白跑一趟）。
#[test]
fn dedupe_keeps_first_occurrence_order() {
    assert_eq!(extract_kouling("小红花口令：AAA\n小红花口令：AAA"), ["AAA"]);
    // B 在 A 之前出现 ⇒ 结果里 B 也在前
    let codes = extract_kouling("小红花口令：B\n小红花口令：A\n小红花口令：B");
    assert_eq!(codes, ["B", "A"]);
}

// ══════════════════════════ 小红书：四级候选与实体解码 ══════════════════════════

/// ⭐ **内嵌 JSON 优先于文档顺序** —— 这一条与直觉相反。
///
/// HTML 里 `<div>` 出现在 `<script>` **之前**，但候选收集顺序是
/// "内嵌 JSON → 元素 → meta → 整页逐行"，所以解出的是 JSON 里那一条。
/// 黄金向量的 `html-entities-and-json-escape` 记着这件事曾经的教训：
/// 手写期望值的第一版以为取 div，与真实现不符。
#[test]
fn embedded_json_wins_over_document_order() {
    let html = r#"<html><body><div>小红花口令：&#35270;&#39057;&#20307;&#39564;</div><script>{"desc":"口令是A\nB"}</script></body></html>"#;
    // JSON 里 `\n` 被折成空格 ⇒ `A B`
    assert_eq!(extract_kouling_from_xiaohongshu_html(html), ["A B"]);
}

/// 四级候选各自单独可用（前一级没有就不挡后一级）。
#[test]
fn each_candidate_level_works_alone() {
    // ① 内嵌 JSON
    assert_eq!(
        extract_kouling_from_xiaohongshu_html(
            r#"<script>{"displayTitle":"小红花口令：来自JSON"}</script>"#
        ),
        ["来自JSON"]
    );
    // ② 服务端渲染的元素
    assert_eq!(
        extract_kouling_from_xiaohongshu_html("<div class=\"note\">口令是云盘AI高效复盘</div>"),
        ["云盘AI高效复盘"]
    );
    // ③ meta
    assert_eq!(
        extract_kouling_from_xiaohongshu_html(
            r#"<meta name="description" content="小红花口令：元数据里的口令" />"#
        ),
        ["元数据里的口令"]
    );
    // ④ 整页纯文本逐行
    assert_eq!(
        extract_kouling_from_xiaohongshu_html("<span>小红花口令：兜底那一层</span>"),
        ["兜底那一层"]
    );
}

/// ⭐ 内嵌 JSON **第一条命中即返回** —— 主页是时间倒序，不能取到历史口令。
///
/// 历史口令一律回 `200108`：除了白跑一趟，还会让"无效口令 ⇒ 重听一次"
/// 这条逻辑被自己触发的噪声带偏。
#[test]
fn first_json_hit_wins_even_when_later_notes_have_codes() {
    let html = r#"<script>window.__INITIAL_STATE__={"noteList":[
      {"displayTitle":"小红花口令：最新一期"},
      {"displayTitle":"小红花口令：上一期"}]}</script>"#;
    assert_eq!(extract_kouling_from_xiaohongshu_html(html), ["最新一期"]);
}

/// HTML 实体解码：数字、hex、六个具名；认不出的**原样保留**。
#[test]
fn html_entities_decode_only_the_six_named_ones() {
    assert_eq!(decode_html_entities("&#35270;"), "视");
    assert_eq!(decode_html_entities("&#x89c6;"), "视");
    assert_eq!(decode_html_entities("&#X89C6;"), "视", "hex 大小写都认");
    assert_eq!(
        decode_html_entities("&amp;&lt;&gt;&quot;&apos;&nbsp;"),
        "&<>\"' "
    );
    assert_eq!(decode_html_entities("&AMP;"), "&", "具名实体大小写不敏感");
    // ⚠ 只有六个：多认几个是**行为变化**（原实现会把 `&copy;` 原样留在文本里）
    assert_eq!(decode_html_entities("&copy;"), "&copy;");
    assert_eq!(decode_html_entities("&nosuch;"), "&nosuch;");
    // 没有实体时原样返回
    assert_eq!(decode_html_entities("没有实体"), "没有实体");
}

/// `html_to_text` 的四步：`<br>` → 换行、块结束 → 换行、标签 → 空格、再压空白。
#[test]
fn html_to_text_collapses_tags_but_keeps_line_breaks() {
    assert_eq!(html_to_text("<div>甲<br>乙</div>"), "甲\n乙");
    assert_eq!(html_to_text("<p>甲</p><p>乙</p>"), "甲\n乙");
    // 标签之间压成一个空格（不是删掉 —— 删掉会把两个词粘成一个）
    assert_eq!(html_to_text("<b>甲</b><b>乙</b>"), "甲 乙");
    assert_eq!(html_to_text("  <div>  甲  </div>  "), "甲");
}

/// JSON 字符串解码：正确路径走 `serde_json`，解不开才手工替换。
#[test]
fn json_string_decoding_prefers_the_real_parser() {
    assert_eq!(decode_json_string(r"A\nB"), "A\nB");
    // 内嵌 JSON 里的转义引号：`\"` 要解成一个 `"`
    assert_eq!(decode_json_string("quote\\\"inside"), "quote\"inside");
    assert_eq!(decode_json_string("中文"), "中文");
    // 手工兜底路径（非法转义 ⇒ JSON 解不开）
    assert_eq!(decode_json_string(r"\n\t\r"), "\n\t\r");
}

/// 候选文本**不含关键词**时直接丢掉（`has_kouling` 是过滤器）。
#[test]
fn candidates_without_keywords_are_dropped() {
    assert!(has_kouling("小红花口令：X"));
    assert!(has_kouling("小红花"));
    assert!(!has_kouling("今天天气不错"));
    assert!(!has_kouling(""));
}

/// `has_kouling` 比提取器**宽得多** —— 宽了只是多算几次，窄了会漏。
#[test]
fn has_kouling_is_deliberately_loose() {
    // 这些解不出口令，但**含关键词** ⇒ 必须被放行到提取器（提取器自己会给出空）
    for text in ["口令", "没有随机口令", "只有这一个口令", "口令兑换不了"] {
        assert!(has_kouling(text), "{text} 含关键词，过滤器不该拦它");
        assert!(extract_kouling(text).is_empty(), "{text} 解不出东西");
    }
}

// ══════════════════════════ 小红书请求构造 ══════════════════════════

/// 主页请求：GET、带桌面 Chrome 伪装头。
///
/// ⚠ UA 与 referer 是**故意的伪装**：小红书对非常规 UA 会回一个空壳页面，
/// 而"空壳页面"与"这期没有口令"在解析层看起来一模一样（都返回 `[]`）。
#[test]
fn xiaohongshu_request_masquerades_as_desktop_chrome() {
    let req = xiaohongshu_profile_request();
    assert_eq!(req.method, "GET");
    assert_eq!(req.url, XIAOHONGSHU_PROFILE_URL);
    assert!(req.url.starts_with("https://www.xiaohongshu.com/"));
    assert!(req.body.is_none(), "拉主页是裸 GET");

    let get = |k: &str| {
        req.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("缺头 {k}"))
    };
    assert!(
        get("User-Agent").contains("Chrome/128"),
        "必须伪装成桌面 Chrome"
    );
    assert!(
        !get("User-Agent").contains("MCloudApp"),
        "别把 App 的 UA 用到这里"
    );
    assert_eq!(get("Referer"), "https://www.xiaohongshu.com/");
    assert!(get("Accept").contains("text/html"));
    assert_eq!(get("Accept-Language"), "zh-CN,zh;q=0.9");
}

// ══════════════════════════ 假阳性契约 ══════════════════════════

/// ⭐⭐ 已知假阳性是**行为契约**，不是待修 bug。
///
/// `口令是相伴什么` 会被解成 `相伴什么`（"相伴什么"不在疑问词表里，
/// 而 `口令是(.+)` 照单全收）。**不要"改进"它**：
///
/// - 口令是服务端当场校验的，解错的代价是 `200108`（实拍证明**零消耗**）+ 一次重试；
/// - 而"顺手写严"会把真口令滤掉，那一侧没有第二次机会（口令只在直播那一小时有效）。
///
/// 黄金向量 `live-false-positive` 把它钉住了，这里再单独标一条，
/// 让改这条行为的人**必须先读到这里**。
#[test]
fn false_positive_is_a_deliberate_contract() {
    assert_eq!(
        extract_kouling("口令是相伴什么"),
        ["相伴什么"],
        "这是刻意的假阳性契约（见模块头）。改它之前先读 test/golden 的 live-false-positive"
    );
    // 与它成对的"真疑问句"必须仍然被滤掉（否则这条契约就变成了漏勺）
    assert!(extract_kouling("口令是什么").is_empty());
    assert!(extract_kouling("口令是什么？").is_empty());
}
