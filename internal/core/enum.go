// 枚举的 JSON 编解码 —— Rust 侧 serde 枚举与 Go 侧 int 枚举之间的桥。
//
// # ⚠⚠ 这是换语言新长出来的一条，而且它改的是**外部契约**
//
// Rust 侧每一个进 JSON 的枚举都带 `#[serde(rename_all = …)]`，序列化成**文本**：
//
//	{"outcome":"manual_required","level":"warn","role":"admin"}
//
// 而 Go 的 `type X int` 默认序列化成**数字**：
//
//	{"outcome":2,"level":1,"role":0}
//
// 两边都读同一批 `run_events.payload`、同一批 SSE `data:`、同一张 `users.role`
// 列 —— 默认行为不一致就是**同一个字段在两侧有两个形状**。它不会报错，
// 只会表现为"另一侧读出来的东西不对"，而且只在真跑起来之后才看得见。
//
// 所以规则是：**凡是会进 JSON 的枚举，必须显式实现 MarshalJSON / UnmarshalJSON**。
// 本文件把这件事收在一处，避免每个枚举各抄一份样板（抄漏一处就是一次静默失真）。
//
// # 为什么不做成 `encoding.TextMarshaler`
//
// `TextMarshaler` 只管编码、不管解码，而且它要求配对的 `TextUnmarshaler`，
// 两个接口都实现之后 `String()` 的语义就容易被误用。这里要的是
// "编解码成对、且与 serde 同口径"，所以用显式的 MarshalJSON / UnmarshalJSON。
//
// # 未知值：编码报错、解码报错
//
// 两条都对**齐 serde**，而不是对齐本包那些宽松的 [ParseRunOutcome] / [ParseRole]：
//
//   - 编码时遇到没有名字的值 → **报错**。它是"我们构造出了一个不该存在的态"，
//     静默输出一个占位串会让脏数据落库；
//   - 解码时遇到不认识的文本 → **报错**。serde 对未知变体就是失败。
//     宽松地"归零值"会造出另一种不对齐：Rust 拒收的那一行，Go 却读成了 success。
//
// ⚠ 那些宽松的 `ParseXxx` 是**另一条路**（应用层读存储时的降级），
// 它们的存在不是"JSON 也该宽松"的理由 —— 两者判据不同，别互相抄。
package core

import (
	"encoding/json"
	"fmt"
	"strings"
)

// enumJSON 是一张"枚举值 ↔ JSON 文本"的表。
//
// `names` 的下标**就是**枚举值，所以它的顺序必须与常量声明顺序一致；
// 空串表示该值没有名字（用于给将来的态留位置，届时编码会报错而不是编出空串）。
type enumJSON struct {
	// typeName 只用于报错信息：库里躺着一行 `outcome:7` 时，
	// 报错要说清是哪个字段的类型，否则排查的人要去猜。
	typeName string
	names    []string
}

// marshal 把枚举值编成带引号的文本。
//
// 走 [marshalJSON] 而不是 `json.Marshal`：前者关掉了 HTML 转义，
// 与 Rust 的 `serde_json` 同形。
func (c enumJSON) marshal(v int) ([]byte, error) {
	if v >= 0 && v < len(c.names) && c.names[v] != "" {
		return marshalJSON(c.names[v])
	}
	return nil, fmt.Errorf("%s: 值 %d 没有对应的文本，拒绝序列化（已知 %d 个态）",
		c.typeName, v, len(c.names))
}

// unmarshal 把 JSON 文本解回枚举值。
//
// ⚠ 数字也**一并拒绝**：Rust 侧写进库的是文本，
// 库里若出现数字说明是别的东西写进去的 —— 放宽只会把那份脏数据当成合法值读进来。
func (c enumJSON) unmarshal(data []byte) (int, error) {
	// ⚠ `null` 必须单独挡：`encoding/json` 把 null 解进 string 是**静默成功**
	// （留空串、不报错），不挡的话它会一路走到下面"未知的文本 \"\""，
	// 结果虽然也是报错，但报错信息指向了错的那一头。Rust 侧对 null 也是拒。
	if string(data) == "null" {
		return 0, fmt.Errorf("%s: JSON 里是 null，而这一列不是可选的", c.typeName)
	}
	var raw string
	if err := json.Unmarshal(data, &raw); err != nil {
		return 0, fmt.Errorf("%s: JSON 里应当是文本（Rust 侧 serde 枚举的形状），得到 %s",
			c.typeName, strings.TrimSpace(string(data)))
	}
	for i, n := range c.names {
		if n != "" && n == raw {
			return i, nil
		}
	}
	return 0, fmt.Errorf("%s: 未知的文本 %q（已知：%s）",
		c.typeName, raw, strings.Join(c.names, ", "))
}

