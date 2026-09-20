// Package golden 定位并读取仓库根的 test/golden/*.json 黄金向量。
//
// 对应 Rust 侧的 include_str!("../../../../../test/golden/*.json")。
//
// # 为什么单独一个包
//
// Go 的 //go:embed **不能跨目录**（只能嵌入本包目录下的文件），而黄金向量是
// 全仓共享资产：Node 校验器（scripts/check-golden-vectors.cjs）、冻结的 Rust
// 实现、以及 Go 侧多个包都要读**同一份**。把向量复制到各包的 testdata/
// 会让"单一真相"失效 —— 于是这里不复制，只把"路径怎么算"封装掉。
//
// # ⚠ 只供测试使用
//
// 本包会读磁盘，不属于任何一层，也不该被生产代码 import。
// 若某天生产代码需要它，说明有东西放错了层。
package golden

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
)

var (
	once sync.Once
	root string
	err  error
)

// Root 返回仓库根目录（即含 go.mod 的那一层）。
//
// 用「从 CWD 向上找 go.mod」而不是 runtime.Caller —— 后者在加了 -trimpath
// 之后会失效，而 Go 测试的 CWD 稳定地是包目录，向上找是可靠且不依赖
// 源码路径是否被剥掉的做法。
func Root() (string, error) {
	once.Do(func() {
		wd, e := os.Getwd()
		if e != nil {
			err = e
			return
		}
		d := wd
		for {
			if _, e := os.Stat(filepath.Join(d, "go.mod")); e == nil {
				root = d
				return
			}
			parent := filepath.Dir(d)
			if parent == d {
				err = fmt.Errorf("从 %s 向上未找到 go.mod，无法定位仓库根", wd)
				return
			}
			d = parent
		}
	})
	return root, err
}

// Dir 返回 test/golden 的绝对路径。
func Dir() (string, error) {
	r, e := Root()
	if e != nil {
		return "", e
	}
	return filepath.Join(r, "test", "golden"), nil
}

// Load 读取 test/golden 下的一个向量文件。
//
// ⚠ name 必须是**字面量**：scripts/check-golden-vectors.cjs 靠正则
// `golden.Load("…")` 判断"这份向量有没有人读"。把文件名藏进包装函数
// 会让探测静默失配，把读得好好的向量标成"死向量" —— 那种失真比漏检更坏。
func Load(name string) ([]byte, error) {
	d, e := Dir()
	if e != nil {
		return nil, e
	}
	return os.ReadFile(filepath.Join(d, name))
}

// FixtureDir 返回 test/fixtures 的绝对路径。
//
// 与 [Dir] 的区别是**这一层装的是什么**：
//
//   - `test/golden/*.json` 是**向量** —— 由规则算出（或由参考实现的库产出），
//     回答"这个变换的结果应该是什么"；
//   - `test/fixtures/**` 是**夹具** —— 由真实数据脱敏而来，
//     回答"真实世界长什么样"。
//
// 两者都是"生成侧 ≠ 消费侧"的对拍素材，但只有前者参与
// check-golden-vectors.cjs 的"有没有人读"探测。
func FixtureDir() (string, error) {
	r, e := Root()
	if e != nil {
		return "", e
	}
	return filepath.Join(r, "test", "fixtures"), nil
}

// LoadFixture 读取 test/fixtures 下的一个夹具（相对路径，如 `legacy/state.json`）。
func LoadFixture(relPath string) ([]byte, error) {
	d, e := FixtureDir()
	if e != nil {
		return nil, e
	}
	return os.ReadFile(filepath.Join(d, filepath.FromSlash(relPath)))
}
