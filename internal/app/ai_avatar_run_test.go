package app

// AI 头像那条链的编排用例：生成五轮 → 抽完当天次数 → 找回写进网盘的文件。
//
// # 这一族钉的是"发没发、按什么顺序发、等了多久"，不钉语义
//
// `rabactive` 那六口**零实拍**（见 `internal/upstream/app_ai_avatar.go` 文件头），
// 所以这里**不能**断言"上游会接受这个响应"—— 全部响应都是我们自己编的形状。
// 能钉的是那些**不依赖上游**的判据：
//
//   - 节奏（先睡再查、轮间 2 秒、抽奖间隔 1 秒、最后一次不等）—— 三处 sleep 是旧实现
//     跑通过的那一份，改一个字就等于换一次接口，所以它必须是**可断言**的 ⇒ 走 [Waiter] 注入点；
//   - 重试形状（一轮最多两次、一次合成最多查八次）；
//   - 抽奖的三态分叉（抽完 / 被 408 叫停 / 某一枪没发出去）—— 旧实现 `break` 与
//     `continue` 的那个分叉，走错了会对着一个已拒绝的账号连打五枪；
//   - 找回那一族**失败不判红**（生成与抽奖都办完了，把整趟判失败会诱导用户重跑 ⇒ 再写五张图）；
//   - 换不到 `deviceId` ⇒ **一枪都不发**（那一族每枪都带 `deviceid` 头）。
//
// ⚠ 刻意**不**钉的：状态档位 3/4 的真实含义、408 是什么、`.webp` 会不会出现。
// 那几格登记在实现文件头与 `docs/pending-decisions.md`，等实拍再钉。

import (
	"context"
	"encoding/json"
	"errors"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 这一族自己的端点子串。写出来是因为断言**两头**都要用：
// "该打的打了几次"与"不该打的一次都没打"。
const (
	aiAvatarMergeEndpoint    = "/market/rabactive/ai/image/merge"
	aiAvatarStatusEndpoint   = "/ai/image/getStatus"
	aiAvatarUploadEndpoint   = "/market/rabactive/image/upload"
	aiAvatarHashLeftEndpoint = "/image/getHashLeft"
	aiAvatarDrawEndpoint     = "/image/draw"
	aiAvatarWorkListEndpoint = "/image/getWorkList"
	aiAvatarCatalogEndpoint  = "/image/catalogFullPath"
	aiAvatarFileListEndpoint = "hcy/file/list"
	// aiAvatarImageHost 是响应里那个 `imageUrl` 的主机 —— 生成链会**真的去下载**它
	// （裸 GET、不带任何头），所以脚本里必须有这一条路由，否则表现为 404 → "下载失败"。
	aiAvatarImageHost = "img.example.com"
)

const aiAvatarTestAccount = "19****01"

// ─────────────────────────────────────────────────────────────────────────────
// 响应夹具
// ─────────────────────────────────────────────────────────────────────────────

func aiAvatarMergeOK(taskID string) string {
	return `{"code":0,"msg":"success","result":{"taskId":"` + taskID + `"}}`
}

// aiAvatarStatusBody 造一次状态查询的响应。`imageURL` 传空串 ⇒ **不给** `imageUrl` 那一格
// （那是"合成成功却没回地址"那一档，实现里单独有一句人话）。
func aiAvatarStatusBody(status int64, imageURL string) string {
	if imageURL == "" {
		return `{"code":0,"msg":"success","result":{"status":` + itoa(status) + `}}`
	}
	return `{"code":0,"msg":"success","result":{"status":` + itoa(status) +
		`,"imageUrl":"https://` + aiAvatarImageHost + "/" + imageURL + `"}}`
}

const aiAvatarStatusPending = `{"code":0,"msg":"success","result":{"status":2}}`
const aiAvatarStatusFailed = `{"code":0,"msg":"success","result":{"status":4}}`

func aiAvatarUploadOK(id string) string {
	return `{"code":0,"msg":"success","result":"` + id + `"}`
}

func aiAvatarHashLeftOK(left int64) string {
	return `{"code":0,"msg":"success","result":` + itoa(left) + `}`
}

// aiAvatarHashLeftMissing 是"上游回了成功但 `result` 是 null"那一档 ——
// 与"这一格没给"在协议层是同一个 nil（[upstream.ParseAiAvatarHashLeft]）。
const aiAvatarHashLeftMissing = `{"code":0,"msg":"success","result":null}`

const aiAvatarHashLeftNotNumber = `{"code":0,"msg":"success","result":"三次"}`

const aiAvatarRejected = `{"code":9999,"msg":"失败"}`
const aiAvatarDrawStoppedBody = `{"code":408,"msg":"请求次数超过限制"}`

func aiAvatarDrawOK(contentName string) string {
	return `{"code":0,"msg":"success","result":{"contentName":"` + contentName + `"}}`
}

func aiAvatarDrawOKByPrizeName(prizeName string) string {
	return `{"code":0,"msg":"success","result":{"prizeName":"` + prizeName + `"}}`
}

// aiAvatarCatalogPlain 的 `fullPath_sub` 末段就是要当 `parentFileId` 用的那一串
// （旧实现 `split("/").filter(Boolean).pop()`）。
func aiAvatarCatalogPlain(fileID string) string {
	return `{"code":0,"msg":"success","result":{"fullPath_sub":"/a/b/` + fileID + `"}}`
}

const aiAvatarCatalogNoPath = `{"code":0,"msg":"success","result":{"fullPath_sub":""}}`

// aiAvatarWorkListOK 列作品：两格图片 URL 的文件名要与网盘列表里的 `name` 对得上。
func aiAvatarWorkListOK(ids ...string) string {
	var parts []string
	for i, id := range ids {
		parts = append(parts, `{"id":"`+id+`","imgUrl":"https://`+aiAvatarImageHost+
			`/origin-`+itoa(int64(i))+`.jpg","qrImgUrl":"https://`+aiAvatarImageHost+
			`/merged-`+itoa(int64(i))+`.jpg"}`)
	}
	return `{"code":0,"msg":"success","result":[` + strings.Join(parts, ",") + `]}`
}

// aiAvatarFileListOK 是**另一套信封**（`code` 是字符串 "0000"、载荷在 `data`）。
func aiAvatarFileListOK(names ...string) string {
	var parts []string
	for i, name := range names {
		parts = append(parts, `{"fileId":"F-`+itoa(int64(i))+`","name":"`+name+`"}`)
	}
	return `{"code":"0000","success":true,"message":"请求成功","data":{"items":[` +
		strings.Join(parts, ",") + `]}}`
}

func itoa(value int64) string { return strconv.FormatInt(value, 10) }

// ─────────────────────────────────────────────────────────────────────────────
// 夹具：世界 + 脚本 + 记录型 waiter + 过程话
// ─────────────────────────────────────────────────────────────────────────────

// aiAvatarStore 只补这一条链会碰的那三个存储入口（登录态、账号、设备特征）。
//
// ⚠ 与签到那侧 [poolSignInStore] 的关键不同：这里 `GetSmidState` 默认**回一份绑好的**
// 特征 + `deviceId`。这条链对 [DeviceID] 失败是"整趟失败、一枪不发"（成文见
// `ai_avatar_run.go` 的 `RunAccount`），照抄那份"回错误"的夹具会让每条用例都停在取 id 那一步。
type aiAvatarStore struct {
	fakeStore
	smid    *core.SmidState
	smidErr error
}

func (s *aiAvatarStore) GetSmidState(
	context.Context, core.Scope, core.AccountID,
) (*core.SmidState, error) {
	if s.smidErr != nil {
		return nil, s.smidErr
	}
	return s.smid, nil
}

func aiAvatarStoreBound() *aiAvatarStore {
	auth := core.SecretValue(wholeAuth)
	fingerprint := upstream.GenerateSmFingerprint(testRng())
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		panic("夹具造不出设备特征：" + perr.Message())
	}
	bound := core.NewSecretValue(text)
	device := core.NewSecretValue("SYNTH-DEVICE-199")
	return &aiAvatarStore{
		fakeStore: fakeStore{state: &core.AccountState{Authorization: &auth}},
		smid: &core.SmidState{
			AccountID: core.NewAccountID(aiAvatarTestAccount),
			// ⚠ 账号标识与 deviceId 都**不**参与出网：这条路径是"库里有就直接返回"。
			FingerprintJSON: &bound, DeviceID: &device,
		},
	}
}

