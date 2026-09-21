package app

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// ─────────────────────────────────────────────────────────────────────────────
// 计划
// ─────────────────────────────────────────────────────────────────────────────

// MigrationPlan 是一次迁移要做的事：从哪读、写到哪、归档到哪。
type MigrationPlan struct {
	// Source 是旧数据位置（`asign.json` + `packages/api/data/`）。
	Source mcloudio.LegacySource
	// DBPath 是目标 SQLite 文件。
	DBPath string
	// BackupDir 是旧数据的归档目录。
	BackupDir string
}

// MigrationPlanFromRepoRoot 按旧项目布局从仓库根推导**读取**位置。
//
// 写入位置**不在这里推导** —— 旧仓库自始至终是只读归档，
// 把"写到哪"默认成旧仓库里的某个路径，是将来最容易误伤的默认值。
func MigrationPlanFromRepoRoot(repoRoot, dbPath, backupDir string) MigrationPlan {
	return MigrationPlan{
		Source:    mcloudio.LegacySourceFromRepoRoot(repoRoot),
		DBPath:    dbPath,
		BackupDir: backupDir,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 结果
// ─────────────────────────────────────────────────────────────────────────────

// MigrationOutcome 是迁移结果。
//
// DBPath 是指针而不是 string：预演**不建库**，用 nil 把这件事表达在类型里，
// 比让调用方去猜"预演时这个字段有没有意义"可靠。
type MigrationOutcome struct {
	// Report 是导入报告（它与 CLI `--json` / 服务端日志共用一套字段名）。
	Report mcloudio.MigrationReport
	// Archived 是归档的源文件数；预演恒为 0。
	Archived int
	// DBPath 是实际写入的库；预演为 nil。
	DBPath *string
	// DryRun 报告这次是不是预演。
	DryRun bool
}

// HumanLines 是给人看的多行输出。
//
// 渲染放在这里而不是 CLI：CLI 与服务端日志将来都会用，
// 两边各写一份就会出现"命令行说 12 条、日志说 0 条"。
func (o MigrationOutcome) HumanLines() []string {
	var lines []string

	if o.DryRun {
		lines = append(lines, "迁移预演（未写入任何文件）")
	} else {
		lines = append(lines, "迁移完成")
	}
	lines = append(lines, o.Report.Summary())

	if o.Archived > 0 {
		lines = append(lines, fmt.Sprintf("已归档 %d 个源文件", o.Archived))
	}
	if o.DBPath != nil {
		lines = append(lines, fmt.Sprintf("数据库: %s", *o.DBPath))
	}

	issues := o.Report.IssueLines()
	if len(issues) == 0 {
		lines = append(lines, "没有需要人工确认的问题")
		return lines
	}
	lines = append(lines, "")
	lines = append(lines, fmt.Sprintf("需要确认的问题（%d 条）：", len(issues)))
	for _, issue := range issues {
		lines = append(lines, "  - "+issue)
	}
	return lines
}

// IssueCount 是问题总数。
func (o MigrationOutcome) IssueCount() int { return len(o.Report.Issues) }

// ─────────────────────────────────────────────────────────────────────────────
// 执行
// ─────────────────────────────────────────────────────────────────────────────

// RunMigration 执行（或预演）迁移。
//
// 顺序是刻意的：**归档在建库之前**。如果导入中途失败，
// 原始数据至少已经在新项目里有一份可追溯的副本。
//
// 预演走 [mcloudio.DiagnoseLegacy] —— 它在内存库上跑**完全相同**的导入路径，
// 所以"预演说没事、真跑报错"在结构上不会发生。
func RunMigration(
	ctx context.Context,
	plan MigrationPlan,
	dryRun bool,
	owner *core.UserID,
	clock core.Clock,
) (MigrationOutcome, error) {
	now := clock.Now()

	if dryRun {
		// 预演不需要用户：DiagnoseLegacy 在临时库里自造占位用户，
		// 走的是与真实导入完全相同的路径（见那里的说明）。
		report, err := mcloudio.DiagnoseLegacy(ctx, plan.Source, now)
		if err != nil {
			return MigrationOutcome{}, err
		}
		return MigrationOutcome{Report: report, Archived: 0, DBPath: nil, DryRun: true}, nil
	}

	archived, err := mcloudio.ArchiveLegacySource(plan.Source, plan.BackupDir)
	if err != nil {
		return MigrationOutcome{}, err
	}
	store, err := mcloudio.Open(plan.DBPath)
	if err != nil {
		return MigrationOutcome{}, err
	}
	defer func() { _ = store.Close() }()

	scope, err := migrationScope(ctx, store, owner)
	if err != nil {
		return MigrationOutcome{}, err
	}
	report, err := mcloudio.ImportLegacy(ctx, store, scope, plan.Source, now)
	if err != nil {
		return MigrationOutcome{}, err
	}
	dbPath := plan.DBPath
	return MigrationOutcome{Report: report, Archived: archived, DBPath: &dbPath, DryRun: false}, nil
}

// migrationScope 决定这次导入用哪个范围。
//
// 谁在操作，就把账号交给谁；没指定执行者才回落到"库里首个用户"。
//
// CLI 走的是 [core.ScopeLocal]（不指定 owner）：导入的账号保持无主，
// 等 `mcloud user init` 时由认领流程接手 —— 于是"先 migrate 再 init"
// 与"先 init 再 migrate"两种顺序都能得到正确结果。
//
// ⚠⭐ 最后那个分支（**库里连一个用户都没有**）必须落到 [core.ScopeLocal]，
// **不能报错**。Rust 侧这里踩过一次：代码原先回落到 `acting_admin`，而它
// 没有用户时直接返回 `NoOwner`，于是 CLI 那条"先 migrate"的路**整个走不通**
// —— 上面这两段注释承诺的顺序，代码自己没兑现。
//
// 保持无主是**安全**的：`accounts.owner` 落 `NULL`，而 `user init` 里的
// `ClaimUnowned` 正是按 `owner IS NULL` 认领的（`schedules` / `pipelines` 同理），
// 所以之后补建管理员时它们会被一并接管。
func migrationScope(
	ctx context.Context,
	store *mcloudio.SqliteStore,
	owner *core.UserID,
) (core.Scope, error) {
	if owner != nil {
		return core.ScopeUser(owner), nil
	}
	users, err := store.ListUsers(ctx)
	if err != nil {
		return core.Scope{}, err
	}
	if len(users) == 0 {
		return core.ScopeLocal(), nil
	}
	first := users[0].ID
	return core.ScopeUser(&first), nil
}

// HasLegacyData 报告是否存在可迁移的旧数据（供"首次启动自动迁移"判断）。
//
// 只要求 **`asign.json` 或任一数据文件**存在：旧项目里 `schedules.json` 实测为空、
// 在某些机器上可能在导入前还没被创建过，把"必须全都在"当条件会静默漏掉整次迁移。
func HasLegacyData(source mcloudio.LegacySource) bool {
	return len(source.ExistingFiles()) != 0
}

// OutcomeToJSON 序列化给外部消费（CLI `--json` / 日志）。
//
// ⚠ `dbPath` 在预演时输出 JSON 的 `null`，不是空串 —— 脚本靠它区分
// "这次没写库"与"写到了一个叫空串的路径"。
func OutcomeToJSON(outcome MigrationOutcome) ([]byte, error) {
	var dbPath any
	if outcome.DBPath != nil {
		dbPath = *outcome.DBPath
	}
	report, err := outcome.Report.ToJSON()
	if err != nil {
		return nil, err
	}
	reportValue, _ := core.ParseJSON(string(report))
	if reportValue == nil {
		reportValue = map[string]any{}
	}
	return core.MarshalJSON(map[string]any{
		"dryRun":   outcome.DryRun,
		"archived": outcome.Archived,
		"dbPath":   dbPath,
		"report":   reportValue,
	})
}
