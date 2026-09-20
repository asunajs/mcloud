package mcloudio

import (
	"context"
	"database/sql"
	"encoding/json"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// [core.ConfigStore] 的单元测试；逐条平移 Rust 侧
// `store/config/tests/{mod,schedules,pipelines,prefs,isolation,ledger,reopen,states}.rs`。
//
// 本文件与 `store_test.go` 分开，但**共用它的 helper**（`store` / `storeWithUser` /
// `account` / `assertKind` / `at` / `openAt` / `newUser`）—— 那些是"怎么建一个库"，
// 与"测哪个接口"无关。重写一份的代价不是多敲几行，而是两份夹具迟早分叉。

// ─────────────────────────────────────────────────────────────────────────────
// 夹具（对应 Rust 侧 config/tests/mod.rs 的 `schedule()` / `pipeline()`）
// ─────────────────────────────────────────────────────────────────────────────

// scheduleAt / pipelineAt 是夹具用的时刻。
//
// ⚠ 用 `time.Date` 而不是 `at(t, …)`：这些是**包级夹具**，没有 `*testing.T`。
// 写死 UTC 时刻是刻意的 —— 它们最终进的是会被 SQL 做字符串排序的列，
// 换成本地时区会让断言随机器变化。
var (
	scheduleCreatedAt = time.Date(2026, 9, 16, 1, 0, 0, 0, time.UTC)
	pipelineCreatedAt = time.Date(2026, 9, 16, 1, 57, 41, 0, time.UTC)
	pipelineUpdatedAt = time.Date(2026, 9, 16, 2, 24, 7, 0, time.UTC)
)

// targetCapability 取"单个能力"目标的指针。
func targetCapability(code string) *core.ScheduleTarget {
	t := core.NewScheduleTargetCapability(code)
	return &t
}

// configSchedule 是定时任务夹具（对应 Rust 侧 `schedule()`）。
//
// ⚠ `Owner` 刻意留 nil：夹具代表"迁移留下的无主数据"，
// 用例自己决定要不要给它主人（`ScopeLocal` 下无主是合法的）。
func configSchedule(id string) core.Schedule {
	sc := core.NewSchedule(id, "每日签到", "0 30 7 * * *", "Asia/Shanghai")
	sc.Target = targetCapability("sign")
	sc.AccountIDs = []core.AccountID{core.NewAccountID("19****01")}
	sc.Body = json.RawMessage(`{"codes":["口令"]}`)
	sc.Enabled = true
	sc.CreatedAt = &scheduleCreatedAt
	return sc
}

// configPipeline 是组合夹具（对应 Rust 侧 `pipeline()`）。
//
// 两步刻意不同：第一步什么都没给（`body` / `continue_on_error` 都是 null），
// 第二步两者都给 —— 一次覆盖"可选字段缺席"与"在场"两种形状。
func configPipeline(id string) core.Pipeline {
	p := core.NewPipeline(id, "轻量巡检")
	desc := "查奖品 + 领奖品"
	p.Description = &desc

	cont := true
	p.Steps = []core.PipelineStep{
		{Task: "prizes"},
		{Task: "receive", Body: json.RawMessage(`{"onlyNew":true}`), ContinueOnError: &cont},
	}
	p.AccountMode = core.ModeSequential
	p.StopOnError = true
	p.CreatedAt = &pipelineCreatedAt
	p.UpdatedAt = &pipelineUpdatedAt

	ok := core.PipelineOK
	p.LastStatus = &ok
	ms := int64(2481)
	p.LastDurationMS = &ms
	return p
}

// ─────────────────────────────────────────────────────────────────────────────
// schedules
// ─────────────────────────────────────────────────────────────────────────────

func TestScheduleRoundtripKeepsEveryField(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	original := configSchedule("sched-1")
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &original); err != nil {
		t.Fatalf("写定时任务: %v", err)
	}

	loaded, err := s.GetSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("读定时任务: %v", err)
	}
	if loaded == nil {
		t.Fatal("刚写的定时任务应当读得到")
	}
	// 整结构体比对：漏掉任何一个字段的写入或读回都会在这里露出来。
	// ⚠ 逐字段断言做不到这件事 —— 加字段时没人会记得补一条断言。
	if !reflect.DeepEqual(*loaded, original) {
		t.Fatalf("往返之后不一致：\n实得 %+v\n期望 %+v", *loaded, original)
	}
}