// ─────────────────────────────────────────────────────────────────────────────
// 各枚举的接线
//
// 表里的文本必须与 Rust 侧 `rename_all` 的结果**逐字相同**。
// ⚠ `snake_case` 与 `lowercase` 两族别混：`lowercase` 只压大小写、
// 不插下划线（`PartialSuccess` → `partialsuccess`，而这里要的是 `partial_success`）。
// ─────────────────────────────────────────────────────────────────────────────

// runStatusJSON 对应 Rust 侧 `model.rs` 的 `#[serde(rename_all = "snake_case")]`。
var runStatusJSON = enumJSON{typeName: "RunStatus", names: []string{"running", "done", "failed"}}

// MarshalJSON 输出 `running` / `done` / `failed`。
func (s RunStatus) MarshalJSON() ([]byte, error) { return runStatusJSON.marshal(int(s)) }

// UnmarshalJSON 是 [RunStatus.MarshalJSON] 的逆。未知文本报错，见文件注释。
func (s *RunStatus) UnmarshalJSON(data []byte) error {
	v, err := runStatusJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*s = RunStatus(v)
	return nil
}

// runOutcomeJSON 对应 Rust 侧 `run.rs` 的 `#[serde(rename_all = "snake_case")]`。
var runOutcomeJSON = enumJSON{
	typeName: "RunOutcome",
	names:    []string{"success", "partial_success", "manual_required", "failed"},
}

// MarshalJSON 输出 `success` / `partial_success` / `manual_required` / `failed`。
//
// ⚠ 这四个串是**外部契约**：进 `run_events.payload`、`outcome` 列、推送文案，
// 前端直接 `switch (ev.outcome)`。
func (o RunOutcome) MarshalJSON() ([]byte, error) { return runOutcomeJSON.marshal(int(o)) }

// UnmarshalJSON 是 [RunOutcome.MarshalJSON] 的逆。
func (o *RunOutcome) UnmarshalJSON(data []byte) error {
	v, err := runOutcomeJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*o = RunOutcome(v)
	return nil
}

// manualTargetJSON 对应 Rust 侧 `run.rs` 的 `#[serde(rename_all = "snake_case")]`。
var manualTargetJSON = enumJSON{
	typeName: "ManualTarget",
	names:    []string{"mobile_app", "web", "webui", "cli", "unspecified"},
}

// MarshalJSON 输出 `mobile_app` / `web` / `webui` / `cli` / `unspecified`。
func (t ManualTarget) MarshalJSON() ([]byte, error) { return manualTargetJSON.marshal(int(t)) }

// UnmarshalJSON 是 [ManualTarget.MarshalJSON] 的逆。
func (t *ManualTarget) UnmarshalJSON(data []byte) error {
	v, err := manualTargetJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*t = ManualTarget(v)
	return nil
}

// logLevelJSON 对应 Rust 侧 `run.rs` 的 `#[serde(rename_all = "snake_case")]`。
var logLevelJSON = enumJSON{typeName: "LogLevel", names: []string{"info", "warn", "error"}}

// MarshalJSON 输出 `info` / `warn` / `error`。
func (l LogLevel) MarshalJSON() ([]byte, error) { return logLevelJSON.marshal(int(l)) }

// UnmarshalJSON 是 [LogLevel.MarshalJSON] 的逆。
func (l *LogLevel) UnmarshalJSON(data []byte) error {
	v, err := logLevelJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*l = LogLevel(v)
	return nil
}

// roleJSON 对应 Rust 侧 `user.rs` 的 `#[serde(rename_all = "lowercase")]`。
var roleJSON = enumJSON{typeName: "Role", names: []string{"user", "admin"}}

// MarshalJSON 输出 `user` / `admin`。
//
// ⚠ 顺序是 `user` 在前：下标即枚举值，而 [RoleUser] 是 0（最小权限），
// 见 [RoleUser] 那条注释。写反会让库里的 `role` 列与权限判据整体互换。
func (r Role) MarshalJSON() ([]byte, error) { return roleJSON.marshal(int(r)) }

// UnmarshalJSON 是 [Role.MarshalJSON] 的逆。
func (r *Role) UnmarshalJSON(data []byte) error {
	v, err := roleJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*r = Role(v)
	return nil
}

// sessionKindJSON 对应 Rust 侧 `user.rs` 的 `#[serde(rename_all = "lowercase")]`。
var sessionKindJSON = enumJSON{typeName: "SessionKind", names: []string{"web", "cli"}}

// MarshalJSON 输出 `web` / `cli`。
func (k SessionKind) MarshalJSON() ([]byte, error) { return sessionKindJSON.marshal(int(k)) }

// UnmarshalJSON 是 [SessionKind.MarshalJSON] 的逆。
func (k *SessionKind) UnmarshalJSON(data []byte) error {
	v, err := sessionKindJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*k = SessionKind(v)
	return nil
}
