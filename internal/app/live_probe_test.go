// 活体探针：把"某一族真打上游一次"做成**可重复**的一格，而不是每轮现编一份临时程序。
//
// # 为什么需要这一族，而不是"跑一趟 app-task 等它自己出来"
//
// app-task 的玩法**只有在那条任务当天真的出现在表里**时才会被跑到。连续两趟真跑都撞上
// 同一格：2026-09-25 那趟 107（云笔记）在表里没有 ⇒ 一枪没发；2026-09-26 这一趟 472/473
// （体验 139 邮箱）同样不在两号的表里 ⇒ 四枪一枪没发。⇒ "接完再真跑"这个节奏对
// **当天不开放**的那一族根本不成立，而那正是 AGENTS.md §1.13 要避免的读法
// （"缺一次运气"不是停工理由，得自己想法子去跑）。探针走的是**生产代码同一条路径**
// （[taskExecutor.mailSessionFor] / [taskExecutor.runMailExperience] /
// [upstream.MailAutoLoginRequest]），所以它的读数就是这套代码真跑出来的读数。
//
// # 三条边界（删掉任何一条都会让它变成"随手真写用户的账"）
//
//   - **默认跳过**：三个 env 全给齐才动，确认串是**一句人话**而不是 `1` ——
//     这一族第 ④ 枪会真把任务记成已做，而上游没有"撤销打卡"那一口；
//   - **只读副本库**：探针自己不写库（换票与建会话都不落库），但仍要求传副本路径 ——
//     与真跑同一纪律，不给"顺手指到 `data/mcloud.db`"留口子。它只**读**账号凭据；
//   - **值级脱敏**：所有打印过 [liveProbeSink.mask]，把这一趟见过的每一串凭据
//     （SSO 票 / `sid` / `rmkey` / `Authorization` / 手机号 / jwt）逐串换掉；
//     响应体只交**形状**（键 + 类型 + 长度），值只有 `code`/`msg` 那一小撮白名单原样出。
//
// ⚠ 这一族**没有断言**：它不是回归测试，是"拿一次读数"的仪器 —— 判据落在
// `docs/status.md` 与 `docs/pending-decisions.md`，不在这里。写成断言的话，
// 上游哪天改口它就会红，而红的那一格既不是缺陷也不是判据。
package app

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	liveProbeDBEnv      = "MCLOUD_LIVE_PROBE_DB"
	liveProbeIndexEnv   = "MCLOUD_LIVE_PROBE_ACCOUNT_INDEX"
	liveProbeDeviceEnv  = "MCLOUD_LIVE_PROBE_DEVICE_FILE"
	liveProbeConfirmEnv = "MCLOUD_LIVE_PROBE_CONFIRM"
	// liveProbeConfirm 是那句**人话**确认串。为什么要打字而不是 1：这一族的 ④
	// 是不可回滚的写，而"env 设了个 1"经常是抄来的命令历史。
	liveProbeConfirm = "确认写真实上游"
)

// liveProbeSink 收探针的过程话，顺手把每一串见过的凭据登记进掩码表。
//
// ⚠ 掩码表是**累加**的：探针越往后走见过的串越多，前面那些行的输出也照样得重出一次
// 才安全 —— 所以这里刻意不缓存已打印的内容，每一行打印时都过一遍当刻的表。
type liveProbeSink struct {
	t       *testing.T
	secrets []string
	lines   []string
}

func newLiveProbeSink(t *testing.T) *liveProbeSink {
	return &liveProbeSink{t: t}
}

// mask 把文本里出现过的每一串已登记凭据换掉（先长后短：短串可能是长串的子串）。
func (s *liveProbeSink) mask(text string) string {
	candidates := append([]string{}, s.secrets...)
	sort.SliceStable(candidates, func(i, j int) bool {
		return len(candidates[i]) > len(candidates[j])
	})
	for _, secret := range candidates {
		if len(secret) < 4 {
			continue // 4 位以下的串会误伤（`WAIT`、`0000` 这类读数本来就要留）
		}
		text = strings.ReplaceAll(text, secret, "<masked>")
	}
	return text
}

func (s *liveProbeSink) see(secret string) {
	if secret != "" {
		s.secrets = append(s.secrets, secret)
	}
}

// say 打一行过程话（同时进 [s.lines]，跑完再整体掩码打一次，防止"登记晚了"）。
func (s *liveProbeSink) say(format string, args ...any) {
	line := fmt.Sprintf(format, args...)
	s.lines = append(s.lines, line)
	s.t.Log(s.mask(line))
}

