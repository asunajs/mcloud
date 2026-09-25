package mcloudio

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 旧项目（mcloud-sign）数据导入：`asign.json` + `packages/api/data/*.json` → SQLite
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-io/src/legacy_import/。
//
// # 幂等靠什么成立
//
// 三道保险叠在一起：
//
//  1. **所有写入都是 UPSERT**，主键稳定（账号用 AccountID、定时任务/组合用自身 id、
//     导入的运行记录用 `legacy-<内容哈希>`）。于是"再跑一次"只是把同样的行再写一遍。
//  2. **按源文件 sha256 记台账**（`imports` 表）：源没变就整份跳过。
//     这一条省的是时间，不是正确性 —— 正确性由第 1 条保证。
//  3. **台账在所有写入之后才落**。中途失败时台账没写，下次重跑会从头补完，
//     而不是留下"一半数据 + 声称已导入"。
//
// # 为什么不做整包事务
//
// 看起来很该做，但这套写入是几十条互相独立的 UPSERT，
// 而**真正危险的一步（掩码冲突）在任何写入之前就检查完了**。
// 为"崩在中间时不用重跑一遍"去把 SqliteStore 的十来个写方法重构成事务版本，
// 收益远小于把已经测过的写入路径全改一遍的风险。
//
// # 绝不静默扩大目标范围
//
// 旧版有一处陷阱：`findAccount` 里纯数字字符串被当成序号，于是"用手机号当账号引用"
// 永远解析不到。这类引用**不能默认落到"全部账号"** ——
// 那是把"过去什么都没跑"变成"现在对所有人跑"，方向完全反了。所以：
// Schedule 有 `enabled`，一律**导入为停用**并报出问题；
// Pipeline 没有这个字段，只能**整条跳过**并报出问题。宁可少一条，不可错一条。
// ─────────────────────────────────────────────────────────────────────────────

// LEGACY_DATA_FILES 是旧数据目录里需要导入的文件（相对于 `packages/api/data/`）。
var LEGACY_DATA_FILES = []string{
	"state.json",
	"history.json",
	"pipelines.json",
	"schedules.json",
	"exchange.json",
	"webui.json",
}

// DefaultTZ 是旧版定时任务的时区（旧 `scheduler.ts` 的 `DEFAULT_TIMEZONE`）。
const DefaultTZ = "Asia/Shanghai"

// ─────────────────────────────────────────────────────────────────────────────
// 源位置
// ─────────────────────────────────────────────────────────────────────────────

// LegacySource 是旧项目的两个数据位置。
//
// 它们**不在一起**：账号配置在仓库根的 `asign.json`，运行数据在 `packages/api/data/`。
// 分开传，免得将来有人把 `asign.json` 也挪进 `data/` 之后这里静默读不到。
type LegacySource struct {
	// ConfigPath 是旧 `asign.json` 路径。
	ConfigPath string
	// DataDir 是旧 `packages/api/data/` 目录。
	DataDir string
}

// NewLegacySource 构造一个源。
func NewLegacySource(configPath, dataDir string) LegacySource {
	return LegacySource{ConfigPath: configPath, DataDir: dataDir}
}

// LegacySourceFromRepoRoot 按旧项目布局从仓库根推导两个位置。
func LegacySourceFromRepoRoot(root string) LegacySource {
	return LegacySource{
		ConfigPath: filepath.Join(root, "asign.json"),
		DataDir:    filepath.Join(root, "packages", "api", "data"),
	}
}

// LegacyFile 是一个旧数据文件（名字 + 实际路径）。
type LegacyFile struct {
	Name string
	Path string
}

// ExistingFiles 返回实际存在的源文件：`asign.json` + 6 个数据文件，不存在的跳过。
//
// 归档与自检都用它，免得"哪些文件算旧数据"这件事在两处各写一遍。
func (s LegacySource) ExistingFiles() []LegacyFile {
	candidates := make([]LegacyFile, 0, len(LEGACY_DATA_FILES)+1)
	candidates = append(candidates, LegacyFile{"asign.json", s.ConfigPath})
	for _, name := range LEGACY_DATA_FILES {
		candidates = append(candidates, LegacyFile{name, filepath.Join(s.DataDir, name)})
	}
	out := candidates[:0]
	for _, c := range candidates {
		if info, err := os.Stat(c.Path); err == nil && !info.IsDir() {
			out = append(out, c)
		}
	}
	return out
}

