package mcloudio

import (
	"context"
	"database/sql"
	"strconv"
	"strings"
	"testing"

	_ "modernc.org/sqlite" // 纯 Go 驱动：注册 "sqlite"
)

// openRawDB 打开一个内存库，只用于 schema 层测试。
//
// ⚠ 三个连接参数都是**必需的**，不是调优：
//   - `MaxOpenConns(1)`：与 Rust 侧 `Mutex<Connection>` 的顺序化语义一致；
//   - `MaxIdleConns(1)` + `ConnMaxLifetime(0)`：`:memory:` 库**随连接消失**，
//     空闲连接一旦被回收，库就没了，后面所有断言会变成"表不存在"这种假红。
func openRawDB(t *testing.T) *sql.DB {
	t.Helper()
	db, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		t.Fatalf("打开内存库失败: %v", err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	db.SetConnMaxLifetime(0)
	t.Cleanup(func() { _ = db.Close() })
	return db
}

func mustExec(t *testing.T, db *sql.DB, sqlText string) {
	t.Helper()
	if _, err := db.Exec(sqlText); err != nil {
		t.Fatalf("执行 SQL 失败: %v\nSQL: %s", err, sqlText)
	}
}

func userVersion(t *testing.T, db *sql.DB) int64 {
	t.Helper()
	var v int64
	if err := db.QueryRow("PRAGMA user_version").Scan(&v); err != nil {
		t.Fatalf("读 user_version 失败: %v", err)
	}
	return v
}

func tableColumns(t *testing.T, db *sql.DB, table string) []string {
	t.Helper()
	rows, err := db.Query("PRAGMA table_info(" + table + ")")
	if err != nil {
		t.Fatalf("读 %s 列失败: %v", table, err)
	}
	defer func() { _ = rows.Close() }()
	var out []string
	for rows.Next() {
		var (
			cid, notNull, pk int
			name, typ        string
			dflt             sql.NullString
		)
		if err := rows.Scan(&cid, &name, &typ, &notNull, &dflt, &pk); err != nil {
			t.Fatalf("扫描 %s 列失败: %v", table, err)
		}
		out = append(out, name)
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("遍历 %s 列失败: %v", table, err)
	}
	return out
}

func hasColumn(cols []string, want string) bool {
	for _, c := range cols {
		if c == want {
			return true
		}
	}
	return false
}

// ── 迁移链本身的形状 ────────────────────────────────────────────────────────

func TestMigrationsAreSequential(t *testing.T) {
	if int64(len(allMigrations)) != SchemaVersion {
		t.Fatalf("迁移数量必须与 SchemaVersion 一致，否则版本号会撒谎：len=%d version=%d",
			len(allMigrations), SchemaVersion)
	}
}

// TestMigrationsAreIdempotentSQL 检查每个迁移里的**每条** `CREATE` 都带 `IF NOT EXISTS`。
//
// 只检查"整段字符串里出现过一次"是不够的：那样只要有一条写了，同段里其它几条
// 用裸 `CREATE TABLE` 也能蒙过去，而重跑时正是那几条会炸。
func TestMigrationsAreIdempotentSQL(t *testing.T) {
	for index, sqlText := range allMigrations {
		var kept []string
		for _, line := range strings.Split(sqlText, "\n") {
			if strings.HasPrefix(strings.TrimSpace(line), "--") {
				continue
			}
			kept = append(kept, line)
		}
		joined := strings.Join(kept, "\n")

		var statements []string
		for _, s := range strings.Split(joined, ";") {
			s = strings.ToLower(strings.TrimSpace(s))
			if s != "" {
				statements = append(statements, s)
			}
		}

		if len(statements) == 0 {
			t.Fatalf("迁移 v%d 里没解析出任何语句，说明拆分逻辑坏了", index+1)
		}

		for _, statement := range statements {
			if strings.HasPrefix(statement, "create ") {
				if !strings.Contains(statement, "if not exists") {
					t.Fatalf("迁移 v%d 里的这条语句缺少 IF NOT EXISTS，重跑会炸：\n%s",
						index+1, statement)
				}
			}
		}
	}
}

// ── 迁移的运行时行为 ────────────────────────────────────────────────────────

func TestFreshDatabaseReachesCurrentVersion(t *testing.T) {
	db := openRawDB(t)
	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}
	if got := userVersion(t, db); got != SchemaVersion {
		t.Fatalf("新库版本应为 %d，实得 %d", SchemaVersion, got)
	}
}

