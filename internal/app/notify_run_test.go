package app

import (
	"context"
	"errors"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 桩
// ─────────────────────────────────────────────────────────────────────────────

// notifyClockAt 是正文时刻断言用的固定时刻（UTC）。
//
// 刻意选一个"分秒都不是整点"的：`09:00:00` 抓不到"把分钟写成了秒"这类错。
func notifyClockAt() time.Time {
	return time.Date(2026, 9, 23, 9, 5, 43, 0, time.UTC)
}

func notifyLine(level core.LogLevel, message string) NotifyLogLine {
	return NotifyLogLine{Level: level, At: notifyClockAt(), Message: message}
}

// fakeNotifyConfig 是一个可编程的"读推送配置"。
type fakeNotifyConfig struct {
	config core.NotifyConfig
	err    error
	calls  int
}

func (f *fakeNotifyConfig) NotifyConfig(context.Context) (core.NotifyConfig, error) {
	f.calls++
	if f.err != nil {
		return core.NotifyConfig{}, f.err
	}
	return f.config, nil
}

// fakeNotifySend 记下"实际发出去的标题与正文"，并回一份预先备好的逐家结论。
type fakeNotifySend struct {
	calls   int
	title   string
	text    string
	channls core.NotifyChannels
	report  upstream.NotifyReport
}

func (f *fakeNotifySend) SendAll(
	_ context.Context, channels core.NotifyChannels, title, text string,
) upstream.NotifyReport {
	f.calls++
	f.title, f.text, f.channls = title, text, channels
	if f.report != nil {
		return f.report
	}
	return upstream.NotifyReport{{Channel: core.NotifyPushplus, Outcome: upstream.NotifySent}}
}

// enabledWith 造一份"总开关开着、配了 pushplus 那一家"的配置。
func enabledWith(mutate func(*core.NotifyConfig)) *fakeNotifyConfig {
	config := core.NotifyConfig{
		Enabled:  true,
		MinLevel: core.NotifyLevelInfo,
		Channels: core.NotifyChannels{
			Pushplus: &core.PushplusConfig{Token: core.NewSecretValue("pp-token")},
		},
	}
	if mutate != nil {
		mutate(&config)
	}
	return &fakeNotifyConfig{config: config}
}

func notifyRunner(configs *fakeNotifyConfig, sender *fakeNotifySend) *RunNotify {
	return NewRunNotify(configs, sender, fixedClock{at: notifyClockAt()}).In(time.UTC)
}

func oneInfoLine() []NotifyLogLine {
	return []NotifyLogLine{notifyLine(core.LevelInfo, "开门中…")}
}

// ─────────────────────────────────────────────────────────────────────────────
// 正文组装
// ─────────────────────────────────────────────────────────────────────────────

func TestBodyKeepsTheOldLineShape(t *testing.T) {
	body := FormatNotifyBody([]NotifyLogLine{
		notifyLine(core.LevelInfo, "开门中…"),
		notifyLine(core.LevelError, "领云朵失败"),
	}, core.NotifyLevelInfo, time.UTC)

	// 旧 `formatLogsForPush` 的形状：`[${type} ${时间}]${msg}`，行间单个 `\n`、末尾无换行。
	want := "[info 09:05:43]开门中…\n[error 09:05:43]领云朵失败"
	if body != want {
		t.Errorf("正文形状不符\n实得: %q\n期望: %q", body, want)
	}
}

func TestErrorOnlyThresholdKeepsJustTheErrors(t *testing.T) {
	lines := []NotifyLogLine{
		notifyLine(core.LevelInfo, "余额 12 豆"),
		notifyLine(core.LevelWarn, "该主播有 2 间在播"),
		notifyLine(core.LevelError, "口令领取失败"),
	}
	body := FormatNotifyBody(lines, core.NotifyLevelError, time.UTC)
	if body != "[error 09:05:43]口令领取失败" {
		t.Errorf("error 门槛只该收 error，实得 %q", body)
	}
	// warn 门槛：error + warn，不含 info。
	if got := FormatNotifyBody(lines, core.NotifyLevelWarn, time.UTC); strings.Contains(got, "余额") {
		t.Errorf("warn 门槛不该收 info，实得 %q", got)
	}
}

func TestWarnAndErrorRanksAreNotOffByOne(t *testing.T) {
	// 这一条守的是 `notifyLevelNumber` 那张映射表：本仓枚举"越严重数字越大"，
	// 旧的门槛表"越严重数字越小"，两边方向相反 ⇒ 映射写反了不会报错，
	// 只会让"只推错误"把每一句正常进展都推出去（或反过来一条都不推）。
	warnOnly := FormatNotifyBody(
		[]NotifyLogLine{notifyLine(core.LevelWarn, "只是提醒")}, core.NotifyLevelError, time.UTC)
	if warnOnly != "" {
		t.Errorf("warn 不该过 error 门槛，实得 %q", warnOnly)
	}
	errorAtWarn := FormatNotifyBody(
		[]NotifyLogLine{notifyLine(core.LevelError, "确实错了")}, core.NotifyLevelWarn, time.UTC)
	if !strings.Contains(errorAtWarn, "确实错了") {
		t.Errorf("error 该过 warn 门槛，实得 %q", errorAtWarn)
	}
}

func TestUnknownThresholdFallsBackToInfo(t *testing.T) {
	lines := []NotifyLogLine{notifyLine(core.LevelInfo, "正常进展")}
	// 库里的门槛文本被手改坏（或干脆没填）时不罢工，跟随 info —— 旧 `?? 3` 同口径。
	for _, minLevel := range []core.NotifyLogLevel{"", "VERBOSE", core.NotifyLevelInfo} {
		if got := FormatNotifyBody(lines, minLevel, time.UTC); got == "" {
			t.Errorf("门槛 %q 不该把 info 挡掉，实得 %q", minLevel, got)
		}
	}
}

func TestDebugThresholdKeepsEverything(t *testing.T) {
	body := FormatNotifyBody([]NotifyLogLine{
		notifyLine(core.LevelInfo, "a"), notifyLine(core.LevelWarn, "b"),
		notifyLine(core.LevelError, "c"),
	}, core.NotifyLevelDebug, time.UTC)
	if strings.Count(body, "\n") != 2 {
		t.Errorf("debug 门槛该收全部三行，实得 %q", body)
	}
}

func TestBodyTimestampsFollowTheGivenLocation(t *testing.T) {
	body := FormatNotifyBody(oneInfoLine(), core.NotifyLevelInfo, time.FixedZone("上海", 8*3600))
	if !strings.Contains(body, "[info 17:05:43]") {
		t.Errorf("时刻该按注入时区显示（UTC 09:05 → +08 17:05），实得 %q", body)
	}
}

func TestBodyIsScrubbedBeforeLeavingTheProcess(t *testing.T) {
	// 正文要经过 pushplus / Telegram / 邮件服务商的服务器 —— 出网那一句不该带
	// 可直接复用的凭据或真实号码形状（`core.ScrubText` 是文本级脱敏的唯一入口）。
	body := FormatNotifyBody([]NotifyLogLine{notifyLine(core.LevelInfo,
		"token="+strings.Repeat("a", 40)+" 手机号 13812345678")}, core.NotifyLevelInfo, time.UTC)
	if strings.Contains(body, strings.Repeat("a", 40)) {
		t.Errorf("长 token 串该被遮蔽，实得 %q", body)
	}
	if strings.Contains(body, "13812345678") {
		t.Errorf("11 位手机号形状该被重映射，实得 %q", body)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 门槛：每一条都要说得出为什么没发
// ─────────────────────────────────────────────────────────────────────────────

func TestDisabledSwitchIsReportedBeforeAnythingElse(t *testing.T) {
	sender := &fakeNotifySend{}
	result := notifyRunner(&fakeNotifyConfig{config: core.NotifyConfig{}}, sender).
		Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if result.Sent() || sender.calls != 0 {
		t.Fatalf("开关没开不该发，实得 sent=%v calls=%d", result.Sent(), sender.calls)
	}
	// 顺序有后果：这一条即使"没有过程话"也必须报"总开关没开"。
	empty := notifyRunner(&fakeNotifyConfig{config: core.NotifyConfig{}}, sender).
		Dispatch(context.Background(), NotifyInput{})
	if !strings.Contains(empty.Skipped, "总开关") {
		t.Errorf("开关那一判该排在前面，实得 %q", empty.Skipped)
	}
}

func TestMissingWiringHasItsOwnReasons(t *testing.T) {
	// 没接配置 / 没接发送器 / 整个对象是 nil —— 三种"没接"分得开，
	// 才不会把部署形态念成配置问题。
	noConfigs := NewRunNotify(nil, &fakeNotifySend{}, fixedClock{at: notifyClockAt()}).
		Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if !strings.Contains(noConfigs.Skipped, "配置") {
		t.Errorf("该说没接配置，实得 %q", noConfigs.Skipped)
	}
	noSender := NewRunNotify(enabledWith(nil), nil, fixedClock{at: notifyClockAt()}).
		Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if !strings.Contains(noSender.Skipped, "发送器") {
		t.Errorf("该说没接发送器，实得 %q", noSender.Skipped)
	}
	var nilRunner *RunNotify
	if nilRunner.Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()}).Sent() {
		t.Error("nil 接收者不该发出去")
	}
}

func TestConfigReadFailureIsVisibleButNeverFatal(t *testing.T) {
	configs := &fakeNotifyConfig{err: errors.New("database is locked")}
	result := notifyRunner(configs, &fakeNotifySend{}).
		Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if result.Sent() || !strings.Contains(result.Skipped, "读不出来") {
		t.Fatalf("读配置失败要说得出来，实得 %+v", result)
	}
	if result.Err == nil {
		t.Error("原始错误要留在 Err 里（只用于日志与测试，不进对外文案）")
	}
}

func TestOnlyErrorWaitsForARealError(t *testing.T) {
	sender := &fakeNotifySend{}
	result := notifyRunner(enabledWith(func(c *core.NotifyConfig) { c.OnlyError = true }), sender).
		Dispatch(context.Background(), NotifyInput{
			Summary: "signin：跑了 1 个账号，全部成功",
			Lines: []NotifyLogLine{
				notifyLine(core.LevelInfo, "余额 12 豆"),
				notifyLine(core.LevelWarn, "该主播有 2 间在播"),
			},
		})
	if result.Sent() || sender.calls != 0 || !strings.Contains(result.Skipped, "只推错误") {
		t.Fatalf("只有 info/warn 时不该发，实得 %+v", result)
	}

	// 反过来：真有一条 error 就要发，且**过滤之前的全量**都算判据（旧口径）。
	sent := notifyRunner(enabledWith(func(c *core.NotifyConfig) { c.OnlyError = true }), sender).
		Dispatch(context.Background(), NotifyInput{
			Lines: []NotifyLogLine{notifyLine(core.LevelError, "口令领取失败")},
		})
	if !sent.Sent() {
		t.Fatalf("有一条 error 就该发，实得 %+v", sent)
	}
}

func TestOnlyErrorAtErrorThresholdStillSeesTheError(t *testing.T) {
	// 门槛设成 error 且开了"只推错误" ⇒ 正文只剩 error 那几行（末尾那句 info 小结
	// 被门槛滤掉）。这是**已知形状**，不是漏发：过滤只看级别，与只推错误判据无关。
	result := notifyRunner(enabledWith(func(c *core.NotifyConfig) {
		c.OnlyError = true
		c.MinLevel = core.NotifyLevelError
	}), &fakeNotifySend{}).Dispatch(context.Background(), NotifyInput{
		Summary: "signin：跑了 1 个账号，失败",
		Lines:   []NotifyLogLine{notifyLine(core.LevelError, "上游回了 90001")},
	})
	if result.Body != "[error 09:05:43]上游回了 90001" {
		t.Errorf("正文该只剩那一行，实得 %q", result.Body)
	}
}

func TestEmptyBodyAfterFilteringIsSkipped(t *testing.T) {
	sender := &fakeNotifySend{}
	// 只有 info 行 + error 门槛 ⇒ 过滤后为空 ⇒ 不发（旧实现那句 `if (!msg) return`）。
	result := notifyRunner(enabledWith(func(c *core.NotifyConfig) {
		c.MinLevel = core.NotifyLevelError
	}), sender).Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if result.Sent() || sender.calls != 0 || !strings.Contains(result.Skipped, "正文为空") {
		t.Fatalf("过滤后为空不该发，实得 %+v", result)
	}
	if !strings.Contains(result.Skipped, "error") {
		t.Errorf("原因里要带上是哪道门槛，实得 %q", result.Skipped)
	}
}

func TestNoChannelConfiguredIsNotSilenceAboutNothingToSend(t *testing.T) {
	sender := &fakeNotifySend{}
	result := notifyRunner(enabledWith(nil).withNoChannels(), sender).
		Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if result.Sent() || sender.calls != 0 {
		t.Fatalf("一家都没配不该发，实得 %+v", result)
	}
	if !strings.Contains(result.Skipped, "一家") {
		t.Errorf("要说清是'没配'而不是'配了没发出去'，实得 %q", result.Skipped)
	}
}

func (f *fakeNotifyConfig) withNoChannels() *fakeNotifyConfig {
	f.config.Channels = core.NotifyChannels{}
	return f
}

func TestSummaryLineIsAppendedAsTheLastInfoLine(t *testing.T) {
	sender := &fakeNotifySend{}
	result := notifyRunner(enabledWith(nil), sender).Dispatch(context.Background(), NotifyInput{
		Summary: "signin：跑了 2 个账号，部分成功",
		Lines:   oneInfoLine(),
	})
	lines := strings.Split(result.Body, "\n")
	if len(lines) != 2 {
		t.Fatalf("小结该补成最后一行，实得 %q", result.Body)
	}
	if !strings.HasPrefix(lines[1], "[info ") || !strings.Contains(lines[1], "部分成功") {
		t.Errorf("小结要以 info 收进正文，实得 %q", lines[1])
	}
	// 标题走配置里的默认值（旧 CLI 那句 `mcloud-v2 运行推送` 已刻意改掉，见 core 注释）。
	if sender.title != core.DefaultNotifyTitle {
		t.Errorf("标题该是默认那句，实得 %q", sender.title)
	}
}

func TestConfiguredTitleWins(t *testing.T) {
	sender := &fakeNotifySend{}
	notifyRunner(enabledWith(func(c *core.NotifyConfig) {
		title := "云盘日报"
		c.Title = &title
	}), sender).Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if sender.title != "云盘日报" {
		t.Errorf("配了标题就该用它，实得 %q", sender.title)
	}
}

func TestReportPassesThroughAndReadsAsOneLine(t *testing.T) {
	sender := &fakeNotifySend{report: upstream.NotifyReport{
		{Channel: core.NotifyPushplus, Outcome: upstream.NotifySent},
		{Channel: core.NotifyDingTalk, Outcome: upstream.NotifyFailed, Reason: "HTTP 500"},
		{Channel: core.NotifyBark, Outcome: upstream.NotifySkipped, Reason: "缺少必填项：key"},
	}}
	result := notifyRunner(enabledWith(nil), sender).
		Dispatch(context.Background(), NotifyInput{Lines: oneInfoLine()})
	if !result.Sent() {
		t.Fatalf("该发出去了，实得 %+v", result)
	}
	if got := result.SummaryLine(); got != "推送：已发 1 / 失败 1 / 跳过 1：dingTalk" {
		t.Errorf("小结要逐档报，实得 %q", got)
	}
	// `%v` 与 SummaryLine 同源：日志里顺手打这个结构体时不该冒出正文或凭据。
	if !strings.Contains(result.String(), "推送") {
		t.Errorf("String 该就是那句小结，实得 %q", result.String())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 攒取与上限
// ─────────────────────────────────────────────────────────────────────────────

func TestSinkStopsCollectingAfterTheCap(t *testing.T) {
	var sink runLogSink
	for i := 0; i < notifyMaxLines+7; i++ {
		sink.add(core.LevelInfo, notifyClockAt(), "第"+strconv.Itoa(i)+"句")
	}
	lines := sink.collect(notifyClockAt())
	if len(lines) != notifyMaxLines+1 {
		t.Fatalf("该是 %d 条 + 一条省略说明，实得 %d", notifyMaxLines, len(lines))
	}
	last := lines[len(lines)-1]
	if last.Level != core.LevelWarn || !strings.Contains(last.Message, "7 条省略") {
		t.Errorf("省略多少条必须写在正文里，实得 %+v", last)
	}
	// 正文里的行数与攒到的一致（默认 info 门槛不过滤任何东西）。
	body := FormatNotifyBody(lines, core.NotifyLevelInfo, time.UTC)
	if strings.Count(body, "\n") != len(lines)-1 {
		t.Errorf("正文行数与攒到的不等：%d 行 vs %d 条", strings.Count(body, "\n")+1, len(lines))
	}
}

func TestNilSinkSwallowsEverything(t *testing.T) {
	var sink *runLogSink
	sink.add(core.LevelInfo, notifyClockAt(), "没人收")
	if got := sink.collect(notifyClockAt()); got != nil {
		t.Errorf("nil 接收者该什么都不攒，实得 %+v", got)
	}
	var rec runLogRecorder
	rec.record(core.LevelError, "也不该炸")
}

// ─────────────────────────────────────────────────────────────────────────────
// 接进运行器
// ─────────────────────────────────────────────────────────────────────────────

func TestRunnersWithoutNotifyLeaveTheFieldNil(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	summary, err := runner.Run(context.Background(), RunPlan{
		Capability: "signin", Accounts: []core.AccountID{runAccount("01")}, ParamsDigest: "d",
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	// "没接" 与 "接了但没发" 是两件事（见 RunSummary.Notify 的头注）。
	if summary.Notify != nil {
		t.Errorf("没接推送时该是 nil，实得 %+v", summary.Notify)
	}
}

func TestRunnerSendsTheBodyItJustShowedTheUI(t *testing.T) {
	store := storeWithAccounts(t)
	registry, bus, _ := newRunner(t, store)
	sender := &fakeNotifySend{}
	runner := NewRunner(store, runScope(), fixedClock{at: runNow()}, registry, bus,
		WithRunNotify(NewRunNotify(enabledWith(nil), sender, fixedClock{at: runNow()}).In(time.UTC)))

	summary, err := runner.Run(context.Background(), RunPlan{
		Capability: "signin", Accounts: []core.AccountID{runAccount("01")}, ParamsDigest: "d",
	}, &seqRandom{next: 1}, func(_ context.Context, _ core.AccountID, progress RunProgress) (core.RunReport, error) {
		progress(core.LevelInfo, "已换成新凭据")
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if sender.calls != 1 {
		t.Fatalf("一趟运行该推一次，实得 %d", sender.calls)
	}
	if !strings.Contains(summary.Notify.Body, "[info ") ||
		!strings.Contains(summary.Notify.Body, "已换成新凭据") {
		t.Errorf("界面看到的过程话该原样进正文，实得 %q", summary.Notify.Body)
	}
	// 单账号那一趟**不**加"账号 N："前缀（多账号才需要分辨）。
	if strings.Contains(summary.Notify.Body, "账号 1：") {
		t.Errorf("单账号不该带账号前缀，实得 %q", summary.Notify.Body)
	}
	// 小结那一句也在正文里（推送要能说出"这趟成没成"）。
	if !strings.Contains(summary.Notify.Body, "全部成功") {
		t.Errorf("正文末尾该有运行小结，实得 %q", summary.Notify.Body)
	}
}

func TestMultiAccountLinesArePrefixedInOrder(t *testing.T) {
	store := storeWithAccounts(t)
	registry, bus, _ := newRunner(t, store)
	sender := &fakeNotifySend{}
	runner := NewRunner(store, runScope(), fixedClock{at: runNow()}, registry, bus,
		WithRunNotify(NewRunNotify(enabledWith(nil), sender, fixedClock{at: runNow()}).In(time.UTC)))

	if _, err := runner.Run(context.Background(), RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), runAccount("02")},
		ParamsDigest: "d",
	}, &seqRandom{next: 1}, func(_ context.Context, _ core.AccountID, progress RunProgress) (core.RunReport, error) {
		// 正文里刻意不写账号标识：它由手机号派生，而推送正文要经过第三方服务器 ——
		// "这一句是谁的"由运行器加的那枚序号前缀负责。
		progress(core.LevelInfo, "签到完成")
		return okReport(), nil
	}); err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if !strings.Contains(sender.text, "]账号 1：签到完成") ||
		!strings.Contains(sender.text, "]账号 2：签到完成") {
		t.Errorf("两账号那一趟要分得清谁是谁，实得 %q", sender.text)
	}
}

func TestTaskExceptionCrossesTheOnlyErrorGate(t *testing.T) {
	// 反向可验的那一格：把 `rec.record(core.LevelError, …)` 那一句删掉，
	// 这条会红 —— 表现正是"只推错误"的推送在真失败那一趟什么都没发。
	store := storeWithAccounts(t)
	registry, bus, _ := newRunner(t, store)
	sender := &fakeNotifySend{}
	runner := NewRunner(store, runScope(), fixedClock{at: runNow()}, registry, bus,
		WithRunNotify(NewRunNotify(enabledWith(func(c *core.NotifyConfig) {
			c.OnlyError = true
		}), sender, fixedClock{at: runNow()}).In(time.UTC)))

	if _, err := runner.Run(context.Background(), RunPlan{
		Capability: "signin", Accounts: []core.AccountID{runAccount("01")}, ParamsDigest: "d",
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return core.RunReport{}, errors.New("上游回了 90001")
	}); err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if sender.calls != 1 {
		t.Fatalf("出异常又开了只推错误 ⇒ 该发，实得 calls=%d", sender.calls)
	}
	if !strings.Contains(sender.text, "[error ") || !strings.Contains(sender.text, "90001") {
		t.Errorf("异常那句要以 error 级进正文，实得 %q", sender.text)
	}
}

func TestPushFailureNeverTurnsIntoRunFailure(t *testing.T) {
	// 推送是运行的附属品：三家全失败也不该让 `Run` 回错、不该改 `Outcome`。
	store := storeWithAccounts(t)
	registry, bus, _ := newRunner(t, store)
	sender := &fakeNotifySend{report: upstream.NotifyReport{
		{Channel: core.NotifyPushplus, Outcome: upstream.NotifyFailed, Reason: "HTTP 502"},
	}}
	runner := NewRunner(store, runScope(), fixedClock{at: runNow()}, registry, bus,
		WithRunNotify(NewRunNotify(enabledWith(nil), sender, fixedClock{at: runNow()}).In(time.UTC)))

	summary, err := runner.Run(context.Background(), RunPlan{
		Capability: "signin", Accounts: []core.AccountID{runAccount("01")}, ParamsDigest: "d",
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("推送失败不该让运行回错: %v", err)
	}
	if summary.Outcome != core.OutcomeSuccess {
		t.Errorf("结局不该被推送影响，实得 %v", summary.Outcome)
	}
	if summary.Notify == nil || !strings.Contains(summary.Notify.SummaryLine(), "失败 1") {
		t.Errorf("推送结论要留在小结里（调用方据此记账），实得 %+v", summary.Notify)
	}
}

func TestWithRunNotifyIgnoresNil(t *testing.T) {
	store := storeWithAccounts(t)
	registry, bus, _ := newRunner(t, store)
	runner := NewRunner(store, runScope(), fixedClock{at: runNow()}, registry, bus,
		WithRunNotify(nil))
	if runner.notify != nil {
		t.Error("传 nil 该等于不接（不留两种表示法）")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具（本文件内）
// ─────────────────────────────────────────────────────────────────────────────

func TestThresholdWordsForHumans(t *testing.T) {
	if notifyLevelWord("") != "info（没填，跟随默认）" {
		t.Errorf("空门槛要说'没填'，实得 %q", notifyLevelWord(""))
	}
	if notifyLevelWord(core.NotifyLevelDebug) != "debug" {
		t.Errorf("已知门槛原样报，实得 %q", notifyLevelWord(core.NotifyLevelDebug))
	}
}

func TestClippingIsByCharacter(t *testing.T) {
	// 按字节切会把汉字切成两半（界面拿到乱码尾巴），这一条守的是 rune 截断。
	if got := clipNotifyText("中文中文中文中文", 4); got != "中文中文…" {
		t.Errorf("按字符截断失效，实得 %q", got)
	}
	if got := clipNotifyText("短", 120); got != "短" {
		t.Errorf("没超长的不该被动，实得 %q", got)
	}
}
