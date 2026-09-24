package app

// 直播口令 → 小红花**业务编排**的测试。
//
// 这一层不联网：HTTP 走注入的 [scriptedUpstream]（见 signin_test.go），
// 它按脚本**顺序**回放响应并留下收到的每一个请求。
//
// # 断言的重点不是"请求长什么样"
//
// 那是 `internal/upstream` 的事（头、body 形态、百分号编码都在那边钉）。
// 这边只钉两件这一层**独有**的事：
//
//  1. **第几步发了哪个接口** —— 也就是"顺序"与"次数"；
//  2. **拿到某个响应之后该不该继续** —— 也就是"哪几支是正常的静默分支"。
//
// ⚠⚠ 为什么第 1 条值得专门写断言：漏发 `visitlog/journaling` 那 5 条埋点，
// 服务端**不会**拒掉 `draw`，它只会回 `200108`（口令不对）。于是日志里
// 表现成"口令抄错了"，而真正的原因是少发了一步。把顺序钉成断言，
// 是让"少发一步"变成**一条失败的测试**的唯一办法 —— 否则它只能等开播才发现。

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 响应体样本
// ─────────────────────────────────────────────────────────────────────────────

// 下面这些响应体逐字取自实拍（docs/capture/app-live-redflower-samples.txt §5.x），
// 只把凭据换成了合成值。**不要**把它们"整理"成更干净的样子：
// `msg` 里那句"口令需当场直播获取才有效哦"是排查时唯一的上下文。
const (
	liveTestJWT = "test-jwt-not-a-real-credential"

	liveJournalingBody          = `{"code":0,"msg":"success","result":null}`
	liveGiveGainedBody          = `{"code":0,"msg":"success","result":{"flowerNum":3}}`
	liveGiveNoQualificationBody = `{"code":200110,"msg":"您已参与过活动，无法获得首次参与惊喜奖"}`
	liveDrawInvalidBody         = `{"code":200108,"msg":"口令不对耶，口令需当场直播获取才有效哦"}`
	liveDrawAlreadyBody         = `{"code":200112,"msg":"该口令已兑换"}`
	liveDrawOtherBody           = `{"code":200199,"msg":"活动尚未开始"}`
	// ⚠⚠ 这一条**不是**实拍原文 —— 它是"最小信封"（只有 `code`）：
	// `200118` 的上游原文（`msg`）本仓**仍无实拍**（`RedFlowerDrawOutcome.Message`
	// 解析了却从不输出）⇒ 这里**刻意不写 `msg`**。编一句"像真的"文案进来，
	// 下一个人会把它当成抓包原文（同一条纪律见 `internal/upstream/live_flower_test.go`
	// 的 `TestClassifyRedFlowerDrawOnExpiredKouling` 函数头）。
	// ⭐ 对本用例没有损失：非 0 的分支**只看 `code`**。
	liveDrawExpiredBody           = `{"code":200118}`
	liveRevivalBody               = `{"code":0,"msg":"success","result":0}`
	liveUserInfoWithoutResultBody = `{"code":0,"msg":"success"}`
)

func liveUserInfoBody(flowerCnt int64) string {
	return fmt.Sprintf(`{"code":0,"msg":"success","result":{"isQuarterDrawed":0,`+
		`"flowerCnt":%d,"prizeId":0,"secondPrizeId":0,`+
		`"redFlowerValidDate":"2026年12月31日","hasNotReceivedRedFlower":0}}`, flowerCnt)
}

func liveDrawOKBody(flowerNum int64) string {
	return fmt.Sprintf(`{"code":0,"msg":"success","result":{"prizeId":1,"flowerNum":%d,`+
		`"prizeName":"小红花","prizeType":1}}`, flowerNum)
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试替身与工具
// ─────────────────────────────────────────────────────────────────────────────

// flowerSource 造一个"票已经在了"的 [SignInSessionSource]，让六步链的用例
// 不必为了建会话再补三枪响应（那会把每条用例的脚本都改写一遍，而且
// [liveStepOf] 认不出 `tyrzLogin` 会当场 Fatal）。
//
// 档案与凭据给**真值**而不是 nil：万一哪条用例的响应让链撞上了 `90001`，
// 刷新会真的去打三枪 —— 那时该响的是"脚本里没有更多响应了"，
// 而不是一个空指针 panic（后者会把"票会刷新"这件事演成一次崩溃）。
func flowerSource(up *upstream.Upstream, jwt string) *SignInSessionSource {
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}
	src := NewSignInSessionSource(up, testProfile(), &cred, testClock(), testRng())
	src.session = &SignInSession{referer: "https://referer.test", jwt: jwt}
	return src
}