// ArchiveLegacySource 把旧数据**原样复制**到 dest，返回复制的文件数。
//
// 这不是"改坏了能回滚"意义上的备份：旧目录自始至终只读，一个字节都不改。
// 它的用途是让新项目自带一份原始数据 —— 旧仓库将来只作归档、乃至某天不再可访问时，
// 这次迁移仍然能被检查、被重跑。
//
// 已存在且内容相同的文件会被跳过，所以重复执行不会做无谓的写盘。
func ArchiveLegacySource(source LegacySource, dest string) (int, error) {
	if err := os.MkdirAll(dest, 0o755); err != nil {
		return 0, &LegacyImportError{Kind: LegacyImportIO, Path: dest, Err: err}
	}
	copied := 0
	for _, file := range source.ExistingFiles() {
		text, err := readText(file.Path)
		if err != nil {
			return copied, err
		}
		to := filepath.Join(dest, file.Name)
		if existing, err := os.ReadFile(to); err == nil && string(existing) == text {
			continue
		}
		if err := os.WriteFile(to, []byte(text), 0o644); err != nil {
			return copied, &LegacyImportError{Kind: LegacyImportIO, Path: to, Err: err}
		}
		copied++
	}
	return copied, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 入口
// ─────────────────────────────────────────────────────────────────────────────

// ImportLegacy 把旧项目数据导入 store。可重复执行（幂等说明见文件头）。
//
// `now` 显式传入而不是内部取 time.Now()：台账的 `imported_at` 因此可测，
// 也让"所有时间都从外部进来"这条纪律在 io 层同样成立。
//
// # scope 决定导入的账号归谁
//
// 账号从 v3 起有归属（`accounts.owner`），而**无主账号对谁都不可见**（见
// StateStore 的隔离契约）。于是：
//
//   - [core.ScopeUser]：导入的账号归那个用户 —— `user init` 之后由管理员执行的迁移；
//   - [core.ScopeLocal]：导入的账号**保持无主**（`owner IS NULL`），
//     等之后初始化管理员时被认领。
func ImportLegacy(
	ctx context.Context,
	store *SqliteStore,
	scope core.Scope,
	source LegacySource,
	now time.Time,
) (MigrationReport, error) {
	var report MigrationReport

	// ── 1. 账号清单：每次都重新读（后面的引用解析都要用），但只在源变了才写 ──
	configText, err := readText(source.ConfigPath)
	if err != nil {
		return report, err
	}
	config, perr := parseWhole[core.LegacyConfig](source.ConfigPath, configText)
	if perr != nil {
		return report, perr
	}
	views := core.DescribeLegacyAccounts(config.Caiyun)

	// ── 2. 掩码冲突检查：**必须在任何写入之前** ──
	// 两个手机号前 2 后 2 相同时会塌缩成一个 AccountID，历史数据会混在一起且不报错。
	// 这里直接失败，代价是"迁移没做"，而不是"迁移做错了"。
	identity, identityErr := core.BuildIdentityMap(views)
	if identityErr != nil {
		return report, &LegacyImportError{
			Kind: LegacyImportIdentity,
			Path: source.ConfigPath,
			Err:  identityErr,
		}
	}

	if changed, err := sourceChanged(ctx, store, source.ConfigPath, configText); err != nil {
		return report, err
	} else if changed {
		written, err := writeAccounts(ctx, store, scope, views)
		if err != nil {
			return report, err
		}
		report.Accounts = int(written)
		if err := commitLedger(ctx, store, source.ConfigPath, configText, written, &report, now); err != nil {
			return report, err
		}
	}

	// ── 3. 逐个数据文件 ──
	for _, name := range LEGACY_DATA_FILES {
		path := filepath.Join(source.DataDir, name)
		if info, err := os.Stat(path); err != nil || info.IsDir() {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind: IssueMissingFile,
				Path: path,
			})
			continue
		}
		text, err := readText(path)
		if err != nil {
			return report, err
		}
		changed, err := sourceChanged(ctx, store, path, text)
		if err != nil {
			return report, err
		}
		if !changed {
			report.SkippedUnchanged = append(report.SkippedUnchanged, name)
			continue
		}
		value, ok := core.ParseJSON(text)
		if !ok {
			return report, &LegacyImportError{
				Kind: LegacyImportParse,
				Path: path,
				Err:  fmt.Errorf("不是合法 JSON"),
			}
		}

		issuesBefore := len(report.Issues)
		var written int64
		switch name {
		case "state.json":
			written, err = writeState(ctx, store, scope, value, identity, &report)
		case "history.json":
			written, err = writeHistory(ctx, store, scope, value, views, &report)
		case "pipelines.json":
			written, err = writePipelines(ctx, store, scope, value, views, &report)
		case "schedules.json":
			written, err = writeSchedules(ctx, store, scope, value, views, &report)
		case "exchange.json":
			written, err = writeExchange(ctx, store, value, views, &report)
		case "webui.json":
			written, err = writeWebui(ctx, store, value, &report)
		default:
			// LEGACY_DATA_FILES 是常量表，走不到这里
			err = fmt.Errorf("LEGACY_DATA_FILES 里没有 %s", name)
		}
		if err != nil {
			return report, err
		}
		newIssues := int64(len(report.Issues) - issuesBefore)

		// 台账最后落：中途失败时台账没写，下次重跑会补完
		if err := store.RecordImport(ctx, &ImportRecord{
			Source:      path,
			SHA256:      core.SHA256Hex([]byte(text)),
			ImportedAt:  now.UTC().Format(time.RFC3339),
			RecordCount: written,
			Issues:      newIssues,
			Raw:         &text,
		}); err != nil {
			return report, &LegacyImportError{Kind: LegacyImportStore, Path: path, Err: err}
		}
	}

	// ── 4. 凭据回填：**必须排在所有文件之后**，且不受哈希门控 ──
	//
	// 两个"必须"都有理由：
	//   - 排在 `state.json` 之后：writeState 走的是 UpsertAccountState，
	//     那是**整体覆盖**（没给的列会被写成 NULL）。先回填再覆盖等于白填。
	//   - 不受门控：老库的 `state.json` 早就导过了，走门控就永远不会补上。
	filled, err := fillCredentials(ctx, store, scope, config.Caiyun, views)
	if err != nil {
		return report, err
	}
	report.CredentialsFilled = int(filled)

	return report, nil
}

