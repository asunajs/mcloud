package server

// 任务组合的对外面：清单读写 + 那枚"一键全跑"。
//
// # 为什么这一族是 `PUT`（id 在路径里）而不是 `POST`
//
// 与定时任务同一条理由（见 [Server.putSchedule]）：id 由调用方给，同一条路径重复
// 提交是同一个结果。本仓不发明 id，否则"改一个组合"与"建一个组合"分不开，
// 而导入的旧数据（`data/pipelines.json`）里的 id 是人家生成的。
//
// # 内置那一版占掉 id `"all"`，且**不许建同名行**
//
// 界面上那枚"一键全跑"与调度目标里的"跑全部任务"都指到 [PipelineRunAllID]。
// 如果允许有人建一行同名组合，那枚按钮跑的就变成他那份清单 —— 而且是**静默**变的，
// 因为查库那一步永远命中他的行。所以 [putPipeline] 直接 400 掉这个 id。
//
// # 为什么清单读写是全新的三条路由，而执行不是
//
// 冻结基准（`docs/api-frozen-routes.md` 那一族）里没有 pipelines，所以它们进
// `routes_test.go` 的 `goAdded` 表；`POST /api/pipelines/{id}/run` 与
// `POST /api/runs` 同族（同一个"受理即返回"的形状），也不在那 15 条里。

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// pipelineBody 是保存组合的请求体（全量覆盖，与 [scheduleBody] 同一形状）。
type pipelineBody struct {
	Name        string           `json:"name"`
	Description *string          `json:"description"`
	Steps       []stepBody       `json:"steps"`
	AccountIDs  []core.AccountID `json:"account_ids"`
	AccountMode string           `json:"account_mode"`
	StopOnError bool             `json:"stop_on_error"`
}

// stepBody 是一步的请求形状。
//
// 刻意不复用 [core.PipelineStep] 做入参：它的 `UnmarshalJSON` 对齐的是 **serde**
// 的语义（`task` 必填、未知字段忽略、`body` 接受任意 JSON 值），那是存量数据的
// 读取契约。写入面要的是**报错清楚**：`{"task":"sign","body":3}` 存进去之后
// 到执行时才在另一处报"参数不是一个 JSON 对象"，指不到是哪一步。
type stepBody struct {
	Task string `json:"task"`
	// Body 是这一步传给能力的参数对象；`nil` = 全部用默认值。
	Body map[string]json.RawMessage `json:"body"`
	// ContinueOnError 是三态：nil = 跟随组合级 `stop_on_error`。
	ContinueOnError *bool `json:"continue_on_error"`
}

func (s *Server) listPipelines(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	pipelines, err := s.store.ListPipelines(r.Context(), scopeOf(auth))
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	rows := make([]map[string]any, 0, len(pipelines)+1)
	// 内置那一版排在**最前面**：这一族在界面上的第一个用途就是"那枚全跑按钮有没有
	// 地方选"。它没有行，所以只能在这里合成 —— 往库里种一行等于让第一个改它的
	// 人替所有人改了默认值（判据见 `pipeline_run.go` 抬头）。
	rows = append(rows, pipelinePayload(allRunPipeline(), true))
	for i := range pipelines {
		rows = append(rows, pipelinePayload(pipelines[i], false))
	}
	writeJSON(w, http.StatusOK, map[string]any{"pipelines": rows})
}

