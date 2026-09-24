package server

// `GET /api/accounts/{id}/cloud-stats` 的**线格式**：`0` 与 `null` 必须在文本里就分得开。
//
// # 为什么单独钉这一条（而不是"结构体是可空的，那就够了"）
//
// 这一格是总览页"今日增量"的数据源。它有两种"没有数字"：
//   - 该账号今天**第一次**有快照 ⇒ 没有可减的基准（null）；
//   - 今天余额和上次一样 ⇒ 增量确实是 0。
//
// 前者要什么都不做，后者才值得看一眼"为什么没涨"。这条区别在领域层（`*int64`）
// 与存储层（NULL 列）都已经各自钉住了，但**出网那一跳**仍然可以静默丢掉它 ——
// 两种典型的丢法：
//   1. 处理器把 nil 折算成 0（`derefOrZero`）⇒ 前端拿到 `"cloudDiff":0`，界面上
//      一个刚加进来的号被念成"今天没涨"；
//   2. 改成 `omitempty` ⇒ nil 那一格**键都没了**，前端 `row.cloudDiff ?? —` 之类
//      的写法在 `undefined` 上表现又和 0 不同，故障形态随实现漂移。
//
// 所以这里断的是**原始响应文本**：同一份响应里 `"cloudDiff":null` 与 `"cloudDiff":0`
// 都得出现，且解出来的键**必须存在**。反向改坏它：把 routes.go 里那两个指针字段
// 换成解引用给 0，第一条红；加 `omitempty`，第二条红。

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// putSnapshot 用采集侧同一个纯函数算好差值再落库。
//
// ⚠ 刻意**不**手搓 `CloudStats{CloudDiff: &zero}`：那样测的是"我写的数能读回来"，
// 而不是"采集侧真会写出的那两种形状能出网"。
func putSnapshot(t *testing.T, h *harness, scope core.Scope, accountID string, date string, count int64, history []core.CloudStats) core.CloudStats {
	t.Helper()
	stats := core.NextCloudStats(core.NewAccountID(accountID), date, count, history)
	if err := h.store.UpsertCloudStats(context.Background(), scope, &stats); err != nil {
		t.Fatalf("写快照 %s 失败: %v", date, err)
	}
	return stats
}

// statRow 取响应里指定日期的那一行（顺带证明它在）。
func statRow(t *testing.T, body map[string]any, date string) map[string]any {
	t.Helper()
	rows, _ := body["stats"].([]any)
	for _, raw := range rows {
		row, _ := raw.(map[string]any)
		if row["date"] == date {
			return row
		}
	}
	t.Fatalf("响应里没有 %s 那一行，实得 %v", date, body["stats"])
	return nil
}

// mustHaveDiffKey 断"键在"—— 挡 omitempty 那种丢法。
func mustHaveDiffKey(t *testing.T, row map[string]any, key string) {
	t.Helper()
	if _, ok := row[key]; !ok {
		t.Fatalf("响应里缺了键 %q（前端就无法区分 null 与字段不存在），实得 %v", key, row)
	}
}

// mustDiffEqualsZero 断这一格是**数字** 0，而不是 null、也不是字符串 "0"。
func mustDiffEqualsZero(t *testing.T, row map[string]any, key string) {
	t.Helper()
	value, ok := row[key].(json.Number)
	if !ok {
		t.Fatalf("%s 应是数字，实得 %#v", key, row[key])
	}
	if value.String() != "0" {
		t.Fatalf("%s 应是 0，实得 %s", key, value.String())
	}
}

func TestCloudStatsRouteKeepsNullAndZeroApart(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, "acc-1")
	scope := core.ScopeUser(userIDOf(t, h))

	// 09-19：这个号的第一条快照 ⇒ 两个差值都是"没有基准"。
	first := putSnapshot(t, h, scope, "acc-1", "2026-09-19", 8708, nil)
	// 09-20：余额没动 ⇒ 日增是**数** 0。周增同样是 0 —— 历史不足 7 天时基准退化到
	// 最早那一条（就是 09-19），这是 [core.NextCloudStats] 的既有口径。
	second := putSnapshot(t, h, scope, "acc-1", "2026-09-20", 8708, []core.CloudStats{first})
	if second.CloudDiff == nil || *second.CloudDiff != 0 {
		t.Fatalf("前置条件坏了：第二天的日增量应是 0，实得 %v", second.CloudDiff)
	}

	rec := h.do(t, "GET", "/api/accounts/acc-1/cloud-stats?days=7", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("状态码 %d，体 %q", rec.Code, rec.Body.String())
	}
	text := rec.Body.String()

	// 线格式那一层：两种写法必须**同时**出现在同一份响应里。
	if !strings.Contains(text, `"cloudDiff":null`) {
		t.Fatalf("第一次采集那一格没出 null，全文 %q", text)
	}
	if !strings.Contains(text, `"cloudDiff":0`) {
		t.Fatalf("增量为 0 那一格没出 0，全文 %q", text)
	}

	body := decode(t, rec)
	dayOne := statRow(t, body, "2026-09-19")
	mustHaveDiffKey(t, dayOne, "cloudDiff")
	mustHaveDiffKey(t, dayOne, "cloudDiffWeek")
	if dayOne["cloudDiff"] != nil || dayOne["cloudDiffWeek"] != nil {
		t.Fatalf("09-19 是第一天，两格都该是 null，实得 %v", dayOne)
	}

	dayTwo := statRow(t, body, "2026-09-20")
	mustHaveDiffKey(t, dayTwo, "cloudDiff")
	mustDiffEqualsZero(t, dayTwo, "cloudDiff")
	mustDiffEqualsZero(t, dayTwo, "cloudDiffWeek")
	// 余额那一格不能被牵连：它一直是整数。
	if n, _ := dayTwo["cloudCount"].(json.Number); n.String() != "8708" {
		t.Fatalf("余额应原样是 8708，实得 %#v", dayTwo["cloudCount"])
	}
}

func TestCloudStatsRouteOnAccountWithSingleSnapshotSendsOnlyNull(t *testing.T) {
	// 2026-09-24 的实况形状：库里有且只有今天这一条。
	// 界面上那一格要显示 `—`，前提是这一跳交出去的是 null 而不是 0。
	h := newHarness(t).initialized(t)
	seedAccount(t, h, "acc-2")
	scope := core.ScopeUser(userIDOf(t, h))
	putSnapshot(t, h, scope, "acc-2", "2026-09-20", 7440, nil)

	rec := h.do(t, "GET", "/api/accounts/acc-2/cloud-stats", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("状态码 %d，体 %q", rec.Code, rec.Body.String())
	}
	text := rec.Body.String()
	if !strings.Contains(text, `"cloudDiff":null`) || !strings.Contains(text, `"cloudDiffWeek":null`) {
		t.Fatalf("单条快照的号两格增量都该是 null，全文 %q", text)
	}
	if strings.Contains(text, `"cloudDiff":0`) {
		t.Fatalf("不许把「没有基准」折算成 0，全文 %q", text)
	}
}
