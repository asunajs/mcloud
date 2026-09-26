package app

// 活体探针（**只读**）：盯住第廿四趟打过卡的那几条，问"过一段时间它们会不会自己翻成 FINISH"。
//
// # 这一发补的是哪一格（R24 剩下的那一半）
//
// R24 今天钉住了两半：① 打了不等于当场认（纯打卡条目复查后全在「没认」里，四趟复现）；
// ② 真做了动作会当场认（106 传一个文件、522 传满 100 个）。第廿四趟又把
// "**分钟级**滞后"这一档排除了 —— 同一趟里读三遍全表，两号各 `变了 0 条`
// （出处：`docs/capture/app-task-run-account-samples.txt` §4）。
// 剩下那一格是**跨时间**的：那些"做了但上游未认"的条目，**当天晚些 / 隔天**会不会自己翻？
// 答"翻"⇒ 复查那四格是滞后读数，整趟停在 `partial_success` 就只是时机问题；
// 答"不翻"⇒ 那一类任务真要去 App 里做动作，我们打卡打多少发都不会涨。两种处置在界面上的
// 说法完全不同，而**它不是靠再跑一遍带写的整趟能问出来的**（那只会再加一批新的"没认"）。
//
// # 为什么它必须只读，以及这条纪律怎么被钉住
//
// 这一支走的是生产读口 [AppTaskMatrix]（16 枪拉表）+ 换会话那几枪，**一发写的不发**。
// 跑完它自己数一份账：`/ycloud/signin/task/click` 与 `/market/signin/page/doTask`
// 两条路径**必须各 0 发**，否则 `t.Errorf`。⚠ 这条断言不是装饰：它一红就说明有人把
// 这一支改成了带写，那它就不再是"随时能跑"的仪器，得回到副本库 + 写授权那一档。
// 也正因为只读，它**不需要**在动手前额外拉一遍"基线"—— 基线是**上一次跑它**留下的那份 stdout。
//
// # 面值行的格式（它就是基线，不需要另造一份文件）
//
// 每条被盯的条目都打两行：一行给人读，一行给机器读
// （`面值 #<id>|<state>|<currstep>|<process>|<enable>`）。
// 今天这份 stdout 存下来 ⇒ 隔天/后天把 `MCLOUD_LIVE_PROBE_BASELINE_FILE` 指到那份文件，
// 探针就自己念"哪几条翻了、从哪格翻到哪格"。字段就是 [liveProbeWordFields] 比的那四格，
// 与第廿四趟差集用的是**同一份判据**，不要另立一套。
//
// # 凭据边界
//
// 只念上游自己的业务读数（id / 组 / 任务名 / `state` / `currstep` / `process` / `enable`），
// 这些在 `docs/capture/` 的成文口径里是可原样打印的。票、设备号一律不出现；
// 报错文本经 [liveProbeSink.mask]（凭据要先进 `see` 才会被掩，本探针只登记 `authorization` 与手机号）。

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
)

// liveProbeRereadWatch 是"第廿四趟为它动过手、或刻意留着当对照"的那几条。
//
// 三类，缺一不可（少了后两类这一份读数就只支持一个方向）：
//   - **打过卡、复查未认**：这一组才是 R24 那一问的正主。
//   - **一枪没发**（434 被 `currstep` 门挡住、120 在旧实现的 SKIP_TASK_LIST 里）：对照组。
//     它们如果也翻 ⇒ "翻档与打卡无关"，那一问就有了反证。
//   - **真做了动作、当场认**（106 / 522）：反向对照。它们如果**退**回非 FINISH ⇒
//     说明这张表本身在动，那第一组的"没翻"就不能算证据。
type liveProbeRereadWatch struct {
	id   int64
	note string
}

var liveProbeRereadWatches = []liveProbeRereadWatch{
	{548, "第廿四趟：两号各一发打卡，复查都是「做了但上游未认」"},
	{550, "第廿四趟：两号各一发打卡，复查都是「做了但上游未认」"},
	{551, "第廿四趟：两号各一发打卡，复查都是「做了但上游未认」"},
	{1005, "第廿四趟：两号都走了玩法 + 末尾一发打卡，复查「做了但上游未认」"},
	{1020, "第廿四趟：号二打过前置那一发、玩法挂在 ⑧ 的 HTTP 404；号一当时已是 FINISH"},
	{1029, "第廿四趟：号一发过打卡、复查未认；号二那张表里没有这一条"},
	{1057, "第廿四趟：号二打过、复查未认；号一当时已是 FINISH"},
	{1078, "第廿四趟：号一发过打卡、复查未认"},
	{1079, "第廿四趟：号一发过打卡、复查未认"},
	{434, "对照组：一枪都没发（currstep 门挡住前置）⇒ 它要是翻了，翻档就不是打卡的功劳"},
	{120, "对照组：按规则跳过（在旧实现的 SKIP_TASK_LIST 里）⇒ 同上"},
	{106, "反向对照：真做了动作、当场认 ⇒ 这一趟该仍读 FINISH"},
	{522, "反向对照：真做了动作、当场认 ⇒ 这一趟该仍读 FINISH"},
}