// liveStepOf 把一个请求归到六步链 / 直播链里的某一步。
//
// ⚠ 认不出的端点直接 `Fatal`：如果这一层哪天多打了一个接口，
// 那个接口应当**当场**让测试红，而不是被安静地忽略掉。
//
// ⚠ 判断顺序有讲究：`/liveRoomFeedback/user/info` 与
// `/cmlive/api/user/auth/getCurrentUserInfoNew` 都含 `user/`，
// 所以**先匹长的、专有的那一个**，不要按"看起来更笼统"的顺序写。
func liveStepOf(t *testing.T, url string) string {
	t.Helper()
	switch {
	// 小红花六步链
	case strings.Contains(url, "/liveRoomFeedback/user/info"):
		return "user_info"
	case strings.Contains(url, "/visitlog/journaling"):
		return "journaling"
	case strings.Contains(url, "/firstParticipate/give"):
		return "first_participate"
	case strings.Contains(url, "/redFlower/draw"):
		return "draw"
	case strings.Contains(url, "/redFlower/queryRevivalFlowers"):
		return "revival_flowers"
	// 门
	case strings.Contains(url, "/user/querySpecTokenV2"):
		return "query_spec_token"
	case strings.Contains(url, "/cmlive/api/user/auth/cmpassLoginByCmToken"):
		return "cm_pass_login"
	case strings.Contains(url, "/channel/channel/tabContentList"):
		return "tab_content"
	// 云盘侧会话（六步链与余额那份 JWT 从这里来 —— 补记见 app.ProbeLiveRoom）
	case strings.Contains(url, "/ycloud/api/timestamp"):
		return "h5_timestamp"
	case strings.Contains(url, "/ycloud/auth-service/auth/tyrzLogin"):
		return "tyrz_login"
	// 进直播间
	case strings.Contains(url, "/cmlive/api/user/auth/getCurrentUserInfoNew"):
		return "live_current_user"
	case strings.Contains(url, "/cmlive/api/user/customer/userToken"):
		return "live_user_token"
	case strings.Contains(url, "/cmlive/api/room/front/roomBeat"):
		return "room_beat"
	case strings.Contains(url, "/v3/room/member/apply"):
		return "room_apply"
	// 源 B
	case strings.Contains(url, "xiaohongshu.com"):
		return "xiaohongshu"
	}
	t.Fatalf("请求打到了一个不属于这条链的端点：%s", url)
	return ""
}

// liveStepTrace 把收到的请求序列翻成步骤名序列。
func liveStepTrace(t *testing.T, client *scriptedUpstream) []string {
	t.Helper()
	out := make([]string, 0, len(client.requests))
	for _, request := range client.requests {
		out = append(out, liveStepOf(t, request.URL))
	}
	return out
}

