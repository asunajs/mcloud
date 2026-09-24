// 「直播口令 → 小红花」**跑一个账号**的唯一实现。
//
// # 为什么流程住在这里，而不是两个产物各写一遍
//
// CLI 的 `mcloud live-room` 与 WebUI 的 `POST /api/runs` 跑的是**同一条链**：
// 开门 → 收口令 → 投 → （有无效口令时）重听一次再投。这条链上有三处
// "少写一句就静默出错"的地方：
//
//   - 重听时的 `exclude` 必须含**第一轮全部**口令（含已成功的），否则第二轮会把
//     同一批再投一遍 —— 成功的被重复计进 `Draws`、被判无效的又进一次 `InvalidCodes`，
//     归因被污染，还会再触发一轮 60 秒重听；
//   - 两轮结果合并时起始余额取**第一轮**、终值取**第二轮**，取错的表现为
//     "进账"少算一轮，而那个数字看着完全正常；
//   - 无效口令要取**并集**，取第二轮那份会让"为什么少了一朵"这件事消失。
//
// ⚠⚠ 上面第一条的**理由**在 2026-09-23 被纠正过，别改回去：
//   - **不是**"口令会被消耗"。口令是**全场通用**的（五条事实见
//     `internal/upstream/live_flower.go` 文件头）—— 所有用户共用同一批，不存在"用掉"；
//   - **是**"同一账号重复提交同一条**正确**口令会回 `200112`（已兑换）"。
//     这个断言本身**成立**（用户口径 2026-09-23），但它说的是**按账号去重**，
//     不是"口令被消耗"。排除已成功的口令，既避免把一条"已兑换"当成新结果，
//     也保持 `Draws` / `InvalidCodes` 的归因干净。
//
// ⚠ `200112` 仍**零实拍**：语义来自用户口述 + 原项目 `standalone.ts:72` 字面量，
// 接口归属是 `redFlower/draw?code=`。哪天拍到样本，把上面这句从"用户口径"改成"实拍"。
//
// 两条产物各写一遍的话，这三处就会各自漂移，而漂移**不会让任何测试变红**
// —— 它只表现为"CLI 跑出来的数与界面显示的数不一样"。所以这里只留一份，
// 两个产物各自只保留"把用户的选择翻成参数、把结果翻成人话"。
//
// # 这一层不做什么
//
//   - **不判门的开合** —— [JudgeKoulingGate]；
//   - **不判"要不要重听"** —— [RetryPlan] / [PickNewCodes]；
//   - **不判六步链走到哪** —— [RedFlowerSession]；
//   - **不解析响应** —— `internal/upstream`；
//   - **不打印、不知道账号是第几个** —— 给人看的前缀（`[1 昵称]`）由调用方加，
//     过程话通过 [LiveRoomRun.Progress] 递出去（CLI 写 stderr，服务端发 SSE 事件）。
package app

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// LiveRoomParams 是一趟运行里**由使用者决定**的那几件事。
//
// ⚠ 构造出来不等于"这组参数能跑"：组合是否成立由 [LiveRoomParams.Check] 判
// （判据只有一份），而**报不报错、怎么说**留在两个产物各自的边界上。
type LiveRoomParams struct {
	// Codes 是手工口令。**非空就不再收集**（这一趟没有口令要听弹幕）。
	Codes []string
	// ListenSeconds 是自动收集时听弹幕的时长（秒）。只在真的要收集时用到。
	ListenSeconds int
	// KoulingOnly 是"只听不投"：收完就结束，**一个口令都不投给上游**。
	//
	// ⚠ 措辞是"不投"，不是"不消耗"—— 口令全场通用、不存在消耗（见
	// `internal/upstream/live_flower.go` 文件头五条事实②）。早先这里写的是
	// "一个口令都不消耗"（2026-09-23 改），那会把"消耗"这个错误概念又带回来。
	KoulingOnly bool
	// SkipKouling 是"只用给定口令"：不开弹幕。它为真时 Codes 必须非空。
	SkipKouling bool
}

// LiveRoomFault 是参数组合上的冲突**种类** —— 只有种类，没有措辞。
type LiveRoomFault int

const (
	// LiveRoomFaultNone 表示这一组参数成立。
	LiveRoomFaultNone LiveRoomFault = iota
	// LiveRoomFaultExclusiveSwitches：「只听不投」与「只用给定口令」同时为真。
	LiveRoomFaultExclusiveSwitches
	// LiveRoomFaultSkipWithoutCodes：关掉了唯一的口令来源，又没给手工口令。
	LiveRoomFaultSkipWithoutCodes
	// LiveRoomFaultListenNotPositive：要听弹幕，可时长不是正数。
	LiveRoomFaultListenNotPositive
)

