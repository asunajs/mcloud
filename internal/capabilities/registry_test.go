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

// TestEveryCapabilityDeclaresMissPolicy 钉住"补跑策略必须逐条显式声明"。
//
// 判据不是"字段填了没填"（Go 里零值也算填了），而是这一条：
// [core.MissPolicy] 的零值是 `MissCatchUp`，也就是说**忘了填**会静默变成
// "错过了就多打一次真上游"。签到那一档漏填看不出问题，抢兑那一档漏填
// 就是"窗口过了还去扣一次云朵"。
//
// 反向改坏这里的方式：把某条的 `MissPolicyOnMiss` 删掉 ⇒ 本用例红。
func TestEveryCapabilityDeclaresMissPolicy(t *testing.T) {
	for _, cap := range All() {
		if cap.MissPolicyOnMiss == nil {
			t.Errorf("%s 没声明错过时补不补 —— 零值是 catch_up，等于替它决定多跑一趟", cap.Code)
		}
	}
}

// TestResolveMissPolicyPutsTheRowBeforeTheCapability 定的是三档优先级：
// 那一行选过的 > 能力声明的 > 谁都没说（⇒ 不补）。
//
// ⚠ 顺序反了的表现最难查：用户在界面上勾了"不补跑"，某天改了一次能力的默认，
// 于是他的那一行又开始每天多跑一趟 —— 而库里存着的明明是他选的那个值。
func TestResolveMissPolicyPutsTheRowBeforeTheCapability(t *testing.T) {
	cap, ok := Get("sign")
	if !ok {
		t.Fatal("sign 必须在注册表中")
	}
	if got := ResolveMissPolicy(cap, nil); got != core.MissCatchUp {
		t.Errorf("没给定时任务时该跟随能力声明，实得 %v", got)
	}

	skip := core.MissSkip
	row := &core.Schedule{MissPolicy: &skip}
	if got := ResolveMissPolicy(cap, row); got != core.MissSkip {
		t.Errorf("用户那一行选了 skip，能力默认不该盖过它，实得 %v", got)
	}

	// 谁都没说 ⇒ skip：这一档是"没有授权就不额外打上游"，不是"随便挑一个默认"
	undeclared := Capability{Code: "没有声明的能力"}
	if got := ResolveMissPolicy(undeclared, &core.Schedule{}); got != core.MissSkip {
		t.Errorf("两样都没声明时该不补跑，实得 %v", got)
	}
}

func TestAuthRefreshIsRegisteredForScheduling(t *testing.T) {
	cap, ok := Get("auth-refresh")
	if !ok {
		t.Fatal("auth-refresh 必须在注册表中 —— 定时任务的 target 要能指到它")
	}
	// 通用列表里没有它（它有自己的入口），但 [Get] 找得到：
	// `Hidden` 管的是"出现在哪份清单里"，不是"能不能被寻址"。
	for _, listed := range ListVisible() {
		if listed.Code == "auth-refresh" {
			t.Error("auth-refresh 不该出现在通用任务列表：那枚按钮的语义是'现在就换'")
		}
	}
	for _, listed := range ListBatchable() {
		if listed.Code == "auth-refresh" {
			t.Error("auth-refresh 不进组合编排：它是'到期才换'，不是每天该跑的一步")
		}
	}
	if cap.Group != core.GroupMaintenance {
		t.Errorf("它是维护型，实得 %v", cap.Group)
	}
	if cap.NeedsConfirmation() {
		t.Error("Medium 不该弹确认框：判据是'花掉不可逆额度'，刷新不消耗额度")
	}
}

// TestAuthRefreshParamsHaveNoForceSwitch 守的是参数面，不是实现。
//
// `force` 的语义是"人拍板现在就换"。它一旦成为**定时任务的参数**，
// 就等于"每个触发点都硬换一次 token"，而没人会为此再确认第二次。
// 手动那条路（账号页那一枚按钮）走的是另一条路由、恒 force，
// 与本能力的参数面无关 —— 所以这里挡的是"以后有人图省事把它接进来"。
func TestAuthRefreshParamsHaveNoForceSwitch(t *testing.T) {
	cap, _ := Get("auth-refresh")
	for _, spec := range cap.Params() {
		switch spec.Key {
		case "force", "leadDays", "lead_days":
			t.Errorf("参数面不该有 %s：定时任务里没有人，越闸的授权也就没有来源", spec.Key)
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

// TestPointerFieldsAreCopiedToo 补的是 [TestAllCannotBeUsedToMutateTheRegistry]
// **挡不住**的那一半。
//
// `copy(out, registry)` 只复制字段值：指针字段复制的是**地址**，
// 于是 `*got.MissPolicyOnMiss = …` 改的就是注册表本体，且比改字段更隐蔽 ——
// 改字段只污染这一次拿到的那份，改指针目标会污染**之后所有人**拿到的那一份，
// 包括本包自己的 [Get]。而表现是"某个能力某天开始补跑了"，没有任何一处代码写着补跑。
func TestPointerFieldsAreCopiedToo(t *testing.T) {
	sign, ok := Get("sign")
	if !ok {
		t.Fatal("sign 必须在注册表中")
	}
	if sign.MissPolicyOnMiss == nil {
		t.Fatal("sign 该显式声明补跑策略（见 [TestEveryCapabilityDeclaresMissPolicy]）")
	}

	original := *sign.MissPolicyOnMiss
	*sign.MissPolicyOnMiss = core.MissPolicy(99) // 越界值：一眼能看出是被人改过的

	again, _ := Get("sign")
	if again.MissPolicyOnMiss == nil || *again.MissPolicyOnMiss != original {
		t.Fatalf("Get 交出了指向注册表的指针：改一份等于改全体（实得 %v，原值 %v）",
			again.MissPolicyOnMiss, original)
	}
	for _, listed := range All() {
		if listed.Code != "sign" {
			continue
		}
		if listed.MissPolicyOnMiss == nil || *listed.MissPolicyOnMiss != original {
			t.Fatalf("All 同样漏了指针那一半：%v", listed.MissPolicyOnMiss)
		}
	}
}