func TestMigrationIsSafeToRerun(t *testing.T) {
	db := openRawDB(t)
	for i := 0; i < 3; i++ {
		if err := Migrate(context.Background(), db); err != nil {
			t.Fatalf("第 %d 次迁移失败: %v", i+1, err)
		}
	}
	if got := userVersion(t, db); got != SchemaVersion {
		t.Fatalf("重跑后版本应为 %d，实得 %d", SchemaVersion, got)
	}
}

func TestFutureVersionIsRefusedInsteadOfCorrupting(t *testing.T) {
	db := openRawDB(t)
	mustExec(t, db, "PRAGMA user_version = "+strconv.FormatInt(SchemaVersion+5, 10))

	err := Migrate(context.Background(), db)
	if err == nil {
		t.Fatal("未来版本必须拒绝，不能硬写下去")
	}
	if !strings.Contains(err.Error(), "请升级程序") {
		t.Fatalf("拒绝理由应提示升级程序，实得: %v", err)
	}
}

func TestExpectedTablesExistAfterMigration(t *testing.T) {
	db := openRawDB(t)
	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}
	for _, table := range []string{
		// v1
		"accounts", "cloud_stats", "runs",
		// v2
		"account_state", "schedules", "pipelines", "exchange_config", "webui_config", "imports",
		// v3
		"users", "sessions",
		// v8
		"run_events",
	} {
		var count int64
		if err := db.QueryRow(
			"SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1", table,
		).Scan(&count); err != nil {
			t.Fatalf("查表 %s 失败: %v", table, err)
		}
		if count != 1 {
			t.Fatalf("缺少表 %s", table)
		}
	}
}

// TestSingleRowConfigTablesRejectASecondRow 单行配置表必须真的只能有一行 ——
// `CHECK (id = 1)` 是这条断言的依据。
func TestSingleRowConfigTablesRejectASecondRow(t *testing.T) {
	db := openRawDB(t)
	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}
	for _, table := range []string{"exchange_config", "webui_config"} {
		if _, err := db.Exec("INSERT INTO " + table + " (id) VALUES (1)"); err != nil {
			t.Fatalf("%s 第一行应能插入: %v", table, err)
		}
		if _, err := db.Exec("INSERT INTO " + table + " (id) VALUES (2)"); err == nil {
			t.Fatalf("%s 不应允许第二行", table)
		}
	}
}

// ── 各版本迁移的形态（事务包装 / 只加该加的列）─────────────────────────────

func TestV3MigrationIsWrappedInATransaction(t *testing.T) {
	v3 := strings.ToLower(allMigrations[2])
	for _, want := range []string{"begin;", "commit;", "alter table accounts add column owner"} {
		if !strings.Contains(v3, want) {
			t.Fatalf("v3 缺少 %q", want)
		}
	}
}

func TestV4MigrationIsWrappedInATransaction(t *testing.T) {
	v4 := strings.ToLower(allMigrations[3])
	if !strings.Contains(v4, "begin;") || !strings.Contains(v4, "commit;") {
		t.Fatal("v4 必须在显式事务里执行")
	}
	for _, want := range []string{
		"alter table schedules add column owner",
		"alter table pipelines add column owner",
	} {
		if !strings.Contains(v4, want) {
			t.Fatalf("v4 缺少 %q", want)
		}
	}
	if strings.Contains(v4, "create ") {
		t.Fatal("v4 只该是两条 ALTER + 事务包装；建新表属于另一版")
	}
}

