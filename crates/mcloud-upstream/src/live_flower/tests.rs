//! 小红花六步链的协议测试。
//!
//! 判据来源优先级（本项目一贯口径）：**实拍黄金向量 > 原实现源码 > 直觉**。
//!
//! 本文件的响应与请求期望值全部取自 `test/golden/live-redflower-vectors.json`
//! （Reqable 抓包，2026-09-23 16:27–16:40），不是编的。

use super::*;

fn vectors() -> Value {
    // 用 `CARGO_MANIFEST_DIR` 而不是相对 `cwd`：测试运行的 cwd 不保证是
    // crate 根，相对路径会让这条测试在不同调用方式下时灵时不灵。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/live-redflower-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

/// 把向量里那段 `raw`（本来就是对象）序列化成响应文本。
fn raw_body(v: &Value, key: &str) -> String {
    serde_json::to_string(&v[key]["raw"]).expect("向量应可序列化")
}

/// 大写无关地取一个头（HTTP 头名大小写不敏感）。
fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// ══════════════════════════ ⭐ body 形态（硬约束 1）══════════════════════════

/// ⭐⭐ `give` 与 `draw` 的 body 必须是 **`None`、且不带 `content-type`**。
///
/// # 为什么这条值得单独一个测试
///
/// 实拍（§5.1）钉住四种请求的 body 形态**各不相同**：
///
/// ```text
/// user/info              content-type: application/json                  body {}
/// visitlog/journaling    content-type: application/x-www-form-urlencoded body module=uservisit&…
/// firstParticipate/give  **无 content-type**（content-length: 0）          body 空
/// redFlower/draw         **无 content-type**（content-length: 0）          body 空
/// ```
///
/// ⚠ `give` 与 `draw` 走 [`HttpRequest::json_body`] 会补一个
/// `content-type: application/json` **并且**发出一个 `{}`（`content-length: 2`）。
/// 多发两个字节**通常无害** —— 正因为无害，将来有人"顺手统一成 json_body"
/// 时不会有任何测试变红，也没有任何线上症状。所以这条必须显式钉住。
#[test]
fn give_and_draw_have_no_body_and_no_content_type() {
    for (name, req) in [
        ("firstParticipate/give", first_participate_give_request()),
        ("redFlower/draw", draw_request("中秋美好存云盘")),
    ] {
        assert!(
            req.body.is_none(),
            "{name} 的 body 必须是 None（实拍 content-length: 0）"
        );
        assert_eq!(
            req.body_str(),
            None,
            "{name} 不该发出任何 body —— 尤其不该是 `{{}}`"
        );
        assert!(
            header(&req, "content-type").is_none(),
            "{name} 不该带 content-type（实拍就没有）。实得：{:?}",
            header(&req, "content-type")
        );
    }
}

/// ⚠ form 体的 `content-type` 必须**显式**声明 —— 它是唯一能压过
/// `client.rs` 那个"有 body 就补 `application/json`"的机制。
///
/// # 实测发现的一处缺陷（不在本任务的写入范围内）
///
/// [`crate::client::ReqwestClient`] 在 `body.is_some()` 时**无条件**补一个
/// `content-type: application/json;charset=UTF-8`，且不去重。
/// 实测（把 `journaling` 的请求真的发到本地监听端口）上线的是**两个**头：
///
/// ```text
/// content-type: application/x-www-form-urlencoded;charset=UTF-8
/// content-type: application/json;charset=UTF-8
/// ```
///
/// 本模块**显式**声明 form 的 content-type 就是为了让**第一个**头存在
/// （否则只剩 JSON，form 体会被服务端按 JSON 解 ⇒ 埋点静默失效）。
/// 但第二个头消不掉 —— 那要改 `client.rs`（**不是本任务的写入范围**），
/// 已如实报给 Lead。
///
/// 这条测试钉住的是**本模块能控制的那一半**：请求对象上必须有 form 头。
#[test]
fn journaling_content_type_is_declared_explicitly() {
    let j = journaling_request("e");
    assert_eq!(
        header(&j, "content-type"),
        Some("application/x-www-form-urlencoded;charset=UTF-8"),
        "没有它的话，client.rs 会把 form 体标成 application/json"
    );
    assert!(j.body.is_some(), "埋点是有 body 的");
}

/// ⭐ 另外两个请求的 body 形态**也是照实拍来的**，别被上一条带偏成"两个极端"。
///
/// 四条请求三种形态，缺一条都会让"统一成 JSON"看起来像是对的。
///
/// # ⚠ `user/info` 的 `content-type` 是**上线时**才补的，不在请求对象上
///
/// [`HttpRequest::json_body`] 只写 `body`，**不写** `content-type`；
/// 那个头由 [`crate::client::ReqwestClient`] 在 `body` 为 `Some` 时补
/// `application/json;charset=UTF-8`。所以这里断言的是 **body 的形状**，
/// 而不是"请求对象里有没有那个头"—— 后者会是一条**假的不变量**。
///
/// 代价是：这一条**单独看**证明不了上线时 `user/info` 真的是 JSON
/// —— 真正的证明在下一条测试（它钉住"form 体不该被当成 JSON"）与
/// `client.rs` 的实现上。
#[test]
fn user_info_and_journaling_keep_their_own_body_shapes() {
    // ① user/info：JSON 空对象 `{}`（实拍 content-length: 2）
    let info = user_info_request();
    assert_eq!(info.body_str(), Some("{}"), "body 必须是空对象");

    // ② journaling：form-urlencoded、**不是** JSON
    let j = journaling_request("National_LiveRoomFeedback_pv");
    let body = j.body_str().expect("埋点有 body");
    assert!(
        !body.starts_with('{'),
        "form 体不是 JSON，别用 json_body：{body}"
    );
    // ⚠ form 体必须**显式**声明它的 content-type：那是唯一能压过
    //   `client.rs` 那个"有 body 就补 application/json"的机制。
    //   实测（本地监听端口）这一条会让**两个** content-type 同时上线，
    //   见 `journaling_content_type_is_declared_explicitly` 的说明。
    assert_eq!(
        header(&j, "content-type"),
        Some("application/x-www-form-urlencoded;charset=UTF-8"),
        "form 体必须显式声明 content-type，否则上线时会被补成 application/json"
    );
    assert_eq!(
        body,
        "module=uservisit&optkeyword=National_LiveRoomFeedback_pv\
         &sourceid=1007&marketName=National_LiveRoomFeedback",
        "参数顺序照抄原项目，下一次抓包才能逐字节比"
    );
}

/// 埋点是 5 条、顺序固定，且每条都真的能构造出请求。
#[test]
fn journaling_events_are_the_five_from_the_reference() {
    assert_eq!(JOURNALING_EVENTS.len(), 5);
    assert_eq!(JOURNALING_EVENTS[0], "National_LiveRoomFeedback_pv");
    assert_eq!(JOURNALING_EVENTS[4], "National_LiveRoomFeedback_flowerBtn");
    // 每条的请求都落在同一个路径上（别把某一条发到别处去）
    for ev in JOURNALING_EVENTS {
        let req = journaling_request(ev);
        assert!(req.url.ends_with(JOURNALING_PATH), "实得：{}", req.url);
        assert!(req.body_str().unwrap().contains(ev));
    }
}

/// 业务码**只判等、不判形状**：`609` 这种三位码也是"别的码"，不是"口令无效"。
///
/// 原项目坑表 #106 那一族：`2301` 是四位、其余三位 ⇒ 别按"都是 2001xx"去推。
#[test]
fn codes_are_compared_exactly_never_by_shape() {
    let mk = |code: i64| {
        classify_red_flower_draw(&RedFlowerEnvelope::parse(&format!(
            r#"{{"code":{code},"msg":"x"}}"#
        )))
    };
    for code in [
        609, 200101, 200103, 200107, 200109, 200111, 200114, 200115, 200130, 2301,
    ] {
        let out = mk(code);
        assert_eq!(
            out.kind,
            RedFlowerDrawKind::Other,
            "code {code} 没实拍 ⇒ 只能落进 Other，别按形状猜"
        );
        assert!(!out.rejected_kouling(), "code {code} 不该被判成无效口令");
    }
    // 只判等：200118 不是 200188、200112 不是 200113
    assert_eq!(mk(200118).kind, RedFlowerDrawKind::KoulingExpired);
    assert_eq!(mk(200188).kind, RedFlowerDrawKind::Other);
    assert_eq!(mk(200112).kind, RedFlowerDrawKind::AlreadyDrawn);
    assert_eq!(mk(200113).kind, RedFlowerDrawKind::Other);
}

/// 这一族共用的头必须带齐（黄金向量 `draw.expectHeaders` 的四条 + 参考实现那两条）。
#[test]
fn requests_carry_the_live_room_header_set() {
    let req = draw_request("x");
    assert_eq!(header(&req, "activityid"), Some("sign_in_3"));
    assert_eq!(header(&req, "appversion"), Some("12.6.0.0"));
    assert_eq!(header(&req, "origin"), Some("https://m.mcloud.139.com"));
    assert_eq!(header(&req, "user-agent"), Some(LIVE_ROOM_USER_AGENT));
    assert_eq!(header(&req, "Accept"), Some("*/*"));
    assert_eq!(
        header(&req, "referer"),
        Some(crate::live_consts::live_room_page_referer().as_str())
    );
}

/// ⚠ `activityid` 是**签到族那个** `sign_in_3`，不是直播专属值。
///
/// 小红花在 `/ycloud` 下与签到同域，请求头里带的就是签到那个 activityid（实拍）。
/// 别"顺手改成直播侧的某个 id"——那不会报错，只会让服务端认不出这是哪个活动。
#[test]
fn activity_id_is_the_signin_one_not_a_live_specific_value() {
    assert_eq!(LIVE_ROOM_ACTIVITY_ID, "sign_in_3");
    assert_eq!(
        header(&user_info_request(), "activityid"),
        Some("sign_in_3")
    );
}

/// ⭐ 构造器**不许**自己带 `jwttoken` / `referer` 之外的会话凭据，也不许带 `jwttoken` 本身。
///
/// 与 `app_signin` 同一条纪律：那两片是**会话的两半**，由 `H5Client` 从同一次
/// 建会话里取。构造器各拼一遍的典型后果是"票是对的、referer 差一个参数"，
/// 而表现是 **90001 未登录** —— 那个码通常被读成"票过期了"，排查方向整个跑偏。
///
/// ⚠ `Referer` **要**带（它是页面 URL，实拍里有），但 `jwttoken` 与
/// `authorization` **不能**带。
#[test]
fn builders_do_not_set_session_bound_credentials() {
    for req in [
        user_info_request(),
        journaling_request("e"),
        first_participate_give_request(),
        draw_request("c"),
        query_revival_flowers_request(),
    ] {
        for bound in ["jwttoken", "authorization", "cookie"] {
            assert!(
                header(&req, bound).is_none(),
                "{bound} 该由 H5Client 注入，构造器不该带：{}",
                req.url
            );
        }
    }
}

/// `Origin` 与直播侧的 `LIVE_ORIGIN` **不是一个值** —— 别"顺手统一"。
#[test]
fn origin_is_the_mcloud_host_not_the_live_host() {
    assert_eq!(LIVE_ROOM_ORIGIN, "https://m.mcloud.139.com");
    assert_ne!(
        LIVE_ROOM_ORIGIN,
        crate::live_consts::LIVE_ORIGIN,
        "小红花在 m.mcloud.139.com 上，直播侧在 v.cmcc-cs.cn —— 两个 Origin 不同是刻意的"
    );
}

// ══════════════════════════ draw 的请求构造 ══════════════════════════

/// ⭐ 对拍黄金向量的 `draw.expectQuery`：口令走 query **且必须 URL 编码**。
///
/// 中文不编码的话，服务端解出来的是一条坏口令，而它只会回 `200108`
/// —— 看起来像"口令抄错了"，排查方向一开始就错。
#[test]
fn draw_query_matches_the_golden_expectation() {
    let v = vectors();
    let code = v["draw"]["code"].as_str().expect("向量应有 code");
    let expect = v["draw"]["expectQuery"]
        .as_str()
        .expect("向量应有 expectQuery");

    let req = draw_request(code);
    let got = req.url.split_once('?').expect("必须是带 query 的 URL").1;
    assert_eq!(format!("?{got}"), expect, "query 与实拍不一致");

    // 逐字再钉一次那个编码结果（防有人把 percent_encode 换成"更宽松"的实现）
    assert_eq!(
        req.url,
        format!("{BASE_LIVE_ROOM_FEEDBACK}{DRAW_PATH}{expect}"),
        "实得：{}",
        req.url
    );
    // 口令是中文 ⇒ 一定全是 %XX，URL 里不该出现原字符
    assert!(!req.url.contains(code), "中文口令必须编码：{}", req.url);
}

/// ⭐ 对拍黄金向量的 `draw.expectReferer` 与 `draw.expectHeaders`。
#[test]
fn draw_referer_and_headers_match_the_golden_expectation() {
    let v = vectors();
    let expect_referer = v["draw"]["expectReferer"].as_str().unwrap();
    let headers = v["draw"]["expectHeaders"].as_object().unwrap();

    let req = draw_request(v["draw"]["code"].as_str().unwrap());
    assert_eq!(header(&req, "referer"), Some(expect_referer));

    for (k, expected) in headers {
        assert_eq!(header(&req, k), expected.as_str(), "头 `{k}` 与实拍不一致");
    }
    // 向量的四条第 5 条判据（`referer` 不在 expectHeaders 里）也要在
    assert_eq!(headers.len(), 4, "黄金向量的 expectHeaders 条数变了");
}

/// ⭐ `Referer` 逐字来自 `live_consts`，且**不带** `token=`。
///
/// 实拍原串里有一段 `token=<JWT>`。纯协议实现里凭据走 `jwttoken` **头**
/// （实拍也确实是这么带的），所以刻意把 `token` 去掉：
/// 一个会过期的凭据拼进 URL，只会让日志与错误信息里多一处凭据。
#[test]
fn referer_comes_from_live_consts_without_the_token() {
    let r = header(&draw_request("x"), "referer").unwrap().to_string();
    assert!(r.contains("path=National_LiveRoomFeedback"), "实得：{r}");
    assert!(r.contains("sourceid=1007"), "实得：{r}");
    assert!(r.contains("targetSourceId=001005"), "实得：{r}");
    assert!(!r.contains("token="), "刻意不带会过期的凭据：{r}");
    // ⚠ 用的是**小红花侧**的 sourceId，不是直播侧的 017027
    assert!(!r.contains("017027"));
}

/// ⭐ `percent_encode` 对会破坏 query 语义的字符必须转义。
///
/// `+` 在 query 里表示空格 ⇒ 不编码会被服务端解成另一个口令，
/// 而它看起来"没报错"，只是抽不到。
#[test]
fn percent_encoding_covers_query_breaking_characters() {
    assert_eq!(percent_encode("a+b"), "a%2Bb");
    assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
    // query 里空格用 `%20`（**不是** `+`；`+` 留给"字面量加号"）
    assert_eq!(percent_encode(" "), "%20");
    // 字母数字与 `-_.~` 原样保留（与抓包一致）
    assert_eq!(percent_encode("Az09-_.~"), "Az09-_.~");
    assert_eq!(percent_encode("中文"), "%E4%B8%AD%E6%96%87");
}

/// ⚠ form 编码的空格是 `+`，与 query 的 `%20` **刻意不同**。
///
/// 两者混用不会报错：服务端会把 form 体里一个字面量 `+` 解成 `+`，
/// 于是埋点关键词静默多一个加号。
#[test]
fn form_encoding_uses_plus_for_space_unlike_query_encoding() {
    let req = journaling_request("a b&c=d");
    let body = req.body_str().unwrap();
    assert!(
        body.contains("optkeyword=a+b%26c%3Dd"),
        "form 体的空格该是 `+`：{body}"
    );
    assert!(!body.contains("%20"), "form 体不该出现 %20：{body}");
    // 而 query 编码用的是 %20
    assert_eq!(percent_encode("a b"), "a%20b");
    assert_ne!(percent_encode(" "), "+");
}

// ══════════════════════════ draw 的响应判定（黄金向量）══════════════════════════

/// ⭐⭐ `drawResponses` 3 条**逐条**对拍：`expectKind` / `expectCode` /
/// `expectMessage` / `expectFlowerNum` / `expectPrizeName` / `expectPrizeId` /
/// `expectPrizeType` / **`expectRejectedKouling`**。
///
/// # 为什么连 `expectRejectedKouling` 也逐条断言
///
/// 它是这个向量段**最有价值的一个字段**：三条里只有 `200108` 是 `true`，
/// 而 `200112`（已兑换）与成功都是 `false`。只看 `kind` 的话，
/// 一个"把 `200112` 也算成无效口令"的实现会**照样通过** ——
/// 而那会把一个**有效**口令从候选里剔掉，让同一批账号里后面的那几个白跑一趟。
#[test]
fn golden_draw_responses_all_pass() {
    let v = vectors();
    let cases = v["drawResponses"]
        .as_array()
        .expect("drawResponses 应为数组");
    assert_eq!(cases.len(), 3, "黄金向量条数变了 —— 先确认是不是被改动过");

    for case in cases {
        let name = case["name"].as_str().expect("用例应有 name");
        let env = RedFlowerEnvelope::parse(&serde_json::to_string(&case["raw"]).unwrap());
        let out = classify_red_flower_draw(&env);

        assert_eq!(
            out.kind.as_str(),
            case["expectKind"].as_str().unwrap(),
            "黄金向量 `{name}` 的 kind 不一致"
        );
        assert_eq!(
            out.code,
            case["expectCode"].as_i64(),
            "黄金向量 `{name}` 的 code 不一致"
        );
        assert_eq!(
            out.message,
            case["expectMessage"].as_str().unwrap(),
            "黄金向量 `{name}` 的 msg 不一致"
        );
        assert_eq!(
            out.rejected_kouling(),
            case["expectRejectedKouling"].as_bool().unwrap(),
            "黄金向量 `{name}` 的 expectRejectedKouling 不一致"
        );

        // 成功那一档才有 result
        if let Some(n) = case.get("expectFlowerNum").and_then(Value::as_i64) {
            assert!(out.is_success(), "`{name}` 该是成功");
            assert_eq!(out.result.flower_num, n, "`{name}` 的 flowerNum");
        }
        if let Some(n) = case.get("expectPrizeName").and_then(Value::as_str) {
            assert_eq!(out.result.prize_name, n, "`{name}` 的 prizeName");
        }
        if let Some(n) = case.get("expectPrizeId").and_then(Value::as_i64) {
            assert_eq!(out.result.prize_id, n, "`{name}` 的 prizeId");
        }
        if let Some(n) = case.get("expectPrizeType").and_then(Value::as_i64) {
            assert_eq!(out.result.prize_type, n, "`{name}` 的 prizeType");
        }
    }
}

/// ⭐ 正向：`200112`（已兑换）**不算失败、更不算无效口令**。
///
/// 口令本身是**对的**（错的回 `200108`）。把它算成无效口令，
/// 会把一个有效口令从候选里剔掉 —— 代价在同一批账号的**后几个**上，
/// 而那时的现象是"某几个号突然抽不到了"，看不出跟这里有关。
#[test]
fn already_drawn_is_neither_a_failure_nor_an_invalid_kouling() {
    let env = RedFlowerEnvelope::parse(
        r#"{"code":200112,"msg":"口令已兑换过了，每个口令只能兑换一次!"}"#,
    );
    let out = classify_red_flower_draw(&env);

    assert_eq!(out.kind, RedFlowerDrawKind::AlreadyDrawn);
    assert!(out.is_already_drawn());
    assert!(!out.is_success(), "它不是一次成功（没有进账）");
    assert!(
        !out.rejected_kouling(),
        "口令是对的 ⇒ 绝不能被当成无效口令（否则会把有效口令剔掉）"
    );
    assert_eq!(out.code, Some(CODE_KOULING_ALREADY_DRAWN));
    assert_eq!(CODE_KOULING_ALREADY_DRAWN, 200112);
}

/// ⭐⭐ `200118`（口令已失效 / 本场已结束）与 `200108`（口令无效）是**两件事**。
///
/// # 为什么要分成两个 kind（Lead 专门强调，原项目坑表 #140）
///
/// 理由在**用户可见的那句话**上：`200118` 最常见的成因是"**直播已经结束了**"
/// —— 若把它归进"无效 / 未分类口令"，界面那句话会把人往"**口令抄错了**"
/// 那边引，排查方向一开始就错。
///
/// ⚠ 而且它**不触发重听**：口令本身是**对的**（错的回 `200108`），
/// 重听只会再拿一次 `200118`。所以 `rejected_kouling()` 必须是 **false**。
///
/// ⚠ 它与 `200108` 的差别是**实拍钉住**的：同账号、同样"直播已关"，
/// 用**真但已过期**的口令去领 ⇒ `200118`；用**乱码 / 不存在的口令** ⇒ `200108`
/// ⇒ 它是**口令级**的码（"我认识它、它失效了"），不是场次级。
#[test]
fn expired_kouling_is_distinct_from_invalid_kouling() {
    let expired = classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"code":200118}"#));
    let invalid = classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"code":200108}"#));

    // 两档必须**分得开**
    assert_eq!(expired.kind, RedFlowerDrawKind::KoulingExpired);
    assert_eq!(invalid.kind, RedFlowerDrawKind::InvalidKouling);
    assert_ne!(expired.kind, invalid.kind, "两者不能是同一档");
    assert_ne!(
        expired.kind.as_str(),
        invalid.kind.as_str(),
        "人读的输出也要分得开：一个说「本场已结束」，一个说「口令不对」"
    );

    // ⭐ 重听判据：只有 200108 触发
    assert!(
        !expired.rejected_kouling(),
        "口令是对的 ⇒ 不该触发重听（重听只会再拿一次 200118）"
    );
    assert!(invalid.rejected_kouling(), "200108 才是真的口令不对");

    // ⭐ 两者都**不是成功**
    assert!(!expired.is_success(), "200118 不许被判成成功");
    assert!(!invalid.is_success(), "200108 不许被判成成功");
    assert!(expired.is_kouling_expired());
    assert!(!invalid.is_kouling_expired());
    assert_eq!(expired.code, Some(CODE_KOULING_EXPIRED));
    assert_eq!(CODE_KOULING_EXPIRED, 200118);
}

