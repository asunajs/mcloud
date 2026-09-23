// Package upstream 是协议层：上游接口的请求构造、响应解析与签名。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-upstream。
//
// # 分层铁律
//
// 本包与 core 同属**纯层**，禁止任何 IO，由 scripts/check-go-deps.js 断言：
//
//   - 不得 import net/http、database/sql；
//   - 不得 import github.com/catlair/mcloud/internal/io。
//
// 需要签名或设备指纹时用 internal/core 的纯函数，**不要**去依赖 io 层 ——
// 那会把 HTTP 客户端整个拖进本包的依赖图。
//
// # 铁律：跨端常量不许互搬
//
// 本包里同名同形的常量值可能因**端**（PC / App）与**环境**而异，且错了是
// **静默失效**、不报错。拿一个常量之前先问两句：「它出现在哪个端？由谁发出？」
//
// 详见 docs/client-profiles.md 与 docs/device-and-source-codes.md。
package upstream
