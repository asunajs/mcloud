package core

// 本文件对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/legacy/tests/ 的 17 条测试，
// 外加 Go 侧新长出来的两批：
//   - 第九份黄金向量 `legacy-shape-vectors.json` 的消费者（形状比对）；
//   - `internal/golden.LoadFixture` 读真实夹具的键名认领断言。

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/golden"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// ⚠ Rust 侧用 `include_str!`（夹具缺失 = **编译失败**）。
// Go 没有跨目录的等价物：`//go:embed` 只能嵌本包目录下的文件，
// 而 `test/fixtures/` 是全仓共享资产（不可能为每个用到的包复制一份）。
//
// 代价是"夹具缺失"从编译期挪到了运行期。用 `t.Fatalf` 顶回去，
// 保证它**不会静默跳过断言** —— 这正是 `include_str!` 要达到的效果。
func loadFixture(t *testing.T, name string) string {
	t.Helper()
	raw, err := golden.LoadFixture("legacy/" + name)
	if err != nil {
		t.Fatalf("读夹具 legacy/%s 失败：%v", name, err)
	}
	return string(raw)
}

// loadLegacy 解析一段旧文档。
//
// ⚠ 走 [decodeJSONStrict] 而不是 `json.Unmarshal`：这一层有 19 位的
// `userDomainId`，默认解成 float64 会**静默丢精度**（§6.1 #2）。
func loadLegacy[T any](t *testing.T, text string) T {
	t.Helper()
	var value T
	if err := decodeJSONStrict([]byte(text), &value); err != nil {
		t.Fatalf("夹具解析失败：%v", err)
	}
	return value
}

// authFor 造一个能通过 [ParseAuth] 的 auth，用来在测试里构造任意手机号的账号。
func authFor(phone string) string {
	return "Basic " + B64Encode([]byte("2:"+phone+":ak|bk|ck|1893456000000|fk"))
}

// legacyAccount 造一条旧账号配置。
func legacyAccount(phone string, nickname *string) LegacyAccountConfig {
	return LegacyAccountConfig{Auth: authFor(phone), Nickname: nickname}
}

func fixtureViews(t *testing.T) []LegacyAccountView {
	t.Helper()
	config := loadLegacy[LegacyConfig](t, loadFixture(t, "asign.json"))
	return DescribeLegacyAccounts(config.Caiyun)
}

func ptr[T any](v T) *T { return &v }

// ─────────────────────────────────────────────────────────────────────────────
// 规整规则（对应 tests/normalize.rs）
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeWindowConversionFollowsOldRules `windowToCron`：
// **前导零会被 `Number()` 去掉**（`09` → `9`）。
func TestExchangeWindowConversionFollowsOldRules(t *testing.T) {
	cases := []struct{ in, want string }{
		{"09:59:40", "40 59 9 * * *"},
		{"00:00:00", "0 0 0 * * *"},
		{"23:59:59", "59 59 23 * * *"},
		// 旧版在校验前先 trim
		{" 09:59:40 ", "40 59 9 * * *"},
	}
	for _, c := range cases {
		got, err := ExchangeWindowToCron(c.in)
		if err != nil {
			t.Fatalf("%q 应合法，实得 %v", c.in, err)
		}
		if got != c.want {
			t.Fatalf("%q → 实得 %q，期望 %q", c.in, got, c.want)
		}
	}

	for _, bad := range []string{
		"9:59:40", "09:59", "09:59:40:00",
		"24:00:00", "09:60:00", "09:59:60",
		"", "abc",
	} {
		if _, err := ExchangeWindowToCron(bad); err == nil {
			t.Fatalf("%q 应被判为非法时刻", bad)
		}
	}
}

func TestNormalizeWindowsPadsDedupesAndSorts(t *testing.T) {
	windows := func(list ...string) []string {
		got, err := NormalizeExchangeWindows(list)
		if err != nil {
			return nil
		}
		return got
	}

	if got := windows("11:59:40", "09:59:40", "11:59:40"); !reflect.DeepEqual(got, []string{"09:59:40", "11:59:40"}) {
		t.Fatalf("去重并排序：实得 %v", got)
	}
	if got := windows("09:59"); !reflect.DeepEqual(got, []string{"09:59:00"}) {
		t.Fatalf("5 位补 :00：实得 %v", got)
	}
	if got := windows(" 09:59:40 ", ""); !reflect.DeepEqual(got, []string{"09:59:40"}) {
		t.Fatalf("空串被剔除：实得 %v", got)
	}

	for _, empty := range [][]string{{}, {""}} {
		_, err := NormalizeExchangeWindows(empty)
		if !LegacyErrorIs(err, LegacyErrNoExchangeWindow) {
			t.Fatalf("%v 应报 NoExchangeWindow，实得 %v", empty, err)
		}
	}
}

// TestPipelineAccountsFollowLegacyNormalization 组合的账号选择：
// 只有真数组才算数，空结果表示"全部账号"。
func TestPipelineAccountsFollowLegacyNormalization(t *testing.T) {
	// 废弃的单值字段 → 数组
	if got := NormalizePipelineAccounts(nil, ptr(any("账号1"))); !reflect.DeepEqual(got, []any{"账号1"}) {
		t.Fatalf("废弃单值字段：实得 %v", got)
	}
	// 去空、去重，保持首次出现顺序
	if got := NormalizePipelineAccounts([]any{"a", "", "a", nil}, nil); !reflect.DeepEqual(got, []any{"a"}) {
		t.Fatalf("去空去重：实得 %v", got)
	}
	// ★ 空数组会**盖掉**废弃字段（旧版先判 Array.isArray）
	if got := NormalizePipelineAccounts([]any{}, ptr(any("账号1"))); got != nil {
		t.Fatalf("空数组应盖掉废弃字段（结果 = 全部账号），实得 %v", got)
	}
	// 都没有 → 全部账号
	if got := NormalizePipelineAccounts(nil, nil); got != nil {
		t.Fatalf("都没给应是全部账号，实得 %v", got)
	}
	// 数字元素原样保留
	if got := NormalizePipelineAccounts([]any{json.Number("1"), json.Number("2")}, nil); !reflect.DeepEqual(got, []any{json.Number("1"), json.Number("2")}) {
		t.Fatalf("数字元素：实得 %v", got)
	}
}

