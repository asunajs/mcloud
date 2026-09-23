package server

// 触发执行：`POST /api/runs` —— 界面上那个"开始"按钮的后端。
//
// # 为什么是 202 而不是 200
//
// 这条链**必须**异步：一次直播口令的运行要连着听 60 秒弹幕（每个账号各一次、
// 串行跑），而一个 HTTP 请求不该挂那么久 —— 挂着的后果是浏览器或反代超时，
// 用户看到一个与"任务其实还在跑"毫无关系的连接错误。
//
// 于是这个接口的**全部内容**就是"这些 runId 归你了"：响应给出账号↔runId 的配对，
// 前端拿它们去订阅 `GET /api/runs/{id}/events`（SSE）。
// runId 之所以要**预分配**（见 [app.RunPlan.RunIDs]）就是为了这件事：
// 账号是串行跑的，第二个要等第一个跑完才轮到，现分配的话响应里只能给出第一个。
//
// # 三种"订阅之后发现什么都没有"的窗口（界面上都是那次订阅拿到 404）
//
//   - **账号正被另一趟占着**：互斥判据在 [mcloudexec.RunRegistry]，被挡下的账号
//     **不写** `runs` 行（它根本没开跑），所以它那个预分配的 runId 查不到东西。
//     这一条在 202 时刻**无法预知**（准入发生在跑的那一刻），所以宁可让它以
//     "404 = 这一趟没有执行"呈现，也不在响应里假装"每个账号都会有结果"。
//   - **编排自身失败**（写库失败那一类）：`Runner.Run` 直接返回错误，
//     后面的账号不再跑 —— 它们的 runId 同样没有记录。
//   - **进程重启**：跑了一半的 run 停在 `running`，事件不再增加。
//
// ⚠ 这三条都**不能**靠"响应里多带一个状态字段"补掉：202 之后服务端与这个客户端
// 之间没有别的通道了，真相只在事件流里。
//
// # 为什么能力分派不在本文件
//
// 判据（一个账号怎么跑）在领域层（[app.LiveRoomRun]、[app.SignInRun]），
// "这一条能力跑不跑、参数怎么翻成领域参数"在 [runBuilders]，
// 这里只剩**HTTP 面**：请求体怎么解、错该回什么状态码、202 里那一份配对长什么样。
// 分派表里没有的能力回 [ErrNotImplemented]，**不**假装受理：受理了却什么都不做的
// 表现是界面上那条进度流永远等不到事件。

