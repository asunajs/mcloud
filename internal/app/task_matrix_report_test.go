package app

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// app-task 那一层**播报**的测试。
//
// 分两层各测各的：
//
//   - 纯函数那三条（组名表、组序与组内四档顺序、SKIP 表与隐藏组）只看
//     [TaskVerifyLines] 交回的**行序列**，不需要装会话、也不数枪。
//   - 端到端那两条用一份真脚本，钉"实时行出现在分节之前"与"最后一行是小结原文"
//     —— 那是这一层唯一无法从纯函数读出来的两件事（时机）。
//
// ⚠ 期望文本逐字对旧实现的实拍日志（账号 1 那一轮，`------【每日任务】------` /
// `成功 手动上传一个文件 106` / `失败 上传中秋团圆照 614` /
// `需前往云盘完成 出行攻略问AI 616` / `已完成 去体验AI相机`），
// 只有"刻意不同"那四处不同（双空格、档位、SKIP 只挡行、空组名）。

// mxLog 收事件流里的人话，供"说了什么、什么级别、按什么顺序"三类断言。
//
// 不复用 `liveTaskProgress`：那一份只记 level+message 两格、且属 live-room 那一族的夹具。
type mxLog struct {
	levels []core.LogLevel
	texts  []string
}

func (l *mxLog) handle() RunProgress {
	return func(level core.LogLevel, message string) {
		l.levels = append(l.levels, level)
		l.texts = append(l.texts, message)
	}
}

func (l *mxLog) has(want string) bool {
	for _, text := range l.texts {
		if strings.Contains(text, want) {
			return true
		}
	}
	return false
}

// hasAt 钉"这一行的级别"（文案对上还不够：分节走 info、需前往走 warn、失败走 error，
// 那三档是这一层对旧实现五档的唯一投影，投错了这一条要红）。
func (l *mxLog) hasAt(level core.LogLevel, want string) bool {
	for i, text := range l.texts {
		if l.levels[i] == level && strings.Contains(text, want) {
			return true
		}
	}
	return false
}

// indexOf 找第一枚含 `want` 的行号（没有 ⇒ -1，交回给"顺序"类断言用）。
func (l *mxLog) indexOf(want string) int {
	for i, text := range l.texts {
		if strings.Contains(text, want) {
			return i
		}
	}
	return -1
}

func (l *mxLog) joined() string { return strings.Join(l.texts, "\n") }

// TestTaskGroupNameCN 钉那张组名表覆盖**全部**旧实现认识的组。
//
// 反向改坏的样子：漏一个键 ⇒ 那一组的分节标题变成英文原值（本条红）；
// 把兜底改成"丢掉未知组" ⇒ 那一组在报告里凭空消失（本条红，因为它要求回落成原值）。
func TestTaskGroupNameCN(t *testing.T) {
	cases := []struct{ group, want string }{
		{"day", "每日任务"},
		{"month", "每月任务"},
		{"new", "新用户任务"},
		{"time", "热门任务"},
		{"hidden", "隐藏任务"},
		{"hiddenabc", "隐藏任务"},
		{"beiyong1", "临时任务"},
		{"cloudEmail", "云邮箱任务"},
		// 未知组：宁可显示英文，也不静默丢。
		{"whatever", "whatever"},
		// 刻意不同④：`groupid` 缺席时旧实现会印出 `------【】------`。
		{"", "未分组任务"},
	}
	for _, c := range cases {
		if got := taskGroupNameCN(c.group); got != c.want {
			t.Errorf("组名 %q 应译成 %q，实得 %q", c.group, c.want, got)
		}
	}
}

