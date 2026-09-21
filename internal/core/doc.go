// Package core 是纯计算层：领域类型、纯函数、以及被注入的契约定义。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-core。
//
// # 分层铁律
//
// 本包**禁止任何 IO**，由 scripts/check-go-deps.sh 断言：
//
//   - 不得 import net/http、database/sql；
//   - 不得 import github.com/catlair/mcloud/internal/io。
//
// 密码学放在本包而不是 io 层 —— 因为「能对某个函数单独测试」这条性质
// 靠的就是这条边界。详见 docs/dev-gates.md「分层铁律」。
//
// # 为什么这里没有 IO 却仍要注入
//
// 时间、随机源、HTTP、存储四个外部世界一律走接口注入（见 contracts.go），
// 不用包级函数直取。理由不是"解耦"这种空话，而是：
// smid 构造混用了 randomUUID 与 Math.random，不注入则夹具回放与黄金向量
// 对拍无法复现。
package core
