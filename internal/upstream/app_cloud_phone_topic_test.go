package upstream

// 红包派对（云机）族 · 答题与 RCS 的测试。
//
// ⚠ **三条口全部零实拍样本**（`configTopicList` / `userTopicAnswer` / `getRcsToken`
// 在夹具 39 条里 grep 命中 0；夹具里两条同名 `getRcsToken` 在
// `cloud.139.com/uniphone` 上，是**另一个主机、另一个 appId**）。
// 所以与 `app_cloud_phone_install_test.go` 同理：本文件**不**假装对拍过上游，
// 钉的是"成文给过的那一格取值/键名有没有被写歪"与"没给的那些**没有**被我们编出来"。
//
// 三条判据方向各不一样，值得单独说：
//   - `answer` 的四个取值**有成文**（§2.4 第 376 行逐字）⇒ 不认识的值直接**不发**
//     （空值也不发：把"答不上来"折成某个选项等于每次拿一个没依据的选项去赌）；
//   - 题目列表**没有**任何键名证据 ⇒ 只交原文，且"列表里出现非对象的一项"
//     被做成**错误**（与安装类那一条**刻意不同**：答错题的代价比列错一台机高）；
//   - `getRcsToken` 的返回值被旧实现**丢弃**后还 `return true`（成文 §2.4 第 377 行
//     点名那个假成功）⇒ 本层交原文、且不判"任务完成"。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 路径与 body
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneTopicPathsAndBodies 三条口的 URL 形状与"body 带 version"这一**推的**约定。
//
// ⚠ 三条的前缀**分家**：答题两条挂 `redpacket/`（与实拍那四条同类），
// `getRcsToken` 挂 `user/`（本族唯一实拍的 `user/tokenValidate` 那一类）。
// 两处都是推的 ⇒ 404 时改常量那一行，别改这些断言的**形状**。
func TestCloudPhoneTopicPathsAndBodies(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	list, perr := CloudPhoneRequest(cloudPhoneConfigTopicListPath, nil, auth, "")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	rcs, perr := CloudPhoneGetRcsTokenRequest(auth, "", map[string]any{"someId": "199"})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	answer, perr := CloudPhoneUserTopicAnswerRequest(auth, "", CloudPhoneTopicAnswerA, nil)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	for name, request := range map[string]core.HTTPRequest{
		"configTopicList": list, "getRcsToken": rcs, "userTopicAnswer": answer,
	} {
		if !strings.HasPrefix(request.URL, CloudPhoneBaseURL+"/") {
			t.Errorf("%s 的 URL 应挂在入口根下，实得 %q", name, request.URL)
		}
		if request.Method != core.MethodPost {
			t.Errorf("%s 应是 POST，实得 %s", name, request.Method)
		}
		if !strings.Contains(string(request.Body), `"`+CloudPhoneVersion+`"`) {
			t.Errorf("%s 的 body 应带 version（⚠ 这一条是**推的**）：%s", name, request.Body)
		}
		if request.IdempotencyKey != nil {
			t.Errorf("%s 不该被本层安上幂等键（上游不认）", name)
		}
	}
	if got := list.URL; got != CloudPhoneBaseURL+"/redpacket/configTopicList" {
		t.Errorf("configTopicList 实得 %q", got)
	}
	if got := rcs.URL; got != CloudPhoneBaseURL+"/user/getRcsToken" {
		t.Errorf("getRcsToken 实得 %q", got)
	}
	if got := answer.URL; got != CloudPhoneBaseURL+"/redpacket/userTopicAnswer" {
		t.Errorf("userTopicAnswer 实得 %q", got)
	}
	// ⚠ 别的域那两条 `getRcsToken` 的 body 键是 `targetSourceId`/`token` ——
	// 本层**没有**把它们抄进任何请求（那是另一个主机的证据）。
	if strings.Contains(string(rcs.Body), "targetSourceId") {
		t.Errorf("不该把另一个域的键名搬进来：%s", rcs.Body)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 题目列表
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudPhoneTopicListLayers ⭐ 取列表的顺序与"读不出来"那一档。
//
// ⚠ 候选键名 `list` / `records` / `topics` **全部零样本** —— 这条测试钉的是
// "本层在按某个顺序找、并且看得见自己走到了哪一层"，不是"上游就给这个键"。
//
// 反向改坏它：把 `Nested` 写死 ⇒ 第二条红；把空数组读成"一层非空列表"⇒ 第三条红。
func TestParseCloudPhoneTopicListLayers(t *testing.T) {
	for _, body := range []string{
		`{"header":{"status":"200"},"data":{"topics":[{"q":"SYNTH-199-a"},{"q":"SYNTH-199-b"}]}}`,
		`{"header":{"status":"200"},"data":{"list":[{"q":"SYNTH-199-a"},{"q":"SYNTH-199-b"}]}}`,
		`{"header":{"status":"200"},"data":{"records":[{"q":"SYNTH-199-a"},{"q":"SYNTH-199-b"}]}}`,
	} {
		got, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Fatalf("解析失败: %v", perr)
		}
		if nested {
			t.Errorf("浅层取到的不该标 nested：%s", body)
		}
		if len(got) != 2 || got[0]["q"] != "SYNTH-199-a" {
			t.Errorf("题目应原样交出，实得 %#v", got)
		}
	}
	// 成文本族另一条口点名要兼容的那一层：`data.data`。
	deep, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"data":[{"q":"SYNTH-199-a"}]}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if !nested || len(deep) != 1 {
		t.Errorf("从 data.data 取到的必须标 nested，实得 %#v / %v", deep, nested)
	}
	// 两种都拿不到 ⇒ nil 列表、不报错（形状未知不是失败）。
	for name, body := range map[string]string{
		"data 缺失": `{"header":{"status":"200"}}`,
		"没有候选键":   `{"header":{"status":"200"},"data":{"anything":1}}`,
		"给了空数组":   `{"header":{"status":"200"},"data":{"list":[]}}`,
		"候选键是对象":  `{"header":{"status":"200"},"data":{"list":{"1":{}}}}`,
	} {
		got, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：只读口不该报错: %v", name, perr)
			continue
		}
		if got != nil || nested {
			t.Errorf("%s：应是 nil 列表 / 未嵌套，实得 %#v / %v", name, got, nested)
		}
	}
	// 信封不过 ⇒ 报错。
	if _, _, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"题库挂了"},"data":{}}`)); perr == nil {
		t.Error("status 非 200 应报错")
	}
}

// TestParseCloudPhoneTopicListRefusesHalfKnownItems ⭐ 与安装类**刻意不同**的一处处置。
//
// 列表里出现非对象的一项 ⇒ 整份列表判为读不出（报错），而不是"跳过那一项继续"：
// 旧实现的答题链是"取 `[0]` 去答"，跳过一项会让 `[0]` 变成**另一道题**，
// 而答错题的代价是真金白银（这一族产出钱）。
// 列云机那一条反过来（读不出的实例不该让整份清单作废）——两处的不同是有意的，
// 别"顺手统一"。
//
// 反向改坏它：把 `return nil, false, core.NewParse(...)` 改成 `continue` ⇒ 这一条红。
func TestParseCloudPhoneTopicListRefusesHalfKnownItems(t *testing.T) {
	// ⚠ 两份都是"第一条能读、第二条读不出"：跳过一项会让 `[0]` 换成另一道题，
	// 而坏项在头还是在尾都必须整份作废（`continue` 那一种在尾部坏项上也会红，
	// 因为那时交出的是一份**残缺**列表）。
	for name, body := range map[string]string{
		// ⚠ 三份都是"能读的一项 + 读不出的一项"：跳过一项会让 `[0]` 换成另一道题，
		// 而坏项在头还是在尾都必须整份作废（`continue` 那一种在尾部坏项上也会红，
		// 因为那时交出的是一份**残缺**列表）。
		"坏项在尾":  `{"header":{"status":"200"},"data":{"list":[{"q":"SYNTH-199-a"},"SYNTH-199-not-object"]}}`,
		"坏项在头":  `{"header":{"status":"200"},"data":{"list":["SYNTH-199-not-object",{"q":"SYNTH-199-a"}]}}`,
		"深层的坏项": `{"header":{"status":"200"},"data":{"data":["SYNTH-199-not-object"]}}`,
		"数字的一项": `{"header":{"status":"200"},"data":{"list":[{"q":"SYNTH-199-a"},199]}}`,
	} {
		got, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t, body))
		if perr == nil {
			t.Fatalf("%s：半生不熟的列表应报错，实得 %#v / %v", name, got, nested)
		}
		if got != nil {
			t.Errorf("%s：报错时不该交出半成品，实得 %#v", name, got)
		}
		// 报错时 `nested` 也必须是 false：交出的是一份"什么都没有"的结论，
		// 此时再报"我走到深层了"会让人以为深层那一层已被判定过。
		if nested {
			t.Errorf("%s：报错时 nested 应为 false，实得 %v", name, nested)
		}
		// 这是**形状不认识**，不是业务答复 ⇒ [core.KindParse]（与 status 非 200 那一档分开）。
		if perr.Kind() != core.KindParse {
			t.Errorf("%s：kind 应是 Parse，实得 %v（%q）", name, perr.Kind(), perr.Message())
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 答案
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneTopicAnswerValuesAreUpstreamLiterals 四个选项逐字大写。
//
// ⚠ `Valid()` 只认这四个，**不代表**上游只接受这四个（零样本）——
// 它代表的是"我们不替一个没依据的取值发请求"。
func TestCloudPhoneTopicAnswerValuesAreUpstreamLiterals(t *testing.T) {
	for _, c := range []struct {
		value CloudPhoneTopicAnswer
		valid bool
	}{
		{CloudPhoneTopicAnswerA, true}, {CloudPhoneTopicAnswerB, true},
		{CloudPhoneTopicAnswerC, true}, {CloudPhoneTopicAnswerD, true},
		{"", false},    // ⭐ 空值不合法："答不上来"是真实存在的状态
		{"a", false},   // 大小写不猜
		{"E", false},   // 有没有第五个选项不知道
		{"A ", false},  // 带空白不算
		{"A|B", false}, // 多选不猜
	} {
		if c.value.Valid() != c.valid {
			t.Errorf("%q 的合法性应为 %v，实得 %v", c.value, c.valid, c.value.Valid())
		}
		if c.value.String() != string(c.value) {
			t.Errorf("String 应返回上游字面量，实得 %q", c.value.String())
		}
	}
	if CloudPhoneTopicAnswerA != "A" || CloudPhoneTopicAnswerD != "D" {
		t.Errorf("四个值应逐字照抄成文那一句")
	}
}

// TestCloudPhoneUserTopicAnswerRequest ⭐ body 里那一个**有出处的键**。
//
// 成文 §2.4 第 376 行逐字给了 `userTopicAnswer {answer:"A"|"B"|"C"|"D"}` ——
// 这是这一族零样本口里唯一被点过名的 body 键名，所以它写死在构造器里，
// 而**题目标识**仍然交回调用方（成文那一句里没有它）。
//
// 反向改坏它：把 `answer` 写成小写或换成 `option` ⇒ 逐字那条红；
// 把不认识的取值也发出去 ⇒ "空值不发"那条红。
func TestCloudPhoneUserTopicAnswerRequest(t *testing.T) {
	request, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(),
		cloudPhoneSyntheticReferer, CloudPhoneTopicAnswerA, nil)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"answer":"A","version":"SYS_CONFIG_Y"}`; string(request.Body) != want {
		t.Errorf("body 实得 %s，应为 %s", request.Body, want)
	}
	// 题目标识由调用方补：补了就带上，本层不猜键名。
	extra := map[string]any{"someQuestionId": "199"}
	withExtra, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(), "",
		CloudPhoneTopicAnswerC, extra)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	body := string(withExtra.Body)
	for _, key := range []string{`"answer":"C"`, `"someQuestionId":"199"`, `"version"`} {
		if !strings.Contains(body, key) {
			t.Errorf("body 里少了 %s：%s", key, body)
		}
	}
	if _, dirty := extra["answer"]; dirty {
		t.Error("调用方的 map 被写脏了（重试 15 次会共用它，见函数注释）")
	}
	// ⚠ 不认识的取值 ⇒ **不发**：宁可不答，也不拿一个没依据的选项去赌。
	for _, bad := range []CloudPhoneTopicAnswer{"", "a", "E", "A|B"} {
		if _, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(), "", bad, nil); perr == nil {
			t.Errorf("answer=%q 不该被发出去", bad)
		}
	}
}