func TestV5MigrationAddsTheSmidColumnsInATransaction(t *testing.T) {
	v5 := strings.ToLower(allMigrations[4])
	if !strings.Contains(v5, "begin;") || !strings.Contains(v5, "commit;") {
		t.Fatal("v5 必须在显式事务里执行")
	}
	for _, want := range []string{
		"alter table account_state add column smid_fingerprint",
		"alter table account_state add column smid_device_id",
	} {
		if !strings.Contains(v5, want) {
			t.Fatalf("v5 缺少 %q", want)
		}
	}
	if strings.Contains(v5, "create ") {
		t.Fatal("v5 只该是两条 ALTER + 事务包装；建新表属于另一版")
	}
}

func TestV6MigrationAddsThePhoneColumnInATransaction(t *testing.T) {
	v6 := strings.ToLower(allMigrations[5])
	if !strings.Contains(v6, "begin;") || !strings.Contains(v6, "commit;") {
		t.Fatal("v6 必须在显式事务里执行")
	}
	if !strings.Contains(v6, "alter table account_state add column phone") {
		t.Fatal("v6 要加完整手机号列")
	}
	if strings.Contains(v6, "create ") {
		t.Fatal("v6 只该是一条 ALTER + 事务包装；建新表属于另一版")
	}
}

// TestV8MigrationCreatesRunEventsWithAUniqueSequence v8 是**建表**而非加列，
// 所以断言的重点与 v3…v7 不同。
//
// v3…v7 的形态都是 `ALTER TABLE ... ADD COLUMN`，它们的风险是
// "库改一半、版本没升"，所以要盯事务。v8 用的是 `CREATE TABLE IF NOT EXISTS`，
// 它本身幂等，真正需要盯的是**语义约束有没有落到 DDL 上** ——
// 因为约束一旦漏了，补起来要动数据，而那时库里已经有重复行了。
func TestV8MigrationCreatesRunEventsWithAUniqueSequence(t *testing.T) {
	v8 := strings.ToLower(allMigrations[7])
	if !strings.Contains(v8, "create table if not exists run_events") {
		t.Fatal("v8 要建 run_events 表")
	}
	if !strings.Contains(v8, "primary key (run_id, seq)") {
		t.Fatal("run_events 的 (run_id, seq) 必须唯一，否则续传会漏事件")
	}
	if strings.Contains(v8, "alter table") {
		t.Fatal("v8 只该建 run_events；改已有表属于另一版")
	}
}

// ── 老库升级：列加上、数据一条不动 ──────────────────────────────────────────

func TestV2DatabaseGainsOwnerColumnAndKeepsRows(t *testing.T) {
	db := openRawDB(t)
	// 模拟 v2 老库：只应用 v1 + v2（v3 交给 Migrate）
	mustExec(t, db, allMigrations[0])
	mustExec(t, db, allMigrations[1])
	mustExec(t, db, "PRAGMA user_version = 2")
	mustExec(t, db, `INSERT INTO accounts (id, nickname, enabled, sort_order, fail_streak)
	                 VALUES ('19****01', '账号2', 1, 0, 0)`)

	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	var owner sql.NullString
	if err := db.QueryRow("SELECT owner FROM accounts WHERE id = '19****01'").Scan(&owner); err != nil {
		t.Fatalf("读 owner 失败: %v", err)
	}
	if owner.Valid {
		t.Fatal("迁移期不能给历史账号硬编 owner —— 那时首个管理员还不存在，只能落到未归属")
	}

	if cols := tableColumns(t, db, "accounts"); !hasColumn(cols, "owner") {
		t.Fatalf("缺列 owner: %v", cols)
	}
}