func liveStringsEqual(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func liveAssertTrace(t *testing.T, client *scriptedUpstream, want []string) {
	t.Helper()
	got := liveStepTrace(t, client)
	if !liveStringsEqual(got, want) {
		t.Fatalf("请求序列不符\n实际: %v\n期望: %v", got, want)
	}
}

// liveHeader 取一个请求头（大小写不敏感）。
func liveHeader(request core.HTTPRequest, key string) (string, bool) {
	for _, header := range request.Headers {
		if strings.EqualFold(header[0], key) {
			return header[1], true
		}
	}
	return "", false
}

// liveJournalingKeywords 从埋点请求的 form 体里抠出 `optkeyword`（保序）。
//
// ⚠ 这里**必须**逐条解析而不是 `strings.Contains`：5 条埋点的 URL 完全相同，
// 只有 form 体里的 `optkeyword` 不同 —— 用 URL 去断言等于没断言。
func liveJournalingKeywords(t *testing.T, client *scriptedUpstream) []string {
	t.Helper()
	var out []string
	for _, request := range client.requests {
		if !strings.Contains(request.URL, "/visitlog/journaling") {
			continue
		}
		keyword := ""
		for _, field := range strings.Split(string(request.Body), "&") {
			if value, ok := strings.CutPrefix(field, "optkeyword="); ok {
				keyword = value
			}
		}
		if keyword == "" {
			t.Fatalf("埋点请求体里没有 optkeyword：%q", string(request.Body))
		}
		out = append(out, keyword)
	}
	return out
}

// liveDrawCodes 从 draw 请求的 query string 里抠出 `code`（保序，未解码）。
func liveDrawCodes(client *scriptedUpstream) []string {
	var out []string
	for _, request := range client.requests {
		if !strings.Contains(request.URL, "/redFlower/draw") {
			continue
		}
		_, query, _ := strings.Cut(request.URL, "?")
		for _, field := range strings.Split(query, "&") {
			if value, ok := strings.CutPrefix(field, "code="); ok {
				out = append(out, value)
			}
		}
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// ① 六步链的完整走法
// ─────────────────────────────────────────────────────────────────────────────

// TestRedFlowerSessionWalksSixStepChain 是这条链的**主断言**。
//
// 它一次钉住四件事：接口顺序、埋点次数、口令逐个投、以及①②⑥用的是同一个接口
// 但语义不同（起始余额 / 埋点 / 终值）。
func TestRedFlowerSessionWalksSixStepChain(t *testing.T) {
	codes := []string{"云盘AI高效复盘", "国庆红包口令"}
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(235), // ① 起始余额
		liveJournalingBody,    // ② 埋点 ×5（顺序即 LiveRoomJournalingEvents）
		liveJournalingBody,
		liveJournalingBody,
		liveJournalingBody,
		liveJournalingBody,
		liveGiveGainedBody,    // ③ 首次参与惊喜奖
		liveDrawOKBody(5),     // ④ 第 1 个口令
		liveDrawOKBody(8),     // ④ 第 2 个口令
		liveRevivalBody,       // ⑤ 复活花
		liveUserInfoBody(251), // ⑥ 终值
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT), NewRedFlowerSession(codes))
	if err != nil {
		t.Fatalf("整条链不该失败：%s", err.Message())
	}

	liveAssertTrace(t, client, []string{
		"user_info",
		"journaling", "journaling", "journaling", "journaling", "journaling",
		"first_participate",
		"draw", "draw",
		"revival_flowers",
		"user_info",
	})

	// 埋点发的是这 5 条、按这个顺序。
	if got := liveJournalingKeywords(t, client); !liveStringsEqual(got, upstream.LiveRoomJournalingEvents) {
		t.Fatalf("埋点顺序不符\n实际: %v\n期望: %v", got, upstream.LiveRoomJournalingEvents)
	}

	// 口令走 query string，且**百分号编码**（中文不编码会被截断）。
	if got := liveDrawCodes(client); !liveStringsEqual(got, []string{
		upstream.PercentEncode("云盘AI高效复盘"),
		upstream.PercentEncode("国庆红包口令"),
	}) {
		t.Fatalf("draw 的 code 不符：%v", got)
	}

	if report.BalanceBefore.FlowerCnt != 235 || report.BalanceAfter.FlowerCnt != 251 {
		t.Fatalf("两次读数不对：%d → %d",
			report.BalanceBefore.FlowerCnt, report.BalanceAfter.FlowerCnt)
	}
	// ⚠ 16 = (5 + 8) 两个口令 + 3 朵首次参与奖。
	// 只把 draw 的 flowerNum 加起来会得到 13，而余额差是 16 ——
	// 那 3 朵**不在**任何一个口令的结果里。
	if report.Gained() != 16 {
		t.Fatalf("进账应为 16（含首次参与奖 3），实际 %d", report.Gained())
	}
	if report.Succeeded() != 2 {
		t.Fatalf("应有 2 个口令成功，实际 %d", report.Succeeded())
	}
	if report.FirstParticipate.Kind != upstream.FirstParticipateGained ||
		report.FirstParticipate.FlowerNum != 3 {
		t.Fatalf("首次参与结论不对：%+v", report.FirstParticipate)
	}
	if len(report.InvalidCodes) != 0 || len(report.OtherCodes) != 0 {
		t.Fatalf("一次正常运行不该有无效/未分类口令：%v / %v",
			report.InvalidCodes, report.OtherCodes)
	}
}

// TestRedFlowerSessionDeduplicatesCodes 钉住"两个来源可能给出同一个口令"。
//
// WebSocket 弹幕与小红书主页**地位相同**，同一个口令在两边都出现是常态。
// 不去重就会对同一个口令投两次，而**同一账号**的第二次必然回 `200112` —— 日志里看起来
// 像"口令坏了"，而它其实只是重复。
func TestRedFlowerSessionDeduplicatesCodes(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(10),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawOKBody(5),
		liveDrawOKBody(2),
		liveRevivalBody,
		liveUserInfoBody(17),
	}}
	up := upstream.NewUpstream(client)

	session := NewRedFlowerSession([]string{"AAA", "AAA", " AAA ", "BBB"})
	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT), session)
	if err != nil {
		t.Fatalf("整条链不该失败：%s", err.Message())
	}
	// 去重 + trim 之后只剩 2 个。
	if got := session.Codes(); !liveStringsEqual(got, []string{"AAA", "BBB"}) {
		t.Fatalf("口令没有去重/trim：%v", got)
	}
	// ⭐ 4 个输入（含重复与空白）⇒ 只发 2 个 draw。
	if got := liveDrawCodes(client); len(got) != 2 {
		t.Fatalf("draw 请求数应为 2（去重后），实际 %d：%v", len(got), got)
	}
	if len(report.Draws) != 2 {
		t.Fatalf("Draws 应保序覆盖每个口令，实际 %d 条", len(report.Draws))
	}
	if report.Gained() != 7 {
		t.Fatalf("进账应为 7（5+2），实际 %d", report.Gained())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ② 静默分支：哪几支**不该**中断
// ─────────────────────────────────────────────────────────────────────────────

// TestRedFlowerNoQualificationDoesNotAbortBatch 钉住 `200110` 是**正常**的一支。
//
// 每个账号第二次跑这条链都会拿到它。用 `EnsureOK` 那种"非 0 即失败"的写法，
// 一次完全正常的运行会以一句"上游返回 code=200110"结束，读起来像故障；
// 更糟的是如果把它当成错误**中止**，后面 N 个口令一个都投不出去。
func TestRedFlowerNoQualificationDoesNotAbortBatch(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawOKBody(5),
		liveRevivalBody,
		liveUserInfoBody(105),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("`200110` 不该让整条链失败：%s", err.Message())
	}
	if report.FirstParticipate.Kind != upstream.FirstParticipateNoQualification {
		t.Fatalf("首次参与结论应为「无资格」，实际 %v", report.FirstParticipate.Kind)
	}
	// ⭐ 关键：口令**照样投了**。
	if report.Succeeded() != 1 {
		t.Fatalf("`200110` 之后口令仍应被投出去，实际成功 %d 个", report.Succeeded())
	}
	if report.Gained() != 5 {
		t.Fatalf("这次进账应只有口令那 5 朵，实际 %d", report.Gained())
	}
}