func aiAvatarStoreWithoutDeviceID() *aiAvatarStore {
	return &aiAvatarStore{
		fakeStore: aiAvatarStoreBound().fakeStore,
		smidErr:   errNoFingerprint,
	}
}

var errNoFingerprint = errors.New("本用例不带 deviceId")

// aiAvatarHarness 是一次运行的四件配套：世界、响应脚本、等待记录、过程话。
type aiAvatarHarness struct {
	run    AiAvatarRun
	script *liveTaskScript
	wait   *recordingWaiter
	events *liveTaskProgress
}

// aiAvatarHappyRoutes 是"五轮全部一次合成成功 + 抽两次 + 找回得到两个文件"的默认响应。
//
// 各条用例只**覆盖**自己关心的那一路（[liveTaskScript] 按路由表的**先后**匹配），
// 所以默认表放在最后：这样每条用例都还能看见别的接口有响应，
// 而"多打了不该打的端点"由各条自己的计数断言抓，不靠 404。
func aiAvatarHappyRoutes() []liveTaskRoute {
	return []liveTaskRoute{
		{aiAvatarMergeEndpoint, []string{aiAvatarMergeOK("TASK-1")}},
		{aiAvatarStatusEndpoint, []string{aiAvatarStatusBody(3, "merged.jpg")}},
		{aiAvatarImageHost, []string{"SYNTH-BINARY-BYTES"}},
		{aiAvatarUploadEndpoint, []string{
			aiAvatarUploadOK("UP-0"), aiAvatarUploadOK("UP-1"), aiAvatarUploadOK("UP-2"),
			aiAvatarUploadOK("UP-3"), aiAvatarUploadOK("UP-4"),
		}},
		{aiAvatarHashLeftEndpoint, []string{aiAvatarHashLeftOK(2)}},
		{aiAvatarDrawEndpoint, []string{aiAvatarDrawOK("100 云朵")}},
		{aiAvatarCatalogEndpoint, []string{aiAvatarCatalogPlain("FOLDER-ID")}},
		{aiAvatarWorkListEndpoint, []string{aiAvatarWorkListOK(
			"UP-0", "UP-1", "UP-2", "UP-3", "UP-4")}},
		{aiAvatarFileListEndpoint, []string{aiAvatarFileListOK(
			"origin-0.jpg", "merged-0.jpg", "别的照片.jpg")}},
	}
}

func newAiAvatarHarness(routes ...liveTaskRoute) *aiAvatarHarness {
	return newAiAvatarHarnessWithStore(aiAvatarStoreBound(), routes...)
}

func newAiAvatarHarnessWithStore(store *aiAvatarStore, routes ...liveTaskRoute) *aiAvatarHarness {
	script := liveTaskScriptWith(append(append(sessionRoutes(), routes...), aiAvatarHappyRoutes()...)...)
	events := &liveTaskProgress{}
	wait := &recordingWaiter{}
	return &aiAvatarHarness{
		script: script,
		wait:   wait,
		events: events,
		run: AiAvatarRun{
			SignInWorld: SignInWorld{
				Store: store, Scope: core.ScopeLocal(),
				Upstream: upstream.NewUpstream(script),
				Device: upstream.NewIPadAppDevice(
					"67F4C56A00000000000000000000315C", "8A708867DBC3"),
				Clock: testClock(), Random: testRng(),
				// ⚠ 必须接池：这条链一次运行就要发五十多枪，"每次点击重打三枪建会话"
				// 在别的链上是浪费，在这里会被下面的"会话只建一次"那条断言抓到。
				Sessions: NewSessionPool(),
				Progress: events.handle(),
			},
			Wait: wait,
		},
	}
}

func (h *aiAvatarHarness) goNow() (AiAvatarResult, *core.Error) {
	return h.run.RunAccount(context.Background(), core.NewAccountID(aiAvatarTestAccount))
}

func (h *aiAvatarHarness) urls() []string {
	var out []string
	for _, request := range h.script.snapshot() {
		out = append(out, request.URL)
	}
	return out
}

// countWaits 数"被要求等了多久"里面的某一档。
func countWaits(durations []time.Duration, d time.Duration) int {
	n := 0
	for _, got := range durations {
		if got == d {
			n++
		}
	}
	return n
}

