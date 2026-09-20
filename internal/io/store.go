package mcloudio

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// SqliteStore 是 [core.StateStore] 的 SQLite 实现。
//
// # 为什么用 SQLite（这是对原项目惯例的有意变更）
//
// 原项目用本地 JSON（`packages/api/data/*.json`）。但重写后有三类**查询型**需求：
// 30 天云朵趋势、运行历史按账号/能力/状态筛选、事件流回放。
// JSON 方案每次都要整文件读写 + 内存全表扫，既慢又易损。
// 因此改为 SQLite，而 `asign.json` 仍作为配置源保留 —— 配置要人手改，数据不要。
//
// # 并发模型
//
// 单进程、本地文件、操作都是微秒级，因此把连接池**压到 1**（`MaxOpenConns(1)`）
// 顺序化，不做真连接池 —— 与 Rust 侧 `Mutex<Connection>` 是同一个选择。
//
// ⚠ 由此得到一条实现约束：**方法之间不能互相调用**（会重入同一把连接，
// 在 MaxOpenConns(1) 下自锁）。Rust 侧靠"所有方法体内不含 .await"避免，
// Go 侧靠这条注释与 code review 避免 —— 语言层面没有等价保证。
type SqliteStore struct {
	db *sql.DB
}

// Open 打开（或创建）数据库并执行迁移。
//
// ⚠ 迁移走 [context.Background]：这是**启动期**的一次性动作，与请求无关，
// 而 Rust 侧的 `SqliteStore::open` 同样不接受取消信号。
func Open(path string) (*SqliteStore, error) {
	if dir := filepath.Dir(path); dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, core.NewInternal(fmt.Sprintf("创建数据目录失败 %s: %v", dir, err))
		}
	}
	return openDSN("file:" + path + "?_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)")
}

// OpenInMemory 返回一个内存库，用于单测。
//
// ⚠ `_pragma=foreign_keys(1)` 写在 DSN 里而不是开库后 `Exec`：
// pragma 是**按连接**生效的，写进 DSN 才能保证池里每条新连接都带上它。
func OpenInMemory() (*SqliteStore, error) {
	return openDSN("file::memory:?_pragma=foreign_keys(1)")
}

func openDSN(dsn string) (*SqliteStore, error) {
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, core.NewInternal(fmt.Sprintf("打开数据库失败: %v", err))
	}
	// 单连接：见 SqliteStore 的并发模型说明。
	// `ConnMaxLifetime(0)` 不是调优 —— 内存库**随连接消失**，
	// 连接一旦被回收，后面所有断言都会变成"表不存在"这种假红。
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	db.SetConnMaxLifetime(0)

	if err := Migrate(context.Background(), db); err != nil {
		_ = db.Close()
		return nil, err
	}
	return &SqliteStore{db: db}, nil
}

// Close 关掉底层连接池。
func (s *SqliteStore) Close() error { return s.db.Close() }