// TestRedFlowerAlreadyDrawnIsNotInvalidKouling 钉住 `200112` **不是**无效口令。
//
// 已兑换说明口令本身**是对的**。把它算进 InvalidCodes 会有两个后果：
// 一是触发一次毫无意义的 60 秒重听，二是把这个有效口令从缓存里剔掉，
// 让同一批账号里后面的那几个白跑一趟。
func TestRedFlowerAlreadyDrawnIsNotInvalidKouling(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawAlreadyBody,
		liveRevivalBody,
		liveUserInfoBody(100),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("`200112` 不该让整条链失败：%s", err.Message())
	}
	if len(report.InvalidCodes) != 0 {
		t.Fatalf("`200112` 不该被算成无效口令：%v", report.InvalidCodes)
	}
	if len(report.OtherCodes) != 0 {
		t.Fatalf("`200112` 已被单独分类，不该落进未分类：%v", report.OtherCodes)
	}
	if report.Draws[0].Outcome.Kind != upstream.RedFlowerDrawAlreadyDrawn {
		t.Fatalf("draw 结论应为 already_drawn，实际 %v", report.Draws[0].Outcome.Kind)
	}
	if report.Succeeded() != 0 {
		t.Fatalf("已兑换不算成功，实际 %d", report.Succeeded())
	}
	// 没有无效口令 ⇒ 不该重听（这条由 RetryPlan 决定，这里顺带钉一下联动）。
	if retry, _ := RetryPlan([]string{"AAA"}, report.InvalidCodes); retry {
		t.Fatal("没有无效口令却要求重听")
	}
}

// TestRedFlowerInvalidKoulingDrivesRetry 钉住 `200108` **是**无效口令，
// 且它不该被塞进 OtherCodes（那会让"上游说了个没分类的码"这一栏失真）。
func TestRedFlowerInvalidKoulingDrivesRetry(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawOKBody(5),
		liveDrawInvalidBody,
		liveRevivalBody,
		liveUserInfoBody(105),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA", "BBB"}))
	if err != nil {
		t.Fatalf("单个口令无效不该让整条链失败：%s", err.Message())
	}
	if !liveStringsEqual(report.InvalidCodes, []string{"BBB"}) {
		t.Fatalf("无效口令应为 [BBB]，实际 %v", report.InvalidCodes)
	}
	if len(report.OtherCodes) != 0 {
		t.Fatalf("`200108` 已单独分类，不该同时落进未分类：%v", report.OtherCodes)
	}
	// ⭐ 前一个口令失败**没有**带走后一个：两个都投了。
	if len(report.Draws) != 2 {
		t.Fatalf("两个口令都该被投出去，实际 %d 个", len(report.Draws))
	}
	retry, exclude := RetryPlan([]string{"AAA", "BBB"}, report.InvalidCodes)
	if !retry {
		t.Fatal("有无效口令却没有要求重听")
	}
	// ⚠ 排除表里**既有**无效的，也有第一次就成功的 —— 少了后者，
	// 第二次会把 AAA 再投一遍，而**同一账号**的第二次换回 `200112`。
	if !liveStringsEqual(exclude, []string{"AAA", "BBB"}) {
		t.Fatalf("重听排除表应为 [AAA BBB]，实际 %v", exclude)
	}
}

