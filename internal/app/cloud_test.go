package app

import (
	"context"
	"errors"
	"sort"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// cloudStore 用内嵌接口做部分假实现，并**只**实现本层该碰的那三个入口。
// 没重写的方法一调用就 panic —— "余额采集碰了别的存储入口"会当场响。
type cloudStore struct {
	core.StateStore

	stats        map[string]core.CloudStats // date -> 快照
	current      *int64
	updatedAt    *time.Time
	writes       []string // 写入顺序：先 "stats" 后 "current"
	failStats    bool
	failCurrent  bool
	sinceErr     error
	notFoundFor  bool // 模拟"账号不属于这个 scope"
	lastSinceArg string
	// attempts 记录**尝试过**的写入（不管成没成）。
	// ⚠ 它与 writes 不是一回事：越权那条要证明的是"一次都没试"，
	// 而失败的写入不会进 writes —— 只看 writes 的话，
	// "忽略了错误照写"这种改法查不出来（已实测）。
	attempts []string
}

func newCloudStore() *cloudStore {
	return &cloudStore{stats: make(map[string]core.CloudStats)}
}

func (s *cloudStore) CloudStatsSince(_ context.Context, _ core.Scope, _ core.AccountID, since string) ([]core.CloudStats, error) {
	s.lastSinceArg = since
	if s.sinceErr != nil {
		return nil, s.sinceErr
	}
	out := make([]core.CloudStats, 0, len(s.stats))
	for _, v := range s.stats {
		out = append(out, v)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Date < out[j].Date })
	return out, nil
}

func (s *cloudStore) UpsertCloudStats(_ context.Context, _ core.Scope, stats *core.CloudStats) error {
	s.attempts = append(s.attempts, "stats")
	if s.notFoundFor {
		return core.NewNotFound("账号不属于这个用户")
	}
	if s.failStats {
		return errors.New("日快照写入失败")
	}
	s.writes = append(s.writes, "stats")
	s.stats[stats.Date] = *stats
	return nil
}

func (s *cloudStore) SetAccountCloud(_ context.Context, _ core.Scope, _ core.AccountID, count int64, at time.Time) error {
	s.attempts = append(s.attempts, "current")
	if s.notFoundFor {
		return core.NewNotFound("账号不属于这个用户")
	}
	if s.failCurrent {
		return errors.New("当前余额写入失败")
	}
	s.writes = append(s.writes, "current")
	s.current = &count
	s.updatedAt = &at
	return nil
}

func (s *cloudStore) seed(date string, count int64) {
	s.stats[date] = core.CloudStats{Date: date, CloudCount: count}
}

// cloudClock 北京时间 2026-09-19 14:00 —— `DailyKey` 给 `2026-09-19`。
func cloudClock() core.Clock {
	return fixedClock{at: time.Date(2026, 9, 19, 6, 0, 0, 0, time.UTC)}
}

var cloudAccount = core.NewAccountID("199****0001")

func TestFirstCollectionRecordsTheValueAndReportsNoDelta(t *testing.T) {
	store := newCloudStore()
	stats, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if stats.Date != "2026-09-19" {
		t.Errorf("日键应按东八区算，实得 %q", stats.Date)
	}
	if stats.CloudCount != 5392 {
		t.Errorf("实得 %d", stats.CloudCount)
	}
	if stats.CloudDiff != 0 {
		t.Errorf("首次采集不该虚报增量，实得 %d", stats.CloudDiff)
	}
	if stats.CloudDiffWeek != 0 {
		t.Errorf("实得 %d", stats.CloudDiffWeek)
	}
	if store.current == nil || *store.current != 5392 {
		t.Errorf("当前值应被写入，实得 %v", store.current)
	}
	if store.updatedAt == nil || !store.updatedAt.Equal(cloudClock().Now()) {
		t.Errorf("采集时间应取自**注入的时钟**，实得 %v", store.updatedAt)
	}
}

func TestDailyDeltaIsMeasuredAgainstThePreviousSnapshot(t *testing.T) {
	store := newCloudStore()
	store.seed("2026-09-18", 4200)
	stats, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if stats.CloudDiff != 1192 {
		t.Errorf("实得 %d", stats.CloudDiff)
	}
	if stats.CloudDiffWeek != 1192 {
		t.Errorf("历史不足 7 天时周增量退化为最早一条之差，实得 %d", stats.CloudDiffWeek)
	}
}

// TestSecondCollectionOnTheSameDayIsIdempotent：
// 同一天重复采集**不能把今天的行当成自己的基准** —— 那样 `cloud_diff` 恒为 0。
func TestSecondCollectionOnTheSameDayIsIdempotent(t *testing.T) {
	store := newCloudStore()
	store.seed("2026-09-18", 4200)

	if _, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5000, cloudClock()); perr != nil {
		t.Fatalf("%v", perr)
	}
	second, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if second.CloudDiff != 1192 {
		t.Errorf("基准仍应是昨天那条 4200，而不是今天早些时候那条 5000，实得 %d", second.CloudDiff)
	}
	if len(store.stats) != 2 {
		t.Errorf("09-18 与 09-19 各一行，当天是 UPSERT 不是追加，实得 %d 行", len(store.stats))
	}
	if store.stats["2026-09-19"].CloudCount != 5392 {
		t.Errorf("当天那行应被覆盖，实得 %d", store.stats["2026-09-19"].CloudCount)
	}
}