// SchemaVersion 返回库里的当前 schema 版本（供"数据版本与迁移函数清单一致"的门禁使用）。
func (s *SqliteStore) SchemaVersion(ctx context.Context) (int64, error) {
	var v int64
	if err := s.db.QueryRowContext(ctx, "PRAGMA user_version").Scan(&v); err != nil {
		return 0, mapSQLite(err)
	}
	return v, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 共用工具
// ─────────────────────────────────────────────────────────────────────────────

func mapSQLite(err error) *core.Error {
	return core.NewInternal("SQLite 错误: " + err.Error())
}

// rowScanner 让 `*sql.Row` 与 `*sql.Rows` 共用同一个扫描函数。
type rowScanner interface{ Scan(dest ...any) error }

// ownerArg 把 [core.Scope] 的归属转成 SQL 参数（nil = 不过滤）。
func ownerArg(scope core.Scope) any {
	if o := scope.Owner(); o != nil {
		return o.String()
	}
	return nil
}

func optStr(p *string) any {
	if p == nil {
		return nil
	}
	return *p
}

func optI64(p *int64) any {
	if p == nil {
		return nil
	}
	return *p
}

// optTime 序列化可选时间列。
//
// 走 [formatTime]（与 chrono 逐字节一致），**不是** [time.RFC3339Nano] ——
// 理由见 `datetime.go`：这一列在 SQL 里被当字符串比较与排序，
// 混两种写法会让"同一时刻的先后"取决于后缀。
func optTime(p *time.Time) any {
	if p == nil {
		return nil
	}
	return formatTime(*p)
}

func optSecret(p *core.SecretValue) any {
	if p == nil {
		return nil
	}
	return p.Value()
}

func optFullPhone(p *core.FullPhone) any {
	if p == nil {
		return nil
	}
	return p.Value()
}

func optUserDomainID(p *core.UserDomainID) any {
	if p == nil {
		return nil
	}
	return p.String()
}

// parseTime 解析库里的时间列。
//
// 吃 chrono 写出来的全部形态（`+00:00` 后缀、0/3/6/9 位亚秒）以及 Go 自己
// 曾经写过的 `Z` —— 老库两种都可能有。
//
// ⚠ 与 chrono 的 `parse_from_rfc3339` 相比，Go **严格更严**，实测有四处
// chrono 收而 Go 拒：空格代替 `T`、小写 `t`/`z`、闰秒 `12:00:60`。
// 方向是安全的（Go 绝不会把 chrono 认为非法的串当合法读进来），
// 而且这四种 chrono 自己**从不写出来**。逐条钉在
// `test/golden/datetime-format-vectors.json` 的 `$parse` 段里。
func parseTime(raw string) (time.Time, bool) {
	t, err := time.Parse(time.RFC3339, raw)
	if err != nil {
		return time.Time{}, false
	}
	return t.UTC(), true
}

// requiredTime 读取**非空**时间列。
//
// 解析失败时报错，而不是退回一个"现在" —— 后者会让"库里存着坏时间"这件事
// 永远不被发现，而它偏偏出现在 `created_at` / `expires_at` 这种关键列上。
func requiredTime(raw string) (time.Time, error) {
	t, ok := parseTime(raw)
	if !ok {
		return time.Time{}, core.NewInternal("时间列不是合法 RFC 3339: " + raw)
	}
	return t, nil
}

func ptrStr(ns sql.NullString) *string {
	if !ns.Valid {
		return nil
	}
	v := ns.String
	return &v
}

func ptrI64(ni sql.NullInt64) *int64 {
	if !ni.Valid {
		return nil
	}
	v := ni.Int64
	return &v
}

func ptrTime(ns sql.NullString) *time.Time {
	if !ns.Valid {
		return nil
	}
	t, ok := parseTime(ns.String)
	if !ok {
		return nil
	}
	return &t
}

// ─────────────────────────────────────────────────────────────────────────────
// 归属校验
// ─────────────────────────────────────────────────────────────────────────────

// validateOwned 校验账号属于该用户；否则返回 [core.NewNotFound]。
//
// # 为什么写入路径用 NotFound 而不是 Forbidden
//
// 写入路径没法像读取路径那样返回 nil，但**对外结论必须与读取路径一致**：
// 读取说"没有这个账号"，写入却说"你不能写这个账号"，两者的差异本身就告诉调用方
// "这个 id 是存在的"，账号 id 空间于是变成可枚举的。
func validateOwned(ctx context.Context, db *sql.DB, scope core.Scope, id core.AccountID) error {
	var found int64
	err := db.QueryRowContext(ctx,
		"SELECT 1 FROM accounts WHERE id = ? AND (? IS NULL OR owner = ?)",
		id.String(), ownerArg(scope), ownerArg(scope),
	).Scan(&found)
	if errors.Is(err, sql.ErrNoRows) {
		return core.NewNotFound("账号 " + id.String())
	}
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// validateRunInScope 确认这个 run 在范围内 —— **`run_events` 的归属校验全在这里**。
//
// # 为什么要顺着 `runs` 查，而不是给事件表加一个 `owner` 列
//
// 事件行上没有 `owner`，只有 `run_id`。给事件表加一列"冗余的 owner"看起来省事，
// 但那会立刻带来一个经典问题：**同一份事实两个存储位置，迟早不一致** ——
// 而修复方式只能靠"记得两个地方一起改"。顺着 `runs` 查一次只有一次子查询的成本，
// 换来的是"归属只有一个来源"。
//
// # 为什么 `account_id` 为空的 run 只有 [core.ScopeLocal] 能用
//
// 与 [SqliteStore.ListRuns] 对它们的可见性完全一致：那种 run 无法归属
// （旧数据导入时"匹配不到账号"的历史），所以它不出现在任何人的列表里。
// 若这里放行 `ScopeUser`，服务层就能往一个自己看不见的 run 里塞日志。
func validateRunInScope(ctx context.Context, db *sql.DB, scope core.Scope, runID core.RunID) error {
	var found int64
	err := db.QueryRowContext(ctx,
		`SELECT 1 FROM runs
		 WHERE id = ?
		   AND (? IS NULL OR account_id IN (SELECT id FROM accounts WHERE owner = ?))`,
		runID.String(), ownerArg(scope), ownerArg(scope),
	).Scan(&found)
	if errors.Is(err, sql.ErrNoRows) {
		return core.NewNotFound("运行记录 " + runID.String())
	}
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 事件序列化
// ─────────────────────────────────────────────────────────────────────────────

// serializeEvent 把事件序列化成落库的 payload。
//
// ⚠ 直接调 [core.RunEvent.MarshalJSON] 而**不是** `json.Marshal(event)`：
// 后者会在拿到 Marshaler 的字节之后再跑一遍 compact，而那一遍**会转义
// `<` `>` `&`**（`json.Marshal` 的 escapeHTML 默认开），与 Rust 的 serde_json
// 不一致。事件正文里带尖括号是常事（错误信息、URL）。
//
// 序列化失败在这里是 Internal 而不是丢弃：`RunEvent` 全是字符串/数字/枚举，
// 正常不可能失败。真失败了说明类型定义里混进了不可序列化的东西 ——
// 那是**程序缺陷**，静默丢一条事件只会让排查更晚。
func serializeEvent(event core.RunEvent) (string, error) {
	b, err := event.MarshalJSON()
	if err != nil {
		return "", core.NewInternal("运行事件序列化失败（类型定义有问题）: " + err.Error())
	}
	return string(b), nil
}

// deserializeEvent 从 payload 还原事件。
//
// # 解不出来时给的是 Internal，不是"跳过这条"
//
// 这个选择影响排障体验：跳过会让前端看到一个**少了中间几条**的进度
// （比如 `progress 3/20` 之后直接跳到 `9/20`），而人只会怀疑"是不是漏发了"，
// 不会想到是库里有一条坏了。直接报错至少能把"哪一行坏了"暴露出来。
func deserializeEvent(payload string) (core.RunEvent, error) {
	var e core.RunEvent
	if err := json.Unmarshal([]byte(payload), &e); err != nil {
		return core.RunEvent{}, core.NewInternal(
			fmt.Sprintf("运行事件反序列化失败（payload: %s）: %v", payload, err))
	}
	return e, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// accounts
// ─────────────────────────────────────────────────────────────────────────────

// accountColumns 是 `accounts` 的列顺序 —— **只此一处**。
//
// 加列时改这里 + [scanAccount] + UpsertAccount；
// 三条 SELECT 各写一份列清单必然漂移，而漂移的表现是
// "某条查询静默少一列"，不会报错。
const accountColumns = "id, nickname, phone_masked, platform, enabled, sort_order, " +
	"cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note, owner"

func scanAccount(s rowScanner) (core.Account, error) {
	var (
		id                    string
		nickname, phoneMasked sql.NullString
		platform              sql.NullString
		enabled, sortOrder    int64
		cloudCurrent          sql.NullInt64
		cloudUpdatedAt        sql.NullString
		authExpire            sql.NullInt64
		lastOKAt              sql.NullString
		failStreak            int64
		note, owner           sql.NullString
	)
	if err := s.Scan(&id, &nickname, &phoneMasked, &platform, &enabled, &sortOrder,
		&cloudCurrent, &cloudUpdatedAt, &authExpire, &lastOKAt, &failStreak, &note, &owner,
	); err != nil {
		return core.Account{}, err
	}
	a := core.Account{
		ID:             core.NewAccountID(id),
		Nickname:       ptrStr(nickname),
		PhoneMasked:    ptrStr(phoneMasked),
		Platform:       ptrStr(platform),
		Enabled:        enabled != 0,
		SortOrder:      sortOrder,
		CloudCurrent:   ptrI64(cloudCurrent),
		CloudUpdatedAt: ptrTime(cloudUpdatedAt),
		AuthExpire:     ptrI64(authExpire),
		LastOKAt:       ptrTime(lastOKAt),
		FailStreak:     failStreak,
		Note:           ptrStr(note),
	}
	if owner.Valid {
		u := core.NewUserID(owner.String)
		a.Owner = &u
	}
	return a, nil
}

// UpsertAccount 写入账号。见 [core.StateStore.UpsertAccount]。
func (s *SqliteStore) UpsertAccount(ctx context.Context, scope core.Scope, account *core.Account) error {
	// ① 用户范围下不能替别人建账号，也不能把账号写成"无主"
	//（无主只由 schema 迁移与本地运维产生）。本地范围不做这条检查。
	if uid := scope.Owner(); uid != nil {
		if account.Owner == nil || account.Owner.String() != uid.String() {
			return core.NewNotFound("账号 " + account.ID.String())
		}
	}
	if account.ID.String() == "" {
		return core.NewBadRequest("账号 id 不能为空")
	}

	// ② 同一个 AccountId 已被别人占用时**不能覆盖**。
	//
	// AccountId 由脱敏手机号派生，是**全局唯一**的，所以"两个系统用户添加同一个
	// 云盘账号"在当前的 `accounts(id)` 主键下做不到 —— 这是已知限制，正确修法是
	// 把主键改成 `(owner, id)`（需一次表重建迁移）。
	// 在那之前这里要被显式挡住，而不是让 upsert 静默把别人的账号顶掉。
	var existingOwner sql.NullString
	err := s.db.QueryRowContext(ctx,
		"SELECT owner FROM accounts WHERE id = ?", account.ID.String()).Scan(&existingOwner)
	switch {
	case err == nil:
		if uid := scope.Owner(); uid != nil && existingOwner.Valid && existingOwner.String != uid.String() {
			// 给出可行动的提示：让它表现成"账号不存在"会变成查不出原因的怪象
			return core.NewBadRequest("该账号已被其他用户添加")
		}
	case errors.Is(err, sql.ErrNoRows):
		// 没有旧行，继续插入
	default:
		return mapSQLite(err)
	}

	_, err = s.db.ExecContext(ctx, `
		INSERT INTO accounts (
			id, nickname, phone_masked, platform, enabled, sort_order,
			cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note, owner
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			nickname         = excluded.nickname,
			phone_masked     = excluded.phone_masked,
			platform         = excluded.platform,
			enabled          = excluded.enabled,
			sort_order       = excluded.sort_order,
			cloud_current    = excluded.cloud_current,
			cloud_updated_at = excluded.cloud_updated_at,
			auth_expire      = excluded.auth_expire,
			last_ok_at       = excluded.last_ok_at,
			fail_streak      = excluded.fail_streak,
			note             = excluded.note,
			owner            = excluded.owner
		`,
		account.ID.String(),
		optStr(account.Nickname),
		optStr(account.PhoneMasked),
		optStr(account.Platform),
		boolToInt(account.Enabled),
		account.SortOrder,
		optI64(account.CloudCurrent),
		optTime(account.CloudUpdatedAt),
		optI64(account.AuthExpire),
		optTime(account.LastOKAt),
		account.FailStreak,
		optStr(account.Note),
		userIDArg(account.Owner),
	)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// GetAccount 取账号。不在该范围内时返回 nil, nil。
func (s *SqliteStore) GetAccount(ctx context.Context, scope core.Scope, id core.AccountID) (*core.Account, error) {
	// 归属条件**写进 SQL**：不属于该用户的行根本不会出现在结果集里，
	// 所以它天然返回 nil（与"不存在"同一个结论）
	a, err := scanAccount(s.db.QueryRowContext(ctx,
		"SELECT "+accountColumns+" FROM accounts WHERE id = ? AND (? IS NULL OR owner = ?)",
		id.String(), ownerArg(scope), ownerArg(scope)))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &a, nil
}

// ListAccounts 该范围内可见的账号，按 `sort_order` 升序。
func (s *SqliteStore) ListAccounts(ctx context.Context, scope core.Scope) ([]core.Account, error) {
	rows, err := s.db.QueryContext(ctx,
		"SELECT "+accountColumns+" FROM accounts WHERE (? IS NULL OR owner = ?) "+
			"ORDER BY sort_order ASC, id ASC",
		ownerArg(scope), ownerArg(scope))
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.Account{}
	for rows.Next() {
		a, err := scanAccount(rows)
		if err != nil {
			return nil, mapSQLite(err)
		}
		out = append(out, a)
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// SetAccountCloud 记下账号**当前**的云朵余额（窄写）。
func (s *SqliteStore) SetAccountCloud(
	ctx context.Context, scope core.Scope, id core.AccountID, count int64, at time.Time,
) error {
	// 归属检查放在 UPDATE 之前：`ScopeUser` 下写别人的账号要报错，
	// 而不是"UPDATE 影响 0 行"这种看不出对错的静默结果。
	if err := validateOwned(ctx, s.db, scope, id); err != nil {
		return err
	}
	if _, err := s.db.ExecContext(ctx,
		"UPDATE accounts SET cloud_current = ?, cloud_updated_at = ? WHERE id = ?",
		count, formatTime(at), id.String()); err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// account_state
// ─────────────────────────────────────────────────────────────────────────────

// GetAccountState 读取账号登录态（含凭据）。
//
// 归属条件用 JOIN 和取值放在**同一条语句**里：先校验再取会有 TOCTOU 窗口，
// 而且那条路径上"忘了校验"不会有任何提示。
func (s *SqliteStore) GetAccountState(ctx context.Context, scope core.Scope, id core.AccountID) (*core.AccountState, error) {
	var (
		accountID                       string
		jwtToken, deviceID              sql.NullString
		userDomainID                    sql.NullString
		redpack                         sql.NullString
		phone, authorization, updatedAt sql.NullString
	)
	err := s.db.QueryRowContext(ctx,
		`SELECT s.account_id, s.jwt_token, s.device_id, s.user_domain_id,
		        s.cloud_phone_redpack_token, s.phone, s.authorization, s.updated_at
		 FROM account_state s JOIN accounts a ON a.id = s.account_id
		 WHERE s.account_id = ? AND (? IS NULL OR a.owner = ?)`,
		id.String(), ownerArg(scope), ownerArg(scope),
	).Scan(&accountID, &jwtToken, &deviceID, &userDomainID, &redpack, &phone, &authorization, &updatedAt)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	st := core.AccountState{
		AccountID:     core.NewAccountID(accountID),
		JWTToken:      ptrSecret(jwtToken),
		DeviceID:      ptrSecret(deviceID),
		UserDomainID:  ptrUserDomainID(userDomainID),
		RedpackToken:  ptrSecret(redpack),
		Phone:         ptrFullPhone(phone),
		Authorization: ptrSecret(authorization),
		UpdatedAt:     ptrTime(updatedAt),
	}
	return &st, nil
}

// UpsertAccountState 覆盖写入登录态。
//
// 是**覆盖**而不是"合并"：登录态是一个整体快照，nil 表示"上游这次没给"，
// 若做合并就永远删不掉已经失效的 token（例如重新登录换了新 deviceId 之后，
// 旧 deviceId 还留在库里，下次就会被拿出来用）。
func (s *SqliteStore) UpsertAccountState(ctx context.Context, scope core.Scope, state *core.AccountState) error {
	if err := validateOwned(ctx, s.db, scope, state.AccountID); err != nil {
		return err
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO account_state (
			account_id, jwt_token, device_id, user_domain_id,
			cloud_phone_redpack_token, phone, authorization, updated_at
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(account_id) DO UPDATE SET
			jwt_token                 = excluded.jwt_token,
			device_id                 = excluded.device_id,
			user_domain_id            = excluded.user_domain_id,
			cloud_phone_redpack_token = excluded.cloud_phone_redpack_token,
			phone                     = excluded.phone,
			authorization             = excluded.authorization,
			updated_at                = excluded.updated_at
		`,
		state.AccountID.String(),
		optSecret(state.JWTToken),
		optSecret(state.DeviceID),
		optUserDomainID(state.UserDomainID),
		optSecret(state.RedpackToken),
		optFullPhone(state.Phone),
		optSecret(state.Authorization),
		optTime(state.UpdatedAt),
	)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ── smid：绑定账号的设备特征 + 最近拿到的 deviceId ──
//
// 这三条**刻意不走** `UpsertAccountState` 那张"整体覆盖"的路，理由见
// [core.SmidState] 与 schema.go 的 v5 注释：换凭据不该换设备长相。

// GetSmidState 读这个账号的 smid 状态。
func (s *SqliteStore) GetSmidState(ctx context.Context, scope core.Scope, id core.AccountID) (*core.SmidState, error) {
	var (
		accountID               string
		fingerprint, smidDevice sql.NullString
	)
	// 与 GetAccountState 同一个形状：归属条件写在**同一条语句**里（TOCTOU 的理由见那处）
	err := s.db.QueryRowContext(ctx,
		`SELECT s.account_id, s.smid_fingerprint, s.smid_device_id
		 FROM account_state s JOIN accounts a ON a.id = s.account_id
		 WHERE s.account_id = ? AND (? IS NULL OR a.owner = ?)`,
		id.String(), ownerArg(scope), ownerArg(scope),
	).Scan(&accountID, &fingerprint, &smidDevice)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &core.SmidState{
		AccountID:       core.NewAccountID(accountID),
		FingerprintJSON: ptrSecret(fingerprint),
		DeviceID:        ptrSecret(smidDevice),
	}, nil
}

// SetSmidFingerprint 写入**新的**设备特征，并把 `smid_device_id` 清空。
//
// 写特征与清空旧 deviceId 是**同一条语句**：拆成两次调用，中间崩溃就留下
// "新长相 + 旧 id"，而这两列各自都合法，事后看不出发生过什么。
func (s *SqliteStore) SetSmidFingerprint(ctx context.Context, scope core.Scope, id core.AccountID, fingerprintJSON string) error {
	if err := validateOwned(ctx, s.db, scope, id); err != nil {
		return err
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO account_state (account_id, smid_fingerprint, smid_device_id)
		VALUES (?, ?, NULL)
		ON CONFLICT(account_id) DO UPDATE SET
			smid_fingerprint = excluded.smid_fingerprint,
			smid_device_id   = NULL
		`, id.String(), fingerprintJSON)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// SetSmidDeviceID 记下最近一次上报换回的 `deviceId`（含 `B` 前缀）。
//
// 只碰这一列，**不动 updated_at**：那一列记的是"登录态快照的时间"，
// 一次指纹上报不是新快照。
func (s *SqliteStore) SetSmidDeviceID(ctx context.Context, scope core.Scope, id core.AccountID, deviceID string) error {
	if err := validateOwned(ctx, s.db, scope, id); err != nil {
		return err
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO account_state (account_id, smid_device_id)
		VALUES (?, ?)
		ON CONFLICT(account_id) DO UPDATE SET
			smid_device_id = excluded.smid_device_id
		`, id.String(), deviceID)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// FillAccountCredentials 把账号**缺**的 `authorization` / `phone` 两列填上 ——
// 只碰这两列，**不动 `updated_at`**，也**不覆盖已有值**。
//
// # 为什么 COALESCE 的两边是反的
//
// 写惯了 `= excluded.x` 的人会本能地把 `excluded` 放前面，那样就成了"有值就覆盖"。
// 这里要的是**填空缺**：已有值优先，`excluded` 只在那一列是 NULL 时才生效。
// 顺序反了不会报错，只会让"每次 migrate 都重跑的回填"把重登后的新凭据
// 用旧配置里的串盖回去 —— 而且事后完全看不出发生过什么。
func (s *SqliteStore) FillAccountCredentials(
	ctx context.Context, scope core.Scope, id core.AccountID, authorization, phone *string,
) error {
	if err := validateOwned(ctx, s.db, scope, id); err != nil {
		return err
	}
	// 刻意**不写 updated_at**：那一列是"登录态快照的时间"，补一个缺口不是新快照。
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO account_state (account_id, authorization, phone)
		VALUES (?, ?, ?)
		ON CONFLICT(account_id) DO UPDATE SET
			authorization = COALESCE(account_state.authorization, excluded.authorization),
			phone         = COALESCE(account_state.phone,         excluded.phone)
		`, id.String(), optStr(authorization), optStr(phone))
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// cloud_stats
// ─────────────────────────────────────────────────────────────────────────────

// UpsertCloudStats 按 `account + date` UPSERT 日快照。
func (s *SqliteStore) UpsertCloudStats(ctx context.Context, scope core.Scope, stats *core.CloudStats) error {
	if err := validateOwned(ctx, s.db, scope, stats.AccountID); err != nil {
		return err
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO cloud_stats (account_id, date, cloud_count, cloud_diff, cloud_diff_week)
		VALUES (?, ?, ?, ?, ?)
		ON CONFLICT(account_id, date) DO UPDATE SET
			cloud_count     = excluded.cloud_count,
			cloud_diff      = excluded.cloud_diff,
			cloud_diff_week = excluded.cloud_diff_week
		`,
		stats.AccountID.String(), stats.Date, stats.CloudCount, stats.CloudDiff, stats.CloudDiffWeek)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// CloudStatsSince 取 `date >= since` 的快照，按日期升序。
func (s *SqliteStore) CloudStatsSince(ctx context.Context, scope core.Scope, id core.AccountID, since string) ([]core.CloudStats, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT s.account_id, s.date, s.cloud_count, s.cloud_diff, s.cloud_diff_week
		 FROM cloud_stats s JOIN accounts a ON a.id = s.account_id
		 WHERE s.account_id = ? AND (? IS NULL OR a.owner = ?) AND s.date >= ?
		 ORDER BY s.date ASC`,
		id.String(), ownerArg(scope), ownerArg(scope), since)
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.CloudStats{}
	for rows.Next() {
		var (
			accountID string
			date      string
			count     int64
			diff      int64
			diffWeek  int64
		)
		if err := rows.Scan(&accountID, &date, &count, &diff, &diffWeek); err != nil {
			return nil, mapSQLite(err)
		}
		out = append(out, core.CloudStats{
			AccountID:     core.NewAccountID(accountID),
			Date:          date,
			CloudCount:    count,
			CloudDiff:     diff,
			CloudDiffWeek: diffWeek,
		})
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// runs
// ─────────────────────────────────────────────────────────────────────────────

// InsertRun 写运行记录。带账号时按范围校验归属。
func (s *SqliteStore) InsertRun(ctx context.Context, scope core.Scope, run *core.RunRecord) error {
	// 带账号的运行记录必须先确认账号是自己的；不带账号的（旧数据里"匹配不到账号"
	// 的历史）允许写入 —— 它随后通过 ListRuns 也对谁都不可见，只是留个痕迹。
	if run.AccountID != nil {
		if err := validateOwned(ctx, s.db, scope, *run.AccountID); err != nil {
			return err
		}
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO runs (
			id, account_id, capability, status, started_at, finished_at,
			cloud_gained, cloud_after, message
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(id) DO NOTHING
		`,
		run.ID,
		accountIDArg(run.AccountID),
		run.Capability,
		run.Status.String(),
		formatTime(run.StartedAt),
		optTime(run.FinishedAt),
		optI64(run.CloudGained),
		optI64(run.CloudAfter),
		optStr(run.Message),
	)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// UpdateRun 更新运行记录。只能更新该范围内的记录。
func (s *SqliteStore) UpdateRun(ctx context.Context, scope core.Scope, run *core.RunRecord) error {
	res, err := s.db.ExecContext(ctx, `
		UPDATE runs SET
			status       = ?,
			finished_at  = ?,
			cloud_gained = ?,
			cloud_after  = ?,
			message      = ?
		WHERE id = ?
		  AND (? IS NULL OR account_id IN (SELECT id FROM accounts WHERE owner = ?))
		`,
		run.Status.String(),
		optTime(run.FinishedAt),
		optI64(run.CloudGained),
		optI64(run.CloudAfter),
		optStr(run.Message),
		run.ID,
		ownerArg(scope), ownerArg(scope),
	)
	if err != nil {
		return mapSQLite(err)
	}
	affected, err := res.RowsAffected()
	if err != nil {
		return mapSQLite(err)
	}
	if affected == 0 {
		// 与读取路径同一个结论：不属于你的记录，对你而言就是不存在。
		// （不用 Internal：那是"程序内部出问题"，而这是正常的越权请求。）
		return core.NewNotFound("运行记录 " + run.ID)
	}
	return nil
}

// ListRuns 该范围内可见的运行记录，按开始时间倒序。
//
// `account_id` 为空的记录**不出现**在任何人的列表里：它无法归属，
// 而"无法归属就谁都看得见"会让别人的历史混进我的列表。
func (s *SqliteStore) ListRuns(ctx context.Context, scope core.Scope, limit int) ([]core.RunRecord, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT id, account_id, capability, status, started_at, finished_at,
		        cloud_gained, cloud_after, message
		 FROM runs
		 WHERE (? IS NULL OR account_id IN (SELECT id FROM accounts WHERE owner = ?))
		 ORDER BY started_at DESC LIMIT ?`,
		ownerArg(scope), ownerArg(scope), limit)
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.RunRecord{}
	for rows.Next() {
		var (
			id, capability, status, started string
			accountID                       sql.NullString
			finished                        sql.NullString
			cloudGained, cloudAfter         sql.NullInt64
			message                         sql.NullString
		)
		if err := rows.Scan(&id, &accountID, &capability, &status, &started, &finished,
			&cloudGained, &cloudAfter, &message); err != nil {
			return nil, mapSQLite(err)
		}
		startedAt, err := requiredTime(started)
		if err != nil {
			return nil, err
		}
		rec := core.RunRecord{
			ID:          id,
			AccountID:   ptrAccountID(accountID),
			Capability:  capability,
			Status:      core.ParseRunStatus(status),
			StartedAt:   startedAt,
			FinishedAt:  ptrTime(finished),
			CloudGained: ptrI64(cloudGained),
			CloudAfter:  ptrI64(cloudAfter),
			Message:     ptrStr(message),
		}
		out = append(out, rec)
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// run_events
// ─────────────────────────────────────────────────────────────────────────────

// AppendRunEvents 追加运行事件。
func (s *SqliteStore) AppendRunEvents(ctx context.Context, scope core.Scope, runID core.RunID, events []core.Sequenced) error {
	if len(events) == 0 {
		// 空批不校验归属：没有东西可写，报"不属于你"会让一个无害的调用
		// 看起来像越权。但也不开事务 —— 空事务只是白跑一趟。
		return nil
	}
	if err := validateRunInScope(ctx, s.db, scope, runID); err != nil {
		return err
	}

	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return mapSQLite(err)
	}
	defer func() { _ = tx.Rollback() }() // 已提交时是无害的 no-op

	now := formatTime(time.Now())
	for _, sequenced := range events {
		payload, err := serializeEvent(sequenced.Event)
		if err != nil {
			return err
		}
		// seq 是 uint64，SQLite 的 INTEGER 是有符号 64 位。
		// 超过 math.MaxInt64 会回绕成负数 —— 现实中到不了（`Seq` 自己会先饱和），
		// 所以这里直接转换而不是报错：报错会给一个跑不到的分支加一条错误路径。
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO run_events (run_id, seq, kind, payload, created_at)
			 VALUES (?, ?, ?, ?, ?)
			 ON CONFLICT(run_id, seq) DO NOTHING`,
			runID.String(), int64(sequenced.Seq.Value()), sequenced.Event.EventKind(), payload, now,
		); err != nil {
			return mapSQLite(err)
		}
	}
	if err := tx.Commit(); err != nil {
		return mapSQLite(err)
	}
	return nil
}

// RunEventsAfter 读某个 run 的事件，seq 严格大于 after，按 seq 升序。
func (s *SqliteStore) RunEventsAfter(
	ctx context.Context, scope core.Scope, runID core.RunID, after *core.Seq, limit int,
) ([]core.Sequenced, error) {
	if err := validateRunInScope(ctx, s.db, scope, runID); err != nil {
		return nil, err
	}

	// after = nil → 从头开始。序号从 1 起、0 从不出现，
	// 所以用 0 做下界与"没有下界"等价 —— 不必写两种 SQL。
	var from int64
	if after != nil {
		from = int64(after.Value())
	}
	rows, err := s.db.QueryContext(ctx,
		`SELECT seq, payload FROM run_events
		 WHERE run_id = ? AND seq > ?
		 ORDER BY seq ASC LIMIT ?`,
		runID.String(), from, limit)
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.Sequenced{}
	for rows.Next() {
		var (
			seq     int64
			payload string
		)
		if err := rows.Scan(&seq, &payload); err != nil {
			return nil, mapSQLite(err)
		}
		event, err := deserializeEvent(payload)
		if err != nil {
			return nil, err
		}
		// 存的是 seq 本身（不是"seen"那种语义），所以直接按原值还原。
		out = append(out, core.NewSequenced(core.SeqFromStored(seq), event))
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// DeleteRunEvents 删掉某个 run 的全部事件，返回删掉的行数。
func (s *SqliteStore) DeleteRunEvents(ctx context.Context, scope core.Scope, runID core.RunID) (int, error) {
	if err := validateRunInScope(ctx, s.db, scope, runID); err != nil {
		return 0, err
	}
	res, err := s.db.ExecContext(ctx, "DELETE FROM run_events WHERE run_id = ?", runID.String())
	if err != nil {
		return 0, mapSQLite(err)
	}
	affected, err := res.RowsAffected()
	if err != nil {
		return 0, mapSQLite(err)
	}
	return int(affected), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 认领
// ─────────────────────────────────────────────────────────────────────────────

// ClaimUnowned 认领未归属的数据（`owner IS NULL`）：账号、定时任务、组合。
//
// 三张表在**同一个事务**里认领：半途而废会留下"账号回来了、定时任务没回来"
// 这种很难向用户解释的状态，而它偏偏发生在升级路径上（用户第一眼就会看到）。
func (s *SqliteStore) ClaimUnowned(ctx context.Context, userID core.UserID) (core.ClaimedRows, error) {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return core.ClaimedRows{}, mapSQLite(err)
	}
	defer func() { _ = tx.Rollback() }()

	claim := func(table string) (uint64, error) {
		res, err := tx.ExecContext(ctx,
			"UPDATE "+table+" SET owner = ? WHERE owner IS NULL", userID.String())
		if err != nil {
			return 0, mapSQLite(err)
		}
		n, err := res.RowsAffected()
		if err != nil {
			return 0, mapSQLite(err)
		}
		return uint64(n), nil
	}

	accounts, err := claim("accounts")
	if err != nil {
		return core.ClaimedRows{}, err
	}
	schedules, err := claim("schedules")
	if err != nil {
		return core.ClaimedRows{}, err
	}
	pipelines, err := claim("pipelines")
	if err != nil {
		return core.ClaimedRows{}, err
	}
	if err := tx.Commit(); err != nil {
		return core.ClaimedRows{}, mapSQLite(err)
	}
	return core.ClaimedRows{Accounts: accounts, Schedules: schedules, Pipelines: pipelines}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

func boolToInt(b bool) int64 {
	if b {
		return 1
	}
	return 0
}

func userIDArg(u *core.UserID) any {
	if u == nil {
		return nil
	}
	return u.String()
}

func accountIDArg(a *core.AccountID) any {
	if a == nil {
		return nil
	}
	return a.String()
}

func ptrSecret(ns sql.NullString) *core.SecretValue {
	if !ns.Valid {
		return nil
	}
	v := core.NewSecretValue(ns.String)
	return &v
}

func ptrFullPhone(ns sql.NullString) *core.FullPhone {
	if !ns.Valid {
		return nil
	}
	v := core.FullPhone(ns.String)
	return &v
}

func ptrUserDomainID(ns sql.NullString) *core.UserDomainID {
	if !ns.Valid {
		return nil
	}
	v := core.UserDomainID(ns.String)
	return &v
}

func ptrAccountID(ns sql.NullString) *core.AccountID {
	if !ns.Valid {
		return nil
	}
	v := core.NewAccountID(ns.String)
	return &v
}