// TestRedFlowerOtherCodeIsNotTreatedAsInvalidKouling 钉住"码只判等、不判形状"。
//
// 原项目的判据是 `typeof code === "number"` ⇒ 一律当无效口令（它没记过 `200108`
// 时的兜底）。换成"只判等"之后，未分类的码**不会**触发重听 ——
// 代价是可能漏掉将来新增的"口令无效"码，收益是不为一个"活动没开始"白等 60 秒。
func TestRedFlowerOtherCodeIsNotTreatedAsInvalidKouling(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawOtherBody,
		liveRevivalBody,
		liveUserInfoBody(100),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("未分类的业务码不该让整条链失败：%s", err.Message())
	}
	if len(report.InvalidCodes) != 0 {
		t.Fatalf("未分类的码不该被当成无效口令：%v", report.InvalidCodes)
	}
	if !liveStringsEqual(report.OtherCodes, []string{"AAA"}) {
		t.Fatalf("未分类的口令应记进 OtherCodes，实际 %v", report.OtherCodes)
	}
	if retry, _ := RetryPlan([]string{"AAA"}, report.InvalidCodes); retry {
		t.Fatal("未分类的码不该触发重听")
	}
}

// TestRedFlowerExpiredKoulingIsItsOwnBucket 钉住 `200118` 有**自己那一栏**。
//
// # 为什么非要有这一栏
//
// 它原本与"上游回了个没分类的码"共用 `OtherCodes` ⇒ 人读的输出两件事都打
// 「未分类口令」，而这一格**最常见**的成因恰恰是"**直播已经结束了**" ——
// 那句话把人往"口令抄错了"（`200108`）那边引，排查方向一开始就错（坑表 **#140** 做法 ④）。
//
// ⚠ 两条不变量与 upstream 侧那条用例同源：它**不是**无效口令（否则白重听 60 秒）、
// 也**不是**成功。这里额外钉"它**不**落进另外两栏"，以及**人读的那句话**说了什么。
func TestRedFlowerExpiredKoulingIsItsOwnBucket(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawExpiredBody,
		liveRevivalBody,
		liveUserInfoBody(100),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("过期口令不该让整条链失败：%s", err.Message())
	}
	if !liveStringsEqual(report.ExpiredCodes, []string{"AAA"}) {
		t.Fatalf("过期口令应记进 ExpiredCodes，实际 %v", report.ExpiredCodes)
	}
	if len(report.InvalidCodes) != 0 {
		t.Fatalf("过期口令**不是**无效口令（口令本身是对的）：%v", report.InvalidCodes)
	}
	if len(report.OtherCodes) != 0 {
		t.Fatalf("`200118` 已有自己那一栏，不该同时落进未分类：%v", report.OtherCodes)
	}
	if retry, _ := RetryPlan([]string{"AAA"}, report.InvalidCodes); retry {
		t.Fatal("过期口令不该触发重听 —— 重听也只会再拿一次 `200118`")
	}
	// ⭐ 用户可见的那句话：说清是"失效"，不是"未分类"。
	if line := report.SummaryLine(); !strings.Contains(line, "口令已失效（本场已结束）") {
		t.Fatalf("人读的那句话应说清口令已失效，实际 %q", line)
	}
}