// putPipeline 新建或覆盖一个组合。
func (s *Server) putPipeline(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := r.PathValue("id")
	if perr := validateScheduleID(id); perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	if id == PipelineRunAllID {
		ErrDomain(core.NewBadRequest(
			"「" + PipelineRunAllID + "」是内置「一键全跑」占用的 id，不能另建同名组合")).Write(w)
		return
	}
	body, ok := decodeJSON[pipelineBody](w, r)
	if !ok {
		return
	}
	ctx := r.Context()
	scope := scopeOf(auth)
	now := s.Now()

	name := strings.TrimSpace(body.Name)
	if name == "" {
		ErrDomain(core.NewBadRequest("组合名不能为空")).Write(w)
		return
	}
	if len([]rune(name)) > maxNameLen {
		ErrDomain(core.NewBadRequest("组合名最多 " + strconv.Itoa(maxNameLen) + " 个字")).Write(w)
		return
	}
	mode, perr := pipelineAccountMode(body.AccountMode)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	pipeline := core.NewPipeline(id, name)
	pipeline.Owner = &auth.User.ID
	pipeline.Description = body.Description
	pipeline.AccountMode = mode
	pipeline.StopOnError = body.StopOnError
	pipeline.Steps, perr = pipelineSteps(body.Steps)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	pipeline.AccountIDs = body.AccountIDs
	if pipeline.AccountIDs == nil {
		pipeline.AccountIDs = []core.AccountID{}
	}
	// 逐账号校验归属，与 [Server.putSchedule] 同一条判据：这一列将来会真的拿去执行，
	// 混进别人的账号等于让 A 的组合去打 B 的账号。空 = 全部启用账号，不报错。
	for _, accountID := range pipeline.AccountIDs {
		account, err := s.store.GetAccount(ctx, scope, accountID)
		if err != nil {
			ErrDomain(asDomain(err)).Write(w)
			return
		}
		if account == nil {
			ErrDomain(core.NewBadRequest(
				"云盘账号不存在或不属于你: " + accountID.String())).Write(w)
			return
		}
	}
	// **存进去之前**就把整条清单跑一遍预检：一个都跑不了的组合存进库，
	// 表现是"界面上建成功了，点执行永远 400"，而那时用户已经忘了自己填过什么。
	if _, perr := pipelineRequests(pipeline); perr != nil {
		ErrDomain(perr).Write(w)
		return
	}

	// 与 putSchedule 同一格"留旧值"的纪律：`created_at` 与那四列执行记账
	// 都不在请求体里，全量覆盖会把"上次跑成没成"抹掉 —— 而那正是用户想看的东西。
	existing, err := s.store.GetPipeline(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if existing != nil {
		pipeline.CreatedAt = existing.CreatedAt
		pipeline.LastRunAt = existing.LastRunAt
		pipeline.LastStatus = existing.LastStatus
		pipeline.LastError = existing.LastError
		pipeline.LastDurationMS = existing.LastDurationMS
	}
	if pipeline.CreatedAt == nil {
		pipeline.CreatedAt = &now
	}
	pipeline.UpdatedAt = &now

	if err := s.store.UpsertPipeline(ctx, scope, &pipeline); err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"pipeline": pipelinePayload(pipeline, false)})
}

