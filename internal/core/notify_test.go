package core

// 推送配置模型的用例。这一层没有网络，所以只有两类断言值得写，
// 而它们都是"错了不会炸、只会静默少发或多发"的那一类：
//
//  1. **渠道名与顺序** —— 名字是对外标识（库里的 JSON、界面上的选择项），
//     拼错一个字母表现成"这一家凭空消失"；顺序是发送序，
//     没钉住的话同一份配置两次的日志先后都不一样。
//  2. **必填判据走的是真值** —— [SecretValue] 的 `String()` 恒非空
//     （`<redacted 0 字符>`），所以"判填没填"只要错用一次 `String()`，
//     整族渠道就会在**没配 token 的情况下**被放行，表现成"每次推送都失败一次"
//     而配置页面上看着一切正常。这两条下面各有一例专门钉。

import (
	"strings"
	"testing"
)

// TestNotifyChannelNamesAreTheOldChannelMapLiterals 钉住十个名字的**逐字**写法
// （含大小写：`serverChan` 的 C、`dingTalk` 的 T、`workWeixinBot` 的 B）。
//
// 反向改坏它的样子：把常量值改成 `serverchan`（"更规范的小写"）——
// 编译照过、本仓测试照绿（如果没有这一例），而库里已存的那份配置
// 从此读不出这一家，用户看到的是"配过又没了"。
func TestNotifyChannelNamesAreTheOldChannelMapLiterals(t *testing.T) {
	want := []string{
		"pushplus", "serverChan", "workWeixin", "workWeixinBot", "tgBot",
		"bark", "twoIm", "dingTalk", "email", "customPost",
	}
	got := NotifyChannelOrder()
	if len(got) != len(want) {
		t.Fatalf("渠道数量应是 %d，实得 %d", len(want), len(got))
	}
	for i, name := range want {
		if string(got[i]) != name {
			t.Errorf("第 %d 家应是 %q，实得 %q", i+1, name, string(got[i]))
		}
		if !got[i].Valid() {
			t.Errorf("%q 应判成已知渠道", name)
		}
	}
}

// TestNotifyChannelOrderIsFixedNotLiteralOrder 钉"顺序与写入顺序无关"。
//
// 这一例防的是把 `Names()` 写成遍历 map 那一类退化：Go 的 map 顺序每轮随机，
// 表现是"日志里渠道的先后每次都不同"，排障的人会先怀疑渠道而不是排序。
func TestNotifyChannelOrderIsFixedNotLiteralOrder(t *testing.T) {
	channels := NotifyChannels{
		DingTalk:   &DingTalkConfig{Token: "t"},
		Bark:       &BarkConfig{Key: "k"},
		Pushplus:   &PushplusConfig{Token: "p"},
		CustomPost: []CustomPostConfig{{URL: "https://example.invalid/hook"}},
	}
	want := []NotifyChannel{NotifyPushplus, NotifyBark, NotifyDingTalk, NotifyCustomPost}
	got := channels.Names()
	if len(got) != len(want) {
		t.Fatalf("应列出 %d 家，实得 %d：%v", len(want), len(got), got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("第 %d 家应是 %s，实得 %s", i+1, want[i], got[i])
		}
	}
	// 同一个输入跑五次必须给同一个顺序（"没有随机性"这件事只能重复测）。
	for round := 0; round < 5; round++ {
		again := channels.Names()
		for i := range want {
			if again[i] != want[i] {
				t.Fatalf("顺序不稳定：第 %d 轮第 %d 家是 %s", round, i+1, again[i])
			}
		}
	}
}

// TestUnknownChannelNameIsRejected 钉大小写敏感与"未知不是已知"。
func TestUnknownChannelNameIsRejected(t *testing.T) {
	for _, name := range []NotifyChannel{"", "serverchan", "ServerChan", "twoim", "smtp"} {
		if name.Valid() {
			t.Errorf("%q 不该是已知渠道", name)
		}
	}
}

// TestNotifyLevelCeilingMatchesOldTable 复刻旧 `PUSH_LEVEL_MAX`（0 / 1 / 3 / 4）。
//
// ⚠ 这里没有 2 那一档，而且**不该被"顺手补齐"**：补齐等于把"info 收哪些行"
// 整体挪一格，表现是"以前能进正文的 warn 行现在不进了"，而没人会改测试。
func TestNotifyLevelCeilingMatchesOldTable(t *testing.T) {
	cases := map[NotifyLogLevel]int{
		NotifyLevelError: 0, NotifyLevelWarn: 1, NotifyLevelInfo: 3, NotifyLevelDebug: 4,
	}
	for level, want := range cases {
		if got := level.LevelCeiling(); got != want {
			t.Errorf("%s 的上界应是 %d，实得 %d", level, want, got)
		}
	}
	if got := NotifyLogLevel("trace").LevelCeiling(); got != -1 {
		t.Errorf("未知门槛应返回 -1（判\"不认识\"），实得 %d", got)
	}
	if NotifyLevelInfo.Valid() != true || NotifyLogLevel("trace").Valid() != false {
		t.Error("Valid 与 LevelCeiling 的判据分叉了")
	}
	if len(NotifyLevels()) != 4 {
		t.Errorf("合法门槛应是 4 档，实得 %d", len(NotifyLevels()))
	}
}