// DiagnoseLegacy 只读预演：把导入跑在一个**内存库**上，跑完连库一起丢掉。
//
// 为什么不另写一套"只检查、不写入"的代码：那样诊断逻辑和真实导入会各自演化，
// 迟早出现"预演说没问题、真跑却报错"。这里让两者**走完全同一条路径**，
// 唯一的区别是最后那个库被丢掉 —— 于是漂移在结构上不可能发生。
func DiagnoseLegacy(ctx context.Context, source LegacySource, now time.Time) (MigrationReport, error) {
	scratch, err := OpenInMemory()
	if err != nil {
		return MigrationReport{}, &LegacyImportError{Kind: LegacyImportStore, Err: err}
	}
	defer func() { _ = scratch.Close() }()

	return ImportLegacy(ctx, scratch, core.ScopeLocal(), source, now)
}

// ─────────────────────────────────────────────────────────────────────────────
// 错误
// ─────────────────────────────────────────────────────────────────────────────

// LegacyImportErrorKind 是导入期错误的四类。
type LegacyImportErrorKind int

const (
	LegacyImportIO LegacyImportErrorKind = iota
	LegacyImportParse
	// LegacyImportIdentity 是**"什么都还没写"的那个失败**：
	// 掩码冲突会在任何写入之前中止。
	LegacyImportIdentity
	LegacyImportStore
)

// LegacyImportError 是导入期错误。
//
// ⚠ Rust 侧是 thiserror 的四个变体，各自带不同的字段（`Io` / `Parse` 带 path，
// `Identity` / `Store` 不带）。Go 里合成一个结构体 + Kind：
// 换来的是"调用方用同一个类型接住所有失败"，代价是 `Path` 对后两类是空串 ——
// 那两类本来就没有文件可指。
type LegacyImportError struct {
	Kind LegacyImportErrorKind
	// Path 是出错的文件路径；[LegacyImportIdentity] / [LegacyImportStore] 为空。
	Path string
	Err  error
}