// progress 接 [RunProgress]：玩法自己念的那些行从这里出来。
func (s *liveProbeSink) progress(level core.LogLevel, message string) {
	s.say("播报[%d] %s", int(level), message)
}

// dump 打一份**响应形状**（不是响应体）。`why` 只为人话前缀。
func (s *liveProbeSink) dump(why string, response core.HTTPResponse) {
	text, err := response.Text()
	if err != nil {
		s.say("%s：HTTP %d，体解不成 UTF-8（%d 字节）", why, response.Status, len(response.Body))
		return
	}
	s.say("%s：HTTP %d，体形状 %s", why, response.Status, liveProbeShape("", text))
}

// liveProbeValueKeys 是"值可以原样打印"的那几枚键 —— 它们要么是状态码要么是给人看的话，
// 而这一族的凭据键（sid/rmkey/token/authorization/…）一律不在名单里。
var liveProbeValueKeys = map[string]bool{
	"code": true, "msg": true, "message": true, "success": true, "result": true,
}

// liveProbeShape 把一段响应文本压成"键 + 类型 + 长度"的读数是这一族要的那格：
// inlogin 的真实响应**到底有哪些键**至今零样本，而直接把体打出来等于把凭据发进日志。
//
// 非 JSON 时不猜：只给字节数与前 40 字节的**掩码后**内容（旧 TS 那一族有可能回
// `text/javascript` 壳或一段 HTML，那一格本身也是读数）。
func liveProbeShape(key, text string) string {
	value, ok := core.ParseJSON(text)
	if !ok {
		loose := strings.ReplaceAll(text, "'", `"`)
		value, ok = core.ParseJSON(loose)
		if !ok {
			return fmt.Sprintf("非 JSON（%d 字节，前 40 字节 %q）", len(text), liveProbeClip(text, 40))
		}
		key += "(松散解)"
	}
	return liveProbeShapeValue(key, value)
}

func liveProbeShapeValue(key string, value any) string {
	switch typed := value.(type) {
	case map[string]any:
		keys := make([]string, 0, len(typed))
		for name := range typed {
			keys = append(keys, name)
		}
		sort.Strings(keys)
		parts := make([]string, 0, len(keys))
		for _, name := range keys {
			parts = append(parts, liveProbeShapeValue(name, typed[name]))
		}
		return fmt.Sprintf("%s={%s}", key, strings.Join(parts, " "))
	case []any:
		if len(typed) == 0 {
			return key + "=[]"
		}
		return fmt.Sprintf("%s=[%d 项，第 0 项 %s]", key, len(typed),
			liveProbeShapeValue("x", typed[0]))
	case string:
		if liveProbeValueKeys[key] {
			return fmt.Sprintf("%s=%q", key, liveProbeClip(typed, 24))
		}
		return fmt.Sprintf("%s=str<%d>", key, len(typed))
	default:
		// ⚠ **必须带类型**（2026-09-26 第廿四趟为这一格专门补的）：JSON 里的数字 `0`
		// 与字符串 `"0"` 在这一族里**长得一模一样**（旧实现与上游两头都见过两种写法），
		// 而"要不要按 `code != 0` 判失败"取决于解析器读到的那一格是哪种 ——
		// 不带类型时那份读数只能回答"有这一格"，答不了"这一格怎么解"（R26 补判定的前提）。
		return fmt.Sprintf("%s=%s", key, liveProbeJSONTypedWord(typed))
	}
}

// liveProbeJSONTypedWord 给标量那一格配上它**在 JSON 里**的写法（数字带 `(数字)`，字符串带引号）。
//
// 与 [liveProbeScalarWord] 分家：那一格是给 `items[]` 明细**看值**用的（不区分形态），
// 这一格要回答的是"这一枚到底是 `0` 还是 `"0"`" —— 本仓的 JSON 解析走 `UseNumber`，
// 所以数字到这里是 `json.Number`，与真字符串是两个 case，不分开打就会两种长得一样。
func liveProbeJSONTypedWord(value any) string {
	switch typed := value.(type) {
	case json.Number:
		return typed.String() + "(数字)"
	case string:
		return strconv.Quote(typed)
	case bool:
		return strconv.FormatBool(typed) + "(布尔)"
	case nil:
		return "null"
	}
	return strconv.Quote(fmt.Sprint(value)) + "(未知类型)"
}

