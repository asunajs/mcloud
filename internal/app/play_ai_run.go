package app

// 拍拍（#16 · 旧实现 `play-ai-special`）· **只读那一半**的编排：查活动侧状态 + 查任务表。
//
// # 为什么这一轮只做只读（另外三口各自卡在不一样的地方）
//
// 上游那一族有五个口子（`internal/upstream/app_play_ai.go` 文件头那张 ①…⑤ 表），
// 这里只接 ①②，因为剩下三口的"该不该发"**今天都答不出来**：
//
//   - **③ `task/step/click`（写）**：实拍序列是"查 → 上报一步 → 回查"，也就是说
//     上游确实**认**这一枪（`state` 从 `WAIT` 变 `FINISH`）。但旧实现把
//     `visitlog/journaling` 埋点当**门槛**用（上报失败就连 `step/click` 都不发，
//     `task.ts:89-99`、`:115-125`），而那两条埋点本仓**零实拍** ⇒ 接不接门槛是
//     **待拍板 P6**，不是这里能顺手定的。
//   - **④ `blindbox/lottery`（写）**：判据本该是 ① 那个 `totalLotteryCount`，
//     而这一格**语义未验证**（实拍那条 `1` 出现在**抽奖之前**，所以它至少含"还没花掉"的意思，
//     但一次观测说不出花掉会不会减回去）。拿一个读不懂的数当"还能抽几次"的判据去抽 = 猜，
//     而且抽奖是**不可回滚**的写请求。要收这一格只能在活动窗口内做（窗外补不了样）。
//   - **⑤ `aiRecognize`（写云盘）**：它 `uploadToCloud` 实拍恒 `true` ⇒ **真的往用户网盘写一张图**，
//     而"识别出来的 `fileId` 谁来删"那一族（`hcy/file/get` + `batchTrash`）今天**接口层冻结**
//     （P1）。产物账本 `internal/exec` 已经建好，但这条链没有调用点 ⇒ 今天不发。
//
// 于是这里交"看得见的一层"：这个号在这个活动里有什么任务、做到哪一步、次数那一格读到多少。
// 它单独成立（用户能照着任务名去 App 里做），而且它把这一族最容易错的**头集合**
// （比仓库其余 H5 接口少三个头）跑在真实链路上。
//
// # ⚠ `jwttoken` 使用公共来源（AGENTS.md §1.12 要求留痕的那一句）
//
// 这一条链要的票是**签到页那一套参数**换的（[SignInWorld.Source] → `RecipeSignInPage`）。
// 与小红花那条链同一处置，但这里的情况**更具体**：这一页的特有取法**有出处、且已实现**
// （`upstream.PlayAIQuerySpecTokenRequest` → `PlayAITyrzLoginRequest`，32 条测试），
// 缺的只是**实拍** —— 归档 13 条里连一次换票都没拍到（`docs/capture/app-play-ai-samples.txt` §5）。
// ⇒ 今天**不切**：切过去要多发两枪本仓零实拍的请求，而"上游认不认这一页自己那张票"
// 与"签到页那张票打这一族的接口认不认"**两个都没有样本**（后者是夹具给的暗示：
// 那六条带的就是 `jwttoken`，但没拍到它是哪一步换的）。这一格是**待拍板 P5**。
//
// ⚠ 但 **Referer 不借**：这一页的落地 URL 用 [upstream.PlayAIPageReferer]（旧 TS 逐字，B 档），
// **不用**会话里那份签到页 referer。"票借、页面身份不借"这个组合**零实拍**，
// 真打时如果回非零码，第一个要试的就是把 referer 换成会话自带的那一份。
//
// # 这一层不解释任何档位语义
//
// 只念读数：任务表里 `state` 只有 `WAIT` / `FINISH` 两个实拍值，其余（含缺键）
// 一律算"状态读不出"并单独计数，**不**折进"未完成"（将来接 ③ 时那一格会直接决定发不发请求）。