// TestTaskVerifyLinesOrder ⭐ 组序 + 组内四档顺序，逐字对旧实现那一轮实拍。
//
// 夹具是账号 1 那一轮的形状：`day` 有一条**本次做完**的、`beiyong1` 与 `time` 与
// `month` 有失败的、`time` 里既有已完成也有"需前往"。
//
// 反向改坏的三种样子各自会红在哪：
//   - 把组序改成固定表（day/month/…）⇒ 第一行就不是"每日任务"（本条红）。
//     旧实现那条"按四档拼长队、组名首次出现"的规则是**有信息量**的：
//     它让"这趟真做了事"的组排前面。
//   - 把组内顺序改成按 id ⇒ "已完成"会跑到"成功"前面。
//   - 忘记 `已完成` 不带 id ⇒ 那一行多出一个数字。
func TestTaskVerifyLinesOrder(t *testing.T) {
	verified := []TaskVerifyRecord{
		{TaskID: 578, Name: "去体验AI相机", Group: "time", Class: VerifyAlreadyWas},
		{TaskID: 616, Name: "出行攻略问AI", Group: "time", Class: VerifyStillOpen, Label: "云盘"},
		{TaskID: 1057, Name: "体验AI工作台", Group: "cloudEmail", Class: VerifyStillOpen, Label: "邮箱"},
		{TaskID: 547, Name: "下月签到领惊喜福利", Group: "month", Class: VerifyAlreadyWas},
		{TaskID: 614, Name: "上传中秋团圆照", Group: "beiyong1", Class: VerifyFailed},
		{TaskID: 106, Name: "手动上传一个文件", Group: "day", Class: VerifyDoneNow},
		{TaskID: 522, Name: "当月上传文件满100个", Group: "time", Class: VerifyFailed},
		{TaskID: 1005, Name: "从固定入口进入云朵中心", Group: "time", Class: VerifyFailed},
	}
	want := []string{
		"------【每日任务】------",
		"成功 手动上传一个文件 106",
		"------【临时任务】------",
		"失败 上传中秋团圆照 614",
		"------【热门任务】------",
		"已完成 去体验AI相机",
		"失败 当月上传文件满100个 522",
		"失败 从固定入口进入云朵中心 1005",
		"需前往云盘完成 出行攻略问AI 616",
		"------【云邮箱任务】------",
		"需前往邮箱完成 体验AI工作台 1057",
		"------【每月任务】------",
		"已完成 下月签到领惊喜福利",
	}
	lines := TaskVerifyLines(verified)
	if len(lines) != len(want) {
		t.Fatalf("行数应 %d，实得 %d：\n%s", len(want), len(lines), joinReportLines(lines))
	}
	for i, line := range lines {
		if line.Text != want[i] {
			t.Errorf("第 %d 行应是 %q，实得 %q", i+1, want[i], line.Text)
		}
	}
	// 级别那一格单独钉（文本对了级别投错也算坏）。
	lines = TaskVerifyLines(verified)
	for _, line := range lines {
		switch {
		case strings.HasPrefix(line.Text, "失败 "):
			if line.Level != core.LevelError {
				t.Errorf("%q 应是 error（做了却没认），实得 %v", line.Text, line.Level)
			}
		case strings.HasPrefix(line.Text, "需前往"):
			if line.Level != core.LevelWarn {
				t.Errorf("%q 应是 warn（要人去做点什么），实得 %v", line.Text, line.Level)
			}
		default:
			if line.Level != core.LevelInfo {
				t.Errorf("%q 应是 info，实得 %v", line.Text, line.Level)
			}
		}
	}
}