// TestLongGapDoesNotSilentlyReportZeroGrowth：
// 一个两周没采集过的账号：两条基准都在"近 7 天"之外。
// 窗口取窄了这里会算成 0 —— 把"涨了 300"写成"没变化"。
func TestLongGapDoesNotSilentlyReportZeroGrowth(t *testing.T) {
	store := newCloudStore()
	store.seed("2026-09-01", 5092)
	stats, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if stats.CloudDiff != 300 {
		t.Errorf("实得 %d", stats.CloudDiff)
	}
	if stats.CloudDiffWeek != 300 {
		t.Errorf("实得 %d", stats.CloudDiffWeek)
	}
	// 窗口必须是"取全量"而不是"近 7 天"
	if store.lastSinceArg != HistoryFrom {
		t.Errorf("历史下界应是 %q，实得 %q", HistoryFrom, store.lastSinceArg)
	}
}

// TestHistoryBeforeTodayStillCountsWhenItPredatesTheWeek：
// 今天**之前**的历史一律参与计算 —— 滤掉的只有今天。
func TestHistoryBeforeTodayStillCountsWhenItPredatesTheWeek(t *testing.T) {
	store := newCloudStore()
	store.seed("2026-09-18", 4200)
	store.seed("2026-09-19", 5000) // 今天（由上一次采集写入）
	stats, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if stats.CloudDiff != 1192 {
		t.Errorf("今天那行必须被排除在基准之外，实得 %d", stats.CloudDiff)
	}
}

// TestWriteOrderIsStatsThenCurrent：
// 先写历史再写当前值。反过来的窗口里丢的是**永久少一格**的累积数据。
func TestWriteOrderIsStatsThenCurrent(t *testing.T) {
	store := newCloudStore()
	if _, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock()); perr != nil {
		t.Fatalf("%v", perr)
	}
	if len(store.writes) != 2 || store.writes[0] != "stats" || store.writes[1] != "current" {
		t.Errorf("写入顺序应是 [stats, current]，实得 %v", store.writes)
	}
}

// TestCurrentWriteFailureStillHandsBackTheSnapshot：
// 日快照**已经写进去了**，所以失败也要把 stats 交回去 ——
// 返回空快照会让调用方以为"什么都没记"，于是再采一次，
// 而第二次的基准变成了刚才那条今天的行，`cloud_diff` 就此恒为 0。
func TestCurrentWriteFailureStillHandsBackTheSnapshot(t *testing.T) {
	store := newCloudStore()
	store.failCurrent = true
	stats, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr == nil {
		t.Fatal("应报错")
	}
	if stats.CloudCount != 5392 || stats.Date != "2026-09-19" {
		t.Errorf("快照已落库，就该把它交回去，实得 %+v", stats)
	}
	if len(store.writes) != 1 || store.writes[0] != "stats" {
		t.Errorf("应只写了日快照，实得 %v", store.writes)
	}
}

// TestAForeignAccountIsRefusedBeforeAnythingIsWritten：
// 越权采集必须在**写之前**被拒 —— 留下日快照等于把别人的数据写进了自己的趋势图。
func TestAForeignAccountIsRefusedBeforeAnythingIsWritten(t *testing.T) {
	store := newCloudStore()
	store.notFoundFor = true
	_, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock())
	if perr == nil {
		t.Fatal("应被拒")
	}
	if perr.Kind() != core.KindNotFound {
		t.Errorf("应是 NotFound，实得 %v", perr.Kind())
	}
	// 被拒就**停在第一次写**：不许"日快照失败无所谓、接着把当前值写了"。
	// 一次都没试（`attempts` 为空）也不对 —— 那意味着这一层在写之前
	// 自己查了一遍归属，而"这个 scope 看不看得见这个账号"是存储层的判据。
	if len(store.attempts) != 1 || store.attempts[0] != "stats" {
		t.Errorf("应只尝试过一次日快照写入就停下，实得 %v", store.attempts)
	}
	if len(store.writes) != 0 {
		t.Errorf("被拒的采集不该留下任何写入，实得 %v", store.writes)
	}
}

// TestHistoryReadFailureIsNotSwallowed：
// 读历史失败若被当成"没有历史"，就会按首次采集记 0 ——
// 把"这段时间涨了 300"静默写成"没变化"。
func TestHistoryReadFailureIsNotSwallowed(t *testing.T) {
	store := newCloudStore()
	store.seed("2026-09-18", 4200)
	store.sinceErr = errors.New("磁盘只读")
	if _, perr := RecordCloud(context.Background(), store, core.ScopeLocal(),
		cloudAccount, 5392, cloudClock()); perr == nil {
		t.Fatal("读历史失败必须报出来")
	}
	if len(store.writes) != 0 {
		t.Errorf("读不到历史时不该写任何东西，实得 %v", store.writes)
	}
}
