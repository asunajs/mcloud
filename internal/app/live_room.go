// 直播口令 → 小红花：**业务编排**（纯层，不发请求、不碰连接）。
//
// 对应原项目 `packages/core/src/tasks/live-room-kouling.ts` 与
// `packages/core/src/tasks/red-flower/standalone.ts`。
//
// # 为什么做成状态机，而不是一个"从头跑到尾"的函数
//
// 这条链有 **六步固定顺序 + 一个变长循环**（N 个口令各抽一次），而它一年里
// 只有主播开播的那几小时才能真跑一次。写成一口气跑完的函数，唯一能验证它的
// 方式就是**等开播**；写成状态机之后，"第几步该发什么请求、拿到什么响应该
// 走到第几步"全部可以在单测里走完（见 live_room_test.go）。
//
// 更实际的一条：**漏一步不会报错**。`visitlog/journaling` 那 5 条埋点少发一条，
// 服务端**不会**拒掉 `draw`，它只会回 `200108`（口令不对）——
// 于是排查方向指向"口令抄错了"，而不是"少发了一步"。状态机让"少发一步"
// 变成一条失败的断言。
//
// # 两个刻意的设计
//
//   - **`Next` 不推进**：它返回"当前这一步该发什么"，推进只发生在 [Feed]。
//     这样"请求构造失败"与"发出去失败了"不会各自偷偷推进一次。
//   - **本层不 sleep、不取时钟**：重听一次的等待由调用方负责，
//     "要不要重听"由 [RetryPlan] 回答（纯函数）。
package app

