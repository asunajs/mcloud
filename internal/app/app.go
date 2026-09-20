package app

import mcloudio "github.com/catlair/mcloud/internal/io"

// Deps 是组装层持有的注入依赖。
//
// ⚠ 它现在只有一个字段，但**不要因为"暂时用不上"就删掉这个结构体或那个字段**。
// scripts/check-go-deps.sh 断言「组装根必须依赖 internal/io」，方向与
// 「纯层不许依赖 io」同样重要 —— 它挡的是"没人接上 IO 实现"这个**静默死配置**：
// 编译能过、跑起来什么都没发生，而没有任何测试会变红。
type Deps struct {
	// HTTP 是上游请求用的客户端。
	HTTP *mcloudio.Client
}

// New 用真实实现接好线，返回组装层。
//
// 迁移后这里会长出签到链路、登录驱动、账号档案等入口；
// 现在它存在的意义是让"组装根真的接上了 IO"这条断言**可被执行**。
func New(http *mcloudio.Client) *Deps {
	return &Deps{HTTP: http}
}
