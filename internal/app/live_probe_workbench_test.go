package app

// 活体探针：**玩法 1028「AI 工作台」**那一族（四枪，2026-09-26 落码时**全部零实拍**）。
//
// # 为什么这一族单独一份，不塞进 [TestLiveProbeMail139Family]
//
// 那一份的抬头写着"139 邮箱那一族的四枪"，而本族虽然**借**邮箱会话开头（①② 两口），
// 它的四枪打在**另外三台**主机上、且用的是从邮箱会话**派生**出来的第二套凭据
// （DES 解出的手机号 + ①′ 给的 token ⇒ RSA / Basic）。两族混在一处，
// 读读数时会把"邮箱那侧认这张会话"与"AI 那侧认这枚 Basic 派生"读成同一件事 ——
// 而本探针存在的理由正是把它们分开。
//
// # 逐枪要拿的那几格（全部对应 `docs/pending-decisions.md` 里 1028 那一档）
//
//   - **①′** `disk:queryUserToken`：这一口在**整个仓库里是第二次**被打（第一次是 1020 那族
//     的 ⑧ 同主机不同路由）⇒ 先看**体形状到底有没有** `var.data.data.account/token/userDomainId`
//     那三格（解析器按旧 TS 的文字形状写，零实拍）；再看 DES 解出来的是不是一枚手机号
//     （只报长度与"像不像 11 位数字"，值不进日志）。
//   - **②′** `getUserTaskAndCloudStatusExternal`：**不带任何鉴权头**、身份全在 body 那枚
//     RSA 密文里 ⇒ 这一格的读数就是"上游认不认这种自证身份"。它回什么 `code` /
//     有没有 `result` / 两枚状态各是几，决定本层走哪一档念法（三档见
//     [`task_matrix_workbench.go`](task_matrix_workbench.go) 文件头第一条）。
//   - **③′** `chat/v2/add`：与 585 那一族的 ②**同 URL 不同形状**（头集、体形状、SSE 读法
//     三处都不同）⇒ 这一发是"两套读法各自能不能在真体上读出东西"的唯一判据。
//   - **④′** 与 ②′ 同一口的第二遍：只回答"问完之后上游那条进度动没动"。
//
// # ⚠ 写的那一格只发一次
//
// ①′②′ 是**读**，探针直发一档 + 生产路径再发一档（多发只是多敲一下，与邮箱那族同一口径）；
// ③′ 是**写**（在 AI 工作台那侧留下一条会话记录，而本仓零实现"删会话"那一口）⇒
// 它**只由生产路径发一次**。所以"体形状"那一格在 ③′ 这里交回的是 [upstream.AIWorkbenchChatRead]
// 的读数（事件数 / 末条 code / 内容），不是原始 SSE —— ⚠ 那一格仍是缺口，
// 要拿它得另配一次"直发但不发写"的路子（登记在待拍板那一档）。
//
// ⚠ 与 [TestLiveProbeMail139Family] 同样：**这一族没有断言**，它是仪器不是回归测试。
// 判据落在 `docs/status.md` 与 `docs/pending-decisions.md`。

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/upstream"
)

// workbenchPhoneShape 判"DES 解出来的是不是一枚 11 位手机号形状"—— 只回答像不像，
// 值不打印（这一族最容易漏的一格：手机号是**从上游响应里解出来的用户身份**）。
var workbenchPhoneShape = regexp.MustCompile(`^1[0-9]{10}$`)