// TestRedFlowerDrawWithoutResultIsNotSuccess 钉住"`code == 0` 但 `result` 缺失"。
//
// 判据是**两个都成立**才算成功。只看 `code` 的话，这一支会记成一次成功，
// 而"换到几朵"是空的 —— 余额对不上却没有任何线索。
func TestRedFlowerDrawWithoutResultIsNotSuccess(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveUserInfoWithoutResultBody,
		liveRevivalBody,
		liveUserInfoBody(100),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("不该失败：%s", err.Message())
	}
	if report.Succeeded() != 0 {
		t.Fatalf("没有 result 不该记成成功，实际成功 %d 个", report.Succeeded())
	}
	if report.Draws[0].Outcome.Kind != upstream.RedFlowerDrawUnparsable {
		t.Fatalf("结论应为 unparsable，实际 %v", report.Draws[0].Outcome.Kind)
	}
	// ⚠ 也**不该**被判无效（原项目这一支是"warn 后跳过、不判无效"，避免误伤）。
	if len(report.InvalidCodes) != 0 {
		t.Fatalf("信封不全不该被判成口令无效：%v", report.InvalidCodes)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ 没有口令时不该发 draw
// ─────────────────────────────────────────────────────────────────────────────

// TestRedFlowerEmptyCodesSkipsDraw 钉住"一个口令都没有"这条路径。
//
// 它是常态（没抓到口令、或口令都被过滤掉了）。这时候**不能**发 `draw`
// —— 发一个空口令回去必然是 `200108`，而那条错误会把"今天没抓到口令"
// 伪装成"口令抄错了"。
func TestRedFlowerEmptyCodesSkipsDraw(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveRevivalBody,
		liveUserInfoBody(100),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT), NewRedFlowerSession(nil))
	if err != nil {
		t.Fatalf("没有口令不该失败：%s", err.Message())
	}
	liveAssertTrace(t, client, []string{
		"user_info",
		"journaling", "journaling", "journaling", "journaling", "journaling",
		"first_participate",
		"revival_flowers",
		"user_info",
	})
	if len(report.Draws) != 0 {
		t.Fatalf("没有口令时不该有 draw 记录：%v", report.Draws)
	}
	if report.Gained() != 0 {
		t.Fatalf("没有口令时进账应为 0，实际 %d", report.Gained())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ④ 失败时的状态推进
// ─────────────────────────────────────────────────────────────────────────────

// TestRedFlowerSessionNextDoesNotAdvance 钉住 `Next` **不推进状态**。
//
// 这是本状态机唯一一条刻意的设计。如果 `Next` 顺手推进，那么"请求构造失败"
// 与"发出去失败了"会各自偷偷前进一次 —— 少投一个口令是**静默**的：
// 余额会告诉你有问题，但不会告诉你是哪一个。
func TestRedFlowerSessionNextDoesNotAdvance(t *testing.T) {
	session := NewRedFlowerSession([]string{"AAA"})

	step, _, err := session.Next(liveTestJWT)
	if err != nil {
		t.Fatalf("构造请求失败：%s", err.Message())
	}
	if step != StepBalanceBefore {
		t.Fatalf("第一步应为起始余额，实际 %v", step)
	}
	// 连叫三次，状态与返回值都不该变。
	for i := 0; i < 3; i++ {
		again, _, _ := session.Next(liveTestJWT)
		if again != StepBalanceBefore {
			t.Fatalf("第 %d 次重叫之后步骤变了：%v", i+1, again)
		}
		if session.Step() != StepBalanceBefore {
			t.Fatalf("第 %d 次重叫之后状态变了：%v", i+1, session.Step())
		}
	}
}

// TestRunRedFlowerStopsWithoutAdvancingOnSendFailure 钉住"发失败 ⇒ 停在这一步"。
//
// 用一个**空脚本**制造发送失败（[scriptedUpstream] 脚本用尽即报错），
// 这样断言的是"发送失败"这条路径本身，而不是某个具体的错误文案。
func TestRunRedFlowerStopsWithoutAdvancingOnSendFailure(t *testing.T) {
	client := &scriptedUpstream{} // 一条响应都没有 ⇒ 第一个请求就失败
	up := upstream.NewUpstream(client)

	session := NewRedFlowerSession([]string{"AAA"})
	_, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT), session)
	if err == nil {
		t.Fatal("发送失败必须返回错误")
	}
	if session.Step() != StepBalanceBefore {
		t.Fatalf("发送失败之后不该推进，实际停在 %v", session.Step())
	}
	// 错误里带步名 —— 六步里 ①②⑥ 打的是同一个接口，不带步名分不清是哪一次。
	if !strings.HasPrefix(err.Message(), "[user_info_before] ") {
		t.Fatalf("错误里应带步名，实际：%s", err.Message())
	}
}

// TestRunRedFlowerStopsOnUnparsableEnvelopeWithStepName 钉住"解析失败 ⇒ 中止且带步名"。
//
// 六步里每一步的响应都带着后面要用的信息。解析失败意味着"上游换了信封形状"
// 或"我们打错了端点"，继续跑只会把同一个问题重复六遍，而日志里看不出
// 是从哪一步开始的。
func TestRunRedFlowerStopsOnUnparsableEnvelopeWithStepName(t *testing.T) {
	client := &scriptedUpstream{responses: []string{liveUserInfoWithoutResultBody}}
	up := upstream.NewUpstream(client)

	session := NewRedFlowerSession([]string{"AAA"})
	_, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT), session)
	if err == nil {
		t.Fatal("`result` 缺失必须返回错误")
	}
	if session.Step() != StepBalanceBefore {
		t.Fatalf("解析失败之后不该推进，实际停在 %v", session.Step())
	}
	if !strings.HasPrefix(err.Message(), "[user_info_before] ") {
		t.Fatalf("错误里应带步名，实际：%s", err.Message())
	}
}