// endpointOf 把 URL 收成一个短名字，好让"按顺序发"这类断言读得下去。
func endpointOf(url string) string {
	for _, name := range []string{
		aiAvatarMergeEndpoint, aiAvatarStatusEndpoint, aiAvatarUploadEndpoint,
		aiAvatarHashLeftEndpoint, aiAvatarWorkListEndpoint, aiAvatarCatalogEndpoint,
		aiAvatarDrawEndpoint, aiAvatarFileListEndpoint,
	} {
		if strings.Contains(url, name) {
			return name
		}
	}
	switch {
	case strings.Contains(url, aiAvatarImageHost):
		return "download"
	case strings.Contains(url, "/user/querySpecTokenV2"):
		return "session-sso"
	case strings.Contains(url, "/ycloud/api/timestamp"):
		return "session-ts"
	case strings.Contains(url, "tyrzLogin"):
		return "session-jwt"
	}
	return "other:" + url
}

func (h *aiAvatarHarness) endpoints() []string {
	var out []string
	for _, url := range h.urls() {
		out = append(out, endpointOf(url))
	}
	return out
}

func sameStrings(got, want []string) bool {
	if len(got) != len(want) {
		return false
	}
	for i := range got {
		if got[i] != want[i] {
			return false
		}
	}
	return true
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据：还能抽几次 → 该打几枪
// ─────────────────────────────────────────────────────────────────────────────

func TestPlanAiAvatarDrawRounds(t *testing.T) {
	cases := []struct {
		name string
		left *int64
		want int64
	}{
		{"没给 ⇒ 兜底照抽", nil, aiAvatarDrawFallback},
		{"0 ⇒ 一枪都不抽", ptr(int64(0)), 0},
		{"负数按 0 办", ptr(int64(-3)), 0},
		{"1 次", ptr(int64(1)), 1},
		{"上游给了 12 次就抽 12 次", ptr(int64(12)), 12},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := PlanAiAvatarDrawRounds(c.left); got != c.want {
				t.Errorf("应抽 %d 枪，实得 %d", c.want, got)
			}
		})
	}
	// ⚠ 这一档与 [PlanFunai] **相反**（趣玩AI 读不到判据 ⇒ `FunaiPlanNoCriteria`，一枪都不抽）：
	// 两条链的代价方向相反 —— 那一天只有一次额度、判错抽不掉；这一族是**消耗上游给的次数**，
	// 少抽的代价是"今天的额度作废"。成文见 `ai_avatar_run.go` 的 [PlanAiAvatarDrawRounds]。
	// 这里把**两边**都钉一次，是为了"顺手对齐成一样"必须在测试里响。
	if PlanFunai(upstream.FunaiModule{}, testClock().Now()) != FunaiPlanNoCriteria {
		t.Error("趣玩AI 读不到判据应「不抽」—— 它变了就说明有人把两条链对齐成了同一种处置")
	}
}

func ptr[T any](v T) *T { return &v }

// ─────────────────────────────────────────────────────────────────────────────
// 节奏：三处 sleep 都是判据的一部分
// ─────────────────────────────────────────────────────────────────────────────

// TestAiAvatarRunPacesEveryWait ⭐ 这一条是那条链的"节奏契约"。
//
// 期望序列（五轮全成功、抽奖两次）：
//
//	每轮：3 秒（**先睡再查**，旧循环体第一行是 sleep）→ 轮间 2 秒（最后一轮之后不等）
//	抽奖：两次之间 1 秒（最后一次不等）
//
// 合起来 [3,2,3,2,3,2,3,2,3,1]。⚠ 少一个"轮间不睡"或把"先睡再查"写成"先查再睡"
// 都会改变**上游看到的请求密度**，而这三秒正是旧实现跑通过的那一份。
func TestAiAvatarRunPacesEveryWait(t *testing.T) {
	h := newAiAvatarHarness()
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("跑一趟失败：%s", perr.Message())
	}
	want := []time.Duration{
		3 * time.Second, 2 * time.Second,
		3 * time.Second, 2 * time.Second,
		3 * time.Second, 2 * time.Second,
		3 * time.Second, 2 * time.Second,
		3 * time.Second,
		time.Second,
	}
	if !sameStrings(durationNames(h.wait.durations), durationNames(want)) {
		t.Errorf("等待序列应 %v，实得 %v", want, h.wait.durations)
	}
	// 生成五轮：每轮 merge → getStatus → 下载 → upload 各一次。
	for _, c := range []struct {
		endpoint string
		times    int
	}{
		{aiAvatarMergeEndpoint, 5}, {aiAvatarStatusEndpoint, 5},
		{"download", 5}, {aiAvatarUploadEndpoint, 5},
		{aiAvatarHashLeftEndpoint, 1}, {aiAvatarDrawEndpoint, 2},
	} {
		n := 0
		if c.endpoint == "download" {
			n = h.script.count(aiAvatarImageHost)
		} else {
			n = h.script.count(c.endpoint)
		}
		if n != c.times {
			t.Errorf("%s 应打 %d 次，实得 %d 次（全部 URL：%v）", c.endpoint, c.times, n, h.urls())
		}
	}
	if res.Generated != 5 || res.Rounds != AiAvatarDailyGenerateCount {
		t.Errorf("应生成 5/%d 张，实得 %d/%d", AiAvatarDailyGenerateCount, res.Generated, res.Rounds)
	}
	if res.Draws != 2 || res.DrawPhase != AiAvatarDrawRan {
		t.Errorf("应抽中 2 次且没被叫停，实得 %d 次 / %v", res.Draws, res.DrawPhase)
	}
	if res.Status != AiAvatarDone {
		t.Errorf("有产出应 Done，实得 %v", res.Status)
	}
	if got := strings.Join(res.Prizes, ","); got != "100 云朵,100 云朵" {
		t.Errorf("两次都该记下奖品名，实得 %q", got)
	}
}

func durationNames(durations []time.Duration) []string {
	out := make([]string, 0, len(durations))
	for _, d := range durations {
		out = append(out, d.String())
	}
	return out
}

