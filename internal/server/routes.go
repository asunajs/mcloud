package server

// 业务只读接口：能力清单 / 账号 / 运行历史与事件流 / 定时任务。
//
// # 出网形状一律**手写**
//
// 这一批结构体（`core.Account` / `core.RunRecord` / `core.Schedule`）都不加
// JSON tag，也**不**在 `core` 里加 —— 理由与 [userPayload] 那条一样：
// 一旦它们能直接序列化，某天一句 `writeJSON(w, 200, account)` 就会把
// `Owner`、`PasswordHash` 这类字段递出去，而那一刻没人审过。
// 这里逐字段列出**允许出网**的名字：新增字段默认是"不出网"。
//
// # 为什么这一组能先落地，而"跑任务"不能
//
// 它们全部是**读**：不执行任何能力。跑任务的另一半（`POST /api/runs`）
// 要等能力层真的能跑，路由可以提前写，但后面没有东西可跑。

import (
	"context"
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
)

// ─────────────────────────────────────────────────────────────────────────────
// 元信息
// ─────────────────────────────────────────────────────────────────────────────

// getVersion 与 getCapabilities 的 payload 都不在这里拼：
// 实现住在 [app.VersionPayloadOf] / [app.CapabilitiesPayloadOf]，
// 因为 CLI 也要打印同一张表，而 CLI 不该为了打个表把 net/http 拖进来。
func (s *Server) getVersion(w http.ResponseWriter, _ *http.Request, _ *AuthUser) {
	writeJSON(w, http.StatusOK, app.VersionPayloadOf())
}

func (s *Server) getCapabilities(w http.ResponseWriter, _ *http.Request, _ *AuthUser) {
	writeJSON(w, http.StatusOK, app.CapabilitiesPayloadOf())
}