// Check 判这一组参数**是否成立**（纯函数）。
//
// # 为什么判据在这层、措辞不在
//
// 三条规则本身只有一个答案，两个产物各写一遍必然漂移（一处放行、一处拒绝，
// 而"哪一处才是对的"没人能从那两份代码里读出来）。措辞则**必须**各写一遍：
// CLI 要说 `--kouling-only` 并退 2，服务端要说"只听不投"并回 400，
// 把 flag 名塞进领域层会让界面上输出一句没有 flag 的话。
//
// ⚠ 第三条不看 `SkipKouling`：时长非正在**任何**情况下都不是合法输入，
// 与 CLI 那一条的口径一致（见 `docs/cli-usage.md` 的 `--listen-seconds`）。
func (p LiveRoomParams) Check() LiveRoomFault {
	switch {
	case p.KoulingOnly && p.SkipKouling:
		return LiveRoomFaultExclusiveSwitches
	case p.SkipKouling && len(p.Codes) == 0:
		return LiveRoomFaultSkipWithoutCodes
	case p.ListenSeconds <= 0:
		return LiveRoomFaultListenNotPositive
	}
	return LiveRoomFaultNone
}

// LiveRoomRun 是跑一个账号需要的**世界**。
//
// `Device` 与 `Driver` 由调用方在**账号循环之外**解析一次：同一个安装就该是
// 同一台设备。放进循环里会让每个账号拿到不同的 `device_short`（不传时现生成），
// 于是"一次运行"对应好几种设备形态 —— 与登录时的形态也对不上。
type LiveRoomRun struct {
	Store  core.StateStore
	Scope  core.Scope
	Driver LiveRoomDriver
	Device upstream.AppDevice
	Params LiveRoomParams

	// Progress 收"给人看的过程话"，可为 nil。
	//
	// 它有第二个消费者：服务端把它直接接到 [core.NewEventLog] 上，于是界面在
	// 任务跑的过程中就能看到"开门中 / 在播房间 X / 有 2 个口令无效"。
	// ⚠ 级别由本层决定而不是由调用方补：`warn` 那几条是**取舍告知**
	// （只听第一间、门没开），CLI 与服务端必须看到同一份判级，否则界面上一片绿、
	// 终端里一片黄。
	Progress RunProgress

	// Sessions 是进程内的会话票池（同一账号这一份票跨轮复用）。可为 nil ⇒ 回落成
	// "这一轮那一条"，理由与接线点见 [SignInRun.Sessions] 的同一句。
	//
	// ⚠ 这一条链今天**借用**签到页那一份配方（`RecipeSignInPage`）——
	// 见下面取票那一段的"jwttoken 使用公共来源"。
	Sessions *SessionPool
}

// LiveRoomAccountResult 是一个账号这一趟的全部产出。
type LiveRoomAccountResult struct {
	// Gate 是门的判定（含"两种没有"的区分：谁都没在播 / 有在播但不是那个主播）。
	Gate KoulingGate
	// HasJWT 表示门那三步真的换到了直播侧凭据。
	//
	// ⚠ 只存"有没有"，不存那一串：它是凭据，不该顺着进度与结果流进日志。
	HasJWT bool
	// Codes 是这次实际拿到并投出去的口令（`KoulingOnly` 时是收到的）。
	Codes []string
	// Report 是六步链的结论；nil 表示**没投**（只听不投、门没开、或一个口令都没收到）。
	Report *RedFlowerReport
	// Retried 表示触发过"重听一次"。
	Retried bool
	// Params 是这一趟用的参数（回显给调用方拼结论用，不是新事实）。
	Params LiveRoomParams
}

