package server

// 签到页那一族（`sign` / `receive` / `multiple` / `backup-gift` / `revival` /
// `prizes` / `cloud-snapshot` / `task-list` / `notice` / `msg-push` / `fun-ai`）
// 的**接线**用例。文件末尾另有一节 `ai-avatar`：它同属这份装配，但走专用外壳、
// 且是这一族里唯一往用户网盘写东西的一条，所以单独一张计数表（理由写在那一节开头）。
//
// 判据在 `internal/app`（`signin_claims_run_test.go` 那批），本文件只钉这一层独有的两件事：
//
//  1. **每条 code 真的跑到终局** —— 分派表里少一行不会编译报错，只会让界面上的
//     "开始"变回 501；而这一族的十一个 code 共用一份装配，装配写错则十一个一起坏。
//  2. **每个 code 打的是自己那条链的端点** —— ⭐ 这一条是 [buildClaimRun] 那条注释
//     立下的账：三条开关只差一个 [app.ClaimKind]，而它是 int，
//     分派表里配错档**不会有任何一处变红**，表现却是
//     "界面上点『备份好礼』，实际把翻倍的名额用掉了"（每月一次，退不回来）。
//
// ⚠ 与 `run_dispatch_test.go` 同一口径：这里判的是"分派出去的那一步"，
// 不判链跑对了没有。