// liveProbeBaselineMarker 是给人读那两行里**给机器读**的那一行的前缀。
const liveProbeBaselineMarker = "面值 #"

// TestLiveProbeRereadClickedTasks 只读回读那几条的面值，并在给了基线时念出"翻了哪几条"。
func TestLiveProbeRereadClickedTasks(t *testing.T) {
	run, account, sink, recorder := liveProbeWorldRecording(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	words := liveProbeTableWords(ctx, opening, sink, "只读回读")
	if words == nil {
		t.Fatalf("表没读出来 ⇒ 这一遍一个读数都没有（不是「那些条目都翻档了」）")
	}
	baseline := liveProbeLoadRereadBaseline(sink)

	ids := make([]int64, 0, len(liveProbeRereadWatches))
	byID := make(map[int64]liveProbeRereadWatch, len(liveProbeRereadWatches))
	for _, watch := range liveProbeRereadWatches {
		ids = append(ids, watch.id)
		byID[watch.id] = watch
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })

	var inTable, finished, flipped int
	for _, id := range ids {
		watch := byID[id]
		word, ok := words[id]
		if !ok {
			sink.say("回读 #%d ⇒ **不在今天这张表里**（%s）", id, watch.note)
			continue
		}
		inTable++
		if word.state == "FINISH" {
			finished++
		}
		sink.say("回读 #%d [%s] %s ⇒ state=%s currstep=%s process=%s enable=%s ｜ %s",
			id, word.group, word.name, word.state, word.currstep, word.process, word.enable,
			watch.note)
		sink.say("%s%d|%s|%s|%s|%s", liveProbeBaselineMarker, id,
			word.state, word.currstep, word.process, word.enable)

		older, had := baseline[id]
		if !had {
			continue
		}
		diffs := liveProbeWordFields(older, word)
		if len(diffs) == 0 {
			continue
		}
		flipped++
		sink.say("翻档对照 #%d [%s] %s ⇒ %s（基线 → 现在）",
			id, word.group, word.name, strings.Join(diffs, "、"))
	}

	sink.say("回读读数：%d 条在表（其中 FINISH %d 条）｜与基线比：翻了 %d 条｜基线：%d 条读数",
		inTable, finished, flipped, len(baseline))
	if len(baseline) == 0 {
		sink.say("⚠ 这一遍**没有基线** ⇒ 它只回答「现在是什么状态」，不回答「翻没翻」。" +
			"把这份 stdout 存下来，下一次跑时指 MCLOUD_LIVE_PROBE_BASELINE_FILE 就答得出那一问。")
	}

	clicks := recorder.countFor(liveProbeClickPath)
	entries := recorder.countFor(liveProbeMrpDoTaskPath)
	recorder.sayTally()
	if clicks != 0 || entries != 0 {
		t.Errorf("这一支必须保持**只读**（它要能在任意时间跑，不该靠写授权）："+
			"task/click=%d 发、doTask=%d 发", clicks, entries)
	}
	sink.say("读数怎么用：① 上面那些 `回读 #…` 就是今天这张表的原样面值；② `翻档对照` 那几行" +
		"是 R24 剩下那一格的直接答案（翻了几条、翻的是打过卡的那一组还是对照组）；" +
		"③ 传输层账本里 click 与 doTask 必须各 0 发，那一格由上面那条 t.Errorf 钉住。")
}

// liveProbeLoadRereadBaseline 读上一次跑这一支留下的 stdout，把 `面值 #…` 那些行摊成快照。
//
// 交回 nil 的三种情形都**念得出来**（"没给 env" / "路径不是绝对" / "读不到"），
// 因为"没有对照"与"对照里没人翻"是两件事 —— 静默给空表会让调用方把前者读成后者。
// ⚠ 绝对路径这一条不是洁癖：`go test` 的工作目录是**包目录**，相对路径会指到
// `internal/app/` 底下去（坑见 `docs/capture/app-mail139-samples.txt` 抬头那条）。
func liveProbeLoadRereadBaseline(sink *liveProbeSink) map[int64]liveProbeTaskWord {
	path := strings.TrimSpace(os.Getenv("MCLOUD_LIVE_PROBE_BASELINE_FILE"))
	if path == "" {
		sink.say("没给 MCLOUD_LIVE_PROBE_BASELINE_FILE ⇒ 这一遍只念当前面值，它本身就是下一次的基线")
		return nil
	}
	if !filepath.IsAbs(path) {
		sink.say("⚠ 基线路径不是绝对路径（%s）⇒ 对照**不成立**（这一遍不等于「没人翻」）", path)
		return nil
	}
	text, rerr := os.ReadFile(path)
	if rerr != nil {
		sink.say("⚠ 基线读不到：%s ⇒ 对照**不成立**（这一遍不等于「没人翻」）",
			sink.mask(rerr.Error()))
		return nil
	}
	out := map[int64]liveProbeTaskWord{}
	for _, line := range strings.Split(string(text), "\n") {
		at := strings.Index(line, liveProbeBaselineMarker)
		if at < 0 {
			continue
		}
		fields := strings.Fields(line[at:])
		if len(fields) < 2 {
			continue
		}
		// t.Log 会在行首加"文件名:行号: "，而标记本身是"面值 #"+id ⇒ 从标记处切之后，
		// 第二格才是那份 `#<id>|<state>|…`。
		token := fields[1]
		parts := strings.Split(token, "|")
		if len(parts) != 5 {
			sink.say("⚠ 基线里有一格不是 5 段（%s）⇒ 那一行跳过，别把它当成「没有读数」",
				liveProbeClip(token, 60))
			continue
		}
		id, aerr := strconv.ParseInt(strings.TrimPrefix(parts[0], "#"), 10, 64)
		if aerr != nil {
			continue
		}
		out[id] = liveProbeTaskWord{state: parts[1], currstep: parts[2],
			process: parts[3], enable: parts[4]}
	}
	sink.say("基线已载入：%s（%d 条面值读数）", liveProbeClip(path, 80), len(out))
	return out
}

