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

// TestProcessChangeLines ⭐ R15 的归因那一格：涨的是**哪一条**要点了名。
//
// 反向改坏它的四处，各自会红：
//   - 面值相同也出行 ⇒ 第一条断言红（把"没动"念成"动了"，等于制造假样本）；
//   - 把"没给"折成 0（用 [taskProcessOf] 而不是 [sameProcess]）⇒ 第二条断言红
//     （"没给 → 0"这一格恰恰是要看见的，它不是没变化）；
//   - 吞掉"只出现在一遍"的那条 ⇒ 第三条断言红；
//   - 不排序 ⇒ 第四条断言红（明细行的次序变成 map 顺序，每次跑不一样）。
func TestProcessChangeLines(t *testing.T) {
	item := func(id int64, name string, process *int64) upstream.TaskV2Item {
		return upstream.TaskV2Item{TaskItem: upstream.TaskItem{
			TaskID: mxInt64(id), Name: strPtr(name)}, Process: process}
	}
	mx := func(items ...upstream.TaskV2Item) TaskMatrix { return TaskMatrix{Items: items} }
	before := mx(
		item(500, "没动的", mxInt64(0)),
		item(522, "当月上传文件满100个", mxInt64(64)),
		item(501, "从没给到 0", nil),
		item(502, "消失的那条", mxInt64(3)),
	)
	after := mx(
		item(500, "没动的", mxInt64(0)),
		item(522, "当月上传文件满100个", mxInt64(69)),
		item(501, "从没给到 0", mxInt64(0)),
		item(503, "新出现的那条", mxInt64(7)),
	)
	lines := ProcessChangeLines("复查后", before, after)
	if len(lines) != 5 {
		t.Fatalf("一行总结 + 四行明细（没动的那条不出行），实得 %d 行：%v", len(lines), lines)
	}
	if want := "process 逐条对照（复查后）：对照 5 条、面值变了 2 条、消失 1 条、新出现 1 条"; lines[0] != want {
		t.Errorf("总结那行不合：\n got %q\nwant %q", lines[0], want)
	}
	// 明细按 id 升序：501 / 502 / 503 / 522（500 没动 ⇒ 不出行）。
	if want := "process 逐条对照（复查后） 从没给到 0 501：没给 → 0"; lines[1] != want {
		t.Errorf("\"没给\"要单独成一档、不得折成 0：\n got %q\nwant %q", lines[1], want)
	}
	if want := "process 逐条对照（复查后） 消失的那条 502：3 ⇒ 复查那遍没这条"; lines[2] != want {
		t.Errorf("只出现在一遍的条目也要出行：\n got %q\nwant %q", lines[2], want)
	}
	if want := "process 逐条对照（复查后） 新出现的那条 503：拉表前没这条 ⇒ 复查读到 7"; lines[3] != want {
		t.Errorf("新出现的条目也要出行：\n got %q\nwant %q", lines[3], want)
	}
	if want := "process 逐条对照（复查后） 当月上传文件满100个 522：64 → 69"; lines[4] != want {
		t.Errorf("明细要按 id 升序（否则每次跑出来顺序不一样）：\n got %q\nwant %q", lines[4], want)
	}
	// 没有任何条目 ⇒ 总结那行仍要给（"没跑这一格"与"跑了、没变化"不能同一形状）。
	empty := ProcessChangeLines("", TaskMatrix{}, TaskMatrix{})
	if len(empty) != 1 || !strings.HasPrefix(empty[0], "process 逐条对照：对照 0 条") {
		t.Errorf("空表也要给总结那行、且不带空括号，实得 %v", empty)
	}
	// 没 id 的条目：不参与对照，但要在总结里报出来（不能读成"没变化"）。
	noID := TaskMatrix{Items: []upstream.TaskV2Item{
		{TaskItem: upstream.TaskItem{Name: strPtr("没 id")}}}}
	if got := ProcessChangeLines("", noID, noID); len(got) != 1 ||
		!strings.Contains(got[0], "另有 2 条没有 id、未参与对照") {
		t.Errorf("没 id 的条数要报出来，实得 %v", got)
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

// mxItemMarketAt 造一条"同一 id、属于指定市场名、`process` 给定值"的条目。
//
// [mxItem] 的 `process` 恒为 0，而 [mxItemProcessAt] 只能改那一格 ⇒ 拼"两边给同一条
// 不同面值"这一形状得两个动作合成一个。
func mxItemMarketAt(id int64, group, name, market string, process int64) string {
	return mxItemProcessAt(mxItem(id, group, name, "WAIT", market, 0), process)
}

// TestDedupLossLines ⭐ R15 归因剩下的那一格：**输掉的那一份**读的是几、还要不要做。
//
// 反向改坏它的七处，各自会红：
//   - `id` 与 `process` 都一样也出行 ⇒ 第一条断言红（一次跑平白多出七八行噪声，
//     而 R15 要的不是"上游给了几份"）；
//   - `id` 不同也当成"同一条"念 ⇒ 第二条断言红（那说明去重键 `name` 撞在了两条
//     不同的任务上，是本层口径的一部分，得看得见）；
//   - 总结行缺 `丢掉 N 份` / 缺"待做"那一格 / 只在有明细时才给 ⇒ 第三、四条断言红
//     （"这一格没跑"与"跑了、只是没差别"必须是两种形状）；
//   - 把"没 id"折成 `0`、"没市场名 / 没 state / 没 enable"折成空串 ⇒ 第五条断言红
//     （那是"上游没给这一格"，不是一次实测读数）；
//   - 字面相同的两份明细各起一行 ⇒ 第六条断言红（第九趟实拍就有两份逐字相同的输掉条目：
//     同一个 549 在两个组里各出现一次）；
//   - "待做"那一格把 `enable=0` 的也算进去 ⇒ 第七条断言红（那道门是 `enable==1` 且非 FINISH）；
//   - `label` 为空仍拼括号 ⇒ 第八条断言红。
func TestDedupLossLines(t *testing.T) {
	item := func(id int64, name, market, state string, enable, process *int64) upstream.TaskV2Item {
		out := upstream.TaskV2Item{TaskItem: upstream.TaskItem{
			TaskID: mxInt64(id), Name: strPtr(name)}, Process: process, Enable: enable}
		if market != "" {
			value := market
			out.MarketName = &value
		}
		if state != "" {
			value := state
			out.State = &value
		}
		return out
	}
	on := func() *int64 { return mxInt64(1) }
	const email = upstream.TaskListV2EmailMarketName
	matrix := TaskMatrix{
		// 留下的那份：522 读到 78（云盘市场名）；501 与输掉的那份逐字相同（不出行）；
		// 601 与输掉的 600 **同名不同 id** ⇒ 去重把它们当成了一条（第九趟实拍就是这个形状）。
		Items: []upstream.TaskV2Item{
			item(501, "两边一模一样", upstream.MarketName, "FINISH", on(), mxInt64(0)),
			item(522, "当月上传文件满100个", upstream.MarketName, "WAIT", on(), mxInt64(78)),
			item(601, "同名但不是同一条", email, "WAIT", on(), mxInt64(2)),
		},
		Duplicates: []upstream.TaskV2Item{
			item(501, "两边一模一样", email, "FINISH", on(), mxInt64(0)),
			item(522, "当月上传文件满100个", email, "WAIT", on(), mxInt64(79)),
			item(600, "同名但不是同一条", upstream.MarketName, "WAIT", on(), mxInt64(5)),
			// 同一个 600 在另一个组里又出现一次：读数逐字相同 ⇒ 合行，不另起一行。
			item(600, "同名但不是同一条", upstream.MarketName, "WAIT", on(), mxInt64(5)),
		},
	}
	lines := DedupLossLines("复查后", matrix)
	if len(lines) != 3 {
		t.Fatalf("一行总结 + 两行明细（一模一样那份不出行、两份相同的合一行），实得 %d 行：%v", len(lines), lines)
	}
	if want := "同名去重（复查后）输掉的那一份 当月上传文件满100个：同一条｜留下 id=522 state=WAIT enable=1 process=78 市场名 sign_in_3｜输掉 id=522 state=WAIT enable=1 process=79 市场名 newsign_139mail"; lines[1] != want {
		t.Errorf("差 1 的那一条要把两边的读数、市场名和\"这一份要不要做\"都点出来：\n got %q\nwant %q", lines[1], want)
	}
	if want := "同名去重（复查后）输掉的那一份 同名但不是同一条：同名但 id 不同 ⇒ 不是同一条｜留下 id=601 state=WAIT enable=1 process=2 市场名 newsign_139mail｜输掉 id=600 state=WAIT enable=1 process=5 市场名 sign_in_3（同一读数重复 2 份）"; lines[2] != want {
		t.Errorf("id 不同要念成\"不是同一条\"、两份相同要合行：\n got %q\nwant %q", lines[2], want)
	}
	if want := "同名去重（复查后）输掉的那一份：丢掉 4 份、其中与留下的那份读得不一样 3 份、那些里面按本仓那道门待做 3 份（合成 2 行）"; lines[0] != want {
		t.Errorf("总结那行要在最前面，并把\"丢了几份 / 几份不同 / 几份本来要做\"三格分开给：\n got %q\nwant %q", lines[0], want)
	}
	// 全都没差别 ⇒ 只剩总结那一行（它买的是"跑过了、只是没新读数"）。
	quiet := TaskMatrix{
		Items: []upstream.TaskV2Item{item(501, "两边一模一样", upstream.MarketName, "FINISH", on(), mxInt64(0))},
		Duplicates: []upstream.TaskV2Item{
			item(501, "两边一模一样", email, "FINISH", on(), mxInt64(0))},
	}
	only := DedupLossLines("", quiet)
	if len(only) != 1 ||
		only[0] != "同名去重输掉的那一份：丢掉 1 份、其中与留下的那份读得不一样 0 份、那些里面按本仓那道门待做 0 份" {
		t.Errorf("零差别时只留总结行、且不带空括号与\"合成 N 行\"，实得 %v", only)
	}
	// 没 id / 没市场名 / 没 state / 没 enable ⇒ 念"没给"，不折成 0 或空串。
	blank := TaskMatrix{
		Items: []upstream.TaskV2Item{{TaskItem: upstream.TaskItem{Name: strPtr("没 id 没市场名")},
			Process: mxInt64(0)}},
		Duplicates: []upstream.TaskV2Item{{TaskItem: upstream.TaskItem{Name: strPtr("没 id 没市场名")},
			Process: mxInt64(3)}},
	}
	blankLines := DedupLossLines("拉表前", blank)
	if len(blankLines) != 2 || !strings.Contains(blankLines[1],
		"留下 id=没给 state=没给 enable=没给 process=0 市场名 没给｜输掉 id=没给 state=没给 enable=没给 process=3 市场名 没给") {
		t.Errorf("\"没给\"要念得出（不得折成 0 / 空串），实得 %v", blankLines)
	}
	// `enable=0` 的那一份**不算待做**（门是 enable==1），但仍然出行。
	off := TaskMatrix{
		Items: []upstream.TaskV2Item{item(700, "关着的那条", upstream.MarketName, "WAIT", on(), mxInt64(1))},
		Duplicates: []upstream.TaskV2Item{
			item(701, "关着的那条", email, "WAIT", mxInt64(0), mxInt64(2))},
	}
	offLines := DedupLossLines("", off)
	if len(offLines) != 2 || !strings.Contains(offLines[0], "待做 0 份") ||
		!strings.Contains(offLines[1], "enable=0") {
		t.Errorf("enable 不是 1 就不算待做（但明细照给）：\n%v", offLines)
	}
	// 一条都没丢掉 ⇒ 不给行（没有读数可念时不播一句"没读数"，同 [MarketSummaryLine]）。
	if got := DedupLossLines("拉表前", TaskMatrix{Items: blank.Items}); got != nil {
		t.Errorf("没丢过东西时不该播行，实得 %v", got)
	}
	// 输掉的那份在留下的表里找不到同名 ⇒ 出行不了，但**总结里仍数得着**。
	orphan := TaskMatrix{
		Items: []upstream.TaskV2Item{item(501, "留下的", upstream.MarketName, "FINISH", on(), mxInt64(0))},
		Duplicates: []upstream.TaskV2Item{
			item(700, "对不上号", email, "WAIT", on(), mxInt64(9))},
	}
	if got := DedupLossLines("", orphan); len(got) != 1 ||
		!strings.Contains(got[0], "丢掉 1 份、其中与留下的那份读得不一样 0 份") {
		t.Errorf("对不上同名的那份只进总结，实得 %v", got)
	}
}

// TestAppTaskMatrixKeepsDedupLosses ⭐ R15 的判据面：输掉的那一份要**留得下来**。
//
// 没有这一格，"两边差 1"这件事在去重后的表里**根本不存在** —— 第八趟真跑的读数就是
// 这个形状（留下的 78、输掉的 79），光读 [TaskMatrix.Items] 永远答不出另一边是谁。
//
// 反向改坏它的两处，各自会红：
//   - `normalizeTaskMatrix` 只交回 `kept`（丢掉的不留）⇒ 第一条断言红；
//   - 把赋值写成 `matrix.Items, _ = …` 或把 `dropped` 覆盖成 `matrix.Items`
//     ⇒ 第二条断言红（留下的是 78 那一份，不是 79）。
func TestAppTaskMatrixKeepsDedupLosses(t *testing.T) {
	bodies := mxRound(map[int]string{
		0: mxBody(mxItemMarketAt(522, "new", "当月上传文件满100个", upstream.MarketName, 78)),
		8: mxBody(mxItemMarketAt(522, "new", "当月上传文件满100个",
			upstream.TaskListV2EmailMarketName, 79)),
	})
	script := taskMatrixScript([]string{"JWT-1"},
		liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if len(matrix.Items) != 1 || len(matrix.Duplicates) != 1 {
		t.Fatalf("同名两份 ⇒ 留 1 输 1，实得留 %d 输 %d", len(matrix.Items), len(matrix.Duplicates))
	}
	if raw, got := matrix.Items[0].Process, int64(78); got != *raw {
		t.Errorf("留下的该是 id 小的赢之后**请求顺序**里那一份（78），实得 %d", *raw)
	}
	lost := matrix.Duplicates[0]
	if *lost.Process != 79 || lost.MarketName == nil ||
		*lost.MarketName != upstream.TaskListV2EmailMarketName {
		t.Errorf("输掉的那一份要原样留着（79 / newsign_139mail），实得 %v / %v",
			*lost.Process, lost.MarketName)
	}
	// 留下来的读数进得了那一行 —— 这一格才是 R15 的出口。
	lines := DedupLossLines("拉表前", matrix)
	if len(lines) != 2 || !strings.Contains(lines[1], "留下 id=522 state=WAIT enable=1 process=78") ||
		!strings.Contains(lines[1], "输掉 id=522 state=WAIT enable=1 process=79") {
		t.Errorf("78/79 这一对照（连同『这一份要不要做』）要念得出来，实得 %v", lines)
	}
	if !strings.Contains(lines[0], "待做 1 份") {
		t.Errorf("输掉的那一份本来要做 ⇒ 总结里要数得着，实得 %v", lines)
	}
}

// TestTaskMatrixRunBroadcastsDedupLosses ⭐ 那一行要**真的出现在事件流里**，两遍各一次。
//
// 夹具照第八趟真跑的形状：**拉表前**两边同值（差别不出行、只剩总结），
// **复查后**才差 1 ⇒ 只念拉表前那一遍就看不见它（这正是当初漏掉的那格）。
//
// ⚠ 条目名字用一条普通任务（[mxItem] 那套）而不是 522：本条只验**播报**，
// 用"当月上传文件满100个"会把上传那一串请求拖进来（那是 522 自己的用例该管的事）。
//
// 反向改坏它的四处，各自会红：
//   - 删掉 `verify` 里那一句 ⇒ 第二条断言红（复查后那行整个消失）；
//   - 复查那一句念成 `matrix`（第一趟那份）⇒ 第三条断言红（差 1 那一格读不出来）；
//   - 两遍都标"拉表前" ⇒ 第四条断言红（分不清谁在前）；
//   - 把拉表前那一句挪到组读数**之前** ⇒ 第五条断言红（两半被读成两件不同的事）。
func TestTaskMatrixRunBroadcastsDedupLosses(t *testing.T) {
	before := mxRound(map[int]string{
		0: mxBody(mxItemMarketAt(700, "new", "看一枪", upstream.MarketName, 74)),
		8: mxBody(mxItemMarketAt(700, "new", "看一枪", upstream.TaskListV2EmailMarketName, 74)),
	})
	after := mxRound(map[int]string{
		0: mxBody(mxItemMarketAt(700, "new", "看一枪", upstream.MarketName, 78)),
		8: mxBody(mxItemMarketAt(700, "new", "看一枪", upstream.TaskListV2EmailMarketName, 79)),
	})
	script := mxScript([]string{"JWT-1"}, before, after, mxHappy()...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	if _, perr := mxResult(t, run); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	first, second := log.lineWith("同名去重（拉表前）"), log.lineWith("同名去重（复查后）")
	if first == "" || second == "" {
		t.Fatalf("两遍都要播，实得：\n%s", log.joined())
	}
	if !strings.Contains(first, "丢掉 1 份、其中与留下的那份读得不一样 0 份") {
		t.Errorf("拉表前两边同值 ⇒ 只剩总结那一行（有差别才出明细），实得 %q", first)
	}
	if !strings.Contains(second, "丢掉 1 份、其中与留下的那份读得不一样 1 份") {
		t.Errorf("复查后总结那行要报出\"丢了几份 / 其中几份读得不一样\"，实得 %q", second)
	}
	// 明细是**独立的一行**（排在总结后面），所以得按内容另找一行，不能在总结里找。
	detail := log.lineWith("输掉 id=700")
	if detail == "" || !strings.Contains(detail, "留下 id=700 state=WAIT enable=1 process=78") ||
		!strings.Contains(detail, "输掉 id=700 state=WAIT enable=1 process=79") {
		t.Errorf("复查后要念得出**差的那 1 是谁给的**、以及那一份本来要不要做，实得 %q：\n%s", detail, log.joined())
	}
	if !strings.Contains(second, "待做 1 份") {
		t.Errorf("总结那行要数得出『本来还要做』的份数，实得 %q", second)
	}
	if a, b := log.indexOf("同名去重（复查后）"), log.indexOf("输掉 id=700"); a > b {
		t.Errorf("明细要排在总结那行后面，实得 总结=%d 明细=%d", a, b)
	}
	if a, b := log.indexOf("同名去重（拉表前）"), log.indexOf("同名去重（复查后）"); a > b {
		t.Errorf("两行要按前→后排列，实得 前=%d 后=%d：\n%s", a, b, log.joined())
	}
	if !log.hasAt(core.LevelInfo, "同名去重（复查后）") {
		t.Errorf("读数行该走 info 档，实得：\n%s", log.joined())
	}
	// 拉表前那一遍：去重行紧贴组读数（两个市场名 ⇒ 中间只隔两行）。
	if a, b := log.indexOf("组读数（拉表前）"), log.indexOf("同名去重（拉表前）"); b != a+2 {
		t.Errorf("去重行该紧跟在两行组读数后面，实得 组=%d 去重=%d：\n%s", a, b, log.joined())
	}
	// 复查后那一遍：组读数 → 逐条对照 → 去重读数（三者是同一格读数的三半，不得倒序）。
	if a, b, c := log.indexOf("组读数（复查后）"), log.indexOf("process 逐条对照（复查后）"),
		log.indexOf("同名去重（复查后）"); a < 0 || b < a || c < b {
		t.Errorf("复查后的顺序该是 组读数 < 逐条对照 < 去重读数，实得 %d/%d/%d：\n%s",
			a, b, c, log.joined())
	}
}
