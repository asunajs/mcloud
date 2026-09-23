package mcloudio

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 账号
// ─────────────────────────────────────────────────────────────────────────────

func writeAccounts(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	views []core.LegacyAccountView,
) (int64, error) {
	for _, view := range views {
		account := core.NewAccount(view.AccountID.String())
		// 导入即归属：旧数据里的账号交给执行导入的这个用户
		account.Owner = scope.Owner()
		account.Nickname = view.Nickname
		// 手机号非空时 AccountID 本身就是掩码，直接复用，避免两处掩码逻辑跑偏
		if view.Phone != "" {
			masked := view.AccountID.String()
			account.PhoneMasked = &masked
		}
		account.Platform = view.Platform
		account.Enabled = true
		// 保持旧配置里的顺序
		account.SortOrder = int64(view.Index) - 1
		// `0` 在旧语义里是"没有过期信息"，不是"1970 年过期"
		if view.AuthExpire != 0 {
			expire := view.AuthExpire
			account.AuthExpire = &expire
		}
		if err := store.UpsertAccount(ctx, scope, &account); err != nil {
			return 0, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
	}
	return int64(len(views)), nil
}

// fillCredentials 把旧 `asign.json` 里的整串 auth（与手机号）回填进 account_state。
//
// # 为什么单独一步，而不是塞进 writeAccounts / writeState
//
// 那两处写的是**两张不同的表**（`accounts` 与 `account_state`），而且各自被
// "源文件哈希没变就跳过"的门控着。凭据回填需要跨过那道门控 —— 理由见下。
//
// # 为什么**不受 source_changed 门控**
//
// writeState 里那段注释把问题说得很清楚：一条被哈希门控着的回填，对**已经导入过的库**
// 永远不会再跑 —— 于是"同一个版本的程序"会产出两种库形态（老库 NULL、新库有值），
// 而这两种形态在别处长得分毫不差，排障时根本想不到差别在这里。
//
// 所以这里换一条判据：**按状态判**（"这一列空着就填"），不按哈希判。
// [SqliteStore.FillAccountCredentials] 只填空缺、不覆盖已有值，于是它天然幂等。
//
// # 为什么按**下标**对上号
//
// `DescribeLegacyAccounts` 是顺序 map，第 i 个视图的 Index 恒为 i+1。
// 这里断言钉住这个不变量 —— 将来有人往那里面加过滤，应当**当场炸**，
// 而不是悄悄把 A 账号的凭据回填到 B 账号头上。
//
// # 什么算"能填的 auth"
//
// 只有 [core.ParseAuth] 认得的才算。旧配置里可能留着占位串或半截手改过的值，
// 把那种东西存进去，下一次请求会带着一个格式不对的头出去，
// **失败长得像"未登录"** —— 那正是这个项目最不想要的错误方向。
func fillCredentials(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	configs []core.LegacyAccountConfig,
	views []core.LegacyAccountView,
) (int64, error) {
	var filled int64
	for position := range configs {
		if position >= len(views) {
			break
		}
		config, view := configs[position], views[position]
		if view.Index != position+1 {
			// 张冠李戴比少填一条糟得多 —— 直接炸
			return filled, &LegacyImportError{
				Kind: LegacyImportIdentity,
				Err: fmt.Errorf("第 %d 条配置的视图序号是 %d，两者必须一致",
					position+1, view.Index),
			}
		}

		var authorization *string
		if info, ok := core.ParseAuth(config.Auth); ok {
			value := info.Auth
			authorization = &value
		}
		var phone *string
		if view.Phone != "" {
			value := view.Phone
			phone = &value
		}
		if authorization == nil && phone == nil {
			continue
		}

		// 先看一眼有没有缺口。回填本身是幂等的，但**报告要诚实** ——
		// 每次都报"回填了 2 个账号"，这个数字从第二次运行起就失去意义了。
		existing, err := store.GetAccountState(ctx, scope, view.AccountID)
		if err != nil {
			return filled, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		hasAuth := existing != nil && existing.Authorization != nil
		hasPhone := existing != nil && existing.Phone != nil
		if (authorization == nil || hasAuth) && (phone == nil || hasPhone) {
			continue
		}

		if err := store.FillAccountCredentials(ctx, scope, view.AccountID, authorization, phone); err != nil {
			return filled, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		filled++
	}
	return filled, nil
}

func writeState(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	value any,
	identity map[string]core.AccountID,
	report *MigrationReport,
) (int64, error) {
	root, _ := value.(map[string]any)
	accounts, ok := root["accounts"].(map[string]any)
	if !ok {
		return 0, nil
	}

	// ⚠ map 遍历顺序是**故意随机化**的（§6.1 #21）。旧 state.json 的键是数据，
	// 逐条写入互不依赖，所以顺序不影响结果 —— 但报错顺序会变，所以**排序**再走。
	keys := make([]string, 0, len(accounts))
	for key := range accounts {
		keys = append(keys, key)
	}
	sortStrings(keys)

	var written int64
	for _, key := range keys {
		raw, err := core.MarshalJSON(accounts[key])
		if err != nil {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueUnparsableAccountState,
				Key:   key,
				Error: err.Error(),
			})
			continue
		}
		state, ok := core.ParseJSONTo[core.LegacyAccountState](string(raw))
		if !ok {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueUnparsableAccountState,
				Key:   key,
				Error: "形状不合",
			})
			continue
		}

		// 未知字段如实报出，而不是静默丢弃 —— 旧版本写过的东西凭什么当它不存在
		if unknown := state.UnknownFields(); len(unknown) != 0 {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:   IssueUnknownStateFields,
				Key:    key,
				Fields: unknown,
			})
		}

		if state.IsEmpty() {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind: IssueEmptyAccountState,
				Key:  key,
			})
			continue
		}

		// 键是旧 accountKey（手机号原文/昵称/"account"）。映射不到就不导入：
		// 凭空造一个账号会把"这是谁的数据"这件事变成永久的谜
		accountID, ok := identity[key]
		if !ok {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind: IssueOrphanAccountState,
				Key:  key,
			})
			continue
		}

		// state.json 对 authorization / phone **没有主张** —— 它本来就没有这两个字段
		//（键才是手机号原文/昵称）。而 UpsertAccountState 是**整体覆盖**
		//（nil = 清空），直接传 nil 会先把库里已有的凭据抹掉，再由 fillCredentials
		// 从 asign.json 补回来 —— 补回来的却是**旧配置里的串**：
		// 如果这中间用户重登过一次，新鲜凭据就被换成了旧的，而且事后完全看不出发生过什么。
		//
		// 所以这里把库里现有的值**原样带过去**：语义上就是"这次快照对这两列没有新信息"。
		existing, err := store.GetAccountState(ctx, scope, accountID)
		if err != nil {
			return written, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}

		newState := core.NewAccountState(accountID.String())
		if state.JWTToken != nil {
			newState.JWTToken = secretOrNil(*state.JWTToken)
		}
		if state.DeviceID != nil {
			newState.DeviceID = secretOrNil(*state.DeviceID)
		}
		if state.UserDomainID != nil {
			id := core.UserDomainID(*state.UserDomainID)
			newState.UserDomainID = &id
		}
		if state.CloudPhoneRedpackToken != nil {
			newState.RedpackToken = secretOrNil(*state.CloudPhoneRedpackToken)
		}
		if existing != nil {
			newState.Authorization = existing.Authorization
			newState.Phone = existing.Phone
		}
		if err := store.UpsertAccountState(ctx, scope, &newState); err != nil {
			return written, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		written++
	}

	report.AccountStates = int(written)
	return written, nil
}

