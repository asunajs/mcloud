// Package server 是产物之一：HTTP 接口 + WebUI 静态资源。
//
// 对应 Rust 侧的 legacy/crates/mcloud-server（axum + tower-http）。
//
// # 分层铁律
//
// 本包是**组装根**之一，**必须** import github.com/catlair/mcloud/internal/io，
// 否则就是"没人接上 IO 实现"的静默死配置 —— 编译能过、跑起来什么都没发生。
//
// # 产物之间不得互相依赖
//
// 本包与 internal/cli 是两个独立产物，**只能共用 internal/app**。
// 这条由 scripts/check-go-deps.sh 的 FORBIDDEN_EDGES 断言（方向：cli 不得
// 依赖 server）。理由不是洁癖：CLI 一旦拿到本包，就会连带把路由、静态资源
// 那一整套拖进命令行产物，而这件事**不会让任何测试变红**，
// 只会让"CLI 能在没有 HTTP 环境的地方跑"这条性质悄悄消失。
package server
