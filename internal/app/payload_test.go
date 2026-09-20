package app

import (
	"encoding/json"
	"sort"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// marshal 走 core 的那份实现（不转义 HTML、不补尾换行），
// 与产物层真正输出字节的路径**是同一条**。
func marshal(t *testing.T, value any) string {
	t.Helper()
	raw, err := core.MarshalJSON(value)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	return string(raw)
}

func decode(t *testing.T, text string) map[string]any {
	t.Helper()
	var out map[string]any
	if err := json.Unmarshal([]byte(text), &out); err != nil {
		t.Fatalf("解析失败: %v（原文 %s）", err, text)
	}
	return out
}

func findCapability(t *testing.T, code string) map[string]any {
	t.Helper()
	text := marshal(t, CapabilitiesPayloadOf())
	root := decode(t, text)
	all, _ := root["all"].([]any)
	for _, item := range all {
		m, _ := item.(map[string]any)
		if m["code"] == code {
			return m
		}
	}
	t.Fatalf("payload 里没有 %s", code)
	return nil
}

func TestPayloadExposesDeclarativeFormMetadata(t *testing.T) {
	ex := findCapability(t, "exchange")

	if ex["hidden"] != true {
		t.Errorf("hidden：实得 %v", ex["hidden"])
	}
	if ex["risk"] != "high" {
		t.Errorf("risk：实得 %v", ex["risk"])
	}
	if ex["needsConfirmation"] != true {
		t.Errorf("needsConfirmation：实得 %v", ex["needsConfirmation"])
	}
	if ex["runInBatch"] != false {
		t.Errorf("runInBatch：实得 %v", ex["runInBatch"])
	}
	if ex["defaultAccountMode"] != "sequential" {
		t.Errorf("defaultAccountMode：实得 %v", ex["defaultAccountMode"])
	}

	// 前端需要的表单字段必须齐备
	params, _ := ex["params"].([]any)
	has := func(key string) bool {
		for _, p := range params {
			m, _ := p.(map[string]any)
			if m["key"] == key {
				return true
			}
		}
		return false
	}
	if !has("prizeIdOrName") || !has("skipWait") {
		t.Errorf("params 缺少表单字段：%v", params)
	}

	// 出参也要给（用于自动结果卡片）
	output, _ := ex["output"].([]any)
	found := false
	for _, o := range output {
		m, _ := o.(map[string]any)
		if m["key"] == "cloudCost" {
			found = true
		}
	}
	if !found {
		t.Errorf("output 缺少 cloudCost：%v", output)
	}
}

func TestVisibleAndBatchableListsExcludeHiddenCapability(t *testing.T) {
	root := decode(t, marshal(t, CapabilitiesPayloadOf()))

	contains := func(key, want string) bool {
		items, _ := root[key].([]any)
		for _, it := range items {
			if s, _ := it.(string); s == want {
				return true
			}
		}
		return false
	}
	if !contains("visible", "sign") {
		t.Error("sign 应可见")
	}
	if contains("visible", "exchange") {
		t.Error("exchange 不应可见")
	}
	if contains("batchable", "exchange") {
		t.Error("exchange 不应进批量列表")
	}
}

func TestVersionPayloadComesFromTheProductNameNotThePackage(t *testing.T) {
	got := VersionPayloadOf()

	if got.Name != core.ProductName {
		t.Errorf("name 应取产品名 %q，实得 %q", core.ProductName, got.Name)
	}
	// 对外暴露的必须是**产品名**，而不是"碰巧实现它的那个包"的名字。
	// 少了这条，哪天把 version_payload 挪到别的包，响应就会悄悄改名。
	if got.Name == "app" {
		t.Error("name 不应等于实现它的包名")
	}
	if got.Version == "" {
		t.Error("version 不得为空")
	}
	if got.Timezone != "Asia/Shanghai" {
		t.Errorf("timezone 实得 %q", got.Timezone)
	}
}

// TestAddingACapabilityChangesOnlyTheRegistry 表达的是设计承诺：
// payload 完全由注册表推导，没有任何一处硬编码能力名。
// 因此新增能力不会牵动服务层与前端。
func TestAddingACapabilityChangesOnlyTheRegistry(t *testing.T) {
	root := decode(t, marshal(t, CapabilitiesPayloadOf()))
	all, _ := root["all"].([]any)
	registry := capabilities.All()
	if len(all) != len(registry) {
		t.Fatalf("payload 的能力数应与注册表一致：%d vs %d", len(all), len(registry))
	}
	for i, cap := range registry {
		m, _ := all[i].(map[string]any)
		if m["code"] != cap.Code {
			t.Errorf("第 %d 项：payload %v vs 注册表 %s", i, m["code"], cap.Code)
		}
	}
}

func TestGroupsAreSortedAndDeduplicated(t *testing.T) {
	root := decode(t, marshal(t, CapabilitiesPayloadOf()))
	items, _ := root["groups"].([]any)
	groups := make([]string, 0, len(items))
	for _, it := range items {
		s, _ := it.(string)
		groups = append(groups, s)
	}

	sorted := append([]string(nil), groups...)
	sort.Strings(sorted)
	if len(groups) == 0 {
		t.Fatal("groups 不应为空")
	}
	for i := range groups {
		if groups[i] != sorted[i] {
			t.Fatalf("groups 应已排序：%v", groups)
		}
	}
	unique := map[string]bool{}
	for _, g := range groups {
		unique[g] = true
	}
	if len(unique) != len(groups) {
		t.Errorf("分组不应重复：%v", groups)
	}
}

// TestPayloadKeysAreCamelCase 守住"这套契约不是 snake_case"这件事。
//
// 项目里绝大多数进 JSON 的字段名是 snake_case（serde 默认），
// 于是"顺手写成 `run_in_batch`"是很容易发生的 —— 而它的后果不是报错，
// 是**前端拿到 undefined**。这条断言按**文本**查，往返测试查不出来。
//
// ⚠ 反向测试：把 tag 改成 snake_case 之后这条**必须变红**（已实测）。
func TestPayloadKeysAreCamelCase(t *testing.T) {
	text := marshal(t, CapabilityPayloadOf(mustGet(t, "exchange")))
	for _, want := range []string{`"runInBatch"`, `"needsConfirmation"`, `"defaultAccountMode"`} {
		if !contains(text, want) {
			t.Errorf("payload 里缺 %s：%s", want, text)
		}
	}
	for _, bad := range []string{`run_in_batch`, `needs_confirmation`, `default_account_mode`} {
		if contains(text, bad) {
			t.Errorf("payload 不该出现 snake_case 键 %s：%s", bad, text)
		}
	}
}

// TestEnumNamesComeFromCoreNotFromADuplicateTable 守住"不重写名字表"。
//
// payload 里每个枚举名都必须是 core 那份 `String()` 的产物。
// 一旦有人在这里抄一份字面量，`RiskLevel` 多一档时这里会默默输出空串，
// 而空串在前端就是"没有风险等级"。
func TestEnumNamesComeFromCoreNotFromADuplicateTable(t *testing.T) {
	for _, cap := range capabilities.All() {
		got := CapabilityPayloadOf(cap)
		if want := cap.Group.String(); got.Group != want || want == "" {
			t.Errorf("%s 的 group：实得 %q 期望 %q", cap.Code, got.Group, want)
		}
		if want := cap.Kind.String(); got.Kind != want || want == "" {
			t.Errorf("%s 的 kind：实得 %q 期望 %q", cap.Code, got.Kind, want)
		}
		if want := cap.Risk.String(); got.Risk != want || want == "" {
			t.Errorf("%s 的 risk：实得 %q 期望 %q", cap.Code, got.Risk, want)
		}
		if want := cap.DefaultAccountMode.String(); got.DefaultAccountMode != want || want == "" {
			t.Errorf("%s 的 defaultAccountMode：实得 %q 期望 %q", cap.Code, got.DefaultAccountMode, want)
		}
	}
}

func mustGet(t *testing.T, code string) capabilities.Capability {
	t.Helper()
	cap, ok := capabilities.Get(code)
	if !ok {
		t.Fatalf("注册表里没有 %s", code)
	}
	return cap
}

func contains(haystack, needle string) bool {
	for i := 0; i+len(needle) <= len(haystack); i++ {
		if haystack[i:i+len(needle)] == needle {
			return true
		}
	}
	return false
}