// TestMissingUsesTheRealValueNotTheMaskedOne ⭐ 本文件最重要的一例。
//
// 把 `fieldOf(c.Pushplus.Token.Value(), …)` 改成 `string(c.Pushplus.Token.String())`
// 之后，空 token 会被判成"填了"（`String()` 恒返回 `<redacted 0 字符>`），
// 而界面上"能发 0 家"会变成"能发 1 家"—— 用户看到的就是每次运行都白失败一次。
func TestMissingUsesTheRealValueNotTheMaskedOne(t *testing.T) {
	var empty SecretValue
	if empty.String() == "" {
		t.Fatal("前提不成立：SecretValue.String() 竟会是空串，这一例要重写")
	}
	if empty.Value() != "" {
		t.Fatalf("前提不成立：Value() 应是原文，实得 %q", empty.Value())
	}
	channels := NotifyChannels{
		Pushplus:   &PushplusConfig{Token: ""},
		ServerChan: &ServerChanConfig{Token: "   "},
		Bark:       &BarkConfig{Key: "key-1"},
	}
	if got := channels.Missing(NotifyPushplus); got != "token" {
		t.Errorf("空 token 应判缺 token，实得 %q", got)
	}
	if got := channels.Missing(NotifyServerChan); got != "token" {
		t.Errorf("纯空白 token 也应判缺，实得 %q", got)
	}
	if got := channels.Missing(NotifyBark); got != "" {
		t.Errorf("填全的渠道不该报缺，实得 %q", got)
	}
}

// TestMissingCoversEveryChannelOnceCompleteAndOnceEmpty 逐个渠道对一遍：
// 没配这一家 ⇒ 报这一家；配全 ⇒ 不报。
//
// 这一例的存在理由：`present` 与 `Missing` 是两个 switch，新增一家只改一个
// 的话，表现是"这一家永远读不到"或"永远不校验"，两样都不会被别的测试带到。
func TestMissingCoversEveryChannelOnceCompleteAndOnceEmpty(t *testing.T) {
	complete := NotifyChannels{
		Pushplus:      &PushplusConfig{Token: "p"},
		ServerChan:    &ServerChanConfig{Token: "s"},
		WorkWeixin:    &WorkWeixinConfig{CorpID: "c", CorpSecret: "s"},
		WorkWeixinBot: &WorkWeixinBotConfig{URL: "https://example.invalid/bot"},
		TGBot:         &TGBotConfig{Token: "t", ChatID: "1"},
		Bark:          &BarkConfig{Key: "k"},
		TwoIM:         &TwoIMConfig{Key: "k", SID: "s"},
		DingTalk:      &DingTalkConfig{Token: "d"},
		Email:         &EmailConfig{Host: "smtp.example.invalid", From: "a@b.c", Pass: "pw"},
		CustomPost:    []CustomPostConfig{{URL: "https://example.invalid/hook"}},
	}
	for _, name := range NotifyChannelOrder() {
		if got := complete.Missing(name); got != "" {
			t.Errorf("%s 配全了却报缺：%q", name, got)
		}
		if !complete.present(name) {
			t.Errorf("%s 配全了却没被 present 认出来", name)
		}
		// 空配置里每一家都必须报"缺这一家"，而不是空串（空串 = 齐了 ⇒ 会发出去）。
		if got := (NotifyChannels{}).Missing(name); got != string(name) {
			t.Errorf("没配 %s 时应报 %q，实得 %q", name, name, got)
		}
	}
	if got := (NotifyChannels{}).Names(); len(got) != 0 {
		t.Errorf("空配置不该列出任何渠道，实得 %v", got)
	}
}

// TestWorkWeixinAndEmailReportTheFirstMissingField 多渠道必填的**先后**：
// 旧实现是 `!pass || !from || !host` 一个条件，只说"没配"；这里要说得出缺哪个，
// 但一次只报第一个 —— 界面上一句"缺 corpsecret"比"缺两个"更好修。
func TestWorkWeixinAndEmailReportTheFirstMissingField(t *testing.T) {
	channels := NotifyChannels{
		WorkWeixin: &WorkWeixinConfig{CorpID: "corpid", CorpSecret: ""},
		Email:      &EmailConfig{Host: "", From: "a@b.c", Pass: "pw"},
	}
	if got := channels.Missing(NotifyWorkWeixin); got != "corpsecret" {
		t.Errorf("应报 corpsecret，实得 %q", got)
	}
	if got := channels.Missing(NotifyEmail); got != "host" {
		t.Errorf("应报 host，实得 %q", got)
	}
}

