// core 的 JSON 出入口。
//
// 对应 Rust 侧散落在 `sanitize.rs`（`marshalJSON` / `parseJSON`）里的那两个
// 私有辅助函数 —— 它们之所以要**导出**一半，理由见 [MarshalJSON]。

package core

import (
	"bytes"
	"encoding/json"
)

// MarshalJSON 序列化，且**与 `serde_json::to_vec` 逐字节同形**。
//
// 这是 [marshalJSON] 的导出版本，给**纯层之外**的产物用
// （`internal/app` 拼 `/api/capabilities` 的响应体就是它）。
//
// # 为什么要导出，而不是让每个包自己写一份
//
// "不转义 HTML"与"不补尾换行"这两条规则**必须只有一份实现**：
// 它们是"Go 默认行为与 serde 不同"这一整类差异的两条，
// 而这类差异的共同点是**改坏了却不响**（夹具解析回来完全正确，
// 只有人眼与按文本扫描的守卫看得见）。
// 让 app / server / cli 各写一份 `json.NewEncoder` + `SetEscapeHTML(false)`，
// 等于给自己造第二份真相 —— 而两份真相迟早会分叉，
// 分叉那天的表现只是"某一个接口的输出里多了 `\u003c`"。
func MarshalJSON(value any) ([]byte, error) { return marshalJSON(value) }

// ParseJSON 反序列化，语义对齐 `serde_json::from_str`。
//
// 与 [MarshalJSON] 同样的理由导出：产物层读配置与读响应体时
// 也要能拿到"19 位标识不丢精度"那条保证（见 [parseJSON]）。
func ParseJSON(text string) (any, bool) { return parseJSON(text) }

// ParseJSONTo 是解析到**具体类型**的版本，语义对齐 `serde_json::from_str::<T>`。
//
// # 为什么要有它，而不是让每个包自己 `json.Unmarshal`
//
// [ParseJSON] 那条"19 位标识不丢精度"的保证（`UseNumber`）是**解码器**上的开关，
// 而 `json.Unmarshal` 走的是默认配置。要落到一个结构体上的时候，
// 顺手写一句 `json.Unmarshal` 就绕开了它 —— 那种绕开不报错，
// 只在某个 int64 字段被悄悄截断成 float64 时才显形。
//
// 所以它复用的仍是同一个解码器：先 `UseNumber` 解成通用值，再转进 `T`。
// 同理"尾随内容必须报错"那条（[parseJSON] 里的 `io.EOF` 检查）也一并继承。
//
// ⚠ 走一遍通用值意味着 `T` 里**不要**指望拿到 `json.RawMessage` 之外的原始字节；
// 需要原样保留的字段用 `any`（会拿到 `json.Number` 形态的数字）。
func ParseJSONTo[T any](text string) (T, bool) {
	var out T
	if !parseInto(text, &out) {
		var zero T
		return zero, false
	}
	return out, true
}

// parseInto 先按 [parseJSON] 的口径解一遍（同时继承它的"尾随内容"检查），
// 再把结果转进 `out`。
func parseInto(text string, out any) bool {
	value, ok := parseJSON(text)
	if !ok {
		return false
	}
	// 走 marshalJSON 而不是 json.Marshal：转义与尾换行的规则必须只有一份。
	buf, err := marshalJSON(value)
	if err != nil {
		return false
	}
	dec := json.NewDecoder(bytes.NewReader(buf))
	dec.UseNumber()
	return dec.Decode(out) == nil
}
