package server

import mcloudio "github.com/catlair/mcloud/internal/io"

// Deps 是 HTTP 产物持有的依赖。
//
// ⚠ 与 internal/app 同理：必须真的依赖 internal/io，否则
// scripts/check-go-deps.sh 会红 —— 那条断言挡的是"没人接上 IO 实现"这个
// 静默死配置，不是形式主义。
type Deps struct {
	// HTTP 是上游请求用的客户端。
	HTTP *mcloudio.Client
}
