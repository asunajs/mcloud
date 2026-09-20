package mcloudio

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/catlair/mcloud/internal/core"
	sqlite "modernc.org/sqlite"
	sqlite3 "modernc.org/sqlite/lib"
)

// 系统用户与会话的 SQLite 实现（schema v3）。
//
// # 两条**不**放进这一层的判断
//
//   - **口令校验不在存储层**：这里只按用户名取回 `password_hash`，
//     校验由 [core.VerifyPassword] 做（纯计算、可单测）；
//   - **"现在几点"不在存储层**：会话是否有效由调用方传时刻判断
//     （[core.Session.IsValidAt]），清理过期会话也要显式传 `now`。
//     存储实现偷偷读时钟，会让"到期那一刻算不算有效"变成不可测的。

// mapUserWrite 把"用户名被占用"翻译成可读的错误。
//
// 把 SQLite 的 `UNIQUE constraint failed: users.name` 直接抛给用户，
// 既看不懂也不该看到表结构；这里只保留"名字被占用"这个结论。
//
// ⚠ 判据必须看**错误码**而不是匹配错误文本：文本里带着表名与列名，
// 一旦 schema 改了列名，匹配就会静默失效 —— 而失效的表现是
// "本该是 BadRequest 的变成 Internal"，不会有人注意到。
func mapUserWrite(err error) *core.Error {
	var se *sqlite.Error
	if errors.As(err, &se) && se.Code()&0xff == sqlite3.SQLITE_CONSTRAINT {
		return core.NewBadRequest("用户名已被占用")
	}
	return mapSQLite(err)
}

const userColumns = "id, name, password_hash, role, created_at"

func scanUser(s rowScanner) (core.User, error) {
	var id, name, passwordHash, role, createdAt string
	if err := s.Scan(&id, &name, &passwordHash, &role, &createdAt); err != nil {
		return core.User{}, err
	}
	t, err := requiredTime(createdAt)
	if err != nil {
		return core.User{}, err
	}
	return core.NewUser(id, name, core.PasswordHash(passwordHash), core.ParseRole(role), t), nil
}