// deletePipeline 删除一个组合。
//
// 被定时任务引用时**拒绝**，不级联删那几条任务：级联会静默地把用户另一处配好的
// "每天 08:10 跑它"一起带走，而删组合的人多半只想清一份清单。让他先去改那几条，
// 界面上那句错误会指名是哪几条。
func (s *Server) deletePipeline(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := r.PathValue("id")
	ctx := r.Context()
	scope := scopeOf(auth)
	if id == PipelineRunAllID {
		ErrDomain(core.NewBadRequest("内置「一键全跑」没有行，也不需要删")).Write(w)
		return
	}
	schedules, err := s.store.ListSchedules(ctx, scope)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	var users []string
	for _, schedule := range schedules {
		if schedule.Target != nil && schedule.Target.Kind == core.TargetPipeline &&
			schedule.Target.ID == id {
			users = append(users, "「"+schedule.Name+"」")
		}
	}
	if len(users) > 0 {
		ErrDomain(core.NewBadRequest(
			"这些定时任务还在用它，先改掉它们的目标：" + strings.Join(users, "、"))).Write(w)
		return
	}
	deleted, err := s.store.DeletePipeline(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if !deleted {
		// "不属于你"与"不存在"同一结论（存储层已经这么实现，不泄漏存在性）。
		ErrDomain(core.NewNotFound("任务组合 " + id)).Write(w)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// runPipelineRoute 是那枚按钮的后端：`POST /api/pipelines/{id}/run`。
//
// # 为什么回 202 而不给 runId 配对
//
// `POST /api/runs` 能在响应里给配对，是因为它只有一步、账号列表在建计划时就定了。
// 组合有十七八步，每一步的 runId 要等上一步跑完才分配得出来（账号列表也可能每步
// 不同）。所以这里只说"受理了"，界面上的进度另有两条路：订阅随后出现在历史里的
// 那些单趟 run，或者等收尾那一条推送。
//
// ⚠ 预检**同步**做完再走：清单本身有问题（能力不存在、参数键拼错）时回 400，
// 不留一条"受理了但一步没跑"的记录 —— 那种记录在界面上的表现与"还在跑"一模一样。
func (s *Server) runPipelineRoute(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := r.PathValue("id")
	ctx := r.Context()
	scope := scopeOf(auth)
	if s.runs == nil {
		ErrDomain(core.NewInternal("这个服务端没有装配执行面")).Write(w)
		return
	}
	pipeline, perr := s.pipelineToRun(ctx, scope, id)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	requests, perr := pipelineRequests(pipeline)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	s.runPipelineAsync(ctx, scope, pipeline, nil)
	writeJSON(w, http.StatusAccepted, map[string]any{
		"pipelineId": pipeline.ID,
		"name":       pipeline.Name,
		"steps":      len(requests),
	})
}

// pipelineToRun 把路径上那个 id 翻成"要跑的清单"。
//
// 保留 id [PipelineRunAllID] 走内置那份定义（库里没有它的行，所以**不能**先查库）；
// 其余先按 scope 查，查不到回 404 —— 别人的组合与不存在的组合同一个结论，
// 与 [Server.resolveTargets] 同一口径。
func (s *Server) pipelineToRun(
	ctx context.Context, scope core.Scope, id string,
) (core.Pipeline, *core.Error) {
	if id == PipelineRunAllID {
		return allRunPipeline(), nil
	}
	pipeline, err := s.store.GetPipeline(ctx, scope, id)
	if err != nil {
		return core.Pipeline{}, asDomain(err)
	}
	if pipeline == nil {
		return core.Pipeline{}, core.NewNotFound("任务组合 " + id)
	}
	return *pipeline, nil
}

// pipelineAccountMode 校验执行方式那一栏。
//
// 空串 = 没填，落 [core.ModeSequential]（零值，与 Rust 侧 `#[default]` 同一档）。
// 未知文本**报错而不是兜成默认**：这一栏决定会不会并发打上游，静默改写用户的
// 选择正是旧实现里最难查的那类差异。
func pipelineAccountMode(raw string) (core.AccountMode, *core.Error) {
	text := strings.TrimSpace(raw)
	if text == "" {
		return core.ModeSequential, nil
	}
	if text != core.ModeSequential.String() && text != core.ModeParallel.String() {
		return core.ModeSequential, core.NewBadRequest(
			"执行方式只认「" + core.ModeSequential.String() + "」「" +
				core.ModeParallel.String() + "」")
	}
	return core.ParseAccountMode(text), nil
}

// pipelineSteps 把请求体里的步骤翻成模型，并逐个验能力。
//
// 空清单**在这里**就拒（而不是等到执行时靠 [pipelineRequests] 那句"一步都没有"）：
// 存一个空组合进库的唯一后果是界面上多一条永远跑不动的行。
func pipelineSteps(steps []stepBody) ([]core.PipelineStep, *core.Error) {
	if len(steps) == 0 {
		return nil, core.NewBadRequest("至少要有一步")
	}
	out := make([]core.PipelineStep, 0, len(steps))
	for i, step := range steps {
		task := strings.TrimSpace(step.Task)
		if task == "" {
			return nil, core.NewBadRequest(fmt.Sprintf("第 %d 步没有任务名", i+1))
		}
		if _, found := capabilities.Get(task); !found {
			return nil, core.NewBadRequest(fmt.Sprintf("第 %d 步没有这个能力：%s", i+1, task))
		}
		encoded, err := json.Marshal(step.Body)
		if err != nil {
			return nil, core.NewBadRequest(fmt.Sprintf("第 %d 步的参数解不开：%s", i+1, err))
		}
		// `nil` 与 `{}` 在这里合成同一份（都不传参数），而**不能**写成 `null`：
		// 模型层会把 `null` 归一成 nil，但序列化出来是四字节文本，
		// 往返一圈之后 `Body == nil` 的判定会变（[core.PipelineStep.UnmarshalJSON]
		// 那条注释说的就是这个坑）。空对象没有这个两面性。
		if step.Body == nil {
			encoded = []byte("{}")
		}
		out = append(out, core.PipelineStep{
			Task: task, Body: encoded, ContinueOnError: step.ContinueOnError,
		})
	}
	return out, nil
}

// pipelinePayload 是组合的对外形状；`builtIn` 那一版没有行，只有清单本身。
//
// `steps` 直接带 [core.PipelineStep] 的序列化结果，因此线格式是 `task` /
// `body` / `continue_on_error`（serde 冻结形状），与外层这些 camelCase 键**刻意不一致**：
// 改它等于改存量数据的读法，那不该顺带发生在"给组合加个界面"这一轮里。
func pipelinePayload(pipeline core.Pipeline, builtIn bool) map[string]any {
	ids := make([]string, 0, len(pipeline.AccountIDs))
	for _, id := range pipeline.AccountIDs {
		ids = append(ids, id.String())
	}
	steps := pipeline.Steps
	if steps == nil {
		steps = []core.PipelineStep{}
	}
	return map[string]any{
		"id":             pipeline.ID,
		"name":           pipeline.Name,
		"description":    pipeline.Description,
		"steps":          steps,
		"accountIds":     ids,
		"accountMode":    pipeline.AccountMode.String(),
		"stopOnError":    pipeline.StopOnError,
		"builtIn":        builtIn,
		"createdAt":      rfc3339Of(pipeline.CreatedAt),
		"updatedAt":      rfc3339Of(pipeline.UpdatedAt),
		"lastRunAt":      rfc3339Of(pipeline.LastRunAt),
		"lastStatus":     pipeline.LastStatus,
		"lastError":      pipeline.LastError,
		"lastDurationMs": pipeline.LastDurationMS,
	}
}
