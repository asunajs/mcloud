package core

import (
	"encoding/json"
	"fmt"
	"testing"

	"github.com/catlair/mcloud/internal/golden"
)

// 本文件是 internal/core/enum.go 的消费者 —— 期望值全部来自
// test/golden/enum-json-vectors.json，而那份向量由 **Rust 侧的 serde** 产出
// （生成器 = scripts/gen-enum-json-vectors.rs）。
//
// ⚠ 这是**跨语言对拍**，不是单元测试：生成侧（serde）与消费侧（本文件）
// 必须是两份实现。用 Go 生成期望值再用 Go 验证是自证 ——
// 两边一起错的时候照样全绿。
//
// ⚠ 取向量函数的文件名必须写成**字面量**：门禁靠扫源码里的字符串判断
// "这份向量有没有人读"，藏进变量会让门禁静默失配、把活向量标成死向量。

type enumTextVector struct {
	Enum    string `json:"enum"`
	Variant string `json:"variant"`
	Text    string `json:"text"`
}

type eventJSONVector struct {
	Kind string `json:"kind"`
	JSON string `json:"json"`
}

type enumDecodeVector struct {
	Type string `json:"type"`
	JSON string `json:"json"`
	OK   bool   `json:"ok"`
}

type enumJSONVectors struct {
	EnumText  []enumTextVector   `json:"$enumText"`
	EventJSON []eventJSONVector  `json:"$eventJson"`
	Decode    []enumDecodeVector `json:"$decode"`
}