// TestAiAvatarRunFiveRoundsShareOneSession：一次运行五十多枪，但建会话只该**一趟**。
//
// 漏接票池不会报错，只会让"合成 → 查状态 → 上传"每一轮前面多出三枪 ——
// 用户 2026-09-23 点名的就是这件事，而这条链是**每轮都要票**的那一类，最容易看出来。
func TestAiAvatarRunFiveRoundsShareOneSession(t *testing.T) {
	h := newAiAvatarHarness()
	if _, perr := h.goNow(); perr != nil {
		t.Fatalf("跑一趟失败：%s", perr.Message())
	}
	if got := h.script.count("/user/querySpecTokenV2"); got != 1 {
		t.Errorf("五轮生成只该建一条会话，实得 %d 次", got)
	}
	var tokens []string
	for _, token := range poolTokens(h.script) {
		if token != "" {
			tokens = append(tokens, token)
		}
	}
	if len(tokens) == 0 {
		t.Fatal("业务请求一条都没带 jwttoken")
	}
	for i, token := range tokens {
		if token != "JWT-1" {
			t.Errorf("第 %d 条请求带的票是 %q，应始终是建会话那一次的 JWT-1", i+1, token)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 重试形状
// ─────────────────────────────────────────────────────────────────────────────

// TestAiAvatarRunPollsAtMostEightTimesThenGivesUp：查满八次还没好 ⇒ 这一枪算废，
// 重试一次（每轮两次尝试），**仍然不发** upload，也不影响后面的轮次。
func TestAiAvatarRunPollsAtMostEightTimesThenGivesUp(t *testing.T) {
	h := newAiAvatarHarness(
		liveTaskRoute{aiAvatarStatusEndpoint, []string{aiAvatarStatusPending}},
		// 抽奖判据也给 0：整趟零产出才是 [AiAvatarNothing] 那一档
		// （否则"抽中两次"会把结论抬成 Done，本条想钉的东西就被顶掉了）。
		liveTaskRoute{aiAvatarHashLeftEndpoint, []string{aiAvatarHashLeftOK(0)}},
	)
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("零产出不该回错误：%s", perr.Message())
	}
	if got := h.script.count(aiAvatarStatusEndpoint); got != 8*2*AiAvatarDailyGenerateCount {
		t.Errorf("应查满 8×2×%d 次，实得 %d 次", AiAvatarDailyGenerateCount, got)
	}
	if got := h.script.count(aiAvatarMergeEndpoint); got != 2*AiAvatarDailyGenerateCount {
		t.Errorf("每轮两次尝试 ⇒ 合成应打 %d 次，实得 %d 次", 2*AiAvatarDailyGenerateCount, got)
	}
	if got := h.script.count(aiAvatarUploadEndpoint); got != 0 {
		t.Errorf("没合成好就不该往网盘写，实得 %d 次", got)
	}
	if res.Generated != 0 || res.Status != AiAvatarNothing {
		t.Errorf("零产出应算 Nothing（不是失败），实得 %v / %v", res.Generated, res.Status)
	}
	if !h.events.has(core.LevelWarn, "查了 8 次还没合成好") {
		t.Errorf("过程话该说清楚为什么这一轮废了，实得：%s", h.events.joined())
	}
	// 一张都没写进网盘 ⇒ 那三枪"找回"没必要打（其中 `file/list` 还是另一套头）。
	if got := h.script.count(aiAvatarFileListEndpoint); got != 0 {
		t.Errorf("没有上传 id 就不该去列网盘，实得 %d 次", got)
	}
}

// TestAiAvatarRunRetriesFailedRoundOnce：上游判死（status 4）⇒ 同一轮再试一次；
// 重试成功**只算一张**（旧实现 `count` 按轮加，不按尝试加）。
func TestAiAvatarRunRetriesFailedRoundOnce(t *testing.T) {
	h := newAiAvatarHarness(
		liveTaskRoute{aiAvatarStatusEndpoint, []string{aiAvatarStatusFailed,
			aiAvatarStatusBody(3, "merged.jpg")}},
	)
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("跑一趟失败：%s", perr.Message())
	}
	if got := h.script.count(aiAvatarMergeEndpoint); got != 6 {
		t.Errorf("第一轮试两次、其余各一次 ⇒ 合成应 6 次，实得 %d 次", got)
	}
	if res.Generated != 5 {
		t.Errorf("重试成功只算一张 ⇒ 应 5 张，实得 %d（全部 URL：%v）", res.Generated, h.urls())
	}
	if got := h.script.count(aiAvatarUploadEndpoint); got != 5 {
		t.Errorf("应写盘 5 次，实得 %d 次", got)
	}
	// 重试成功 ⇒ **不该**说这一轮废了：过程话里只该有五句"生成成功"。
	if strings.Contains(h.events.joined(), "生成失败") {
		t.Errorf("第二次尝试成功了，不该报这一轮失败：%s", h.events.joined())
	}
	if got := strings.Count(h.events.joined(), "次生成成功"); got != 5 {
		t.Errorf("该报五句成功（按轮不按尝试），实得 %d 句：%s", got, h.events.joined())
	}
}

// TestAiAvatarRunSuccessWithoutImageURLIsNotAnUpload：合成成功却没回地址 ⇒ 不写盘。
//
// 这一档钉的是"成功"的两个条件**都要**成立；只看 `status` 就往下走的表现是
// 拿着空串去下载、再拿空 body 去上传（body 少一格，上游怎么回零样本）。
func TestAiAvatarRunSuccessWithoutImageURLIsNotAnUpload(t *testing.T) {
	h := newAiAvatarHarness(liveTaskRoute{
		aiAvatarStatusEndpoint, []string{aiAvatarStatusBody(3, "")},
	})
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("不该回错误：%s", perr.Message())
	}
	if got := h.script.count(aiAvatarUploadEndpoint); got != 0 {
		t.Errorf("没有图片地址就不该上传，实得 %d 次", got)
	}
	if !h.events.has(core.LevelWarn, "合成成功但没回图片地址") {
		t.Errorf("应说明卡在哪一格，实得：%s", h.events.joined())
	}
	if res.Generated != 0 {
		t.Errorf("应零产出，实得 %d", res.Generated)
	}
}