// TestCustomPostNeedsAURLPerItem 钉"数组里有一条没 URL ⇒ 整家不健康"。
//
// 旧实现是**逐条** `if (!apiTemplate?.url) return;`（静默跳过那一条，其余照发）。
// 这里刻意判整家：一家配了两条、其中一条是空的时候，"少发一条且没人知道"
// 比"这一家被拦下、界面上看得见原因"更难查。差异记在这儿，不写进注释就走形。
func TestCustomPostNeedsAURLPerItem(t *testing.T) {
	good := NotifyChannels{CustomPost: []CustomPostConfig{{URL: "https://a.invalid"}, {URL: "https://b.invalid"}}}
	if got := good.Missing(NotifyCustomPost); got != "" {
		t.Errorf("两条都有 URL，不该报缺，实得 %q", got)
	}
	bad := NotifyChannels{CustomPost: []CustomPostConfig{{URL: "https://a.invalid"}, {URL: "  "}}}
	if got := bad.Missing(NotifyCustomPost); got != "url" {
		t.Errorf("第二条空 URL 应报 url，实得 %q", got)
	}
}

// TestConfiguredNamesIsNarrowerThanNames 钉"配了 4 家 / 能发 3 家"这一格区分。
func TestConfiguredNamesIsNarrowerThanNames(t *testing.T) {
	config := NotifyConfig{
		Channels: NotifyChannels{
			Pushplus: &PushplusConfig{Token: "p"},
			Bark:     &BarkConfig{Key: ""}, // 配了这一家，但没填 key
			DingTalk: &DingTalkConfig{Token: "d"},
			Email:    &EmailConfig{Host: "h", From: "f", Pass: "p"},
		},
	}
	if got := len(config.Channels.Names()); got != 4 {
		t.Errorf("配了 4 家，实得 %d", got)
	}
	happy := config.ConfiguredNames()
	if len(happy) != 3 {
		t.Fatalf("能发的应是 3 家，实得 %d：%v", len(happy), happy)
	}
	for _, name := range happy {
		if name == NotifyBark {
			t.Error("bark 没填 key，不该出现在\"能发\"那一列")
		}
	}
}

// TestValidateStillChecksChannelsWhenTheLevelIsEmpty ⭐ 反向例：
// 曾经写过一版"空门槛就早退"，那会把**整份**校验跳掉，
// 于是"配了 bark 没填 key"能在保存时通过，到运行时才失败一次。
func TestValidateStillChecksChannelsWhenTheLevelIsEmpty(t *testing.T) {
	config := NotifyConfig{
		MinLevel: "", // 空 = 按 info 走，不判非法
		Channels: NotifyChannels{Bark: &BarkConfig{Key: ""}},
	}
	err := config.Validate()
	if err == nil {
		t.Fatal("空门槛不该让渠道检查一起被跳过")
	}
	if !strings.Contains(err.Error(), "bark") {
		t.Errorf("错误串里要点名渠道，实得 %q", err.Error())
	}

	if err := (NotifyConfig{MinLevel: "trace"}).Validate(); err == nil {
		t.Error("未知门槛应被拒")
	}
	if err := (NotifyConfig{MinLevel: NotifyLevelWarn}).Validate(); err != nil {
		t.Errorf("合法门槛不该报错：%v", err)
	}
	if err := DefaultNotifyConfig().Validate(); err != nil {
		t.Errorf("默认配置应可直接保存：%v", err)
	}
}

// TestDefaultConfigIsOffAndInfo 钉两个默认值。
//
// 默认关闭**必须**有例：这一族会把运行正文发到第三方与用户邮箱，
// "升级后自动开始外发"是不可逆的噪声；而它坏起来是静默的（没人会注意到
// 自己没配过的渠道在往外发东西）。
func TestDefaultConfigIsOffAndInfo(t *testing.T) {
	config := DefaultNotifyConfig()
	if config.Enabled {
		t.Error("推送默认必须关闭")
	}
	if config.MinLevel != NotifyLevelInfo {
		t.Errorf("默认门槛应是 info（旧实现的默认），实得 %q", config.MinLevel)
	}
	if config.OnlyError {
		t.Error("默认不收\"只发错误\"——那是用户勾上去才有的一项")
	}
	if len(config.Channels.Names()) != 0 {
		t.Error("默认不该有任何渠道")
	}
	if got := config.TitleOrDefault(); got != DefaultNotifyTitle {
		t.Errorf("没配标题时应给默认标题，实得 %q", got)
	}
}

// TestTitleFallsBackOnWhitespaceOnly 钉"填空白串 = 没填"。
// 反向例：只判 `nil` 的话，一个空串标题会真的推一条没有标题的消息。
func TestTitleFallsBackOnWhitespaceOnly(t *testing.T) {
	blank := "   "
	if got := (NotifyConfig{Title: &blank}).TitleOrDefault(); got != DefaultNotifyTitle {
		t.Errorf("空白标题应回落到默认，实得 %q", got)
	}
	named := "签到日报"
	if got := (NotifyConfig{Title: &named}).TitleOrDefault(); got != named {
		t.Errorf("填了标题就该用填的那个，实得 %q", got)
	}
}
