package mcloudio

// 本文件是 [core.ConfigStore] 的 SQLite 实现：定时任务、任务组合、抢兑配置、
// 界面偏好、推送配置，外加一张"旧数据导入台账"。
//
// 与 `store.go`（`StateStore`）分开的理由见 trait 定义处的注释：
// 两者服务不同的调用方，合在一起会逼着"只想给能力层写个内存假实现"的人
// 去实现一堆用不到的方法。
//
// # JSON 字段的取舍
//
// `target` / `account_ids` / `body` / `steps` / `windows` / `bindings` / `channels`
// 一律存 TEXT（JSON 原文）。
// 它们都是"整块读写"的结构，拆成列只会让 schema 跟着能力参数的变化不停改版；
// 而真正需要被 SQL 查询/排序的维度（`enabled`、`cron`、`last_status`）已经单独成列。
//
// # 列表顺序
//
// `ORDER BY created_at ASC, rowid ASC`。第二排序键用 `rowid` 是刻意的：
// 旧文件里的顺序就是数组顺序，导入后 rowid 即那个顺序，
// 而 `ON CONFLICT DO UPDATE` 不改 rowid —— 于是**迁移前后顺序一致**，
// 不会出现"界面上的定时任务莫名重排了"这种没法解释的现象。

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// JSON 列编解码
// ─────────────────────────────────────────────────────────────────────────────

// toJSON 序列化为 JSON 文本（用于 TEXT 列）。
//
// 走 [core.MarshalJSON] 而不是 `json.Marshal`：后者会把 `<` `>` `&` 转义成
// `\u003c` 等，而 `serde_json` 不转 —— 于是"库里存着同一份数据、
// 字节却不一样"。这条在 `sanitize` 那一步已经踩过，判据见方案 §6.1。
func toJSON(value any) (string, error) {
	b, err := core.MarshalJSON(value)
	if err != nil {
		return "", core.NewInternal("JSON 编码失败: " + err.Error())
	}
	return string(b), nil
}

// fromJSON 从 TEXT 列还原成具体类型。
//
// ⚠⚠ **`null` 在这里要显式挡掉**：`json.Unmarshal` 把 `null` 解进任何类型都是
// **静默成功**（切片留 nil、指针留 nil、结构体留零值），而 serde 对
// `Vec<T>` / 结构体一律**拒**。不挡的话，一个被写成 `"null"` 的列
// 在 Go 侧表现成"这个配置是空的"，在 Rust 侧却是**报错** ——
// 两边对同一份数据给出不同结论，而谁都不会报出来。
//
// ⚠ 只挡**具体类型**这条路径。`body` 列走 [textToRaw]：那里 `"null"` 是
// **合法**的（对应 Rust 的 `Option<Value>` 里的 `Some(Value::Null)`，
// 即"显式传了一个 null 参数"），与"没给参数"（SQL NULL）是两回事。
func fromJSON[T any](text string) (T, error) {
	var v T
	if strings.TrimSpace(text) == "null" {
		return v, core.NewInternal("JSON 列是显式的 null（serde 会拒）: " + text)
	}
	if err := json.Unmarshal([]byte(text), &v); err != nil {
		return v, core.NewInternal("JSON 解码失败: " + err.Error())
	}
	return v, nil
}

// optRawToText 把可选的 JSON 原文写进 TEXT 列；nil 写 NULL。
//
// ⚠⚠ **必须显式转成 `string`**：`json.RawMessage` 的底层类型是 `[]byte`，
// 而 `database/sql` 对 `[]byte` 的默认处理是**当 BLOB 写**。
// SQLite 是动态类型的，同一个列里 TEXT 与 BLOB 可以共存，于是"看起来写进去了"，
// 而它读回来与 Rust 写的那些行不是同一种存储类 —— 只有 `typeof()`、
// 或按文本比较（`WHERE body = ?`）时才看得出来。这是本文件最容易犯的错。
func optRawToText(raw json.RawMessage) any {
	if raw == nil {
		return nil
	}
	return string(raw)
}

// textToRaw 读回 JSON 原文列（NULL → nil）。
func textToRaw(ns sql.NullString) json.RawMessage {
	if !ns.Valid {
		return nil
	}
	return json.RawMessage(ns.String)
}

func accountIDsToText(ids []core.AccountID) (string, error) {
	strs := make([]string, 0, len(ids))
	for _, id := range ids {
		strs = append(strs, id.String())
	}
	return toJSON(strs)
}

func accountIDsFromText(text string) ([]core.AccountID, error) {
	strs, err := fromJSON[[]string](text)
	if err != nil {
		return nil, err
	}
	ids := make([]core.AccountID, 0, len(strs))
	for _, s := range strs {
		ids = append(ids, core.NewAccountID(s))
	}
	return ids, nil
}