import (
	"context"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 门：现在有没有值得听的口令直播
// ─────────────────────────────────────────────────────────────────────────────

// KoulingGate 是"要不要去听口令"的判定结果。
type KoulingGate struct {
	// Active 表示**有**符合条件（直播中 + 主播是那个固定昵称）的直播间。
	Active bool
	// Rooms 是通过筛选的直播间（已按 id 去重）。
	Rooms []upstream.LiveRoom
	// LiveCount 是筛完 `status == 1` 之后剩下的房间数（**用于区分两种"没有"**）。
	//
	// ⚠ 它与 `Active` 一起才说得清现状：
	//
	//	LiveCount == 0          → 谁都没在播（今天没有直播）
	//	LiveCount > 0, !Active  → **有人**在播，但不是那个主播（实拍踩过：
	//	                          房间 147490「福建移动」在播）
	//
	// 只报 `Active=false` 会把这两种情况混成一句"没有直播"，
	// 而它们的下一步完全不同（一个是等，一个是别等了）。
	LiveCount int
}

// JudgeKoulingGate 判定门的开合（纯函数）。
//
// # 判据是 **两个条件同时成立**
//
//	room.status == 1  **且**  room.anchor.nickname == "中国移动云盘"
//
// ⚠ **只看 `status == 1` 会误判**：同一个页签里别的主播也在播（实拍 147490）。
// ⚠ **`channelId` 不能当过滤条件**：返回的房间的 `channelId` 有 29/51/243/134
// —— 它表达的是"房间归属哪个频道"，不是"我拉的是哪个频道"。
func JudgeKoulingGate(rooms []upstream.LiveRoom) KoulingGate {
	live := 0
	for _, room := range rooms {
		if room.Status == upstream.LiveRoomStatusLive {
			live++
		}
	}
	return KoulingGate{
		Active:    len(upstream.KoulingRooms(rooms)) > 0,
		Rooms:     upstream.KoulingRooms(rooms),
		LiveCount: live,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口令合并
// ─────────────────────────────────────────────────────────────────────────────

// MergeKouling 合并多个来源的口令（保序去重、逐条 trim、丢掉空串）。
//
// 对应原项目的 `mergeKoulingSources`。两个来源（WebSocket 弹幕 / 小红书主页）
// **地位相同**：任一来源为空不影响另一个。
func MergeKouling(sources ...[]string) []string {
	seen := make(map[string]struct{})
	var out []string
	for _, source := range sources {
		for _, value := range source {
			code := strings.TrimSpace(value)
			if code == "" {
				continue
			}
			if _, ok := seen[code]; ok {
				continue
			}
			seen[code] = struct{}{}
			out = append(out, code)
		}
	}
	return out
}

// RetryPlan 回答"要不要为无效口令重听一次，以及听的时候该排除哪些"（纯函数）。
//
// 对应原项目 `live-room-kouling.ts` 里那段重试逻辑，但把它从"顺手写在流程里"
// 提成一个纯函数 —— 那段逻辑有两个容易写错的边界：
//
//   - **没有无效口令就不重听**：一次完全正常的运行不该多听 60 秒；
//   - **重听时要排除"已经试过的"**：既包括被判无效的，也包括第一次就成功的
//     —— 不排除已成功的话，第二次会把同一批口令再投一遍，而**同一账号**对同一条
//     **正确**口令的第二次请求会回 `200112`（已兑换）：它不是新结果，却会污染
//     `Draws` / `InvalidCodes` 的归因，看起来像"口令坏了"。
//     ⚠ 这里的理由**不是**"口令会被消耗"（口令全场通用，见 `live_flower.go` 文件头）。
func RetryPlan(firstBatch, invalidCodes []string) (retry bool, exclude []string) {
	if len(invalidCodes) == 0 {
		return false, nil
	}
	return true, MergeKouling(firstBatch, invalidCodes)
}

// PickNewCodes 从重听结果里挑出**这次真正要投**的口令（纯函数）。
//
// 判据是"既不在第一批里、也不在无效表里"—— 与 [RetryPlan] 的排除表同一份
// 依据，但用途不同：那个决定"听的时候排除谁"，这个决定"投的时候投谁"。
func PickNewCodes(retried []string, alreadyTried []string, invalidCodes []string) []string {
	tried := make(map[string]struct{}, len(alreadyTried))
	for _, code := range alreadyTried {
		tried[strings.TrimSpace(code)] = struct{}{}
	}
	for _, code := range invalidCodes {
		tried[strings.TrimSpace(code)] = struct{}{}
	}
	var out []string
	for _, code := range MergeKouling(retried) {
		if _, ok := tried[code]; ok {
			continue
		}
		out = append(out, code)
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 六步链
// ─────────────────────────────────────────────────────────────────────────────

// RedFlowerStep 是六步链里的一步。
type RedFlowerStep int

const (
	// StepBalanceBefore 是 ① `user/info`（**起始余额**）。
	StepBalanceBefore RedFlowerStep = iota
	// StepJournaling 是 ② `visitlog/journaling`（**要发 5 次**）。
	StepJournaling
	// StepFirstParticipate 是 ③ `firstParticipate/give`（首次参与惊喜奖）。
	StepFirstParticipate
	// StepDraw 是 ④ `redFlower/draw?code=`（**每个口令一次**，这是写操作）。
	StepDraw
	// StepRevivalFlowers 是 ⑤ `queryRevivalFlowers`（复活花，**不是余额**）。
	StepRevivalFlowers
	// StepBalanceAfter 是 ⑥ `user/info`（**终值**，与①相减才是本次进账）。
	StepBalanceAfter
	// StepDone 表示链已走完（`Next` 返回零请求）。
	StepDone
)

func (s RedFlowerStep) String() string {
	switch s {
	case StepBalanceBefore:
		return "user_info_before"
	case StepJournaling:
		return "journaling"
	case StepFirstParticipate:
		return "first_participate"
	case StepDraw:
		return "draw"
	case StepRevivalFlowers:
		return "revival_flowers"
	case StepBalanceAfter:
		return "user_info_after"
	case StepDone:
		return "done"
	}
	return ""
}

// RedFlowerDrawRecord 是一个口令的抽取结果。
type RedFlowerDrawRecord struct {
	Code    string
	Outcome upstream.RedFlowerDrawOutcome
}

// RedFlowerReport 是走完六步之后的结论。
//
// ⚠ 这里**不带**任何 JWT（原先有一位 `JWT string`，2026-09-22 删掉）：六步链用的
// 是云盘侧会话（[OpenSignInSession]），那是能签到、能读云朵的凭据，而它**零消费者** ——
// 结果会顺着 CLI 打印与 run 报告走，"顺手把凭据也带上"就是泄漏最常见的路径。
// 界面上要的那一位是 [LiveRoomAccountResult.HasJWT]，一个 bool。
type RedFlowerReport struct {
	// BalanceBefore / BalanceAfter 是①与⑥两次读数。
	BalanceBefore upstream.RedFlowerBalance
	BalanceAfter  upstream.RedFlowerBalance
	// FirstParticipate 是③的结论（`200110` 是**正常**的一支）。
	FirstParticipate upstream.FirstParticipateOutcome
	// Draws 是④逐口令的结果（**保序**，与传入的 codes 同序）。
	Draws []RedFlowerDrawRecord
	// RevivalFlowers 是⑤的读数。
	RevivalFlowers int64
	// InvalidCodes 是被判"口令不对"的那些（决定要不要重听一次）。
	InvalidCodes []string
	// OtherCodes 是回了**别的**业务码的那些（既不是无效也不是已兑换）。
	//
	// ⚠ 单独留一栏：它们是"上游说了个我们没分类的码"，与"口令错"排查方向
	// 完全不同。混进 `InvalidCodes` 会触发一次毫无意义的 60 秒重听。
	OtherCodes []string
}

// Gained 是本次进账几朵（⑥ − ①）。
//
// ⚠ 用**同一个接口的两次读数**相减，而不是把每个口令的 `flowerNum` 加起来：
// 后者会漏掉③（首次参与惊喜奖）进账的那几朵，而它**不在**任何一个口令的结果里
// —— 那正好是"总对不上"这一类问题最常见的来源。
func (r RedFlowerReport) Gained() int64 {
	return r.BalanceAfter.FlowerCnt - r.BalanceBefore.FlowerCnt
}

// Succeeded 报告有几个口令真的换到了花。
func (r RedFlowerReport) Succeeded() int {
	count := 0
	for _, record := range r.Draws {
		if record.Outcome.IsSuccess() {
			count++
		}
	}
	return count
}

// RedFlowerSession 是六步链的状态机。
//
// 用法（顺序固定）：
//
//	for !session.Done() {
//	    _, request, err := session.Next()   // 当前该发什么
//	    envelope, err := up.Call(ctx, request)
//	    session.Feed(envelope)              // 推进
//	}
type RedFlowerSession struct {
	h5JWT string
	codes []string

	step           RedFlowerStep
	journalingSent int
	drawIndex      int

	report RedFlowerReport
}

// NewRedFlowerSession 构造一次六步链。
//
// `h5JWT` 是**云盘侧**（`tyrzLogin` 换的那一份），不是门换到的直播侧 JWT：
// 这一族的六个请求全打在 `m.mcloud.139.com/ycloud`，拿直播侧那份去投实测回
// `90001 未登录`（判据与实拍对照见 [ProbeLiveRoom] 的补记）。
// 名字刻意不叫 `jwt` —— 这条链上同时存在两份 JWT，摆错的那一种表现是
// "六个请求全部未登录"，而不是编译错误。
//
// `codes` 的顺序**有意义**（决定先投哪个口令）：**同一账号**对同一条**正确**口令的
// 第二次请求会回 `200112`，所以"先投最新的那个"能让日志里第一条就是最有信息量的。
func NewRedFlowerSession(h5JWT string, codes []string) *RedFlowerSession {
	return &RedFlowerSession{
		h5JWT: h5JWT,
		codes: MergeKouling(codes),
	}
}

// Done 报告链是否已走完。
func (s *RedFlowerSession) Done() bool { return s.step == StepDone }

// Step 返回当前（或已走完时的最后）一步。
func (s *RedFlowerSession) Step() RedFlowerStep { return s.step }

// Codes 返回这次要投的口令（已去重）。
func (s *RedFlowerSession) Codes() []string {
	out := make([]string, len(s.codes))
	copy(out, s.codes)
	return out
}

// Next 返回**当前这一步**要发的请求。**不推进状态**。
//
// 推进只发生在 [Feed]。这样"构造失败"与"发送失败"都不会让状态机偷偷前进一步
// —— 少投一个口令是**静默**的（余额会告诉你有问题，但不会告诉你是哪个）。
func (s *RedFlowerSession) Next() (RedFlowerStep, core.HTTPRequest, *core.Error) {
	switch s.step {
	case StepBalanceBefore:
		request, err := upstream.RedFlowerUserInfoRequest(s.h5JWT)
		return StepBalanceBefore, request, err

	case StepJournaling:
		// ⚠ 埋点发的是**固定 5 条**，顺序即 [upstream.LiveRoomJournalingEvents]。
		// 其中只有 `_flowerBtn` 是实拍证实的，其余 4 条照抄原项目（理由见那边注释）。
		event := upstream.LiveRoomJournalingEvents[s.journalingSent]
		return StepJournaling, upstream.LiveRoomJournalingRequest(event, s.h5JWT), nil

	case StepFirstParticipate:
		return StepFirstParticipate, upstream.FirstParticipateGiveRequest(s.h5JWT), nil

	case StepDraw:
		return StepDraw, upstream.RedFlowerDrawRequest(s.codes[s.drawIndex], s.h5JWT), nil

	case StepRevivalFlowers:
		return StepRevivalFlowers, upstream.QueryRevivalFlowersRequest(s.h5JWT), nil

	case StepBalanceAfter:
		request, err := upstream.RedFlowerUserInfoRequest(s.h5JWT)
		return StepBalanceAfter, request, err
	}
	return StepDone, core.HTTPRequest{}, nil
}

// Feed 用响应推进状态机。
//
// # ⚠ 三条"不中断整批"的判据
//
//   - `firstParticipate/give` 的 `200110`（已参与过）是**正常**的一支 ——
//     每个账号第二次跑这条链都会拿到它；
//   - `draw` 的 `200112`（**同一账号**第二次请求同一条**正确**口令 ⇒ 已兑换）
//     说明口令**是对的**，不该算无效；
//   - 某个口令回了别的码（[upstream.RedFlowerDrawOther]）**不该**让其余口令
//     一起被跳过：逐个口令重试是正常路径。
//
// 所以本函数**只在解析失败（响应不像信封）时**返回错误，业务码一律记进报告。
func (s *RedFlowerSession) Feed(envelope *upstream.Envelope) *core.Error {
	switch s.step {
	case StepBalanceBefore:
		balance, err := upstream.ParseRedFlowerBalance(envelope)
		if err != nil {
			return err
		}
		s.report.BalanceBefore = balance
		s.step = StepJournaling

	case StepJournaling:
		// 埋点的响应**不解析**：它是"到场"证据，回什么码都不改变后面的步骤。
		// ⚠ 这不是偷懒 —— 拿它当门槛会让"埋点接口偶发 5xx"变成整条链失败，
		// 而它本来只是噪声。
		s.journalingSent++
		if s.journalingSent >= len(upstream.LiveRoomJournalingEvents) {
			s.step = StepFirstParticipate
		}

	case StepFirstParticipate:
		s.report.FirstParticipate = upstream.ClassifyFirstParticipate(envelope)
		if len(s.codes) == 0 {
			s.step = StepRevivalFlowers
			break
		}
		s.step = StepDraw

	case StepDraw:
		outcome := upstream.ClassifyRedFlowerDraw(envelope)
		s.report.Draws = append(s.report.Draws, RedFlowerDrawRecord{
			Code:    s.codes[s.drawIndex],
			Outcome: outcome,
		})
		if outcome.RejectedKouling() {
			s.report.InvalidCodes = append(s.report.InvalidCodes, s.codes[s.drawIndex])
		}
		if outcome.Kind == upstream.RedFlowerDrawOther {
			s.report.OtherCodes = append(s.report.OtherCodes, s.codes[s.drawIndex])
		}
		s.drawIndex++
		if s.drawIndex >= len(s.codes) {
			s.step = StepRevivalFlowers
		}

	case StepRevivalFlowers:
		flowers, err := upstream.ParseRevivalFlowers(envelope)
		if err != nil {
			// ⚠ 复活花**不是余额**，拿不到它不该让整条链失败 ——
			// 六步里只有它纯属"顺便看一眼"。留 0 并继续。
			s.report.RevivalFlowers = 0
		} else {
			s.report.RevivalFlowers = flowers
		}
		s.step = StepBalanceAfter

	case StepBalanceAfter:
		balance, err := upstream.ParseRedFlowerBalance(envelope)
		if err != nil {
			return err
		}
		s.report.BalanceAfter = balance
		s.step = StepDone
	}
	return nil
}

// Report 返回当前的报告（可在任意时刻读，未走到的步骤留零值）。
func (s *RedFlowerSession) Report() RedFlowerReport {
	out := s.report
	out.Draws = make([]RedFlowerDrawRecord, len(s.report.Draws))
	copy(out.Draws, s.report.Draws)
	out.InvalidCodes = append([]string(nil), s.report.InvalidCodes...)
	out.OtherCodes = append([]string(nil), s.report.OtherCodes...)
	return out
}

// RunRedFlower 把六步链一口气跑完（`Next` → `Call` → `Feed` 的循环）。
//
// # 为什么"解析失败"要**中止**而不是跳过
//
// 六步里每一步的响应都带着后面要用到的信息（余额、判定）。解析失败意味着
// "上游换了信封形状"或"我们打错了端点"，继续跑只会把同一个问题重复六遍，
// 而且日志里看不出是哪一步开始的。中止 + 把错误原样带回去，排查面最小。
func RunRedFlower(
	ctx context.Context, up *upstream.Upstream, session *RedFlowerSession,
) (RedFlowerReport, *core.Error) {
	for !session.Done() {
		step, request, err := session.Next()
		if err != nil {
			return session.Report(), err
		}
		envelope, err := up.Call(ctx, request)
		if err != nil {
			return session.Report(), wrapStepError(step, err)
		}
		if err := session.Feed(envelope); err != nil {
			return session.Report(), wrapStepError(step, err)
		}
	}
	return session.Report(), nil
}

// wrapStepError 把"哪一步出的问题"写进错误消息。
//
// 六步里 ①②⑥ 用的是同一个 `user/info`，而 ② 会发 5 次 ——
// 不带步名的错误（"user/info 响应没有 result"）会让人分不清是起始余额还是终值。
func wrapStepError(step RedFlowerStep, err *core.Error) *core.Error {
	return err.WithPrefix("[" + step.String() + "] ")
}
