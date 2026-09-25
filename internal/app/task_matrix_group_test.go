package app

import (
	"context"
	"encoding/json"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// taskMatrixGroupBody 手拼**一次**拉取的响应：条目原样放进去，`groupState` 也原样给。
//
// 为什么不走 `taskMatrixBody`：那个辅助把 `groupState` 钉成"永远是本次那个组、永远 2"，
// 而 R15 要验的恰恰是"键名与请求那一个组对不对得上""值随不随完成度动"两格 ——
// 用固定 2 的辅助拼不出这两种样子。
func taskMatrixGroupBody(t *testing.T, group, groupState string, items ...string) string {
	t.Helper()
	body := `{"code":0,"msg":"success","result":{"` + group + `":[` +
		strings.Join(items, ",") + `]},"groupState":` + groupState + `}`
	if !json.Valid([]byte(body)) {
		t.Fatalf("拼出来的响应不合法（改辅助时先修这里）: %s", body)
	}
	return body
}

// taskMatrixGroupItem 拼一条任务：`process` 传 nil ⇒ **不给那一键**（要演"读不到"那一档）。
func taskMatrixGroupItem(id int64, name, state string, process *int64) string {
	parts := []string{
		`"id":` + strconv.FormatInt(id, 10),
		`"name":` + quoteJSONString(name),
		`"state":` + quoteJSONString(state),
		`"enable":1`, `"groupid":"day"`, `"marketname":"sign_in_3"`,
	}
	if process != nil {
		parts = append(parts, `"process":`+strconv.FormatInt(*process, 10))
	}
	return "{" + strings.Join(parts, ",") + "}"
}

func mxInt64(v int64) *int64 { return &v }

// TestAppTaskMatrixRecordsGroupReadings ⭐ R15 的判据面：组级读数要**留得下来**。
//
// 反向改坏它的三处，各自会红：
//   - 把 `reading.States` 的收集改成"只取请求那一个组名"⇒ 第四个断言红
//     （`beiyong1` 那一条键被丢掉，而"响应给的键与请求组名对不对得上"正是 R15 要看的一格）；
//   - 把 `reading.Finished` 数成"条目总数"⇒ 第二个断言红（组内并非全完成这一档读不出来）；
//   - 把 `ProcessValues` 的"缺失"也算成一档 ⇒ 第一个断言红（读出来是 3 档）。
//
// ⚠ 顺带钉住**这一行念不出的那一格**：`{"day":{"taskFinishState":null}}` 那枚键在
// 上游层就被 [parseGroupState] 丢了（它只收整数档）⇒ 界面上"没给"与"给了但不是整数"
// 是**同一档**。这不是本层的判断，是解析器的形状，改判据之前先回看这里。
func TestAppTaskMatrixRecordsGroupReadings(t *testing.T) {
	body := taskMatrixGroupBody(t, "new",
		`{"beiyong1":{"taskFinishState":2},"day":{"taskFinishState":null}}`,
		taskMatrixGroupItem(700, "做了的", "FINISH", mxInt64(0)),
		taskMatrixGroupItem(701, "没做的", "WAIT", mxInt64(7)),
		taskMatrixGroupItem(702, "没给进度", "WAIT", nil),
	)
	bodies := taskMatrixMarketQueue(taskMatrixEmptyBody, taskMatrixEmptyBody)
	bodies[0] = body
	script := taskMatrixScript([]string{"JWT-1"},
		liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if len(matrix.Groups) != TaskMatrixFetches() {
		t.Fatalf("每一次拉取都要留一段读数，实得 %d 段（应 %d）", len(matrix.Groups), TaskMatrixFetches())
	}
	first := matrix.Groups[0]
	if first.MarketName != upstream.MarketName || first.Group != upstream.TaskListV2Groups[0] {
		t.Fatalf("第一段要点各自的市场名与**请求**组名，实得 %s/%s", first.MarketName, first.Group)
	}
	if first.Items != 3 || first.Finished != 1 {
		t.Errorf("条数与其中 FINISH 的条数：期望 3 / 1，实得 %d / %d", first.Items, first.Finished)
	}
	if len(first.ProcessValues) != 2 || first.ProcessValues[0] != 0 || first.ProcessValues[1] != 7 {
		t.Errorf("process 面值要**升序去重**、且缺失的不算一档，实得 %v", first.ProcessValues)
	}
	if len(first.States) != 1 || first.States[0].Group != "beiyong1" ||
		first.States[0].Value == nil || *first.States[0].Value != 2 {
		t.Errorf("groupState 要**全键照抄**（含不等于请求组名的那些），实得 %+v", first.States)
	}
	// 其余那十五次（空对象 + 空条目）也要各留一段，不能只留非空的那些。
	last := matrix.Groups[len(matrix.Groups)-1]
	if last.Items != 0 || len(last.States) != 0 || len(last.ProcessValues) != 0 {
		t.Errorf("空响应那一段该念成\"没给 + 回 0 条\"，实得 %+v", last)
	}
}

// TestTaskMatrixGroupReadingLines ⭐ 同一份读数**每个市场名一行**，行头带标签。
//
// 为什么单独钉渲染：这一行是 R15 唯一的证据出口，而它的两截（组级 / 条目级）必须
// 在一行里对得上 —— 分两行会让人以为念了两次。
//
// 反向改坏它的两处，各自会红：
//   - `label` 为空时仍拼括号 ⇒ 第三条断言红（"拉表后"那一档的行头变成 `组读数（）：`）；
//   - 按市场名分组那段写成"全塞进一行"⇒ 第一条断言红（只剩一行）。
func TestTaskMatrixGroupReadingLines(t *testing.T) {
	matrix := TaskMatrix{Groups: []TaskMatrixGroupCount{
		{MarketName: upstream.MarketName, Group: "beiyong1", Items: 3, Finished: 3,
			ProcessValues: []int64{0, 7},
			States:        []TaskMatrixStateEntry{{Group: "beiyong1", Value: mxInt64(2)}}},
		{MarketName: upstream.MarketName, Group: "day", Items: 1, Finished: 0,
			ProcessValues: []int64{}, States: []TaskMatrixStateEntry{}},
		{MarketName: upstream.TaskListV2EmailMarketName, Group: "time", Items: 0,
			Finished: 0, ProcessValues: []int64{}, States: []TaskMatrixStateEntry{}},
	}}
	lines := matrix.GroupReadingLines("拉表前")
	if len(lines) != 2 {
		t.Fatalf("两个市场名各一行，实得 %d 行：%v", len(lines), lines)
	}
	if want := "组读数（拉表前） sign_in_3：beiyong1：taskFinishState{beiyong1=2}、3 条（FINISH 3）、process{0、7}；day：taskFinishState 没给、1 条（FINISH 0）、process 全没给"; lines[0] != want {
		t.Errorf("云盘那一行不合：\n got %q\nwant %q", lines[0], want)
	}
	if want := "组读数（拉表前） newsign_139mail：time：taskFinishState 没给、回 0 条"; lines[1] != want {
		t.Errorf("邮箱那一行不合：\n got %q\nwant %q", lines[1], want)
	}
	// 标签空 ⇒ 只写"组读数"，不带空括号。
	plain := TaskMatrix{Groups: matrix.Groups[:1]}.GroupReadingLines("")
	if len(plain) != 1 || !strings.HasPrefix(plain[0], "组读数 sign_in_3：") {
		t.Errorf("不给标签时行头不该有空括号，实得 %v", plain)
	}
	// 没有读数 ⇒ 不给行（与 [MarketSummaryLine] 空表回空串同一取舍）。
	if got := (TaskMatrix{}).GroupReadingLines("拉表前"); got != nil {
		t.Errorf("没有读数时不该播一行，实得 %v", got)
	}
}

// TestTaskMatrixRunBroadcastsGroupReadingsTwice ⭐ R15 的"动没动"只能靠同一趟的两次并排。
//
// 这一条钉的是**播报**（前两行都在、且顺序对），不是判据：跑完之后"这一组从 2 变 3"
// 该怎么解释是 R15 那一问的答案，不在代码里。
//
// 反向改坏它的三处，各自会红：
//   - 删掉 `verify` 里那一句 ⇒ 第二条断言红（复查后那一行整个消失，前后对比无从做起）；
//   - 把两次都标成"拉表前"⇒ 第三条断言红（两行分不清谁在前）；
//   - 把 `second` 换成 `matrix`（念第一趟那份）⇒ 第四个断言红（FINISH 那一格停在 0）。
func TestTaskMatrixRunBroadcastsGroupReadingsTwice(t *testing.T) {
	before := mxRound(map[int]string{0: taskMatrixGroupBody(t, "new",
		`{"new":{"taskFinishState":2}}`, mxItem(700, "new", "看一枪", "WAIT", "", 0))})
	after := mxRound(map[int]string{0: taskMatrixGroupBody(t, "new",
		`{"new":{"taskFinishState":3}}`, mxItem(700, "new", "看一枪", "FINISH", "", 0))})
	script := mxScript([]string{"JWT-1"}, before, after, mxHappy()...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	if _, perr := mxResult(t, run); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	first, second := log.lineWith("组读数（拉表前）"), log.lineWith("组读数（复查后）")
	if first == "" || second == "" {
		t.Fatalf("两次读数都要出现在事件流里，实得：\n%s", log.joined())
	}
	if !strings.Contains(first, "new：taskFinishState{new=2}、1 条（FINISH 0）") {
		t.Errorf("拉表前那一行要念得出组级面值与完成条数，实得 %q", first)
	}
	if !strings.Contains(second, "new：taskFinishState{new=3}、1 条（FINISH 1）") {
		t.Errorf("复查后那一行该念**第二次**的读数，实得 %q", second)
	}
	if a, b := log.indexOf("组读数（拉表前）"), log.indexOf("组读数（复查后）"); a > b {
		t.Errorf("两行要按前→后排列，实得 前=%d 后=%d：\n%s", a, b, log.joined())
	}
	// 级别：这是读数，不是告警（分节与需前往那两档才换级别）。
	if !log.hasAt(core.LevelInfo, "组读数（复查后）") {
		t.Errorf("读数行该走 info 档，实得：\n%s", log.joined())
	}
}
