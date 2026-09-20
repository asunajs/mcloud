// core 的 JSON 出入口。
//
// 对应 Rust 侧散落在 `sanitize.rs`（`marshalJSON` / `parseJSON`）里的那两个
// 私有辅助函数 —— 它们之所以要**导出**一半，理由见 [MarshalJSON]。

package core

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