// TestRunRedFlowerDoesNotFailOnMissingRevivalFlowers 钉住⑤是"顺便看一眼"。
//
// 复活花**不是余额**，拿不到它不该让整条链失败 —— 六步里只有它纯属附带。
func TestRunRedFlowerDoesNotFailOnMissingRevivalFlowers(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveUserInfoBody(100),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawOKBody(5),
		`{"code":0,"msg":"success","result":{"unexpected":true}}`, // ⑤ 形状变了
		liveUserInfoBody(105),
	}}
	up := upstream.NewUpstream(client)

	report, err := RunRedFlower(context.Background(), flowerSource(up, liveTestJWT),
		NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("复活花解析失败不该让整条链失败：%s", err.Message())
	}
	if report.RevivalFlowers != 0 {
		t.Fatalf("复活花拿不到时应留 0，实际 %d", report.RevivalFlowers)
	}
	// ⭐ 关键是⑥照样跑了 —— 余额才是结论。
	if report.Gained() != 5 {
		t.Fatalf("⑥ 应照常执行，进账 5，实际 %d", report.Gained())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑤ 纯函数：重听的两道边界
// ─────────────────────────────────────────────────────────────────────────────

// TestRetryPlanBoundaries 钉住"什么时候才值得重听一次"。
//
// 一次完全正常的运行不该多听 60 秒 —— 这是"没有无效口令就不重听"的全部意义。
func TestRetryPlanBoundaries(t *testing.T) {
	cases := []struct {
		name    string
		first   []string
		invalid []string
		retry   bool
		exclude []string
	}{
		{
			name:  "没有无效口令 ⇒ 不重听",
			first: []string{"AAA", "BBB"},
		},
		{
			name:  "全部无效 ⇒ 重听且排除全部",
			first: []string{"AAA", "BBB"}, invalid: []string{"AAA", "BBB"},
			retry: true, exclude: []string{"AAA", "BBB"},
		},
		{
			// ⚠ 这一条是重点：排除表里**同时**有成功的 AAA 与失败的 CCC。
			// 只排除失败的，第二次会把 AAA 再投一遍、换回 `200112`，
			// 看起来像"口令坏了"。
			name:  "部分无效 ⇒ 成功的也一起排除",
			first: []string{"AAA", "BBB", "CCC"}, invalid: []string{"CCC"},
			retry: true, exclude: []string{"AAA", "BBB", "CCC"},
		},
		{
			name:  "第一批为空但被判无效 ⇒ 仍要重听",
			first: nil, invalid: []string{"CCC"},
			retry: true, exclude: []string{"CCC"},
		},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			retry, exclude := RetryPlan(testCase.first, testCase.invalid)
			if retry != testCase.retry {
				t.Fatalf("retry 应为 %v，实际 %v", testCase.retry, retry)
			}
			if !liveStringsEqual(exclude, testCase.exclude) {
				t.Fatalf("排除表不符\n实际: %v\n期望: %v", exclude, testCase.exclude)
			}
		})
	}
}

// TestPickNewCodesExcludesTriedAndInvalid 钉住"重听之后真正投哪些"。
//
// 与 [RetryPlan] 用的是同一份依据，但用途不同：那个决定"听的时候排除谁"，
// 这个决定"投的时候投谁"。两者分开写是因为**漏掉任一侧**都会表现为
// "口令投了两遍"或"新口令没投"。
func TestPickNewCodesExcludesTriedAndInvalid(t *testing.T) {
	cases := []struct {
		name     string
		retried  []string
		tried    []string
		invalid  []string
		expected []string
	}{
		{
			name:     "全新口令照投",
			retried:  []string{"DDD"},
			tried:    []string{"AAA"},
			invalid:  []string{"BBB"},
			expected: []string{"DDD"},
		},
		{
			name:     "已成功的不再投",
			retried:  []string{"AAA", "DDD"},
			tried:    []string{"AAA"},
			expected: []string{"DDD"},
		},
		{
			name:     "无效的也不再投（重听又听到了同一个）",
			retried:  []string{"BBB", "DDD"},
			tried:    []string{"AAA"},
			invalid:  []string{"BBB"},
			expected: []string{"DDD"},
		},
		{
			name:     "重听结果全被排除 ⇒ 空",
			retried:  []string{"AAA", "BBB"},
			tried:    []string{"AAA"},
			invalid:  []string{"BBB"},
			expected: nil,
		},
		{
			name:     "重听结果里有重复 ⇒ 只投一次",
			retried:  []string{"DDD", " DDD "},
			expected: []string{"DDD"},
		},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			got := PickNewCodes(testCase.retried, testCase.tried, testCase.invalid)
			if !liveStringsEqual(got, testCase.expected) {
				t.Fatalf("应投口令不符\n实际: %v\n期望: %v", got, testCase.expected)
			}
		})
	}
}

