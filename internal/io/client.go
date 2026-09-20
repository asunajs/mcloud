package mcloudio

import "net/http"

// Client 是上游请求用的 HTTP 客户端。
//
// 骨架阶段它只是 net/http.Client 的一层薄包装 —— 但它**必须是本包的类型**。
// scripts/check-go-deps.sh 断言「组装根（app / server）必须依赖本包」，
// 那条断言挡的是"没人接上 IO 实现"这个**静默死配置**：编译能过、跑起来什么都没发生。
// 若组装层直接持 net/http.Client，那条断言就失去了意义。
type Client struct {
	*http.Client
}

// NewClient 返回一个默认配置的客户端。
//
// ⚠ 迁移时这里要长出真正的实现：cookie jar、gzip / brotli / deflate 解压、
// 超时与重试。Rust 侧对应 crates/mcloud-io 里的 reqwest 封装。
// 注意 Go 的 net/http **只自动解 gzip**，brotli 要自己接 transport。
func NewClient() *Client {
	return &Client{Client: &http.Client{}}
}