/// ⭐ 成功必须**同时**满足 `code == 0` 与"有 `result`"。
///
/// 原项目的判据逐字是 `code === 0 && result`。只有 code 而没 result，
/// 说明拿不到"换到几朵" —— 那就不能记成一次成功
/// （否则余额对不上，却没有任何线索指向是哪一枪出的问题）。
#[test]
fn success_requires_both_zero_code_and_a_result() {
    let ok = classify_red_flower_draw(&RedFlowerEnvelope::parse(
        r#"{"code":0,"msg":"success","result":{"flowerNum":10}}"#,
    ));
    assert!(ok.is_success());
    assert_eq!(ok.result.flower_num, 10);

    let no_result =
        classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"code":0,"msg":"success"}"#));
    assert_eq!(no_result.kind, RedFlowerDrawKind::Unparsable);
    assert!(!no_result.is_success(), "没有 result 就不能记成成功");
    assert!(
        !no_result.rejected_kouling(),
        "响应没说口令不对 ⇒ 不能判无效（避免误伤）"
    );
}

/// ⭐⭐ **响应无法解析 → 不算无效口令**（`invalid_codes` 不该含它）。
///
/// 原项目那一支刻意"warn 之后跳过、**不判无效**"：响应解不开时我们
/// **不知道**口令对不对，把它当"错"的代价是丢掉一个可能是真的口令
/// —— 而口令只在直播那一小时有效，那一侧没有第二次机会。
#[test]
fn unparsable_responses_are_never_invalid_koulings() {
    // 完全不是 JSON（网关错误页 / HTML）
    let html = classify_red_flower_draw(&RedFlowerEnvelope::parse("<html>502 Bad Gateway</html>"));
    assert_eq!(html.kind, RedFlowerDrawKind::Unparsable);
    assert!(
        html.code.is_none(),
        "没有 code 字段 ⇒ code 是 None，不是哨兵值"
    );
    assert!(!html.rejected_kouling(), "解不开 ≠ 口令不对");
    assert!(!html.is_success());

    // 是 JSON，但没有 code 字段
    let no_code = classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"msg":"什么也没说"}"#));
    assert_eq!(no_code.kind, RedFlowerDrawKind::Unparsable);
    assert!(!no_code.rejected_kouling());

    // 空串
    let empty = classify_red_flower_draw(&RedFlowerEnvelope::parse(""));
    assert_eq!(empty.kind, RedFlowerDrawKind::Unparsable);
    assert!(!empty.rejected_kouling());
}

