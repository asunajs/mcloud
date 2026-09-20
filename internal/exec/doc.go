// Package mcloudexec 是任务编排层。
//
// 对应 Rust 侧的 legacy/crates/mcloud-exec。
//
// # ⚠ 包名不是 exec
//
// 目录叫 internal/exec（与 Rust 的 crate 名一一对应，便于对照），但**包名**
// 是 mcloudexec —— 因为标准库的 os/exec 会被大量文件同时用到，
// 若本包也叫 exec，每个 import 它俩的文件都得写别名，而漏写一个就是
// 编译错误或更糟的静默错包。目录名与包名不同在 Go 里完全合法。
//
// # 分层铁律
//
// 与 core / upstream / capabilities 同属**纯层**，禁止任何 IO，
// 由 scripts/check-go-deps.sh 断言。
package mcloudexec