import (
	"context"
	"encoding/json"
	"net/http"
	"sort"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 参数：HTTP 形状 → 领域参数
// ─────────────────────────────────────────────────────────────────────────────

// runRequestBody 是 `POST /api/runs` 的请求体。
//
// `Params` 是**按键取裸 JSON**而不是一个具名结构体，因为键名的唯一权威是
// 能力声明（[capabilities.Capability.Params]）：界面上的表单就是照它渲染的，
// 后端再写一份具名结构体等于同一组字段有两个定义处，而漂移的表现是
// "界面上填了、后端没收到"（于是静默用默认值跑完一趟）。
// 所以这里先按声明校验键名（[checkParamKeys]），再逐键取值。
type runRequestBody struct {
	Capability string                     `json:"capability"`
	Params     map[string]json.RawMessage `json:"params"`
}

// runParams 是"按能力声明取一栏"的那一层。
type runParams struct {
	raw  map[string]json.RawMessage
	spec map[string]core.FieldSpec
}

func newRunParams(capability capabilities.Capability, raw map[string]json.RawMessage) runParams {
	spec := make(map[string]core.FieldSpec)
	for _, field := range capability.Params() {
		spec[field.Key] = field
	}
	return runParams{raw: raw, spec: spec}
}

// label 取这一栏的中文标签：错误要说清是**哪一栏**，只报键名等于让用户去查表。
func (p runParams) label(key string) string {
	if field, ok := p.spec[key]; ok && field.Label != "" {
		return field.Label
	}
	return key
}

// take 把某一栏解进 `into`。
//
// 没给这一栏时：声明里有默认值就用它，没有则**保持零值**（那正是各栏的缺省语义：
// 口令为空 = 自动收集、账号为空 = 全部启用账号）。
//
// ⚠ 出错时**不插值**用户给的内容（与账号写面同一条纪律，见 accounts_write.go）：
// 这一句会原样显示在界面上，而"类型不对"这件事用栏目名就说清了。
func (p runParams) take(key string, into any) *core.Error {
	raw, found := p.raw[key]
	if !found {
		field, ok := p.spec[key]
		if !ok {
			// 走不到：调用方先过了 [checkParamKeys]。留 Internal 而不是 panic ——
			// "声明里其实没有这一栏"是本包的编程错误，不是使用者的错。
			return core.NewInternal("能力声明里没有参数：" + key)
		}
		if field.Default == nil {
			return nil
		}
		raw = field.Default
	}
	if err := json.Unmarshal(raw, into); err != nil {
		return core.NewBadRequest("「" + p.label(key) + "」这一栏的类型不对")
	}
	return nil
}

func (p runParams) text(key string) (string, *core.Error) {
	var value string
	if err := p.take(key, &value); err != nil {
		return "", err
	}
	return value, nil
}

func (p runParams) flag(key string) (bool, *core.Error) {
	var value bool
	if err := p.take(key, &value); err != nil {
		return false, err
	}
	return value, nil
}

func (p runParams) number(key string) (int, *core.Error) {
	var value int
	if err := p.take(key, &value); err != nil {
		return 0, err
	}
	return value, nil
}

// texts 取多选那一栏（字符串数组）。
//
// ⚠ 只认数组，不顺手接受"逗号分隔的字符串"：这一栏在界面上是多选控件，
// 宽进会让"前端传成了字符串"这种接线错误静默通过 —— 而它的表现是
// 一个都没选中，而"账号留空 = 全部账号"（见 [resolveTargets]）会把一次
// 只该跑一个账号的操作变成**跑全部账号**。
func (p runParams) texts(key string) ([]string, *core.Error) {
	var items []json.RawMessage
	if err := p.take(key, &items); err != nil {
		return nil, err
	}
	out := make([]string, 0, len(items))
	for _, item := range items {
		var value string
		if err := json.Unmarshal(item, &value); err != nil {
			return nil, core.NewBadRequest("「" + p.label(key) + "」里有不是字符串的项")
		}
		out = append(out, value)
	}
	return out, nil
}

// checkParamKeys 拒掉声明里没有的参数键。
//
// ⚠ 这条守卫存在的理由是**静默**：JSON 解码对多余键一律宽容，于是
// `skipKouling` 拼成 `skip_kouling` 不报错 —— 那一趟会照常跑，
// 只是把用户明确关掉了的那一步又做了一遍（这里是又投了一遍口令，
// 而投口令是**写操作**、会动这个账号的兑换状态）。键名的权威只有一处：能力声明。
func checkParamKeys(
	capability capabilities.Capability, raw map[string]json.RawMessage,
) *core.Error {
	declared := make(map[string]struct{})
	var keys []string
	for _, field := range capability.Params() {
		declared[field.Key] = struct{}{}
		keys = append(keys, field.Key)
	}
	var unknown []string
	for key := range raw {
		if _, ok := declared[key]; !ok {
			unknown = append(unknown, key)
		}
	}
	if len(unknown) == 0 {
		return nil
	}
	// 未知键要**排序**：map 遍历顺序是随机化的，不排的话同一份请求体两次报错
	// 会给出两份不同的文字，而界面上那句话是要给人比对与截图反馈用的。
	sort.Strings(unknown)
	return core.NewBadRequest("不认识的参数：" + strings.Join(unknown, "、") +
		"（这一项能填：" + strings.Join(keys, "、") + "）")
}

// ─────────────────────────────────────────────────────────────────────────────
// 处理器
// ─────────────────────────────────────────────────────────────────────────────

// triggerRun 受理一次执行。
//
// 校验的顺序是刻意的：**先问能力存不存在，再看参数，最后才查账号** ——
// 每一步都比下一步便宜，而"能力都不存在"时不该回一句参数错误。
func (s *Server) triggerRun(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[runRequestBody](w, r)
	if !ok {
		return
	}
	code := strings.TrimSpace(body.Capability)
	if code == "" {
		ErrDomain(core.NewBadRequest("capability 不能为空")).Write(w)
		return
	}
	capability, found := capabilities.Get(code)
	if !found {
		ErrDomain(core.NewNotFound("能力 " + code)).Write(w)
		return
	}
	if err := checkParamKeys(capability, body.Params); err != nil {
		ErrDomain(err).Write(w)
		return
	}
	if s.runs == nil {
		// 装配漏了这一份：宁可 500 也不回 202（202 之后前端就会一直等事件）。
		ErrDomain(core.NewInternal("这个服务端没有装配执行面")).Write(w)
		return
	}
	builder, wired := builderFor(capability.Code)
	if !wired {
		ErrNotImplemented("能力「" + capability.Title + "」还没有接上执行").Write(w)
		return
	}
	spec, perr := builder(r.Context(), s, scopeOf(auth), capability, body.Params)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	// `done` 传 nil：这一路的结论走事件流给前端，不需要服务端自己记账。
	accepted, perr := s.launchRun(r.Context(), spec, nil)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	writeJSON(w, http.StatusAccepted, map[string]any{
		"capability": capability.Code,
		"runs":       runPairings(accepted),
	})
}

// runPairings 是 202 里那一份账号↔runId 配对。
//
// 手写而不是把 [acceptedRun] 直接序列化：那两个字段是**对外契约**
// （前端拿 `runId` 去订阅事件流），而这个结构体还要给调度用，
// 将来加字段不该自动出网（同 [refreshAuthPayload] 那条理由）。
func runPairings(accepted []acceptedRun) []map[string]any {
	out := make([]map[string]any, 0, len(accepted))
	for _, entry := range accepted {
		out = append(out, map[string]any{
			"accountId": entry.Account.String(),
			"runId":     entry.RunID.String(),
		})
	}
	return out
}

// liveRoomRequest 解析并校验这一趟的参数与目标账号。
//
// 单独成函数是为了让"这一条接口有哪几种 400"在一处数得清。
func (s *Server) liveRoomRequest(
	p runParams, scope core.Scope, ctx context.Context,
) (app.LiveRoomParams, []core.AccountID, *core.Error) {
	codes, perr := p.text("codes")
	if perr != nil {
		return app.LiveRoomParams{}, nil, perr
	}
	listenSeconds, perr := p.number("listenSeconds")
	if perr != nil {
		return app.LiveRoomParams{}, nil, perr
	}
	koulingOnly, perr := p.flag("koulingOnly")
	if perr != nil {
		return app.LiveRoomParams{}, nil, perr
	}
	skipKouling, perr := p.flag("skipKouling")
	if perr != nil {
		return app.LiveRoomParams{}, nil, perr
	}
	// ⚠ 执行方式**不**翻成"并行"，判据与另外两条链共用一处（[sequentialOnly]）。
	mode, perr := p.text("accountMode")
	if perr != nil {
		return app.LiveRoomParams{}, nil, perr
	}
	if perr := sequentialOnly(mode); perr != nil {
		return app.LiveRoomParams{}, nil, perr
	}

	params := app.LiveRoomParams{
		Codes:         app.SplitKouling(codes),
		ListenSeconds: listenSeconds,
		KoulingOnly:   koulingOnly,
		SkipKouling:   skipKouling,
	}
	// 组合是否成立判在领域层（[app.LiveRoomParams.Check]），与 CLI 共用一份判据；
	// 这里只有本服务面的措辞。
	switch params.Check() {
	case app.LiveRoomFaultExclusiveSwitches:
		return params, nil, core.NewBadRequest(
			"「只听不投」与「只用给定口令」互斥，两者不能同时打开")
	case app.LiveRoomFaultSkipWithoutCodes:
		return params, nil, core.NewBadRequest(
			"打开「只用给定口令」时必须在「口令」那一栏至少填一个口令")
	case app.LiveRoomFaultListenNotPositive:
		return params, nil, core.NewBadRequest("「听弹幕时长（秒）」必须为正整数")
	}

	ids, perr := p.texts("accounts")
	if perr != nil {
		return params, nil, perr
	}
	targets, perr := s.resolveTargets(ctx, scope, ids)
	if perr != nil {
		return params, nil, perr
	}
	return params, targets, nil
}

// resolveTargets 把界面上选中的账号翻成"这一趟要跑的账号"。
//
// 留空 = 该用户**启用中**的全部账号（与 CLI 的 `--account 0` 同一条缺省，
// 也与参数声明里那句"留空 = 全部账号"一致）。
//
// # 为什么归属错误一律 404
//
// 别人的账号与"根本没有这个账号"必须**长得一样**：报 403 等于确认
// "这个 id 确实存在、只是不是你的"（与 [Server.runEvents] 同一口径）。
//
// # 为什么重复的 id 要去重而不是照跑
//
// 多选控件给不出重复项，出现了就是接线错误；而这里**不能**让它过去 ——
// 编排是串行的，同一账号跑两遍不会被自己的互斥键挡住（第一遍跑完才轮到第二遍），
// 于是那会变成"同一批口令投两次"。
func (s *Server) resolveTargets(
	ctx context.Context, scope core.Scope, ids []string,
) ([]core.AccountID, *core.Error) {
	accounts, err := s.store.ListAccounts(ctx, scope)
	if err != nil {
		return nil, asDomain(err)
	}
	byID := make(map[string]core.Account, len(accounts))
	var targets []core.AccountID
	for _, account := range accounts {
		byID[account.ID.String()] = account
		if len(ids) == 0 && account.Enabled {
			targets = append(targets, account.ID)
		}
	}
	seen := make(map[string]struct{}, len(ids))
	for _, raw := range ids {
		id := strings.TrimSpace(raw)
		if id == "" {
			return nil, core.NewBadRequest("「账号」里有空项")
		}
		if _, ok := seen[id]; ok {
			continue
		}
		account, ok := byID[id]
		if !ok {
			return nil, core.NewNotFound("云盘账号 " + id)
		}
		if !account.Enabled {
			// ⚠ 停用不是"不存在"：这一栏是用户**自己**勾出来的，
			// 404 会让人去查"账号是不是被删了"，而这里要做的是去账号页启用它。
			return nil, core.NewBadRequest("账号 " + id + " 已停用，先在账号页启用它")
		}
		seen[id] = struct{}{}
		targets = append(targets, account.ID)
	}
	if len(targets) == 0 {
		return nil, core.NewBadRequest("没有可执行的账号（启用中的账号为 0 个）")
	}
	return targets, nil
}