// TestTaskVerifyLinesDropsHiddenAndSkipList 钉"不进报告"的那两类。
//
// ⚠ 两类**为什么**不进报告是两件事，所以分开钉：
//   - 隐藏组（`hidden` / `hiddenabc`）由 [ClassifyVerifiedTask] 判成
//     [VerifyHidden]，在这一层就没了 ⇒ 计数仍进 [TaskRunCounts.Hidden]。
//   - SKIP 表那 11 个 id 是**这一层**丢的（旧实现 `results.ts:33`），
//     而丢的只有行：计数照记（刻意不同③）。
func TestTaskVerifyLinesDropsHiddenAndSkipList(t *testing.T) {
	verified := []TaskVerifyRecord{
		{TaskID: 900, Name: "隐藏的那条", Group: "hidden", Class: VerifyHidden},
		{TaskID: 901, Name: "隐藏abc的那条", Group: "hiddenabc", Class: VerifyHidden},
		// 117 是 SKIP 表里的第一个（逐字取自旧实现 `tasks.ts:111`）。
		{TaskID: TaskMatrixNeverClick[0], Name: "报告里不该出现的", Group: "day",
			Class: VerifyStillOpen, Label: "云盘"},
		{TaskID: 777, Name: "正常那条", Group: "day", Class: VerifyDoneNow},
	}
	lines := TaskVerifyLines(verified)
	got := joinReportLines(lines)
	if len(lines) != 2 {
		t.Fatalf("只剩分节 + 一行，实得 %d 行：\n%s", len(lines), got)
	}
	if strings.Contains(got, "隐藏") || strings.Contains(got, "报告里不该出现的") {
		t.Errorf("隐藏组与 SKIP 表都不该出播报行，实得：\n%s", got)
	}
	if !strings.Contains(got, "成功 正常那条 777") {
		t.Errorf("正常那条要在，实得：\n%s", got)
	}
	// 反向验证的另一半：SKIP 只挡**行**，不挡复查里的那一格。
	records := []TaskVerifyRecord{{TaskID: TaskMatrixNeverClick[0], Group: "day",
		Class: VerifyStillOpen}}
	var counts TaskRunCounts
	for _, record := range records {
		counts.absorbVerify(record.Class)
	}
	if counts.StillOpen != 1 {
		t.Errorf("SKIP 表的 id 仍要进计数（否则界面上的数与行内条数对不上），实得 %+v", counts)
	}
}

// TestTaskVerifyLineAppendsNote 钉"补打卡那一则挂在它所属那一行"。
//
// 这一格的意义是**位置**：481 那条今天被动过两次（前置 + 复查补的一发），
// 只写在 Notes 里再拼进小结尾巴，读事件流的人看不出"哪一条被动过两次"。
func TestTaskVerifyLineAppendsNote(t *testing.T) {
	verified := []TaskVerifyRecord{{
		TaskID: 481, Name: "逛云朵中心", Group: "day", Class: VerifyDoneNow,
		Note: "补打卡已发出（task2，这一枚 key 零实拍）",
	}}
	lines := TaskVerifyLines(verified)
	if len(lines) != 2 {
		t.Fatalf("应两行（分节 + 那一行），实得 %d", len(lines))
	}
	want := "成功 逛云朵中心 481：补打卡已发出（task2，这一枚 key 零实拍）"
	if got := lines[1].Text; got != want {
		t.Errorf("实得 %q， want %q", got, want)
	}
}

