package app

// 139 邮箱那一半（玩法 **472 / 473「体验 139 邮箱」**）：四枪一串，跨两台主机。
//
//	① POST user-njs.yun.139.com/user/querySpecTokenV2   {toSourceId:"001003"}   → SSO 票
//	② POST mail.10086.cn/login/inlogin.action?cguid=…                            → sid + rmkey
//	③ GET  mail.10086.cn/login/autologin.action?sid=…&rmkey=…                    → 不判响应
//	④ POST caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskClickV2            → code=0
//
// 四枪的请求面与判定全在 `internal/upstream/app_mail139.go`（那里成文了哪两枪有实拍、
// 哪两枪零样本）。这一层只管**编排**：什么时候不发、发失败时念什么、错误的先后。
//
// # 为什么这一条与 1004 / 1020 / 1014 / 1028 只有"前三枪"是共用的
//
// 旧实现那四条各自在拿到会话之后打**自己那一口**（`mbox:compose` 真发一封信、
// `mailsms/sms/backup`、PDF 转换、AI 工作台），而那些口本仓一个都没有 ⇒ 它们仍留在
// [taskMatrixToolGaps]。本文件只兑现"体验邮箱"这一条：它后面那一发就是 ④，
// 而 ④ 是签到族自己的 click 的 V2 变体 —— 这一类枪本仓已经会打。
//
// ⚠ 本文件**没有** PE 活跃上报。真流量里 inlogin 之后确实紧跟一发 PE，但那一发的
// 发起方是"打开了某个邮箱页面"而不是任务 472；旧实现里 `triggerMailPEvent` 也只被
// 1004 那一族用。⇒ "472 要不要发 PE"登记成问句（`docs/pending-decisions.md`），不在此落码。
//
// # 播报为什么按"哪一枪挂了"分档
//
// ①②③ 挂的是邮箱那两侧的会话（④ 压根没发出去，任务自然没被认）；③ 挂了而 ④ 成功时，
// 这一条是"活跃没做成、但任务记上了"。两档的处置不同：前者去看邮箱侧的响应，
// 后者回签到族看 `58999`（坑表 #149）。混成一句"邮箱任务失败"等于两问都不答。
//
// # 这一族**不往用户账号留产物**
//
// 不传文件、不建笔记、不发信 ⇒ [TaskMatrixRunResult.CloudFileIDs] 那一条清理链与它无关，
// 链尾也不该为它发 `batchTrash`。它唯一的"产物"是 ④ 记上的那条任务进度，
// 而那一格**不可回滚**（没有撤销打卡那一口）。

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// mailSessionFor 取这一趟的邮箱会话（②的产物），**同一趟只换一次**（①②各一发）。
//
// 第二格 fired 是"这一句里有没有枪真的出过网"（口径见 [apply]）：① 已经发出去了，
// 哪怕 ② 没换到会话也算动过 —— 它真的把登录态发给了邮箱那台主机。
//
// ⚠ **只在成功时缓存**（照旧实现 `ctx.state.mail139`：它读的是
// `cached?.sid && cached?.rmkey`，而写入在 `loginEmail` 返回非 null 之后）。
// 失败不缓存的后果照它写清楚：下一任务（473）会**再走一遍** ①②。
// 不跟着改成"失败也记一笔、整趟放弃"的理由：票很短命（`app_mail139.go` 文件头引的实拍
// —— 换到 3 分 15 秒后重放就被拒 `200050409`），而失败原因里"被风控"与"这一次网络抖了"
// 在响应上分不开；一次失败拖垮整趟，等于把瞬时故障放大成"这个号今天没有邮箱任务"。
// 这一格最坏的表现是**多打两枪**，不是多写一份用户的账。
//
// ⚠ 下面两格"缺接线"的判断**未被断言**（AGENTS.md §1.3）：这一条链在**拉表**那一步就要
// 用随机源与时钟（`AppTaskMatrix` → [do] → 建会话），两者为 nil 时整趟早就 panic 在
// 到不了这里的地方 ⇒ 造不出一份"跑到这一句而 Random 是 nil"的夹具。留着它们的理由是
// [UserTimestamp] 与 [core.UUIDv4] 都是 nil 接口直取（**panic，不是回零值**），
// 而这一族将来可能被别的入口复用（不经过拉表）。**不要**据此以为"缺接线时会安全停下"
// —— 今天真实的表现在到达这里之前就有了。
func (e *taskExecutor) mailSessionFor(
	ctx context.Context,
) (upstream.MailSession, bool, []string) {
	if e.mailSession != nil && e.mailSession.OK() {
		return *e.mailSession, false, nil
	}
	if e.run.Random == nil {
		return upstream.MailSession{}, false, []string{
			"邮箱会话没换：随机源没接，造不出 ② 那一发的 cguid（本工具侧缺接线）",
		}
	}
	if e.run.Clock == nil {
		// ⚠ 必须先判 nil 再用它：[UserTimestamp] 是 `clock.Now()` 直取，接口为 nil 时
		// **panic**（不是回 0）—— 与 [nowMillis] 那里同一处置，理由也在那儿：
		// 一个零值签名时间戳会照常发出去，而上游看到的是 1970 年，比"这条没做成"难查得多。
		return upstream.MailSession{}, false, []string{
			"邮箱会话没换：时钟没接，① 的签名时间戳取不到（本工具侧缺接线）",
		}
	}
	timestamp := UserTimestamp(e.run.Clock)
	ssoToken, perr := e.src.up.AppQuerySpecToken(ctx, e.opening.Profile,
		timestamp, e.opening.Credential.Authorization(), upstream.Mail139SourceID)
	if perr != nil {
		return upstream.MailSession{}, true, []string{"换邮箱用的 SSO 票（①）失败：" + perr.Error() +
			" ⇒ ②③④ 不发，这一条没往邮箱那边写任何东西"}
	}
	session, perr := e.src.up.AppMail139Login(ctx, ssoToken, core.UUIDv4(e.run.Random))
	if perr != nil {
		// ⚠ 这一句里的错误文本已由协议层脱敏（`ParseMailSession` 那次 `redactSecrets`）；
		// 本层**不再** echo `ssoToken`，也不自己拼响应体（那正是"顺手 marshal 一下"的入口）。
		return upstream.MailSession{}, true, []string{"邮箱会话（②）没换到：" + perr.Error() +
			" ⇒ ③④ 不发，不会打这一条任务的卡"}
	}
	e.mailSession = &session // 只有成功才缓存（见函数头）
	return session, true, nil
}

