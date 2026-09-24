package app

// [CloudSnapshotRun] 的用例：把 `infoV3.total` 记成一条日快照。
//
// 这一条链上唯一**不能反**的两件事：
//
//  1. 取的数是 `infoV3.total` —— 与签到链上记的**同一个数**，所以两条链同一天各跑一次
//     不会打架（[RecordCloud] 同日 UPSERT）。换成 `beforeTotal` 之类的表现是
//     "趋势图与列表页的余额对不上"。
//  2. **写库失败交回错误** —— 与签到那条链刻意相反：那里余额是顺带采集的副产品，
//     这里写库就是目标本身。
//
// 增量怎么算（`cloud_diff` 相对"今天之前"）由 [RecordCloud] 自己的用例钉在 `cloud_test.go`，
// 这里不重复。

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// snapshotRunStore 只补这一条链会碰的那几个存储入口，其余仍然"没重写就 panic"。
type snapshotRunStore struct {
	fakeStore

	stats      []core.CloudStats
	current    *int64
	currentSet bool
	failWrite  error
}

func (s *snapshotRunStore) GetSmidState(
	context.Context, core.Scope, core.AccountID,
) (*core.SmidState, error) {
	return nil, errors.New("本用例不需要 deviceId")
}

func (s *snapshotRunStore) CloudStatsSince(
	context.Context, core.Scope, core.AccountID, string,
) ([]core.CloudStats, error) {
	return append([]core.CloudStats{}, s.stats...), nil
}

func (s *snapshotRunStore) UpsertCloudStats(
	_ context.Context, _ core.Scope, stats *core.CloudStats,
) error {
	if s.failWrite != nil {
		return s.failWrite
	}
	s.stats = append(s.stats, *stats)
	return nil
}

func (s *snapshotRunStore) SetAccountCloud(
	_ context.Context, _ core.Scope, _ core.AccountID, count int64, _ time.Time,
) error {
	if s.failWrite != nil {
		return s.failWrite
	}
	s.current, s.currentSet = &count, true
	return nil
}

// snapshotScript 给一份"建会话三枪 + 一条 `infoV3`"的脚本。
//
// ⚠ `infoV3` 必须走 [statusPayload]：那四个整数字段是协议层硬要求的，
// 少任一个的话本用例红在"解析失败"上，而想测的数压根没取到。
func snapshotScript(body string) *liveTaskScript {
	return liveTaskScriptWith(append(sessionRoutes(),
		liveTaskRoute{"/ycloud/signin/page/infoV3", []string{body}})...)
}

// TestCloudSnapshotRunWritesTheServerReportedBalance：记的就是 `infoV3.total`。
//
// `statusPayload` 给的 total 是 10496 —— 换成签到链上那个数以外的任何东西都会红。
func TestCloudSnapshotRunWritesTheServerReportedBalance(t *testing.T) {
	store := &snapshotRunStore{fakeStore: *liveTaskStore()}
	script := snapshotScript(statusPayload(`"cal":[]`))
	run := CloudSnapshotRun{SignInWorld: signInWorldFor(store, script)}

	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("采集失败：%v", perr)
	}
	if res.Count != 10496 {
		t.Errorf("应取 `infoV3.total`，实得 %d", res.Count)
	}
	if res.Stats == nil {
		t.Fatal("写进去的那条快照要交回调用方（小结里的增量从它念）")
	}
	if want := core.DailyKey(testClock().Now()); res.Stats.Date != want {
		t.Errorf("快照日期按本地时区切，实得 %q 期望 %q", res.Stats.Date, want)
	}
	if len(store.stats) != 1 || store.stats[0].CloudCount != 10496 {
		t.Errorf("库里应有这一条，实得 %+v", store.stats)
	}
	if !store.currentSet || *store.current != 10496 {
		t.Errorf("账号当前余额也要同步，实得 %v", store.current)
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
		t.Errorf("实得 %v", report.Outcome)
	}
	if !strings.Contains(summaryOf(res.RunReport()), "余额 10496 豆") {
		t.Errorf("小结要念出余额，实得 %q", summaryOf(res.RunReport()))
	}
}

// TestCloudSnapshotRunReturnsStoreError：写不进去就是失败，而且**交回错误**。
//
// 反向改法（把 cerr 吞掉、只报成功）的表现是"历史列表绿、趋势图缺今天这一格"，
// 而缺的那一格下次采集不会补 —— 所以这里必须是错误。
func TestCloudSnapshotRunReturnsStoreError(t *testing.T) {
	store := &snapshotRunStore{
		fakeStore: *liveTaskStore(),
		failWrite: errors.New("库是只读的"),
	}
	run := CloudSnapshotRun{SignInWorld: signInWorldFor(store, snapshotScript(statusPayload(`"cal":[]`)))}
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatalf("写库失败必须交回错误，实得 %+v", res)
	}
	if !strings.Contains(perr.Error(), "库是只读的") {
		t.Errorf("要带上库里那句话，实得 %q", perr.Error())
	}
	if res.Stats != nil {
		t.Errorf("没写进去就不该交回一条快照，实得 %+v", res.Stats)
	}
	// 数仍然带回去：排查时"取到了多少"与"为什么没记上"是两条信息。
	if res.Count != 10496 {
		t.Errorf("余额原值仍要带回去，实得 %d", res.Count)
	}
	if got := summaryOf(res.RunReport()); got != "余额 10496 豆" {
		t.Errorf("Stats 为 nil 时只报余额，不该编出增量，实得 %q", got)
	}
}

// TestCloudSnapshotRunReturnsErrorWhenStatusFails：连数都没取到 ⇒ 错误。
func TestCloudSnapshotRunReturnsErrorWhenStatusFails(t *testing.T) {
	script := liveTaskScriptWith(append(sessionRoutes(), liveTaskRoute{
		"/ycloud/signin/page/infoV3", []string{`{"code":1,"msg":"系统繁忙"}`},
	})...)
	store := &snapshotRunStore{fakeStore: *liveTaskStore()}
	run := CloudSnapshotRun{SignInWorld: signInWorldFor(store, script)}
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("取不到数就该交回错误")
	}
	if len(store.stats) != 0 {
		t.Errorf("没取到数不该写库，实得 %+v", store.stats)
	}
}