// secretOrNil 把字符串包成 [core.SecretValue]；空串按"没有"处理。
//
// 旧文件里"空串"常常是"曾经有、后来被清掉"，而一个空的凭据字段
// 会让下游认为"有凭据、只是解不开" —— 那是最难查的一类失败。
func secretOrNil(text string) *core.SecretValue {
	if text == "" {
		return nil
	}
	value := core.NewSecretValue(text)
	return &value
}

// ─────────────────────────────────────────────────────────────────────────────
// 运行历史
// ─────────────────────────────────────────────────────────────────────────────

func writeHistory(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	value any,
	views []core.LegacyAccountView,
	report *MigrationReport,
) (int64, error) {
	items := arrayField(value, "runs")

	var written int64
	for _, entry := range parseEachIndexed[core.LegacyRun]("history.json", items, report) {
		index, run := entry.Index, entry.Value

		startedAt := core.ParseRFC3339(run.Start)
		if startedAt == nil {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueBadRunTimestamp,
				File:  "history.json",
				Index: index + 1,
				Start: run.Start,
			})
			continue
		}

		// runs[].account 存的是**展示名**，不是标识 —— 这是旧版留下的第二个标识体系
		var accountID *core.AccountID
		switch matches := core.FindByLabel(views, run.Account); len(matches) {
		case 1:
			id := matches[0].AccountID
			accountID = &id
		case 0:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:   IssueUnresolvedRunAccount,
				Index:  index + 1,
				Label:  run.Account,
				Reason: "匹配不到任何账号",
			})
		default:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:   IssueUnresolvedRunAccount,
				Index:  index + 1,
				Label:  run.Account,
				Reason: "对应多个账号（展示名重复）",
			})
		}

		status := core.StatusFailed
		if run.OK {
			status = core.StatusDone
		}
		record := core.RunRecord{
			ID:         legacyRunID(run),
			AccountID:  accountID,
			Capability: run.Task,
			Status:     status,
			StartedAt:  *startedAt,
			FinishedAt: core.ParseRFC3339(run.End),
			// 旧数据从未采到"净增云朵"（原项目该字段的写入被注释掉了），
			// 编不出来就不编：留空比瞎填一个 0 诚实
			CloudGained: nil,
			CloudAfter:  nil,
			Message:     run.Error,
		}
		if err := store.InsertRun(ctx, scope, &record); err != nil {
			return written, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		written++
	}

	report.Runs = int(written)
	return written, nil
}

