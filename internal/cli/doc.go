// Package cli 是产物之一：命令行。
//
// 对应 Rust 侧的 crates/mcloud-cli（clap derive）。
//
// # 分层铁律
//
// **不得** import github.com/catlair/mcloud/internal/server —— 由
// scripts/check-go-deps.sh 的 FORBIDDEN_EDGES 断言。两个产物只共用 internal/app。
//
// # 约定：零参数 + 下标
//
// 不带子命令直接跑 = 对**全部启用**账号各签一次；账号用**下标**指定
// （--account 2），不是掩码 id。这是用户明确要求的形态，见 docs/cli-usage.md。
//
// ⚠ 参数互斥在 Go 侧用 cobra 的 MarkFlagsMutuallyExclusive 表达，
// 比 Rust 侧手写 conflicts_with_all 更省事 —— 但**只读开关与写开关互斥**
// 这条语义要照搬，别退化成"都能一起给"。
//
// # ⚠ --json 时进度行一律走 stderr
//
// 否则 `| jq` 直接语法错，而人肉看不出。单账号给对象、多账号给数组。
package cli
