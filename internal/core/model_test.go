package core

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

// 本文件对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/model.rs 的 11 条测试，
// 外加 Go 侧新长出来的那几条（打印脱敏的三条路、SmidState 拒绝序列化、
// 空数组不是 null、枚举真值表）。
//
// ⚠ 结构化形状的期望值来自 test/golden/enum-json-vectors.json（serde 产出），
// 见 enum_test.go —— 这里不重复造期望值。

// ─────────────────────────────────────────────────────────────────────────────
// Account
// ─────────────────────────────────────────────────────────────────────────────

func TestDisplayNamePrefersNicknameThenPhoneThenID(t *testing.T) {
	acc := NewAccount("19900001234")
	if got := acc.DisplayName(); got != "19900001234" {
		t.Fatalf("实得 %q，期望 id", got)
	}

	masked := "19****01"
	acc.PhoneMasked = &masked
	if got := acc.DisplayName(); got != masked {
		t.Fatalf("实得 %q，期望脱敏手机号", got)
	}

	nick := "账号2"
	acc.Nickname = &nick
	if got := acc.DisplayName(); got != nick {
		t.Fatalf("实得 %q，期望昵称", got)
	}
}

// TestDisplayNameKeepsAnExplicitlyEmptyNickname 钉住"空昵称不被降级"。
//
// Rust 的 `as_deref().or(...)` 对 `Some("")` **不会**跳到下一级（空串是 Some）。
// 顺手"判空再降级"会让用户自己设的空昵称被 ID 顶掉 —— 那是用户明确的选择。
func TestDisplayNameKeepsAnExplicitlyEmptyNickname(t *testing.T) {
	acc := NewAccount("19900001234")
	empty := ""
	acc.Nickname = &empty
	masked := "19****01"
	acc.PhoneMasked = &masked
	if got := acc.DisplayName(); got != "" {
		t.Fatalf("实得 %q，期望空串（空昵称是 Some，不该降级到掩码）", got)
	}
}

func TestAuthExpiryUnknownIsTreatedAsValid(t *testing.T) {
	acc := NewAccount("19900001234")
	const now = int64(1_789_000_000_000) // ⚠ 毫秒量级：测试数据不该教出错误的单位印象
	if acc.IsAuthExpiredAt(now) {
		t.Fatal("未知（nil）应按未过期处理")
	}

	earlier := now - 1
	acc.AuthExpire = &earlier
	if !acc.IsAuthExpiredAt(now) {
		t.Fatal("到期时间早于现在应判为过期")
	}

	later := now + 1
	acc.AuthExpire = &later
	if acc.IsAuthExpiredAt(now) {
		t.Fatal("到期时间晚于现在不该判为过期")
	}

	// 边界：**正好等于**算过期 —— 与 Session 的 `<=` 同口径。
	same := now
	acc.AuthExpire = &same
	if !acc.IsAuthExpiredAt(now) {
		t.Fatal("到期时间正好等于现在应判为过期")
	}
}