func TestScheduleTargetAbsentSurvivesAsNone(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	sc := configSchedule("sched-1")
	sc.Target = nil
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc); err != nil {
		t.Fatalf("写定时任务: %v", err)
	}

	loaded, err := s.GetSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("读定时任务: %v", err)
	}
	if loaded.Target != nil {
		t.Fatalf("没给 target 应当读回 nil，实得 %+v", *loaded.Target)
	}
}

// TestAbsentBodyIsNullNotTheStringNull 断言"没给参数"与"给了个 null 参数"
// 在存储层是**两种东西**：前者是 SQL NULL，后者是文本 `null`。
//
// ⚠ 这条只能直接查列来验：走 `GetSchedule` 两条路径都得到 nil，
// 而它们的区别恰恰在于"库里存的是什么" —— 迁到别的实现时，
// 把 NULL 写成 `"null"` 会让 Rust 侧读成 `Some(Value::Null)`
// （一个"显式传了 null"的参数），行为完全不同。
func TestAbsentBodyIsNullNotTheStringNull(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	sc := configSchedule("sched-1")
	sc.Body = nil
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc); err != nil {
		t.Fatalf("写定时任务: %v", err)
	}

	var raw sql.NullString
	if err := s.db.QueryRowContext(ctx,
		"SELECT body FROM schedules WHERE id = 'sched-1'").Scan(&raw); err != nil {
		t.Fatalf("直查 body 列: %v", err)
	}
	if raw.Valid {
		t.Fatalf("没给参数应存 NULL，实得 %q", raw.String)
	}

	loaded, err := s.GetSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("读定时任务: %v", err)
	}
	if loaded.Body != nil {
		t.Fatalf("读回应当是 nil，实得 %q", string(loaded.Body))
	}
}

func TestScheduleUpsertUpdatesInPlace(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	sc := configSchedule("sched-1")
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc); err != nil {
		t.Fatalf("首次写入: %v", err)
	}

	sc.Name = "改名了"
	sc.Enabled = false
	skipped := core.ScheduleSkipped
	sc.LastStatus = &skipped
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc); err != nil {
		t.Fatalf("覆盖写入: %v", err)
	}

	all, err := s.ListSchedules(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(all) != 1 {
		t.Fatalf("重复写入不应产生第二条，实得 %d 条", len(all))
	}
	if all[0].Name != "改名了" {
		t.Fatalf("名字没被覆盖: %q", all[0].Name)
	}
	if all[0].Enabled {
		t.Fatal("enabled 没被覆盖成 false")
	}
	if all[0].LastStatus == nil || *all[0].LastStatus != core.ScheduleSkipped {
		t.Fatalf("last_status 没被覆盖: %v", all[0].LastStatus)
	}
}

// TestSchedulesKeepInsertionOrderWhenCreatedAtTies 守的是 `ORDER BY … rowid ASC`
// 那半个排序键 —— 没有它，"同 created_at"的行顺序由 SQLite 的实现决定。
func TestSchedulesKeepInsertionOrderWhenCreatedAtTies(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	// 三个都没有 created_at —— 排序完全靠 rowid，也就是插入顺序
	for _, id := range []string{"c", "a", "b"} {
		sc := configSchedule(id)
		sc.CreatedAt = nil
		if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &sc); err != nil {
			t.Fatalf("写 %s: %v", id, err)
		}
	}

	all, err := s.ListSchedules(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	ids := make([]string, 0, len(all))
	for _, x := range all {
		ids = append(ids, x.ID)
	}
	if !reflect.DeepEqual(ids, []string{"c", "a", "b"}) {
		t.Fatalf("同 created_at 时应保持插入顺序，实得 %v", ids)
	}
}