// TestAppCloudPhoneTopicAnswersJudgeTheEnvelope ⭐ 三条口的发送层都判信封。
//
// `userTopicAnswer` 那一枪尤其：成文记的旧实现拿"答对没有"驱动一个
// `15×150ms` 的重试循环 ⇒ 把信封失败读成"成功"会让循环**在错误上自转**。
// 本层因此只交 `data` 原文，不判对错（对错那一格的键名零样本）。
func TestAppCloudPhoneTopicAnswersJudgeTheEnvelope(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	bad := core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"答题通道忙"},"data":{"errorMsg":"今日次数已用完"}}`))
	ok := core.NewResponse(200, []byte(`{"header":{"status":"200"},"data":{"anything":"SYNTH-199"}}`))

	failing := &scriptedClient{response: bad}
	if _, perr := NewUpstream(failing).AppCloudPhoneUserTopicAnswer(nil, auth, "",
		CloudPhoneTopicAnswerB, nil); perr == nil {
		t.Error("status 非 200 时答题必须报错")
	}
	if n := len(failing.requests); n != 1 {
		// 这一枪**是**发出去了（信封不过 ≠ 没发），但**只发一次**：
		// 本层不许"没成就再来一枪"—— 成文那个 `15×150ms` 的循环归编排层。
		t.Errorf("应答恰好一条，实得 %d 条", n)
	}
	passing := &scriptedClient{response: ok}
	data, perr := NewUpstream(passing).AppCloudPhoneUserTopicAnswer(nil, auth, "",
		CloudPhoneTopicAnswerB, nil)
	if perr != nil {
		t.Fatalf("成功信封不该报错: %v", perr)
	}
	if data["anything"] != "SYNTH-199" {
		t.Errorf("载荷该交原文（判对错归编排层），实得 %#v", data)
	}
	// 题目列表这一条同样判信封，且不发就不报错。
	lists := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"200"},"data":{"list":[{"q":"SYNTH-199-a"}]}}`))}
	got, nested, perr := NewUpstream(lists).AppCloudPhoneConfigTopicList(nil, auth, "", nil)
	if perr != nil || nested || len(got) != 1 {
		t.Errorf("取题目实得 %#v / %v / %v", got, nested, perr)
	}
	// `getRcsToken`：旧实现**丢弃返回值**后还 `return true`（成文 §2.4 第 377 行的假成功）。
	// 本层把原文交出来，并让"打过这一枪"与"任务完成"是两件事。
	rcsClient := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"200"},"data":{"anything":"SYNTH-199-rcs"}}`))}
	rcsData, perr := NewUpstream(rcsClient).AppCloudPhoneGetRcsToken(nil, auth, "", nil)
	if perr != nil {
		t.Fatalf("不该报错: %v", perr)
	}
	if rcsData == nil || rcsData["anything"] == nil {
		t.Error("返回值不该被丢掉 —— 旧实现丢了它、然后宣称任务完成，那是 §1.4 记的假成功")
	}
	rcsFailing := &scriptedClient{response: bad}
	if _, perr := NewUpstream(rcsFailing).AppCloudPhoneGetRcsToken(nil, auth, "", nil); perr == nil {
		t.Error("status 非 200 时应报错")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 补：`Nested` 的第三档、共用 map 重试、护栏不留半条请求、两套信封不通用
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudPhoneTopicListEmptyDeepLayerStillReportsNested ⭐ `Nested` 这一位
// 存在的**唯一**理由就在这一档：深层给了**空数组**。
//
// 上面那条只钉到"浅层空数组 ⇒ nested=false"；而 [ParseCloudPhoneTopicList] 的注释
// 承诺的是"『列表为空』与『取错了层』分得开"。两种空必须长得不一样：
//   - 浅层 `data.list: []`   ⇒ nil 列表 + nested=false（这一层确实是空的）；
//   - 深层 `data.data: []`   ⇒ nil 列表 + nested=**true**（列表在那一层，只是没内容）。
//
// 反向改坏它：把 `len(topics)==0 ⇒ return nil, nested` 改成 `return nil, false`
// （"反正列表是空的"）⇒ 本条红，而接线时的表现是"深层明明命中了却被当成没命中"。
func TestParseCloudPhoneTopicListEmptyDeepLayerStillReportsNested(t *testing.T) {
	deep, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"data":[]}}`))
	if perr != nil {
		t.Fatalf("空数组不该报错: %v", perr)
	}
	if deep != nil {
		t.Errorf("空列表该交 nil（不是一条元素都没有 ≠ 有一条空壳），实得 %#v", deep)
	}
	if !nested {
		t.Error("深层取到的空列表**必须**标 nested=true —— 这一位就是为这一档留的")
	}
	// 对照：浅层那个空数组不该标 nested。
	shallow, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"list":[]}}`))
	if perr != nil || shallow != nil || nested {
		t.Errorf("浅层空数组应是 nil/false，实得 %#v / %v / %v", shallow, nested, perr)
	}
}

// TestCloudPhoneUserTopicAnswerRequestKeepsTheSharedMapReusable ⭐ 成文那个
// `15×150ms` 重试循环会**共用同一份上下文 map**（函数注释点名的就是这件事），
// 所以每一枪的 body 只能含**本轮**那个答案。
//
// 反向改坏它：把 `answer` 直接写进传进来的 `data`（省掉副本）⇒ 第二次构造时
// 那份 map 里留着上一轮的键，第一条断言就红；更糟的是重试到第 15 次时
// body 里到底是哪个答案取决于**谁最后写过它**。
func TestCloudPhoneUserTopicAnswerRequestKeepsTheSharedMapReusable(t *testing.T) {
	shared := map[string]any{"someQuestionId": "199"}
	first, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(), "",
		CloudPhoneTopicAnswerA, shared)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	second, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(), "",
		CloudPhoneTopicAnswerB, shared)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if !strings.Contains(string(first.Body), `"answer":"A"`) ||
		strings.Contains(string(first.Body), `"answer":"B"`) {
		t.Errorf("第一枪只该含 A：%s", first.Body)
	}
	if !strings.Contains(string(second.Body), `"answer":"B"`) ||
		strings.Contains(string(second.Body), `"answer":"A"`) {
		t.Errorf("第二枪只该含 B（共用 map 被写脏就是现在这个样子）：%s", second.Body)
	}
	if len(shared) != 1 {
		t.Errorf("调用方那份 map 不该多出任何键，实得 %#v", shared)
	}
	// data 为 nil 就是成文那一条 body（逐字两键，不多不少）。
	bare, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(), "",
		CloudPhoneTopicAnswerD, nil)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"answer":"D","version":"SYS_CONFIG_Y"}`; string(bare.Body) != want {
		t.Errorf("body 应逐字为 %s，实得 %s", want, bare.Body)
	}
}

