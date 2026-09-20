// Package capabilities 是能力注册表 —— CLI 与 WebUI 的唯一契约来源。
//
// 对应 Rust 侧的 crates/mcloud-capabilities。
//
// # 分层铁律
//
// 与 core / upstream 同属**纯层**，禁止任何 IO，由 scripts/check-go-deps.sh 断言。
//
// 之所以要"唯一契约来源"：两个产物（命令行与 HTTP 服务）如果各自描述一遍
// 同一批能力，就会出现"CLI 有、WebUI 没有"这种只有人肉比对才发现的漂移。
package capabilities