func liveProbeClip(text string, n int) string {
	runes := []rune(strings.ReplaceAll(strings.ReplaceAll(text, "\n", " "), "\r", ""))
	if len(runes) <= n {
		return string(runes)
	}
	return string(runes[:n]) + "…"
}

// liveProbeWorld 装配"真客户端 + 真库（副本）+ 真设备档案"的那一份世界。
//
// 设备档案刻意**复用**同一趟真跑那份 `--app-device-file`：探针与真跑用的是同一台
// "机器"，否则读到的一句"设备不合法"会被当成上游对这一族的态度。
func liveProbeWorld(t *testing.T) (TaskMatrixRun, core.AccountID, *liveProbeSink, core.HTTPClient) {
	return liveProbeWorldWith(t, nil)
}

// liveProbeWorldWith 与 [liveProbeWorld] 同一份世界，多给一个**客户端加工点**。
//
// 为什么开这个口子而不是各处自己重装配一遍：这份世界里有六格容易装配错的面
// （四个 env 的校验、"两个路径必须绝对且已存在"、副本库 + 同一台设备档案、
// `Progress` 接探针的掩码表）。任何一处自己重装配，就会出现"探针 A 走了掩码、
// 探针 B 忘了"那种泄漏面 —— 而它只在日志里，门禁抓不到。
//
// `wrap` 拿到的 `client` 是**还没被上游包起来**的那一份真实客户端，返回值替它进
// [upstream.NewUpstream] ⇒ 这一趟**所有**请求都经过它（不是只经过某一条链）。
// 刻意在 `sink` 之后调用：加工点多半是一层"记录器"，它要往同一份掩码表里登记见过的串。
// 传 nil 与 [liveProbeWorld] 逐字等价。
func liveProbeWorldWith(
	t *testing.T, wrap func(client core.HTTPClient, sink *liveProbeSink) core.HTTPClient,
) (TaskMatrixRun, core.AccountID, *liveProbeSink, core.HTTPClient) {
	t.Helper()
	dbPath := os.Getenv(liveProbeDBEnv)
	accountIndex := os.Getenv(liveProbeIndexEnv)
	deviceFile := os.Getenv(liveProbeDeviceEnv)
	confirm := os.Getenv(liveProbeConfirmEnv)
	if dbPath == "" && accountIndex == "" && deviceFile == "" && confirm == "" {
		t.Skipf("没给 %s / %s / %s / %s ⇒ 探针不参与常规回归（这是默认档）",
			liveProbeDBEnv, liveProbeIndexEnv, liveProbeDeviceEnv, liveProbeConfirmEnv)
	}
	if confirm != liveProbeConfirm {
		t.Fatalf("%s 必须逐字是 %q（这一族第 ④ 枪是不可回滚的写），实得 %q",
			liveProbeConfirmEnv, liveProbeConfirm, confirm)
	}
	index, err := strconv.Atoi(accountIndex)
	if err != nil {
		t.Fatalf("%s 要是账号下标，实得 %q", liveProbeIndexEnv, accountIndex)
	}
	// ⚠ 两个路径都**只收绝对路径**：`go test` 的工作目录是**包目录**
	// （`internal/app`），相对路径既打不到副本库、又会被下面两个「不存在就新建」的
	// 构造函数字面兑现 —— 结果是「在包目录里凭空长出一份空库和一个设备指纹文件」，
	// 而探针只是读到 0 个账号。这一族跑的是真上游，不给它这种静默造物的口子。
	for name, path := range map[string]string{liveProbeDBEnv: dbPath, liveProbeDeviceEnv: deviceFile} {
		if !filepath.IsAbs(path) {
			t.Fatalf("%s 必须是绝对路径（实得 %q）：相对路径按 go test 的工作目录解释，"+
				"而库与设备文件都是「不存在就新建」⇒ 会在包目录里造出一份空档，读数全废",
				name, path)
		}
		if _, statErr := os.Stat(path); statErr != nil {
			t.Fatalf("%s 指的文件读不到：%v（探针**不**创建它）", name, statErr)
		}
	}
	store, err := mcloudio.Open(dbPath)
	if err != nil {
		t.Fatalf("打不开副本库 %s：%v", dbPath, err)
	}
	t.Cleanup(func() { _ = store.Close() })
	client := mcloudio.NewClient()

	ctx := context.Background()
	accounts, aerr := store.ListAccounts(ctx, core.ScopeLocal())
	if aerr != nil {
		t.Fatalf("读账号列表失败：%v", aerr)
	}
	if index < 0 || index >= len(accounts) {
		t.Fatalf("账号下标 %d 越界（这份库里 %d 个）", index, len(accounts))
	}
	account := accounts[index].ID

	deviceID, perr := LoadOrCreateAppDeviceID(deviceFile, mcloudio.SystemRandom{})
	if perr != nil {
		t.Fatalf("读不出设备档案 %s：%s", deviceFile, perr.Error())
	}

	sink := newLiveProbeSink(t)
	sink.say("探针现场：库里 %d 个账号，用第 %d 个（id 长度 %d，值不打印）",
		len(accounts), index, len(account.String()))
	var probed core.HTTPClient = client
	if wrap != nil {
		// 加工点换的是**整趟**那一份客户端 ⇒ 用一个新变量，不把 `mcloudio.Client`
		// 那份具体类型冲掉（下面返回给调用方的就是被包起来的那一份）。
		probed = wrap(client, sink)
	}
	run := TaskMatrixRun{SignInWorld: SignInWorld{
		Store:    store,
		Scope:    core.ScopeLocal(),
		Upstream: upstream.NewUpstream(probed),
		Device: upstream.NewIPadAppDevice(
			deviceID, NewAppDeviceShort(mcloudio.SystemRandom{})),
		Clock:    mcloudio.SystemClock{},
		Random:   mcloudio.SystemRandom{},
		Sessions: NewSessionPool(),
		Progress: sink.progress,
	}}
	return run, account, sink, probed
}

