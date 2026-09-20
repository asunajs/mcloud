// Package app 是组装层：把 io 的实现注入 core 定义的契约，供两个产物共用。
//
// 对应 Rust 侧的 legacy/crates/mcloud-app。
//
// # 分层铁律
//
// 本包是**组装根**之一，**必须** import github.com/catlair/mcloud/internal/io
// —— 由 scripts/check-go-deps.sh 断言"必须"，不只是"允许"。
//
// 允许依赖：core、upstream、capabilities、io。
//
// # 这里放什么
//
// 签到全链路编排、登录驱动、账号档案、迁移策略、能力注册表 JSON。
// 注意这些**都是编排**，不是协议 —— 协议在 upstream，纯计算在 core。
package app