// Error 实现 error 接口。
func (e *LegacyImportError) Error() string {
	switch e.Kind {
	case LegacyImportIO:
		return fmt.Sprintf("读取 %s 失败: %v", e.Path, e.Err)
	case LegacyImportParse:
		return fmt.Sprintf("解析 %s 失败: %v", e.Path, e.Err)
	case LegacyImportIdentity:
		return fmt.Sprintf("账号身份映射失败，已中止迁移且未写入任何数据：%v", e.Err)
	case LegacyImportStore:
		return fmt.Sprintf("数据库操作失败: %v", e.Err)
	}
	return e.Err.Error()
}

// Unwrap 让 errors.Is / errors.As 能穿透到根因。
func (e *LegacyImportError) Unwrap() error { return e.Err }

// ─────────────────────────────────────────────────────────────────────────────
// 报告
// ─────────────────────────────────────────────────────────────────────────────

// MigrationIssueKind 是迁移中发现的问题种类。
//
// ⚠ 文案**不**进这里 —— 同一种类的文案里嵌着具体的值（账号名、下标、原因）。
// 文案由 [MigrationIssue.String] 统一产出，与 Rust 侧 `thiserror` 的
// `#[error("…")]` 逐字对应（那是外部契约：CLI 直接打印它）。
type MigrationIssueKind int

const (
	IssueRecoveredAccountRef MigrationIssueKind = iota
	IssueDeadAccountRef
	IssueMissingTarget
	IssueEmptyCron
	IssueDroppedStepAccount
	IssueEmptySteps
	IssuePipelineSkipped
	IssueOrphanAccountState
	IssueEmptyAccountState
	IssueUnparsableAccountState
	IssueUnknownStateFields
	IssueUnparsableRecord
	IssueBadRunTimestamp
	IssueUnresolvedRunAccount
	IssueBadExchangeWindows
	IssueBadPrizeID
	IssueBindingWithoutAccount
	IssueMissingFile
)

// MigrationIssue 是迁移中发现的问题。
//
// 每一条都要能指向一个**具体可修的东西**，并且说明"我们替你做了什么决定"。
// 迁移最忌讳"猜一个合理的值然后不说" —— 那会让问题延后到某天记录对不上时才爆出来。
//
// ⚠ `Fields` 是切片 ⇒ 本结构体**不可**用 `==` 比较（编译错误，不是静默出错）。
// 测试里比 [MigrationIssue.String]。
type MigrationIssue struct {
	Kind    MigrationIssueKind
	At      string
	Digits  string
	Account string
	Raw     string
	Action  string
	Key     string
	Fields  []string
	File    string
	// Index 是**给人看的**序号，1 起（history / exchange 的条目下标）。
	Index  int
	Error  string
	Start  string
	Label  string
	Reason string
	Path   string
}