// TestAiAvatarRunMergeWithoutTaskIDSendsNoStatus：合成回了成功却没给任务 id ⇒
// 不查状态（拿空 taskId 去查是白打八枪，而那一族每一次查询的额度**未知**）。
func TestAiAvatarRunMergeWithoutTaskIDSendsNoStatus(t *testing.T) {
	h := newAiAvatarHarness(liveTaskRoute{
		aiAvatarMergeEndpoint, []string{`{"code":0,"msg":"success","result":{}}`},
	})
	if _, perr := h.goNow(); perr != nil {
		t.Fatalf("不该回错误：%s", perr.Message())
	}
	if got := h.script.count(aiAvatarStatusEndpoint); got != 0 {
		t.Errorf("没有 taskId 就不该查状态，实得 %d 次（全部 URL：%v）", got, h.urls())
	}
	if !h.events.has(core.LevelWarn, "上游回了成功但没给任务 id") {
		t.Errorf("过程话该说这一轮为什么废，实得：%s", h.events.joined())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 抽奖：三态分叉
// ─────────────────────────────────────────────────────────────────────────────

// TestAiAvatarRunDrawStoppedByUpstream ⭐ 上游叫停 ⇒ 剩下的判据次数**不发**。
//
// 判据是 5 次，响应给 [ok, ok, 408] ⇒ 只该打三枪。
// 反向改坏点：把 `outcome.Stopped` 那一档写成 `continue`（也就是旧实现对**非 408**
// 的处理）⇒ 这里变成五枪。408 到底是什么意思仍然未验证（见 [AiAvatarDrawStopped]），
// 但"认它之后就停手"这件事是旧实现的行为，必须钉住。
func TestAiAvatarRunDrawStoppedByUpstream(t *testing.T) {
	h := newAiAvatarHarness(
		liveTaskRoute{aiAvatarHashLeftEndpoint, []string{aiAvatarHashLeftOK(5)}},
		liveTaskRoute{aiAvatarDrawEndpoint, []string{
			aiAvatarDrawOK("云朵"), aiAvatarDrawOK("会员天数"), aiAvatarDrawStoppedBody,
		}},
	)
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("被叫停不是失败：%s", perr.Message())
	}
	if got := h.script.count(aiAvatarDrawEndpoint); got != 3 {
		t.Errorf("判据 5 次、第三枪被叫停 ⇒ 应打 3 枪，实得 %d 次", got)
	}
	if res.Draws != 2 || res.DrawPhase != AiAvatarDrawStopped {
		t.Errorf("应记下抽中 2 次并停在「被叫停」那一档，实得 %d / %v", res.Draws, res.DrawPhase)
	}
	if !strings.Contains(res.SummaryLine(), "后被上游叫停") {
		t.Errorf("小结该说停在哪儿，实得 %q", res.SummaryLine())
	}
	if !h.events.has(core.LevelInfo, "AI头像抽奖：") {
		t.Errorf("不是「抽完」那一档就该说一句过程话，实得：%s", h.events.joined())
	}
}

// TestAiAvatarRunDrawFailureStopsInsteadOfContinuing ⭐ 本仓与旧实现的**一处刻意不同**。
//
// 旧实现对非 0 码是 `continue`（换下一枪）；这里停手，因为协议层把"上游回了非 0 码"
// 与"请求压根没发出去"合成同一个 [core.Error]，分不开就按更保守那一档办。
// 反向改坏点：把这里改成 `continue`（恢复旧行为）⇒ 抽枪次数从 2 变 3，本条红。
// 真要恢复旧实现那条，先得在协议层做失败分型（登记在 `docs/pending-decisions.md`）。
func TestAiAvatarRunDrawFailureStopsInsteadOfContinuing(t *testing.T) {
	h := newAiAvatarHarness(
		liveTaskRoute{aiAvatarHashLeftEndpoint, []string{aiAvatarHashLeftOK(3)}},
		liveTaskRoute{aiAvatarDrawEndpoint, []string{aiAvatarDrawOK("云朵"), aiAvatarRejected}},
	)
	res, perr := h.goNow()
	if perr == nil {
		t.Fatal("某一枪发不出去应回错误：这一族的失败**不是**「今天没东西」")
	}
	if got := h.script.count(aiAvatarDrawEndpoint); got != 2 {
		t.Errorf("第二枪就该停手 ⇒ 应打 2 枪，实得 %d 次", got)
	}
	if res.Status != AiAvatarFailed || res.DrawPhase != AiAvatarDrawBlocked {
		t.Errorf("应落 Failed / Blocked，实得 %v / %v", res.Status, res.DrawPhase)
	}
	if res.Generated != 5 {
		t.Errorf("抽奖失败不该抹掉生成那一半的数字，实得 %d 张", res.Generated)
	}
	if !strings.Contains(perr.Message(), "第2次抽奖没发出去") {
		t.Errorf("错误该指出是第几枪，实得 %q", perr.Message())
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeFailed {
		t.Errorf("Failed 那一档应落失败报告，实得 %v", report.Outcome)
	}
}

// TestAiAvatarRunSkipsDrawWhenUpstreamSaysNoLeft：判据 0 ⇒ **一个写请求都不发**。
//
// 这一条钉的是"读了判据再决定"这件事真的接上了：`getHashLeft` 一次、`draw` 零次。
// 生成那一半照跑（那是另一族），所以整趟仍是 Done。
func TestAiAvatarRunSkipsDrawWhenUpstreamSaysNoLeft(t *testing.T) {
	h := newAiAvatarHarness(liveTaskRoute{
		aiAvatarHashLeftEndpoint, []string{aiAvatarHashLeftOK(0)},
	})
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("没次数不算失败：%s", perr.Message())
	}
	if got := h.script.count(aiAvatarHashLeftEndpoint); got != 1 {
		t.Errorf("判据该读一次，实得 %d 次", got)
	}
	if got := h.script.count(aiAvatarDrawEndpoint); got != 0 {
		t.Errorf("剩余 0 就不该发抽奖，实得 %d 次", got)
	}
	if res.DrawPhase != AiAvatarDrawSkipped || res.Status != AiAvatarDone {
		t.Errorf("应 Skipped + Done，实得 %v / %v", res.DrawPhase, res.Status)
	}
	if !strings.Contains(res.SummaryLine(), "没抽（剩余 0）") {
		t.Errorf("小结该说没抽，实得 %q", res.SummaryLine())
	}
}

// TestAiAvatarRunDrawsWithFallbackWhenLeftUnreadable ⭐ "读不到剩余次数"三档都照抽兜底次数。
//
// 与 [PlanAiAvatarDrawRounds] 那段"为什么照抽"配套：判据读不到时**不是**停手，
// 而是按旧实现的兜底次数抽、靠上游那句 408 停。三档分别是
// `result: null`、`result` 不是数字、这一枪本身失败。
func TestAiAvatarRunDrawsWithFallbackWhenLeftUnreadable(t *testing.T) {
	cases := []struct {
		name   string
		body   string
		warned string
	}{
		{"result 是 null：没有错误可 warn", aiAvatarHashLeftMissing, ""},
		{"result 不是数字：形状变了也要留痕", aiAvatarHashLeftNotNumber, "AI头像读剩余次数失败"},
		{"这一枪被上游拒", aiAvatarRejected, "AI头像读剩余次数失败"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			h := newAiAvatarHarness(liveTaskRoute{
				aiAvatarHashLeftEndpoint, []string{c.body},
			})
			res, perr := h.goNow()
			if perr != nil {
				t.Fatalf("读不到判据不该让整趟失败：%s", perr.Message())
			}
			// ⭐ 期望值写**字面量 5**，不写 `aiAvatarDrawFallback`：这条用例的全部意义是
			// "读不到判据时兜底抽几枪"，拿常量当期望值等于把判据交给实现自己 ——
			// 把 5 改成 1 时本条不会红（实测过一次，故此处钉死）。口径出处是旧实现的
			// `MAX_DRAW_FALLBACK`（`task.ts`），改这个数要先改那条成文。
			if got := h.script.count(aiAvatarDrawEndpoint); got != 5 {
				t.Errorf("应兜底抽 5 枪（旧 `MAX_DRAW_FALLBACK`），实得 %d 次", got)
			}
			if res.DrawPhase != AiAvatarDrawRan {
				t.Errorf("抽完了应 Ran，实得 %v", res.DrawPhase)
			}
			if c.warned != "" && !h.events.has(core.LevelWarn, c.warned) {
				t.Errorf("该留一句 warn（%s），实得：%s", c.warned, h.events.joined())
			}
			// 兜底次数之间的等待同样要等（否则等于对着一个读不到的判据连打五枪）。
			if got := countWaits(h.wait.durations, time.Second); got != 4 {
				t.Errorf("兜底 5 枪之间应等 4 次 1 秒（最后一枪不等），实得 %d 次", got)
			}
		})
	}
}