/// ⭐ `Unparsable` 与 `Other` **不是同一档**：一个说"上游没按信封说话"，
/// 一个说"上游说了个别的码"—— 排查方向完全不同。
#[test]
fn unparsable_and_other_are_separate_kinds() {
    let other = classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"code":200199}"#));
    let unparsable = classify_red_flower_draw(&RedFlowerEnvelope::parse("<html></html>"));
    assert_eq!(other.kind, RedFlowerDrawKind::Other);
    assert_eq!(unparsable.kind, RedFlowerDrawKind::Unparsable);
    assert_ne!(other.kind, unparsable.kind);
    assert_eq!(other.code, Some(200199), "Other 要保住原始码");
    assert_eq!(unparsable.code, None);
}

/// 非数字的 `code`（形状不对）也走 `Unparsable`，不 panic、不当成 `Other`。
#[test]
fn non_numeric_code_is_unparsable() {
    let out = classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"code":"abc"}"#));
    assert_eq!(out.kind, RedFlowerDrawKind::Unparsable);
    assert_eq!(out.code, None);
}

/// `200108` 的上游原文要如实带出来（实拍那句"需当场直播获取"）。
#[test]
fn invalid_kouling_keeps_the_upstream_message() {
    let v = vectors();
    let third = &v["drawResponses"][2];
    let out = classify_red_flower_draw(&RedFlowerEnvelope::parse(
        &serde_json::to_string(&third["raw"]).unwrap(),
    ));
    assert_eq!(out.kind, RedFlowerDrawKind::InvalidKouling);
    assert_eq!(out.message, third["expectMessage"].as_str().unwrap());
    assert!(
        out.message.contains("当场直播"),
        "上游原文要原样保留：{}",
        out.message
    );
}