// TestCloudPhoneUserTopicAnswerGuardLeavesNothingToSend ⭐ 护栏的两个"不留"。
//
// [CloudPhoneTopicAnswer.Valid] 不过时构造函数返回的是**零值** [core.HTTPRequest]
// （URL 空、无头、无 body）：半条请求比没有请求更危险 —— 调用方一旦只看 perr
// 而把 request 传下去，发出去的就是一条**没有答案**的答题请求，而上游对它的反应零样本。
// 发送层那一半钉的是"一条都不发"。
//
// 反向改坏它：把 guard 挪到构造之后、或让它带着半条请求返回 ⇒ 两档都红。
func TestCloudPhoneUserTopicAnswerGuardLeavesNothingToSend(t *testing.T) {
	for _, bad := range []CloudPhoneTopicAnswer{"", "a", "E", "A|B", "  "} {
		request, perr := CloudPhoneUserTopicAnswerRequest(cloudPhoneSyntheticAuth(),
			cloudPhoneSyntheticReferer, bad, map[string]any{"someQuestionId": "199"})
		if perr == nil {
			t.Errorf("answer=%q 应报错", bad)
			continue
		}
		if request.URL != "" || len(request.Headers) != 0 || len(request.Body) != 0 {
			t.Errorf("answer=%q 报错时不许留下一条发得出去的请求，实得 %+v", bad, request)
		}
		if perr.Kind() != core.KindParse {
			t.Errorf("answer=%q：这是我们自己拒发的请求，应归 Parse，实得 %v", bad, perr.Kind())
		}
		// 错误文案不许把注入的凭据带出来（它只该说"答案不认识"）。
		if strings.Contains(perr.Message(), cloudPhoneSyntheticToken) {
			t.Errorf("错误文案里出现了凭据：%q", perr.Message())
		}
	}
	// 发送层：护栏没过 ⇒ **一条都不发**（发出去就是拿一个没依据的选项答题）。
	client := &scriptedClient{response: core.NewResponse(200,
		[]byte(`{"header":{"status":"200"},"data":{}}`))}
	if _, perr := NewUpstream(client).AppCloudPhoneUserTopicAnswer(nil,
		cloudPhoneSyntheticAuth(), "", CloudPhoneTopicAnswer(""), nil); perr == nil {
		t.Error("空答案应报错")
	}
	if n := len(client.requests); n != 0 {
		t.Errorf("护栏没过时一条都不该发，实得 %d 条", n)
	}
}

