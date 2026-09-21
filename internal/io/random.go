package mcloudio

import (
	"crypto/rand"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 系统随机源
// ─────────────────────────────────────────────────────────────────────────────

// 编译期断言：这两个实现必须真的满足 core 那两个接口。
//
// ⚠ 别小看这两行。Go 的接口是**结构性**实现 —— 悄悄改掉一个方法签名，
// 唯一的报错会出现在组装根（app / server）而不是这里；
// 而 Rust 侧的 `impl Random for SystemRandom` 是**名义**的，改错了当场就在本文件红。
var (
	_ core.Random = SystemRandom{}
	_ core.Clock  = SystemClock{}
)

// SystemRandom 是生产随机源：直接读 OS 的 CSPRNG。
//
// 放在 io 层而不是 core：取系统熵是 IO，core 的纯净性靠这条边界维持。
// core 只定义 [core.Random] 接口，测试里换成固定脚本的实现 ——
// 这正是"设备指纹能被 fixture 复现"的前提。
type SystemRandom struct{}

// FillBytes 用密码学安全的随机字节填满 dst。
//
// ⚠ 取不到熵时**必须失败**而不是退化成弱随机：弱随机会让设备指纹 / smid
// 可预测，进而影响风控判定。所以这里读的是 [crypto/rand]（它失败时返回 error），
// 且失败就 panic —— 拿不出熵的程序不该继续以"看起来正常"的方式跑下去。
func (SystemRandom) FillBytes(dst []byte) {
	if _, err := rand.Read(dst); err != nil {
		// 不去 fallback 到 math/rand：那正是"弱随机"这条路的入口。
		panic("mcloud: 取不到系统熵，拒绝退化成弱随机: " + err.Error())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 真实时钟
// ─────────────────────────────────────────────────────────────────────────────

// SystemClock 是真实时钟。
//
// 同样放在 io 层而不是 core：`time.Now()` 是 IO。
type SystemClock struct{}

// Now 返回当前时刻，**归一化到 UTC**（[core.Clock] 的契约要求如此）。
func (SystemClock) Now() time.Time { return time.Now().UTC() }
