package mcloudio

import (
	"context"
	"reflect"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// 调度循环那两条窄写（`MarkScheduleFire` / `RecordScheduleRun`）。
//
// 两条的**失败形状都不是报错**，而是安静地把别的数据抹掉：
//
//   - 写成 `UpsertSchedule`（整行覆盖）→ 一次触发把 WebUI 上刚编辑的 cron 打回
//     几秒前读到的旧值。症状是"我改了时间它没理"，而库里那一行确实是被循环改回去的。
//   - 循环用 `last_run_at` 当水位 → 按 [core.MissSkip] 弃掉的错过在每个 tick
//     被重新捡起，表现成"每刷一次页面多一条 skipped"。
//   - `UpsertSchedule` 跟着覆盖 `last_fire_at` → 编辑一行定时任务把水位倒回去，
//     于是**已处理过的那一次再跑一遍**，而那一遍带真上游请求（签到与换凭据都不可逆）。
//
// 所以断言全部盯着"不该动的列有没有被动"。

// seededSchedule 是一条"每一列都有值"的定时任务：窄写写错成整行覆盖时，
// 那些列会变成 NULL，而"NULL 还是原值"只有原先有值才看得出来。
func seededSchedule(t *testing.T, s *SqliteStore, id string) core.Schedule {
	t.Helper()
	sc := configSchedule(id)
	run := time.Date(2026, 9, 20, 7, 30, 0, 0, time.UTC)
	status := core.ScheduleOK
	msg := "上次的旧错误"
	miss := core.MissCatchUp
	fire := time.Date(2026, 9, 20, 7, 30, 0, 0, time.UTC)
	sc.LastRunAt = &run
	sc.LastStatus = &status
	sc.LastError = &msg
	sc.MissPolicy = &miss
	sc.LastFireAt = &fire
	if err := s.UpsertSchedule(context.Background(), core.ScopeLocal(), &sc); err != nil {
		t.Fatalf("建定时任务: %v", err)
	}
	return sc
}

// loadedOrFatal 读回整行，用来跟"写入前那一份"整体比对 —— 逐列断言漏一列看不出来。
func loadedOrFatal(t *testing.T, s *SqliteStore, id string) core.Schedule {
	t.Helper()
	loaded, err := s.GetSchedule(context.Background(), core.ScopeLocal(), id)
	if err != nil || loaded == nil {
		t.Fatalf("读定时任务 %s: %v / nil?", id, err)
	}
	return *loaded
}

func TestMarkScheduleFireOnlyTouchesTheWaterColumn(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	before := seededSchedule(t, s, "sched-1")

	ok, err := s.MarkScheduleFire(ctx, core.ScopeLocal(), "sched-1",
		time.Date(2026, 9, 21, 7, 30, 0, 0, time.UTC))
	if err != nil || !ok {
		t.Fatalf("推水位: %v / 写到 0 行", err)
	}

	after := loadedOrFatal(t, s, "sched-1")
	newFire := time.Date(2026, 9, 21, 7, 30, 0, 0, time.UTC)
	want := before
	want.LastFireAt = &newFire
	if !reflect.DeepEqual(after, want) {
		t.Fatalf("只该动 last_fire_at：\n实得 %+v\n期望 %+v", after, want)
	}
	// 反向改坏：把这条窄写换成 UpsertSchedule(整行)，上面那句就会红 ——
	// 而那句红的同时 `last_run_at`/`last_status`/`body` 也全被抹回调用方手上那份旧数据。
	if after.LastStatus == nil || *after.LastStatus != core.ScheduleOK {
		t.Errorf("last_status 被卷进来了：%v", after.LastStatus)
	}
	if after.LastError == nil || *after.LastError != "上次的旧错误" {
		t.Errorf("last_error 被卷进来了：%v", after.LastError)
	}
	if after.Cron != before.Cron || after.Body == nil || string(after.Body) != string(before.Body) {
		t.Errorf("定义（cron/body）被卷进来了：%q / %v", after.Cron, string(after.Body))
	}
}

func TestRecordScheduleRunOnlyTouchesTheAccountingColumns(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	before := seededSchedule(t, s, "sched-1")

	ran := time.Date(2026, 9, 22, 7, 30, 0, 0, time.UTC)
	errMsg := "上游 500"
	ok, err := s.RecordScheduleRun(ctx, core.ScopeLocal(), "sched-1",
		&ran, core.ScheduleError, &errMsg)
	if err != nil || !ok {
		t.Fatalf("写记账: %v / 写到 0 行", err)
	}

	after := loadedOrFatal(t, s, "sched-1")
	want := before
	statusErr := core.ScheduleError
	want.LastRunAt = &ran
	want.LastStatus = &statusErr
	want.LastError = &errMsg
	// `last_status` 从 ok 变成 error，其余一律不动 —— 尤其 **水位不动**：
	// 记账与择时是两件事，写"跑成什么样"绝不该顺手把"跑到哪一刻了"挪走。
	if !reflect.DeepEqual(after, want) {
		t.Fatalf("只该动 last_run_at / last_status / last_error：\n实得 %+v\n期望 %+v", after, want)
	}
	if after.LastStatus == nil || *after.LastStatus != core.ScheduleError {
		t.Fatalf("last_status 没写成 error：%v", after.LastStatus)
	}
}

// TestRecordScheduleRunWithNilRanAtKeepsThePreviousOne 钉住 nil 的第一层含义。
//
// 按 [core.MissSkip] 弃掉的错过、以及撞锁被挡下的那一趟**都没跑**：
// 它们要留下 `skipped` 与一句原因，但"上次真跑是哪一刻"不能被抹掉 ——
// 抹掉之后界面上这个任务看起来"从来不执行"，而排查的人会去查 cron 表达式。
func TestRecordScheduleRunWithNilRanAtKeepsThePreviousOne(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	before := seededSchedule(t, s, "sched-1")

	reason := "上一次还在跑，本次不排队"
	ok, err := s.RecordScheduleRun(ctx, core.ScopeLocal(), "sched-1",
		nil, core.ScheduleSkipped, &reason)
	if err != nil || !ok {
		t.Fatalf("写 skipped: %v / 写到 0 行", err)
	}

	after := loadedOrFatal(t, s, "sched-1")
	if after.LastRunAt == nil || !after.LastRunAt.Equal(*before.LastRunAt) {
		t.Errorf("没跑的这一趟把上次运行时刻抹了：%v", after.LastRunAt)
	}
	if after.LastStatus == nil || *after.LastStatus != core.ScheduleSkipped {
		t.Errorf("skipped 该留下状态：%v", after.LastStatus)
	}
}

// TestRecordScheduleRunClearsLastErrorWhenTheRerunIsClean 钉住 nil 的第二层含义，
// 以及它与上面那条**刻意不对称**。
//
// `lastError == nil` 是"这次跑干净了"，而清空它本身就是结论：留着的话
// `last_status = ok` 配一句旧错误在界面上并排出现，那正是查不出原因的形态。
// 反过来 `ranAt == nil` 是"这一趟没跑"，没有哪一刻该被写进那一列，所以宁可不写。
func TestRecordScheduleRunClearsLastErrorWhenTheRerunIsClean(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	seededSchedule(t, s, "sched-1")

	ran := time.Date(2026, 9, 22, 7, 30, 0, 0, time.UTC)
	if _, err := s.RecordScheduleRun(ctx, core.ScopeLocal(), "sched-1",
		&ran, core.ScheduleOK, nil); err != nil {
		t.Fatalf("写一次干净的执行: %v", err)
	}

	after := loadedOrFatal(t, s, "sched-1")
	if after.LastError != nil {
		t.Errorf("跑干净了该把旧错误清成 NULL，实得 %q", *after.LastError)
	}
	if after.LastRunAt == nil || !after.LastRunAt.Equal(ran) {
		t.Errorf("跑过了，last_run_at 该是新时刻：%v", after.LastRunAt)
	}
}

// TestUpsertScheduleNeverRewindsTheWaterColumn 是这一版最要紧的一条。
//
// 前端那次 PUT 带回来的是它**早先读到**的那一份：期间循环可能已经推过水位，
// 跟着覆盖就等于"把已处理过的那一次再跑一遍"。签到与换凭据都是不可逆动作，
// 所以 `last_fire_at` 从 DO UPDATE 那一串里**整条拿掉**，由本用例钉住。
//
// 反例造法：把 `last_fire_at = excluded.last_fire_at` 加回 DO UPDATE。
func TestUpsertScheduleNeverRewindsTheWaterColumn(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	seeded := seededSchedule(t, s, "sched-1")

	fired := time.Date(2026, 9, 22, 7, 30, 0, 0, time.UTC)
	if _, err := s.MarkScheduleFire(ctx, core.ScopeLocal(), "sched-1", fired); err != nil {
		t.Fatalf("推水位: %v", err)
	}

	stale := seeded // 前端早先读到的那一份：水位还是旧值
	stale.Name = "改名了"
	stale.LastFireAt = nil
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &stale); err != nil {
		t.Fatalf("PUT 定时任务: %v", err)
	}

	after := loadedOrFatal(t, s, "sched-1")
	if after.LastFireAt == nil || !after.LastFireAt.Equal(fired) {
		t.Fatalf("编辑定义把水位倒回去了：%v（期望 %v）", after.LastFireAt, fired)
	}
	// 同一条语句里定义那一半**要**照改 —— 别把"不覆盖水位"做成"整条 PUT 无效"。
	if after.Name != "改名了" {
		t.Fatalf("名字没被覆盖：%q", after.Name)
	}
}