// getStatus 是"后端自述"：界面上那一行状态用它。
//
// 判据是**只放后端确实知道的事实**，一件都不推断：
// 版本来自 [app.VersionPayloadOf]（与 `--version`、`/api/version` 同一份），
// 最近一次执行直接复用 [runPayload]（不另起一个 run 的形状）。
//
// 刻意**不**在这里放账号数 / 任务数：那些是各自页面的内容，
// 搬进常驻侧栏就等于把"同一个数字出现在两个地方"再犯一遍。
func (s *Server) getStatus(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	// limit 1：这一行只关心"最近一次"，而 ListRuns 已按 started_at DESC 排。
	runs, err := s.store.ListRuns(r.Context(), scopeOf(auth), 1)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	var lastRun any
	if len(runs) > 0 {
		lastRun = runPayload(runs[0])
	}
	version := app.VersionPayloadOf()
	writeJSON(w, http.StatusOK, map[string]any{
		"name":    version.Name,
		"version": version.Version,
		// ⚠ 当前**恒为 false**：Go 侧还没有把 `schedules` 表变成"到点触发"的
		// 循环（`docs/status.md` 运行时层那条写着"调度接入中"）。
		// 于是保存了定时任务也**不会自己跑** —— 这一项宁可显式报 false，
		// 也不能省略：省略的表现是界面上一片绿，而绿的意思是"在跑"。
		"schedulerRunning": false,
		"lastRun":          lastRun,
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 账号
// ─────────────────────────────────────────────────────────────────────────────

const (
	// defaultDays / maxDays 是云朵趋势的取数窗口（前端那张图的默认区间）。
	defaultDays = 30
	maxDays     = 365
)

func (s *Server) listAccounts(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	accounts, err := s.store.ListAccounts(r.Context(), scopeOf(auth))
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	// ⚠ "过期了没有"必须用**注入的时钟**判：直接用系统时间会让这条断言在测试里没法拨
	nowMillis := s.Now().UnixMilli()
	payload := make([]map[string]any, 0, len(accounts))
	for i := range accounts {
		payload = append(payload, accountPayload(accounts[i], nowMillis))
	}
	writeJSON(w, http.StatusOK, map[string]any{"accounts": payload})
}

func (s *Server) cloudStats(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := core.NewAccountID(r.PathValue("id"))
	ctx := r.Context()
	scope := scopeOf(auth)

	// 账号不存在**或不属于你** → 404，而不是"空数组"。
	//
	// 空数组会让三种情况长得一样：账号不属于你 / 账号没有历史 / `days` 筛掉了全部。
	// 三者该导出完全不同的动作（换账号 / 等采集 / 调窗口），而前端无从区分。
	account, err := s.store.GetAccount(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if account == nil {
		ErrDomain(core.NewNotFound("云盘账号 " + id.String())).Write(w)
		return
	}

	days := clampLimit(r.URL.Query().Get("days"), defaultDays, maxDays)
	// "今天"必须按 [core.LocalTZ] 切分：日快照的 `date` 就是这个时区下的日期，
	// 用 UTC 算会让每天的窗口在早上 8 点之前整体错一天。
	today := s.Now().In(core.LocalTZ())
	since := today.AddDate(0, 0, -days).Format("2006-01-02")

	stats, err := s.store.CloudStatsSince(ctx, scope, id, since)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}

	// 差值（`cloudDiff` / `cloudDiffWeek`）由采集侧按 `core.NextCloudStats` 算好落库，
	// 这里**不重算** —— 重算一遍等于让"图上显示的数"与"库里存的数"有两个来源。
	rows := make([]map[string]any, 0, len(stats))
	for _, stat := range stats {
		rows = append(rows, map[string]any{
			"date":          stat.Date,
			"cloudCount":    stat.CloudCount,
			"cloudDiff":     stat.CloudDiff,
			"cloudDiffWeek": stat.CloudDiffWeek,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"accountId": id.String(),
		"days":      days,
		"since":     since,
		"stats":     rows,
	})
}

// accountPayload 是账号的对外形状。
//
// 刻意**不含**：
//   - `owner` —— 那是本系统的用户 id，前端一个都用不上，出网只是多一处信息泄漏面；
//   - 任何凭据 —— 它们本来就不在 [core.Account] 里（在 `account_state`，只有能力层
//     读得到），这里是"连结构体都不往外递"的那道双保险。
func accountPayload(account core.Account, nowMillis int64) map[string]any {
	return map[string]any{
		"id": account.ID.String(),
		// 展示名（昵称 → 脱敏手机号 → id）由领域层决定，前端不再自己挑
		"displayName":    account.DisplayName(),
		"nickname":       account.Nickname,
		"phoneMasked":    account.PhoneMasked,
		"platform":       account.Platform,
		"enabled":        account.Enabled,
		"sortOrder":      account.SortOrder,
		"cloudCurrent":   account.CloudCurrent,
		"cloudUpdatedAt": rfc3339Of(account.CloudUpdatedAt),
		"authExpire":     account.AuthExpire,
		// 给结论，也给依据：前端要能显示"已过期"而不用自己再做一次毫秒比较
		"authExpired": account.IsAuthExpiredAt(nowMillis),
		"lastOkAt":    rfc3339Of(account.LastOKAt),
		"failStreak":  account.FailStreak,
		"note":        account.Note,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 运行历史
// ─────────────────────────────────────────────────────────────────────────────

const (
	defaultRuns = 50
	maxRuns     = 200
)

func (s *Server) listRuns(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	limit := clampLimit(r.URL.Query().Get("limit"), defaultRuns, maxRuns)
	runs, err := s.store.ListRuns(r.Context(), scopeOf(auth), limit)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	rows := make([]map[string]any, 0, len(runs))
	for _, run := range runs {
		rows = append(rows, runPayload(run))
	}
	writeJSON(w, http.StatusOK, map[string]any{"limit": limit, "runs": rows})
}

// runPayload 是历史记录的对外形状。
func runPayload(run core.RunRecord) map[string]any {
	var accountID any
	if run.AccountID != nil {
		accountID = run.AccountID.String()
	}
	return map[string]any{
		"id":         run.ID,
		"accountId":  accountID,
		"capability": run.Capability,
		// `status` 是**存储层的生命周期**（running / done / failed），
		// 不等于业务结论：done 可以是"全成 / 部分成 / 需人工"三种。
		// 业务结论在事件流的 `finished` 里，别在这里补一个假的 outcome。
		"status":      run.Status.String(),
		"startedAt":   run.StartedAt.UTC().Format(time.RFC3339),
		"finishedAt":  rfc3339Of(run.FinishedAt),
		"cloudGained": run.CloudGained,
		"cloudAfter":  run.CloudAfter,
		"message":     run.Message,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 事件流（SSE）
// ─────────────────────────────────────────────────────────────────────────────

const (
	// eventPage 是一次从库里取多少条。断线续传靠 `seq` 往后接着要，
	// 不靠这一页装下全部。
	eventPage = 500
	// pollInterval 是向总线拉取新事件的间隔。
	//
	// 总线是**拉取式**的（[mcloudexec.EventBus] 没有通知机制），所以这里每 50ms
	// 拉一次**内存队列** —— 不查库，只是拿一把锁看一眼。要替掉的"轮询"是
	// **浏览器侧**的 HTTP 轮询：那是每个客户端每秒一次真实请求。
	pollInterval = 50 * time.Millisecond
)

// runEvents 是 `GET /api/runs/{id}/events` —— SSE 事件流。
//
// # 前端契约：收到终局事件后请自己 `close()`
//
// 终局事件（`finished` / `errored`）发出去之后，这个响应**就结束了**。
// `EventSource` 在连接关闭后会按规范自动重连，所以前端拿到终局事件必须主动
// `close()`。不这么做，一个跑完的 run 会被浏览器一直重连（每次带上
// `Last-Event-ID`，服务端重放 0 条再关闭）。反过来"发完不关"更糟：
// 那是每个打开的页面挂着一个永久任务 + 每 50ms 一次拉取。
//
// # 归属
//
// 第一次查库就完成了"这个 run 存在吗 / 是不是你的"两个判断，结果都是 404 ——
// 归属错误**不能报成 403**，那等于确认了"这个 id 确实存在"。
func (s *Server) runEvents(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	raw := r.PathValue("id")
	runID, ok := core.ParseRunID(raw)
	if !ok {
		// 形状不对的字符串**不可能是我们生成的**，所以"不存在"是最贴切的结论
		//（[core.RunID] 的文档就是这么约定的），不必先查一次库。
		ErrDomain(core.NewNotFound("运行记录 " + raw)).Write(w)
		return
	}

	// `after` = "客户端已经看到的那一条"（**不是**"下一条要取的值"）。
	// 两个来源：`?after=` 给手工调用（curl / 测试），`Last-Event-ID` 是浏览器
	// 自动重连时带的。语义相同，都用 [core.SeqSeen]。
	var seen *core.Seq
	if value, found := parseAfter(r); found {
		seq := core.SeqSeen(value)
		seen = &seq
	}

	// 顺序是「先订阅总线、再读库」，两个原因：
	// ① 订阅会把总线里 `seen` 之后的历史灌进队列，于是"读库"与"订阅"之间发布的
	//    事件不会掉 —— 反过来的话那个窗口里的事件两条路都赶不上；
	// ② 订阅不需要知道 run 是否存在，而"是否存在"由紧接着的查库回答。
	subscriber := s.bus.Subscribe(runID, core.SeqStart())
	_ = subscriber

	// 这一次查询同时完成三件事：存在性/归属校验（不属于你 → NotFound）、
	// 取回总线里没有的那部分（进程重启后总线是空的）、给出历史。
	persisted, err := s.store.RunEventsAfter(r.Context(), scopeOf(auth), runID, seen, eventPage)
	if err != nil {
		s.bus.Unsubscribe(runID, subscriber)
		ErrDomain(asDomain(err)).Write(w)
		return
	}

	flusher, ok := w.(http.Flusher)
	if !ok {
		// net/http 的 ResponseWriter 一定实现它；走不到这里。
		// 不 panic 是为了不让一个接口把整个进程带下去。
		s.bus.Unsubscribe(runID, subscriber)
		ErrDomain(core.NewInternal("这个连接不支持流式响应")).Write(w)
		return
	}

	w.Header().Set("Content-Type", "text/event-stream; charset=utf-8")
	// `X-Accel-Buffering: no` 让 nginx 不缓冲 —— 缓冲会让 SSE 变成"一次性到达"。
	w.Header().Set("X-Accel-Buffering", "no")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.WriteHeader(http.StatusOK)
	flusher.Flush()

	pumpEvents(r.Context(), s.bus, runID, subscriber, persisted, w, flusher)
}

// pumpEvents 把事件推给一个 SSE 连接，直到它断开或这个 run 出现终局事件。
//
// ⚠ 它在**处理器返回之后**还在跑：SSE 的响应体必须活得比处理器调用更久。
// 这是本文件唯一的"后台 goroutine 由请求驱动"的地方。
func pumpEvents(
	ctx context.Context,
	bus *mcloudexec.EventBus,
	runID core.RunID,
	subscriber uint64,
	initial []core.Sequenced,
	w http.ResponseWriter,
	flusher http.Flusher,
) {
	defer bus.Unsubscribe(runID, subscriber)

	last := core.SeqStart()

	// ① 库里已有的（与总线里的会有重叠 —— 落库的那批同时也发布过，靠 `last` 去重）
	terminal, alive := deliver(w, flusher, initial, &last)
	if !alive {
		return
	}
	if terminal {
		// 库里就带着终局事件（"来看一次已经跑完的 run"）
		return
	}

	// ② 在线的：轮询总线
	ticker := time.NewTicker(pollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}

		terminal, alive = deliver(w, flusher, bus.Drain(runID, subscriber), &last)
		if !alive {
			// 客户端走了：**必须**退订，否则那条队列会一直留在总线里。
			// （由上面的 defer 负责）
			return
		}
		if terminal {
			return
		}

		if bus.Dropped(runID, subscriber) > 0 {
			// 队列满时丢的是**最新**的那几条（见 [mcloudexec.EventBus.Publish]）。
			// 处置是重订阅（把 `last` 之后的历史再灌一遍，顺带把计数清零）。
			// 这里**不静默跳过**：丢事件不报出来，表现就是"日志少了半截"而没人知道。
			bus.Unsubscribe(runID, subscriber)
			subscriber = bus.Subscribe(runID, last)
			// 重订阅已经把 `last` 之后的内存历史灌进队列了，下一轮 drain 会取到。
		}
	}
}

// deliver 发一批事件。
//
// 返回 `alive=false` 表示客户端已经走了；`terminal` 表示这一批里出现了终局事件。
//
// 序号 `<= last` 的一律跳过：订阅回放与库里取回的两批一定重叠，
// 而**重复推**和**漏推**一样会让前端显示一个错的进度。
func deliver(
	w http.ResponseWriter, flusher http.Flusher,
	batch []core.Sequenced, last *core.Seq,
) (terminal, alive bool) {
	alive = true
	for _, item := range batch {
		if item.Seq <= *last {
			continue
		}
		if !writeSSE(w, flusher, item) {
			return false, false
		}
		*last = item.Seq
		terminal = terminal || item.Event.IsTerminal()
	}
	return terminal, true
}

// writeSSE 写一条 SSE 消息。
//
// `id` 就是序号（客户端重连时原样带回来），`data` 就是事件本身的 JSON ——
// 形状由 [core.RunEvent.MarshalJSON] 定死，前端直接 `switch (ev.type)`。
func writeSSE(w http.ResponseWriter, flusher http.Flusher, item core.Sequenced) bool {
	data, err := core.MarshalJSON(item.Event)
	if err != nil {
		// 序列化失败理论上不可能（字段全是字符串与数字）。真发生了也不能让一条事件
		// 把整条流打断 —— 退化成一条看得见的错误，比静默断线强。
		fallback, _ := core.MarshalJSON(core.NewEventLog(
			item.Event.RunID, core.LevelError, "事件序列化失败"))
		data = fallback
	}
	if _, err := w.Write([]byte("id: " + strconv.FormatUint(item.Seq.Value(), 10) + "\n")); err != nil {
		return false
	}
	if _, err := w.Write([]byte("data: " + string(data) + "\n\n")); err != nil {
		return false
	}
	flusher.Flush()
	return true
}

// parseAfter 取"客户端已看到的最后一条序号"。
//
// 解析不出来时按"从头开始"处理而不是报错：这个头不在客户端的完全控制之下，
// 为一个畸形值拒绝整条流，会让页面卡在"连不上"而看不出原因。
func parseAfter(r *http.Request) (uint64, bool) {
	text := r.URL.Query().Get("after")
	if text == "" {
		text = strings.TrimSpace(r.Header.Get("Last-Event-ID"))
	}
	if text == "" {
		return 0, false
	}
	value, err := strconv.ParseUint(text, 10, 64)
	if err != nil {
		return 0, false
	}
	return value, true
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时任务
// ─────────────────────────────────────────────────────────────────────────────

const (
	// maxIDLen 是任务 id 的长度上限。
	//
	// 它不是路径安全的问题（id 只当数据库主键用），而是"别让一个 id 撑爆列表响应"：
	// 这个字段会原样回给前端并在 URL 里出现。
	maxIDLen = 64
	// maxNameLen 是名称长度上限（同上，纯展示字段）。
	maxNameLen = 100
)

// scheduleBody 是保存定时任务的请求体。
type scheduleBody struct {
	Name string `json:"name"`
	// `{"kind":"capability","code":"sign"}` 或 `{"kind":"pipeline","id":"..."}`
	Target     core.ScheduleTarget `json:"target"`
	Cron       string              `json:"cron"`
	Timezone   *string             `json:"timezone"`
	AccountIDs []core.AccountID    `json:"account_ids"`
	Body       any                 `json:"body"`
	Enabled    *bool               `json:"enabled"`
}

func (s *Server) listSchedules(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	schedules, err := s.store.ListSchedules(r.Context(), scopeOf(auth))
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	now := s.Now()
	rows := make([]map[string]any, 0, len(schedules))
	for i := range schedules {
		rows = append(rows, schedulePayload(schedules[i], now))
	}
	writeJSON(w, http.StatusOK, map[string]any{"schedules": rows})
}

// putSchedule 新建或覆盖。
//
// 用 `PUT`（而不是 `POST`）是因为 id 由调用方给在路径里：同一条路径重复提交
// 是**同一个结果**（幂等），这正是 PUT 的语义。id 由前端生成（沿用旧数据的字符串），
// 服务端不发明 id —— 那样"改一个任务"与"建一个任务"就分不出来了。
func (s *Server) putSchedule(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := r.PathValue("id")
	if perr := validateScheduleID(id); perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	body, ok := decodeJSON[scheduleBody](w, r)
	if !ok {
		return
	}
	ctx := r.Context()
	scope := scopeOf(auth)
	now := s.Now()

	name := strings.TrimSpace(body.Name)
	if name == "" {
		ErrDomain(core.NewBadRequest("任务名不能为空")).Write(w)
		return
	}
	if len([]rune(name)) > maxNameLen {
		ErrDomain(core.NewBadRequest("任务名最多 " + strconv.Itoa(maxNameLen) + " 个字")).Write(w)
		return
	}

	timezone := core.TZName
	if body.Timezone != nil && strings.TrimSpace(*body.Timezone) != "" {
		timezone = *body.Timezone
	}
	// 解析失败与"永远不触发"都要拒：前者是写错了，后者是**存下去也不会跑** ——
	// 而它不会报任何错，只会每天安静地什么都不做。
	if _, fires, err := core.NextFire(body.Cron, timezone, now); err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	} else if !fires {
		ErrDomain(core.NewBadRequest("这个 cron 永远不会触发: " + body.Cron)).Write(w)
		return
	}

	// 目标必须存在。两种目标各自查一遍，用的是**同一个 scope** ——
	// 于是"别人的组合"在这里也查不到，不必额外写一条越权判断。
	switch body.Target.Kind {
	case core.TargetCapability:
		if _, ok := capabilities.Get(body.Target.Code); !ok {
			ErrDomain(core.NewBadRequest("没有这个能力: " + body.Target.Code)).Write(w)
			return
		}
	case core.TargetPipeline:
		pipeline, err := s.store.GetPipeline(ctx, scope, body.Target.ID)
		if err != nil {
			ErrDomain(asDomain(err)).Write(w)
			return
		}
		if pipeline == nil {
			ErrDomain(core.NewBadRequest("没有这个组合: " + body.Target.ID)).Write(w)
			return
		}
	default:
		ErrDomain(core.NewBadRequest("未知的调度目标")).Write(w)
		return
	}

	// 账号逐个校验归属。空表示"全部账号"（模型里定的语义），所以不报错。
	//
	// 这一条不只是数据整洁：定时任务会拿着这些 id 去执行，
	// 混进别人的账号等于让 A 的任务去打 B 的账号。
	for _, accountID := range body.AccountIDs {
		account, err := s.store.GetAccount(ctx, scope, accountID)
		if err != nil {
			ErrDomain(asDomain(err)).Write(w)
			return
		}
		if account == nil {
			ErrDomain(core.NewBadRequest("云盘账号不存在或不属于你: " + accountID.String())).Write(w)
			return
		}
	}

	// 已有的那一行：`created_at` 与调度器写的**记账字段**要留着。
	// 全量覆盖的话，用户改一次启用状态就会把"上次跑成没成"抹掉 ——
	// 而那正是他打开这个页面想看的东西。
	existing, err := s.store.GetSchedule(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}

	rawBody := jsonRawMessageOf(body.Body)
	schedule := core.NewSchedule(id, name, body.Cron, timezone)
	schedule.Owner = &auth.User.ID
	schedule.Target = &body.Target
	if body.AccountIDs != nil {
		schedule.AccountIDs = body.AccountIDs
	}
	schedule.Body = rawBody
	// 缺省启用：会走到这一步的人都是"特意建了一条定时任务"。
	// 默认停用会让他第二天来问"为什么没跑"，而那是个**静默**的失败。
	schedule.Enabled = true
	if body.Enabled != nil {
		schedule.Enabled = *body.Enabled
	}
	if existing != nil {
		schedule.CreatedAt = existing.CreatedAt
		schedule.LastRunAt = existing.LastRunAt
		schedule.LastStatus = existing.LastStatus
		schedule.LastError = existing.LastError
	}
	if schedule.CreatedAt == nil {
		schedule.CreatedAt = &now
	}

	if err := s.store.UpsertSchedule(ctx, scope, &schedule); err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"schedule": schedulePayload(schedule, now)})
}

func (s *Server) deleteSchedule(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := r.PathValue("id")
	deleted, err := s.store.DeleteSchedule(r.Context(), scopeOf(auth), id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if !deleted {
		// "不属于你"与"不存在"同一结论（存储层已经这么实现了，不泄漏存在性）
		ErrDomain(core.NewNotFound("定时任务 " + id)).Write(w)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// validateScheduleID 校验任务 id。
func validateScheduleID(id string) *core.Error {
	if id == "" {
		return core.NewBadRequest("任务 id 不能为空")
	}
	if len([]rune(id)) > maxIDLen {
		return core.NewBadRequest("任务 id 最多 " + strconv.Itoa(maxIDLen) + " 个字符")
	}
	// 允许的字符集与"kebab-case 文件名"同一套：id 会被拼进 URL 与日志，
	// 放开到任意字符只会让"看起来像两个任务"的东西在别处变成同一个。
	for _, r := range id {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' {
			continue
		}
		if r == '-' || r == '_' || r == '.' {
			continue
		}
		return core.NewBadRequest("任务 id 只能用字母、数字与 - _ . ：" + id)
	}
	return nil
}

// schedulePayload 是定时任务的对外形状（含**下次执行时间**）。
//
// 下次执行由服务端算：前端要显示它，而它依赖 `cron` + `timezone` 两个字段的
// 联合语义（5 段还是 6 段、周日写 0 还是 7、日与周同时限定时取 OR），
// 让每个客户端各实现一遍必然分叉。
func schedulePayload(schedule core.Schedule, now time.Time) map[string]any {
	// 停用即不会触发 —— 这时给一个"下次执行时间"就是在骗人。
	// 算不出来（导入进来的坏数据）也退化成 null，**不让一整份列表因此 500**：
	// 一条坏数据不该让用户的整个定时任务页打不开。
	var nextFire any
	if schedule.Enabled {
		if fired, ok, _ := core.NextFire(schedule.Cron, schedule.Timezone, now); ok {
			nextFire = fired.UTC().Format(time.RFC3339)
		}
	}

	accountIDs := make([]string, 0, len(schedule.AccountIDs))
	for _, id := range schedule.AccountIDs {
		accountIDs = append(accountIDs, id.String())
	}

	return map[string]any{
		"id":         schedule.ID,
		"name":       schedule.Name,
		"target":     schedule.Target,
		"cron":       schedule.Cron,
		"timezone":   schedule.Timezone,
		"accountIds": accountIDs,
		"body":       schedule.Body,
		"enabled":    schedule.Enabled,
		"nextFireAt": nextFire,
		"createdAt":  rfc3339Of(schedule.CreatedAt),
		"lastRunAt":  rfc3339Of(schedule.LastRunAt),
		"lastStatus": schedule.LastStatus,
		"lastError":  schedule.LastError,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// scopeOf 把身份映射成数据可见范围。
//
// 数据隔离本身在存储层那份过滤表达式里，这一层只负责"把请求映射成某个人"。
func scopeOf(auth *AuthUser) core.Scope { return core.ScopeUser(&auth.User.ID) }

// clampLimit 把查询参数收进 `[1, max]`，缺省给 default。
//
// ⚠ 不合法的值**夹紧而不是拒绝**：`?limit=0` 与 `?limit=99999` 都是使用者的手滑，
// 为了一个手滑把整份列表变成 400，代价不匹配。
func clampLimit(raw string, fallback, max int) int {
	if raw == "" {
		return fallback
	}
	value, err := strconv.Atoi(raw)
	if err != nil {
		return fallback
	}
	if value < 1 {
		return 1
	}
	if value > max {
		return max
	}
	return value
}

// rfc3339Of 把可选时间格式化；nil 落成 JSON 的 null。
//
// ⚠ 不能返回 `""`：空串在前端是"有一个时间，只是它是空"，
// 而 `null` 是"没有这个时间" —— 两者在"上次跑成没成"那张表上意思相反。
func rfc3339Of(value *time.Time) any {
	if value == nil {
		return nil
	}
	return value.UTC().Format(time.RFC3339)
}

// jsonRawMessageOf 把"可能是任何 JSON 值"的字段转成 [json.RawMessage]。
//
// ⚠ 缺失必须落成 nil 而不是 `null`：定时任务的 body 是 `Option<Value>` 语义，
// 「没给」与「给了 JSON 的 null」在调度器那边是两个分支（缺席走缺省参数，
// null 走显式空）。顺手塞 `null` 会让"这条从来没设过参数"变成"参数是 null"。
//
// 与 io 侧那个同名 helper 是两处代码而不是一个公共函数 —— 它们属于不同的层，
// 强并成一个会把存储层的语义拖进 HTTP 层。代价是这条规则要写两遍，
// 所以两边都留了指向对方的注释。
func jsonRawMessageOf(value any) json.RawMessage {
	if value == nil {
		return nil
	}
	raw, err := core.MarshalJSON(value)
	if err != nil {
		return nil
	}
	return json.RawMessage(raw)
}
