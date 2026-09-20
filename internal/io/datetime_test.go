package mcloudio

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/golden"
)

// datetimeFormatVectors 是 `test/golden/datetime-format-vectors.json` 的形状。
//
// 两份都由 **chrono 0.4.45 真跑**产出（生成器
// `scripts/gen-datetime-format-vectors.rs`）—— 跨语言对拍要求生成侧 ≠ 消费侧。
type datetimeFormatVectors struct {
	Emit  []datetimeEmitVector  `json:"$emit"`
	Parse []datetimeParseVector `json:"$parse"`
}

type datetimeEmitVector struct {
	Label string `json:"label"`
	Secs  int64  `json:"secs"`
	Nanos int64  `json:"nanos"`
	Text  string `json:"text"`
}

type datetimeParseVector struct {
	Input string  `json:"input"`
	OK    bool    `json:"ok"`
	UTC   *string `json:"utc"`
}

// loadDatetimeVectors 取向量。
//
// ⚠ 文件名必须是**字面量**：藏进变量会让黄金向量门禁的正则扫不到，
// 于是活向量被标成"死向量" —— 那种失真比漏检更坏，它教人忽略报告。
func loadDatetimeVectors(t *testing.T) datetimeFormatVectors {
	t.Helper()
	raw, err := golden.Load("datetime-format-vectors.json")
	if err != nil {
		t.Fatalf("读向量失败: %v", err)
	}
	var v datetimeFormatVectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatalf("解析向量失败: %v", err)
	}
	if len(v.Emit) == 0 || len(v.Parse) == 0 {
		t.Fatalf("向量缺段：emit=%d parse=%d", len(v.Emit), len(v.Parse))
	}
	return v
}

// TestFormatTimeMatchesChronoVectors 逐字节比对写出格式。
//
// 这条是整份向量存在的理由：`sessions.expires_at` / `users.created_at` /
// `runs.started_at` 都在 SQL 里做字符串比较与排序，**同一列里只能有一种写法**。
// 而 Rust 侧（冻结的那一份）已经往库里写了 `+00:00` 形态的行。
func TestFormatTimeMatchesChronoVectors(t *testing.T) {
	v := loadDatetimeVectors(t)
	for _, c := range v.Emit {
		got := formatTime(time.Unix(c.Secs, c.Nanos).UTC())
		if got != c.Text {
			t.Errorf("%s（secs=%d nanos=%d）与 chrono 不一致:\n实得 %s\n期望 %s",
				c.Label, c.Secs, c.Nanos, got, c.Text)
		}
	}
}

// chronoAcceptsGoRejects 是**已知且刻意保留**的分歧：chrono 收、Go 拒。
//
// 方向是安全的（Go 严格更严），而且这几种 chrono 自己从不写出来 ——
// 实际数据里不会遇到，遇到了就说明是手改或损坏，那时报错才对。
//
// ⚠ 它同时是一张**待复查名单**：向量里若不再出现某个输入，
// 下面的测试会报"名单对不上"，逼人删掉而不是让它烂在那里。
var chronoAcceptsGoRejects = map[string]string{
	"2026-09-20 12:00:00Z": "空格代替 `T`（RFC 3339 允许，Go 的布局要求字面 `T`）",
	"2026-09-20t12:00:00z": "小写 `t` 与小写 `z`",
	"2026-09-20T12:00:00z": "小写 `z`",
	"2026-09-20T12:00:60Z": "闰秒 —— Go 的 time.Time 表示不了 60 秒",
}

// TestParseTimeIsNeverLooserThanChrono 钉住读取侧的**宽严方向**。
//
// 承重的是这一条不变量：**chrono 认为非法的串，Go 绝不能当合法读进来**。
// 反过来的松紧只影响"手改过的数据会不会报错"，不影响正确性。
//
// ⚠ 若哪天 Go 侧被换成更宽松的解析器（比如自己写一个吃掉空格的），
// 这条会红 —— 那正是它该响的时候：多接受一种写法，就等于多一种
// "坏数据被静默当成合法时间"的路径。
func TestParseTimeIsNeverLooserThanChrono(t *testing.T) {
	v := loadDatetimeVectors(t)

	seen := map[string]bool{}
	for _, c := range v.Parse {
		parsed, ok := parseTime(c.Input)

		if !c.OK && ok {
			t.Errorf("chrono 拒绝的串 Go 却接受了（**静默接受坏数据**）: %q → %s",
				c.Input, formatTime(parsed))
			continue
		}
		if c.OK && !ok {
			reason, known := chronoAcceptsGoRejects[c.Input]
			if !known {
				t.Errorf("Go 拒了 chrono 接受的串，但它不在已知分歧名单里: %q\n"+
					"（若是有意收紧，请登记理由；否则说明解析器变严了）", c.Input)
				continue
			}
			t.Logf("已知分歧（Go 更严）：%q —— %s", c.Input, reason)
			seen[c.Input] = true
			continue
		}
		if !c.OK {
			continue
		}

		// 两边都收：解出来的时刻也必须一致 —— 否则"能读"只是假象。
		if c.UTC == nil {
			t.Fatalf("向量里 %q 标了 ok 却没给 utc", c.Input)
		}
		if got := formatTime(parsed); got != *c.UTC {
			t.Errorf("%q 解析出的时刻与 chrono 不同:\n实得 %s\n期望 %s",
				c.Input, got, *c.UTC)
		}
	}

	for input, reason := range chronoAcceptsGoRejects {
		if !seen[input] {
			t.Errorf("已知分歧名单里的 %q 在向量里没再出现（%s）—— 请删掉这条，别让名单烂掉",
				input, reason)
		}
	}
}