// TestUpsertScheduleStillSetsTheWaterColumnOnInsert 是上一条的另一半：
// 新建那一支**带着** `last_fire_at` 写进去（否则 CLI/导入建的行永远没有水位，
// 循环会把"从没有过的一次"当成待补的错过）。
func TestUpsertScheduleStillSetsTheWaterColumnOnInsert(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	fire := time.Date(2026, 9, 22, 7, 30, 0, 0, time.UTC)
	sc := configSchedule("sched-new")
	sc.LastFireAt = &fire
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc); err != nil {
		t.Fatalf("新建带水位的定时任务: %v", err)
	}
	after := loadedOrFatal(t, s, "sched-new")
	if after.LastFireAt == nil || !after.LastFireAt.Equal(fire) {
		t.Fatalf("新建时没写水位：%v", after.LastFireAt)
	}
}

func TestMissPolicyRoundTripsAndNilMeansUnchosen(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	// nil 与"显式选了 skip"是**两种东西**：前者跟随能力默认，后者是用户的决定。
	// 把 nil 读成任何默认值，等于替没选过的人拍板。
	unchosen := configSchedule("sched-nil")
	unchosen.MissPolicy = nil
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &unchosen); err != nil {
		t.Fatalf("写未选的: %v", err)
	}
	skip := core.MissSkip
	chosen := configSchedule("sched-skip")
	chosen.MissPolicy = &skip
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &chosen); err != nil {
		t.Fatalf("写 skip 的: %v", err)
	}

	if got := loadedOrFatal(t, s, "sched-nil"); got.MissPolicy != nil {
		t.Errorf("NULL 该读回 nil（没人选过），实得 %v", *got.MissPolicy)
	}
	got := loadedOrFatal(t, s, "sched-skip")
	if got.MissPolicy == nil || *got.MissPolicy != core.MissSkip {
		t.Fatalf("skip 该原样读回，实得 %v", got.MissPolicy)
	}
}