import (
	"context"
	"strconv"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// playAICapabilityCode 是这一条能力在注册表里的 code。
const playAICapabilityCode = capabilities.CodePlayAISpecial

// 任务表 `state` 的两个字面值。
//
// ⚠ 刻意**不**引用 `upstream.TaskStateWait` / `TaskStateFinish`（那是签到族那份枚举的名字）：
// 两族字面相同，但各自的**取值集**各自只由各自的样本支撑
// （`app_play_ai.go` 文件头那段"字面相同不等于同一份枚举"说的就是这件事）。
// 借用过来会让"这一族哪天出现第三个值"看起来像上游改了枚举，而实际是我们引错了表。
const (
	playAITaskStateWaiting = "WAIT"
	playAITaskStateDone    = "FINISH"
)

// PlayAIBoard 是这一屏的两份读数（①与②，**按实拍顺序**：夹具里先查状态再查任务表）。
//
// ⚠ `Tasks` 为**空数组**不等于"这个号没事做"：上游给 `result: []` 与"这一期没配任务"
// 是同一种回法 ⇒ 念的时候只数条数，不下"没有可做的"那个结论。
type PlayAIBoard struct {
	// Status 是 ① `newyear/user/info`。
	Status upstream.PlayAIUserStatus
	// Tasks 是 ② `newyear/task/list`。
	Tasks []upstream.PlayAIUserTask
}

// PlayAIStatus 是一次只读盘点的结局档位。
type PlayAIStatus int

const (
	// PlayAIReadDone 是"两枪都拿到了合法信封"。
	//
	// ⚠ 它**不**要求 `TotalLotteryCount` 非 nil，也不要求任务表非空：
	// 那些是"上游给了几个读数"，不是"这一趟成没成"。
	PlayAIReadDone PlayAIStatus = iota
	// PlayAIReadFailed 是没走到能报结论的那一步（建会话挂 / 任一枪挂）。
	PlayAIReadFailed
)

// PlayAIResult 是一个账号这一趟的产出。
type PlayAIResult struct {
	Account core.AccountID
	Status  PlayAIStatus
	Board   PlayAIBoard
	Error   string
}

// PlayAITaskCounts 是从任务表**算出来**的四个计数。
//
// ⚠ 单独成一个类型（而不是在 `SummaryLine` 里就地算）是为了让"未知那一档存在"
// 这件事在测试里可读 —— 把 `2` 那种没见过的值折进"未完成"是最容易顺手写错的归并。
type PlayAITaskCounts struct {
	Total   int
	Done    int
	Waiting int
	Unknown int
}

// Counts 数任务表的三档 + 总数。
func (b PlayAIBoard) Counts() PlayAITaskCounts {
	counts := PlayAITaskCounts{Total: len(b.Tasks)}
	for _, task := range b.Tasks {
		switch derefString(task.State) {
		case playAITaskStateDone:
			counts.Done++
		case playAITaskStateWaiting:
			counts.Waiting++
		default:
			// 缺键、空串、以及这一族没见过的任何值都在这里。
			counts.Unknown++
		}
	}
	return counts
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路
// ─────────────────────────────────────────────────────────────────────────────

// ReadPlayAIBoard 跑只读那两枪：`user/info` → `task/list`。
//
// # 两枪**都**是致命读
//
// ① 给的是这一屏唯一的"次数"读数，②给的是"还有什么事可做"。
// 缺任何一句都只是"少一行"，**不**只是"少一行" —— 缺 ② 会让用户以为这个号没事做，
// 那是会改变行为的误读，所以不学直播探针那条"播报失败只留 nil"的宽容档。
//
// # 为什么不在这里判"该不该抽"
//
// 判据是 ① 那一格，而它的语义未验证（文件头第二条）。这里只把原值交出去。
func ReadPlayAIBoard(
	ctx context.Context, src *SignInSessionSource,
) (PlayAIBoard, *core.Error) {
	status, perr := do(ctx, src, func(s SignInSession) (upstream.PlayAIUserStatus, *core.Error) {
		return s.callPlayAIStatus(ctx, src.up)
	})
	if perr != nil {
		return PlayAIBoard{}, perr
	}
	tasks, perr := do(ctx, src, func(s SignInSession) ([]upstream.PlayAIUserTask, *core.Error) {
		return s.callPlayAITaskList(ctx, src.up)
	})
	if perr != nil {
		return PlayAIBoard{}, perr
	}
	return PlayAIBoard{Status: status, Tasks: tasks}, nil
}

// callPlayAIStatus / callPlayAITaskList 把"用这张票打这一枪"收在一处。
//
// 收成 [SignInSession] 上的方法（而不是在 [ReadPlayAIBoard] 里直接写四行）只为一件事：
// 这一族的 `referer` **不是**会话自带的那一份（见文件头"Referer 不借"）。
// 放在方法里，"票从会话来、referer 从这一页来"这件事有唯一一个写法；
// 写在调用点则每一处都可能漏掉 referer 而拿 `s.referer` 顶上 —— 那看起来完全正常。
func (s SignInSession) callPlayAIStatus(
	ctx context.Context, up *upstream.Upstream,
) (upstream.PlayAIUserStatus, *core.Error) {
	return up.PlayAIUserStatus(ctx, s.jwt, upstream.PlayAIPageReferer(), upstream.PlayAIShowLoading)
}

func (s SignInSession) callPlayAITaskList(
	ctx context.Context, up *upstream.Upstream,
) ([]upstream.PlayAIUserTask, *core.Error) {
	return up.PlayAITaskList(ctx, s.jwt, upstream.PlayAIPageReferer(), upstream.PlayAIShowLoading)
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIRun 跑一次只读盘点。
//
// 它嵌 [SignInWorld] 而不是自建世界：**这一条链借的就是签到页那张票**，
// 所以票池、档案与凭据取法都该与签到那一条同源（换成特有取法时改的是
// [SignInWorld.Source] 里那个配方，见文件头 P5 那一段）。
type PlayAIRun struct {
	SignInWorld
}

// RunAccount 跑**一个账号**的这一笔。
func (r PlayAIRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (PlayAIResult, *core.Error) {
	// **jwttoken 使用公共来源**：[SignInWorld.Source] 交出的就是 `RecipeSignInPage`
	// 那一条会话（留痕与"哪天改成特有取法要动哪一处"写在文件头）。
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return PlayAIResult{Account: account, Status: PlayAIReadFailed, Error: perr.Error()}, perr
	}
	board, perr := ReadPlayAIBoard(ctx, src)
	if perr != nil {
		result := PlayAIResult{Account: account, Status: PlayAIReadFailed, Error: perr.Error()}
		r.progress(core.LevelWarn, result.SummaryLine())
		return result, perr
	}
	result := PlayAIResult{Account: account, Status: PlayAIReadDone, Board: board}
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件）。
//
// # 三个"不念"
//
//   - **不念"还能抽 N 次"**：那一格语义未验证，念出来只写"读到 N"并标明没解释。
//   - **不念任务名**：`name` / `flag` 是上游文案（实拍「拍夏日关键词」这种带期次色彩的串），
//     进 `runs.message` 等于把上游文案写进我们对外承诺的那一列（同 `NoticeResult` 那条理由）。
//   - **不念手机号**：这一族的响应里不带账号标识，但这一句会被抄进日志，
//     所以将来加字段时也别把上游可能给的掩码号（带尾号那种形态）塞进来。
func (res PlayAIResult) SummaryLine() string {
	head := "拍拍"
	if res.Status == PlayAIReadFailed {
		if res.Error == "" {
			return head + "没读成"
		}
		return head + "没读成：" + res.Error
	}
	counts := res.Board.Counts()
	line := head + "任务 " + strconv.Itoa(counts.Total) + " 件（未完成 " +
		strconv.Itoa(counts.Waiting) + "、已完成 " + strconv.Itoa(counts.Done)
	if counts.Unknown > 0 {
		// 单独念出来：这一档存在就说明"上游给了这一族没见过的值"，
		// 藏进总数里的话，将来接 ③ 时这一格会被当成"已完成/未完成"两档之一。
		line += "、状态读不出 " + strconv.Itoa(counts.Unknown)
	}
	line += "）；" + playAILotteryCountLine(res.Board.Status.TotalLotteryCount)
	line += "；本工具只读：不做任务、不抽奖、不写网盘"
	return line
}

// playAILotteryCountLine 念 ① 那一格。
//
// ⚠ `nil` 与 `0` 分开：实拍**两条都有值**（`0` 与 `1`），所以"没给这一位"是
// 一种**没样本**的形态 ⇒ 只报"没给"，不猜它等于几（§1.7）。
func playAILotteryCountLine(count *int64) string {
	if count == nil {
		return "抽奖次数那一格上游没给（这一格语义本就未验证，不据此判断）"
	}
	return "抽奖次数那一格读到 " + strconv.FormatInt(*count, 10) +
		"（" + playAILotteryCountHedge + "）"
}

// playAILotteryCountHedge 是那句"这一格没解释"的原话。
//
// 单独成常量是为了让它**只有一处**：这句是要抄进 `Output` 声明与测试断言的，
// 抄成两处就早晚只改一处。
const playAILotteryCountHedge = "是累计获得还是还剩，未验证"

// RunReport 把结论映射到 [core.RunReport]。
//
// 只读那一半**不产 ManualStep**：`core.ManualStep` 的语义是"得有人去 App 里点一下才继续"，
// 而这一条今天的用途就是"看一眼" —— 它不"继续"任何流程。
// ⚠ 这一族将来接 ③④ 时，"哪几件必须人做"（实拍三条任务里「拍夏日关键词」要在 App 内拍照、
// 「去小红书点赞」是**站外**动作）才是从这里出去的东西，那时要一并过
// `runs.capability` 那一列的取值。
func (res PlayAIResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	if res.Status == PlayAIReadFailed {
		return core.NewRunReportFailed(summary)
	}
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}

// derefString 把 `*string` 折成可 switch 的值：nil ⇒ 空串，落进 default 那一档。
//
// 为什么不用 `tea.Ptr` 那类：这一族只有两个地方要读指针字符串，而**语义**恰好是
// "没有也算一种不知道" ⇒ 折成空串交给 default 分支比到处 `!= nil` 更不容易漏。
func derefString(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}