// TestMergeKoulingTrimsDedupesAndDropsEmpty 钉住合并语义。
//
// 两个来源（WebSocket 弹幕 / 小红书主页）**地位相同**：任一来源为空
// 不影响另一个；顺序保留（"先听到的先投"）。
func TestMergeKoulingTrimsDedupesAndDropsEmpty(t *testing.T) {
	got := MergeKouling(
		[]string{"  AAA  ", "", "BBB"},
		nil,
		[]string{"BBB", "CCC"},
	)
	if !liveStringsEqual(got, []string{"AAA", "BBB", "CCC"}) {
		t.Fatalf("合并结果不符：%v", got)
	}
	if out := MergeKouling(nil, nil); len(out) != 0 {
		t.Fatalf("两个来源都为空时应返回空：%v", out)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑥ 门：有没有值得听的口令直播
// ─────────────────────────────────────────────────────────────────────────────

// TestJudgeKoulingGateSeparatesNobodyLiveFromSomeoneElseLive 钉住两种"没有"。
//
// ⚠ 实拍踩过：`tabContentList` 里房间 147490「福建移动」**在播**，
// 但主播不是"中国移动云盘" —— 那一轮一条口令都听不到。
// 如果只报 `Active=false`，"今天没有直播"与"有人在播但不是那个主播"
// 会被混成同一句，而它们的下一步完全不同（一个是等，一个是别等了）。
func TestJudgeKoulingGateSeparatesNobodyLiveFromSomeoneElseLive(t *testing.T) {
	// 实拍那天的形状：一间未开播（status 0），一间别的主播在播（status 1）。
	notStarted := upstream.LiveRoom{
		ID: 147608, Status: upstream.LiveRoomStatusNotStarted,
		AnchorNickname:  upstream.KoulingAnchor,
		ExpectStartTime: "2026-09-23 14:30:00",
	}
	otherAnchor := upstream.LiveRoom{
		ID: 147490, Status: upstream.LiveRoomStatusLive,
		AnchorNickname: "福建移动",
	}

	gate := JudgeKoulingGate([]upstream.LiveRoom{notStarted, otherAnchor})
	if gate.Active {
		t.Fatal("主播不是那个昵称，门不该开")
	}
	if gate.LiveCount != 1 {
		t.Fatalf("应有 1 间在播（别的主播），实际 %d", gate.LiveCount)
	}
	if len(gate.Rooms) != 0 {
		t.Fatalf("通过筛选的房间应为空，实际 %v", gate.Rooms)
	}

	// 谁都没在播：LiveCount 为 0 —— 与上面那条区分开。
	quiet := JudgeKoulingGate([]upstream.LiveRoom{notStarted})
	if quiet.Active || quiet.LiveCount != 0 {
		t.Fatalf("谁都没在播时应是 (false, 0)，实际 (%v, %d)", quiet.Active, quiet.LiveCount)
	}

	// 正主在播：门开。
	onAir := upstream.LiveRoom{
		ID: 147606, Status: upstream.LiveRoomStatusLive,
		AnchorNickname: upstream.KoulingAnchor,
	}
	open := JudgeKoulingGate([]upstream.LiveRoom{notStarted, otherAnchor, onAir})
	if !open.Active || open.LiveCount != 2 {
		t.Fatalf("正主在播时门应开、LiveCount 应为 2，实际 (%v, %d)", open.Active, open.LiveCount)
	}
	if len(open.Rooms) != 1 || open.Rooms[0].ID != 147606 {
		t.Fatalf("应只挑出正主那间房，实际 %v", open.Rooms)
	}
}

// TestJudgeKoulingGateKeepsAnchorEqualityExact 钉住判据是**逐字相等**。
//
// 昵称上带个后缀（"中国移动云盘官方"）就不是它 —— 这个判定只能靠
// 实拍那张表，所以用一个"很像但不是"的取值把它钉住。
func TestJudgeKoulingGateKeepsAnchorEqualityExact(t *testing.T) {
	almost := upstream.LiveRoom{
		ID: 1, Status: upstream.LiveRoomStatusLive,
		AnchorNickname: upstream.KoulingAnchor + "官方",
	}
	if gate := JudgeKoulingGate([]upstream.LiveRoom{almost}); gate.Active {
		t.Fatal("昵称只是前缀相同，不该开门")
	}
}

// TestRedFlowerReportGainedUsesBothReadings 钉住"进账"只能由两次读数相减得出。
//
// 每个口令的 `flowerNum` 加起来会漏掉首次参与奖那几朵，而它**不在**任何一个
// 口令的结果里 —— 那正好是"总对不上"这类问题最常见的来源。
func TestRedFlowerReportGainedUsesBothReadings(t *testing.T) {
	report := RedFlowerReport{
		BalanceBefore: upstream.RedFlowerBalance{FlowerCnt: 235},
		BalanceAfter:  upstream.RedFlowerBalance{FlowerCnt: 251},
		Draws: []RedFlowerDrawRecord{
			{Code: "AAA", Outcome: upstream.RedFlowerDrawOutcome{
				Kind:   upstream.RedFlowerDrawSucceeded,
				Result: upstream.RedFlowerDrawResult{FlowerNum: 5},
			}},
			{Code: "BBB", Outcome: upstream.RedFlowerDrawOutcome{
				Kind:   upstream.RedFlowerDrawSucceeded,
				Result: upstream.RedFlowerDrawResult{FlowerNum: 8},
			}},
		},
	}
	// 口令加起来是 13，而真实进账是 16 —— 差的 3 朵来自首次参与奖。
	if report.Gained() != 16 {
		t.Fatalf("进账应由两次读数相减得出（16），实际 %d", report.Gained())
	}
	if report.Succeeded() != 2 {
		t.Fatalf("成功口令数应为 2，实际 %d", report.Succeeded())
	}
}
