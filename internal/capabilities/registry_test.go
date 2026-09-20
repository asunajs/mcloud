package capabilities

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

func TestCodesAreUnique(t *testing.T) {
	seen := map[string]bool{}
	for _, cap := range All() {
		if seen[cap.Code] {
			t.Fatalf("能力 code 重复: %s", cap.Code)
		}
		seen[cap.Code] = true
	}
}

func TestParamsKeysAreUniqueWithinACapability(t *testing.T) {
	for _, cap := range All() {
		seen := map[string]bool{}
		for _, spec := range cap.Params() {
			if seen[spec.Key] {
				t.Fatalf("%s 的参数 key 重复: %s", cap.Code, spec.Key)
			}
			seen[spec.Key] = true
		}
	}
}

func TestExchangeIsHiddenAndHighRisk(t *testing.T) {
	exchange, ok := Get("exchange")
	if !ok {
		t.Fatal("exchange 必须在注册表中")
	}
	if !exchange.Hidden {
		t.Error("兑换不应出现在通用任务列表")
	}
	if exchange.RunInBatch {
		t.Error("兑换不应参与组合编排")
	}
	if !exchange.NeedsConfirmation() {
		t.Error("消耗云朵必须二次确认")
	}
}

func TestExchangePrizeParamIsRequired(t *testing.T) {
	exchange, _ := Get("exchange")
	var prize *core.FieldSpec
	for _, spec := range exchange.Params() {
		if spec.Key == "prizeIdOrName" {
			spec := spec
			prize = &spec
			break
		}
	}
	if prize == nil {
		t.Fatal("应有奖品参数")
	}
	if !prize.Required {
		t.Error("奖品是必填项")
	}
}

func TestQueryCapabilitiesAreNotBatchableAndAreVisible(t *testing.T) {
	snapshot, ok := Get("cloud-snapshot")
	if !ok {
		t.Fatal("cloud-snapshot 必须在注册表中")
	}
	if snapshot.Kind != core.KindQuery {
		t.Errorf("云朵快照应是查询型，实际 %v", snapshot.Kind)
	}
	if snapshot.RunInBatch {
		t.Error("查询型能力不进批量执行")
	}
	if snapshot.Hidden {
		t.Error("云朵快照应当可见")
	}
}

func TestDefaultAccountModeIsSequential(t *testing.T) {
	for _, cap := range All() {
		if cap.DefaultAccountMode != core.ModeSequential {
			t.Errorf("%s 默认应为串行以防风控，实际 %v", cap.Code, cap.DefaultAccountMode)
		}
	}
}

func TestVisibleListExcludesHidden(t *testing.T) {
	visible := map[string]bool{}
	for _, cap := range ListVisible() {
		visible[cap.Code] = true
	}
	if !visible["sign"] {
		t.Error("sign 应可见")
	}
	if visible["exchange"] {
		t.Error("exchange 不应可见")
	}
}

func TestBatchableListExcludesNonBatchable(t *testing.T) {
	batchable := map[string]bool{}
	for _, cap := range ListBatchable() {
		batchable[cap.Code] = true
	}
	if !batchable["sign"] {
		t.Error("sign 应可批量执行")
	}
	if batchable["exchange"] {
		t.Error("exchange 不应进批量列表（隐藏 + 高风险）")
	}
	if batchable["cloud-snapshot"] {
		t.Error("cloud-snapshot 是查询型，不进批量列表")
	}
}

func TestRegistrySerializesForFrontendConsumption(t *testing.T) {
	exchange, _ := Get("exchange")
	text, err := core.MarshalJSON(map[string]any{
		"code":              exchange.Code,
		"title":             exchange.Title,
		"hidden":            exchange.Hidden,
		"needsConfirmation": exchange.NeedsConfirmation(),
		"params":            exchange.Params(),
		"output":            exchange.Output(),
	})
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	got := string(text)
	for _, want := range []string{`"skipWait"`, `"sequential"`, `"cloudCost"`} {
		if !strings.Contains(got, want) {
			t.Errorf("payload 里缺 %s: %s", want, got)
		}
	}
}

// TestAllCannotBeUsedToMutateTheRegistry 守住 Go 侧新长出来的那条约束。
//
// Rust 的 `REGISTRY` 取出来只能读；Go 的切片**可写**。若 [All] 把底层数组
// 直接交出去，"任何一个调用方改了一条声明"会立刻影响下一个读到它的能力，
// 而且不报错 —— 表现是"某一个能力忽然少了一个参数"。
//
// ⚠ 反向测试：`All` 改成直接 `return registry` 之后这条**必须变红**
// （已实测：那样 `Get("sign")` 读到的 Title 会是被改过的那个）。
func TestAllCannotBeUsedToMutateTheRegistry(t *testing.T) {
	all := All()
	if len(all) == 0 {
		t.Fatal("注册表不应为空")
	}
	all[0].Title = "被改坏了"
	all[0].Params = noOutput

	for _, code := range []string{"sign", "exchange", "cloud-snapshot"} {
		got, ok := Get(code)
		if !ok {
			t.Fatalf("%s 应在注册表中", code)
		}
		if got.Title == "被改坏了" {
			t.Errorf("All() 交出了注册表本体：%s 的声明被外部改写", code)
		}
		if code == "sign" && len(got.Params()) == 0 {
			t.Error("All() 交出了注册表本体：sign 的参数被外部清空")
		}
	}
}