// TestLiveProbeMail139Family 让 139 邮箱那一族的四枪各出一次真实读数，
// 并在会话成立时追加资产侧四口（⑤⑦ 只读 / ⑧⑥ 是写，各发一次）。
//
// 逐枪要看的是这几格（都在 `docs/pending-decisions.md` 里挂着）：
//   - ② 的真实响应**到底有没有** `var.sid` / `var.rmkey`（形状零样本 ⇒ 我们的解析器
//     是按旧 TS 的文字形状写的）；
//   - ③ 到底要不要 `authorization`（带与不带各发一次，比对状态码；这一口零实拍）；
//   - ④ `taskClickV2` 对 472 认不认（回 `code` 就是上游的原话）；
//   - ⑤ `disk:surplusNum` 与 ⑦ `mbox:getComposeId` 在**另外两台**子域上认不认这张会话
//     —— R35 剩下的那一半靠这两发收窄（详见函数末尾那一档的注释）；
//   - ⑧ `mailsms/sms/backup`（玩法 1020 的本体）与 ⑥ `disk:convertPdfFromYunFile`
//     （玩法 1014 的本体）**认不认这一发**。这两口今天只有夹具依据 ⇒ 没有本探针就永远
//     没有真实反馈，而 ⑧ 与 ③ 同机：两句读数不一样，R35 才能收到"路径"这一级。
//   - 我们那两个解析器（[upstream.ParseMailSurplusNum] / [upstream.ParseMailComposeID]）
//     在真实体上读不读得出来 —— 它们同样是零实拍落码的。
//
// ⚠ 472 今天**不在表里**也算读数：那一格的读数就是"回什么码"，不是"任务有没有被记成完成"。
// 表里有没有这一条，由 ① 之前那次拉表说 —— 本探针不拉表（那是 app-task 的只读那一半）。
func TestLiveProbeMail139Family(t *testing.T) {
	run, account, sink, client := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())
	executor := newTaskExecutor(run, opening, account)

	// ── ① 换 SSO 票（这一发 app-task 真跑里已经验过；这里只要那张票）
	timestamp := UserTimestamp(run.Clock)
	ssoToken, perr := run.Upstream.AppQuerySpecToken(ctx, opening.Profile, timestamp,
		opening.Credential.Authorization(), upstream.Mail139SourceID)
	if perr != nil {
		sink.say("① querySpecTokenV2(sourceId=%s) 失败：%s ⇒ ②③④ 不发",
			upstream.Mail139SourceID, perr.Error())
		t.FailNow()
	}
	sink.see(ssoToken)
	sink.say("① 换到票：长度 %d（值不打印，已登记进掩码表）", len(ssoToken))

	// ── ② 换邮箱会话：**先看真实响应形状**，再让我们的解析器读它
	request, perr := upstream.MailInLoginRequest(ssoToken, core.UUIDv4(run.Random))
	if perr != nil {
		t.Fatalf("② 的构造失败：%s", perr.Error())
	}
	sink.say("② 请求：POST %s，头 %d 枚，体 %d 字节（体内含票 ⇒ 不打印）",
		request.URL, len(request.Headers), len(request.Body))
	response, serr := client.Send(ctx, request)
	if serr != nil {
		sink.say("② 传输层失败：%s ⇒ ③④ 不发", sink.mask(serr.Error()))
		t.FailNow()
	}
	sink.dump("② inlogin 真实响应", response)
	session, parseErr := upstream.ParseMailSession(response, ssoToken)
	if parseErr != nil {
		sink.say("② 我们的解析器读不出来：%s", parseErr.Error())
	} else {
		sink.see(session.SID.Value())
		sink.see(session.RMKey.Value())
		sink.say("② 解析结果：可用=%v sid 长度 %d rmkey 长度 %d（值不打印）",
			session.OK(), len(session.SID.Value()), len(session.RMKey.Value()))
	}
	if !session.OK() {
		sink.say("⇒ 没有会话，③④ 两枪不发（这一族的边界：② 不成则整条不做）")
		t.FailNow()
	}

	// ── ③ 活跃：三档对照，逐档把**状态码 + 体形状**都念出来
	//
	// 为什么要三档：这一口零实拍，而旧 TS 的 `triggerMailActive` 外面套的是 `tryRequest`
	// —— 失败**不打日志**，所以"这一口在旧实现里到底通没通"从来不是已知事实。
	// 三档分别排除三种解释：头没给对（带 / 不带 authorization）、
	// 值被我们编码坏了（生产代码走 `PercentEncode`，旧 TS 是**裸拼**进 URL）、
	// 还是这一口本身就不是那个地址。
	//
	// ⚠ `client.Send` 在非 2xx 时**同时**交回响应与错误（见 `internal/io/client.go`
	// 的 `Send`），所以别把 `serr != nil` 当成"没有响应"跳过 —— 那一档就白跑了。
	for _, variant := range []struct {
		label string
		build func() (core.HTTPRequest, *core.Error)
	}{
		{"带 authorization（生产代码走的那一档）", func() (core.HTTPRequest, *core.Error) {
			return upstream.MailAutoLoginRequest(session, opening.Credential.Authorization())
		}},
		{"不带 authorization", func() (core.HTTPRequest, *core.Error) {
			return upstream.MailAutoLoginRequest(session, "")
		}},
		{"两枚值**裸拼**进 URL（照旧 TS 的字面形状）", func() (core.HTTPRequest, *core.Error) {
			request := core.NewRequestGet("https://mail.10086.cn/login/autologin.action"+
				"?sid="+session.SID.Value()+"&rmkey="+session.RMKey.Value()).
				WithHeader("authorization", opening.Credential.Authorization())
			return request, nil
		}},
	} {
		probe, perr := variant.build()
		if perr != nil {
			sink.say("③ %s：构造失败 %s", variant.label, perr.Error())
			continue
		}
		probeResponse, serr := client.Send(ctx, probe)
		sink.dump("③ autologin（"+variant.label+"）", probeResponse)
		if serr != nil {
			sink.say("③ %s：错误 %s（URL 里有两枚凭据 ⇒ 已掩码）",
				variant.label, sink.mask(serr.Error()))
		}
	}

	// ── ④ 打卡：先把**响应体**念一遍（生产代码那一条只交回错误类别），
	// 再走一遍生产路径拿它自己的读数。两档都打同一个 404 地址 ⇒ 不会记成已做。
	id := int64(472)
	name := "体验139邮箱"
	state := "WAIT"
	enable := int64(1)
	market := upstream.MarketName
	item := upstream.TaskV2Item{
		TaskItem:   upstream.TaskItem{TaskID: &id, Name: &name, State: &state},
		Enable:     &enable,
		MarketName: &market,
	}
	// ⚠ 直发失败时包成 [core.NewBadRequest] 而不是原样交回：[SignInSessionSource.Call]
	// 只在"会话过期"那一类上刷新重放，而这一档要看的是**上游对这一枪第一次说什么**，
	// 被刷新重放一遍就变成两次 404 现场，读不出哪一条是原话。
	_, directErr := do(ctx, executor.src, func(s SignInSession) (struct{}, *core.Error) {
		sink.see(s.jwt) // 这一档要打错误原文 ⇒ 先把这一枚登记进掩码表
		request, perr := upstream.MailMarketTaskClickRequest(
			s.jwt, s.referer, upstream.MarketName, id)
		if perr != nil {
			return struct{}{}, perr
		}
		response, serr := client.Send(ctx, request)
		sink.dump("④ taskClickV2（直发，看体）", response)
		// ④-对照枪（**只读**）：同一台 `:7071` 入口上的另一条路由。
		// ⚠ 这一档刻意标成 "④-对照" 而不是 "⑤"：`app_mail139_assets.go` 文件头把资产侧
		// 那四口编成 ⑤⑥⑦⑧，本函数末尾的 ⑤⑦ 两枪就是那两口的读数，同号会读串。
		// 为什么要它：④ 回的是一个 **JSON 形状**的 404（`{code:"404", msg:"访问资源不存在"}`），
		// 而 ③ 回的是 HTML 404 页 —— 两句不一样，说明 ④ 那一枪**到了应用**。
		// 但"到了应用"仍有两种可能：整个 `:7071/ycloud/` 入口都不认，或只有
		// `signin/task/taskClickV2` 这一条路由没了。这一枪专门把两者分开：
		// 它 200 ⇒ 入口活着，挂的是那一条路由；它也 404 ⇒ 更可能是整片入口的问题。
		controlResponse, _ := client.Send(ctx, upstream.NoticeTaskInfoRequest(s.jwt, s.referer))
		sink.dump("④-对照（同入口另一条只读路由 getTaskInfo）", controlResponse)
		if serr != nil {
			return struct{}{}, core.NewBadRequest(sink.mask(serr.Error()))
		}
		return struct{}{}, nil
	})
	if directErr != nil {
		sink.say("④ 直发那一档的结论：%s", sink.mask(directErr.Error()))
	}
	fired, problems := executor.runMailExperience(ctx, item)
	sink.say("④ 走完整条玩法：发过枪=%v 失败原因 %d 条", fired, len(problems))
	for _, problem := range problems {
		sink.say("④ 原因：%s", problem)
	}

	// ── ⑤⑦ 资产侧两口（**都只读**）：拿 ② 这张会话打**另外两台**子域
	//
	// 这一档要收的是 R35 剩下的那一半，而它的分量全在"打的不是同一台"：
	//   - ③④ 都回 404，两台里 ③ 打的是 `mail.10086.cn` 本身 ⇒ 光看那两枪分不开
	//     "这张会话无效" 与 "只有那两条路由没了"。
	//   - ⑤ 打 `html5.mail.10086.cn`、⑦ 打 `appmail.mail.10086.cn`，用的正是 ② 换到的
	//     同一枚 `sid` / `rmkey`。它们回 `S_OK` ⇒ 会话**是有效的**，③④ 的 404 只能是
	//     那两条路由；它们也 404 ⇒ 范围从"两条路由"扩到"这一片 web 入口"。
	// 两种读数都有用，所以这一档**不设**"前面挂了就不发"的闸（② 成了就有会话）。
	//
	// ⚠ 两口都**只发一次直发档**：生产路径（[upstream.Upstream.AppMailSurplusNum] /
	// [upstream.Upstream.AppMailComposeID]）走的是同一个构造器，只多一道判定，
	// 所以这里把**体形状**与**我们解析器的读数**都念出来，判定文本另发一次拿原话。
	cguid, rnd := mailDiskRandoms(run.Random)
	assetCounts, assetStatus, assetErr := run.Upstream.AppMailSurplusNum(ctx, session, cguid, rnd)
	if assetErr != nil {
		sink.say("⑤ surplusNum（生产路径）：%s", sink.mask(assetErr.Error()))
	} else {
		sink.say("⑤ surplusNum（生产路径）：权益计数=%q 状态=%q",
			liveProbeClip(assetCounts, 24), liveProbeClip(assetStatus, 24))
	}
	surplusRequest, perr := upstream.MailSurplusNumRequest(session, cguid, rnd)
	if perr != nil {
		sink.say("⑤ 构造失败：%s", perr.Error())
	} else {
		surplusResponse, serr := client.Send(ctx, surplusRequest)
		sink.dump("⑤ surplusNum（直发，看体）", surplusResponse)
		if serr != nil {
			sink.say("⑤ 直发错误：%s（URL 里有 sid ⇒ 已掩码）", sink.mask(serr.Error()))
		}
		counts, status, pererr := upstream.ParseMailSurplusNum(surplusResponse)
		if pererr != nil {
			sink.say("⑤ 我们的解析器读不出来：%s", pererr.Error())
		} else {
			sink.say("⑤ 解析结果：benefitSurplusCount=%q benefitStatus=%q（缺键即空串）",
				liveProbeClip(counts, 24), liveProbeClip(status, 24))
		}
	}
	composeID, composeErr := run.Upstream.AppMailComposeID(ctx, session)
	if composeErr != nil {
		sink.say("⑦ getComposeId（生产路径）：%s", sink.mask(composeErr.Error()))
	} else {
		// ⚠ 草稿号本身不是凭据，但它**是** ⑦ 那一发的唯一读数（"这一口认不认这张会话"
		// 就看它给不给号）⇒ 只报长度，值不进日志（与票同一口径）。
		sink.say("⑦ getComposeId（生产路径）：拿到号，长度 %d（值不打印）", len(composeID))
	}
	composeRequest, perr := upstream.MailComposeIDRequest(session)
	if perr != nil {
		sink.say("⑦ 构造失败：%s", perr.Error())
	} else {
		composeResponse, serr := client.Send(ctx, composeRequest)
		sink.dump("⑦ getComposeId（直发，看体）", composeResponse)
		if serr != nil {
			sink.say("⑦ 直发错误：%s（URL 里有 sid ⇒ 已掩码）", sink.mask(serr.Error()))
		}
		idText, pererr := upstream.ParseMailComposeID(composeResponse)
		if pererr != nil {
			sink.say("⑦ 我们的解析器读不出来：%s", pererr.Error())
		} else {
			sink.say("⑦ 解析结果：拿到号，长度 %d（值不打印）", len(idText))
		}
	}

	// ── ⑧⑥ 资产侧那两口**写**的（玩法 1020 / 1014）：走生产代码，各发一次 ──
	//
	// 为什么这两口不发"直发档"（与 ③⑤⑦ 不同）：那三档发的是**读**，多发一次只是多敲一下；
	// ⑧ 请求的是"给这个号跑一次备份"、⑥ 请求的是"把这一份转成 PDF"，同一趟发两遍等于
	// 让上游做两件事。生产路径那一句判定已经含了状态码（非 2xx 交回错误、⑥ 还带 `code`
	// 与 `summary`）⇒ 一次就够拿到本族要的那格读数。
	//
	// ⑤⑦ 已经把"这张会话有效"钉住了（两台子域都回 `S_OK`）⇒ 这两口的失败**只能**归因到
	// 它们各自那条路由，而不是"票不行"。这正是这一档存在的理由：R35 剩下的那一半按主机收，
	// 而 ⑧ 与 ③ 同机（`mail.10086.cn`）—— 两句不一样就是"路径没了"，两句一样才需要
	// 去看整个入口。
	backupItem := liveProbeTaskItem(1020, "邮箱资产备份")
	backupFired, backupProblems := executor.runMailBackup(ctx, backupItem)
	sink.say("⑧ 走完整条玩法（1020）：发过枪=%v 失败 %d 条", backupFired, len(backupProblems))
	for _, problem := range backupProblems {
		sink.say("⑧ 原因：%s", sink.mask(problem))
	}
	// ⚠ ⑧ 的"成功"只到"上游没拒"：本层不解析体（零样本，见 `AppMailBackup` 头注），
	// 所以这里念不念得出一句人话全凭上一行有没有 problems。
	if len(backupProblems) == 0 {
		sink.say("⑧ 没有失败 ⇒ 这一发是 2xx；**体长什么样仍然零样本**（本层不解析，别把这一行读成\"备份做完了\"）")
	}

	// ⑥ 要先往网盘写一份真文件（**写**，用户口径"包括上传网盘文件"）⇒ 链尾自己清掉。
	// 生产链的清理在 [TaskMatrixRun.RunAccount] 末尾，探针只调 runner 不调整趟 ⇒
	// 自己把 `executor.cloudFileIDs` 删掉；删不掉时**把 id 念出来**（只报长度会让人觉得
	// "没留东西"，而那一格是要人手工去清的）。
	pdfItem := liveProbeTaskItem(1014, "PDF 转换")
	pdfFired, pdfProblems := executor.runMailPDF(ctx, pdfItem, &TaskStepRecord{TaskID: 1014})
	sink.say("⑥ 走完整条玩法（1014）：发过枪=%v 失败 %d 条", pdfFired, len(pdfProblems))
	for _, problem := range pdfProblems {
		sink.say("⑥ 原因：%s", sink.mask(problem))
	}
	uploader, uerr := executor.cloudUploader(ctx)
	if uerr != nil {
		sink.say("⑥ 的清理没做：拿不到上传器（%s）⇒ 传上去那一份要手工清", sink.mask(uerr.Error()))
	} else {
		for _, fileID := range executor.cloudFileIDs {
			sink.see(fileID) // 真实 fileId 不进成文（与预签名 URL 同一纪律）
		}
		sent, derr := uploader.DeleteCloudFiles(ctx, executor.cloudFileIDs)
		switch {
		case derr != nil:
			sink.say("⑥ 链尾清理失败（发过枪=%v）：%s ⇒ 网盘里可能留着 %d 份素材，要手工清",
				sent, sink.mask(derr.Error()), len(executor.cloudFileIDs))
		case sent:
			sink.say("⑥ 链尾清理：一次 batchTrash，覆盖这一趟记下的 %d 份素材（id 不打印）",
				len(executor.cloudFileIDs))
		default:
			sink.say("⑥ 链尾清理：没要删的（这一趟没有新的 fileId ⇒ 与\"⑥ 没做成\"两档，别并成一句）")
		}
	}
	// ⚠ 上面那一句**不**说"用户网盘恢复原状"：转换产物（⑥ 在邮箱/网盘那侧生成的那一份）
	// 不在这条清理链上，它存不存在、落在哪至今零样本（R36）。
	sink.say("⑥ 边界：转换产物**不在**上面那条清理链上（R36）⇒ 这一行不代表\"什么都没留下\"")

	// ── 链尾**回读**（只读一发 `hcy/file/list`）：清理那句"上游没拒"到底是不是真的 ──
	//
	// 为什么要有这一档：`batchTrash` 的判据只有顶层 `success`（协议层成文），
	// 而"删没删得掉"在**用户的网盘**里是另一件事。这一发把它变成读数：
	// 根目录里还有没有以 `asign-` 开头的名字（本仓临时文件的前缀，见 [CloudUploadNamePrefix]）。
	//
	// ⚠ 三条边界，免得这一行被读成它没说的事：
	//   - 只列**根目录一页**（`pageSize` 与 AI 头像那一族同档）⇒ 子目录里的看不到，
	//     多于这一页的也看不到（那一格登记过：R16 / Q11 不翻页）；
	//   - 回收站里的**不该**出现在这一页里 ⇒ "0 个"只到"清理链对得上下游"，
	//     不等于"网盘里从来没有过"；
	//   - 反过来"命中 N 个"也**不**自动等于缺陷：那可能是本仓之外留下的同名文件。
	//     所以这里只念**数量**，名字不念（名字里可能带用户内容）。
	page, lerr := run.Upstream.AppCloudFileList(ctx, opening.Profile,
		UserTimestamp(run.Clock), opening.Credential.Authorization(), "/", 100)
	if lerr != nil {
		sink.say("⑥ 链尾回读没做成：%s ⇒ \"删干净了\"这一格仍只有 batchTrash 那句上游原话",
			sink.mask(lerr.Error()))
	} else {
		leftovers := 0
		for _, file := range page.Items {
			if strings.HasPrefix(file.Name, CloudUploadNamePrefix) {
				leftovers++
			}
		}
		sink.say("⑥ 链尾回读：根目录这一页共 %d 项，其中前缀 %q 的 %d 个（回收站里的不算 ⇒ 见函数头那三条边界）",
			len(page.Items), CloudUploadNamePrefix, leftovers)
	}
}

// liveProbeTaskItem 造一条只有 id 与名字的条目：探针打的这两条玩法只读这两格
// （[taskExecutor.runMailBackup] / [runMailPDF] 用 `taskNameOf` 与 `derefTaskID`），
// 其余字段留空就是"上游没给"的形状 —— 那两族本来也不读别的键。
func liveProbeTaskItem(id int64, name string) upstream.TaskV2Item {
	taskID, taskName := id, name
	enable := int64(1)
	return upstream.TaskV2Item{
		TaskItem:   upstream.TaskItem{TaskID: &taskID, Name: &taskName},
		Enable:     &enable,
		MarketName: strPtr(upstream.MarketName),
	}
}