// ══════════════════════════ give 的响应判定 ══════════════════════════

/// ⭐ `200110`（已参与过）**不是错误**，是"这次没有"。
///
/// 每个账号第二次跑这条链都会拿到它。把它当失败，会让一次完全正常的运行
/// 以一句"上游返回 code=200110"结束，读起来像故障。
#[test]
fn already_joined_is_normal_not_a_failure() {
    let out = classify_first_participate(&RedFlowerEnvelope::parse(
        r#"{"code":200110,"msg":"您已参与过活动，无法获得首次参与惊喜奖"}"#,
    ));
    assert_eq!(out.kind, FirstParticipateKind::AlreadyJoined);
    assert!(out.is_already_joined());
    assert!(!out.is_success(), "没拿到奖，不能记成进账");
    assert_eq!(out.code, Some(CODE_FIRST_PARTICIPATE_ALREADY));
    assert_eq!(CODE_FIRST_PARTICIPATE_ALREADY, 200110);
    assert!(out.message.contains("已参与过"), "原文要带出来（只进日志）");
}

/// `give` 成功时读 `result.flowerNum`（它是"这次进账多少"的一部分）。
#[test]
fn first_participate_success_reads_flower_num() {
    let out = classify_first_participate(&RedFlowerEnvelope::parse(
        r#"{"code":0,"msg":"success","result":{"flowerNum":5}}"#,
    ));
    assert_eq!(out.kind, FirstParticipateKind::Gained);
    assert!(out.is_success());
    assert_eq!(out.flower_num, 5);

    // 成功但没有 result ⇒ flowerNum 给 0，**不判失败**（奖励本身已经拿到了）
    let bare =
        classify_first_participate(&RedFlowerEnvelope::parse(r#"{"code":0,"msg":"success"}"#));
    assert_eq!(bare.kind, FirstParticipateKind::Gained);
    assert_eq!(bare.flower_num, 0);
}

/// ⭐ `give` 的**其它**非零码只记 info，**不当失败**（也不返回 `Err`）。
///
/// ⚠ 这只是"这一层不替编排层做决定"：本仓纪律是**禁止全链吞错**。
/// 把这一支放进报告的 `info` 里是编排层的选择，不是本层的默认。
#[test]
fn first_participate_other_codes_do_not_error_and_keep_the_code() {
    for code in [200101, 200111, 609, 500, 200103] {
        let out = classify_first_participate(&RedFlowerEnvelope::parse(&format!(
            r#"{{"code":{code},"msg":"x"}}"#
        )));
        assert_eq!(out.kind, FirstParticipateKind::Other, "code {code}");
        assert!(!out.is_success());
        assert!(!out.is_already_joined());
        assert_eq!(
            out.code,
            Some(code),
            "原始码必须原样带出来，否则日志里查不出是什么"
        );
    }
}

/// ⭐ `200110` 是 **`give` 的码，不是 `draw` 的码** —— 张冠李戴探针。
///
/// 这个码最容易犯的错就是拿它去判 `draw`。实拍里 `draw` 从来没回过 `200110`
/// ⇒ 它在 `draw` 的判定里只能落进 `Other`。
#[test]
fn give_code_must_not_be_used_to_judge_draw() {
    let out = classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"code":200110}"#));
    assert_eq!(
        out.kind,
        RedFlowerDrawKind::Other,
        "200110 出自 firstParticipate/give ⇒ 在 draw 里它没有任何特殊含义"
    );
    assert!(!out.rejected_kouling());
    assert!(!out.is_success());
    assert!(!out.is_already_drawn());
}

/// `give` 的 `Unparsable` 与 `Other` 同样要分开。
#[test]
fn first_participate_unparsable_is_its_own_kind() {
    let out = classify_first_participate(&RedFlowerEnvelope::parse("<html></html>"));
    assert_eq!(out.kind, FirstParticipateKind::Unparsable);
    assert_eq!(out.code, None);
    assert!(!out.is_success());
    assert!(!out.is_already_joined());
}

// ══════════════════════════ 余额（userInfo）══════════════════════════

/// ⭐ 对拍黄金向量的 `userInfo`：`flowerCnt == 245`、`redFlowerValidDate` 正确。
#[test]
fn golden_user_info_matches_and_reads_the_right_key() {
    let v = vectors();
    let env = RedFlowerEnvelope::parse(&raw_body(&v, "userInfo"));
    let balance = parse_red_flower_balance(&env).unwrap();

    assert_eq!(
        balance.flower_cnt,
        v["userInfo"]["expectFlowerCnt"].as_i64().unwrap()
    );
    assert_eq!(
        balance.flower_cnt, 245,
        "实拍：成功那一枪之后 245 = 基线 235 + 10"
    );
    assert_eq!(
        balance.red_flower_valid_date,
        v["userInfo"]["expectRedFlowerValidDate"].as_str().unwrap()
    );
    assert_eq!(balance.red_flower_valid_date, "2026年12月31日");
    assert_eq!(balance.is_quarter_drawed, 0);
    assert_eq!(balance.prize_id, 0);
    assert_eq!(balance.second_prize_id, 0);
    assert_eq!(balance.has_not_received_red_flower, 0);
}

/// ⭐ 余额读的是 **`result.flowerCnt`** —— 改别的键必须不影响它。
///
/// ⚠ 排行榜 `getRedFlowerTop10List` 里**也有**一个 `flowerCnt`，
/// 而那个是**别人的**（实拍第一名 2055）。两者同名不同义。
/// 本模块的做法是**根本不提供**那个接口 —— 不提供就不会被误用。
///
/// 这条测试用"篡改邻居字段"的方式再钉一次读的是哪个键。
#[test]
fn balance_reads_flower_cnt_and_ignores_neighbours() {
    let body = serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "flowerCnt": 245,
            // 排行榜那种"别人的数"混进来也不能影响余额
            "topFlowerCnt": 2055,
            "prizeId": 9, "secondPrizeId": 8,
            "isQuarterDrawed": 1, "hasNotReceivedRedFlower": 2,
            "redFlowerValidDate": "2026年12月31日"
        }
    });
    let b = parse_red_flower_balance(&RedFlowerEnvelope::parse(&body.to_string())).unwrap();
    assert_eq!(b.flower_cnt, 245, "读的必须是 result.flowerCnt");
    assert_ne!(b.flower_cnt, 2055, "2055 是排行榜第一名的数，不是我的余额");

    // 改掉 flowerCnt，解析结果必须跟着变
    let mut tampered = body.clone();
    tampered["result"]["flowerCnt"] = serde_json::json!(999);
    let t = parse_red_flower_balance(&RedFlowerEnvelope::parse(&tampered.to_string())).unwrap();
    assert_eq!(t.flower_cnt, 999);
}

