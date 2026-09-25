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
		return fmt.Sprintf("%s=%s", key, strconv.Quote(fmt.Sprint(typed)))
	}
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
	run := TaskMatrixRun{SignInWorld: SignInWorld{
		Store:    store,
		Scope:    core.ScopeLocal(),
		Upstream: upstream.NewUpstream(client),
		Device: upstream.NewIPadAppDevice(
			deviceID, NewAppDeviceShort(mcloudio.SystemRandom{})),
		Clock:    mcloudio.SystemClock{},
		Random:   mcloudio.SystemRandom{},
		Sessions: NewSessionPool(),
		Progress: sink.progress,
	}}
	return run, account, sink, client
}

// TestLiveProbeMail139Family 让 139 邮箱那一族的四枪各出一次真实读数。
//
// 逐枪要看的是这三格（都在 `docs/pending-decisions.md` 里挂着）：
//   - ② 的真实响应**到底有没有** `var.sid` / `var.rmkey`（形状零样本 ⇒ 我们的解析器
//     是按旧 TS 的文字形状写的）；
//   - ③ 到底要不要 `authorization`（带与不带各发一次，比对状态码；这一口零实拍）；
//   - ④ `taskClickV2` 对 472 认不认（回 `code` 就是上游的原话）。
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
		// ⑤ 对照枪（**只读**）：同一台 `:7071` 入口上的另一条路由。
		// 为什么要它：④ 回的是一个 **JSON 形状**的 404（`{code:"404", msg:"访问资源不存在"}`），
		// 而 ③ 回的是 HTML 404 页 —— 两句不一样，说明 ④ 那一枪**到了应用**。
		// 但"到了应用"仍有两种可能：整个 `:7071/ycloud/` 入口都不认，或只有
		// `signin/task/taskClickV2` 这一条路由没了。这一枪专门把两者分开：
		// 它 200 ⇒ 入口活着，挂的是那一条路由；它也 404 ⇒ 更可能是整片入口的问题。
		controlResponse, _ := client.Send(ctx, upstream.NoticeTaskInfoRequest(s.jwt, s.referer))
		sink.dump("⑤ 对照（同入口另一条只读路由 getTaskInfo）", controlResponse)
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
}