func TestV4DatabaseGainsSmidColumnsAndKeepsCredentials(t *testing.T) {
	db := openRawDB(t)
	for _, sqlText := range allMigrations[:4] {
		mustExec(t, db, sqlText)
	}
	mustExec(t, db, "PRAGMA user_version = 4")
	mustExec(t, db, `INSERT INTO accounts (id, nickname, enabled, sort_order, fail_streak)
	                 VALUES ('19****01', '账号2', 1, 0, 0)`)
	mustExec(t, db, `INSERT INTO account_state (account_id, jwt_token, device_id, updated_at)
	                 VALUES ('19****01', 'OLD-TOKEN', 'OLD-ENDIN', '2026-09-18T00:00:00Z')`)

	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	cols := tableColumns(t, db, "account_state")
	for _, want := range []string{"smid_fingerprint", "smid_device_id"} {
		if !hasColumn(cols, want) {
			t.Fatalf("缺列 %s: %v", want, cols)
		}
	}

	// 新列对老行是 NULL：绑定的特征**只能由用户主动生成**，迁移期凭空造一份
	// 等于替每台老库发明一台设备（与 v3 不给 owner 硬编值是同一条理由）
	var (
		jwt, device          string
		fingerprint, smidDev sql.NullString
	)
	if err := db.QueryRow(
		`SELECT jwt_token, device_id, smid_fingerprint, smid_device_id
		 FROM account_state WHERE account_id = '19****01'`,
	).Scan(&jwt, &device, &fingerprint, &smidDev); err != nil {
		t.Fatalf("读登录态失败: %v", err)
	}
	if jwt != "OLD-TOKEN" {
		t.Fatalf("升级不该动凭据，实得 %q", jwt)
	}
	if device != "OLD-ENDIN" {
		t.Fatalf("升级不该动 device_id，实得 %q", device)
	}
	if fingerprint.Valid || smidDev.Valid {
		t.Fatalf("新列对老行必须是 NULL，实得 %v / %v", fingerprint, smidDev)
	}
}

// TestV5DatabaseGainsThePhoneColumnWithoutBackfillingTheMask
// 断言"是 NULL"而不是"等于某个值"是有意的：`NULL` = 如实记录"当时没存"，
// 拿 `phone_masked`（`19****01`）去填会让 `Authorization` 拼出一个
// **服务端认不认未知**的头 —— 那正是本项目反复踩的"能过但不该过"。
func TestV5DatabaseGainsThePhoneColumnWithoutBackfillingTheMask(t *testing.T) {
	db := openRawDB(t)
	for _, sqlText := range allMigrations[:5] {
		mustExec(t, db, sqlText)
	}
	mustExec(t, db, "PRAGMA user_version = 5")
	mustExec(t, db, `INSERT INTO accounts (id, nickname, phone_masked, enabled, sort_order, fail_streak)
	                 VALUES ('19****01', '账号2', '19****01', 1, 0, 0)`)
	mustExec(t, db, `INSERT INTO account_state (account_id, jwt_token, device_id, updated_at)
	                 VALUES ('19****01', 'OLD-TOKEN', 'OLD-ENDIN', '2026-09-18T00:00:00Z')`)

	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	var (
		jwt   string
		phone sql.NullString
	)
	if err := db.QueryRow(
		"SELECT jwt_token, phone FROM account_state WHERE account_id = '19****01'",
	).Scan(&jwt, &phone); err != nil {
		t.Fatalf("读登录态失败: %v", err)
	}
	if jwt != "OLD-TOKEN" {
		t.Fatalf("升级不该动凭据，实得 %q", jwt)
	}
	if phone.Valid {
		t.Fatalf("老行的 phone 必须是 NULL，不许拿掩码回填，实得 %q", phone.String)
	}
}

func TestUpgradingKeepsExistingRows(t *testing.T) {
	db := openRawDB(t)
	// 只应用 v1，模拟"老库"
	mustExec(t, db, allMigrations[0])
	mustExec(t, db, "PRAGMA user_version = 1")
	mustExec(t, db, `INSERT INTO accounts (id, nickname, enabled, sort_order, fail_streak)
	                 VALUES ('19****01', '账号2', 1, 0, 0)`)

	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	var nickname string
	if err := db.QueryRow("SELECT nickname FROM accounts WHERE id = '19****01'").Scan(&nickname); err != nil {
		t.Fatalf("读 nickname 失败: %v", err)
	}
	if nickname != "账号2" {
		t.Fatalf("升级不该动老数据，实得 %q", nickname)
	}
}