func TestDeleteScheduleReportsWhetherARowWentAway(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &[]core.Schedule{configSchedule("sched-1")}[0]); err != nil {
		t.Fatalf("写定时任务: %v", err)
	}

	gone, err := s.DeleteSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("删除: %v", err)
	}
	if !gone {
		t.Fatal("第一次删除应返回 true")
	}

	gone, err = s.DeleteSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("重复删除: %v", err)
	}
	if gone {
		t.Fatal("第二次删除应返回 false（没删掉任何行）")
	}

	all, err := s.ListSchedules(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(all) != 0 {
		t.Fatalf("删完之后列表应为空，实得 %d 条", len(all))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// pipelines
// ─────────────────────────────────────────────────────────────────────────────

func TestPipelineRoundtripKeepsStepsAndAccounts(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	original := configPipeline("pipe-1")
	original.AccountIDs = []core.AccountID{
		core.NewAccountID("19****01"), core.NewAccountID("19****02"),
	}
	original.AccountMode = core.ModeParallel
	original.StopOnError = false
	if err := s.UpsertPipeline(ctx, core.ScopeLocal(), &original); err != nil {
		t.Fatalf("写组合: %v", err)
	}

	loaded, err := s.GetPipeline(ctx, core.ScopeLocal(), "pipe-1")
	if err != nil {
		t.Fatalf("读组合: %v", err)
	}
	if loaded == nil {
		t.Fatal("刚写的组合应当读得到")
	}
	if !reflect.DeepEqual(*loaded, original) {
		t.Fatalf("往返之后不一致：\n实得 %+v\n期望 %+v", *loaded, original)
	}
	if len(loaded.Steps) != 2 {
		t.Fatalf("步骤数不符: %d", len(loaded.Steps))
	}
	if loaded.Steps[1].ContinueOnError == nil || !*loaded.Steps[1].ContinueOnError {
		t.Fatalf("第二步的 continue_on_error 应是 true: %v", loaded.Steps[1].ContinueOnError)
	}
}

// TestStatusColumnsRejectTheOtherFamilysThirdState 守的是"两族状态枚举不是同一个"。
//
// ⚠ 这条的要点在**存储层**而不在纯函数：`skipped` 写进 `pipelines.last_status`
// 必须读回 nil，而不是被解析成某个合法值。若把两族合并成一个枚举
// （或者让组合复用了定时任务的解析函数），`skipped` 会在组合那一列里
// 被读成"上次执行跳过"—— 而组合根本没有"跳过"这个终态。
//
// 两族的第三态：定时任务有 `skipped`（同「任务+账号」在跑，本次不排队），
// 组合有 `partial`（部分步骤失败但没中断）。**互相都不认识。**
func TestStatusColumnsRejectTheOtherFamilysThirdState(t *testing.T) {
	ctx := context.Background()

	// 纯函数那一面：两族各自认识自己的第三态、不认识对方的
	if _, ok := core.ParsePipelineStatus("skipped"); ok {
		t.Fatal("组合状态不该认识 skipped")
	}
	if _, ok := core.ParseScheduleStatus("partial"); ok {
		t.Fatal("定时任务状态不该认识 partial")
	}
	if v, ok := core.ParsePipelineStatus("partial"); !ok || v != core.PipelinePartial {
		t.Fatalf("组合应认识 partial: %v %v", v, ok)
	}
	if v, ok := core.ParseScheduleStatus("skipped"); !ok || v != core.ScheduleSkipped {
		t.Fatalf("定时任务应认识 skipped: %v %v", v, ok)
	}

	// 存储那一面：把对方的值写进列里，读回必须是"没有状态"
	s := store(t)
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &[]core.Schedule{configSchedule("sched-1")}[0]); err != nil {
		t.Fatalf("写定时任务: %v", err)
	}
	if err := s.UpsertPipeline(ctx, core.ScopeLocal(), &[]core.Pipeline{configPipeline("pipe-1")}[0]); err != nil {
		t.Fatalf("写组合: %v", err)
	}
	mustExec(t, s.db, "UPDATE schedules SET last_status = 'partial' WHERE id = 'sched-1'")
	mustExec(t, s.db, "UPDATE pipelines SET last_status = 'skipped' WHERE id = 'pipe-1'")

	sc, err := s.GetSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("读定时任务: %v", err)
	}
	if sc.LastStatus != nil {
		t.Fatalf("列里是 partial，定时任务应读成「没有状态」，实得 %v", *sc.LastStatus)
	}
	p, err := s.GetPipeline(ctx, core.ScopeLocal(), "pipe-1")
	if err != nil {
		t.Fatalf("读组合: %v", err)
	}
	if p.LastStatus != nil {
		t.Fatalf("列里是 skipped，组合应读成「没有状态」，实得 %v", *p.LastStatus)
	}
}