// TestWebuiBooleanDefaultsPointInOppositeDirections 界面偏好：
// 两个布尔的默认方向相反 —— 这是旧版的真实行为。
func TestWebuiBooleanDefaultsPointInOppositeDirections(t *testing.T) {
	prefs := NormalizeWebui(&LegacyWebuiFile{})
	if !prefs.RememberLastPage {
		t.Fatal("缺省即 true（!== false）")
	}
	if prefs.LogExpanded {
		t.Fatal("缺省即 false（=== true）")
	}
	if prefs.BackgroundRun {
		t.Fatal("缺省即 false")
	}
	if prefs.LastPage != "tasks" {
		t.Fatalf("默认页应是 tasks，实得 %q", prefs.LastPage)
	}

	explicit := NormalizeWebui(&LegacyWebuiFile{
		RememberLastPage: ptr(false),
		LogExpanded:      ptr(true),
		LastPage:         ptr("history"),
	})
	if explicit.RememberLastPage {
		t.Fatal("显式 false 应生效")
	}
	if !explicit.LogExpanded {
		t.Fatal("显式 true 应生效")
	}
	if explicit.LastPage != "history" {
		t.Fatalf("白名单内的页应保留，实得 %q", explicit.LastPage)
	}

	// 白名单外的页面回落到默认
	bogus := NormalizeWebui(&LegacyWebuiFile{LastPage: ptr("../../etc/passwd")})
	if bogus.LastPage != "tasks" {
		t.Fatalf("白名单外的值必须回落，实得 %q", bogus.LastPage)
	}
}

func TestCapHistoryKeepsTheTail(t *testing.T) {
	runs := make([]int, 250)
	for i := range runs {
		runs[i] = i
	}
	CapHistory(&runs, LegacyHistoryCap)
	if len(runs) != 200 {
		t.Fatalf("应截到 200 条，实得 %d", len(runs))
	}
	if runs[0] != 50 || runs[len(runs)-1] != 249 {
		t.Fatalf("应保留尾段：首=%d 尾=%d", runs[0], runs[len(runs)-1])
	}

	small := []int{1, 2, 3}
	CapHistory(&small, LegacyHistoryCap)
	if !reflect.DeepEqual(small, []int{1, 2, 3}) {
		t.Fatalf("不足上限时不动，实得 %v", small)
	}
}

// TestParseEachIsolatesBrokenItems 一条坏记录不该拖垮整份文件。
func TestParseEachIsolatesBrokenItems(t *testing.T) {
	items := []any{
		map[string]any{"task": "sign", "account": "账号1", "start": "t", "end": "t"},
		json.Number("42"),
		map[string]any{"task": "prize", "account": "账号2"},
	}
	ok, bad := ParseEach[LegacyRun](items)
	if len(ok) != 2 {
		t.Fatalf("应救回 2 条，实得 %d", len(ok))
	}
	if ok[0].Task != "sign" {
		t.Fatalf("第一条应是 sign，实得 %q", ok[0].Task)
	}
	if len(bad) != 1 {
		t.Fatalf("应记录 1 条坏数据，实得 %d", len(bad))
	}
	if bad[0].Index != 1 {
		t.Fatalf("坏条目的下标要能定位，实得 %d", bad[0].Index)
	}
}