import (
	"context"
	"net/http"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 这一族各条链的样本响应（凭据一律合成值；四态与文案不在这里判）。
const (
	// 签到状态：`signInPoints` / `signCount` / `total` / `monthDays` 是协议层硬要求的
	// 四个整数，少任一个会红在"解析失败"上而不是判据本身。
	signInStatusClaimable = `{"code":0,"msg":"success","result":{` +
		`"cal":[],"curMonthBackup":true,"curMonthBackupSignAccept":false,` +
		`"receiveList":[{"cloudType":0,"cloudNum":50,"recordId":"R1"}],` +
		`"signInPoints":3,"signCount":7,"total":10496,"monthDays":7}}`
	signInMultipleOK = `{"code":0,"msg":"success","result":{"cloudCount":100,"multiple":2}}`
	backupGiftInfo   = `{"code":0,"msg":"success",` +
		`"result":{"state":0,"curMonth":true}}`
	backupGiftReceive = `{"code":0,"msg":"success","result":88}`
	revivalRewardBody = `{"code":0,"msg":"success","result":{"cardType":1,` +
		`"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}`
	prizeRecordBody = `{"code":0,"msg":"success","result":{"records":[` +
		`{"prizeName":"一个月会员","flag":1,"oid":1270932714}]}}`
	receiveV3Body = `{"code":0,"msg":"success","result":{"receive":50}}`

	// ── 新链路批那四条（2026-09-23）：每条一份"判据成立"的响应 ──────────────────
	//
	// ⚠ 这些**不是**实拍样本，是为"分派走没走对那条链"造的合成读数 ——
	// 各档判定与真实响应形状在 `internal/app` 与 `internal/upstream` 各自的测试里钉，
	// 那里才是口径的出处。这里只要求"这一族自己的端点被打到了"。
	taskListEmptyBody = `{"code":0,"msg":"success","result":[]}`
	// 通知任务：`cloudStatus` / `taskStatus` 的两个读数是**猜的对应关系**
	// （`internal/app/notice_run.go` 里那两个常量旁边写清了怎么定它）。
	noticeClaimableBody = `{"code":0,"msg":"success","result":{` +
		`"taskOneInfo":{"cloudStatus":1,"sum":200,"taskStatus":3},` +
		`"taskTwoInfo":{"cloudStatus":2,"sum":100,"taskStatus":3}}}`
	noticeClaimedBody = `{"code":0,"msg":"success","result":{` +
		`"taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},` +
		`"taskTwoInfo":{"cloudStatus":2,"sum":100,"taskStatus":3}}}`
	// `reward` 的实拍响应**没有 `result`**（所以这一族报不出"领到多少朵"）。
	noticeRewardBody = `{"code":0,"msg":"success"}`
	// ⚠ 键名 `onDuaration` 是上游自己的拼写错误，**不许改对**（`app_msg_push.go`）。
	msgPushReadyBody = `{"code":0,"msg":"success","result":{` +
		`"pushOn":1,"firstTaskStatus":3,"secondTaskStatus":2,"onDuaration":41}}`
	msgPushObtainBody = `{"code":0,"msg":"success","result":{"obtainCode":0}}`
	// 活动期取一个"必然夹住 harness 那个固定时钟"的宽窗：
	// 判定用哪几个计数、以及窗口外一律不发写请求，是 `funai_run_test.go` 的账。
	funaiModuleBody = `{"code":0,"msg":"success","result":{` +
		`"moduleId":"1-250730","drawTimes":1,"drawsUsed":0,"todayGetLotteryCount":1,` +
		`"startTime":"2000-01-01T00:00:00","endTime":"2099-12-31T23:59:59"}}`
	funaiLotteryBody = `{"code":0,"msg":"success","result":{` +
		`"prizeName":"合成奖品","prizeId":260107009,"isWin":1,"moduleId":"1-250730"}}`
)

// signInSessionRoutes 是建会话那三枪（与 live_room_probe_test.go 那一份同源）。
func signInSessionRoutes() []probeRoute {
	return []probeRoute{
		{match: "/user/querySpecTokenV2", bodies: []string{probeSSOBody}},
		{match: "/ycloud/api/timestamp", bodies: []string{probeH5TimestampBody}},
		{match: "/ycloud/auth-service/auth/tyrzLogin", bodies: []string{probeH5LoginOk}},
	}
}

// wiredSignInCodes 是"接上了执行、且第一枪就会失败"的那十一个。
//
// 名单**不**含 `live-room` 与 `auth-refresh`：它们各自有端到端用例，
// 而 `live-room` 会去开 WS（本文件用的是"拒绝联网"的默认客户端）。
var wiredSignInCodes = []string{
	"sign", "receive", "multiple", "backup-gift", "revival", "prizes", "cloud-snapshot",
	"task-list", "notice", "msg-push", "fun-ai",
}

// TestSignInFamilyIsAcceptedAndReachesTerminal：十一个 code 都能受理并跑到终局。
//
// 这里**刻意**用默认的 [refusingClient]：要证的不是链跑对了，而是
// "点开始之后那一条流会结束"。少这一条的话，接上一条能力但忘在分派表加一行，
// 界面上的表现是"进度条停在那儿"（与"还在跑"一模一样）。
func TestSignInFamilyIsAcceptedAndReachesTerminal(t *testing.T) {
	for _, code := range wiredSignInCodes {
		t.Run(code, func(t *testing.T) {
			h := newHarness(t).initialized(t)
			seedLoginAccount(t, h, fullPhone)

			rec := h.do(t, "POST", "/api/runs",
				`{"capability":`+quoteJSON(code)+`,"params":{}}`, nil)
			if rec.Code != http.StatusAccepted {
				t.Fatalf("%s 应受理，实得 %d：%s", code, rec.Code, brief(rec.Body.String()))
			}
			rows := runsPairing(t, rec)
			if len(rows) != 1 {
				t.Fatalf("应给出一个账号的配对，实得 %v", rows)
			}
			events := waitForRunFinished(t, h, firstRunID(t, rows))
			// 失败的那一句要能在事件里看见：这条链的错误原文一路传到历史列表，
			// 否则用户只看到一个红色标记而不知道为什么。
			if !anyMessageContains(events, "拒绝联网") {
				t.Errorf("事件流里没有上游失败的原因：%v", messagesOf(events))
			}
			assertRunCapability(t, h, rows, code)
		})
	}
}

// TestSignInFamilyDispatchHitsItsOwnEndpoint：⭐ 每个 code → 它自己那条链的端点。
//
// 三条开关共用 [buildClaimRun]，只差一个档位参数；这里给每一档配好它**该**打到的
// 端点，再断言"该打的打了一次、另外两档的端点一次都没打"。
// 把分派表里的 `ClaimKindBackupGift` 换成 `ClaimKindSignInMultiple`，本条立刻红
// （实测红成"应把 backupgift/receive 打 1 次，实得 0 次"。⚠ 做这一步时**必须**
// 断言替换真的落到文件上了 —— 第一次跑成"全绿"就是因为 gofmt 对齐过冒号后的空格，
// 替换是空操作，见坑表 #143）。
func TestSignInFamilyDispatchHitsItsOwnEndpoint(t *testing.T) {
	cases := []struct {
		code   string
		routes []probeRoute
		want   []string // 各打一次
		forbid []string // 一次都不该打
	}{
		{
			code: "multiple",
			routes: []probeRoute{
				{match: "/ycloud/signin/page/infoV3", bodies: []string{signInStatusClaimable}},
				{match: "/ycloud/signin/page/multiple", bodies: []string{signInMultipleOK}},
			},
			want:   []string{"/ycloud/signin/page/multiple"},
			forbid: []string{"backupgift/receive", "receiveRevivalReward", "receiveV3"},
		},
		{
			code: "backup-gift",
			routes: []probeRoute{
				{match: "backupgift/info", bodies: []string{backupGiftInfo}},
				{match: "backupgift/receive", bodies: []string{backupGiftReceive}},
			},
			want:   []string{"backupgift/receive"},
			forbid: []string{"/ycloud/signin/page/multiple", "receiveRevivalReward"},
		},
		{
			code: "revival",
			routes: []probeRoute{
				{match: "receiveRevivalReward", bodies: []string{revivalRewardBody}},
			},
			want: []string{"receiveRevivalReward"},
			// 复活卡**没有**门槛查询：多打一次 `infoV3` 说明这一档被接成了别的档。
			forbid: []string{"/ycloud/signin/page/multiple", "backupgift/receive", "infoV3"},
		},
		{
			code: "receive",
			routes: []probeRoute{
				{match: "/ycloud/signin/page/infoV3", bodies: []string{signInStatusClaimable}},
				{match: "/ycloud/signin/page/receiveV3", bodies: []string{receiveV3Body}},
			},
			want: []string{"/ycloud/signin/page/receiveV3"},
			// 只领取的链**不签**到，也不碰三条开关。
			forbid: []string{"startSignIn", "/ycloud/signin/page/multiple", "receiveRevivalReward"},
		},
		{
			code: "prizes",
			routes: []probeRoute{
				{match: "/ycloud/signin/public/record", bodies: []string{prizeRecordBody}},
			},
			want:   []string{"/ycloud/signin/public/record"},
			forbid: []string{"receiveV3", "/ycloud/signin/page/multiple", "startSignIn"},
		},
		{
			code: "cloud-snapshot",
			routes: []probeRoute{
				{match: "/ycloud/signin/page/infoV3", bodies: []string{signInStatusClaimable}},
			},
			want:   []string{"/ycloud/signin/page/infoV3"},
			forbid: []string{"startSignIn", "receiveV3", "public/record"},
		},
	}
	for _, c := range cases {
		t.Run(c.code, func(t *testing.T) {
			h := newHarness(t).initialized(t)
			// 凭据齐全的 App 账号（`seedProbeAccount` 那一份）：这一族都要建会话。
			seedProbeAccount(t, h)
			client := newProbeScript(
				append(signInSessionRoutes(), c.routes...)...)
			h.replaceRunWorld(t, client)

			rec := h.do(t, "POST", "/api/runs",
				`{"capability":`+quoteJSON(c.code)+`,"params":{}}`, nil)
			if rec.Code != http.StatusAccepted {
				t.Fatalf("%s 应受理，实得 %d：%s",
					c.code, rec.Code, brief(rec.Body.String()))
			}
			rows := runsPairing(t, rec)
			waitRuns(t, h, rows)

			for _, endpoint := range c.want {
				if got := client.count(endpoint); got != 1 {
					t.Errorf("%s 应把 %s 打 1 次，实得 %d 次（全部 URL：%v）",
						c.code, endpoint, got, urlsOf(client))
				}
			}
			for _, forbidden := range c.forbid {
				if got := client.count(forbidden); got != 0 {
					t.Errorf("%s 不该打 %s，实得 %d 次 —— 分派把这一档接成了别的能力",
						c.code, forbidden, got)
				}
			}
			assertRunCapability(t, h, rows, c.code)
		})
	}
}

// TestNewChainDispatchHitsItsOwnEndpoint：2026-09-23 那四条**各打自己那条链**。
//
// 与上面那组同一口径：判"分派出去的那一步"，不判链内判定（判定在
// `notice_run_test.go` / `msg_push_run_test.go` / `funai_run_test.go` /
// `task_list_run_test.go`）。分成两条用例而不并进同一张表，是因为这四条的
// **次数**才是读数：`notice` 是"查 → 领 → 复查"三枪，`fun-ai` 是"读判据 → 最多抽一次"，
// 只断言"打到过"的话，把 `notice` 接成"查完就不领"也不会红。
//
// ⚠ 四条的 `forbid` 里都列了**另外三条**的第一枪端点：分派表用常量当键，
// 但键与 builder 配错对（`CodeNotice` 配上 `buildMsgPushRun`）不会编译报错，
// 表现是"界面上点『开启通知』，实际把消息推送那条打了"。
func TestNewChainDispatchHitsItsOwnEndpoint(t *testing.T) {
	// 计数写成 {端点, 次数} 而不是"打过就算"：见上面那条注释。
	type endpointCount struct {
		endpoint string
		times    int
	}
	cases := []struct {
		code   string
		routes []probeRoute
		want   []endpointCount
		forbid []string
	}{
		{
			code: "task-list",
			routes: []probeRoute{
				{match: "/ycloud/signin/task/taskListV3", bodies: []string{taskListEmptyBody}},
			},
			want: []endpointCount{{"/ycloud/signin/task/taskListV3", 1}},
			// 只查不做：`task/click` 与 `receiveTask` 是"做任务 / 领任务奖"那两口，一次都不该打。
			forbid: []string{
				"signin/task/click", "receiveTask", "openEmailsms", "msgPushOn",
				"funai", "startSignIn", "receiveV3",
			},
		},
		{
			code: "notice",
			routes: []probeRoute{
				// 第一枪"有一槽可领"，第二枪（复查）"两槽都已领"。
				{match: "openEmailsms/getTaskInfo", bodies: []string{noticeClaimableBody, noticeClaimedBody}},
				{match: "openEmailsms/reward", bodies: []string{noticeRewardBody}},
			},
			want: []endpointCount{
				{"openEmailsms/getTaskInfo", 2}, {"openEmailsms/reward", 1},
			},
			// `receiveTask` 与 `reportAppNoticeStatus` 在领域层**刻意没接**（理由见 `app_notice.go`）。
			forbid: []string{
				"receiveTask", "reportAppNoticeStatus", "msgPushOn",
				"/ycloud/signin/task/taskListV3", "funai", "startSignIn",
			},
		},
		{
			code: "msg-push",
			routes: []probeRoute{
				{match: "/market/msgPushOn/task/status", bodies: []string{msgPushReadyBody}},
				{match: "/market/msgPushOn/task/obtain", bodies: []string{msgPushObtainBody}},
			},
			// 这一族**刻意不复查**（零样本，见 `msg_push_run.go` 头注）⇒ status 只有一枪。
			want: []endpointCount{
				{"/market/msgPushOn/task/status", 1}, {"/market/msgPushOn/task/obtain", 1},
			},
			forbid: []string{
				"openEmailsms", "/ycloud/signin/task/taskListV3", "funai",
				"startSignIn", "receiveV3",
			},
		},
		{
			code: "fun-ai",
			routes: []probeRoute{
				{match: "/ycloud/funai/api/page/getmodule", bodies: []string{funaiModuleBody}},
				{match: "/ycloud/funai/api/page/lottery", bodies: []string{funaiLotteryBody}},
			},
			want: []endpointCount{
				{"/ycloud/funai/api/page/getmodule", 1}, {"/ycloud/funai/api/page/lottery", 1},
			},
			forbid: []string{
				"openEmailsms", "msgPushOn", "/ycloud/signin/task/taskListV3",
				"startSignIn", "receiveV3",
			},
		},
	}
	for _, c := range cases {
		t.Run(c.code, func(t *testing.T) {
			h := newHarness(t).initialized(t)
			seedProbeAccount(t, h)
			client := newProbeScript(append(signInSessionRoutes(), c.routes...)...)
			h.replaceRunWorld(t, client)

			rec := h.do(t, "POST", "/api/runs",
				`{"capability":`+quoteJSON(c.code)+`,"params":{}}`, nil)
			if rec.Code != http.StatusAccepted {
				t.Fatalf("%s 应受理，实得 %d：%s",
					c.code, rec.Code, brief(rec.Body.String()))
			}
			rows := runsPairing(t, rec)
			waitRuns(t, h, rows)

			for _, want := range c.want {
				if got := client.count(want.endpoint); got != want.times {
					t.Errorf("%s 应把 %s 打 %d 次，实得 %d 次（全部 URL：%v）",
						c.code, want.endpoint, want.times, got, urlsOf(client))
				}
			}
			for _, forbidden := range c.forbid {
				if got := client.count(forbidden); got != 0 {
					t.Errorf("%s 不该打 %s，实得 %d 次 —— 分派把这一条接成了别的能力",
						c.code, forbidden, got)
				}
			}
			assertRunCapability(t, h, rows, c.code)
		})
	}
}

// TestCloudSnapshotRunActuallyWritesTheDailyRow：这条能力的**目标**是落库那一行。
//
// 上一组用例只看到"它打了 infoV3"。这里补上另一头：趋势图上那一格真的有了，
// 且取的数是 `infoV3.total`（与签到链同一个数，所以两条链同一天各跑一次不打架）。
func TestCloudSnapshotRunActuallyWritesTheDailyRow(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	client := newProbeScript(append(signInSessionRoutes(),
		probeRoute{match: "/ycloud/signin/page/infoV3", bodies: []string{signInStatusClaimable}})...)
	h.replaceRunWorld(t, client)

	rec := h.do(t, "POST", "/api/runs", `{"capability":"cloud-snapshot","params":{}}`, nil)
	rows := runsPairing(t, rec)
	waitRuns(t, h, rows)

	snapshots, err := h.store.CloudStatsSince(context.Background(),
		core.ScopeUser(userIDOf(t, h)), core.NewAccountID(id), "0000-00-00")
	if err != nil {
		t.Fatalf("读日快照失败: %v", err)
	}
	if len(snapshots) != 1 {
		t.Fatalf("应写进一条日快照，实得 %+v", snapshots)
	}
	if snapshots[0].CloudCount != 10496 {
		t.Errorf("应记 `infoV3.total`，实得 %d", snapshots[0].CloudCount)
	}
}

// TestSignInFamilySharesOneSessionPerAccount：界面上连着点两次 ⇒ 只建一条会话。
//
// 这是票池接线那一层的判据（口径来自用户 2026-09-23），而它**只有**在服务端这一层
// 才成立：领域层的池在 `signin_session_pool_test.go` 测过，但"分派确实把那份池
// 传下去了"只在 [Server.signInWorld] 那一处 —— 漏接不报错，只多打两枪。
//
// ⚠ 两个**不同**的 code 各跑一次（而不是同一 code 两次）：它们同属签到页那一张票，
// 但走的是两个 builder —— 每个 builder 各自漏接池、或各自建一条，都会在这里露出来。
// 两条都是只读，所以本条不掺入写请求的响应编排。
func TestSignInFamilySharesOneSessionPerAccount(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedProbeAccount(t, h)
	client := newProbeScript(append(signInSessionRoutes(),
		probeRoute{match: "/ycloud/signin/page/infoV3", bodies: []string{signInStatusClaimable}},
		probeRoute{match: "/ycloud/signin/public/record", bodies: []string{prizeRecordBody}})...)
	h.replaceRunWorld(t, client)

	for _, code := range []string{"prizes", "cloud-snapshot"} {
		rec := h.do(t, "POST", "/api/runs",
			`{"capability":`+quoteJSON(code)+`,"params":{}}`, nil)
		if rec.Code != http.StatusAccepted {
			t.Fatalf("%s 应受理，实得 %d：%s", code, rec.Code, brief(rec.Body.String()))
		}
		waitRuns(t, h, runsPairing(t, rec))
	}
	if got := client.count("/ycloud/auth-service/auth/tyrzLogin"); got != 1 {
		t.Errorf("两趟共用一张票，tyrzLogin 应打 1 次，实得 %d 次（URL：%v）",
			got, urlsOf(client))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// AI 头像（`ai-avatar`）：这一条**不在** [wiredSignInCodes] 里
// ─────────────────────────────────────────────────────────────────────────────
//
// 它走 [buildAiAvatarRun] 那个专用外壳（多塞一份 `Waiter` 注入点），而且它是这一族
// 里唯一**会往用户网盘写文件**的一条 ⇒ 单独一条用例、单独一张计数表：
// 混进上面那两张表的话，"写请求"这件事就没有任何一处成文在盯着它了。
//
// 这里钉的是**接线**：28 枪的分布（五轮各 merge→查→下载→写盘、一次判据、一枪抽奖、
// 三枪找回）与"别的族的端点一枪没打"。链内判定（节奏时长、抽奖三态、找回失败不判红）
// 在 `internal/app/ai_avatar_run_test.go`。
//
// ⚠ 这些响应**不是**实拍样本：`rabactive` 那六口零实拍（见
// `internal/upstream/app_ai_avatar.go` 文件头），全部形状是照着"要让链走完"编的。

// aiAvatarScriptRoutes 是"五轮全部一次合成成功 + 抽一次 + 找回"的响应表。
//
// 路由**按先后**匹配，所以每一条都用完整路径当 match，避免 `/image/draw`
// 之类的短串把别的端点也吞掉。队列用尽后重复最后一条 ⇒ 五轮共用同一份 merge/status，
// 本用例判的是**次数与分布**，不是每轮内容。
func aiAvatarScriptRoutes() []probeRoute {
	return []probeRoute{
		{match: "/market/rabactive/ai/image/merge",
			bodies: []string{`{"code":0,"msg":"success","result":{"taskId":"TASK-1"}}`}},
		{match: "/market/rabactive/ai/image/getStatus",
			bodies: []string{`{"code":0,"msg":"success",` +
				`"result":{"status":3,"imageUrl":"https://img.example.com/merged.jpg"}}`}},
		// 合成图那一枪是**裸 GET、不带任何头**（`aiAvatarDownloadImage`），
		// 所以它也会经过这份脚本；没有这条路由就是 404 ⇒ "下载失败" ⇒ 五轮全废。
		{match: "img.example.com", bodies: []string{"SYNTH-BINARY-BYTES"}},
		{match: "/market/rabactive/image/upload",
			bodies: []string{`{"code":0,"msg":"success","result":"UP-1"}`}},
		{match: "/market/rabactive/image/getHashLeft",
			bodies: []string{`{"code":0,"msg":"success","result":1}`}},
		{match: "/market/rabactive/image/draw",
			bodies: []string{`{"code":0,"msg":"success",` +
				`"result":{"contentName":"合成奖品"}}`}},
		{match: "/market/rabactive/image/catalogFullPath",
			bodies: []string{`{"code":0,"msg":"success",` +
				`"result":{"fullPath_sub":"/a/b/FOLDER-ID"}}`}},
		{match: "/market/rabactive/image/getWorkList",
			bodies: []string{`{"code":0,"msg":"success","result":[` +
				`{"id":"UP-1","imgUrl":"https://img.example.com/origin-0.jpg",` +
				`"qrImgUrl":"https://img.example.com/merged-0.jpg"}]}`}},
		// 网盘那一口是**另一套信封**（`code` 是字符串、载荷在 `data.items`）。
		{match: "hcy/file/list",
			bodies: []string{`{"code":"0000","success":true,"message":"请求成功",` +
				`"data":{"items":[{"fileId":"F-0","name":"origin-0.jpg"}]}}`}},
	}
}

// seedBoundDevice 给账号绑一份设备特征 + 一个 `deviceId`，让这一趟**不必**为取 id 出网。
//
// 为什么必须绑：这条链对 `deviceId` 的处置是"换不到就整趟失败、一枪都不发"
// （成文见 `ai_avatar_run.go` 的 `RunAccount`），而库里没有绑定时它会去打
// smid 上报那一枪（本用例没给它编响应 ⇒ 404 ⇒ 整条链停在取 id 上，
// 而红成的样子是"merge 应打 5 次实得 0 次"，指错方向）。
// ⚠ 写特征与写 id 的**顺序**不能反：`SetSmidFingerprint` 会顺手清空 id（换长相 = 换设备）。
func seedBoundDevice(t *testing.T, h *harness, id string) {
	t.Helper()
	fingerprint := upstream.GenerateSmFingerprint(newRandom())
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		t.Fatalf("造不出合成设备特征：%s", perr.Message())
	}
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))
	account := core.NewAccountID(id)
	if err := h.store.SetSmidFingerprint(ctx, scope, account, text); err != nil {
		t.Fatalf("绑定设备特征失败: %v", err)
	}
	if err := h.store.SetSmidDeviceID(ctx, scope,
		account, "BSYNTH-DEVICE-ID-00000000000001"); err != nil {
		t.Fatalf("写入 deviceId 失败: %v", err)
	}
}