// TestUnknownMissPolicyAndStatusAreRefusedAtTheDoor 守"不把脏数据洗成合法"。
//
// 两条窄写若放行未知值，读回来分别是"没人选过"与"没有状态" —— 那是把一个
// 来路不明的值改写成两种**看起来正常**的形状，比报错难查得多。
func TestUnknownMissPolicyAndStatusAreRefusedAtTheDoor(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	seededSchedule(t, s, "sched-1")

	bogus := core.MissPolicy(7)
	sc := configSchedule("sched-1")
	sc.MissPolicy = &bogus
	err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc)
	assertKind(t, mustErr(t, err), core.KindBadRequest)
	// 拒绝要**留原值**：报错了还把 NULL 写进去，等于把用户的决定丢了
	if after := loadedOrFatal(t, s, "sched-1"); after.MissPolicy == nil ||
		*after.MissPolicy != core.MissCatchUp {
		t.Errorf("被拒的写入动了 miss_policy：%v", after.MissPolicy)
	}

	if _, err := s.RecordScheduleRun(ctx, core.ScopeLocal(), "sched-1",
		nil, core.ScheduleStatus(9), nil); err == nil {
		t.Fatal("未知的执行状态该拒绝")
	}
	if after := loadedOrFatal(t, s, "sched-1"); after.LastStatus == nil ||
		*after.LastStatus != core.ScheduleOK {
		t.Errorf("被拒的记账写入动了 last_status：%v", after.LastStatus)
	}
}