// TestAiAvatarDrawPrizeNamePriority：`contentName` 优先、退到 `prizeName`、两格都没有 ⇒
// **不收空串**（那会在界面上摆出一个"中了一个叫空的奖"）。
func TestAiAvatarDrawPrizeNamePriority(t *testing.T) {
	cases := []struct {
		name  string
		body  string
		prize []string
	}{
		{"contentName 优先", `{"code":0,"result":{"contentName":"甲","prizeName":"乙"}}`,
			[]string{"甲"}},
		{"只有 prizeName", aiAvatarDrawOKByPrizeName("乙"), []string{"乙"}},
		{"两格都没有", `{"code":0,"result":{}}`, []string{}},
		{"没有 result", `{"code":0,"msg":"success"}`, []string{}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			h := newAiAvatarHarness(
				liveTaskRoute{aiAvatarHashLeftEndpoint, []string{aiAvatarHashLeftOK(1)}},
				liveTaskRoute{aiAvatarDrawEndpoint, []string{c.body}},
			)
			res, perr := h.goNow()
			if perr != nil {
				t.Fatalf("不该回错误：%s", perr.Message())
			}
			if !sameStrings(res.Prizes, c.prize) {
				t.Errorf("奖品清单应 %v，实得 %v", c.prize, res.Prizes)
			}
			if res.Draws != 1 {
				t.Errorf("抽中了就该记 1 次（与有没有名字无关），实得 %d", res.Draws)
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 找回写进网盘的那两个文件
// ─────────────────────────────────────────────────────────────────────────────

// TestAiAvatarRunCollectsThroughThreeRequests ⭐ 三枪的顺序、参数与**头**。
//
// 头这一格是本仓唯一一处"同一个域用两套头集合"的地方：
// `hcy/file/list` 在 `m.mcloud.139.com` 的 jwttoken 名单**之外**（见
// `internal/upstream/app_ai_avatar.go` 文件头），所以它带 `Authorization` 与
// `x-yun-*`，**不带** `jwttoken` —— 少这一条断言，"顺手给它加上票"就没人拦。
func TestAiAvatarRunCollectsThroughThreeRequests(t *testing.T) {
	h := newAiAvatarHarness()
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("跑一趟失败：%s", perr.Message())
	}
	order := h.endpoints()
	wantTail := []string{aiAvatarCatalogEndpoint, aiAvatarWorkListEndpoint,
		aiAvatarFileListEndpoint}
	if len(order) < 3 || !sameStrings(order[len(order)-3:], wantTail) {
		t.Fatalf("最后三枪应是 catalog → workList → file/list，实得尾段 %v", order[max(0, len(order)-6):])
	}
	requests := h.script.snapshot()
	fileList := requests[len(requests)-1]
	if !strings.Contains(fileList.URL, aiAvatarFileListEndpoint) {
		t.Fatalf("最后一枪不是 file/list：%s", fileList.URL)
	}
	if has := headerValue(fileList, "jwttoken"); has != "" {
		t.Errorf("file/list 不该带 jwttoken（那一台主机不在旧实现的名单里），实带 %q", head(has, 8))
	}
	if has := headerValue(fileList, "Authorization"); has == "" {
		t.Error("file/list 该带账号侧 Authorization")
	}
	if has := headerValue(fileList, "x-yun-user-timestamp"); has == "" {
		// `WithAppHeaders` 的那一整套；只挑一枚代表，其余由 upstream 侧的用例钉。
		t.Errorf("file/list 该走 App 那套 x-yun-* 头，实得头名 %v", headerNames(fileList))
	}
	var body map[string]any
	if err := json.Unmarshal(fileList.Body, &body); err != nil {
		t.Fatalf("file/list 的 body 不是 JSON：%v", err)
	}
	if body["parentFileId"] != "FOLDER-ID" {
		t.Errorf("parentFileId 应取 catalog 路径的末段 FOLDER-ID，实得 %v", body["parentFileId"])
	}
	pageInfo, _ := body["pageInfo"].(map[string]any)
	// ⭐ 期望值写字面量：`100` 就是"一页取完、不翻页"那一格判据本身
	// （旧实现常量）。拿 `aiAvatarFileListPageSize` 当期望值等于让实现自己定判据 ——
	// 改成 50 时这条不会红，而表现是"多于 50 个文件时静默漏找"。
	if pageInfo == nil || pageInfo["pageSize"] != float64(100) {
		t.Errorf("pageSize 应是 100（一页取完、不翻页），实得 %v", body["pageInfo"])
	}
	// 作品列表要 `type=1`（旧实现的默认值）。
	catalogSeen := 0
	for _, request := range requests {
		if strings.Contains(request.URL, aiAvatarWorkListEndpoint) {
			catalogSeen++
			if !strings.Contains(request.URL, "type=1") {
				t.Errorf("getWorkList 该带 type=1，实得 %s", request.URL)
			}
		}
	}
	if catalogSeen != 1 {
		t.Errorf("作品列表该只读一次，实得 %d 次", catalogSeen)
	}
	// 找回到的那两个文件（UP-0 的两张图）就是将来清理的输入。
	if !sameStrings(res.TempFileIDs, []string{"F-0", "F-1"}) {
		t.Errorf("应找回 F-0/F-1，实得 %v", res.TempFileIDs)
	}
	if res.Note != "" {
		t.Errorf("都找回了不该有备注，实得 %q", res.Note)
	}
	if !strings.Contains(res.SummaryLine(), "网盘里留了 2 个待清理文件") {
		t.Errorf("小结该报出待清理个数，实得 %q", res.SummaryLine())
	}
}

// TestAiAvatarRunFileListUsesTheStoredDomainProfile ⭐ 网盘那一枪必须用**回填过 `x-yun-uni`**
// 的那份档案来发 —— 这是 [SignInWorld.Open] 存在的唯一理由。
//
// 表现型错误：自己 `upstream.NewAppProfile(device)` 一份，请求看着完全正常，
// 只有网盘侧回"未登录"（而 `hcy/file/list` 在这一族是**只读**的，红起来像上游抽风）。
// 反向改坏点：把 [collectTempFiles] 里那句 `opening.Profile` 换成现造的一份 ⇒ 本条红。
func TestAiAvatarRunFileListUsesTheStoredDomainProfile(t *testing.T) {
	store := aiAvatarStoreBound()
	domain := core.UserDomainID("UD-77")
	store.state.UserDomainID = &domain
	h := newAiAvatarHarnessWithStore(store)
	if _, perr := h.goNow(); perr != nil {
		t.Fatalf("跑一趟失败：%s", perr.Message())
	}
	requests := h.script.snapshot()
	fileList := requests[len(requests)-1]
	if !strings.Contains(fileList.URL, aiAvatarFileListEndpoint) {
		t.Fatalf("最后一枪不是 file/list：%s", fileList.URL)
	}
	if got := headerValue(fileList, "x-yun-uni"); got != "UD-77" {
		t.Errorf("file/list 该带库里那个用户域 UD-77，实得 %q（头名 %v）", got, headerNames(fileList))
	}
	// 同一趟里那一族自己的六枪**不**带 x-yun-*（它们是另一套头集合，见 upstream 文件头）。
	for _, request := range requests {
		if strings.Contains(request.URL, aiAvatarMergeEndpoint) &&
			headerValue(request, "x-yun-uni") != "" {
			t.Error("merge 那一枪不该带网盘侧的 x-yun-uni")
		}
	}
}

func headerValue(request core.HTTPRequest, name string) string {
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1]
		}
	}
	return ""
}