func TestAiAvatarDispatchHitsItsOwnChain(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	seedBoundDevice(t, h, id)
	client := newProbeScript(
		append(signInSessionRoutes(), aiAvatarScriptRoutes()...)...)
	h.replaceRunWorld(t, client)
	// 把那份注入点换成记录型的：这条链是三处 sleep 的**唯一**消费者，而
	// [buildAiAvatarRun] 传错 waiter（写成 `app.RealWaiter{}`）的表现**不是**变红而是变慢
	// —— 那条路走的是同一个 [Waiter] 接口，只有"等没等、等了多久"能区分。
	waits := &recordingWaits{}
	h.server.runs.waiter = waits

	rec := h.do(t, "POST", "/api/runs", `{"capability":"ai-avatar","params":{}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("AI 头像应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	events := waitForRunFinished(t, h, firstRunID(t, rows))

	// 次数分布：五轮生成（每轮 merge→查→下载→写盘各一次）+ 一次判据 + 一枪抽奖 + 三枪找回。
	for _, want := range []struct {
		endpoint string
		times    int
	}{
		{"/market/rabactive/ai/image/merge", 5},
		{"/market/rabactive/ai/image/getStatus", 5},
		{"img.example.com", 5},
		{"/market/rabactive/image/upload", 5},
		{"/market/rabactive/image/getHashLeft", 1},
		{"/market/rabactive/image/draw", 1},
		{"/market/rabactive/image/catalogFullPath", 1},
		{"/market/rabactive/image/getWorkList", 1},
		{"hcy/file/list", 1},
	} {
		if got := client.count(want.endpoint); got != want.times {
			t.Errorf("%s 应打 %d 次，实得 %d 次（全部 URL：%v）",
				want.endpoint, want.times, got, urlsOf(client))
		}
	}
	// ⭐ 28 枪共用**一张**票：这条链是全部能力里单趟请求数最多的一条，
	// 漏接那份票池在别的链上只是浪费三枪，在这里是每轮都可能重打一次。
	if got := client.count("/ycloud/auth-service/auth/tyrzLogin"); got != 1 {
		t.Errorf("一趟 AI 头像应只建一条会话，tyrzLogin 实得 %d 次（URL：%v）",
			got, urlsOf(client))
	}
	// 别的族的端点一枪都不该打：分派表的键与 builder 配错对不会编译报错。
	for _, forbidden := range []string{
		"startSignIn", "/ycloud/signin/page/multiple", "receiveV3",
		"backupgift", "receiveRevivalReward", "/ycloud/signin/public/record",
		"openEmailsms", "msgPushOn", "/ycloud/signin/task/taskListV3",
		"/ycloud/funai/", "exchangeV3", "getSlide",
	} {
		if got := client.count(forbidden); got != 0 {
			t.Errorf("ai-avatar 不该打 %s，实得 %d 次 —— 分派把这一条接成了别的能力",
				forbidden, got)
		}
	}
	// ⭐ 节奏真的走了那份注入点：五轮"每次查询前先等 3 秒" + 轮间 2 秒 ×4（最后一轮不等）
	// = 9 次。只判"没真睡"的话，把 `Wait` 全去掉也不会红。
	if got := waits.count(3 * time.Second); got != 5 {
		t.Errorf("该等 5 次 3 秒（先睡再查），实得 %d 次（全部：%v）", got, waits.all())
	}
	if got := waits.count(2 * time.Second); got != 4 {
		t.Errorf("该等 4 次 2 秒（轮间，最后一轮不等），实得 %d 次（全部：%v）", got, waits.all())
	}
	if got := waits.count(time.Second); got != 0 {
		t.Errorf("只抽一次 ⇒ 抽奖那一次等待不该发生（最后一次不等），实得 %d 次", got)
	}
	if got := len(waits.all()); got != 9 {
		t.Errorf("该等 9 次，实得 %d 次（全部：%v）", got, waits.all())
	}
	// 过程话里那句小结是这条链**跑完了**的证据（五轮全成才会是 "5/5"）：
	// 只判计数的话，"五轮全失败但每轮打了四枪"也算过。
	if !anyMessageContains(events, "AI头像生成 5/5") {
		t.Errorf("事件流里没有那条 5/5 的小结：%v", messagesOf(events))
	}
	assertRunCapability(t, h, rows, "ai-avatar")
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// recordingWaits 是"不真等、只记账"的那份 [app.Waiter]。
//
// 后台那一趟在自己的 goroutine 里等，所以读写都上锁（与 [probeScript] 同一理由）。
type recordingWaits struct {
	mu        sync.Mutex
	durations []time.Duration
}

func (w *recordingWaits) Wait(_ context.Context, d time.Duration) *core.Error {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.durations = append(w.durations, d)
	return nil
}

func (w *recordingWaits) count(d time.Duration) int {
	w.mu.Lock()
	defer w.mu.Unlock()
	n := 0
	for _, got := range w.durations {
		if got == d {
			n++
		}
	}
	return n
}

func (w *recordingWaits) all() []time.Duration {
	w.mu.Lock()
	defer w.mu.Unlock()
	return slices.Clone(w.durations)
}

func firstRunID(t *testing.T, rows []map[string]any) core.RunID {
	t.Helper()
	id, ok := core.ParseRunID(asText(t, rows[0]["runId"]))
	if !ok {
		t.Fatalf("响应里的 runId 不是合法形状：%v", rows[0])
	}
	return id
}

func anyMessageContains(events []core.Sequenced, substr string) bool {
	for _, item := range events {
		if strings.Contains(item.Event.Message, substr) {
			return true
		}
	}
	return false
}

// urlsOf 按发出顺序列出请求 URL（失败时打出来：分派错了链，一眼就能看出来）。
//
// [probeScript] 那边只有 `count`，所以这里补一个同锁的读数口 —— 与它原来那两个方法
// 一样走 mutex，避免与后台那一趟的写入打架。
func urlsOf(client *probeScript) []string {
	client.mu.Lock()
	defer client.mu.Unlock()
	out := make([]string, 0, len(client.requests))
	for _, request := range client.requests {
		out = append(out, request.URL)
	}
	return out
}

func messagesOf(events []core.Sequenced) []string {
	out := make([]string, 0, len(events))
	for _, item := range events {
		out = append(out, item.Event.Message)
	}
	return out
}