// String 产出对外文案，句式与 Rust 侧 `thiserror` 的 `#[error(...)]` 对应。
//
// ⚠ **不带 Markdown 星号**：这些串会出现在网页端的导入结果里（不渲染 Markdown），
// 也出现在终端输出里，星号只会原样显示成噪音（2026-09-25 真账号跑 `app-task` 那一轮
// 在播报里撞见同一形状，顺手把这一族也清了）。
func (i MigrationIssue) String() string {
	switch i.Kind {
	case IssueRecoveredAccountRef:
		return fmt.Sprintf("%s：账号引用「%s」在旧版被当成序号（越界）因而从未生效，"+
			"但按手机号能唯一匹配到 %s；已按手机号导入，请确认目标是否正确",
			i.At, i.Digits, i.Account)
	case IssueDeadAccountRef:
		return fmt.Sprintf("%s：账号引用「%s」在旧版就没有生效过，现在也匹配不到任何账号；已%s，请人工确认",
			i.At, i.Raw, i.Action)
	case IssueMissingTarget:
		return fmt.Sprintf("%s：缺少执行目标（`task` 与 `pipelineId` 都没有）；已停用，请指定目标后再启用", i.At)
	case IssueEmptyCron:
		return fmt.Sprintf("%s：cron 表达式为空；已停用", i.At)
	case IssueDroppedStepAccount:
		return fmt.Sprintf("%s：步骤级 `account` 已废弃（旧版读取时就会丢掉）；该字段未导入", i.At)
	case IssueEmptySteps:
		return fmt.Sprintf("%s：没有任何步骤；已跳过整条组合", i.At)
	case IssuePipelineSkipped:
		return fmt.Sprintf("%s：整条组合被跳过（账号引用无法解析，导入它会改变它的作用范围）", i.At)
	case IssueOrphanAccountState:
		return fmt.Sprintf("state.json：账号「%s」在账号配置里找不到；已跳过（不凭空造账号）", i.Key)
	case IssueEmptyAccountState:
		return fmt.Sprintf("state.json：账号「%s」的状态是空的；已跳过", i.Key)
	case IssueUnparsableAccountState:
		return fmt.Sprintf("state.json：账号「%s」的状态无法解析（%s）；已跳过该账号", i.Key, i.Error)
	case IssueUnknownStateFields:
		// Rust 侧是 `{fields:?}`（Vec<String> 的 Debug）⇒ `["a", "b"]`
		return fmt.Sprintf("state.json：账号「%s」出现过未识别字段 %q；这些字段未导入（原文已归档）",
			i.Key, i.Fields)
	case IssueUnparsableRecord:
		return fmt.Sprintf("%s 第 %d 条无法解析（%s）；已跳过该条，其余照常导入", i.File, i.Index, i.Error)
	case IssueBadRunTimestamp:
		return fmt.Sprintf("%s 第 %d 条的起止时间非法（start=%q）；已跳过该条", i.File, i.Index, i.Start)
	case IssueUnresolvedRunAccount:
		return fmt.Sprintf("history.json 第 %d 条的展示名「%s」%s；该条已导入但未归属账号",
			i.Index, i.Label, i.Reason)
	case IssueBadExchangeWindows:
		return fmt.Sprintf("exchange.json 的抢兑时刻非法（%s）；已回落到默认窗口", i.Error)
	case IssueBadPrizeID:
		return fmt.Sprintf("exchange.json 第 %d 条绑定的奖品 ID 非法（%q）；已跳过该绑定", i.Index, i.Raw)
	case IssueBindingWithoutAccount:
		return fmt.Sprintf("exchange.json 第 %d 条绑定没有指定账号；已跳过该绑定", i.Index)
	case IssueMissingFile:
		return fmt.Sprintf("%s 不存在；已跳过（该文件不是每台机器都有）", i.Path)
	}
	return ""
}

// MigrationReport 是导入结果。
type MigrationReport struct {
	// Accounts 是导入的账号数。
	Accounts int
	// AccountStates 是导入的账号登录态数。
	AccountStates int
	// CredentialsFilled 是**本次实际补上**整串 Authorization / 手机号的账号数。
	//
	// "实际"是刻意的：没有缺口可补的账号不计入。否则第二次运行还会报同样的数字，
	// 而用户正是靠这个数字判断"我那个老库补上了吗"。
	//
	// 与 AccountStates **不是一回事**，也不是它的子集：凭据来自 `asign.json`，
	// 登录态来自 `state.json`，两个文件的台账各自独立。
	CredentialsFilled int
	Schedules         int
	Pipelines         int
	// Runs 是导入的运行历史条数。
	Runs int
	// SkippedUnchanged 是因源文件未变而整份跳过的文件。
	SkippedUnchanged []string
	Issues           []MigrationIssue
}

// HasIssues 报告是否发现了问题。
func (r MigrationReport) HasIssues() bool { return len(r.Issues) != 0 }

// Summary 是一行式摘要（可安全进日志：不含任何凭据）。
func (r MigrationReport) Summary() string {
	return fmt.Sprintf(
		"账号 %d / 登录态 %d / 凭据回填 %d / 定时任务 %d / 组合 %d / 历史 %d 条；"+
			"跳过未变的文件 %d 个；问题 %d 个",
		r.Accounts, r.AccountStates, r.CredentialsFilled, r.Schedules, r.Pipelines,
		r.Runs, len(r.SkippedUnchanged), len(r.Issues))
}

// IssueLines 是问题清单（给 CLI 逐条打印）。
func (r MigrationReport) IssueLines() []string {
	out := make([]string, 0, len(r.Issues))
	for _, issue := range r.Issues {
		out = append(out, issue.String())
	}
	return out
}