func TestLegacyStateReportsUnknownFieldsInsteadOfDroppingThem(t *testing.T) {
	var state LegacyAccountState
	if err := decodeJSONStrict([]byte(`{"jwtToken":"j","totalCloud":123}`), &state); err != nil {
		t.Fatalf("解析失败：%v", err)
	}
	if derefOrEmpty(state.JWTToken) != "j" {
		t.Fatalf("已知字段应读到，实得 %v", state.JWTToken)
	}
	if got := state.UnknownFields(); !reflect.DeepEqual(got, []string{"totalCloud"}) {
		t.Fatalf("旧版本写过的字段应被报告，而不是静默丢弃：实得 %v", got)
	}
	if state.IsEmpty() {
		t.Fatal("有 jwtToken 就不该算空")
	}
	if !(LegacyAccountState{}).IsEmpty() {
		t.Fatal("全空状态应为空")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 标识映射（对应 tests/identity.rs）
// ─────────────────────────────────────────────────────────────────────────────

// TestMaskCollisionAbortsMigration 掩码撞车必须**中止迁移**：
// 两个账号会塌缩成一个 [AccountID]，且不会报错。
func TestMaskCollisionAbortsMigration(t *testing.T) {
	views := DescribeLegacyAccounts([]LegacyAccountConfig{
		legacyAccount("19000000001", ptr("甲")),
		legacyAccount("19100000001", ptr("乙")),
	})
	// 两个号码前 2 后 2 都是 19 / 01
	if string(views[0].AccountID) != "19****01" || string(views[1].AccountID) != "19****01" {
		t.Fatalf("掩码应撞车：%s / %s", views[0].AccountID, views[1].AccountID)
	}

	_, err := BuildIdentityMap(views)
	if !LegacyErrorIs(err, LegacyErrMaskCollision) {
		t.Fatalf("应报掩码冲突，实得 %v", err)
	}
	var le *LegacyError
	if !errorsAsLegacy(err, &le) {
		t.Fatalf("应取得出 *LegacyError：%v", err)
	}
	if le.AccountID != "19****01" {
		t.Fatalf("冲突的 id 应是 19****01，实得 %q", le.AccountID)
	}
	if !reflect.DeepEqual(le.Keys, []string{"19000000001", "19100000001"}) {
		t.Fatalf("冲突来源应是两个键，实得 %v", le.Keys)
	}
}

// TestMaskCollisionReportsTheLexicographicallyFirstGroup **两组**冲突时，
// 报的必须是**字典序靠前**的那一组。
//
// 这条存在的理由是一处 Rust → Go 差异：
// Rust 侧 `sources` 是 `BTreeMap`，"找到的第一个冲突"天然是字典序第一个；
// 而 **Go 的 map 遍历顺序是随机的**（语言规范刻意如此）。
// 不显式排序的话，报错内容会**每次运行都不同** —— 而它是要进迁移报告给人看的。
//
// ⚠ 这条断言一开始**没有承重**：只造一组冲突时，排不排序报的都是它
// （反向测试：去掉 `sort.Strings(ids)` 后这组用例照样绿）。
// 必须是**两组以上**才能把"顺序"这件事问出来。
func TestMaskCollisionReportsTheLexicographicallyFirstGroup(t *testing.T) {
	views := DescribeLegacyAccounts([]LegacyAccountConfig{
		legacyAccount("19000000001", ptr("甲")), // 19****01 ┐ 第一组
		legacyAccount("19100000001", ptr("乙")), // 19****01 ┘
		legacyAccount("19200000002", ptr("丙")), // 19****02 ┐ 第二组
		legacyAccount("19300000002", ptr("丁")), // 19****02 ┘
	})

	_, err := BuildIdentityMap(views)
	if !LegacyErrorIs(err, LegacyErrMaskCollision) {
		t.Fatalf("应报掩码冲突，实得 %v", err)
	}
	var le *LegacyError
	if !errorsAsLegacy(err, &le) {
		t.Fatalf("应取得出 *LegacyError：%v", err)
	}
	if le.AccountID != "19****01" {
		t.Fatalf("两组冲突时应报字典序靠前的那一组，实得 %q", le.AccountID)
	}
	if !reflect.DeepEqual(le.Keys, []string{"19000000001", "19100000001"}) {
		t.Fatalf("冲突来源应是第一组的两个键，实得 %v", le.Keys)
	}
}

func TestIdentityMapIsBuiltForDistinctMasks(t *testing.T) {
	views := DescribeLegacyAccounts([]LegacyAccountConfig{
		legacyAccount("19000000001", ptr("甲")),
		legacyAccount("19100000002", ptr("乙")),
	})
	mapOut, err := BuildIdentityMap(views)
	if err != nil {
		t.Fatalf("掩码不同应能建立映射：%v", err)
	}
	if string(mapOut["19000000001"]) != "19****01" {
		t.Fatalf("实得 %q", mapOut["19000000001"])
	}
	if string(mapOut["19100000002"]) != "19****02" {
		t.Fatalf("实得 %q", mapOut["19100000002"])
	}
}

// TestAccountsWithoutPhoneFallBackToNickname 没有手机号的账号退化为昵称
// （旧 `accountKey` 的兜底链）。
func TestAccountsWithoutPhoneFallBackToNickname(t *testing.T) {
	views := DescribeLegacyAccounts([]LegacyAccountConfig{
		{Auth: "!!!not-base64!!!", Nickname: ptr("备用账号")},
		{Auth: ""},
	})
	if views[0].Phone != "" {
		t.Fatalf("解不出手机号应为空串，实得 %q", views[0].Phone)
	}
	if views[0].LegacyKey != "备用账号" || string(views[0].AccountID) != "备用账号" || views[0].Label != "备用账号" {
		t.Fatalf("应退化到昵称：key=%q id=%q label=%q",
			views[0].LegacyKey, views[0].AccountID, views[0].Label)
	}
	if views[1].LegacyKey != "account" || string(views[1].AccountID) != "account" || views[1].Label != "账号" {
		t.Fatalf("应退化到兜底值：key=%q id=%q label=%q",
			views[1].LegacyKey, views[1].AccountID, views[1].Label)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 账号引用（对应 tests/references.rs）
// ─────────────────────────────────────────────────────────────────────────────

// TestLegacyTargetResolutionReproducesFindAccount 旧 `findAccount`：
// **纯数字字符串按序号解释**，手机号字符串永远匹配不到。
func TestLegacyTargetResolutionReproducesFindAccount(t *testing.T) {
	views := fixtureViews(t)
	resolve := func(v any) LegacyTarget { return ResolveLegacyTarget(views, &v) }

	if got := ResolveLegacyTarget(views, nil); got.Kind != LegacyTargetAll {
		t.Fatalf("未指定应是全部账号，实得 %v", got)
	}

	cases := []struct {
		in   any
		kind LegacyTargetKind
		idx  int
		raw  string
	}{
		{json.Number("1"), LegacyTargetIndex, 1, ""},
		{json.Number("2"), LegacyTargetIndex, 2, ""},
		{"1", LegacyTargetIndex, 1, ""}, // 纯数字串 = 序号
		{"账号1", LegacyTargetNamed, 2, ""},
		{"账号2", LegacyTargetNamed, 1, ""},
		// 越界序号
		{json.Number("0"), LegacyTargetUnresolved, 0, "0"},
		{json.Number("3"), LegacyTargetUnresolved, 0, "3"},
		{json.Number("-1"), LegacyTargetUnresolved, 0, "-1"},
		// ★ 手机号字符串：被当成序号（19 亿），必然越界 —— 这就是那个陷阱
		{"19900000002", LegacyTargetUnresolved, 0, "19900000002"},
		{"", LegacyTargetUnresolved, 0, ""},
		{"不存在", LegacyTargetUnresolved, 0, "不存在"},
	}
	for _, c := range cases {
		got := resolve(c.in)
		if got.Kind != c.kind {
			t.Fatalf("%v：kind 实得 %d，期望 %d", c.in, got.Kind, c.kind)
		}
		switch c.kind {
		case LegacyTargetIndex, LegacyTargetNamed:
			if got.Index != c.idx {
				t.Fatalf("%v：序号实得 %d，期望 %d", c.in, got.Index, c.idx)
			}
		case LegacyTargetUnresolved:
			if got.Raw != c.raw {
				t.Fatalf("%v：原文实得 %q，期望 %q", c.in, got.Raw, c.raw)
			}
		}
	}

	// 其它 JSON 类型按 `Value::to_string()` 的形态记原文
	if got := resolve(true); got.Kind != LegacyTargetUnresolved || got.Raw != "true" {
		t.Fatalf("布尔应记 true，实得 %+v", got)
	}
}

// TestAdviceFlagsReferencesThatNeverTookEffect 迁移建议与旧语义分开表达：
// 能对应上的会**明确标出这是一次行为变更**。
func TestAdviceFlagsReferencesThatNeverTookEffect(t *testing.T) {
	views := fixtureViews(t)
	advise := func(v any) ReferenceAdvice { return AdviseReference(views, &v) }

	if got := AdviseReference(views, nil); got.Kind != AdviceAllAccounts {
		t.Fatalf("未指定应是全部账号，实得 %+v", got)
	}
	if got := advise("账号1"); got.Kind != AdviceSame || got.Index != 2 {
		t.Fatalf("昵称命中：实得 %+v", got)
	}
	if got := advise(json.Number("1")); got.Kind != AdviceSame || got.Index != 1 {
		t.Fatalf("序号命中：实得 %+v", got)
	}
	if got := advise("19900000002"); got.Kind != AdviceRecoveredFromPhone ||
		got.Digits != "19900000002" || got.Index != 2 {
		t.Fatalf("越界的数字串若是已知手机号，应给出可恢复的建议：实得 %+v", got)
	}
	if got := advise("12345678901"); got.Kind != AdviceWasNeverEffective || got.Raw != "12345678901" {
		t.Fatalf("不认识的数字串不能猜：实得 %+v", got)
	}

	// 明确的分支才给结果；拿不准的一律让调用方去报告
	if id, has, bad := ResolveReferenceStrict(views, ptr(any("19900000002"))); bad != nil ||
		!has || string(id) != "19****02" {
		t.Fatalf("可恢复的引用应给出 id：id=%s has=%v bad=%+v", id, has, bad)
	}
	if _, _, bad := ResolveReferenceStrict(views, ptr(any("12345678901"))); bad == nil {
		t.Fatal("拿不准的引用必须返回 bad")
	}
	if _, has, bad := ResolveReferenceStrict(views, nil); bad != nil || has {
		t.Fatalf("未指定应是全部账号：has=%v bad=%+v", has, bad)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具端到端（对应 tests/fixtures.rs）
// ─────────────────────────────────────────────────────────────────────────────

// TestFixtureConfigDerivesIdentityFromAuth 账号配置 → 视图：
// 手机号来自 auth 现场解析，标识走掩码。
func TestFixtureConfigDerivesIdentityFromAuth(t *testing.T) {
	config := loadLegacy[LegacyConfig](t, loadFixture(t, "asign.json"))
	if config.Version == nil || *config.Version != 2 {
		t.Fatalf("旧配置是 version 2，实得 %v", config.Version)
	}
	if len(config.Caiyun) != 2 {
		t.Fatalf("夹具应有 2 个账号，实得 %d", len(config.Caiyun))
	}

	views := DescribeLegacyAccounts(config.Caiyun)
	// 真实配置里**没有** phone 字段，号码只能从 auth 里解 —— 这正是 ResolvePhone 的价值
	for i, v := range views {
		if v.ConfigPhone != nil {
			t.Fatalf("第 %d 个账号不该有配置里的 phone，实得 %v", i, *v.ConfigPhone)
		}
	}

	if derefOrEmpty(views[0].Nickname) != "账号2" {
		t.Fatalf("实得 %q", derefOrEmpty(views[0].Nickname))
	}
	if views[0].Phone != "19900000001" {
		t.Fatalf("实得 %q", views[0].Phone)
	}
	if views[0].Label != "账号2" {
		t.Fatalf("有昵称时展示名就是昵称，实得 %q", views[0].Label)
	}
	if views[0].LegacyKey != "19900000001" {
		t.Fatalf("实得 %q", views[0].LegacyKey)
	}
	if string(views[0].AccountID) != "19****01" {
		t.Fatalf("实得 %q", views[0].AccountID)
	}

	if derefOrEmpty(views[1].Nickname) != "账号1" || views[1].Phone != "19900000002" ||
		string(views[1].AccountID) != "19****02" {
		t.Fatalf("第二个账号：%+v", views[1])
	}

	if derefOrEmpty(views[0].Platform) != "2" {
		t.Fatalf("平台号来自 auth 第 1 段，实得 %v", views[0].Platform)
	}

	// 序号从 1 起，与旧 findAccount 一致
	if views[0].Index != 1 || views[1].Index != 2 {
		t.Fatalf("序号应从 1 起：%d / %d", views[0].Index, views[1].Index)
	}

	// 夹具的 expire 刻意放在未来，免得这份测试随时间变成"凭据过期"。
	// 这里同时钉住**量级**：真实数据是 13 位毫秒，写成 10 位秒值会让下游整条链路的
	// 单位假设悄悄错掉（见 auth.go 包注释）。
	for i, v := range views {
		if v.AuthExpire <= 1_700_000_000_000 {
			t.Fatalf("第 %d 个账号的 expire 应在未来，实得 %d", i, v.AuthExpire)
		}
		if len(itoa64(v.AuthExpire)) != 13 {
			t.Fatalf("第 %d 个账号的 expire 应是 13 位毫秒，实得 %d", i, v.AuthExpire)
		}
	}
}

// TestFixtureStateKeysMapOntoTheSameAccountIDs **跨文件**：
// `state.json` 的键是手机号原文，必须能映射到与 `asign.json` 相同的 [AccountID]。
// 这条挂了就意味着历史凭据会挂到错误的账号上。
func TestFixtureStateKeysMapOntoTheSameAccountIDs(t *testing.T) {
	state := loadLegacy[LegacyStateFile](t, loadFixture(t, "state.json"))
	views := fixtureViews(t)
	mapOut, err := BuildIdentityMap(views)
	if err != nil {
		t.Fatalf("夹具内不应有掩码冲突：%v", err)
	}

	if len(state.Accounts) != 2 || len(mapOut) != 2 {
		t.Fatalf("state=%d map=%d", len(state.Accounts), len(mapOut))
	}

	derived := make([]string, 0, len(views))
	for _, v := range views {
		derived = append(derived, string(v.AccountID))
	}
	mapped := make([]string, 0, len(state.Accounts))
	for k := range state.Accounts {
		mapped = append(mapped, string(mapOut[k]))
	}
	sortStrings(derived)
	sortStrings(mapped)
	if !reflect.DeepEqual(derived, mapped) {
		t.Fatalf("state.json 的键应恰好覆盖全部账号：\n实得 %v\n期望 %v", derived, mapped)
	}

	// 四个状态字段都要能读出来；未识别字段应为空（夹具里不该有）
	for key, accountState := range state.Accounts {
		if accountState.IsEmpty() {
			t.Fatalf("%s 的状态不该是空的", key)
		}
		if unknown := accountState.UnknownFields(); len(unknown) != 0 {
			t.Fatalf("%s 出现了未识别字段 %v", key, unknown)
		}
		if len(derefOrEmpty(accountState.JWTToken)) <= 500 {
			t.Fatalf("%s 的 jwtToken 应像真实数据（>500 字符），实得 %d",
				key, len(derefOrEmpty(accountState.JWTToken)))
		}
		if len(derefOrEmpty(accountState.DeviceID)) <= 80 {
			t.Fatalf("%s 的 deviceId 应像真实数据（>80 字符），实得 %d",
				key, len(derefOrEmpty(accountState.DeviceID)))
		}
		if accountState.UserDomainID == nil {
			t.Fatalf("%s 应有 userDomainId", key)
		}
	}
}

// TestFixtureHistoryLabelsEachResolveToOneAccount **跨文件**：
// 历史里的展示名必须唯一对应一个账号。
// 展示名撞车（两个账号同名）会让历史无法归属 —— 这里是发现它的地方。
func TestFixtureHistoryLabelsEachResolveToOneAccount(t *testing.T) {
	history := loadLegacy[LegacyHistoryFile](t, loadFixture(t, "history.json"))
	if len(history.Runs) != 12 {
		t.Fatalf("夹具只保留最后 12 条，实得 %d", len(history.Runs))
	}

	views := fixtureViews(t)
	for _, run := range history.Runs {
		if run.Task == "" {
			t.Fatal("每条历史都应记下任务名")
		}
		if hits := FindByLabel(views, run.Account); len(hits) != 1 {
			t.Fatalf("展示名 %q 应唯一对应一个账号，实际命中 %d", run.Account, len(hits))
		}
	}

	found := false
	for _, r := range history.Runs {
		if r.Task == "auth-refresh" {
			found = true
		}
	}
	if !found {
		t.Fatal("夹具里应有 auth-refresh 记录")
	}
}

// TestFixtureLogEntriesKeepLevelAndTypeSeparately 日志条目同时保留数值 `level`
// 与语义 `type`，且**同一个 level 对应多个 type**。
func TestFixtureLogEntriesKeepLevelAndTypeSeparately(t *testing.T) {
	history := loadLegacy[LegacyHistoryFile](t, loadFixture(t, "history.json"))

	type combo struct {
		level int64
		kind  string
	}
	seen := make(map[combo]bool)
	for _, run := range history.Runs {
		for _, entry := range run.Logs {
			if entry.Msg == nil {
				t.Fatal("日志条目应有 msg")
			}
			if entry.Date == nil {
				t.Fatal("日志条目应有 date")
			}
			if entry.Level != nil && entry.Kind != nil {
				seen[combo{*entry.Level, *entry.Kind}] = true
			}
		}
	}

	if !seen[combo{3, "info"}] {
		t.Fatal("应有 (3, info)")
	}
	if !seen[combo{0, "error"}] {
		t.Fatal("应有 (0, error)")
	}
	// 这是本测试存在的理由：level 3 之下不止一种语义，只看 level 会丢掉 start/success
	level3 := 0
	for c := range seen {
		if c.level == 3 {
			level3++
		}
	}
	if level3 < 2 {
		t.Fatalf("level=3 应对应多个 type，实际只有 %d 种", level3)
	}
}

// TestFixturePipelineMigratesLegacyAccountField 组合：夹具里没给账号字段
// → 语义是"全部账号"，而不是"没有账号"。
func TestFixturePipelineMigratesLegacyAccountField(t *testing.T) {
	file := loadLegacy[LegacyPipelinesFile](t, loadFixture(t, "pipelines.json"))
	if len(file.Pipelines) != 1 {
		t.Fatalf("夹具应有 1 个组合，实得 %d", len(file.Pipelines))
	}
	p := file.Pipelines[0]

	if p.Name != "轻量巡检" {
		t.Fatalf("实得 %q", p.Name)
	}
	if len(p.Steps) != 2 {
		t.Fatalf("应有 2 步，实得 %d", len(p.Steps))
	}
	if p.Steps[0].Task != "prizes" || p.Steps[1].Task != "receive" {
		t.Fatalf("步骤应是 prizes/receive，实得 %q / %q", p.Steps[0].Task, p.Steps[1].Task)
	}
	if p.StopOnError == nil || !*p.StopOnError {
		t.Fatalf("stopOnError 应是 true，实得 %v", p.StopOnError)
	}
	if derefOrEmpty(p.LastStatus) != "ok" {
		t.Fatalf("lastStatus 应是 ok，实得 %v", p.LastStatus)
	}
	if p.LastDurationMS == nil || *p.LastDurationMS != 2481 {
		t.Fatalf("lastDurationMs 应是 2481，实得 %v", p.LastDurationMS)
	}

	if p.Accounts != nil || p.LegacyAccount != nil {
		t.Fatal("夹具里没给账号字段")
	}
	if got := NormalizePipelineAccounts(nil, p.LegacyAccount); got != nil {
		t.Fatalf("没给账号 = 全部账号，实得 %v", got)
	}
}

// TestFixtureExchangeWindowsBecomeSixFieldCron 抢兑：窗口按旧规则转成 6 段 cron；
// 夹具里 `bindings` 为空但字段必须都在。
func TestFixtureExchangeWindowsBecomeSixFieldCron(t *testing.T) {
	exchange := loadLegacy[LegacyExchangeFile](t, loadFixture(t, "exchange.json"))
	if exchange.Enabled {
		t.Fatal("夹具里抢兑是关闭的")
	}
	if len(exchange.Bindings) != 0 {
		t.Fatalf("bindings 应为空，实得 %d", len(exchange.Bindings))
	}
	if len(exchange.ScheduleIDs) != 0 {
		t.Fatalf("scheduleIds 应为空，实得 %d", len(exchange.ScheduleIDs))
	}
	if exchange.UpdatedAt == nil {
		t.Fatal("updatedAt 字段应被识别（不是掉进 Unknown）")
	}

	windows, err := NormalizeExchangeWindows(exchange.Windows)
	if err != nil {
		t.Fatalf("夹具窗口应合法：%v", err)
	}
	if !reflect.DeepEqual(windows, []string{"09:59:40", "11:59:40"}) {
		t.Fatalf("实得 %v", windows)
	}
	crons := make([]string, 0, len(windows))
	for _, w := range windows {
		c, err := ExchangeWindowToCron(w)
		if err != nil {
			t.Fatalf("%q 应合法：%v", w, err)
		}
		crons = append(crons, c)
	}
	if !reflect.DeepEqual(crons, []string{"40 59 9 * * *", "40 59 11 * * *"}) {
		t.Fatalf("实得 %v", crons)
	}
}

func TestFixtureSchedulesFileParsesEvenWhenEmpty(t *testing.T) {
	file := loadLegacy[LegacySchedulesFile](t, loadFixture(t, "schedules.json"))
	if len(file.Schedules) != 0 {
		t.Fatalf("真实数据里定时任务列表为空，实得 %d", len(file.Schedules))
	}
}

func TestFixtureWebuiNormalizesWithLegacyDefaults(t *testing.T) {
	prefs := NormalizeWebui(loadLegacyPtr[LegacyWebuiFile](t, loadFixture(t, "webui.json")))
	if !prefs.RememberLastPage {
		t.Fatal("rememberLastPage 应为 true")
	}
	if prefs.LastPage != "tasks" {
		t.Fatalf("lastPage 实得 %q", prefs.LastPage)
	}
	if prefs.LogExpanded || prefs.BackgroundRun {
		t.Fatal("logExpanded / backgroundRun 应为 false")
	}
	if prefs.UpdatedAt == nil {
		t.Fatal("updatedAt 应解析得出来")
	}
}

// TestFixturePhoneAssignmentAgreesWithDerivedPhones 夹具声明的"分配了哪些脱敏号码"
// 必须与我们从 `asign.json` 推出来的号码一致。
//
// 否则说明夹具与它的元信息脱节了 —— 那是最糟的情况：测试还在过，但依据已经错了。
func TestFixturePhoneAssignmentAgreesWithDerivedPhones(t *testing.T) {
	var meta struct {
		PhoneAssignment struct {
			Assigned []string `json:"$assigned"`
		} `json:"$phoneAssignment"`
		SourceSha256 map[string]string `json:"$sourceSha256"`
	}
	if err := decodeJSONStrict([]byte(loadFixture(t, "$meta.json")), &meta); err != nil {
		t.Fatalf("解析 $meta.json 失败：%v", err)
	}
	if len(meta.PhoneAssignment.Assigned) == 0 {
		t.Fatal("$meta.json 应有 $phoneAssignment.$assigned")
	}

	views := fixtureViews(t)
	declared := append([]string(nil), meta.PhoneAssignment.Assigned...)
	derived := make([]string, 0, len(views))
	for _, v := range views {
		derived = append(derived, v.Phone)
	}
	sortStrings(declared)
	sortStrings(derived)
	if !reflect.DeepEqual(declared, derived) {
		t.Fatalf("夹具声明的号码应与推出来的号码一致：\n声明 %v\n实得 %v", declared, derived)
	}

	if _, ok := meta.SourceSha256["history.json"]; !ok {
		t.Fatal("元信息里应记下各源文件的 sha256")
	}
}

// TestFixturesContainNoRealIdentifiers 夹具里**不得**出现真实标识。
//
// 这条是给"元信息/注释里顺手写下原值"这类泄漏兜底的。它会把 base64
// （`auth` 字段）里的载荷解码后再查一遍 —— 只扫字面正是历史上漏过的那次。
func TestFixturesContainNoRealIdentifiers(t *testing.T) {
	for _, name := range []string{
		"$meta.json", "asign.json", "state.json", "history.json",
		"pipelines.json", "schedules.json", "exchange.json", "webui.json",
	} {
		assertOnlySyntheticIdentifiers(t, name, loadFixture(t, name))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第九份黄金向量：形状比对
// ─────────────────────────────────────────────────────────────────────────────

type legacyShapeVectors struct {
	SerializedShapes []struct {
		Type   string `json:"type"`
		Empty  string `json:"empty"`
		Filled string `json:"filled"`
	} `json:"$serializedShapes"`
	FixtureKeys []struct {
		File  string   `json:"file"`
		Paths []string `json:"paths"`
	} `json:"$fixtureKeys"`
	PipelineStepEncode []struct {
		Label string `json:"label"`
		JSON  string `json:"json"`
	} `json:"$pipelineStepEncode"`
	PipelineStepDecode []struct {
		Label      string  `json:"label"`
		Input      string  `json:"input"`
		OK         bool    `json:"ok"`
		Error      string  `json:"error"`
		Task       string  `json:"task"`
		BodyIsNone bool    `json:"bodyIsNone"`
		BodyText   *string `json:"bodyText"`
		CoeIsNone  bool    `json:"coeIsNone"`
	} `json:"$pipelineStepDecode"`
}

func loadLegacyShapeVectors(t *testing.T) legacyShapeVectors {
	t.Helper()
	raw, err := golden.Load("legacy-shape-vectors.json")
	if err != nil {
		t.Fatalf("读第九份黄金向量失败：%v", err)
	}
	var v legacyShapeVectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatalf("解析失败：%v", err)
	}
	if len(v.SerializedShapes) == 0 || len(v.FixtureKeys) == 0 {
		t.Fatalf("向量缺段：shapes=%d fixtureKeys=%d", len(v.SerializedShapes), len(v.FixtureKeys))
	}
	if len(v.PipelineStepEncode) == 0 || len(v.PipelineStepDecode) == 0 {
		t.Fatalf("向量缺段：stepEncode=%d stepDecode=%d",
			len(v.PipelineStepEncode), len(v.PipelineStepDecode))
	}
	return v
}

// TestLegacySerializedShapesMatchSerdeVectors 逐类型比对**整串**形状。
//
// 这条接的是本模块最要紧的那个洞：旧文档的所有字段都带 `#[serde(default)]`
// ⇒ **字段名写错不会报错，只会静默读到零值**。
//
// 期望值由**真实的 mcloud-core**（path 依赖，非镜像）的 serde 产出，
// 所以它是"参考实现说形状是什么"，不是"我们把规则又推了一遍"。
//
// ⚠ 两条串缺一不可：
//   - `empty` 钉住"空集合是 `[]` 还是 `null`"、`Option` 是 `null` 还是字段消失；
//   - `filled` 钉住字段**顺序**（逐字段比对会把它整个放过）。
func TestLegacySerializedShapesMatchSerdeVectors(t *testing.T) {
	v := loadLegacyShapeVectors(t)

	// ⚠ 填充 JSON 与生成器里的**逐字相同** —— 它是这条断言的输入，
	// 两边不一致时"填充后仍是零值"，失败的会是形状比对而不是填充本身。
	filled := map[string]string{
		"LegacyConfig":        `{"version":2,"caiyun":[{"auth":"Basic x","nickname":"账号1"}]}`,
		"LegacyAccountConfig": `{"auth":"Basic x","nickname":"账号1","phone":"19900000001"}`,
		"LegacyLogEntry":      `{"level":3,"type":"info","msg":"m","date":"d"}`,
		"LegacyStep":          `{"task":"prizes","body":{"b":1},"continueOnError":true,"account":"旧单值"}`,
		"LegacyBinding":       `{"account":1,"prizeId":7,"prizeName":"云朵"}`,
		"LegacyRun":           `{"task":"sign","account":"账号1","ok":true,"start":"2026-09-16T12:00:00.000Z","end":"2026-09-16T12:00:01.000Z","durationMs":1000,"logs":[{"level":3,"type":"info","msg":"m","date":"d"}],"result":{"a":1},"error":"e"}`,
		"LegacyPipeline":      `{"id":"p-1","name":"n","description":"d","steps":[{"task":"prizes"}],"account":"旧单值","accounts":["a","b"],"accountMode":"parallel","stopOnError":true,"createdAt":"c","updatedAt":"u","lastRunAt":"l","lastStatus":"ok","lastError":"e","lastDurationMs":2481}`,
		"LegacySchedule":      `{"id":"s-1","name":"n","task":"sign","pipelineId":"p-1","cron":"0 9 * * *","timezone":"Asia/Shanghai","account":1,"body":{"b":1},"enabled":true,"createdAt":"c","lastRunAt":"l","lastStatus":"ok","lastError":"e"}`,
	}

	for _, shape := range v.SerializedShapes {
		t.Run(shape.Type, func(t *testing.T) {
			// 空实例：与 Rust 侧同路 —— 由 `{}` 解出来，而不是手写零值结构体。
			empty := legacyZeroValue(t, shape.Type)
			if got := mustMarshalLegacy(t, empty); got != shape.Empty {
				t.Fatalf("空实例与 serde 的输出不一致:\n实得 %s\n期望 %s", got, shape.Empty)
			}

			src, ok := filled[shape.Type]
			if !ok {
				t.Fatalf("向量里的 %s 没有对应的填充 JSON —— 补上，别删向量", shape.Type)
			}
			gotFilled := mustMarshalLegacy(t, legacyDecodeInto(t, shape.Type, src))
			if gotFilled != shape.Filled {
				t.Fatalf("填充实例与 serde 的输出不一致:\n实得 %s\n期望 %s", gotFilled, shape.Filled)
			}
		})
	}
}

// legacyZeroValue 由 `{}` 解出某个 legacy 类型的"全默认"实例。
//
// ⚠ 走 `{}` 而不是手写零值结构体：Rust 侧也是这么做的
// （这批类型大多没有 derive `Default`，靠 `#[serde(default)]` 让 `{}` 可解）。
// 手写零值会让人以为"默认值是什么"在这里有第二份定义。
func legacyZeroValue(t *testing.T, typeName string) any {
	t.Helper()
	return legacyDecodeInto(t, typeName, `{}`)
}

func legacyDecodeInto(t *testing.T, typeName, src string) any {
	t.Helper()
	switch typeName {
	case "LegacyConfig":
		return loadLegacyPtrWith(t, &LegacyConfig{}, src)
	case "LegacyAccountConfig":
		return loadLegacyPtrWith(t, &LegacyAccountConfig{}, src)
	case "LegacyRun":
		return loadLegacyPtrWith(t, &LegacyRun{}, src)
	case "LegacyLogEntry":
		return loadLegacyPtrWith(t, &LegacyLogEntry{}, src)
	case "LegacyPipeline":
		return loadLegacyPtrWith(t, &LegacyPipeline{}, src)
	case "LegacyStep":
		return loadLegacyPtrWith(t, &LegacyStep{}, src)
	case "LegacySchedule":
		return loadLegacyPtrWith(t, &LegacySchedule{}, src)
	case "LegacyBinding":
		return loadLegacyPtrWith(t, &LegacyBinding{}, src)
	}
	t.Fatalf("未知的 legacy 类型 %q", typeName)
	return nil
}

func loadLegacyPtrWith[T any](t *testing.T, dst *T, src string) any {
	t.Helper()
	if err := decodeJSONStrict([]byte(src), dst); err != nil {
		t.Fatalf("解析 %q 失败：%v", src, err)
	}
	return dst
}

func loadLegacyPtr[T any](t *testing.T, text string) *T {
	t.Helper()
	var v T
	if err := decodeJSONStrict([]byte(text), &v); err != nil {
		t.Fatalf("夹具解析失败：%v", err)
	}
	return &v
}

func mustMarshalLegacy(t *testing.T, v any) string {
	t.Helper()
	raw, err := marshalJSON(v)
	if err != nil {
		t.Fatalf("序列化失败：%v", err)
	}
	return string(raw)
}

// TestLegacyFieldTagsCoverEveryKeySeenInFixtures 夹具里出现的**每一个键**
// 都必须被 Go 侧的 json tag 认领。
//
// 这是 [TestLegacySerializedShapesMatchSerdeVectors] 的补充，也是它补不到的那一半：
// 上面那条只对**能序列化**的 8 个类型生效，而 `LegacyStateFile` /
// `LegacyWebuiFile` 这些只反序列化的类型没有"输出形状"可比 ——
// 恰恰是它们最要紧（`state.json` 的键就是凭据的归属依据）。
//
// 所以用反射读 tag：tag 是"Go 侧认识的键名"，`$fixtureKeys` 是
// "真实数据里出现过的键名"，两边一比就知道有没有写错或漏写。
//
// ⚠ 方向是"夹具的键 ⊆ 认识的键"，不是相等：
// 类型里可以有夹具没用到的字段（`#[serde(default)]` 允许），反之则不行。
func TestLegacyFieldTagsCoverEveryKeySeenInFixtures(t *testing.T) {
	v := loadLegacyShapeVectors(t)

	// 路径前缀 → 该层应该用哪个类型去认领键名。
	// `{}` 是"键是数据、值是记录"的那一层（旧 `accountKey`），跳过。
	owners := map[string]any{
		"$":                     LegacyConfig{},
		"$.caiyun[]":            LegacyAccountConfig{},
		"$.accounts.{}":         LegacyAccountState{},
		"$.runs[]":              LegacyRun{},
		"$.runs[].logs[]":       LegacyLogEntry{},
		"$.pipelines[]":         LegacyPipeline{},
		"$.pipelines[].steps[]": LegacyStep{},
		"$.bindings[]":          LegacyBinding{},
		"$.schedules[]":         LegacySchedule{},
	}
	// 自由数据层：这些字段在类型里是 `*any`（任意 JSON），
	// 它们**之下**的键是业务数据，不是字段名 —— 没有类型可认领，跳过。
	//
	// ⚠ 这一层是"类型说它是任意值"的显式登记，与上面那张表性质不同：
	// 表里的是"该由谁认领"，这里是"谁都不该认领"。
	freeForm := []string{
		"$.runs[].result",
		"$.runs[].logs[]", // 日志条目本身是结构体，但夹具里没有更深的嵌套
		"$.pipelines[].steps[].body",
	}

	// 顶层文件各有一种，按文件名单独认领。
	rootOwners := map[string]any{
		"state.json":     LegacyStateFile{},
		"history.json":   LegacyHistoryFile{},
		"pipelines.json": LegacyPipelinesFile{},
		"schedules.json": LegacySchedulesFile{},
		"exchange.json":  LegacyExchangeFile{},
		"webui.json":     LegacyWebuiFile{},
		"asign.json":     LegacyConfig{},
	}

	for _, file := range v.FixtureKeys {
		root := rootOwners[file.File]
		if root == nil {
			t.Fatalf("夹具 %s 没有登记顶层类型 —— 补上 owners 表", file.File)
		}
		for _, path := range file.Paths {
			owner, ok := ownerForPath(owners, freeForm, root, path)
			if !ok {
				t.Fatalf("路径 %s（%s）没有登记认领类型 —— 补上 owners 表", path, file.File)
			}
			if owner == nil {
				continue // 这一层的键是数据，不是字段名
			}
			key := path[strings.LastIndex(path, ".")+1:]
			if key == "" || key == "[]" {
				continue
			}
			// `$` 开头的是**夹具自己的元信息**，不是旧文档的字段
			// （`$fixtureNote` / `$meta` …）。serde 也会忽略它们。
			if strings.HasPrefix(key, "$") {
				continue
			}
			if _, known := jsonTagOf(owner, key); !known {
				t.Fatalf("%s 的 %s 里出现了 Go 侧不认识的键 %q（tag 写错或漏了）",
					file.File, path[:strings.LastIndex(path, ".")], key)
			}
		}
	}
}

// ownerForPath 找出该路径的键名该由哪个类型认领。
//
// ⚠ 返回 `(nil, true)` 表示"这一层的键是数据（旧 accountKey），跳过"。
func ownerForPath(owners map[string]any, freeForm []string, root any, path string) (any, bool) {
	for _, prefix := range freeForm {
		if strings.HasPrefix(path, prefix) && path != prefix {
			return nil, true
		}
	}
	lastDot := strings.LastIndex(path, ".")
	if lastDot < 0 {
		return root, true
	}
	parent := path[:lastDot]
	// ⚠ `$` 必须**先**落到"这份文件的顶层类型"：
	// `owners["$"]` 是给 `asign.json` 用的，若让它抢先，
	// 其它文件的顶层键（`$.accounts` / `$.runs` …）就会被拿 LegacyConfig 去认领。
	if parent == "$" {
		return root, true
	}
	// `$.accounts.{}` 这类：这一层的键是数据（旧 accountKey），不是字段名 ⇒ 跳过。
	// ⚠ 但 `$.accounts.{}.jwtToken` 不能被跳过 —— 它的 parent 是 `$.accounts.{}`，
	// 由 owners 表认领，走的是下面那条。
	if strings.HasSuffix(path, "{}") {
		return nil, true
	}
	if owner, ok := owners[parent]; ok {
		return owner, true
	}
	return nil, false
}

// jsonTagOf 返回该类型里 tag 为 key 的字段名，以及"认不认得这个键"。
//
// 用反射而不是序列化：这一半恰好覆盖的是**无法序列化**的那些类型。
func jsonTagOf(v any, key string) (string, bool) {
	typ := reflect.TypeOf(v)
	for i := 0; i < typ.NumField(); i++ {
		field := typ.Field(i)
		tag := field.Tag.Get("json")
		if tag == "" || tag == "-" {
			continue
		}
		name := tag
		if idx := strings.Index(tag, ","); idx >= 0 {
			name = tag[:idx]
		}
		if name == key {
			return field.Name, true
		}
	}
	return "", false
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

func errorsAsLegacy(err error, target **LegacyError) bool {
	le, ok := err.(*LegacyError)
	if ok {
		*target = le
	}
	return ok
}

func sortStrings(list []string) {
	for i := 1; i < len(list); i++ {
		for j := i; j > 0 && list[j] < list[j-1]; j-- {
			list[j], list[j-1] = list[j-1], list[j]
		}
	}
}

func itoa64(v int64) string {
	if v == 0 {
		return "0"
	}
	neg := v < 0
	if neg {
		v = -v
	}
	var buf [24]byte
	i := len(buf)
	for v > 0 {
		i--
		buf[i] = byte('0' + v%10)
		v /= 10
	}
	if neg {
		i--
		buf[i] = '-'
	}
	return string(buf[i:])
}

// ─────────────────────────────────────────────────────────────────────────────
// `PipelineStep` 的编解码
//
// 它是**唯一一个嵌进 JSON 再读回来**的 core 类型（`pipelines.steps` 列），
// 所以也是唯一一处"字段级的形状差异会在**往返之后**才显形"的地方。
// 上面那批 `$serializedShapes` 只覆盖"写出去长什么样"，覆盖不到这里。
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelineStepEncodeMatchesSerdeVectors 比对 `pipelines.steps` 那一列的编码形状。
//
// 期望值由 serde 真跑出来（`$pipelineStepEncode`），钉两件事：
//
//   - `None` 序列化成 **`null`，不是字段消失**（Rust 侧那两个字段没有
//     `skip_serializing_if`）⇒ Go 侧不能顺手加 `omitempty`；
//   - `Some(Value::Null)` 与 `None` **编码后无法区分** —— 所以下面解码时把
//     `body: null` 归一成 nil **不丢信息**，那是 JSON 本身就不区分的东西。
func TestPipelineStepEncodeMatchesSerdeVectors(t *testing.T) {
	v := loadLegacyShapeVectors(t)
	want := map[string]PipelineStep{
		"both-none": {Task: "prizes"},
		"both-some": {
			Task:            "receive",
			Body:            json.RawMessage(`{"onlyNew":true}`),
			ContinueOnError: boolPtr(true),
		},
		"body-null-value": {
			Task:            "receive",
			Body:            json.RawMessage("null"),
			ContinueOnError: boolPtr(false),
		},
		// ⚠ 这条专门守 `MarshalJSON` **必须走 [marshalJSON]**：
		// 默认路径会对 `json.RawMessage` 的内容再做一遍 HTML 转义
		// （`<a&b>` → `\u003ca\u0026b\u003e`），而 serde_json 不转义。
		// 没有它，"用 `json.Marshal` 也能过"——因为其余用例里没有 `<`。
		"body-html-chars": {
			Task: "receive",
			Body: json.RawMessage(`{"note":"<a&b>"}`),
		},
	}
	for _, c := range v.PipelineStepEncode {
		step, ok := want[c.Label]
		if !ok {
			t.Fatalf("向量里的 %q 没有对应的 Go 实例 —— 补上，别删向量", c.Label)
		}
		if got := mustMarshalLegacy(t, step); got != c.JSON {
			t.Fatalf("%s 与 serde 的输出不一致:\n实得 %s\n期望 %s", c.Label, got, c.JSON)
		}
	}
}

// TestPipelineStepDecodeMatchesSerdeVectors 比对"哪些输入该收、哪些该拒"。
//
// ⚠ 三条**反直觉**的实测结论都钉在这里，每一条我都差点按直觉写反：
//
//   - `body` 可以是**任意 JSON 值**，不只对象（`"not-an-object"` 被收成 String）
//     —— 字段名叫 `body`、注释写着"请求体"，很容易写成"必须是对象"；
//   - 未知字段被**忽略**（serde 没有 `deny_unknown_fields`）；
//   - `task` **必填** —— 而 Go 的 `string` 零值会**静默接受**缺失，
//     于是"某一步没有任务名"变成"任务名是空串"，错在别处才报出来。
//
// 用 `json.Unmarshal`（而不是 `decodeJSONStrict`）：与 `internal/io`
// 读那一列的方式一致，也是这条断言的题设。
func TestPipelineStepDecodeMatchesSerdeVectors(t *testing.T) {
	v := loadLegacyShapeVectors(t)
	for _, c := range v.PipelineStepDecode {
		t.Run(c.Label, func(t *testing.T) {
			var step PipelineStep
			err := json.Unmarshal([]byte(c.Input), &step)

			if gotOK := err == nil; gotOK != c.OK {
				t.Fatalf("serde %s 而 Go %s（输入 %s）\nserde 说：%s\nGo 说：%v",
					acceptVerdict(c.OK), acceptVerdict(gotOK), c.Input, c.Error, err)
			}
			if !c.OK {
				return
			}

			if step.Task != c.Task {
				t.Errorf("task：Go %q / serde %q", step.Task, c.Task)
			}
			if got := step.Body == nil; got != c.BodyIsNone {
				t.Errorf("body 的 None 判定：Go %v / serde %v\n"+
					"⚠ json.RawMessage 把 null 存成**字面的四个字节**（不是 nil），"+
					"归一那一步不能省 —— 少了它 JSON 文本一模一样，只有 == nil 会变",
					got, c.BodyIsNone)
			}
			// 解出来的内容也要一致：`bodyText` 是 serde 侧 `Value::to_string()`。
			if c.BodyText == nil {
				if step.Body != nil {
					t.Errorf("serde 说 body 是 None，Go 却解出了 %s", step.Body)
				}
			} else if string(step.Body) != *c.BodyText {
				t.Errorf("body 内容：Go %s / serde %s", step.Body, *c.BodyText)
			}
			if got := step.ContinueOnError == nil; got != c.CoeIsNone {
				t.Errorf("continue_on_error 的 None 判定：Go %v / serde %v", got, c.CoeIsNone)
			}
		})
	}
}

func acceptVerdict(ok bool) string {
	if ok {
		return "收"
	}
	return "拒"
}

func boolPtr(b bool) *bool { return &b }