// TestLiveProbeRereadBaselineParsesItsOwnStdout 是这一支**唯一不需要真上游**的那条断言。
//
// 它要钉的是"基线格式自洽"：探针打出去的那一行，隔天必须能被同一份解析读回来 ——
// 读不回来时这一支会静默退化成"只念当前面值"，而那句话很容易被读成"没人翻档"。
// 三种交回 nil 的路子（没给 env / 相对路径 / 文件读不到）也各钉一次：
// 它们**都**不等于"对照为空所以没翻"。
//
// 反向改坏它：把解析里的 `fields[1]` 换成 `fields[0]` ⇒ 第一条断言红（`面值` 不是数字 id，
// 一条都进不来）；把 `len(parts) != 5` 放宽成 `!= 4` ⇒ 好样本全被跳过、同一条红。
func TestLiveProbeRereadBaselineParsesItsOwnStdout(t *testing.T) {
	sink := newLiveProbeSink(t)
	const stdout = "    live_probe_reread_ack_test.go:92: 回读 #548 [time] 开启备份领好礼 ⇒ state=WAIT\n" +
		"    live_probe_reread_ack_test.go:92: 面值 #548|WAIT|0|0|1\n" +
		"    live_probe_reread_ack_test.go:92: 面值 #1005|FINISH|3|0|1\n" +
		"    live_probe_reread_ack_test.go:92: 面值 #9999|WAIT\n" +
		"    live_probe_reread_ack_test.go:92: 传输层账本：一共 18 枪 / 3 类\n"
	file := filepath.Join(t.TempDir(), "yesterday.txt")
	if werr := os.WriteFile(file, []byte(stdout), 0o600); werr != nil {
		t.Fatalf("写基线文件失败：%v", werr)
	}

	t.Setenv("MCLOUD_LIVE_PROBE_BASELINE_FILE", file)
	baseline := liveProbeLoadRereadBaseline(sink)
	if len(baseline) != 2 {
		t.Fatalf("两份好读数都要进来、那份 2 段的要跳过，实得 %+v", baseline)
	}
	if got := baseline[548]; got.state != "WAIT" || got.enable != "1" {
		t.Errorf("548 那一份要逐格读回来，实得 %+v", got)
	}
	if got := baseline[1005]; got.state != "FINISH" || got.currstep != "3" {
		t.Errorf("1005 那一份要逐格读回来，实得 %+v", got)
	}
	if _, dirty := baseline[9999]; dirty {
		t.Error("段数不对的那一行不许进基线（进了就会把「解析失败」念成「这一条没翻」）")
	}

	// 与探针**打出去的那一行**自洽：拿它自己的格式生成一行，再读回来。
	produced := fmt.Sprintf("%s%d|%s|%s|%s|%s",
		liveProbeBaselineMarker, 548, "WAIT", "0", "0", "1")
	if at := strings.Index(produced, liveProbeBaselineMarker); at < 0 {
		t.Fatal("标记要在自己打出去的行之里")
	} else if fields := strings.Fields(produced[at:]); len(fields) < 2 ||
		strings.Split(fields[1], "|")[0] != "#548" {
		t.Errorf("探针打的行要能被同一份解析读回来，实得 %q", produced)
	}

	// 三种"没有对照"都要**明确**交回 nil（而不是交回一张空表让调用方读成「没人翻」）。
	t.Setenv("MCLOUD_LIVE_PROBE_BASELINE_FILE", "")
	if got := liveProbeLoadRereadBaseline(newLiveProbeSink(t)); got != nil {
		t.Errorf("没给 env 要交回 nil，实得 %+v", got)
	}
	t.Setenv("MCLOUD_LIVE_PROBE_BASELINE_FILE", "yesterday.txt")
	if got := liveProbeLoadRereadBaseline(newLiveProbeSink(t)); got != nil {
		t.Errorf("相对路径要交回 nil（go test 的目录是包目录），实得 %+v", got)
	}
	t.Setenv("MCLOUD_LIVE_PROBE_BASELINE_FILE", filepath.Join(t.TempDir(), "没有这个文件.txt"))
	if got := liveProbeLoadRereadBaseline(newLiveProbeSink(t)); got != nil {
		t.Errorf("文件读不到要交回 nil，实得 %+v", got)
	}
}