// ToJSON 是机器可读形态（CLI `--json` 与服务端共用一套字段名）。
//
// 问题只以**字符串**输出：枚举的内部形状是实现细节，
// 让脚本去依赖它等于把重构成本转嫁给使用者。
func (r MigrationReport) ToJSON() ([]byte, error) {
	skipped := r.SkippedUnchanged
	if skipped == nil {
		skipped = []string{}
	}
	return core.MarshalJSON(map[string]any{
		"accounts":          r.Accounts,
		"accountStates":     r.AccountStates,
		"credentialsFilled": r.CredentialsFilled,
		"schedules":         r.Schedules,
		"pipelines":         r.Pipelines,
		"runs":              r.Runs,
		"skippedUnchanged":  skipped,
		"issues":            r.IssueLines(),
		"summary":           r.Summary(),
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

func readText(path string) (string, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return "", &LegacyImportError{Kind: LegacyImportIO, Path: path, Err: err}
	}
	return string(raw), nil
}

// parseWhole 整份解析；失败时**点名是哪个文件**。
//
// ⚠ 泛型而不是 `out any`：Go 的 `encoding/json` 往 `any` 里解只会得到
// `map[string]any`，而这里要的是具体类型 —— 用 `any` 会**静默成功**地解出一堆
// 什么都没有的零值，然后"迁移成功但一条没导进来"。
func parseWhole[T any](path, text string) (T, *LegacyImportError) {
	var zero T
	value, ok := core.ParseJSONTo[T](text)
	if !ok {
		return zero, &LegacyImportError{Kind: LegacyImportParse, Path: path, Err: fmt.Errorf("形状不合")}
	}
	return value, nil
}

// indexed 保留下标的元素。
type indexed[T any] struct {
	// Index 是 0 起的下标（对外报 `Index+1`）。
	Index int
	Value T
}

// parseEachIndexed 逐条解析数组元素，坏的那条**只丢自己**。
//
// 与 `core.ParseEach` 的区别：保留下标。
// 报告里写「第 3 条」却指不出是哪一条，等于没说 —— 所以这里宁可自己留下标。
// 下标对外是 **1 起**（给人看的），对内仍是 0 起。
func parseEachIndexed[T any](file string, items []any, report *MigrationReport) []indexed[T] {
	parsed := make([]indexed[T], 0, len(items))
	for index, item := range items {
		raw, err := core.MarshalJSON(item)
		if err != nil {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueUnparsableRecord,
				File:  file,
				Index: index + 1,
				Error: err.Error(),
			})
			continue
		}
		value, ok := core.ParseJSONTo[T](string(raw))
		if !ok {
			report.Issues = append(report.Issues, MigrationIssue{
				Kind:  IssueUnparsableRecord,
				File:  file,
				Index: index + 1,
				Error: "形状不合",
			})
			continue
		}
		parsed = append(parsed, indexed[T]{Index: index, Value: value})
	}
	return parsed
}

// sourceChanged 报告源内容与台账里的 sha256 是否不一致（或从没导入过）。
func sourceChanged(ctx context.Context, store *SqliteStore, path, text string) (bool, error) {
	digest := core.SHA256Hex([]byte(text))
	record, err := store.ImportRecordFor(ctx, path)
	if err != nil {
		return false, &LegacyImportError{Kind: LegacyImportStore, Path: path, Err: err}
	}
	if record == nil {
		return true, nil
	}
	return record.SHA256 != digest, nil
}

// commitLedger 落一条台账。
func commitLedger(
	ctx context.Context,
	store *SqliteStore,
	path, text string,
	records int64,
	report *MigrationReport,
	now time.Time,
) error {
	raw := text
	err := store.RecordImport(ctx, &ImportRecord{
		Source:      path,
		SHA256:      core.SHA256Hex([]byte(text)),
		ImportedAt:  now.UTC().Format(time.RFC3339),
		RecordCount: records,
		Issues:      int64(len(report.Issues)),
		Raw:         &raw,
	})
	if err != nil {
		return &LegacyImportError{Kind: LegacyImportStore, Path: path, Err: err}
	}
	return nil
}

// nonEmpty 去掉首尾空白；空串返回 nil（对齐 JS 的 falsy）。
func nonEmpty(value string) *string {
	trimmed := strings.TrimSpace(value)
	if trimmed == "" {
		return nil
	}
	return &trimmed
}

// fallbackID 是旧记录缺 id 时用的兜底 id。
//
// 用**名称的内容哈希**而不是下标：下标会在文件重排后变，
// 那就等于每次重排都换一批新记录，幂等立刻失效。
func fallbackID(kind, name string) string {
	return "legacy-" + kind + "-" + core.MD5Hex([]byte(name))[:12]
}
