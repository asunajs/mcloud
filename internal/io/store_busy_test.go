package mcloudio

import (
	"context"
	"path/filepath"
	"testing"
	"time"
)

// TestSameFileFromTwoHandlesSurvivesLockContention 钉的是 DSN 里的
// `busy_timeout(5000)` —— 它管的是**撞锁要等多久**，而不是"会不会撞锁"。
//
// 场景是跨进程现实的进程内等价物：同一个库文件被两个独立连接池打开
// （CLI 与 server 各持一份是日常），A 持有写锁时 B 的写落在 SQLite 的
// busy 处理上。本仓库此前只靠 `MaxOpenConns(1)` 串行化**本进程**，
// 跨进程一撞就是裸的 `SQLITE_BUSY`（"database is locked"）。
//
// # 反向改坏它，谁会红
//
// 把 store.go 的 Open 里 `_pragma=busy_timeout(5000)` 删掉再跑：
// B 的写在锁被占的瞬间**立即**收到 "database is locked"（SQLite 的
// 忙等默认是 0），本条断言"没有报错"就会红。
// 碰撞是构造出来的、必然发生：A 在 B 发起写之后仍持锁 200ms，
// 不存在"B 其实排在提交之后、没撞锁也绿"的假绿路径。
func TestSameFileFromTwoHandlesSurvivesLockContention(t *testing.T) {
	path := filepath.Join(t.TempDir(), "busy.db")

	holder, err := Open(path)
	if err != nil {
		t.Fatalf("打开持有锁的库失败: %v", err)
	}
	defer func() { _ = holder.Close() }()

	waiter, err := Open(path)
	if err != nil {
		t.Fatalf("打开第二个句柄失败: %v", err)
	}
	defer func() { _ = waiter.Close() }()

	ctx := context.Background()

	// BEGIN 是 deferred 的，锁在**第一条写语句**才真正落下来 —— 所以必须
	// 先插一行，再让第二个句柄去撞。
	tx, err := holder.db.BeginTx(ctx, nil)
	if err != nil {
		t.Fatalf("持有方开启事务失败: %v", err)
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO runs (id, capability, status, started_at) VALUES ('holder', 'sign', 'Running', '2026-09-22T00:00:00Z')`); err != nil {
		t.Fatalf("持有方写入失败: %v", err)
	}

	writeErr := make(chan error, 1)
	go func() {
		_, err := waiter.db.ExecContext(ctx,
			`INSERT INTO runs (id, capability, status, started_at) VALUES ('waiter', 'sign', 'Running', '2026-09-22T00:00:01Z')`)
		writeErr <- err
	}()

	// 持锁 200ms：保证第二个句柄的写**必然**落在锁上（而不是排在提交之后白跑）。
	time.Sleep(200 * time.Millisecond)
	if err := tx.Commit(); err != nil {
		t.Fatalf("持有方提交失败: %v", err)
	}

	select {
	case err := <-writeErr:
		if err != nil {
			t.Fatalf("第二个句柄的写报了错 —— busy_timeout 没生效（或被改小）: %v", err)
		}
	case <-time.After(6 * time.Second):
		// 上限 6s > 5s：busy_timeout 自己到点时会带错误回来，走到这里是挂死。
		t.Fatal("第二个句柄的写 6s 未完成 —— 等待逻辑挂了")
	}
}