// RunAccount 跑**一个账号**：换凭据 → 开门 → 收口令 → 投 → （必要时）重听再投。
//
// 返回的 `LiveRoomAccountResult` 在出错时也尽量带已经拿到的部分（门、口令），
// 调用方据此能说出"崩在第几步"，而不是一句错误串。
func (r LiveRoomRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (LiveRoomAccountResult, *core.Error) {
	outcome := LiveRoomAccountResult{Params: r.Params}

	// 设备形态必须与登录时一致（同一个文件 → 同一个 `deviceId`）。
	profile, perr := ProfileWithStoredDomainID(ctx, r.Store, r.Scope,
		upstream.NewAppProfile(r.Device), account)
	if perr != nil {
		return outcome, perr
	}

	credential, perr := CredentialFromAccount(ctx, r.Store, r.Scope, account)
	if perr != nil {
		return outcome, perr
	}

	r.progress(core.LevelInfo, "开门中…")
	opened, perr := OpenKoulingGate(ctx, r.Driver.Upstream, profile,
		UserTimestamp(r.Driver.Clock), credential.Authorization())
	outcome.Gate = opened.Gate
	outcome.HasJWT = opened.JWT != ""
	if perr != nil {
		return outcome, perr
	}
	// `Phone` 是**账号相关**的那一份（`guid.name` / `userName` / `invoker` 三处同一个值），
	// 所以它进的是这里的局部副本，而不是调用方在循环外建的那份 driver。
	driver := r.Driver
	driver.Phone = credential.Phone()

	// 手工口令优先：给了 `Codes` 就不再走收集（`SkipKouling` 只是把它变成硬要求）。
	if manual := MergeKouling(r.Params.Codes); len(manual) > 0 {
		outcome.Codes = manual
	}

	if !r.Params.SkipKouling && len(outcome.Codes) == 0 {
		if !opened.Gate.Active {
			// ⭐ 门的两种"没有"分开说 —— 它们的下一步完全不同。
			if opened.Gate.LiveCount == 0 {
				r.progress(core.LevelWarn,
					"今天没有直播（"+upstream.KoulingAnchor+" 一间都没在播）")
			} else {
				r.progress(core.LevelWarn, fmt.Sprintf(
					"有 %d 间在播，但没有一间是「%s」", opened.Gate.LiveCount, upstream.KoulingAnchor))
			}
			return outcome, nil
		}
		// ⚠⚠ **只听第一间** —— 这是**刻意的取舍**，不是漏了循环（2026-09-22 定）。
		//
		// 原项目是 `for (const room of rooms)`，把该主播**每一间**在播房间都听一遍
		// （`tasks/live-room-kouling.ts` 的 `getWebSocketKouling`，逐间 `await`）。
		// 这里只取 `Rooms[0]`：该主播同时开两间以上是**少数情况**，而顺序听 N 间会把
		// 墙钟时间变成 N × `ListenSeconds`（默认 60 s ⇒ 3 间就是 3 分钟）。
		//
		// ⚠ 代价必须说清：**同时开两间时，第一间之外的口令会被整场漏掉** ——
		// 而这条与 [upstream.LiveTabIDReplay] 那边"漏掉整场口令"是同一个错，
		// 只是这里换到的是"少连一次 WS"。这一支不是"更省事所以对"，
		// 是**拿"漏口令"换"墙钟时间"**。
		// 真需要两间都听时：把这里换成对 `opened.Gate.Rooms` 的循环即可 ——
		// 收集那一步（[CollectKouling]）本来就是并发的，不必改它。
		//
		// 所以**只要房间数 > 1 就必须说出来**：不说的话用户看到的是
		// "在播房间 147606，听 60 秒"，会以为已经听全了。
		if len(opened.Gate.Rooms) > 1 {
			r.progress(core.LevelWarn, fmt.Sprintf(
				"⚠ 该主播有 %d 间在播，本次**只听第一间**（其余 %d 间这一趟不覆盖）",
				len(opened.Gate.Rooms), len(opened.Gate.Rooms)-1))
		}
		roomID := opened.Gate.Rooms[0].ID
		r.progress(core.LevelInfo, fmt.Sprintf("在播房间 %d，听 %d 秒弹幕…",
			roomID, r.Params.ListenSeconds))
		codes, perr := driver.Collect(ctx, opened.JWT, roomID, r.listenFor())
		if perr != nil {
			return outcome, perr
		}
		outcome.Codes = codes
	}

	if r.Params.KoulingOnly || len(outcome.Codes) == 0 {
		return outcome, nil
	}

	// 六步链整个一族都在 `m.mcloud.139.com/ycloud`（与签到同域同族，公共头里那个
	// `activityid: sign_in_3` 就是证据），它要的 `jwttoken` 是**云盘侧 `tyrzLogin` 那一份**，
	// 不是门换到的直播侧 JWT —— 拿后者去读实测回 `90001 未登录`
	// （2026-09-22 WebUI 探针第一跑，两个账号都是那一句；判据见 [ProbeLiveRoom] 的补记）。
	//
	// ⚠ 为什么建在**这里**而不是开门之前：SSO token 很短命（换完几十秒内就要用），
	// 而中间夹着一整段听弹幕（默认 60 秒）。放在投之前才是"拿到口令 → 立刻用它"。
	//
	// **jwttoken 使用公共来源**（`AGENTS.md` §1.12 要求留痕的那句）：这条六步链要的票
	// 是签到页那一套参数换的（`upstream.MarketName` / `SourceID`），直播页自己的取法
	// **尚未抓包确认**；哪天查明这一页要按自己的参数换票，改的是这里的**配方**
	// （`SessionRecipe` 新增一个取值 + [OpenSignInSession] 按配方取那三个参数），
	// 并**同时**改掉这句注释与 [RecipeSignInPage] 那一条。
	//
	// ⚠ 走 [SessionPool] 而不是就地 new 一条：界面上"连点三次领取小红花"应当只建一条会话
	// （用户 2026-09-23 的口径），而且这一条链刷出来的新票**只该顶掉它自己那一条**，
	// 不连带签到页那张 —— 键里那个配方就是为这件事存在的（今天两页同配方，所以是同一条）。
	src := sessionSource(r.Sessions, driver.Upstream, profile, &credential,
		r.Driver.Clock, r.Driver.Random, RecipeSignInPage)
	// 先逼它建一条：建会话失败时这一趟**不降级**（这一趟的目的就是投口令，
	// 会话建不起来就是整趟失败，而不是"余额没读到"那种可以带着结论回去的情况）。
	// 放在这里也保住了原来那句 `[小红花·建云盘会话]` 的措辞 —— 让链的第一步去建，
	// 失败会被包成"[起始余额] …"，看不出是建会话坏了。
	if _, perr := src.Session(ctx); perr != nil {
		return outcome, perr.WithPrefix("[小红花·建云盘会话] ")
	}

	r.progress(core.LevelInfo, fmt.Sprintf("投 %d 个口令…", len(outcome.Codes)))
	report, perr := RunRedFlower(ctx, src, NewRedFlowerSession(outcome.Codes))
	if perr != nil {
		// 第一轮就失败时**不**带报告：那一趟连起始余额都可能没读到，
		// 递一份"0 → 0（进账 0）"出去会被读成"投了但没进账"。
		return outcome, perr
	}

	// ⚠ 重听一次：只在**有口令被判无效**时做。判据是纯函数 [RetryPlan]，
	// 所以"什么时候该重听"这件事在这里看得见，而不是埋在流程里。
	if retry, exclude := RetryPlan(outcome.Codes, report.InvalidCodes); retry && !r.Params.SkipKouling {
		outcome.Retried = true
		r.progress(core.LevelWarn, fmt.Sprintf("有 %d 个口令无效，重听 %d 秒…",
			len(report.InvalidCodes), r.Params.ListenSeconds))
		// 重听时**排除**已经试过的（含第一次就成功的）—— 不排除的话第二次会把
		// 同一批口令再投一遍，而**同一账号**对同一条**正确**口令的第二次请求会回
		// `200112`（已兑换），既污染 `Draws` / `InvalidCodes` 的归因、
		// 又看起来像"口令坏了"。
		fresh := r.freshCodes(ctx, driver, opened.JWT, opened.Gate.Rooms[0].ID, exclude)
		outcome.Report = &report
		if toDraw := PickNewCodes(fresh, outcome.Codes, report.InvalidCodes); len(toDraw) > 0 {
			r.progress(core.LevelInfo, fmt.Sprintf("又收到 %d 个新口令，再投一轮…", len(toDraw)))
			// 第二轮**沿用同一个 source**（口径："一轮任务一条 `jwttoken`"）。
			// 早先这里传的是 `opened.JWT`（直播侧那一份），而六步链要的是云盘侧的 ——
			// 状态机自己收票之后这个口子没了（见 [NewRedFlowerSession]），票只能来自 `src`。
			second, perr := RunRedFlower(ctx, src, NewRedFlowerSession(toDraw))
			if perr != nil {
				// 第二轮失败时保留**第一轮**那份：它是完整跑完的。
				return outcome, perr
			}
			// 两轮的结果**合并**：第一次成功的不能因为第二轮而消失。
			outcome.Report = mergeRedFlowerReports(report, second)
			outcome.Codes = append(outcome.Codes, toDraw...)
		}
		return outcome, nil
	}

	outcome.Report = &report
	return outcome, nil
}

// freshCodes 重听一次并返回新听到的口令（已按 `exclude` 过滤）。
//
// 单独一个函数是为了让"重听失败怎么办"这件事只有一处答案：**不致命**。
// 重听是补救措施，它自己失败时应当保留第一轮的结果，而不是把整趟变成失败。
func (r LiveRoomRun) freshCodes(
	ctx context.Context,
	driver LiveRoomDriver,
	jwt string,
	roomID int64,
	exclude []string,
) []string {
	codes, err := driver.Collect(ctx, jwt, roomID, r.listenFor())
	if err != nil {
		return nil
	}
	return PickNewCodes(codes, exclude, nil)
}

// listenFor 把参数里的秒数换成一次收听的时长。
//
// ⚠ 非正数不当错误抛出去：用法校验的措辞是两个产物各自的（见 [LiveRoomParams]），
// 而这里抛错会让"没校验就调"变成一句与参数无关的运行时错误。
// 0 传下去的后果是那次 WS 与并行的那次 GET **一起**立刻超时 ⇒ 收集那一步回
// "两个口令来源都失败了" —— 一条看得见、但方向错的通知。所以**调用方仍须**先挡住非正数，
// 这一层只是不假装它是合法输入。
func (r LiveRoomRun) listenFor() time.Duration {
	if r.Params.ListenSeconds <= 0 {
		return 0
	}
	return time.Duration(r.Params.ListenSeconds) * time.Second
}

func (r LiveRoomRun) progress(level core.LogLevel, message string) {
	if r.Progress != nil {
		r.Progress(level, message)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口令拆分与两轮结果合并
// ─────────────────────────────────────────────────────────────────────────────

// SplitKouling 把**一栏自由文本**拆成口令（换行 / 逗号 / 顿号都算分隔符）。
//
// 分隔符集合是**边界**而不是内部实现：`docs/cli-usage.md` 里那一句"多个用换行或
// 逗号分隔"与界面上那一栏的占位文案都指向它，所以两个产物必须共用一份。
func SplitKouling(raw string) []string {
	fields := strings.FieldsFunc(raw, func(r rune) bool {
		return r == '\n' || r == '\r' || r == ',' || r == '，' || r == '、'
	})
	return MergeKouling(fields)
}

// mergeRedFlowerReports 把第二轮的结果并进第一轮。
//
// ⚠ 余额取**第二轮**的终值（时间上更晚，也更接近"投完之后到底有多少"），
// 而起始余额保留第一轮的 —— 两次读数之差才是"这两轮加起来进账多少"。
func mergeRedFlowerReports(first, second RedFlowerReport) *RedFlowerReport {
	merged := first
	merged.BalanceAfter = second.BalanceAfter
	merged.Draws = append(append([]RedFlowerDrawRecord{}, first.Draws...), second.Draws...)
	// ⚠ 无效口令取**并集**，不取第二轮那份。
	//
	// 第二轮里没有甲，只是因为它被重听时的 `exclude` 排除了 —— 而"甲被判无效"
	// 是一件**已经发生的事实**。取第二轮那份会让这条事实从结果里消失，
	// 于是"成功 1/2"变成一句没有下文的话：用户看不出那一个为什么没成。
	// 而它恰恰是最该看到的一条（意味着弹幕里那条口令是过期的 / 抄错了）。
	merged.InvalidCodes = MergeKouling(first.InvalidCodes, second.InvalidCodes)
	merged.OtherCodes = append(append([]string{}, first.OtherCodes...), second.OtherCodes...)
	// ⚠ 过期口令**直接追加**即可，不需要 [MergeKouling] 去重：
	// [PickNewCodes] 只挑"既不在第一批、也不在无效表"的口令，而过期口令
	// **在第一批里** ⇒ 第二轮根本不会重投它（与 `OtherCodes` 同一处境）。
	merged.ExpiredCodes = append(append([]string{}, first.ExpiredCodes...), second.ExpiredCodes...)
	if second.RevivalFlowers != 0 {
		merged.RevivalFlowers = second.RevivalFlowers
	}
	return &merged
}

// ─────────────────────────────────────────────────────────────────────────────
// 结论：给人看的那一句 + 领域层的四态
// ─────────────────────────────────────────────────────────────────────────────

// SummaryLine 是"这一趟跑成了什么"的那句话，**不含**账号前缀。
//
// ⚠ 进账用**两次读数相减**，不是把口令的 flowerNum 加起来 ——
// 后者会漏掉"首次参与惊喜奖"那几朵，而它不在任何一个口令的结果里。
func (r *RedFlowerReport) SummaryLine() string {
	return fmt.Sprintf("小红花 %d → %d（进账 %d，成功 %d/%d）%s",
		r.BalanceBefore.FlowerCnt, r.BalanceAfter.FlowerCnt,
		r.Gained(), r.Succeeded(), len(r.Draws), r.notesSuffix())
}

// notesSuffix 拼"这次有什么值得多看一眼"的尾巴。
func (r *RedFlowerReport) notesSuffix() string {
	var parts []string
	if len(r.InvalidCodes) > 0 {
		parts = append(parts, "无效口令 "+strings.Join(r.InvalidCodes, " / "))
	}
	if len(r.ExpiredCodes) > 0 {
		// ⚠⚠ 这句是**用户可见的**，措辞按坑表 #140 做法 ④ 定：
		// 说"口令已失效（本场已结束）"，**不说**「未分类口令」——
		// 后者把人往"口令抄错了"（无效口令）那边引，而这两件事的排查方向完全不同。
		parts = append(parts, "口令已失效（本场已结束）"+strings.Join(r.ExpiredCodes, " / "))
	}
	if len(r.OtherCodes) > 0 {
		// ⚠ 与无效口令分开说：它们是"上游回了个我们没分类的码"，
		// 排查方向完全不同（不是口令抄错了）。
		parts = append(parts, "未分类口令 "+strings.Join(r.OtherCodes, " / "))
	}
	if r.FirstParticipate.Kind == upstream.FirstParticipateGained &&
		r.FirstParticipate.FlowerNum > 0 {
		// ⚠ 那个 `> 0` 不是洁癖：[upstream.FirstParticipateGained] 是**零值**，
		// 于是"没走过这一步"的报告（比如合并出来的、或只跑到①就断的）
		// 会拖出一句"首次参与 +0"。
		parts = append(parts, fmt.Sprintf("首次参与 +%d", r.FirstParticipate.FlowerNum))
	}
	if r.RevivalFlowers != 0 {
		parts = append(parts, fmt.Sprintf("复活花 %d", r.RevivalFlowers))
	}
	if len(parts) == 0 {
		return ""
	}
	return "（" + strings.Join(parts, "；") + "）"
}

// RunReport 把这一趟的产出合成领域层结论。
//
// # 四态怎么给
//
//   - **没投**（只听不投 / 门没开 / 一个口令都没收到）⇒ **成功**。
//     "今天没开播"是正常结果：门开着才有口令，而门开着与否由服务端数据决定 ——
//     与签到里"今天已经签过了"同一档。
//   - 投了、**一个都没成** ⇒ 失败。这不是"没跑到"，是"这趟目标没达成"，
//     而小结里带着原因（无效 / 已兑换 / 未分类），所以不会只剩一个光秃秃的"失败"。
//   - 投了、成了**一部分** ⇒ 部分成功。
//
// ⚠ 这里**不放** [core.ManualStep]：那条语义是"要人去客户端点一下"，
// 而这条链上没有这样的动作（没开播就只能等，界面上把这句话显示出来就够）。
func (res LiveRoomAccountResult) RunReport() core.RunReport {
	if res.Report == nil {
		summary := res.idleSummary()
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	}
	report := res.Report
	succeeded, total := report.Succeeded(), len(report.Draws)
	summary := report.SummaryLine()
	switch {
	case total > 0 && succeeded == 0:
		return core.NewRunReportFailed(summary)
	case succeeded < total:
		return core.RunReport{
			Outcome: core.OutcomePartialSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	default:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	}
}

// idleSummary 说清"为什么这一趟什么都没投"。
//
// ⚠ 三种"没投"必须分开：它们的下一步完全不同（等开播 / 主播没喊口令 /
// 本来就不要投）。混成一句"没有可投的口令"会让人去查收集那一步，
// 而那一步根本没被走到过。
func (res LiveRoomAccountResult) idleSummary() string {
	switch {
	case res.Params.KoulingOnly:
		return fmt.Sprintf("收到 %d 个口令，本次只收不投", len(res.Codes))
	case !res.Gate.Active:
		if res.Gate.LiveCount == 0 {
			return "今天没有直播，没有口令可收"
		}
		return fmt.Sprintf("有 %d 间在播，但没有一间是「%s」", res.Gate.LiveCount, upstream.KoulingAnchor)
	default:
		return "开了门但没收到口令"
	}
}