func headerNames(request core.HTTPRequest) []string {
	out := make([]string, 0, len(request.Headers))
	for _, pair := range request.Headers {
		out = append(out, pair[0])
	}
	return out
}

// TestAiAvatarRunCollectFailureDoesNotFailTheRun ⭐ 找回那一族**回不了错误**。
//
// 生成与抽奖都办完了，把整趟判红等于告诉用户"这一趟没成"，而他们据此重跑会
// **再写五张图**进网盘。所以这里只记一句人话 + warn。
// 三个子档分别卡在三枪的不同位置，而且**失败之后不该再打后面那几枪**（省 IO，
// 也让"卡在哪"这件事在计数上看得出来）。
func TestAiAvatarRunCollectFailureDoesNotFailTheRun(t *testing.T) {
	cases := []struct {
		name         string
		routes       []liveTaskRoute
		wantWorkList int
		wantFileList int
		wantNotePart string
	}{
		{"读目录失败", []liveTaskRoute{{aiAvatarCatalogEndpoint, []string{aiAvatarRejected}}},
			0, 0, "读活动目录失败"},
		{"目录路径是空的", []liveTaskRoute{{aiAvatarCatalogEndpoint, []string{aiAvatarCatalogNoPath}}},
			0, 0, "上游没给活动目录的路径"},
		{"读作品列表失败", []liveTaskRoute{{aiAvatarWorkListEndpoint, []string{aiAvatarRejected}}},
			1, 0, "读作品列表失败"},
		{"列文件失败", []liveTaskRoute{{aiAvatarFileListEndpoint, []string{aiAvatarRejected}}},
			1, 1, "列目录失败"},
		{"一个名字都没撞上", []liveTaskRoute{
			{aiAvatarFileListEndpoint, []string{aiAvatarFileListOK("完全不相关的照片.jpg")}}},
			1, 1, "按文件名一个都没撞上"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			h := newAiAvatarHarness(c.routes...)
			res, perr := h.goNow()
			if perr != nil {
				t.Fatalf("找回失败不该让整趟失败：%s", perr.Message())
			}
			if res.Status != AiAvatarDone {
				t.Errorf("生成+抽奖都办完了应仍是 Done，实得 %v", res.Status)
			}
			if res.Note == "" || !strings.Contains(res.Note, c.wantNotePart) {
				t.Errorf("备注该说清楚卡在哪（%s），实得 %q", c.wantNotePart, res.Note)
			}
			if !h.events.has(core.LevelWarn, "AI头像临时文件没找全") {
				t.Errorf("该留一句 warn，实得：%s", h.events.joined())
			}
			if got := h.script.count(aiAvatarWorkListEndpoint); got != c.wantWorkList {
				t.Errorf("作品列表应打 %d 次，实得 %d 次", c.wantWorkList, got)
			}
			if got := h.script.count(aiAvatarFileListEndpoint); got != c.wantFileList {
				t.Errorf("列文件应打 %d 次，实得 %d 次", c.wantFileList, got)
			}
			if len(res.TempFileIDs) != 0 {
				t.Errorf("失败时不该交回任何 id（宁少勿错），实得 %v", res.TempFileIDs)
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 整趟的两种"不发了"
// ─────────────────────────────────────────────────────────────────────────────

// TestAiAvatarRunSendsNothingWithoutDeviceID ⭐ 换不到 `deviceId` ⇒ **一枪都不发**。
//
// 旧实现在这一步直接跳过这个账号，因为那一族**每一枪**都带 `deviceid` 头。
// ⚠ "少这个头上游会不会理"本仓零样本 ⇒ 不赌"少一个头也能跑"。
// 注意这里连建会话那三枪都没打：票是懒的，取 id 在取票之前 —— 反向改坏点：
// 把 `RunAccount` 里那两步换个顺序，本条的计数就从 0 变成 3。
func TestAiAvatarRunSendsNothingWithoutDeviceID(t *testing.T) {
	h := newAiAvatarHarnessWithStore(aiAvatarStoreWithoutDeviceID())
	res, perr := h.goNow()
	if perr == nil {
		t.Fatal("换不到 deviceId 应回错误")
	}
	if got := len(h.script.snapshot()); got != 0 {
		t.Errorf("一枪都不该发，实得 %d 条请求：%v", got, h.urls())
	}
	if res.Status != AiAvatarFailed || res.Error == "" {
		t.Errorf("应落 Failed 并带上原因，实得 %v / %q", res.Status, res.Error)
	}
}

// TestAiAvatarRunWaitCancellationStopsImmediately：等不下去了 ⇒ 后面的轮次**不发**。
//
// 等待序列是 3（第一轮轮询）→ 2（轮间）→ 3（第二轮轮询）⇒ 第 3 次等待被打断：
// 第一轮该有产出，第二轮只起了个头（合成已经发出去了，那是在等待**之前**），
// 但**抽奖与找回都不该再发**。这一条钉的是"取消之后不再发新请求"，不是"不发第二次合成"。
func TestAiAvatarRunWaitCancellationStopsImmediately(t *testing.T) {
	h := newAiAvatarHarness()
	h.run.Wait = &cancellingWaiter{after: 2}
	res, perr := h.goNow()
	if perr == nil {
		t.Fatal("等待被打断应回错误")
	}
	if res.Status != AiAvatarFailed {
		t.Errorf("应落 Failed，实得 %v", res.Status)
	}
	if res.Generated != 1 {
		t.Errorf("第一轮该已完成（1 张），实得 %d", res.Generated)
	}
	if got := h.script.count(aiAvatarMergeEndpoint); got != 2 {
		t.Errorf("打断点之前应只走到第二轮的开头（合成 2 次），实得 %d 次", got)
	}
	if got := h.script.count(aiAvatarUploadEndpoint); got != 1 {
		t.Errorf("只有第一轮写进了网盘，实得 %d 次", got)
	}
	if got := h.script.count(aiAvatarDrawEndpoint); got != 0 {
		t.Errorf("取消之后连抽奖都不该发，实得 %d 次", got)
	}
}

// cancellingWaiter 在第 `after` 次等待时返回错误（模拟 ctx 被取消）。
type cancellingWaiter struct {
	after int
	calls int
}

func (w *cancellingWaiter) Wait(_ context.Context, _ time.Duration) *core.Error {
	w.calls++
	if w.calls > w.after {
		return core.NewTimeout("等待被取消")
	}
	return nil
}

// TestAiAvatarRunRefreshesThePageTicketOn90001：票被判死时**只刷这一页**并重放一次。
//
// 这是 AGENTS §1.12 那条"票 = 账号 × 页面"在这一族上的落点：`draw` 拿到 90001 ⇒
// 重新建签到页那条会话（三枪）、同一个请求重放一次、整趟仍然成功。
// ⚠ 只刷这一页：别的页面的票不该被顶掉，那是票池自己的用例钉的。
func TestAiAvatarRunRefreshesThePageTicketOn90001(t *testing.T) {
	h := newAiAvatarHarness(liveTaskRoute{
		aiAvatarHashLeftEndpoint, []string{notLoggedIn, aiAvatarHashLeftOK(1)},
	})
	res, perr := h.goNow()
	if perr != nil {
		t.Fatalf("刷完票就该成功：%s", perr.Message())
	}
	if got := h.script.count("/user/querySpecTokenV2"); got != 2 {
		t.Errorf("90001 之后应重建一次会话，实得建会话 %d 趟", got)
	}
	if got := h.script.count(aiAvatarHashLeftEndpoint); got != 2 {
		t.Errorf("同一枪应重放一次，实得 %d 次", got)
	}
	if res.DrawPhase != AiAvatarDrawRan || res.Status != AiAvatarDone {
		t.Errorf("刷票成功就不该留痕，实得 %v / %v", res.DrawPhase, res.Status)
	}
}

// TestAiAvatarSummaryLineTellsEveryPhaseApart：四档停法在界面上必须念得出不同的一句话。
//
// 这一条不发出任何请求，只把 [AiAvatarResult] 摆出来 —— 因为它容易被"四档共用一句"
// 悄悄改掉，而那一格是用户唯一看得见"今天为什么少了两次"的地方。
func TestAiAvatarSummaryLineTellsEveryPhaseApart(t *testing.T) {
	base := AiAvatarResult{
		Account: core.NewAccountID(aiAvatarTestAccount), Rounds: 5, Generated: 3,
		Prizes: []string{"云朵"}, Status: AiAvatarDone,
	}
	cases := []struct {
		phase AiAvatarDrawPhase
		draws int
		want  string
	}{
		{AiAvatarDrawRan, 2, "抽奖 2 次"},
		{AiAvatarDrawStopped, 1, "抽奖 1 次后被上游叫停"},
		{AiAvatarDrawSkipped, 0, "抽奖没抽（剩余 0）"},
		{AiAvatarDrawBlocked, 0, "抽奖 0 次后有一枪没发出去"},
	}
	seen := map[string]bool{}
	for _, c := range cases {
		res := base
		res.DrawPhase, res.Draws = c.phase, c.draws
		line := res.SummaryLine()
		if !strings.Contains(line, c.want) {
			t.Errorf("档位 %v 的小结该含 %q，实得 %q", c.phase, c.want, line)
		}
		if seen[line] {
			t.Errorf("两档念出了同一句话：%q", line)
		}
		seen[line] = true
		if strings.Contains(line, "Basic") || strings.Contains(line, "JWT") {
			t.Errorf("小结出网，不该带任何凭据形态：\n%s", line)
		}
	}
	// 零产出那一档仍算成功：这句话不该出现"失败"。
	empty := AiAvatarResult{Account: core.NewAccountID(aiAvatarTestAccount),
		Status: AiAvatarNothing, DrawPhase: AiAvatarDrawSkipped, Prizes: []string{}}
	if strings.Contains(empty.SummaryLine(), "失败") {
		t.Errorf("没东西可抽不是失败：%q", empty.SummaryLine())
	}
	if report := empty.RunReport(); report.Outcome != core.OutcomeSuccess {
		t.Errorf("Nothing 应落成功，实得 %v", report.Outcome)
	}
}