// legacyRunID 是旧运行记录的稳定 id：内容相同即同一条。
//
// 用内容哈希而不是自增序号，是为了让"重跑导入"天然不产生重复行。
// 键取 `task|account|start|end` —— 这四段就是旧记录的**身份**：
// 重排文件、加空行都不会让同一条历史换个 id。
//
// `logs` / `result` **刻意不参与哈希**：它们体积大（夹具里 history.json 有 97KB，
// 几乎全是日志），且同一账号同一时刻不可能有两条不同的运行。
// 把它们算进去只会让"日志被截断上限动过一次"变成一次全量 id 变更。
func legacyRunID(run core.LegacyRun) string {
	canonical := strings.Join([]string{run.Task, run.Account, run.Start, run.End}, "|")
	return "legacy-" + core.MD5Hex([]byte(canonical))[:16]
}

// ─────────────────────────────────────────────────────────────────────────────
// 任务组合
// ─────────────────────────────────────────────────────────────────────────────

func writePipelines(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	value any,
	views []core.LegacyAccountView,
	report *MigrationReport,
) (int64, error) {
	items := arrayField(value, "pipelines")

	var written int64
	for _, entry := range parseEachIndexed[core.LegacyPipeline]("pipelines.json", items, report) {
		pipeline := entry.Value
		at := fmt.Sprintf("pipelines.json「%s」", pipeline.Name)

		if len(pipeline.Steps) == 0 {
			report.Issues = append(report.Issues, MigrationIssue{Kind: IssueEmptySteps, At: at})
			continue
		}

		// 废弃的步骤级账号：旧版读取时就丢掉，这里也丢，但要说一声
		for index, step := range pipeline.Steps {
			if step.LegacyAccount != nil {
				report.Issues = append(report.Issues, MigrationIssue{
					Kind: IssueDroppedStepAccount,
					At:   fmt.Sprintf("%s 第 %d 步", at, index+1),
				})
			}
		}

		accountIDs, ok := resolvePipelineAccounts(views, &pipeline, at, report)
		if !ok {
			// 组合没有 `enabled` 字段，没法"导入但停用"，只能整条跳过
			report.Issues = append(report.Issues, MigrationIssue{Kind: IssuePipelineSkipped, At: at})
			continue
		}

		id := pipeline.ID
		if trimmed := nonEmpty(id); trimmed != nil {
			id = *trimmed
		} else {
			id = fallbackID("pipe", pipeline.Name)
		}
		name := pipeline.Name
		if name == "" {
			name = "未命名组合"
		}
		accountMode := core.ModeSequential
		if pipeline.AccountMode != nil && *pipeline.AccountMode == "parallel" {
			accountMode = core.ModeParallel
		}
		// 旧版注释写明"默认 true"
		stopOnError := true
		if pipeline.StopOnError != nil {
			stopOnError = *pipeline.StopOnError
		}
		steps := make([]core.PipelineStep, 0, len(pipeline.Steps))
		for _, step := range pipeline.Steps {
			steps = append(steps, core.PipelineStep{
				Task:            step.Task,
				Body:            rawMessageOf(step.Body),
				ContinueOnError: step.ContinueOnError,
			})
		}

		if err := store.UpsertPipeline(ctx, scope, &core.Pipeline{
			ID:             id,
			Owner:          scope.Owner(),
			Name:           name,
			Description:    pipeline.Description,
			Steps:          steps,
			AccountIDs:     accountIDs,
			AccountMode:    accountMode,
			StopOnError:    stopOnError,
			CreatedAt:      core.ParseRFC3339(derefOrEmpty(pipeline.CreatedAt)),
			UpdatedAt:      core.ParseRFC3339(derefOrEmpty(pipeline.UpdatedAt)),
			LastRunAt:      core.ParseRFC3339(derefOrEmpty(pipeline.LastRunAt)),
			LastStatus:     parsePipelineStatus(derefOrEmpty(pipeline.LastStatus)),
			LastError:      pipeline.LastError,
			LastDurationMS: pipeline.LastDurationMS,
		}); err != nil {
			return written, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		written++
	}

	report.Pipelines = int(written)
	return written, nil
}

// resolvePipelineAccounts 解析组合的账号选择。**任一条无法解析就返回 false** ——
// 组合没有"停用"这个状态，少导入一个账号等于悄悄扩大了它的作用范围。
func resolvePipelineAccounts(
	views []core.LegacyAccountView,
	pipeline *core.LegacyPipeline,
	at string,
	report *MigrationReport,
) ([]core.AccountID, bool) {
	values := core.NormalizePipelineAccounts(derefSlice(pipeline.Accounts), pipeline.LegacyAccount)
	if values == nil {
		// nil = 全部账号
		return []core.AccountID{}, true
	}

	ids := make([]core.AccountID, 0, len(values))
	for _, reference := range values {
		ref := reference
		advice := core.AdviseReference(views, &ref)
		switch advice.Kind {
		case core.AdviceAllAccounts:
			// accounts 里出现空值已被 normalize 剔除，走到这里说明是别的东西
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:   IssueDeadAccountRef,
				At:     at,
				Raw:    jsonText(reference),
				Action: "跳过整条组合",
			})
			return nil, false
		case core.AdviceSame:
			ids = append(ids, views[advice.Index-1].AccountID)
		case core.AdviceRecoveredFromPhone:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:    IssueRecoveredAccountRef,
				At:      at,
				Digits:  advice.Digits,
				Account: views[advice.Index-1].AccountID.String(),
			})
			ids = append(ids, views[advice.Index-1].AccountID)
		case core.AdviceWasNeverEffective:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:   IssueDeadAccountRef,
				At:     at,
				Raw:    advice.Raw,
				Action: "跳过整条组合",
			})
			return nil, false
		}
	}
	return ids, true
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时任务
// ─────────────────────────────────────────────────────────────────────────────