func TestLiveProbeAIWorkbenchFamily(t *testing.T) {
	run, account, sink, client := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())
	executor := newTaskExecutor(run, opening, account)

	// ── 邮箱会话（本族的前置，两口都只读；与 1020 / 1014 同一份 [mailSessionFor]）──
	session, sessionFired, sessionProblems := executor.mailSessionFor(ctx)
	sink.say("前置：邮箱会话换过枪=%v 失败 %d 条", sessionFired, len(sessionProblems))
	for _, problem := range sessionProblems {
		sink.say("前置原因：%s", sink.mask(problem))
	}
	if len(sessionProblems) > 0 {
		sink.say("⇒ 没有会话，本族四枪都不发（这一族的边界与 1020 / 1014 同一条）")
		t.FailNow()
	}

	// ── ①′ 派生凭据：先直发拿**体形状**，再让生产解析器读它 ──
	//
	// ⚠ 这一口是**读**（只是把手机号与 token 念给本层），直发一次不改变用户任何东西。
	cguid, rnd := mailDiskRandoms(run.Random)
	tokenRequest, perr := upstream.AIWorkbenchQueryUserTokenRequest(session, cguid, rnd)
	if perr != nil {
		sink.say("①′ 构造失败：%s ⇒ ②′③′④′ 的直发档不发", perr.Error())
	} else {
		tokenResponse, serr := client.Send(ctx, tokenRequest)
		sink.dump("①′ queryUserToken（直发，看体）", tokenResponse)
		if serr != nil {
			sink.say("①′ 直发错误：%s", sink.mask(serr.Error()))
		}
		directAuth, pererr := upstream.ParseAIWorkbenchUserToken(tokenResponse)
		if pererr != nil {
			sink.say("①′ 我们的解析器读不出来：%s ⇒ 这一口的响应形状仍是零样本", pererr.Error())
		} else {
			// 先登记，后打印：这三枚都是本族的凭据（手机号 / token / Basic 整串）。
			sink.see(directAuth.Phone.Value())
			sink.see(directAuth.Token.Value())
			sink.see(directAuth.Authorization.Value())
			phone := directAuth.Phone.Value()
			sink.say("①′ 解析结果：手机号长度 %d、形状像手机号=%v、token 长度 %d、"+
				"userDomainId 长度 %d、Basic 整串长度 %d（值一律不打印）",
				len(phone), workbenchPhoneShape.MatchString(phone),
				len(directAuth.Token.Value()), len(directAuth.UserDomainID),
				len(directAuth.Authorization.Value()))
		}
	}
	// 生产路径那一档（后面 ②′③′④′ 要用它，所以这一发**必须**发）。
	auth, aerr := run.Upstream.AppAIWorkbenchUserToken(ctx, session, cguid, rnd)
	if aerr != nil {
		sink.say("①′（生产路径）失败：%s ⇒ 后面的整条玩法也一定挂在这一枪", sink.mask(aerr.Error()))
	} else {
		sink.see(auth.Phone.Value())
		sink.see(auth.Token.Value())
		sink.see(auth.Authorization.Value())
		sink.say("①′（生产路径）可用=%v、userDomainId 长度 %d", auth.OK(), len(auth.UserDomainID))
	}

	// ── ②′ 前置状态（**只读**，不带鉴权头）：直发一档拿形状，生产一档拿判定 ──
	if auth.OK() {
		statusRequest, perr := upstream.AIWorkbenchStatusRequest(auth.Phone,
			upstream.AIWorkbenchTaskID, upstream.AIWorkbenchSourceChannel, run.Random)
		if perr != nil {
			sink.say("②′ 构造失败：%s", perr.Error())
		} else {
			// ⚠ 这一格是"我们发出去的东西"，不是凭据 ⇒ 可以念，但要念的是**形状**：
			// RSA 密文本身不该进日志（它是可解回手机号的那一枚的密文形态）。
			body := string(statusRequest.Body)
			names := strings.ToLower(strings.Join(headerNames(statusRequest), ","))
			sink.say("②′ 请求：POST %s，头 %d 枚（带 authorization=%v），体 %d 字节，"+
				"体内出现手机号明文=%v",
				statusRequest.URL, len(statusRequest.Headers),
				strings.Contains(names, "authorization"),
				len(body), strings.Contains(body, auth.Phone.Value()))
			statusResponse, serr := client.Send(ctx, statusRequest)
			sink.dump("②′ 状态查询（直发，看体）", statusResponse)
			if serr != nil {
				sink.say("②′ 直发错误：%s", sink.mask(serr.Error()))
			}
			directStatus, pererr := upstream.ParseAIWorkbenchTaskStatus(statusResponse)
			if pererr != nil {
				sink.say("②′ 我们的解析器读不出来：%s", pererr.Error())
			} else {
				sink.say("②′ 解析结果：%s", workbenchStatusLine(directStatus))
			}
		}
		status, serr := run.Upstream.AppAIWorkbenchTaskStatus(ctx, auth,
			upstream.AIWorkbenchTaskID, upstream.AIWorkbenchSourceChannel, run.Random)
		if serr != nil {
			sink.say("②′（生产路径）：%s", sink.mask(serr.Error()))
		} else {
			sink.say("②′（生产路径）：%s", workbenchStatusLine(status))
		}
	}

	// ── ②′-对照（**只读**，同一口换 `taskId`）：把"这一条任务不认"与"这一整条路由不认"分开 ──
	//
	// 为什么要有这一档：②′ 真跑回的是 `code:"501" / msg:"任务不存在"`（两次都是），
	// 而这一句有**两种**解释 —— ① 这一口的身份是 RSA 手机号，它认得这个号、只是这个号
	// 今天没有 1028 这一条任务；② 这一口压根不认 `taskId` 这一格（参数名/形状不对），
	// 于是任何 id 都回"任务不存在"。两种解释对**门**的处置完全不同：
	// 前者意味着"任务在表里的那一天门才可能成立"，后者意味着"这一道门在本仓永远读不出来"。
	//
	// 分开它只要多发几发**读**枪：同一构造器换几枚 `taskId`（含一枚明显不存在的），
	// 看"501 任务不存在"是不是**逐字同形**。若某枚回 `result` ⇒ 是解释 ①；
	// 若全部 501 ⇒ 是解释 ②（或那一台整天只认 501，仍需在表里的日子复跑才能收口）。
	//
	// ⚠ 这里换的是**我们自己的**请求参数，不是"拿别人的号试"：身份仍是同一枚 RSA 手机号。
	// 585 / 106 / 1020 是本仓已接玩法的码（都在同一张任务表上），
	// `999999` 是刻意造的一枚不存在的码 —— 它是"这一句是不是通用兜底"的那一枚对照。
	for _, control := range []int64{585, 106, 1020, 999999} {
		controlRequest, perr := upstream.AIWorkbenchStatusRequest(auth.Phone, control,
			upstream.AIWorkbenchSourceChannel, run.Random)
		if perr != nil {
			sink.say("②′-对照 taskId=%d 构造失败：%s", control, perr.Error())
			continue
		}
		controlResponse, serr := client.Send(ctx, controlRequest)
		sink.dump(fmt.Sprintf("②′-对照 taskId=%d", control), controlResponse)
		if serr != nil {
			sink.say("②′-对照 taskId=%d 错误：%s", control, sink.mask(serr.Error()))
		}
		controlStatus, pererr := upstream.ParseAIWorkbenchTaskStatus(controlResponse)
		if pererr != nil {
			sink.say("②′-对照 taskId=%d 解析不出来：%s", control, pererr.Error())
			continue
		}
		sink.say("②′-对照 taskId=%d：%s", control, workbenchStatusLine(controlStatus))
	}

	// ── ③′ + ④′：**只走生产路径**（③′ 是写，这一趟只发一次）──
	//
	// [taskExecutor.runWorkbench] 自己会再发一遍 ①′②′（都是读，与上面两档同型），
	// 然后问那一句、等 5 秒、再看一眼。它的播报从这里进事件流 ⇒ 这三档读数就是
	// "程序自己说了什么"（AGENTS.md §1.14），也就是本族的验收判据。
	item := liveProbeTaskItem(upstream.AIWorkbenchTaskID, "AI 工作台")
	fired, problems := executor.runWorkbench(ctx, item)
	sink.say("整条玩法（1028）走生产路径：发过枪=%v 失败 %d 条", fired, len(problems))
	for _, problem := range problems {
		sink.say("原因：%s", sink.mask(problem))
	}
	// ⚠ 这一族不往网盘写东西 ⇒ 没有清理链要收（与 1014 那一档不同）。它的产物只有
	// "AI 工作台那侧的一次会话记录"，而本仓**没有**"删会话"那一口 ⇒ 下面这一行
	// 不代表"什么都没留下"。
	sink.say("边界：③′ 在 AI 那侧留下一条会话记录，**不可回滚**（本仓无删除那一口）⇒ " +
		"这一趟没有网盘产物要清，但也不是零产物")
}