// runMailExperience 跑"体验 139 邮箱"这一条玩法，交回（发过枪没有，失败原因）。
//
// 472 与 473 共用这一个函数：它们在任务表里是同名同组的两条，差别只在 ④ 里那个
// `taskId`（与 106 / 522 共用上传那一族同理）。
//
// ⚠ 顺序照旧实现：会话（①②）→ 活跃（③）→ 打卡（④），而 **③ 失败不挡 ④**。
// 旧实现那里是 `tryRequest` 吞掉错误、④ 无条件照发 ⇒ 我们跟着发，但把 ③ 的失败
// 念出来（本仓反复批评旧实现"上报失败却静默"，见 `app_mail139.go` 文件头）。
// 反过来说，③ 失败而 ④ 成功时这一条仍算"做了、上游认了"—— 认没认由**复查那趟**说话，
// 不由这一层猜。
func (e *taskExecutor) runMailExperience(
	ctx context.Context, item upstream.TaskV2Item,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	session, fired, problems := e.mailSessionFor(ctx)
	if len(problems) > 0 {
		// 到这里必然是"①② 那一档没过去"：[mailSessionFor] 只有成功才交回可用会话，
		// 所以**不需要**再写一句"会话不可用就跳过"的兜底 —— 那一支没有可达路径，
		// 留着只会让人以为"发过 ③ 却没换到会话"是一种真实状态（AGENTS.md §1.3）。
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱任务失败 %s %d：%s", name, id, problems[0]))
		return fired, problems
	}
	// ③ 活跃那一发：它**不判响应**（为什么写不判，协议层头注成文了），
	// 所以这里唯一的失败是"没发出去 / 非 2xx"。
	fired = true
	if aerr := e.src.up.AppMailAutoLogin(ctx, session,
		e.opening.Credential.Authorization()); aerr != nil {
		problem := "邮箱活跃（③）失败：" + aerr.Error() + " ⇒ 仍照旧实现往下发 ④"
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱任务失败 %s %d：%s", name, id, problem))
		problems = append(problems, problem)
	} else {
		e.run.progress(core.LevelInfo, fmt.Sprintf("邮箱已活跃 %s %d", name, id))
	}
	// ④ 打卡：走签到族那套"拿票 → 发 → 撞 90001 刷一次重放一次"（[do]）。
	// ⚠ 这一发的 jwttoken 是**公共来源**（依据与"将来拍到特有取法要怎么改"写在
	// `upstream.MailMarketTaskClickRequest` 的头注，AGENTS.md §1.12）。
	_, cerr := do(ctx, e.src, func(s SignInSession) (struct{}, *core.Error) {
		return struct{}{}, e.src.up.AppMailMarketTaskClick(
			ctx, s.jwt, s.referer, upstream.MarketName, id)
	})
	if cerr != nil {
		problem := "邮箱市场打卡（④）失败：" + cerr.Error() +
			" ⇒ 前三枪做了、这一条任务上游没记上（" +
			"「活跃过」与「任务被认」是两件事，别把这一句读成邮箱没打开）"
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱任务失败 %s %d：%s", name, id, problem))
		return true, append(problems, problem)
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("邮箱市场已打卡 %s %d", name, id))
	return true, problems
}