func writeSchedules(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	value any,
	views []core.LegacyAccountView,
	report *MigrationReport,
) (int64, error) {
	items := arrayField(value, "schedules")

	var written int64
	for _, entry := range parseEachIndexed[core.LegacySchedule]("schedules.json", items, report) {
		schedule := entry.Value
		at := fmt.Sprintf("schedules.json「%s」", schedule.Name)

		// 旧版 `if (!schedule.enabled) return;` —— 缺字段等于 falsy，等于不启用
		enabled := false
		if schedule.Enabled != nil {
			enabled = *schedule.Enabled
		}

		if strings.TrimSpace(schedule.Cron) == "" {
			report.Issues = append(report.Issues, MigrationIssue{Kind: IssueEmptyCron, At: at})
			enabled = false
		}

		// 目标是二选一；旧版优先 pipelineId
		var target *core.ScheduleTarget
		pipelineID := nonEmpty(derefOrEmpty(schedule.PipelineID))
		task := nonEmpty(derefOrEmpty(schedule.Task))
		switch {
		case pipelineID != nil:
			value := core.NewScheduleTargetPipeline(*pipelineID)
			target = &value
		case task != nil:
			value := core.NewScheduleTargetCapability(*task)
			target = &value
		default:
			report.Issues = append(report.Issues, MigrationIssue{Kind: IssueMissingTarget, At: at})
			enabled = false
		}

		var accountIDs []core.AccountID
		if schedule.Account == nil {
			accountIDs = []core.AccountID{} // 未指定 = 全部账号
		} else {
			reference := schedule.Account
			advice := core.AdviseReference(views, reference)
			switch advice.Kind {
			case core.AdviceAllAccounts:
				accountIDs = []core.AccountID{}
			case core.AdviceSame:
				accountIDs = []core.AccountID{views[advice.Index-1].AccountID}
			case core.AdviceRecoveredFromPhone:
				report.Issues = append(report.Issues, MigrationIssue{
					Kind:    IssueRecoveredAccountRef,
					At:      at,
					Digits:  advice.Digits,
					Account: views[advice.Index-1].AccountID.String(),
				})
				accountIDs = []core.AccountID{views[advice.Index-1].AccountID}
			case core.AdviceWasNeverEffective:
				// 关键：**不能**回落到"全部账号"
				report.Issues = append(report.Issues, MigrationIssue{
					Kind:   IssueDeadAccountRef,
					At:     at,
					Raw:    advice.Raw,
					Action: "停用该定时任务",
				})
				enabled = false
				accountIDs = []core.AccountID{}
			}
		}

		id := schedule.ID
		if trimmed := nonEmpty(id); trimmed != nil {
			id = *trimmed
		} else {
			id = fallbackID("sched", schedule.Name)
		}
		name := schedule.Name
		if name == "" {
			name = "未命名任务"
		}
		timezone := DefaultTZ
		if trimmed := nonEmpty(derefOrEmpty(schedule.Timezone)); trimmed != nil {
			timezone = *trimmed
		}

		if err := store.UpsertSchedule(ctx, scope, &core.Schedule{
			ID:       id,
			Owner:    scope.Owner(),
			Name:     name,
			Target:   target,
			Cron:     schedule.Cron,
			Timezone: timezone,
			// ⚠ `MissPolicy` 与 `LastFireAt` 刻意不填（保持 nil）。
			// 旧文件里没有这两样：填成任何一个策略，都是替用户拍板"错过要不要补"，
			// 而那一趟可能是一次不可逆的真上游请求；填成"刚刚触发过"更是凭空造水位。
			// nil 的语义正是"没人选过 ⇒ 跟随能力声明的默认"（见 [core.MissPolicy]）。
			AccountIDs: accountIDs,
			Body:       rawMessageOf(schedule.Body),
			Enabled:    enabled,
			CreatedAt:  core.ParseRFC3339(derefOrEmpty(schedule.CreatedAt)),
			LastRunAt:  core.ParseRFC3339(derefOrEmpty(schedule.LastRunAt)),
			LastStatus: parseScheduleStatus(derefOrEmpty(schedule.LastStatus)),
			LastError:  schedule.LastError,
		}); err != nil {
			return written, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		written++
	}

	report.Schedules = int(written)
	return written, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时抢兑
// ─────────────────────────────────────────────────────────────────────────────

func writeExchange(
	ctx context.Context,
	store *SqliteStore,
	value any,
	views []core.LegacyAccountView,
	report *MigrationReport,
) (int64, error) {
	// 单对象文件：整体解析。手改坏了就应该报出来，而不是按"每条独立"硬凑一半
	raw, err := core.MarshalJSON(value)
	if err != nil {
		return 0, &LegacyImportError{Kind: LegacyImportParse, Path: "exchange.json", Err: err}
	}
	file, ok := core.ParseJSONTo[core.LegacyExchangeFile](string(raw))
	if !ok {
		report.Issues = append(report.Issues, MigrationIssue{
			Kind:  IssueUnparsableRecord,
			File:  "exchange.json",
			Index: 0,
			Error: "形状不合",
		})
		return 0, nil
	}

	windows, werr := core.NormalizeExchangeWindows(file.Windows)
	if werr != nil {
		report.Issues = append(report.Issues, MigrationIssue{
			Kind:  IssueBadExchangeWindows,
			Error: werr.Error(),
		})
		windows = core.NewExchangeConfig().Windows
	}

	bindings := []core.ExchangeBinding{}
	for index, binding := range file.Bindings {
		prizeID := readPrizeID(binding)
		if prizeID == nil {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueBadPrizeID,
				Index: index,
				Raw:   jsonText(binding.PrizeID),
			})
			continue
		}

		if binding.Account == nil {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueBindingWithoutAccount,
				Index: index,
			})
			continue
		}

		var accountID core.AccountID
		advice := core.AdviseReference(views, binding.Account)
		switch advice.Kind {
		case core.AdviceAllAccounts:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueBindingWithoutAccount,
				Index: index,
			})
			continue
		case core.AdviceSame:
			accountID = views[advice.Index-1].AccountID
		case core.AdviceRecoveredFromPhone:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:    IssueRecoveredAccountRef,
				At:      fmt.Sprintf("exchange.json 第 %d 条绑定", index),
				Digits:  advice.Digits,
				Account: views[advice.Index-1].AccountID.String(),
			})
			accountID = views[advice.Index-1].AccountID
		case core.AdviceWasNeverEffective:
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:   IssueDeadAccountRef,
				At:     fmt.Sprintf("exchange.json 第 %d 条绑定", index),
				Raw:    advice.Raw,
				Action: "跳过该绑定",
			})
			continue
		}

		// 旧版按 `account:prizeId` 去重，保留首次出现
		duplicate := false
		for _, existing := range bindings {
			if existing.AccountID == accountID && existing.PrizeID == *prizeID {
				duplicate = true
				break
			}
		}
		if duplicate {
			continue
		}
		bindings = append(bindings, core.ExchangeBinding{
			AccountID: accountID,
			PrizeID:   *prizeID,
			PrizeName: binding.PrizeName,
		})
	}

	config := core.NewExchangeConfig()
	config.Enabled = file.Enabled && len(bindings) != 0
	config.Windows = windows
	config.Bindings = bindings
	config.ScheduleIDs = file.ScheduleIDs
	config.UpdatedAt = core.ParseRFC3339(derefOrEmpty(file.UpdatedAt))
	if err := store.PutExchangeConfig(ctx, &config); err != nil {
		return 0, &LegacyImportError{Kind: LegacyImportStore, Err: err}
	}
	return 1, nil
}

