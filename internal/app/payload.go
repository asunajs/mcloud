// 能力注册表与版本信息 → **前端要吃的 JSON**。
//
// 对应 Rust 侧的 legacy/crates/mcloud-app/src/payload.rs。

package app

import (
	"sort"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// CapabilityPayload 是单个能力的对外描述。
//
// 前端只需按 `Params` / `Output` 渲染表单与结果卡片，
// 因此新增能力**不需要改前端任何一行**。
//
// # ⚠ 这里的键是 **camelCase**，与 serde 的 snake_case 默认不一致
//
// Rust 侧是用 `json!({...})` **手工**拼的（`runInBatch` / `needsConfirmation` /
// `defaultAccountMode`），不走 derive —— 所以它是这套代码里少数几个
// "外部契约不是 snake_case" 的地方。
// 照"Go 的 struct 默认 snake_case"去推字段名，会得到 `run_in_batch`，
// 而前端只是**静静地拿不到这个字段**（`undefined` 会被当成 false）。
//
// # 键顺序
//
// Rust 侧 `json!` 的 Map 默认按**字典序**输出，Go 的 struct 按**声明顺序**输出。
// 键顺序不影响 JSON 语义，所以这里没有去对齐；
// 若哪天要做整串对拍，先按项目约定**扩黄金向量**（由 serde 产出），别照实现推。
type CapabilityPayload struct {
	Code               string           `json:"code"`
	Title              string           `json:"title"`
	Group              string           `json:"group"`
	Kind               string           `json:"kind"`
	Description        string           `json:"description"`
	Hidden             bool             `json:"hidden"`
	RunInBatch         bool             `json:"runInBatch"`
	Risk               string           `json:"risk"`
	NeedsConfirmation  bool             `json:"needsConfirmation"`
	DefaultAccountMode string           `json:"defaultAccountMode"`
	Params             []core.FieldSpec `json:"params"`
	Output             []core.FieldSpec `json:"output"`
}

// CapabilitiesPayload 是 `/api/capabilities` 的响应体。
type CapabilitiesPayload struct {
	All       []CapabilityPayload `json:"all"`
	Visible   []string            `json:"visible"`
	Batchable []string            `json:"batchable"`
	Groups    []string            `json:"groups"`
}

// VersionPayload 是 `/api/version` 的响应体 ——
// 取代原构建期的 `__APP_VERSION__` 注入。
type VersionPayload struct {
	Name     string `json:"name"`
	Version  string `json:"version"`
	Timezone string `json:"timezone"`
}

// CapabilityPayloadOf 把一个能力声明转成对外描述。
//
// 枚举名一律取 core 的 `String()` —— **不在这里重写一遍名字表**。
// Rust 侧那四个 `*_name` 常量函数是不得已（要 `const fn`），
// 而在 Go 侧重写一遍等于给自己造第二份真相：
// 哪天 `RiskLevel` 多一档，这里会默默输出空串，而空串在前端就是"没有风险等级"。
func CapabilityPayloadOf(cap capabilities.Capability) CapabilityPayload {
	return CapabilityPayload{
		Code:               cap.Code,
		Title:              cap.Title,
		Group:              cap.Group.String(),
		Kind:               cap.Kind.String(),
		Description:        cap.Description,
		Hidden:             cap.Hidden,
		RunInBatch:         cap.RunInBatch,
		Risk:               cap.Risk.String(),
		NeedsConfirmation:  cap.NeedsConfirmation(),
		DefaultAccountMode: cap.DefaultAccountMode.String(),
		Params:             cap.Params(),
		Output:             cap.Output(),
	}
}

// CapabilitiesPayloadOf 生成 `/api/capabilities` 的响应体。
func CapabilitiesPayloadOf() CapabilitiesPayload {
	all := make([]CapabilityPayload, 0, len(capabilities.All()))
	groups := map[string]bool{}
	for _, cap := range capabilities.All() {
		all = append(all, CapabilityPayloadOf(cap))
		groups[cap.Group.String()] = true
	}

	visible := make([]string, 0)
	for _, cap := range capabilities.ListVisible() {
		visible = append(visible, cap.Code)
	}
	batchable := make([]string, 0)
	for _, cap := range capabilities.ListBatchable() {
		batchable = append(batchable, cap.Code)
	}

	sorted := make([]string, 0, len(groups))
	for name := range groups {
		sorted = append(sorted, name)
	}
	sort.Strings(sorted)

	return CapabilitiesPayload{All: all, Visible: visible, Batchable: batchable, Groups: sorted}
}

// VersionPayloadOf 生成 `/api/version` 的响应体。
func VersionPayloadOf() VersionPayload {
	return VersionPayload{
		// 对外暴露的必须是**产品名**，而不是"碰巧实现它的那个包"的名字。
		Name:     core.ProductName,
		Version:  core.Version,
		Timezone: core.TZName,
	}
}