// userIDFromOpt 读回可选归属（NULL → nil，即"无主"）。
func userIDFromOpt(ns sql.NullString) *core.UserID {
	if !ns.Valid {
		return nil
	}
	id := core.NewUserID(ns.String)
	return &id
}

// accountModeFromStr 读回账号执行模式；未知值归 [core.ModeSequential]。
//
// ⚠ 这里**是**"归默认值"，与 [core.ParseScheduleStatus] 那边刻意不同 —— 不是笔误。
// Rust 侧注释写着"缺省串行是刻意偏保守：并行更容易触发风控"。
// 两者的差别在**错答的代价**：状态答错会让界面显示错误的结论（所以宁可"没有状态"），
// 而执行模式答错只是跑得慢一点，且方向是安全的那一侧。
func accountModeFromStr(raw string) core.AccountMode {
	if raw == "parallel" {
		return core.ModeParallel
	}
	return core.ModeSequential
}

// ─────────────────────────────────────────────────────────────────────────────
// 导入台账
// ─────────────────────────────────────────────────────────────────────────────

// ImportRecord 是一次旧数据导入的台账记录。
//
// 放在 io 层而不是 core 的领域模型里：它记录的是**磁盘上的源文件**（路径、哈希、原文），
// 而 core 的纯净性正建立在"不知道文件和路径存在"之上。
//
// ⚠ 字段刻意**不带 `json` tag**：Rust 侧 derive 的是 `Debug, Clone, PartialEq, Eq`，
// **不** Serialize。加 tag 会让人以为"这些字段名是对外契约"。
// 需要展示时用 [ImportRecord.Summary]，它不含 `Raw`。
type ImportRecord struct {
	// Source 是源文件绝对路径。
	Source string
	// SHA256 是源文件内容的 sha256 —— 幂等判断的依据。
	SHA256 string
	// ImportedAt 是导入时刻（RFC 3339 文本，与时间列同一个格式）。
	ImportedAt string
	// RecordCount / Issues 是本次导入的条数与问题数。
	RecordCount int64
	Issues      int64
	// Raw 是原始内容归档（便于将来映射规则变了之后重放）。
	//
	// ⚠ 它可能含凭据 —— **不要把它整段打进日志**。
	Raw *string
}

// Summary 是可安全进日志的摘要（不含 [ImportRecord.Raw]）。
func (r ImportRecord) Summary() string {
	n := 8
	if len(r.SHA256) < n {
		n = len(r.SHA256)
	}
	return fmt.Sprintf("%s (sha256=%s… 记录 %d 条 / 问题 %d 个)",
		r.Source, r.SHA256[:n], r.RecordCount, r.Issues)
}

const importColumns = "source, sha256, imported_at, record_count, issues, raw"

func scanImport(row rowScanner) (ImportRecord, error) {
	var (
		rec ImportRecord
		raw sql.NullString
	)
	if err := row.Scan(&rec.Source, &rec.SHA256, &rec.ImportedAt,
		&rec.RecordCount, &rec.Issues, &raw); err != nil {
		return ImportRecord{}, err
	}
	rec.Raw = ptrStr(raw)
	return rec, nil
}