// TestAccountOwnerNilIsInvisibleToEveryone 钉住"未归属 = 谁都看不见"。
//
// 这是**漏判代价不对称**的那条：判成"谁都看得见"等于把迁移窗口里的
// 全部账号公开；判成"谁都看不见"只是账号暂时不出现。
func TestAccountOwnerNilIsInvisibleToEveryone(t *testing.T) {
	unowned := NewAccount("19900001234")
	for _, viewer := range []UserID{NewUserID("u-1"), NewUserID("u-2"), NewUserID("")} {
		if unowned.IsVisibleTo(viewer) {
			t.Fatalf("未归属账号不该对 %q 可见", viewer)
		}
	}

	owner := NewUserID("u-1")
	owned := NewAccount("19900001234")
	owned.Owner = &owner
	if !owned.IsVisibleTo(NewUserID("u-1")) {
		t.Fatal("归属 u-1 的账号应对 u-1 可见")
	}
	if owned.IsVisibleTo(NewUserID("u-2")) {
		t.Fatal("归属 u-1 的账号不该对 u-2 可见")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 打印脱敏
// ─────────────────────────────────────────────────────────────────────────────

// TestAccountStatePrintRedactsCredentials 凭据不得出现在打印里，
// 但纯标识（`UserDomainID`）要保留 —— 它有排障价值。
//
// ⚠ Go 比 Rust 多两条路，所以这里**三条都测**：`%v`（走 Stringer）、
// `%+v`（**展开字段，不走 struct 的 Stringer**）、`%#v`（走 GoStringer）。
// 只给 struct 写一个 String() 是挡不住后两条的（§6.1 #12）。
//
// 注意这条只管**运行期输出**。同一个值若出现在要进版本库的夹具/字面量里，
// 就必须是合成值：界线是"能不能进 GitHub"，不是"能不能进日志"。
func TestAccountStatePrintRedactsCredentials(t *testing.T) {
	domain := UserDomainID("1990000000000000001")
	state := AccountState{
		AccountID:     NewAccountID("19900001234"),
		JWTToken:      secretPtr("SECRETJWT"),
		DeviceID:      secretPtr("SECRETDEVICE"),
		UserDomainID:  &domain,
		RedpackToken:  secretPtr("SECRETREDPACK"),
		Authorization: secretPtr("SECRETAUTH"),
	}

	for _, verb := range []string{"%v", "%+v", "%#v"} {
		shown := fmt.Sprintf(verb, state)
		for _, leak := range []string{"SECRETJWT", "SECRETDEVICE", "SECRETREDPACK", "SECRETAUTH"} {
			if strings.Contains(shown, leak) {
				t.Fatalf("%s 泄漏了 %s: %s", verb, leak, shown)
			}
		}
		if !strings.Contains(shown, "redacted") {
			t.Fatalf("%s 应标明被脱敏: %s", verb, shown)
		}
		if !strings.Contains(shown, domain.String()) {
			t.Fatalf("%s 应保留纯标识以便排障（裸 *string 只会打印地址）: %s", verb, shown)
		}
	}
}

// TestAccountStatePrintMasksThePhone 手机号既不是凭据也不是"纯标识"：
// 它**要掩码**，不能原样打出来，也不能整段抹掉。
//
// 掩码而不是 `<redacted>` 的理由是排障：多账号时"这是哪个账号"必须看得出来。
func TestAccountStatePrintMasksThePhone(t *testing.T) {
	state := AccountState{AccountID: NewAccountID("19900001234"), Phone: phonePtr("19900000001")}

	for _, verb := range []string{"%v", "%+v", "%#v"} {
		shown := fmt.Sprintf(verb, state)
		if strings.Contains(shown, "19900000001") {
			t.Fatalf("%s 泄漏了完整号码: %s", verb, shown)
		}
		if !strings.Contains(shown, "19****01") {
			t.Fatalf("%s 应掩码而非抹掉: %s", verb, shown)
		}
	}
}

// TestSecretValueCountsRunesNotBytes 钉住"长度按字符数，不按字节数"。
//
// Rust 那边是 `v.chars().count()`。中文凭据按字节算会多报 2 倍长度。
func TestSecretValueCountsRunesNotBytes(t *testing.T) {
	var s SecretValue = "口令abc"
	if got := s.String(); got != "<redacted 5 字符>" {
		t.Fatalf("实得 %q，期望按字符数（5）而不是字节数（11）", got)
	}
	if got := s.Value(); got != "口令abc" {
		t.Fatalf("Value() 应给真值，实得 %q", got)
	}
}

func TestAccountStateEmptinessIsAboutAllFields(t *testing.T) {
	state := NewAccountState("19900001234")
	if !state.IsEmpty() {
		t.Fatal("新建的登录态应为空")
	}

	domain := UserDomainID("1")
	state.UserDomainID = &domain
	if state.IsEmpty() {
		t.Fatal("只有标识也算有内容")
	}

	// 只存了手机号的行**不是**空状态：它是 `mcloud signin --account` 的唯一来源，
	// 判成空就会被导入流程当"没有内容"跳过。
	onlyPhone := AccountState{AccountID: NewAccountID("19900001234"), Phone: phonePtr("19900000001")}
	if onlyPhone.IsEmpty() {
		t.Fatal("只存手机号不该判成空状态")
	}

	// v7 之后整串 auth 才是真正拿来用的那个凭据，它同理。
	onlyAuth := AccountState{AccountID: NewAccountID("19900001234"), Authorization: secretPtr("Basic x")}
	if onlyAuth.IsEmpty() {
		t.Fatal("只存整串 auth 不该判成空状态")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// SmidState
// ─────────────────────────────────────────────────────────────────────────────

// TestSmidStateRefusesToSerialize 钉住"刻意不可序列化"。
//
// Rust 侧的表达是**不派生 `Serialize`**（编译期挡住），Go 没有这种表达，
// 只能靠一个主动报错的 MarshalJSON 近似 —— 弱一档，但仍是**响的**：
// ⚠ 别为了"让它能用"把那个方法删掉。
func TestSmidStateRefusesToSerialize(t *testing.T) {
	state := NewSmidState("19900001234")
	state.FingerprintJSON = secretPtr(`{"a":1}`)
	state.DeviceID = secretPtr("B1234")

	if _, err := marshalJSON(state); err == nil {
		t.Fatal("SmidState 不该能被序列化：它含第三方风控的设备特征")
	}
	// 嵌进别的结构体也一样要响 —— 否则"顺手塞进响应"那条路还在。
	type wrapper struct {
		State SmidState `json:"state"`
	}
	if _, err := marshalJSON(wrapper{State: state}); err == nil {
		t.Fatal("SmidState 嵌进结构体后仍不该能被序列化")
	}
}

func TestSmidStatePrintRedactsBothColumns(t *testing.T) {
	state := NewSmidState("19900001234")
	state.FingerprintJSON = secretPtr("SECRETFP")
	state.DeviceID = secretPtr("SECRETDEV")

	shown := fmt.Sprintf("%+v", state)
	for _, leak := range []string{"SECRETFP", "SECRETDEV"} {
		if strings.Contains(shown, leak) {
			t.Fatalf("泄漏了 %s: %s", leak, shown)
		}
	}
}

func TestSmidStateEmptiness(t *testing.T) {
	state := NewSmidState("19900001234")
	if !state.IsEmpty() {
		t.Fatal("新建的应为空")
	}
	state.DeviceID = secretPtr("B1")
	if state.IsEmpty() {
		t.Fatal("有 deviceId 就不该算空")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时任务 / 任务组合
// ─────────────────────────────────────────────────────────────────────────────

func TestScheduleTargetRoundTripsWithKindTag(t *testing.T) {
	raw, err := marshalJSON(NewScheduleTargetCapability("sign"))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	// 整串形状的期望值在黄金向量里（serde 产出），这里只钉住往返。
	if string(raw) != `{"kind":"capability","code":"sign"}` {
		t.Fatalf("实得 %s", raw)
	}

	var back ScheduleTarget
	if err := json.Unmarshal(raw, &back); err != nil {
		t.Fatalf("反序列化失败: %v", err)
	}
	if back.Kind != TargetCapability || back.Code != "sign" {
		t.Fatalf("往返之后不一致: %+v", back)
	}

	// 两类之间不能串：`pipeline` 目标不该留下 `code`。
	rawPipe, err := marshalJSON(NewScheduleTargetPipeline("p-1"))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	var pipeBack ScheduleTarget
	if err := json.Unmarshal(rawPipe, &pipeBack); err != nil {
		t.Fatalf("反序列化失败: %v", err)
	}
	if pipeBack.Kind != TargetPipeline || pipeBack.ID != "p-1" || pipeBack.Code != "" {
		t.Fatalf("pipeline 目标不该带上 capability 的字段: %+v", pipeBack)
	}
}

// TestPipelineStepContinuationFallsBackToPipelineFlag 步骤级优先，
// 否则取组合级 `stop_on_error` 的**反值**。
func TestPipelineStepContinuationFallsBackToPipelineFlag(t *testing.T) {
	pipelineOf := func(stopOnError bool) Pipeline {
		p := NewPipeline("p-1", "轻量巡检")
		p.StopOnError = stopOnError
		return p
	}
	step := func(coe *bool) PipelineStep { return PipelineStep{Task: "sign", ContinueOnError: coe} }
	yes, no := true, false

	strict := pipelineOf(true)
	if strict.ContinuesAfterFailure(step(nil)) {
		t.Fatal("stopOnError=true 时默认不该继续")
	}
	if !strict.ContinuesAfterFailure(step(&yes)) {
		t.Fatal("步骤级可以覆盖组合级")
	}
	if strict.ContinuesAfterFailure(step(&no)) {
		t.Fatal("步骤级显式 false 应生效")
	}

	lenient := pipelineOf(false)
	if !lenient.ContinuesAfterFailure(step(nil)) {
		t.Fatal("stopOnError=false 时默认应继续")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 界面偏好 / 抢兑
// ─────────────────────────────────────────────────────────────────────────────

func TestWebuiConfigDefaultsAndPageWhitelist(t *testing.T) {
	d := NewWebuiConfig()
	if !d.RememberLastPage {
		t.Fatal("默认应记住上次页面")
	}
	if d.LastPage != DefaultWebuiPage {
		t.Fatalf("默认页实得 %q，期望 %q", d.LastPage, DefaultWebuiPage)
	}
	if d.LogExpanded || d.BackgroundRun {
		t.Fatal("默认都不该开")
	}

	// ⚠ 这是安全检查不只是整洁：`last_page` 会进前端路由，
	// 不收白名单就是一条不受限的路径输入。
	bogus := NewWebuiConfig()
	bogus.LastPage = "../../etc/passwd"
	bogus.NormalizePage()
	if bogus.LastPage != DefaultWebuiPage {
		t.Fatalf("非法页应回落默认，实得 %q", bogus.LastPage)
	}

	ok := NewWebuiConfig()
	ok.LastPage = "history"
	ok.NormalizePage()
	if ok.LastPage != "history" {
		t.Fatalf("白名单内的页不该被动，实得 %q", ok.LastPage)
	}
}

func TestNewExchangeConfigDefaultsToDisabledWithDefaultWindows(t *testing.T) {
	c := NewExchangeConfig()
	if c.Enabled {
		t.Fatal("抢兑默认必须是**关闭**的：它会真的扣云朵，默认开启等于一升级就自动花钱")
	}
	if len(c.Windows) != len(defaultExchangeWindows) {
		t.Fatalf("窗口数实得 %d，期望 %d", len(c.Windows), len(defaultExchangeWindows))
	}
	for i, w := range defaultExchangeWindows {
		if c.Windows[i] != w {
			t.Fatalf("窗口[%d] 实得 %q，期望 %q", i, c.Windows[i], w)
		}
	}
	// ⚠ 提前量是旧版实测得出的（开抢前 20 秒），别顺手改。
	if c.Windows[0] != "09:59:40" || c.Windows[1] != "11:59:40" {
		t.Fatalf("窗口时刻变了: %v", c.Windows)
	}
}

// TestEmptyCollectionsAreArraysNotJSONNull 空是 `[]` 不是 `null`。
//
// Rust 的 `Vec::new()` 序列化成 `[]`，Go 的 nil slice 是 `null` ——
// 前端 `.map()` 在 null 上直接炸。
func TestEmptyCollectionsAreArraysNotJSONNull(t *testing.T) {
	cases := map[string]any{
		"schedule.account_ids":        NewSchedule("s-1", "每日", "0 9 * * *", TZName),
		"pipeline.steps":              NewPipeline("p-1", "巡检"),
		"pipeline.account_ids":        NewPipeline("p-1", "巡检"),
		"exchange.bindings":           NewExchangeConfig(),
		"exchange.schedule_ids":       NewExchangeConfig(),
		"fieldspec.options-omitempty": NewFieldSpecText("k", "l"),
	}
	// ⚠ 只判**集合字段**是 `[]`。不能笼统地判"串里没有 null" ——
	// `updated_at` 这类 `Option` 字段本来就该是 `null`（见包注释那张表），
	// 把 null 一律禁掉会顺手把"Option 是 null 而不是省略"这条也一起改掉。
	wantField := map[string]string{
		"schedule.account_ids":  `"account_ids":[]`,
		"pipeline.steps":        `"steps":[]`,
		"pipeline.account_ids":  `"account_ids":[]`,
		"exchange.bindings":     `"bindings":[]`,
		"exchange.schedule_ids": `"schedule_ids":[]`,
	}
	for name, v := range cases {
		raw, err := marshalJSON(v)
		if err != nil {
			t.Fatalf("%s 序列化失败: %v", name, err)
		}
		s := string(raw)
		if strings.Contains(s, ":[null]") || strings.Contains(s, `"account_ids":null`) ||
			strings.Contains(s, `"steps":null`) || strings.Contains(s, `"bindings":null`) ||
			strings.Contains(s, `"schedule_ids":null`) {
			t.Fatalf("%s 空集合应是 [] 而不是 null: %s", name, s)
		}
		if want, ok := wantField[name]; ok && !strings.Contains(s, want) {
			t.Fatalf("%s 应产出 %s，实得 %s", name, want, s)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 枚举真值表（补 Go 相对 Rust 少掉的那道编译期保证）
// ─────────────────────────────────────────────────────────────────────────────

// assertEnumNames 逐条比对"值 ↔ 文本"，并复核已知态的数量与表长相等。
func assertEnumNames[T ~int](t *testing.T, label string, nameFn func(T) string, table []struct {
	value T
	text  string
}) {
	t.Helper()
	count := countKnownStates(t, label, func(i int) string { return nameFn(T(i)) })
	if count != len(table) {
		t.Fatalf("%s 有 %d 个已知态，真值表只有 %d 行 —— 新增的态没登记", label, count, len(table))
	}
	seen := map[string]bool{}
	for _, row := range table {
		if got := nameFn(row.value); got != row.text {
			t.Fatalf("%s(%d).String() = %q，期望 %q", label, int(row.value), got, row.text)
		}
		if seen[row.text] {
			t.Fatalf("%s: %q 重复出现", label, row.text)
		}
		seen[row.text] = true
	}
}

func TestGroupTruthTable(t *testing.T) {
	assertEnumNames(t, "Group", Group.String, []struct {
		value Group
		text  string
	}{
		{GroupDaily, "daily"},
		{GroupActivity, "activity"},
		{GroupQuery, "query"},
		{GroupExchange, "exchange"},
		{GroupMaintenance, "maintenance"},
	})
}

func TestCapabilityKindTruthTable(t *testing.T) {
	assertEnumNames(t, "CapabilityKind", CapabilityKind.String, []struct {
		value CapabilityKind
		text  string
	}{
		{KindAction, "action"},
		{KindQuery, "query"},
	})
	// ⚠ 未知值归 **Action**（有副作用那一档）而不是 Query：
	// 把查询误判成"有副作用"只是多写一条历史；反过来会把真有副作用的能力
	// 当成可随便重跑 —— 那会重复签到、重复兑换。
	if got := ParseCapabilityKind("nonsense"); got != KindAction {
		t.Fatalf("未知值实得 %v，期望 action", got)
	}
}

func TestRiskLevelTruthTable(t *testing.T) {
	assertEnumNames(t, "RiskLevel", RiskLevel.String, []struct {
		value RiskLevel
		text  string
	}{
		{RiskLow, "low"},
		{RiskMedium, "medium"},
		{RiskHigh, "high"},
	})
	// ⚠ 未知值归 **High**（要求确认那一档）：多一次确认的代价远小于
	// 把"兑换扣云朵"当成可随手点的按钮。
	if got := ParseRiskLevel("nonsense"); got != RiskHigh {
		t.Fatalf("未知值实得 %v，期望 high", got)
	}
	if !RiskHigh.NeedsConfirmation() || RiskLow.NeedsConfirmation() {
		t.Fatal("只有 high 需要二次确认")
	}
}

func TestAccountModeTruthTable(t *testing.T) {
	assertEnumNames(t, "AccountMode", AccountMode.String, []struct {
		value AccountMode
		text  string
	}{
		{ModeSequential, "sequential"},
		{ModeParallel, "parallel"},
	})
	// ⚠ 零值必须是**串行**：忘了指定时拿到"慢但安全"的那个。
	var zero AccountMode
	if zero != ModeSequential {
		t.Fatalf("零值实得 %v，期望 sequential（顺序不能调）", zero)
	}
	if got := ParseAccountMode("nonsense"); got != ModeSequential {
		t.Fatalf("未知值实得 %v，期望 sequential", got)
	}
}

func TestFieldKindTruthTable(t *testing.T) {
	assertEnumNames(t, "FieldKind", FieldKind.String, []struct {
		value FieldKind
		text  string
	}{
		{FieldText, "text"},
		{FieldNumber, "number"},
		{FieldBool, "bool"},
		{FieldSelect, "select"},
		// ⚠ `multi_select` **带下划线** —— snake_case 插分隔符，lowercase 不插。
		// 两个串只差一个字符，写错的话前端 `switch (kind)` 永远走 default 且不报错。
		{FieldMultiSelect, "multi_select"},
		{FieldJSON, "json"},
	})
}

func TestScheduleStatusTruthTable(t *testing.T) {
	assertEnumNames(t, "ScheduleStatus", ScheduleStatus.String, []struct {
		value ScheduleStatus
		text  string
	}{
		{ScheduleOK, "ok"},
		{ScheduleError, "error"},
		{ScheduleSkipped, "skipped"},
	})
	// ⚠ 未知值**不归任何默认值**（`ok` 与 `error` 都是错的答案）——
	// Rust 侧的判据："一个来路不明的状态会让界面显示错误的结论"。
	// 这条曾经被我写成"归 error"，且测试照着实现写、一起绿。
	if got, ok := ParseScheduleStatus("nonsense"); ok {
		t.Fatalf("未知值应当认不出来，实得 %v", got)
	}
	if got, ok := ParseScheduleStatus("error"); !ok || got != ScheduleError {
		t.Fatalf("error 应能读回，实得 %v ok=%v", got, ok)
	}
	if got, ok := ParseScheduleStatus("skipped"); !ok || got != ScheduleSkipped {
		t.Fatalf("skipped 应能读回，实得 %v ok=%v", got, ok)
	}
}

func TestMissPolicyTruthTable(t *testing.T) {
	assertEnumNames(t, "MissPolicy", MissPolicy.String, []struct {
		value MissPolicy
		text  string
	}{
		{MissCatchUp, "catch_up"},
		{MissSkip, "skip"},
	})
	// 与 [ParseScheduleStatus] 同一条纪律：来路不明的文本**不归默认值**。
	// 塞成 catch_up 的后果不是显示错误，而是"每次重启凭空多跑一趟真上游"。
	if got, ok := ParseMissPolicy("nonsense"); ok {
		t.Fatalf("未知值应当认不出来，实得 %v", got)
	}
	// ⚠ `skipped` 是 [ScheduleStatus] 那一族的词，这里必须认不出来
	// （两族共用一个解析函数 = 把"补跑策略"读成"上次执行结果"）。
	if got, ok := ParseMissPolicy("skipped"); ok {
		t.Fatalf("MissPolicy 没有 skipped 这一档，不该读出来：%v", got)
	}
	if got, ok := ParseMissPolicy("skip"); !ok || got != MissSkip {
		t.Fatalf("skip 应能读回，实得 %v ok=%v", got, ok)
	}
	// ⚠ `skip` 与 `catch_up` 只差在字面，但 `catch_up` **带下划线**：
	// 写成 `catchup` 时前端 `switch` 走 default 且不报错，与 FieldKind 那条同理。
	if got, ok := ParseMissPolicy("catchup"); ok {
		t.Fatalf("没有 catchup 这一档（正确字面是 catch_up），实得 %v", got)
	}
	if got, ok := ParseMissPolicy("catch_up"); !ok || got != MissCatchUp {
		t.Fatalf("catch_up 应能读回，实得 %v ok=%v", got, ok)
	}
}

// TestMissPolicyJSONIsTextNotNumber 钉住"对外是文本"这一条：
// Go 的 `type X int` 默认序列化成**数字**，而界面与库里都按文本走。
// 漏写 MarshalJSON 时只有这一条会红 —— String / Parse 那两侧照旧全绿。
func TestMissPolicyJSONIsTextNotNumber(t *testing.T) {
	skip := MissSkip
	data, err := MarshalJSON(skip)
	if err != nil {
		t.Fatalf("序列化 MissPolicy: %v", err)
	}
	if string(data) != `"skip"` {
		t.Fatalf("期望带引号的文本，实得 %s", data)
	}
	var back MissPolicy
	if err := json.Unmarshal(data, &back); err != nil {
		t.Fatalf("反序列化: %v", err)
	}
	if back != MissSkip {
		t.Fatalf("往返之后变了：%v", back)
	}
	// 数字形态必须**报错**：那是"库里被人手工填了 1"的形状，读成合法值等于猜意图
	if err := json.Unmarshal([]byte(`1`), &back); err == nil {
		t.Fatal("数字不该被接受")
	}
	if err := json.Unmarshal([]byte(`"nope"`), &back); err == nil {
		t.Fatal("未知文本该报错，而不是归默认值")
	}
}

func TestPipelineStatusTruthTable(t *testing.T) {
	assertEnumNames(t, "PipelineStatus", PipelineStatus.String, []struct {
		value PipelineStatus
		text  string
	}{
		{PipelineOK, "ok"},
		{PipelineError, "error"},
		{PipelinePartial, "partial"},
	})
	// ⚠ 第三态是 `partial` 而不是 `skipped` —— 与 ScheduleStatus 刻意不同，别合并。
	if PipelinePartial.String() == ScheduleSkipped.String() {
		t.Fatal("两族的第三态不该撞在一起")
	}
	if got, ok := ParsePipelineStatus("nonsense"); ok {
		t.Fatalf("未知值应当认不出来，实得 %v", got)
	}
	// ⚠ 两族第三态不同，所以 `skipped` 在组合这一列里必须**认不出来**
	// （合并两个解析函数会让它被读成合法值）。
	if got, ok := ParsePipelineStatus("skipped"); ok {
		t.Fatalf("组合没有 skipped 态，不该读出来：%v", got)
	}
	if got, ok := ParsePipelineStatus("partial"); !ok || got != PipelinePartial {
		t.Fatalf("partial 应能读回，实得 %v ok=%v", got, ok)
	}
}

func TestRunStatusIsTerminal(t *testing.T) {
	if StatusRunning.IsTerminal() {
		t.Fatal("running 不是终态")
	}
	if !StatusDone.IsTerminal() || !StatusFailed.IsTerminal() {
		t.Fatal("done 与 failed 都是终态")
	}
}

// TestFieldSpecBoolAlwaysWritesTheDefault 钉住"false 也要写出来"。
//
// `omitempty` 会把 `false` 当成"没有默认值"而丢掉 —— 而"默认关"恰恰是最常见的那种。
func TestFieldSpecBoolAlwaysWritesTheDefault(t *testing.T) {
	for _, def := range []bool{false, true} {
		raw, err := marshalJSON(NewFieldSpecBool("skipWait", "跳过等待", def))
		if err != nil {
			t.Fatalf("序列化失败: %v", err)
		}
		if !strings.Contains(string(raw), fmt.Sprintf(`"default":%t`, def)) {
			t.Fatalf("default=%t 没被写出来: %s", def, raw)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

func secretPtr(s string) *SecretValue { v := NewSecretValue(s); return &v }
func phonePtr(s string) *FullPhone    { v := FullPhone(s); return &v }