// readPrizeID 读奖项 ID：旧版走 `Number(...)`，所以数字字符串也算数。
//
// ⚠ Go 侧要显式 switch 三种形态（[json.Number] / string / 原生整数）——
// 漏掉任何一种的表现是"值恒为 nil"且**不报错**，见 §6.1 #40。
func readPrizeID(binding core.LegacyBinding) *int64 {
	if binding.PrizeID == nil {
		return nil
	}
	var parsed int64
	switch v := (*binding.PrizeID).(type) {
	case json.Number:
		n, err := v.Int64()
		if err != nil {
			return nil
		}
		parsed = n
	case string:
		n, err := strconv.ParseInt(strings.TrimSpace(v), 10, 64)
		if err != nil {
			return nil
		}
		parsed = n
	case float64:
		parsed = int64(v)
	default:
		return nil
	}
	if parsed <= 0 {
		return nil
	}
	return &parsed
}

// ─────────────────────────────────────────────────────────────────────────────
// 界面偏好
// ─────────────────────────────────────────────────────────────────────────────

func writeWebui(
	ctx context.Context,
	store *SqliteStore,
	value any,
	report *MigrationReport,
) (int64, error) {
	raw, err := core.MarshalJSON(value)
	if err != nil {
		return 0, &LegacyImportError{Kind: LegacyImportParse, Path: "webui.json", Err: err}
	}
	file, ok := core.ParseJSONTo[core.LegacyWebuiFile](string(raw))
	if !ok {
		report.Issues = append(report.Issues, MigrationIssue{
			Kind:  IssueUnparsableRecord,
			File:  "webui.json",
			Index: 0,
			Error: "形状不合",
		})
		// 读不出来就用默认值：界面偏好不值得让一次迁移失败
		config := core.NewWebuiConfig()
		if err := store.PutWebuiConfig(ctx, &config); err != nil {
			return 0, &LegacyImportError{Kind: LegacyImportStore, Err: err}
		}
		return 0, nil
	}
	config := core.NormalizeWebui(&file)
	if err := store.PutWebuiConfig(ctx, &config); err != nil {
		return 0, &LegacyImportError{Kind: LegacyImportStore, Err: err}
	}
	return 1, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 取值小工具
// ─────────────────────────────────────────────────────────────────────────────

// arrayField 取对象里的数组字段；不是数组时返回 nil（旧文件里"没这个键"很常见）。
func arrayField(value any, field string) []any {
	root, ok := value.(map[string]any)
	if !ok {
		return nil
	}
	items, ok := root[field].([]any)
	if !ok {
		return nil
	}
	return items
}

func derefOrEmpty(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

// derefSlice 把 `*[]any` 摊成 `[]any`，**保留 nil 与空切片的区别**。
//
// ⚠ 这个区别是语义：nil = 没给，空切片 = 给了空数组。
// [core.NormalizePipelineAccounts] 靠它决定要不要去看废弃的单值字段。
func derefSlice(p *[]any) []any {
	if p == nil {
		return nil
	}
	return *p
}

func jsonText(value any) string {
	if value == nil {
		return ""
	}
	raw, err := core.MarshalJSON(value)
	if err != nil {
		return fmt.Sprint(value)
	}
	return string(raw)
}

// parsePipelineStatus 认不出就返回 nil（不归默认值）。
func parsePipelineStatus(raw string) *core.PipelineStatus {
	status, ok := core.ParsePipelineStatus(raw)
	if !ok {
		return nil
	}
	return &status
}

func parseScheduleStatus(raw string) *core.ScheduleStatus {
	status, ok := core.ParseScheduleStatus(raw)
	if !ok {
		return nil
	}
	return &status
}

// sortStrings 是 sort.Strings 的一层薄包装：
// 本文件里所有"遍历 map 之前先排序"都走它，理由写在一处就够了（§6.1 #21）。
func sortStrings(values []string) {
	for i := 1; i < len(values); i++ {
		for j := i; j > 0 && values[j] < values[j-1]; j-- {
			values[j], values[j-1] = values[j-1], values[j]
		}
	}
}

// 让 time 的用途显式：本文件的时间一律来自 core.ParseRFC3339，
// 直接 time.Now() 会让"导入时刻"在测试里钉不住。
var _ = time.Now

// rawMessageOf 把"可能是任何 JSON 值"的旧字段转成 [json.RawMessage]。
//
// ⚠ nil 必须落成 nil 而不是 `null`：新表的这两列是 `Option<Value>` 语义，
// 「没给」与「给了 JSON 的 null」在下游是两个分支（缺席走缺省，null 走显式空）。
// 顺手塞 `null` 会让"这条从来没有参数"变成"这条的参数是 null"。
func rawMessageOf(value *any) json.RawMessage {
	if value == nil {
		return nil
	}
	raw, err := core.MarshalJSON(*value)
	if err != nil {
		return nil
	}
	return json.RawMessage(raw)
}