// RecordImport 写入/覆盖一条导入台账（按 `source` 幂等）。
func (s *SqliteStore) RecordImport(ctx context.Context, record *ImportRecord) error {
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO imports (source, sha256, imported_at, record_count, issues, raw)
		VALUES (?, ?, ?, ?, ?, ?)
		ON CONFLICT(source) DO UPDATE SET
			sha256       = excluded.sha256,
			imported_at  = excluded.imported_at,
			record_count = excluded.record_count,
			issues       = excluded.issues,
			raw          = excluded.raw`,
		record.Source, record.SHA256, record.ImportedAt,
		record.RecordCount, record.Issues, optStr(record.Raw))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ImportRecordFor 读某个源文件的台账；没导入过返回 nil, nil。
func (s *SqliteStore) ImportRecordFor(ctx context.Context, source string) (*ImportRecord, error) {
	rec, err := scanImport(s.db.QueryRowContext(ctx,
		"SELECT "+importColumns+" FROM imports WHERE source = ?", source))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &rec, nil
}

// ListImports 全部台账，按源路径排序。
func (s *SqliteStore) ListImports(ctx context.Context) ([]ImportRecord, error) {
	rows, err := s.db.QueryContext(ctx,
		"SELECT "+importColumns+" FROM imports ORDER BY source ASC")
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []ImportRecord{}
	for rows.Next() {
		rec, err := scanImport(rows)
		if err != nil {
			return nil, mapSQLite(err)
		}
		out = append(out, rec)
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 列清单与归属过滤
// ─────────────────────────────────────────────────────────────────────────────

// scheduleColumns / pipelineColumns 是列顺序 —— **只此一处**。
//
// 与 `store.go` 的 `accountColumns` 同一个理由：列清单只能有一处，
// 否则某条查询会静默少一列而不报错。`owner` 固定放最后。
const (
	scheduleColumns = "id, name, target, cron, timezone, account_ids, body, enabled, " +
		"created_at, last_run_at, last_status, last_error, miss_policy, last_fire_at, owner"
	pipelineColumns = "id, name, description, steps, account_ids, account_mode, " +
		"stop_on_error, created_at, updated_at, last_run_at, last_status, last_error, " +
		"last_duration_ms, owner"
)

// ownerVisibleByID / ownerVisibleOnly 是归属过滤表达式。
//
// `? IS NULL` 即 [core.ScopeLocal]（不过滤），有值时退化成等值比较；
// 未归属（`owner IS NULL`）的行对**所有人**都不匹配
// （`owner = ?` 在 owner 为 NULL 时求值成 NULL，即 falsy）。
//
// # ⚠⚠ 两个常量在这里是**同一个字符串**，但调用处的传参个数不同
//
// Rust 侧用**编号占位符**，所以同一个值可以复用：
//
//	OWNER_VISIBLE_BY_ID = "(?2 IS NULL OR owner = ?2)"   // ?1 是 id
//	OWNER_VISIBLE_ONLY  = "(?1 IS NULL OR owner = ?1)"
//
// Go 的 `database/sql` 是**顺序绑定**，同一个 `?` 出现几次就要传几个值
// —— 所以单条查询传 3 个（id + 归属 ×2），列表查询传 **2 个**（归属 ×2）。
//
// ⚠ 这不是笔误，也不是"多传一个无所谓"：少传一个的表现是
// `missing argument with index 2`（运行期），而 SQL 文本**看起来完全正确**。
// 逐字照抄 Rust 的 SQL 就会踩，且只有跑到那条查询才发现。
//
// Rust 侧仍写两个常量的理由值得照抄：
// "编号错了不会编译报错，只会在运行期报 Wrong number of parameters，
// 而这类错误最容易在'复制一条查询改改'时产生。"
const (
	ownerVisibleByID = "(? IS NULL OR owner = ?)"
	ownerVisibleOnly = "(? IS NULL OR owner = ?)"
)

// ensureNotOwnedByOther 是配置行写入前的归属校验（`sqlText` 只传本文件的常量）。
//
// 与 `store.go` 账号写入的第二道校验同构：id 是主键，upsert 会顺手改掉别人的那一行，
// 所以"已被别人创建"必须显式挡住，而不是让 `ON CONFLICT` 静默顶掉。
//
// ⚠ 三态要分清：**没有这行**（放行）、**有行但 owner 是 NULL**（无主，放行）、
// **有行且 owner 是别人**（拒绝）。把前两者合并会让"无主 → 本人"的认领被误拒。
func ensureNotOwnedByOther(
	ctx context.Context, db *sql.DB, scope core.Scope, sqlText, id, label string,
) error {
	var owner sql.NullString
	err := db.QueryRowContext(ctx, sqlText, id).Scan(&owner)
	if errors.Is(err, sql.ErrNoRows) {
		return nil
	}
	if err != nil {
		return mapSQLite(err)
	}
	if !owner.Valid {
		return nil
	}
	if me := scope.Owner(); me != nil && owner.String != me.String() {
		// 给可行动的提示：表现成"不存在"会变成查不出原因的怪象（同 store.go 的口径）
		return core.NewBadRequest("该" + label + "已被其他用户创建")
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// schedules
// ─────────────────────────────────────────────────────────────────────────────

func scanSchedule(row rowScanner) (core.Schedule, error) {
	var (
		id, name, cron, timezone, accountIDsText string
		targetText, bodyText                     sql.NullString
		enabled                                  int64
		createdAt, lastRunAt, lastFireAt         sql.NullString
		lastStatus, lastError                    sql.NullString
		missPolicyText                           sql.NullString
		owner                                    sql.NullString
	)
	if err := row.Scan(&id, &name, &targetText, &cron, &timezone, &accountIDsText,
		&bodyText, &enabled, &createdAt, &lastRunAt, &lastStatus, &lastError,
		&missPolicyText, &lastFireAt, &owner); err != nil {
		return core.Schedule{}, err
	}

	var target *core.ScheduleTarget
	if targetText.Valid {
		t, err := fromJSON[core.ScheduleTarget](targetText.String)
		if err != nil {
			return core.Schedule{}, err
		}
		target = &t
	}
	ids, err := accountIDsFromText(accountIDsText)
	if err != nil {
		return core.Schedule{}, err
	}
	// ⚠ 未知状态给 nil（"没有状态"），**不归默认值** —— 见 [core.ParseScheduleStatus]。
	var last *core.ScheduleStatus
	if lastStatus.Valid {
		if v, ok := core.ParseScheduleStatus(lastStatus.String); ok {
			last = &v
		}
	}
	// 同一条纪律（见 [core.ParseMissPolicy]）：认不出的文本给 nil，
	// 也就是"跟随能力默认"，而不是硬塞 `catch_up` —— 后者会让一行损坏的数据
	// 在每次重启时凭空多跑一趟真上游。
	var miss *core.MissPolicy
	if missPolicyText.Valid {
		if v, ok := core.ParseMissPolicy(missPolicyText.String); ok {
			miss = &v
		}
	}

	return core.Schedule{
		ID:         id,
		Owner:      userIDFromOpt(owner),
		Name:       name,
		Target:     target,
		Cron:       cron,
		Timezone:   timezone,
		AccountIDs: ids,
		Body:       textToRaw(bodyText),
		Enabled:    enabled != 0,
		MissPolicy: miss,
		LastFireAt: ptrTime(lastFireAt),
		CreatedAt:  ptrTime(createdAt),
		LastRunAt:  ptrTime(lastRunAt),
		LastStatus: last,
		LastError:  ptrStr(lastError),
	}, nil
}

// UpsertSchedule 新建或覆盖一个定时任务。
func (s *SqliteStore) UpsertSchedule(ctx context.Context, scope core.Scope, schedule *core.Schedule) error {
	// ① 用户范围下不能替别人建任务，也不能把任务写成"无主"
	//（无主只由 schema 迁移与本地运维产生）—— 与账号口径一致。
	if me := scope.Owner(); me != nil {
		if schedule.Owner == nil || *schedule.Owner != *me {
			return core.NewNotFound("定时任务 " + schedule.ID)
		}
	}

	var target any
	if schedule.Target != nil {
		text, err := toJSON(*schedule.Target)
		if err != nil {
			return err
		}
		target = text
	}
	accountIDs, err := accountIDsToText(schedule.AccountIDs)
	if err != nil {
		return err
	}
	var lastStatus any
	if schedule.LastStatus != nil {
		lastStatus = schedule.LastStatus.String()
	}
	// ⚠ 同一条纪律（见 [core.ParseScheduleStatus] 那条注释）：能写进库的文本必须
	// 是这一族自己认得出的那几个。这里不静默写 NULL —— 未知策略写进去，
	// 读回来就成了"没人选过"，那是把脏数据洗成合法。
	var missPolicy any
	if schedule.MissPolicy != nil {
		text := schedule.MissPolicy.String()
		if _, ok := core.ParseMissPolicy(text); !ok {
			return core.NewBadRequest(
				"定时任务 " + schedule.ID + " 的补跑策略是未知值，不落库")
		}
		missPolicy = text
	}

	if err := ensureNotOwnedByOther(ctx, s.db, scope,
		"SELECT owner FROM schedules WHERE id = ?", schedule.ID, "定时任务"); err != nil {
		return err
	}

	// ⚠ 语句里**不写注释块**：这是一条反引号包起来的原始串，注释里出现一个
	// 反引号就会把字符串就地截断（`last_fire_at` 那种写法尤其容易顺手写出）。
	//
	// ⚠ **`last_fire_at` 刻意不在 DO UPDATE 那一串里**：它是调度循环的水位，
	// 只有 [MarkScheduleFire] 能动。编辑一行定时任务（改 cron、改名、换账号）
	// 不该把"哪些触发点已经处理过"倒回去 —— 前端那次 PUT 带回来的是它**早先读到**
	// 的那一份，跟着覆盖就会让已处理的那一次再跑一遍，而那一遍是带真上游请求的
	// （签到与换凭据都是不可逆动作）。
	_, err = s.db.ExecContext(ctx, `
		INSERT INTO schedules (
			id, name, target, cron, timezone, account_ids, body, enabled,
			created_at, last_run_at, last_status, last_error, owner,
			miss_policy, last_fire_at
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			name        = excluded.name,
			target      = excluded.target,
			cron        = excluded.cron,
			timezone    = excluded.timezone,
			account_ids = excluded.account_ids,
			body        = excluded.body,
			enabled     = excluded.enabled,
			last_run_at = excluded.last_run_at,
			last_status = excluded.last_status,
			last_error  = excluded.last_error,
			miss_policy = excluded.miss_policy,
			owner       = excluded.owner`,
		// 归属可以在这条语句里被"认领"（无主 → 本人），但不能被换成别人：
		// 换成别人的情况已由上面两道校验挡住。
		schedule.ID, schedule.Name, target, schedule.Cron, schedule.Timezone,
		accountIDs, optRawToText(schedule.Body), boolToInt(schedule.Enabled),
		optTime(schedule.CreatedAt), optTime(schedule.LastRunAt),
		lastStatus, optStr(schedule.LastError), userIDArg(schedule.Owner),
		missPolicy, optTime(schedule.LastFireAt))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// GetSchedule 取定时任务。不在该范围内时返回 nil, nil。
func (s *SqliteStore) GetSchedule(ctx context.Context, scope core.Scope, id string) (*core.Schedule, error) {
	sch, err := scanSchedule(s.db.QueryRowContext(ctx,
		"SELECT "+scheduleColumns+" FROM schedules WHERE id = ? AND "+ownerVisibleByID,
		id, ownerArg(scope), ownerArg(scope)))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &sch, nil
}

// ListSchedules 该范围内可见的定时任务，按 `created_at, rowid` 升序。
//
// ⚠ 归属值要传**两次** —— `ownerVisibleOnly` 里有两个 `?`。见该常量的注释。
func (s *SqliteStore) ListSchedules(ctx context.Context, scope core.Scope) ([]core.Schedule, error) {
	rows, err := s.db.QueryContext(ctx,
		"SELECT "+scheduleColumns+" FROM schedules WHERE "+ownerVisibleOnly+
			" ORDER BY created_at ASC, rowid ASC", ownerArg(scope), ownerArg(scope))
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.Schedule{}
	for rows.Next() {
		sch, err := scanSchedule(rows)
		if err != nil {
			return nil, mapSQLite(err)
		}
		out = append(out, sch)
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// MarkScheduleFire 窄写调度水位：**只碰** `last_fire_at` 一列。
//
// # 为什么不复用 UpsertSchedule
//
// 循环在触发的那一刻要把水位抬上去（不等任务跑完 —— 一趟签到要几十秒，
// 等完再抬的话这段时间里每个 tick 都会再捡一次同一个触发点）。
// 而那一刻它手上只有"我读到的那一份 schedule"，可能是几秒前读的：
// 用整行覆盖会把别人（WebUI 上正在编辑的人）刚写进去的 cron / body 打回旧值。
// 窄写让"改定义"与"推进水位"两件事互不打扰，与 `RotateAccountAuth` 同一条理由。
//
// 返回的 `bool` 是"有没有真的写到这一行"：不可见（别人的）或已被删 ⇒ false，
// 调用方要据此决定"这一趟到底跑不跑"—— 抬不动水位就不要打上游，
// 否则会出现"库里说没跑过、上游已经被真打了一枪"。
func (s *SqliteStore) MarkScheduleFire(
	ctx context.Context, scope core.Scope, id string, fireAt time.Time,
) (bool, error) {
	res, err := s.db.ExecContext(ctx,
		"UPDATE schedules SET last_fire_at = ? WHERE id = ? AND "+ownerVisibleByID,
		optTime(&fireAt), id, ownerArg(scope), ownerArg(scope))
	if err != nil {
		return false, mapSQLite(err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, mapSQLite(err)
	}
	return n > 0, nil
}

// RecordScheduleRun 窄写执行记账：只碰 `last_run_at` / `last_status` /
// `last_error` 三列，且**只碰给了值的那些**。
//
// 这三列自 v2 落下起没有任何写入方（`PUT /api/schedules` 只原样搬运），
// 本方法是第一个。判据与 [MarkScheduleFire] 同一条：跑完一趟之后要写的
// 只有"上一次跑得怎么样"，定义那一半不许动。
//
// # 两个 nil 的含义**刻意不同**
//
//   - `ranAt == nil` ⇒ **那一列不动**。它说的是"跑过"这件事，而按
//     [core.MissSkip] 弃掉的错过、以及撞锁被挡下的那一趟**都没跑**：
//     把上一次真跑的时刻抹掉，界面上就变成"这个任务从来不执行"。
//     没有哪一刻该被写进这一列时，宁可不写。
//   - `lastError == nil` ⇒ **写 NULL**。清空它本身就是结论（"这次跑干净了，
//     上次那条错误别再显示"）。留着的话，`last_status = ok` 配一句旧错误
//     在界面上并排出现，那正是查不出原因的形态。
//
// ⚠ 别把两条凑成对称的"nil 一律不动"：那会让一次成功的执行清不掉错误，
// 而"不动"那一半是这里唯一会丢信息的写法。
func (s *SqliteStore) RecordScheduleRun(
	ctx context.Context, scope core.Scope, id string,
	ranAt *time.Time, status core.ScheduleStatus, lastError *string,
) (bool, error) {
	// 与 [UpsertSchedule] 里 `miss_policy` 那一道同纪律：能进库的文本必须是
	// 这一族自己认得出的。未知值写成 NULL 等于把脏数据洗成"没有状态"。
	text := status.String()
	if _, ok := core.ParseScheduleStatus(text); !ok {
		return false, core.NewBadRequest(
			"定时任务 " + id + " 的执行状态是未知值，不落库")
	}

	set := "last_status = ?, last_error = ?"
	args := []any{text, optStr(lastError)}
	if ranAt != nil {
		set += ", last_run_at = ?"
		args = append(args, optTime(ranAt))
	}
	args = append(args, id, ownerArg(scope), ownerArg(scope))

	res, err := s.db.ExecContext(ctx,
		"UPDATE schedules SET "+set+" WHERE id = ? AND "+ownerVisibleByID, args...)
	if err != nil {
		return false, mapSQLite(err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, mapSQLite(err)
	}
	return n > 0, nil
}

// DeleteSchedule 删除；返回是否真的删掉了一行（不可见等价于不存在）。
func (s *SqliteStore) DeleteSchedule(ctx context.Context, scope core.Scope, id string) (bool, error) {
	res, err := s.db.ExecContext(ctx,
		"DELETE FROM schedules WHERE id = ? AND "+ownerVisibleByID,
		id, ownerArg(scope), ownerArg(scope))
	if err != nil {
		return false, mapSQLite(err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, mapSQLite(err)
	}
	return n > 0, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// pipelines
// ─────────────────────────────────────────────────────────────────────────────

func scanPipeline(row rowScanner) (core.Pipeline, error) {
	var (
		id, name, accountIDsText, accountMode, stepsText string
		description                                      sql.NullString
		stopOnError                                      int64
		createdAt, updatedAt, lastRunAt                  sql.NullString
		lastStatus, lastError                            sql.NullString
		lastDurationMS                                   sql.NullInt64
		owner                                            sql.NullString
	)
	if err := row.Scan(&id, &name, &description, &stepsText, &accountIDsText, &accountMode,
		&stopOnError, &createdAt, &updatedAt, &lastRunAt, &lastStatus, &lastError,
		&lastDurationMS, &owner); err != nil {
		return core.Pipeline{}, err
	}

	steps, err := fromJSON[[]core.PipelineStep](stepsText)
	if err != nil {
		return core.Pipeline{}, err
	}
	if steps == nil {
		steps = []core.PipelineStep{}
	}
	ids, err := accountIDsFromText(accountIDsText)
	if err != nil {
		return core.Pipeline{}, err
	}
	var last *core.PipelineStatus
	if lastStatus.Valid {
		if v, ok := core.ParsePipelineStatus(lastStatus.String); ok {
			last = &v
		}
	}

	return core.Pipeline{
		ID:             id,
		Owner:          userIDFromOpt(owner),
		Name:           name,
		Description:    ptrStr(description),
		Steps:          steps,
		AccountIDs:     ids,
		AccountMode:    accountModeFromStr(accountMode),
		StopOnError:    stopOnError != 0,
		CreatedAt:      ptrTime(createdAt),
		UpdatedAt:      ptrTime(updatedAt),
		LastRunAt:      ptrTime(lastRunAt),
		LastStatus:     last,
		LastError:      ptrStr(lastError),
		LastDurationMS: ptrI64(lastDurationMS),
	}, nil
}

// UpsertPipeline 新建或覆盖一个任务组合。
func (s *SqliteStore) UpsertPipeline(ctx context.Context, scope core.Scope, pipeline *core.Pipeline) error {
	// 两道校验与 UpsertSchedule 完全同构，理由见那里的注释
	if me := scope.Owner(); me != nil {
		if pipeline.Owner == nil || *pipeline.Owner != *me {
			return core.NewNotFound("组合 " + pipeline.ID)
		}
	}

	steps, err := toJSON(pipeline.Steps)
	if err != nil {
		return err
	}
	accountIDs, err := accountIDsToText(pipeline.AccountIDs)
	if err != nil {
		return err
	}
	var lastStatus any
	if pipeline.LastStatus != nil {
		lastStatus = pipeline.LastStatus.String()
	}

	if err := ensureNotOwnedByOther(ctx, s.db, scope,
		"SELECT owner FROM pipelines WHERE id = ?", pipeline.ID, "组合"); err != nil {
		return err
	}

	_, err = s.db.ExecContext(ctx, `
		INSERT INTO pipelines (
			id, name, description, steps, account_ids, account_mode, stop_on_error,
			created_at, updated_at, last_run_at, last_status, last_error, last_duration_ms,
			owner
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			name             = excluded.name,
			description      = excluded.description,
			steps            = excluded.steps,
			account_ids      = excluded.account_ids,
			account_mode     = excluded.account_mode,
			stop_on_error    = excluded.stop_on_error,
			updated_at       = excluded.updated_at,
			last_run_at      = excluded.last_run_at,
			last_status      = excluded.last_status,
			last_error       = excluded.last_error,
			last_duration_ms = excluded.last_duration_ms,
			owner            = excluded.owner`,
		pipeline.ID, pipeline.Name, optStr(pipeline.Description), steps, accountIDs,
		pipeline.AccountMode.String(), boolToInt(pipeline.StopOnError),
		optTime(pipeline.CreatedAt), optTime(pipeline.UpdatedAt), optTime(pipeline.LastRunAt),
		lastStatus, optStr(pipeline.LastError), optI64(pipeline.LastDurationMS),
		userIDArg(pipeline.Owner))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// GetPipeline 取组合。不在该范围内时返回 nil, nil。
func (s *SqliteStore) GetPipeline(ctx context.Context, scope core.Scope, id string) (*core.Pipeline, error) {
	p, err := scanPipeline(s.db.QueryRowContext(ctx,
		"SELECT "+pipelineColumns+" FROM pipelines WHERE id = ? AND "+ownerVisibleByID,
		id, ownerArg(scope), ownerArg(scope)))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &p, nil
}

// ListPipelines 该范围内可见的组合，按 `created_at, rowid` 升序。
//
// ⚠ 归属值要传**两次** —— 同 [SqliteStore.ListSchedules]。
func (s *SqliteStore) ListPipelines(ctx context.Context, scope core.Scope) ([]core.Pipeline, error) {
	rows, err := s.db.QueryContext(ctx,
		"SELECT "+pipelineColumns+" FROM pipelines WHERE "+ownerVisibleOnly+
			" ORDER BY created_at ASC, rowid ASC", ownerArg(scope), ownerArg(scope))
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.Pipeline{}
	for rows.Next() {
		p, err := scanPipeline(rows)
		if err != nil {
			return nil, mapSQLite(err)
		}
		out = append(out, p)
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 抢兑配置（单行表，**无归属**）
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeConfig 取抢兑配置；没有这一行时返回**默认值**（且不写盘）。
//
// 旧版语义就是"文件不存在返回默认值、且不写盘"，所以这里不制造副作用 ——
// 一个读操作顺手写库会让"只读地看一眼配置"变成一次写入。
func (s *SqliteStore) ExchangeConfig(ctx context.Context) (core.ExchangeConfig, error) {
	var (
		enabled                int64
		windows, bindings, ids string
		updatedAt              sql.NullString
	)
	err := s.db.QueryRowContext(ctx,
		"SELECT enabled, windows, bindings, schedule_ids, updated_at "+
			"FROM exchange_config WHERE id = 1").
		Scan(&enabled, &windows, &bindings, &ids, &updatedAt)
	if errors.Is(err, sql.ErrNoRows) {
		return core.NewExchangeConfig(), nil
	}
	if err != nil {
		return core.ExchangeConfig{}, mapSQLite(err)
	}

	w, err := fromJSON[[]string](windows)
	if err != nil {
		return core.ExchangeConfig{}, err
	}
	b, err := fromJSON[[]core.ExchangeBinding](bindings)
	if err != nil {
		return core.ExchangeConfig{}, err
	}
	sids, err := fromJSON[[]string](ids)
	if err != nil {
		return core.ExchangeConfig{}, err
	}
	// 空数组归一成 `[]`（Rust 的 `Vec` 反序列化出来就是空的，不是 nil）
	if w == nil {
		w = []string{}
	}
	if b == nil {
		b = []core.ExchangeBinding{}
	}
	if sids == nil {
		sids = []string{}
	}

	return core.ExchangeConfig{
		Enabled:     enabled != 0,
		Windows:     w,
		Bindings:    b,
		ScheduleIDs: sids,
		UpdatedAt:   ptrTime(updatedAt),
	}, nil
}

// PutExchangeConfig 覆盖抢兑配置（单行表，`id` 恒为 1）。
func (s *SqliteStore) PutExchangeConfig(ctx context.Context, config *core.ExchangeConfig) error {
	windows, err := toJSON(config.Windows)
	if err != nil {
		return err
	}
	bindings, err := toJSON(config.Bindings)
	if err != nil {
		return err
	}
	ids, err := toJSON(config.ScheduleIDs)
	if err != nil {
		return err
	}
	_, err = s.db.ExecContext(ctx, `
		INSERT INTO exchange_config (id, enabled, windows, bindings, schedule_ids, updated_at)
		VALUES (1, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			enabled      = excluded.enabled,
			windows      = excluded.windows,
			bindings     = excluded.bindings,
			schedule_ids = excluded.schedule_ids,
			updated_at   = excluded.updated_at`,
		boolToInt(config.Enabled), windows, bindings, ids, optTime(config.UpdatedAt))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 界面偏好（单行表）
// ─────────────────────────────────────────────────────────────────────────────

// WebuiConfig 取界面偏好；没有这一行时返回默认值（且不写盘）。
func (s *SqliteStore) WebuiConfig(ctx context.Context) (core.WebuiConfig, error) {
	var (
		remember, logExpanded, background int64
		lastPage                          string
		updatedAt                         sql.NullString
	)
	err := s.db.QueryRowContext(ctx,
		"SELECT remember_last_page, last_page, log_expanded, background_run, updated_at "+
			"FROM webui_config WHERE id = 1").
		Scan(&remember, &lastPage, &logExpanded, &background, &updatedAt)
	if errors.Is(err, sql.ErrNoRows) {
		return core.NewWebuiConfig(), nil
	}
	if err != nil {
		return core.WebuiConfig{}, mapSQLite(err)
	}

	cfg := core.WebuiConfig{
		RememberLastPage: remember != 0,
		LastPage:         lastPage,
		LogExpanded:      logExpanded != 0,
		BackgroundRun:    background != 0,
		UpdatedAt:        ptrTime(updatedAt),
	}
	// 白名单约束在读写两侧都收一遍：库里可能是手改过的
	cfg.NormalizePage()
	return cfg, nil
}

// PutWebuiConfig 覆盖界面偏好（单行表，`id` 恒为 1）。
func (s *SqliteStore) PutWebuiConfig(ctx context.Context, config *core.WebuiConfig) error {
	cfg := *config
	cfg.NormalizePage()
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO webui_config (
			id, remember_last_page, last_page, log_expanded, background_run, updated_at
		) VALUES (1, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			remember_last_page = excluded.remember_last_page,
			last_page          = excluded.last_page,
			log_expanded       = excluded.log_expanded,
			background_run     = excluded.background_run,
			updated_at         = excluded.updated_at`,
		boolToInt(cfg.RememberLastPage), cfg.LastPage, boolToInt(cfg.LogExpanded),
		boolToInt(cfg.BackgroundRun), optTime(cfg.UpdatedAt))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 推送配置（单行表，**无归属**）
// ─────────────────────────────────────────────────────────────────────────────

// NotifyConfig 取推送配置；没有这一行时返回默认值（关闭 + info + 一家没配），且不写盘。
//
// 口径与 [SqliteStore.ExchangeConfig] / [SqliteStore.WebuiConfig] 完全一致：
// 读操作不制造副作用，"这一族还没人配过"由默认值表达。
//
// # 三格标量为什么单独成列、渠道为什么整块存 JSON
//
// 见 schema.go 的 v10 那段。这里补一条读取侧的理由：`channels` 那一列里是**各家密钥**，
// 而"这次发不发、门槛多高"三格是不含凭据就能回答的问题 ——
// 分成列之后，将来要给日志或界面加一句"推送是开着的、门槛 info"，
// 不必把整块凭据读进内存再丢掉。
//
// # 门槛文本按原样读回（包括空串）
//
// 空串是**有效信息**（"没填 ⇒ 跟随 info"，见 [core.NotifyConfig.Validate]），
// 读取时替它填成 `info` 会让"我保存的到底是什么"这件事对不上 PUT 进去的那一份。
func (s *SqliteStore) NotifyConfig(ctx context.Context) (core.NotifyConfig, error) {
	var (
		enabled, onlyError int64
		minLevel, channels string
		title              sql.NullString
		updatedAt          sql.NullString
	)
	err := s.db.QueryRowContext(ctx,
		"SELECT enabled, title, only_error, min_level, channels, updated_at "+
			"FROM notify_config WHERE id = 1").
		Scan(&enabled, &title, &onlyError, &minLevel, &channels, &updatedAt)
	if errors.Is(err, sql.ErrNoRows) {
		return core.DefaultNotifyConfig(), nil
	}
	if err != nil {
		return core.NotifyConfig{}, mapSQLite(err)
	}
	parsed, err := fromJSON[core.NotifyChannels](channels)
	if err != nil {
		return core.NotifyConfig{}, err
	}
	return core.NotifyConfig{
		Enabled:   enabled != 0,
		Title:     ptrStr(title),
		OnlyError: onlyError != 0,
		MinLevel:  core.NotifyLogLevel(minLevel),
		Channels:  parsed,
		UpdatedAt: ptrTime(updatedAt),
	}, nil
}

// PutNotifyConfig 覆盖推送配置（单行表，`id` 恒为 1）。
//
// 这里**不**跑 [core.NotifyConfig.Validate]：保存前那句"哪一家缺哪个字段"由
// server 层说（它要面向用户），存储层只负责"整块写进去、原样读回来"。
func (s *SqliteStore) PutNotifyConfig(ctx context.Context, config *core.NotifyConfig) error {
	channels, err := toJSON(config.Channels)
	if err != nil {
		return err
	}
	_, err = s.db.ExecContext(ctx, `
		INSERT INTO notify_config (
			id, enabled, title, only_error, min_level, channels, updated_at
		) VALUES (1, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			enabled    = excluded.enabled,
			title      = excluded.title,
			only_error = excluded.only_error,
			min_level  = excluded.min_level,
			channels   = excluded.channels,
			updated_at = excluded.updated_at`,
		boolToInt(config.Enabled), optStr(config.Title), boolToInt(config.OnlyError),
		string(config.MinLevel), channels, optTime(config.UpdatedAt))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// 编译期断言：少一个方法时是**这里**报错，而不是等到组装根里某处赋值失败才发现。
var _ core.ConfigStore = (*SqliteStore)(nil)