/// ⭐ `flowerCnt` 是**必填**（主判据），缺了要报错。
///
/// 静默成 `0` 会让"余额 235 → 0"看起来像**一次真实的清零**
/// —— 而那正是最该立刻报警的形状。其余五个字段缺了**不**报错。
#[test]
fn balance_requires_flower_cnt_but_not_the_others() {
    let missing = RedFlowerEnvelope::parse(r#"{"code":0,"msg":"success","result":{}}"#);
    let err = parse_red_flower_balance(&missing).unwrap_err();
    assert!(err.to_string().contains("flowerCnt"), "实得：{err}");

    // 其余五个全缺 ⇒ 只留空，不判失败
    let bare = RedFlowerEnvelope::parse(r#"{"code":0,"msg":"success","result":{"flowerCnt":7}}"#);
    let b = parse_red_flower_balance(&bare).unwrap();
    assert_eq!(b.flower_cnt, 7);
    assert_eq!(b.red_flower_valid_date, "");
    assert_eq!(b.has_not_received_red_flower, 0);
    assert_eq!(b.is_quarter_drawed, 0);
    assert_eq!(b.prize_id, 0);
    assert_eq!(b.second_prize_id, 0);
}

/// 余额这一族允许**同一枪取两次**做前后对比（① 与 ⑥ 是同一个接口）。
#[test]
fn balance_before_and_after_use_the_same_endpoint() {
    assert_eq!(USER_INFO_PATH, "/liveRoomFeedback/user/info");
    let a = user_info_request();
    let b = user_info_request();
    assert_eq!(a.url, b.url, "①与⑥必须打同一个端点（前后相减才有意义）");
    assert_eq!(a.method, "POST");
}

/// `flowerCnt` 是数字字符串时也收（上游有的字段是字符串形式的数字）。
#[test]
fn balance_accepts_numeric_strings() {
    let b = parse_red_flower_balance(&RedFlowerEnvelope::parse(
        r#"{"code":0,"result":{"flowerCnt":"245"}}"#,
    ))
    .unwrap();
    assert_eq!(b.flower_cnt, 245);
}

// ══════════════════════════ 复活花 ══════════════════════════

/// 复活花是**裸数字**，且它是 GET。
///
/// ⚠ 它**不是**小红花余额 —— 别拿它当余额。
#[test]
fn revival_flowers_is_a_bare_number_and_a_get() {
    let req = query_revival_flowers_request();
    assert_eq!(req.method, "GET", "这条链上唯一的 GET");
    assert!(req.body.is_none());
    assert!(req.url.ends_with(QUERY_REVIVAL_FLOWERS_PATH));

    let n = parse_revival_flowers(&RedFlowerEnvelope::parse(
        r#"{"code":0,"msg":"success","result":0}"#,
    ))
    .unwrap();
    assert_eq!(n, 0);
    // 数字字符串也收
    let s = parse_revival_flowers(&RedFlowerEnvelope::parse(
        r#"{"code":0,"msg":"success","result":"3"}"#,
    ))
    .unwrap();
    assert_eq!(s, 3);
    // ⚠ 它是对象时要**报错**，不能静默当 0 —— 那会与"真的是 0"混成一个形状
    assert!(parse_revival_flowers(&RedFlowerEnvelope::parse(
        r#"{"code":0,"result":{"revivalFlowers":3}}"#
    ))
    .is_err());
    // 非 0 码是失败
    assert!(parse_revival_flowers(&RedFlowerEnvelope::parse(
        r#"{"code":500,"msg":"系统繁忙"}"#
    ))
    .is_err());
}

/// 复活花与余额是**两个数**，别混用。
#[test]
fn revival_flowers_is_not_the_balance() {
    let env = RedFlowerEnvelope::parse(r#"{"code":0,"msg":"success","result":0}"#);
    let revival = parse_revival_flowers(&env).unwrap();
    // 同一个响应拿去当余额解 ⇒ 必须失败（`result` 是裸数字，不是对象）
    assert!(
        parse_red_flower_balance(&env).is_err(),
        "裸数字的 result 不是余额信封 —— 这两个接口不能互换"
    );
    assert_eq!(revival, 0);
}

// ══════════════════════════ 信封语义 ══════════════════════════

/// `msg` 与 `message` 两个键名都收。
#[test]
fn envelope_accepts_both_message_keys() {
    assert_eq!(
        RedFlowerEnvelope::parse(r#"{"code":1,"msg":"用 msg"}"#).message,
        "用 msg"
    );
    assert_eq!(
        RedFlowerEnvelope::parse(r#"{"code":1,"message":"用 message"}"#).message,
        "用 message"
    );
}

/// `code` 是数字字符串时也认（两种都收），但**语义仍按数字判**。
#[test]
fn envelope_accepts_string_code() {
    let env = RedFlowerEnvelope::parse(r#"{"code":"200112"}"#);
    assert_eq!(env.code, Some(200112));
    assert_eq!(
        classify_red_flower_draw(&env).kind,
        RedFlowerDrawKind::AlreadyDrawn,
        "字符串码也要走同一条判定"
    );
}

/// 非 0 的 code 必须变成**错误**，且把上游原文与是哪一步都带上。
#[test]
fn ensure_ok_names_the_step_and_the_upstream_text() {
    let env = RedFlowerEnvelope::parse(r#"{"code":500,"msg":"系统繁忙"}"#);
    let err = env.ensure_ok("查询小红花余额").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("500"), "要带业务码：{text}");
    assert!(text.contains("系统繁忙"), "要带上游原文：{text}");
    assert!(text.contains("查询小红花余额"), "要带是哪一步：{text}");
}

/// ⚠ `90001` 不是"任务失败"，是**票的问题** —— 要换成会话层那句人话。
///
/// 混进一句干巴巴的"失败 code=90001"，人会去查账号、查权限、查重试，
/// 而真正该做的是**重建会话**。
#[test]
fn session_expired_is_described_as_a_session_problem() {
    let env = RedFlowerEnvelope::parse(&format!(
        r#"{{"code":{},"msg":"未登录"}}"#,
        crate::session_source::CODE_SESSION_EXPIRED
    ));
    let text = env.ensure_ok("查询小红花余额").unwrap_err().to_string();
    assert!(
        text.contains("会话") || text.contains("登录"),
        "要指向会话问题，而不是一句干巴巴的业务码：{text}"
    );
}

/// 成功但 `result` 缺失时 `ok_result` 报错（要 `result` 的接口走 [Self::ok_result]）。
#[test]
fn ok_result_rejects_missing_result() {
    let env = RedFlowerEnvelope::parse(r#"{"code":0,"msg":"success"}"#);
    assert!(env.ensure_ok("x").is_ok(), "没有 result 不影响成败判定");
    assert!(env.ok_result("x").is_err(), "要 result 的接口必须报错");
    // 裸数字的 result 照样返回（形状校验交给各自的解析函数）
    let bare = RedFlowerEnvelope::parse(r#"{"code":0,"result":100}"#);
    assert_eq!(*bare.ok_result("x").unwrap(), serde_json::json!(100));
}

// ══════════════════════════ 端点与只读/写入的区分 ══════════════════════════

/// 端点全部落在 `/ycloud` 下（与签到同域），**不是** `/cmlive`。
#[test]
fn endpoints_are_all_under_ycloud() {
    for path in [
        USER_INFO_PATH,
        JOURNALING_PATH,
        FIRST_PARTICIPATE_GIVE_PATH,
        DRAW_PATH,
        QUERY_REVIVAL_FLOWERS_PATH,
    ] {
        assert!(path.starts_with('/'), "路径要以 / 开头：{path}");
        assert!(!path.contains("/cmlive"), "不在直播侧那一族里：{path}");
    }
    assert_eq!(BASE_LIVE_ROOM_FEEDBACK, "https://m.mcloud.139.com/ycloud");
    for req in [
        user_info_request(),
        journaling_request("e"),
        first_participate_give_request(),
        draw_request("c"),
        query_revival_flowers_request(),
    ] {
        assert!(
            req.url.starts_with("https://m.mcloud.139.com/ycloud/"),
            "实得：{}",
            req.url
        );
    }
}

/// 六步链里**只有一个写请求**（`draw`），其余都是读 —— 这条防的是"哪天有人
/// 把 `give` 也改成 GET"或者"把 `queryRevivalFlowers` 改成 POST"。
#[test]
fn step_chain_http_methods_match_the_capture() {
    assert_eq!(user_info_request().method, "POST");
    assert_eq!(journaling_request("e").method, "POST");
    assert_eq!(first_participate_give_request().method, "POST");
    assert_eq!(draw_request("c").method, "POST");
    assert_eq!(query_revival_flowers_request().method, "GET");
    // 口令走 query，**不在 body 里**
    let draw = draw_request("中文口令");
    assert!(
        draw.url.contains("?code="),
        "口令必须在 query 上：{}",
        draw.url
    );
    assert!(draw.body.is_none(), "口令不该出现在 body 里");
}