// TestTaskStepLiveLine 钉第一趟那几行"还没动手之前"的播报。
//
// 三条判据各有来路：
//   - 会出网的两种（打卡 / 玩法）都出一行，玩法那行带 runner 名 —— 四条玩法共用
//     两档 key，只有 runner 名能答"这一条是谁做的"。
//   - `StepBlockedByRule` **只有带上传时**才出一行，且句子写明"不发打卡"：
//     那道门只挡打卡、上传照做（[apply] 的 `case StepBlockedByRule`），
//     写成"尝试完成"会让人以为我们上报了进度 —— 那是谎报。
//   - 两类"本工具不做"（缺口 / 需人工）各出一行 warn，并把 Reason 带出来。
//     形状照旧实现那句 `跳过无法自动化的任务: 生成我的校园海报(33)`。
//   - 被规则挡又没有上传的 ⇒ **不出行**（一枪都没发，条数由 RuleSkipped 那格说）。
func TestTaskStepLiveLine(t *testing.T) {
	item := func(id int64, group, name string) upstream.TaskV2Item {
		return dispatchItem(id, group, name, "WAIT", 0)
	}
	cases := []struct {
		name     string
		item     upstream.TaskV2Item
		plan     TaskStepPlan
		wantText string
		wantLev  core.LogLevel
		wantOK   bool
	}{
		{
			name: "盲点打卡", item: item(777, "day", "看个广告"),
			plan:     TaskStepPlan{Kind: StepClick, ClickKey: upstream.TaskClickKeyDefault},
			wantText: "尝试完成 看个广告 777", wantLev: core.LevelInfo, wantOK: true,
		},
		{
			name: "专属玩法", item: item(409, "time", "逛云朵中心"),
			plan: TaskStepPlan{Kind: StepRunner, Runner: "randomCloudTask",
				ClickKey: upstream.TaskClickKeyDefault},
			wantText: "尝试完成 逛云朵中心 409（玩法 randomCloudTask）",
			wantLev:  core.LevelInfo, wantOK: true,
		},
		{
			name: "门挡住打卡但仍上传", item: item(481, "beiyong1", "备份照片"),
			plan: TaskStepPlan{Kind: StepBlockedByRule, Reason: "currstep=1 不为 0 ⇒ 不发这一发",
				Upload: &taskUploadShape{Rounds: 1}},
			wantText: "只传文件、不发打卡 备份照片 481：currstep=1 不为 0 ⇒ 不发这一发",
			wantLev:  core.LevelInfo, wantOK: true,
		},
		{
			name: "门挡住打卡、也没有上传", item: item(481, "beiyong1", "备份照片"),
			plan:   TaskStepPlan{Kind: StepBlockedByRule, Reason: "currstep=1 不为 0 ⇒ 不发这一发"},
			wantOK: false,
		},
		{
			name: "本工具没接玩法", item: item(107, "day", "创建云笔记"),
			plan: TaskStepPlan{Kind: StepBlockedToolGap,
				ToolGap: "笔记那一族还没接"},
			wantText: "跳过 创建云笔记 107：笔记那一族还没接", wantLev: core.LevelWarn, wantOK: true,
		},
		{
			name: "要人去 App 开", item: item(548, "time", "开启备份领好礼"),
			plan:     TaskStepPlan{Kind: StepNeedsHuman, Reason: "本月自动备份没开，本工具不代开"},
			wantText: "跳过 开启备份领好礼 548：本月自动备份没开，本工具不代开",
			wantLev:  core.LevelWarn, wantOK: true,
		},
	}
	for _, c := range cases {
		line, ok := taskStepLiveLine(c.item, c.plan)
		if ok != c.wantOK {
			t.Errorf("%s：该行该不该出 = %v，实得 %v", c.name, c.wantOK, ok)
			continue
		}
		if !c.wantOK {
			continue
		}
		if line.Text != c.wantText {
			t.Errorf("%s：实得 %q，want %q", c.name, line.Text, c.wantText)
		}
		if line.Level != c.wantLev {
			t.Errorf("%s：级别实得 %v，want %v", c.name, line.Level, c.wantLev)
		}
	}
}