// TestCloudPhoneTopicParsersRejectTheOtherEnvelopeFamilies ⭐ **两套信封不通用**。
//
// 这一族**完全没有** `code` 字段（成文 §1.2 最后那一行点名这件事）；而同一份夹具里
// 另外两族的判据是 `code`：`ycloud/newyear` 那族是**数字** 0，`ai` 那族是**字符串**
// `"0000"` + `success:true`。通用的 [Envelope.EnsureOK] 对这三份**都**判通过
// （它找 `code`，"没有 code"算通过）⇒ 混解是一条静默通路。
//
// 所以这一条同时钉两个方向：
//   - 把别族的响应喂给本族解析器 ⇒ 必须报错（本族要求 `header.status`）；
//   - 把本族的响应喂给别族的解析器 ⇒ 也必须报错（那一族要求 `code`）。
//
// 反向改坏它：把 [EnsureCloudPhoneOK] 换成 `envelope.EnsureOK()` ⇒ 前两档红
// （`{"code":0}` 与 `{"code":"0000"}` 都会被当成成功）。
func TestCloudPhoneTopicParsersRejectTheOtherEnvelopeFamilies(t *testing.T) {
	for name, foreign := range map[string]string{
		"ycloud/newyear 那族（code 是数字 0）": `{"code":0,"msg":"success","result":{"totalLotteryCount":0}}`,
		"ai 那族（code 是字符串 0000）":         `{"code":"0000","message":"请求成功","success":true,"data":{"taskId":"199"}}`,
	} {
		envelope := cloudPhoneEnvelope(t, foreign)
		if _, _, perr := ParseCloudPhoneTopicList(envelope); perr == nil {
			t.Errorf("%s：不是本族的信封，绝不能判成成功", name)
		} else if perr.Kind() != core.KindParse {
			t.Errorf("%s：信封不认识应归 Parse，实得 %v", name, perr.Kind())
		}
		if _, perr := ParseCloudPhoneData(envelope); perr == nil {
			t.Errorf("%s：载荷口同样要报错", name)
		}
	}
	// 反方向：本族那份交给 ai 那族的解析器（它的 data 段是**对象**，
	// 只差判据字段）⇒ 那一族缺 `code`/`success` 必须红。
	mine := cloudPhoneEnvelope(t, `{"header":{"status":"200"},"data":{"anything":"SYNTH-199"}}`)
	if _, perr := PlayAIParseRecognize(mine); perr == nil {
		t.Error("本族信封喂给 ai 那族也该报错（那一族找的是 code/success，本族没有）")
	}
	if _, perr := PlayAIParseUserStatus(mine); perr == nil {
		t.Error("本族信封喂给 newyear 那族同样该报错")
	}
}