// TestUnknownStatusStringReadsAsNoneInsteadOfAWrongValue 对应 Rust 侧
// `unknown_status_string_reads_as_none_instead_of_a_wrong_value`。
//
// ⚠ 未知值**不归默认值**：硬塞 `error` 会让一行被手改/损坏的数据在界面上
// 表现成"上次执行失败"，而那是查不出原因的。宁可"没有状态"。
func TestUnknownStatusStringReadsAsNoneInsteadOfAWrongValue(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &[]core.Schedule{configSchedule("sched-1")}[0]); err != nil {
		t.Fatalf("写定时任务: %v", err)
	}
	mustExec(t, s.db, "UPDATE schedules SET last_status = 'weird' WHERE id = 'sched-1'")

	sc, err := s.GetSchedule(ctx, core.ScopeLocal(), "sched-1")
	if err != nil {
		t.Fatalf("读定时任务: %v", err)
	}
	if sc.LastStatus != nil {
		t.Fatalf("未知状态应读成 nil，实得 %v", *sc.LastStatus)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 抢兑配置 / 界面偏好（单行表）
// ─────────────────────────────────────────────────────────────────────────────

func TestExchangeConfigDefaultsWhenAbsentThenRoundtrips(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	defaults, err := s.ExchangeConfig(ctx)
	if err != nil {
		t.Fatalf("读默认配置: %v", err)
	}
	if defaults.Enabled {
		t.Fatal("默认必须**关闭** —— 抢兑会真的扣云朵，默认开启等于一升级就自动花钱")
	}
	if !reflect.DeepEqual(defaults.Windows, []string{"09:59:40", "11:59:40"}) {
		t.Fatalf("默认窗口不符: %v", defaults.Windows)
	}
	if len(defaults.Bindings) != 0 {
		t.Fatalf("默认不该有绑定: %v", defaults.Bindings)
	}

	// 读默认值**不应写盘**：一个读操作顺手写库会让"只读地看一眼配置"变成一次写入
	var count int64
	if err := s.db.QueryRowContext(ctx,
		"SELECT COUNT(*) FROM exchange_config").Scan(&count); err != nil {
		t.Fatalf("数行数: %v", err)
	}
	if count != 0 {
		t.Fatalf("读默认值不该建行，实得 %d 行", count)
	}

	prizeName := "100万tokens叠加包"
	toSave := core.ExchangeConfig{
		Enabled: true,
		Windows: []string{"09:59:40", "11:59:40"},
		Bindings: []core.ExchangeBinding{{
			AccountID: core.NewAccountID("19****01"),
			PrizeID:   260624006,
			PrizeName: &prizeName,
		}},
		ScheduleIDs: []string{"sched-x"},
		UpdatedAt:   timePtr(time.Date(2026, 9, 16, 12, 3, 29, 0, time.UTC)),
	}
	if err := s.PutExchangeConfig(ctx, &toSave); err != nil {
		t.Fatalf("写配置: %v", err)
	}

	got, err := s.ExchangeConfig(ctx)
	if err != nil {
		t.Fatalf("读回配置: %v", err)
	}
	if !reflect.DeepEqual(got, toSave) {
		t.Fatalf("往返之后不一致：\n实得 %+v\n期望 %+v", got, toSave)
	}
}

// TestRepeatedPutKeepsTheSingleRowTablesAtOneRow 对应 Rust 侧
// `exchange_and_webui_configs_stay_single_row`。
//
// ⚠ 与 `schema_test.go` 的 `TestSingleRowConfigTablesRejectASecondRow` **不是同一条**：
// 那条直接 `INSERT` 撞主键约束（证明 schema 有闸），这条走 `PutXxx` 三次
// （证明**实现**用的是 upsert 而不是 insert）—— 后者漏了的话，
// 第二次保存配置就会报"主键冲突"，而用户只是改了个设置。
func TestRepeatedPutKeepsTheSingleRowTablesAtOneRow(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	for i := 0; i < 3; i++ {
		exchange := core.NewExchangeConfig()
		webui := core.NewWebuiConfig()
		if err := s.PutExchangeConfig(ctx, &exchange); err != nil {
			t.Fatalf("第 %d 次写抢兑配置: %v", i+1, err)
		}
		if err := s.PutWebuiConfig(ctx, &webui); err != nil {
			t.Fatalf("第 %d 次写界面偏好: %v", i+1, err)
		}
	}

	for _, table := range []string{"exchange_config", "webui_config"} {
		var count int64
		if err := s.db.QueryRowContext(ctx,
			"SELECT COUNT(*) FROM "+table).Scan(&count); err != nil {
			t.Fatalf("数 %s 行数: %v", table, err)
		}
		if count != 1 {
			t.Fatalf("%s 应始终只有一行，实得 %d", table, count)
		}
	}
}

func TestWebuiConfigDefaultsAndPageWhitelistOnWrite(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	defaults, err := s.WebuiConfig(ctx)
	if err != nil {
		t.Fatalf("读默认偏好: %v", err)
	}
	if !reflect.DeepEqual(defaults, core.NewWebuiConfig()) {
		t.Fatalf("默认偏好不符：\n实得 %+v\n期望 %+v", defaults, core.NewWebuiConfig())
	}

	// 写入时就把非法页面收拢，别把脏值留在库里。
	// ⚠ 这是**安全**检查不只是整洁：last_page 会进前端的路由。
	bad := core.NewWebuiConfig()
	bad.LastPage = "../../etc/passwd"
	if err := s.PutWebuiConfig(ctx, &bad); err != nil {
		t.Fatalf("写偏好: %v", err)
	}

	got, err := s.WebuiConfig(ctx)
	if err != nil {
		t.Fatalf("读回偏好: %v", err)
	}
	if got.LastPage != core.DefaultWebuiPage {
		t.Fatalf("非法页面应被收拢到 %q，实得 %q", core.DefaultWebuiPage, got.LastPage)
	}

	// 库里也不该留着脏值（读侧那一道是给"手改过的库"兜底的，不是给人擦屁股的）
	var stored string
	if err := s.db.QueryRowContext(ctx,
		"SELECT last_page FROM webui_config WHERE id = 1").Scan(&stored); err != nil {
		t.Fatalf("直查 last_page: %v", err)
	}
	if stored != core.DefaultWebuiPage {
		t.Fatalf("库里存的应是收拢后的值，实得 %q", stored)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 归属与认领
// ─────────────────────────────────────────────────────────────────────────────

// TestConfigsAreIsolatedBetweenUsers 交叉读取：A 的定时任务与组合，
// B 既看不到也删不掉。
//
// ⚠ "看不到"与"不存在"必须是**同一个结果**（连存在性都不泄露）——
// 回 403 等于确认资源存在，把账号 id 空间变成可枚举。
func TestConfigsAreIsolatedBetweenUsers(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a := uid()
	b := newUser(t, s, "bob")

	sc := configSchedule("sched-a")
	sc.Owner = &a
	if err := s.UpsertSchedule(ctx, core.ScopeUser(&a), &sc); err != nil {
		t.Fatalf("A 写定时任务: %v", err)
	}
	pl := configPipeline("pipe-a")
	pl.Owner = &a
	if err := s.UpsertPipeline(ctx, core.ScopeUser(&a), &pl); err != nil {
		t.Fatalf("A 写组合: %v", err)
	}

	// 本人看得见
	gotSchedules, err := s.ListSchedules(ctx, core.ScopeUser(&a))
	if err != nil {
		t.Fatalf("A 列定时任务: %v", err)
	}
	if len(gotSchedules) != 1 {
		t.Fatalf("A 应看到 1 条定时任务，实得 %d", len(gotSchedules))
	}
	gotPipelines, err := s.ListPipelines(ctx, core.ScopeUser(&a))
	if err != nil {
		t.Fatalf("A 列组合: %v", err)
	}
	if len(gotPipelines) != 1 {
		t.Fatalf("A 应看到 1 个组合，实得 %d", len(gotPipelines))
	}
	if one, err := s.GetSchedule(ctx, core.ScopeUser(&a), "sched-a"); err != nil || one == nil {
		t.Fatalf("A 应读得到自己的定时任务: %v %v", one, err)
	}

	// 别人看不见
	bs, err := s.ListSchedules(ctx, core.ScopeUser(&b))
	if err != nil {
		t.Fatalf("B 列定时任务: %v", err)
	}
	if len(bs) != 0 {
		t.Fatal("B 不该看见 A 的定时任务")
	}
	bp, err := s.ListPipelines(ctx, core.ScopeUser(&b))
	if err != nil {
		t.Fatalf("B 列组合: %v", err)
	}
	if len(bp) != 0 {
		t.Fatal("B 不该看见 A 的组合")
	}
	if one, err := s.GetSchedule(ctx, core.ScopeUser(&b), "sched-a"); err != nil || one != nil {
		t.Fatalf("B 读 A 的定时任务应得 nil: %v %v", one, err)
	}
	if one, err := s.GetPipeline(ctx, core.ScopeUser(&b), "pipe-a"); err != nil || one != nil {
		t.Fatalf("B 读 A 的组合应得 nil: %v %v", one, err)
	}

	// 删也删不掉，且**返回 false 而不是报错**（不可见等价于不存在）
	gone, err := s.DeleteSchedule(ctx, core.ScopeUser(&b), "sched-a")
	if err != nil {
		t.Fatalf("B 删 A 的定时任务: %v", err)
	}
	if gone {
		t.Fatal("B 不该删掉 A 的定时任务")
	}
	after, err := s.ListSchedules(ctx, core.ScopeUser(&a))
	if err != nil {
		t.Fatalf("A 再列: %v", err)
	}
	if len(after) != 1 {
		t.Fatalf("B 的删除不该动到 A 的行，实得 %d 条", len(after))
	}

	// ScopeLocal（CLI 口径）不过滤：同一份过滤表达式，两种范围
	localSchedules, err := s.ListSchedules(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("Local 列定时任务: %v", err)
	}
	if len(localSchedules) != 1 {
		t.Fatalf("Local 应看到 1 条，实得 %d", len(localSchedules))
	}
	localPipelines, err := s.ListPipelines(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("Local 列组合: %v", err)
	}
	if len(localPipelines) != 1 {
		t.Fatalf("Local 应看到 1 个组合，实得 %d", len(localPipelines))
	}
}

// TestACannotOverwriteAnotherUsersSchedule 守的是**写越权**。
//
// ⚠ 这条比"读不到"更要紧：id 是主键、写入是 `ON CONFLICT DO UPDATE`，
// 只过滤读的话越权会从"看得到"升级成"**改得掉**"。
func TestACannotOverwriteAnotherUsersSchedule(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a := uid()
	b := newUser(t, s, "bob")

	mine := configSchedule("sched-1")
	mine.Owner = &a
	if err := s.UpsertSchedule(ctx, core.ScopeUser(&a), &mine); err != nil {
		t.Fatalf("A 写定时任务: %v", err)
	}

	theirs := configSchedule("sched-1")
	theirs.Owner = &b
	theirs.Name = "B 的任务"
	err := s.UpsertSchedule(ctx, core.ScopeUser(&b), &theirs)
	assertKind(t, err, core.KindBadRequest)
	if err == nil || !strings.Contains(err.Error(), "其他用户") {
		t.Fatalf("要给出可行动的提示，而不是让调用方猜: %v", err)
	}

	loaded, err := s.GetSchedule(ctx, core.ScopeUser(&a), "sched-1")
	if err != nil {
		t.Fatalf("A 读回: %v", err)
	}
	if loaded == nil || loaded.Name != "每日签到" {
		t.Fatalf("A 的行不能被改动: %+v", loaded)
	}

	// 也不能把自己写的东西写成"无主"（那会立刻从所有人视野里消失）
	orphan := configSchedule("sched-2")
	if err := s.UpsertSchedule(ctx, core.ScopeUser(&a), &orphan); err == nil {
		t.Fatal("ScopeUser 下不该写出无主的行")
	}
}

// TestClaimingCoversSchedulesAndPipelines 覆盖"认领"的另两族。
//
// ⚠ `TestUnownedAccountsAreClaimedExactlyOnce` 只覆盖账号，并在注释里
// 明确把这两族交到这里 —— 分开的理由是它们的 SQL 是分开写的，
// 一条覆盖不了另一条（"只改了账号那条"是很容易犯的错）。
func TestClaimingCoversSchedulesAndPipelines(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a := uid()

	// 迁移留下的无主数据（ScopeLocal 是迁移/CLI 的口径）
	if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &[]core.Schedule{configSchedule("sched-legacy")}[0]); err != nil {
		t.Fatalf("写无主定时任务: %v", err)
	}
	if err := s.UpsertPipeline(ctx, core.ScopeLocal(), &[]core.Pipeline{configPipeline("pipe-legacy")}[0]); err != nil {
		t.Fatalf("写无主组合: %v", err)
	}

	before, err := s.ListSchedules(ctx, core.ScopeUser(&a))
	if err != nil {
		t.Fatalf("认领前列表: %v", err)
	}
	if len(before) != 0 {
		t.Fatal("无主的定时任务谁都看不见 —— 宁可暂时不出现，也不要谁都拿得到")
	}

	claimed, err := s.ClaimUnowned(ctx, a)
	if err != nil {
		t.Fatalf("认领: %v", err)
	}
	if claimed.Accounts != 0 || claimed.Schedules != 1 || claimed.Pipelines != 1 {
		t.Fatalf("认领条数不符: %+v", claimed)
	}

	afterSchedules, err := s.ListSchedules(ctx, core.ScopeUser(&a))
	if err != nil {
		t.Fatalf("认领后列表: %v", err)
	}
	if len(afterSchedules) != 1 {
		t.Fatalf("认领后 A 应看到 1 条定时任务，实得 %d", len(afterSchedules))
	}
	afterPipelines, err := s.ListPipelines(ctx, core.ScopeUser(&a))
	if err != nil {
		t.Fatalf("认领后列组合: %v", err)
	}
	if len(afterPipelines) != 1 {
		t.Fatalf("认领后 A 应看到 1 个组合，实得 %d", len(afterPipelines))
	}

	again, err := s.ClaimUnowned(ctx, a)
	if err != nil {
		t.Fatalf("重复认领: %v", err)
	}
	if again != (core.ClaimedRows{}) {
		t.Fatalf("认领必须幂等，实得 %+v", again)
	}

	late := newUser(t, s, "late-user")
	stolen, err := s.ClaimUnowned(ctx, late)
	if err != nil {
		t.Fatalf("后来者认领: %v", err)
	}
	if stolen != (core.ClaimedRows{}) {
		t.Fatalf("已归属的数据不能被后来者抢走，实得 %+v", stolen)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 导入台账
// ─────────────────────────────────────────────────────────────────────────────

func TestImportLedgerRecordsAndReadsBack(t *testing.T) {
	ctx := context.Background()
	s := store(t)

	missing, err := s.ImportRecordFor(ctx, "/old/state.json")
	if err != nil {
		t.Fatalf("读不存在的台账: %v", err)
	}
	if missing != nil {
		t.Fatalf("没导入过应返回 nil，实得 %+v", missing)
	}

	raw := `{"accounts":{}}`
	record := ImportRecord{
		Source:      "/old/state.json",
		SHA256:      strings.Repeat("a", 64),
		ImportedAt:  "2026-09-16T22:00:00+00:00",
		RecordCount: 2,
		Issues:      1,
		Raw:         &raw,
	}
	if err := s.RecordImport(ctx, &record); err != nil {
		t.Fatalf("写台账: %v", err)
	}

	loaded, err := s.ImportRecordFor(ctx, "/old/state.json")
	if err != nil {
		t.Fatalf("读台账: %v", err)
	}
	if loaded == nil {
		t.Fatal("刚写的台账应当读得到")
	}
	if !reflect.DeepEqual(*loaded, record) {
		t.Fatalf("往返之后不一致：\n实得 %+v\n期望 %+v", *loaded, record)
	}

	// 同一个源再写一次是覆盖，不是新增
	second := record
	second.SHA256 = strings.Repeat("b", 64)
	if err := s.RecordImport(ctx, &second); err != nil {
		t.Fatalf("覆盖台账: %v", err)
	}
	all, err := s.ListImports(ctx)
	if err != nil {
		t.Fatalf("列台账: %v", err)
	}
	if len(all) != 1 {
		t.Fatalf("同一个源应只有一条，实得 %d 条", len(all))
	}
	if !strings.HasPrefix(all[0].SHA256, "b") {
		t.Fatalf("台账没被覆盖: %s", all[0].SHA256)
	}

	// 摘要必须能安全进日志 —— 它**不含 Raw**
	if !strings.Contains(record.Summary(), "记录 2 条") {
		t.Fatalf("摘要应含条数: %s", record.Summary())
	}
	if strings.Contains(record.Summary(), "accounts") {
		t.Fatalf("摘要不得带出原文: %s", record.Summary())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 重开
// ─────────────────────────────────────────────────────────────────────────────

func TestConfigSurvivesReopen(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "mcloud.db")

	{
		s := openAt(t, path)
		if err := s.UpsertSchedule(ctx, core.ScopeLocal(), &[]core.Schedule{configSchedule("sched-1")}[0]); err != nil {
			t.Fatalf("写定时任务: %v", err)
		}
		if err := s.UpsertPipeline(ctx, core.ScopeLocal(), &[]core.Pipeline{configPipeline("pipe-1")}[0]); err != nil {
			t.Fatalf("写组合: %v", err)
		}
		if err := s.Close(); err != nil {
			t.Fatalf("关库: %v", err)
		}
	}

	s := openAt(t, path)
	t.Cleanup(func() { _ = s.Close() })

	schedules, err := s.ListSchedules(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("重开后列定时任务: %v", err)
	}
	if len(schedules) != 1 {
		t.Fatalf("重开后应有 1 条定时任务，实得 %d", len(schedules))
	}
	pipelines, err := s.ListPipelines(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("重开后列组合: %v", err)
	}
	if len(pipelines) != 1 {
		t.Fatalf("重开后应有 1 个组合，实得 %d", len(pipelines))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录态与账号表的关系
// ─────────────────────────────────────────────────────────────────────────────

// TestAccountStateIsStoredApartFromAccounts 断言**读账号列表不会带出凭据**。
//
// ⚠ 两张表没有 JOIN，凭据在物理上不在结果里 —— 这是结构性的保证，
// 而不是"记得别 SELECT 那一列"。若哪天为了少一次查询把它们合并，
// 这条会红，而那是它唯一该响的时候。
func TestAccountStateIsStoredApartFromAccounts(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")

	state := core.NewAccountState("acc-1")
	state.JWTToken = secretPtr("SECRETJWT")
	state.DeviceID = secretPtr("DEV")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &state); err != nil {
		t.Fatalf("写登录态: %v", err)
	}

	accounts, err := s.ListAccounts(ctx, core.ScopeUser(uidPtr()))
	if err != nil {
		t.Fatalf("列账号: %v", err)
	}
	if len(accounts) != 1 {
		t.Fatalf("应有 1 个账号，实得 %d", len(accounts))
	}
	// ⚠ 这里比的是**账号行**：它不该被登录态污染。
	// `core.Account` 里根本没有凭据字段 —— 但这条断言的用处在于
	// "将来有人往 Account 上加了凭据字段"时它会红。
	want := account("acc-1")
	if !reflect.DeepEqual(accounts[0], want) {
		t.Fatalf("账号行不应被登录态污染：\n实得 %+v\n期望 %+v", accounts[0], want)
	}

	loaded, err := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if loaded == nil {
		t.Fatal("刚写的登录态应当读得到")
	}
	if loaded.JWTToken == nil || loaded.JWTToken.Value() != "SECRETJWT" {
		t.Fatalf("jwt 不符: %v", loaded.JWTToken)
	}
	if !reflect.DeepEqual(*loaded, state) {
		t.Fatalf("往返之后不一致：\n实得 %+v\n期望 %+v", *loaded, state)
	}
}

// TestAccountStateUpsertClearsFieldsThatWereLeftOut 守的是**覆盖写而不是合并**。
//
// ⚠ 这条与 `TestUpsertAccountStateLeavesTheSmidColumnsAlone` 是一对反向的约束：
//   - 登录态自己的列：**整体覆盖**（`None` 要能真的清掉旧值）；
//   - 另一组（smid 两列）：**窄写**（`UpsertAccountState` 不许碰）。
//
// 两条都写反了都会表现成"重新登录之后旧设备号还在用"，而那是查不出原因的。
func TestAccountStateUpsertClearsFieldsThatWereLeftOut(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	id := core.NewAccountID("acc-1")
	// 登录态依附于账号：账号必须先存在且归属于调用者，否则写入会被隔离校验挡住
	putAccount(t, s, "acc-1")

	first := core.NewAccountState("acc-1")
	first.JWTToken = secretPtr("OLD")
	first.DeviceID = secretPtr("OLDDEV")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &first); err != nil {
		t.Fatalf("写旧登录态: %v", err)
	}

	// 重新登录：一份只带新 token 的快照，deviceId 没给（nil）
	second := core.NewAccountState("acc-1")
	second.JWTToken = secretPtr("NEW")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &second); err != nil {
		t.Fatalf("覆盖登录态: %v", err)
	}

	loaded, err := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if loaded == nil {
		t.Fatal("应当读得到")
	}
	if loaded.JWTToken == nil || loaded.JWTToken.Value() != "NEW" {
		t.Fatalf("jwt 应被覆盖成 NEW: %v", loaded.JWTToken)
	}
	if loaded.DeviceID != nil {
		t.Fatalf("旧的 deviceId 必须被清掉，否则重新登录后还会拿旧设备号去用: %v", *loaded.DeviceID)
	}
}