// TestTaskMatrixRunBroadcastsLiveThenReport ⭐ 端到端钉**时机**。
//
// 纯函数那几条测的是"说什么"，这一条测的是"什么时候说"：
//   - 入口两行在最前（分节标题 + 条数），条数那行带"去重后"；
//   - 实时那几行出现在**分节之前**（事情正在发生时就看得见，不是等末尾小结）；
//   - 最后一行**必须**是小结原文 —— 界面的"最近运行"只显最后一行，
//     小结被逐条行挤掉的表现是：界面上看不出这趟一共做了几条。
func TestTaskMatrixRunBroadcastsLiveThenReport(t *testing.T) {
	first := mxItems(
		mxItem(596, "beiyong1", "备份一个文件", "WAIT", "", 0),
		mxItem(777, "day", "看个广告", "WAIT", "", 0),
		mxItem(107, "day", "创建云笔记", "WAIT", "", 0),
	)
	second := mxItems(
		mxItem(596, "beiyong1", "备份一个文件", "FINISH", "", 1),
		mxItem(777, "day", "看个广告", "FINISH", "", 1),
		mxItem(107, "day", "创建云笔记", "WAIT", "", 0),
	)
	log := &mxLog{}
	run := mxDefaultRun(t, mxScript([]string{"JWT-1"}, first, second, mxHappy()...))
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}

	if len(log.texts) < 6 {
		t.Fatalf("一趟至少要播：入口两行 + 三条实时 + 分节若干 + 小结，实得全部：\n%s", log.joined())
	}
	if got := log.texts[0]; got != "------【任务列表】------" {
		t.Errorf("第一行应是任务列表的分节，实得 %q", got)
	}
	if got := log.texts[1]; !strings.Contains(got, "获取到") ||
		!strings.Contains(got, "去重后 3 个") {
		t.Errorf("第二行要说条数与去重后的条数，实得 %q", got)
	}
	// 实时那三行：两条会出网（596 打卡 + 777 打卡），一条是缺口（107）。
	for _, want := range []string{"尝试完成 备份一个文件 596", "尝试完成 看个广告 777"} {
		if !log.has(want) {
			t.Errorf("缺实时行 %q：\n%s", want, log.joined())
		}
	}
	if !log.hasAt(core.LevelWarn, "跳过 创建云笔记 107") {
		t.Errorf("缺口那条要 warn 一行并带上原因，实得：\n%s", log.joined())
	}
	// 实时行在分节**之前**（时机就是这一条的存在理由）。
	if live, section := log.indexOf("尝试完成 备份一个文件"),
		log.indexOf("------【临时任务】------"); live < 0 || section < 0 || live > section {
		t.Errorf("实时行要排在组分节之前，实得 live=%d section=%d：\n%s",
			live, section, log.joined())
	}
	// 复查之后的分节与四档行：两条做完的都在"本次做完"那一档。
	if !log.has("------【每日任务】------") {
		t.Errorf("缺每日任务分节：\n%s", log.joined())
	}
	if !log.hasAt(core.LevelInfo, "成功 备份一个文件 596") {
		t.Errorf("596 该念成'成功'，实得：\n%s", log.joined())
	}
	if !log.hasAt(core.LevelWarn, "需前往云盘完成 创建云笔记 107") {
		t.Errorf("107 没做也没完成 ⇒ 该念成'需前往…'，实得：\n%s", log.joined())
	}
	if last := log.texts[len(log.texts)-1]; last != res.SummaryLine() {
		t.Errorf("最后一行要是小结原文，实得 %q\n（小结：%q）", last, res.SummaryLine())
	}
	// 小结**只播一次**：它是 `runs.message` 那份文本，逐条行之上再叠一份同样的话
	// 会让人以为"跑了两趟"。这一格也是"逐条行排在小结之前"这条顺序的另一半 ——
	// 只断言"最后一行是小结"的话，把小结提前播一次再播一遍是测不出来的。
	var summaries int
	for _, text := range log.texts {
		if text == res.SummaryLine() {
			summaries++
		}
	}
	if summaries != 1 {
		t.Errorf("小结该恰好一行，实得 %d：\n%s", summaries, log.joined())
	}
}