// TestNarrowWritesOnHiddenOrMissingRowsReportFalseWithoutTouching 是循环的准入判据：
// "抬不动水位就不要打上游"。两条窄写都把不可见当成 false 而不是错误 ——
// 循环每 tick 遍历所有行，别人的行不该让它报错，但也不能照跑。
func TestNarrowWritesOnHiddenOrMissingRowsReportFalseWithoutTouching(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a := uid()
	b := newUser(t, s, "bob")

	mine := configSchedule("sched-1")
	mine.Owner = &a
	if err := s.UpsertSchedule(ctx, core.ScopeUser(&a), &mine); err != nil {
		t.Fatalf("A 写定时任务: %v", err)
	}
	run := time.Date(2026, 9, 20, 7, 30, 0, 0, time.UTC)
	miss := core.MissSkip
	mine.LastRunAt = &run
	mine.MissPolicy = &miss
	if err := s.UpsertSchedule(ctx, core.ScopeUser(&a), &mine); err != nil {
		t.Fatalf("A 补记账列: %v", err)
	}
	before := loadedOrFatal(t, s, "sched-1")

	fired, err := s.MarkScheduleFire(ctx, core.ScopeUser(&b), "sched-1",
		time.Date(2026, 9, 22, 7, 30, 0, 0, time.UTC))
	if err != nil {
		t.Fatalf("B 推水位不该报错，该报 false: %v", err)
	}
	if fired {
		t.Fatal("B 推不动 A 的水位")
	}

	attempt := time.Date(2026, 9, 22, 7, 30, 0, 0, time.UTC)
	wrote, err := s.RecordScheduleRun(ctx, core.ScopeUser(&b), "sched-1",
		&attempt, core.ScheduleOK, nil)
	if err != nil || wrote {
		t.Fatalf("B 写 A 的记账: %v / %v", err, wrote)
	}

	// 不存在的那一行同样是 false 而不是错误
	if ok, err := s.MarkScheduleFire(ctx, core.ScopeLocal(), "没有这个任务",
		time.Now()); err != nil || ok {
		t.Fatalf("不存在的行: %v / %v", err, ok)
	}

	after := loadedOrFatal(t, s, "sched-1")
	if !reflect.DeepEqual(after, before) {
		t.Fatalf("越权的窄写改动了 A 的行：\n实得 %+v\n期望 %+v", after, before)
	}
}