func loadEnumJSONVectors(t *testing.T) enumJSONVectors {
	t.Helper()
	raw, err := golden.Load("enum-json-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	var v enumJSONVectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}
	if len(v.EnumText) == 0 || len(v.EventJSON) == 0 || len(v.Decode) == 0 {
		t.Fatalf("黄金向量缺段：enumText=%d eventJson=%d decode=%d",
			len(v.EnumText), len(v.EventJSON), len(v.Decode))
	}
	return v
}

// enumByVariant 按"类型名 + 变体名"取一个 Go 侧枚举值。
//
// ⚠ 这里是**唯一的映射处**：加一个新枚举进 JSON，要么在这里补一行，
// 要么向量里那条就没人认领 —— 缺一行会让下面的循环直接 t.Fatalf，
// 不会静默跳过。
func enumByVariant(t *testing.T, enumName, variant string) any {
	t.Helper()
	switch enumName {
	case "RunStatus":
		switch variant {
		case "Running":
			return StatusRunning
		case "Done":
			return StatusDone
		case "Failed":
			return StatusFailed
		}
	case "RunOutcome":
		switch variant {
		case "Success":
			return OutcomeSuccess
		case "PartialSuccess":
			return OutcomePartialSuccess
		case "ManualRequired":
			return OutcomeManualRequired
		case "Failed":
			return OutcomeFailed
		}
	case "ManualTarget":
		switch variant {
		case "MobileApp":
			return TargetMobileApp
		case "Web":
			return TargetWeb
		case "Webui":
			return TargetWebui
		case "Cli":
			return TargetCli
		case "Unspecified":
			return TargetUnspecified
		}
	case "LogLevel":
		switch variant {
		case "Info":
			return LevelInfo
		case "Warn":
			return LevelWarn
		case "Error":
			return LevelError
		}
	case "Role":
		switch variant {
		case "User":
			return RoleUser
		case "Admin":
			return RoleAdmin
		}
	case "SessionKind":
		switch variant {
		case "Web":
			return SessionWeb
		case "Cli":
			return SessionCLI
		}
	}
	t.Fatalf("黄金向量里的 %s.%s 在本包没有对应值 —— 要么 Go 侧漏了这一态，要么向量写错了", enumName, variant)
	return nil
}

// TestEnumTextMatchesSerdeVectors 逐条比对枚举的 JSON 文本。
//
// 关键是第一条断言（`got[0] != '"'`）：Go 的 `type X int` 默认输出**数字**，
// 而 serde 输出**带引号的文本**。漏写 MarshalJSON 的话这条就是红的，
// 而漏写只影响 JSON —— 其余测试（String / Parse）照旧全绿。
func TestEnumTextMatchesSerdeVectors(t *testing.T) {
	v := loadEnumJSONVectors(t)
	for _, c := range v.EnumText {
		value := enumByVariant(t, c.Enum, c.Variant)
		got, err := marshalJSON(value)
		if err != nil {
			t.Fatalf("%s.%s 序列化失败: %v", c.Enum, c.Variant, err)
		}
		if len(got) == 0 || got[0] != '"' {
			t.Fatalf("%s.%s 应当序列化成带引号的文本，实得 %s（数字说明漏了 MarshalJSON）",
				c.Enum, c.Variant, got)
		}
		var text string
		if err := json.Unmarshal(got, &text); err != nil {
			t.Fatalf("%s.%s 的结果不是合法 JSON 字符串: %s", c.Enum, c.Variant, got)
		}
		if text != c.Text {
			t.Fatalf("%s.%s 实得 %q，serde 那边是 %q", c.Enum, c.Variant, text, c.Text)
		}
	}
}

// TestRunEventJSONMatchesSerdeVectors 整串比对五类事件。
//
// 为什么是"整串"而不是逐字段：字段顺序、`type` 标签的位置、
// `Option` 的 null、以及 `<` `&` 不被转义 —— 这些全在串里，
// 逐字段比对会把它们全部放过。
func TestRunEventJSONMatchesSerdeVectors(t *testing.T) {
	v := loadEnumJSONVectors(t)

	// ⚠ 与生成器里写死的值逐字相同，否则比较的是两个不同的输入。
	const runIDRaw = "64234443-2463-4483-a4a3-c4c3a4e38403"
	runID := RunID(runIDRaw)
	acct := NewAccountID("19900001234")
	// 刻意含 `<` `>` `&` 与中文：这三个字符正是 Go 默认 json.Marshal
	// 会转成 \u003c 的地方，而 serde_json 不转。
	tricky := "风控退避 <b>&amp;</b> 明天再试"

	step := NewManualStep("backup-gift", &acct, "未开启备份，请前往 App 手动开启", TargetMobileApp)
	summary := "有一件待办"

	want := map[string]RunEvent{
		"started":             NewEventStarted(runID, "sign"),
		"log":                 NewEventLog(runID, LevelWarn, tricky),
		"progress":            NewEventProgress(runID, 3, 20),
		"finished-with-steps": NewEventFinished(runID, OutcomeManualRequired, []ManualStep{step}, &summary),
		"finished-empty":      NewEventFinished(runID, OutcomeSuccess, nil, nil),
		"errored":             NewEventErrored(runID, "上游 500"),
	}

	for _, c := range v.EventJSON {
		ev, ok := want[c.Kind]
		if !ok {
			t.Fatalf("黄金向量里的事件 %q 在本测试里没有对应构造 —— 要么 Go 侧漏了这一类，要么向量写错了", c.Kind)
		}
		got, err := marshalJSON(ev)
		if err != nil {
			t.Fatalf("%s 序列化失败: %v", c.Kind, err)
		}
		if string(got) != c.JSON {
			t.Fatalf("%s 与 serde 的输出不一致:\n实得 %s\n期望 %s", c.Kind, got, c.JSON)
		}
	}
}

// TestEnumDecodeMatchesSerdeVectors 比对"哪些输入该收、哪些该拒"。
//
// ⚠ `2` 那一条是重点：它是 Go 侧**没有** MarshalJSON 时写出来的东西。
// 让它在两侧都被拒，库里若真躺着数字，两边才会一起红；
// 只在一侧拒，表现就是"Rust 读不了、Go 能读"，而这种差异不会被任何测试报出来。
func TestEnumDecodeMatchesSerdeVectors(t *testing.T) {
	v := loadEnumJSONVectors(t)
	for _, c := range v.Decode {
		err := decodeEnumByType(c.Type, c.JSON)
		if (err == nil) != c.OK {
			t.Fatalf("%s 解 %s：实得 ok=%v（%v），serde 那边 ok=%v",
				c.Type, c.JSON, err == nil, err, c.OK)
		}
	}
}

func decodeEnumByType(typeName, raw string) error {
	switch typeName {
	case "RunOutcome":
		var x RunOutcome
		return json.Unmarshal([]byte(raw), &x)
	case "LogLevel":
		var x LogLevel
		return json.Unmarshal([]byte(raw), &x)
	case "Role":
		var x Role
		return json.Unmarshal([]byte(raw), &x)
	}
	return fmt.Errorf("未知的枚举类型 %q", typeName)
}

// TestEnumJSONRejectsOutOfRangeValue 钉住"编不出名字的值"必须报错。
//
// 这条没有对应的 Rust 侧向量（Rust 的枚举值域由类型保证，不可能越界），
// 所以它是 **Go 侧独有**的守卫：Go 的 `type X int` 允许任意整数，
// 一个越界值静默编成 `success` 就等于凭空造出一个业务结论。
func TestEnumJSONRejectsOutOfRangeValue(t *testing.T) {
	cases := []struct {
		name  string
		inner func() ([]byte, error)
	}{
		{"RunOutcome", func() ([]byte, error) { return marshalJSON(RunOutcome(99)) }},
		{"ManualTarget", func() ([]byte, error) { return marshalJSON(ManualTarget(-1)) }},
		{"LogLevel", func() ([]byte, error) { return marshalJSON(LogLevel(7)) }},
		{"RunStatus", func() ([]byte, error) { return marshalJSON(RunStatus(5)) }},
		{"Role", func() ([]byte, error) { return marshalJSON(Role(2)) }},
		{"SessionKind", func() ([]byte, error) { return marshalJSON(SessionKind(3)) }},
	}
	for _, c := range cases {
		if _, err := c.inner(); err == nil {
			t.Fatalf("%s 的越界值应当序列化失败，实际通过了", c.name)
		}
	}
}

// TestEnumRoundTripsThroughJSON 是编解码成对的自洽检查。
//
// 与上面的对拍互补：对拍保证"和 serde 一样"，这条保证"自己解得回来"。
// 少了它的话，"Marshal 改名、Unmarshal 忘了改"这种半边改动会被放过。
func TestEnumRoundTripsThroughJSON(t *testing.T) {
	values := []any{
		StatusRunning, StatusDone, StatusFailed,
		OutcomeSuccess, OutcomePartialSuccess, OutcomeManualRequired, OutcomeFailed,
		TargetMobileApp, TargetWeb, TargetWebui, TargetCli, TargetUnspecified,
		LevelInfo, LevelWarn, LevelError,
		RoleUser, RoleAdmin,
		SessionWeb, SessionCLI,
	}
	for _, v := range values {
		raw, err := marshalJSON(v)
		if err != nil {
			t.Fatalf("%T(%v) 序列化失败: %v", v, v, err)
		}
		var back string
		if err := json.Unmarshal(raw, &back); err != nil {
			t.Fatalf("%T(%v) 解回字符串失败: %v", v, v, err)
		}
		if back != fmt.Sprint(v) {
			t.Fatalf("%T(%v) 往返不一致：解回 %q，String() 是 %q", v, v, back, fmt.Sprint(v))
		}
	}
}