// TestRunEventsRejectADuplicateSequence `(run_id, seq)` 的唯一性要在真库上验一遍，
// 不能只看 SQL 文本。
func TestRunEventsRejectADuplicateSequence(t *testing.T) {
	db := openRawDB(t)
	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	if _, err := db.Exec(
		`INSERT INTO run_events (run_id, seq, kind, payload, created_at)
		 VALUES ('run-a', 1, 'log', '{}', '2026-09-20T00:00:00Z')`); err != nil {
		t.Fatalf("第一条事件应能插入: %v", err)
	}
	if _, err := db.Exec(
		`INSERT INTO run_events (run_id, seq, kind, payload, created_at)
		 VALUES ('run-a', 2, 'log', '{}', '2026-09-20T00:00:00Z')`); err != nil {
		t.Fatalf("第二条事件应能插入: %v", err)
	}

	// 同一 run 重复序号 → 拒绝（这正是续传锚点被破坏的样子）
	if _, err := db.Exec(
		`INSERT INTO run_events (run_id, seq, kind, payload, created_at)
		 VALUES ('run-a', 2, 'log', '{}', '2026-09-20T00:00:00Z')`); err == nil {
		t.Fatal("同一 run 的 seq 不能重复")
	}
	// 不同 run 可以用同一个 seq —— 序号是**每个 run 各自**的计数
	if _, err := db.Exec(
		`INSERT INTO run_events (run_id, seq, kind, payload, created_at)
		 VALUES ('run-b', 1, 'log', '{}', '2026-09-20T00:00:00Z')`); err != nil {
		t.Fatalf("不同 run 的 seq 应当互不干扰: %v", err)
	}

	// 最常走的查询是"某个 run 按 seq 序取出"，验证索引/主键真的能用且有序
	rows, err := db.Query("SELECT seq FROM run_events WHERE run_id = ?1 ORDER BY seq", "run-a")
	if err != nil {
		t.Fatalf("查事件失败: %v", err)
	}
	defer func() { _ = rows.Close() }()
	var seqs []int64
	for rows.Next() {
		var s int64
		if err := rows.Scan(&s); err != nil {
			t.Fatalf("扫描 seq 失败: %v", err)
		}
		seqs = append(seqs, s)
	}
	if len(seqs) != 2 || seqs[0] != 1 || seqs[1] != 2 {
		t.Fatalf("seq 应为 [1 2]，实得 %v", seqs)
	}
}

// TestV7DatabaseGainsRunEventsAndKeepsRuns v7 老库升到 v8：`runs` 里的历史记录
// 一条都不能动，且新表是空的。
//
// 空表是对的：v8 之前的事件**根本没落过库**（旧项目只放内存），
// 所以没有东西可以回填 —— 造一批假事件反而会污染"那次运行到底发生了什么"。
func TestV7DatabaseGainsRunEventsAndKeepsRuns(t *testing.T) {
	db := openRawDB(t)
	for _, sqlText := range allMigrations[:7] {
		mustExec(t, db, sqlText)
	}
	mustExec(t, db, "PRAGMA user_version = 7")
	mustExec(t, db, `INSERT INTO runs (id, account_id, capability, status, started_at, message)
	                 VALUES ('run-old', '19****01', 'sign', 'Done', '2026-09-19T00:00:00Z', '旧记录')`)

	if err := Migrate(context.Background(), db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	var message string
	if err := db.QueryRow("SELECT message FROM runs WHERE id = 'run-old'").Scan(&message); err != nil {
		t.Fatalf("读 runs 失败: %v", err)
	}
	if message != "旧记录" {
		t.Fatalf("升级不该动 runs 里的历史，实得 %q", message)
	}

	var events int64
	if err := db.QueryRow("SELECT COUNT(*) FROM run_events").Scan(&events); err != nil {
		t.Fatalf("数事件失败: %v", err)
	}
	if events != 0 {
		t.Fatalf("v8 之前没有事件记录，不许凭空造，实得 %d 条", events)
	}
}