// TestParseCloudPhoneTopicListRefusesHeaderWithoutStatus ⭐ 题目口：有 `header`
// 但**没给 `status`** ⇒ 不许当成功（这一族没有 `code` 那一位，宽松判据会放过去）。
//
// 为什么这一档在题目口格外要紧：答对与否要拿题目列表当依据，
// "列表为空"与"这一口其实失败了"必须分得开 —— 混起来的表现是
// **一题没答就报"答题任务完成"**。所以报错路径交回来的必须是 nil + nested=false。
//
// 反向改坏它：把 [EnsureCloudPhoneOK] 里 `if status == nil` 那一支改成"交 data、不报错"
// ⇒ 本条红（列表还会带着那一项被读成成功）。
func TestParseCloudPhoneTopicListRefusesHeaderWithoutStatus(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	for name, body := range map[string]string{
		"header 在但没 status": `{"header":{"appId":"12345681","errMsg":"OK"},"data":{"list":[{"id":"199"}]}}`,
		"status 是数字":        `{"header":{"status":200},"data":{"list":[{"id":"199"}]}}`,
		"status 是 null":     `{"header":{"status":null},"data":{"data":[]}}`,
		"整个 header 没有":      `{"data":{"list":[{"id":"199"}]}}`,
	} {
		list, nested, perr := ParseCloudPhoneTopicList(cloudPhoneEnvelope(t, body))
		if perr == nil {
			t.Errorf("%s：不能判成成功，实得 %d 项 nested=%v", name, len(list), nested)
			continue
		}
		if perr.Kind() != core.KindParse {
			t.Errorf("%s：信封不认识应归 Parse，实得 %v", name, perr.Kind())
		}
		if list != nil || nested {
			t.Errorf("%s：报错时该交 nil + false，实得 %#v / %v", name, list, nested)
		}
	}
	// 发送层同样拦住：真调用一次，确认错误是从信封判定那里传上来的。
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"appId":"12345681","errMsg":"OK"},"data":{"list":[{"id":"199"}]}}`))}
	got, gotNested, perr := NewUpstream(client).AppCloudPhoneConfigTopicList(nil, auth, "", nil)
	if perr == nil {
		t.Fatalf("发送层把没给 status 读成了成功，实得 %d 项 nested=%v", len(got), gotNested)
	}
	if got != nil || gotNested {
		t.Errorf("报错时不该交出题目：%#v / %v", got, gotNested)
	}
	if n := len(client.requests); n != 1 {
		t.Errorf("应只发一条请求，实得 %d 条", n)
	}
}