// TestTaskMatrixRunBroadcastsUploadRounds 钉上传那几行的**时机与措辞**。
//
// 旧实现是 `logger.debug("上传文件成功")` 一条不带任务名；这里带名字与 id，
// 因为清理那一族（#37）吃的就是这些 id，读日志的人要能把"哪个文件"对上"哪一行"。
func TestTaskMatrixRunBroadcastsUploadRounds(t *testing.T) {
	first := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(522, "time", "当月上传文件满100个", "WAIT", "", 0),
	)
	second := mxItems(
		mxItem(106, "day", "上传一个文件", "FINISH", "", 1),
		mxItem(522, "time", "当月上传文件满100个", "FINISH", "", 1),
	)
	log := &mxLog{}
	run := mxDefaultRun(t, mxScript([]string{"JWT-1"}, first, second,
		append(mxUploadRoutes(6), mxHappy()...)...))
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if res.Counts.Uploaded != 6 {
		t.Fatalf("该传 6 个（106 一个 + 522 五个），实得 %+v", res.Counts)
	}
	// 每一**个**一行：6 行成功。
	var uploaded int
	for _, text := range log.texts {
		if strings.HasPrefix(text, "上传文件成功") {
			uploaded++
		}
	}
	if uploaded != 6 {
		t.Errorf("六个文件该六行，实得 %d：\n%s", uploaded, log.joined())
	}
	if !log.has("上传文件成功 上传一个文件 106") {
		t.Errorf("上传那行要带任务名与 id（清理那一族的输入就是这些 id），实得：\n%s", log.joined())
	}
	// 时机：上传发生在第一趟 ⇒ 它必须在组分节之前。
	if up, section := log.indexOf("上传文件成功"), log.indexOf("------【每日任务】------"); up < 0 ||
		section < 0 || up > section {
		t.Errorf("上传那行要排在复查的分节之前，实得 up=%d section=%d：\n%s",
			up, section, log.joined())
	}
}

// TestTaskMatrixRunBroadcastsReadFailuresEarly 钉"没读到几组"那一行出现在**开头**。
//
// 小结里也有同一格，但那是三十行之后才出现；读实时日志的人在第一行就要知道
// "24 个"不是全表 —— 否则"待做 N 条"会凭空变小而没人起疑。
func TestTaskMatrixRunBroadcastsReadFailuresEarly(t *testing.T) {
	overrides := map[int]string{0: mxBody(mxItem(777, "day", "看个广告", "WAIT", "", 0))}
	overrides[3] = `{"code":1,"msg":"这一组挂了"}`
	log := &mxLog{}
	run := mxDefaultRun(t, mxScript([]string{"JWT-1"},
		mxRound(overrides), mxItems(), mxHappy()...))
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if res.ReadFailures != 1 {
		t.Fatalf("夹具该造出 1 组没读到，实得 %d", res.ReadFailures)
	}
	if !log.hasAt(core.LevelWarn, "1/16 组没读到") {
		t.Errorf("开局就要 warn 一句'这不是全表'，实得：\n%s", log.joined())
	}
	// 位置：那一句要在**条数那行之后**、任何分节之前（它修饰的就是那个条数）。
	counts := log.indexOf("去重后")
	warn := log.indexOf("组没读到")
	if counts < 0 || warn < 0 || warn < counts || log.indexOf("------【") > warn {
		t.Errorf("warn 要紧跟条数行、排在分节之前，实得 counts=%d warn=%d section=%d：\n%s",
			counts, warn, log.indexOf("------【"), log.joined())
	}
	// 反方向：全读到时**不**播这一句（否则每天的日志里多一条假警报）。
	logAll := &mxLog{}
	runAll := mxDefaultRun(t, mxScript([]string{"JWT-1"},
		mxItems(mxItem(777, "day", "看个广告", "WAIT", "", 0)),
		mxItems(mxItem(777, "day", "看个广告", "FINISH", "", 1)), mxHappy()...))
	runAll.Progress = logAll.handle()
	if _, perr := mxResult(t, runAll); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if logAll.has("组没读到") {
		t.Errorf("全读到不该播'没读到'，实得：\n%s", logAll.joined())
	}
	if !logAll.has("成功 看个广告 777") {
		t.Errorf("该有那一行成功，实得：\n%s", logAll.joined())
	}
}

// joinReportLines 把播报行拼成一整段（只给失败信息用）。
func joinReportLines(lines []TaskReportLine) string {
	texts := make([]string, 0, len(lines))
	for _, line := range lines {
		texts = append(texts, line.Level.String()+" "+line.Text)
	}
	return strings.Join(texts, "\n")
}