// UpsertUser 新建或覆盖用户。见 [core.UserStore.UpsertUser]。
//
// ⚠ `created_at` **不在 UPDATE 列里**：它记的是"何时创建"，
// 覆盖写把它一起改了，会让"最早的用户"这种判断随机变化。
func (s *SqliteStore) UpsertUser(ctx context.Context, user *core.User) error {
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO users (id, name, password_hash, role, created_at)
		VALUES (?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET
			name          = excluded.name,
			password_hash = excluded.password_hash,
			role          = excluded.role
		`,
		user.ID.String(),
		user.Name,
		string(user.PasswordHash),
		user.Role.String(),
		formatTime(user.CreatedAt),
	)
	if err != nil {
		return mapUserWrite(err)
	}
	return nil
}

// GetUserByName 按用户名查（登录路径）。大小写不敏感。
func (s *SqliteStore) GetUserByName(ctx context.Context, name string) (*core.User, error) {
	// 列上的 `COLLATE NOCASE` 让这个 `=` 天然就是大小写不敏感的
	u, err := scanUser(s.db.QueryRowContext(ctx,
		"SELECT "+userColumns+" FROM users WHERE name = ?", name))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &u, nil
}

// GetUser 按 id 查。
func (s *SqliteStore) GetUser(ctx context.Context, id core.UserID) (*core.User, error) {
	u, err := scanUser(s.db.QueryRowContext(ctx,
		"SELECT "+userColumns+" FROM users WHERE id = ?", id.String()))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	return &u, nil
}

// ListUsers 按创建时间排序的稳定列表。
func (s *SqliteStore) ListUsers(ctx context.Context) ([]core.User, error) {
	rows, err := s.db.QueryContext(ctx,
		"SELECT "+userColumns+" FROM users ORDER BY created_at ASC, id ASC")
	if err != nil {
		return nil, mapSQLite(err)
	}
	defer func() { _ = rows.Close() }()

	out := []core.User{}
	for rows.Next() {
		u, err := scanUser(rows)
		if err != nil {
			return nil, mapSQLite(err)
		}
		out = append(out, u)
	}
	if err := rows.Err(); err != nil {
		return nil, mapSQLite(err)
	}
	return out, nil
}

// CountUsers 用户总数 —— 它是"是否已完成初始化"的判据。
func (s *SqliteStore) CountUsers(ctx context.Context) (uint64, error) {
	var n int64
	if err := s.db.QueryRowContext(ctx, "SELECT COUNT(*) FROM users").Scan(&n); err != nil {
		return 0, mapSQLite(err)
	}
	return uint64(n), nil
}

// InsertSession 写入会话。
//
// ⚠ **不做 UPSERT**：`token_hash` 是 256 位随机值的哈希，撞上同一个值
// 只可能是调用方拿旧 token 重复签发 —— 那是错误用法，报出来比悄悄覆盖好。
func (s *SqliteStore) InsertSession(ctx context.Context, session *core.Session) error {
	_, err := s.db.ExecContext(ctx,
		"INSERT INTO sessions (token_hash, user_id, expires_at, kind) VALUES (?, ?, ?, ?)",
		session.TokenHash,
		session.UserID.String(),
		formatTime(session.ExpiresAt),
		session.Kind.String(),
	)
	if err != nil {
		return mapSQLite(err)
	}
	return nil
}

// GetSession 按 token 哈希查会话。
func (s *SqliteStore) GetSession(ctx context.Context, tokenHash string) (*core.Session, error) {
	var (
		hash, userID, expiresAt, kind string
	)
	err := s.db.QueryRowContext(ctx,
		"SELECT token_hash, user_id, expires_at, kind FROM sessions WHERE token_hash = ?",
		tokenHash,
	).Scan(&hash, &userID, &expiresAt, &kind)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, mapSQLite(err)
	}
	exp, err := requiredTime(expiresAt)
	if err != nil {
		return nil, err
	}
	sess := core.NewSession(hash, core.NewUserID(userID), exp, core.ParseSessionKind(kind))
	return &sess, nil
}

// DeleteSession 删除单个会话（登出）。返回是否真的删掉了一行。
func (s *SqliteStore) DeleteSession(ctx context.Context, tokenHash string) (bool, error) {
	res, err := s.db.ExecContext(ctx, "DELETE FROM sessions WHERE token_hash = ?", tokenHash)
	if err != nil {
		return false, mapSQLite(err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, mapSQLite(err)
	}
	return n > 0, nil
}

// DeleteExpiredSessions 清理已过期会话，返回删除条数。
//
// ⚠ 字符串比较成立的前提：写入与比较**都走** RFC 3339 + UTC，
// 此时字典序等价于时间序。任一侧改成本地时间或另一种布局，这条就会静默错。
//
// ⚠ 边界是 `<=`：与 [core.Session.IsExpiredAt] 的"到期时刻本身算过期"一致。
// 两边不一致会出现"清理没删、但校验说无效"的窗口。
func (s *SqliteStore) DeleteExpiredSessions(ctx context.Context, now time.Time) (uint64, error) {
	res, err := s.db.ExecContext(ctx, "DELETE FROM sessions WHERE expires_at <= ?",
		formatTime(now))
	if err != nil {
		return 0, mapSQLite(err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return 0, mapSQLite(err)
	}
	return uint64(n), nil
}

// DeleteUserSessions 删除某用户的全部会话（改口令 / 删用户时用）。
func (s *SqliteStore) DeleteUserSessions(ctx context.Context, userID core.UserID) (uint64, error) {
	res, err := s.db.ExecContext(ctx, "DELETE FROM sessions WHERE user_id = ?", userID.String())
	if err != nil {
		return 0, mapSQLite(err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return 0, mapSQLite(err)
	}
	return uint64(n), nil
}

// IssueSession 签发会话：生成明文 token → 落库**哈希** → 返回明文。
//
// 做成一个函数而不是让调用方自己拼 [core.NewSessionToken] + [core.HashSessionToken]：
// **漏掉哈希就是明文入库**，而这种事既不会编译失败也不会运行报错。
//
// 返回的 `token` 是明文，**只此一次**；调用方负责把它交给用户而不是写进日志。
func IssueSession(
	ctx context.Context, store core.UserStore, userID core.UserID,
	expiresAt time.Time, kind core.SessionKind, rng core.Random,
) (string, core.Session, error) {
	token := core.NewSessionToken(rng)
	session := core.NewSession(core.HashSessionToken(token), userID, expiresAt, kind)
	if err := store.InsertSession(ctx, &session); err != nil {
		return "", core.Session{}, err
	}
	return token, session, nil
}

// 让"实现了接口"这件事在编译期就有断言 —— 少一个方法时是**这里**报错，
// 而不是等到组装根里某处赋值失败才发现。
//
// ⚠ `ConfigStore` 不在这里：它由 store/config.go 实现（另一个模块），
// 现在就断言会让本文件编译不过。
var (
	_ core.StateStore = (*SqliteStore)(nil)
	_ core.UserStore  = (*SqliteStore)(nil)
)
