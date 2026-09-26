package app

// 活体探针（带写）：跑**整趟生产入口** [TaskMatrixRun.RunAccount]。
//
// # 这一发补的是哪一格
//
// 第廿三趟（`live_probe_mail_pdf_production_test.go`）刻意只走了 `apply` +
// [TaskMatrixRun.cleanupCloudFiles] + [TaskMatrixRunResult.SummaryLine] 那三处生产代码，
// 边界里写着"整趟 `RunAccount` 今天没跑"。而 app-task 的**生产入口**只有这一个：
// 界面上点那一下（`POST /api/runs`）与调度到点跑的都是它（`internal/server/run_dispatch_signin.go`）。
// 这一发把它跑掉，同时收三格读数：
//
//   - **① 事件流**：`RunAccount` 自己播的每一行经 [liveProbeSink.progress] 落地 ⇒
//     这里念出的与用户在界面上看到的是**同一句文本**（第廿三趟为这一格立过的规矩）。
//   - **② 隔天回读（R24 剩下那一格）**：R24 已经钉住"打了不等于当场认"，但
//     "复查里那些 `没认` 到底是**滞后**（昨天的打卡今天翻）还是**永不翻**（要真去 App 做动作）"
//     那一格成文写的是"要隔天再读同一张表对比"。所以本探针在**动手前**拉一遍表、
//     跑完再拉**第三遍**，逐条比对 `state` —— 并把它分成两类：
//     **本趟发过枪的**（那是当场的功劳）与**本趟一个字都没动却翻档的**（那才是滞后的证据）。
//   - **③ 传输层逐枪（R26）**：`mrp …/market/signin/page/doTask` 这一口整族零实拍，
//     而成文里那句"第一次真跑 409 时把这一枪的请求与响应逐字记下来"要有个落点。
//     探针给 [liveProbeWorldWith] 挂一层记录器 ⇒ 它看到的是**整趟所有**请求，
//     不只是这一条链（顺带把"这一趟到底打了哪些类枪、各几发"念成一份账）。
//
// # 这一发**会真写**上游
//
// 打卡（不可回滚）+ 往网盘写文件（链尾自己删）。授权口径沿用 2026-09-25 那句
// "除兑换（真扣豆、已实测）外全部实测，包括上传网盘文件"，且它跑的就是**平时那点定时任务**
// 会跑的那条链 —— 不额外多做任何一件本仓在生产里不做的事。
// 四个 env 不齐直接 skip（默认档），带写那一发只在**副本库**上跑。
//
// # 凭据边界（这层比别处更要紧，因为它看的是**全部**请求）
//
// 记录器只打：方法 + 主机 + 路径 + **查询串的键与各值的长度** + 响应状态码 + 响应**形状**。
// 值只按 [liveProbeValueKeys] 那一份白名单念（`code` / `msg` / `message` / `success` / `result`），
// 其余一律"类型 + 长度"。预签名 `uploadUrl` 的查询串里就是一次性 token ⇒ **键可以、值不行**。
// ⚠ 而 `PUT` 那一枪**整条 URL 都不打**（换成一句占位，只保留"发了几发"）：它的主机与对象键
// 本身就是上一枪响应交回来的写入入口，"键可以、值不行"这一档在它身上不够用。
// 响应体解不成 JSON 时**不打内容**（别处那一份 `dump` 会打前 40 字节，这一层看不到请求是谁发的，
// 万一是回 HTML 的鉴权页就等于把票念出来），只给字节数。

import (
	"context"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// liveProbeMrpDoTaskPath 与 liveProbeClickPath 是记录器**逐枪打印**的两条路径（其余只进最后那份计数账）。
//
// 为什么只逐枪打这两条：整趟有四五十枪（两次拉表就是 32 枪），全打会把事件流埋掉；
// 这两条是**今天仍零实拍**或**只有真跑才补得上**的那两条（R26 与 R25 的 key 档位）。
const (
	liveProbeMrpDoTaskPath = "/market/signin/page/doTask"
	liveProbeClickPath     = "/ycloud/signin/task/click"
)

var liveProbeWatchedPaths = map[string]string{
	liveProbeMrpDoTaskPath: "R26 这一口整族零实拍",
	liveProbeClickPath:     "R25 的 key 档位 + 58999 那一族的原话",
}

// liveProbeRecorder 攒一份"这一趟打了哪些类枪"的账，并逐枪念被盯着的那几条。
type liveProbeRecorder struct {
	mu     sync.Mutex
	shots  int
	counts map[string]int
	sink   *liveProbeSink
	why    map[string]string
}

func newLiveProbeRecorder(sink *liveProbeSink) *liveProbeRecorder {
	return &liveProbeRecorder{
		counts: map[string]int{},
		sink:   sink,
		why:    liveProbeWatchedPaths,
	}
}

// liveProbeRecordingClient 是挂在真实客户端外面的那层**只看不改**的壳。
type liveProbeRecordingClient struct {
	inner    core.HTTPClient
	recorder *liveProbeRecorder
}

func (c *liveProbeRecordingClient) Send(
	ctx context.Context, request core.HTTPRequest,
) (core.HTTPResponse, error) {
	response, err := c.inner.Send(ctx, request)
	c.recorder.record(request, response, err)
	return response, err
}

// record 落一枪的读数。⚠ 它**不吞**任何东西：出错那一枪同样要记账，
// 因为"这一枪挂了"本身就是 R26 要的那格（形状错与网络错在两处读数上长相不同）。
func (r *liveProbeRecorder) record(
	request core.HTTPRequest, response core.HTTPResponse, err error,
) {
	host, path, queryWord := liveProbeSplitURL(request.URL)
	key := request.Method.String() + " " + host + path
	// ⚠ **预签名写入入口**（`PUT <uploadUrl>`，主机与对象键都来自上一枪的响应）不进账本：
	// 本仓对它的纪律是"绝不进日志、夹具、错误文本"（见 `internal/upstream/app_cloud_upload.go`
	// 文件头那一节）。2026-09-26 第廿六趟真跑时这份账把那一枪的**主机 + 对象键**打了出去
	// （查询串本来就只有长度，所以只有那两格越界）⇒ 现在整条 URL 换成占位，
	// 只有"发了几发 PUT"这一格读数保留（那一格才是探针要的）。
	if request.Method == core.MethodPut {
		key = "PUT <预签名写入入口：主机与路径不打印>"
	}

	r.mu.Lock()
	r.shots++
	shot := r.shots
	r.counts[key]++
	_, watched := r.why[path]
	r.mu.Unlock()

	if !watched {
		return
	}
	line := "枪[" + strconv.Itoa(shot) + "] " + key
	if queryWord != "" {
		line += " 查询串 " + queryWord
	}
	if len(request.Body) > 0 {
		line += "，请求体 " + strconv.Itoa(len(request.Body)) + " 字节"
	}
	switch {
	case err != nil:
		// 只念**类别**：core 的错误文本里可能带着 URL（含一次性 token）。
		line += " ⇒ 传输层报错（值不打印，长度 " + strconv.Itoa(len(err.Error())) + "）"
	default:
		line += " ⇒ HTTP " + strconv.Itoa(int(response.Status)) + "，" + liveProbeResponseBodyWord(response)
	}
	// 尾巴上那句"为什么盯着它"由读数的人看，不进每一行（同一句重复几十遍会盖掉内容）。
	r.sink.say("%s", line)
}

// sayTally 念最后那份账：每一类枪打了几发（按 host+path 升序，跨趟可比对）。
func (r *liveProbeRecorder) sayTally() {
	keys := make([]string, 0, len(r.counts))
	for key := range r.counts {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	r.sink.say("传输层账本：一共 %d 枪 / %d 类", r.shots, len(keys))
	for _, key := range keys {
		r.sink.say("  %s ×%d", key, r.counts[key])
	}
}

// countFor 数**路径**命中某一发的枪数。账本按"方法 主机 路径"分键，而探针要问的常常是
// "这一条路径今天出了几发"（同一平台上还有别的域用同一路径，故只按后缀匹配）。
func (r *liveProbeRecorder) countFor(path string) int {
	r.mu.Lock()
	defer r.mu.Unlock()
	total := 0
	for key, n := range r.counts {
		if strings.HasSuffix(key, path) {
			total += n
		}
	}
	return total
}

// liveProbeSplitURL 把一个 URL 拆成"主机 + 路径 + 查询串的键与各值长度"。
//
// 值长度而不是值：`deviceId`、预签名 `uploadUrl` 里那串 `X-Amz-Signature` 都是
// 不能进日志的东西，而"有没有带、带了多长"正是这一族要的那格读数。
func liveProbeSplitURL(raw string) (host, path, queryWord string) {
	parsed, perr := url.Parse(raw)
	if perr != nil {
		return "(URL 解不开)", liveProbeClip(raw, 40), ""
	}
	names := make([]string, 0, len(parsed.Query()))
	for name := range parsed.Query() {
		names = append(names, name)
	}
	sort.Strings(names)
	parts := make([]string, 0, len(names))
	for _, name := range names {
		values := parsed.Query()[name]
		width := 0
		for _, value := range values {
			width += len(value)
		}
		parts = append(parts, name+"(值长 "+strconv.Itoa(width)+")")
	}
	return parsed.Host, parsed.Path, strings.Join(parts, " ")
}

// TestLiveProbeRecorderKeepsPresignedURLOutOfOutput ⭐ 反向钉：账本里不得出现预签名写入入口的
// 主机、对象键或查询串值。
//
// 反向改坏它的样子：把 [liveProbeRecorder.record] 里那句 `MethodPut` 的占位换回真实
// `host + path` ⇒ 这个测试红（第一条断言），而真跑的日志里会重新长出对象存储的主机与对象键。
//
// 为什么值得单独一个测试而不靠人工审：这一枪的 URL **来自上一枪的响应**，
// 所以它不像凭据头那样有固定名字可抓 —— 记录器是整趟里唯一会把它打印出来的地方。
// 夹具全部是 `SYNTH-*` 合成值（与网盘那一族的夹具同一口径）。
func TestLiveProbeRecorderKeepsPresignedURLOutOfOutput(t *testing.T) {
	sink := newLiveProbeSink(t)
	recorder := newLiveProbeRecorder(sink)
	request := core.HTTPRequest{
		Method: core.MethodPut,
		URL: "https://synth-bucket.example.com/SYNTH-object-key-0123456789" +
			"?X-Amz-Signature=SYNTH-secret-signature-value",
	}
	recorder.record(request, core.HTTPResponse{Status: 200}, nil)
	recorder.sayTally()

	joined := strings.Join(sink.lines, "\n")
	for _, must := range []string{"synth-bucket.example.com", "SYNTH-object-key-0123456789",
		"SYNTH-secret-signature-value"} {
		if strings.Contains(joined, must) {
			t.Errorf("账本里出现了预签名入口的那一格 %q ⇒ 输出：\n%s", must, joined)
		}
	}
	if !strings.Contains(joined, "PUT") {
		t.Error("占位把「发过几发 PUT」这一格读数也一起抹掉了：账本里连 PUT 都没有 ⇒ 探针失去用途")
	}
	if recorder.countFor("SYNTH-object-key-0123456789") != 0 {
		t.Error("countFor 不该按对象键命中：那一格已经不进账本的键了")
	}
}

// liveProbeResponseBodyWord 只给**形状**，且比别处那一份 `dump` 更窄：
// 非 JSON 时连前 40 字节都不打（这一层认不出请求是谁发的，HTML 鉴权页里就带着票）。
func liveProbeResponseBodyWord(response core.HTTPResponse) string {
	text, terr := response.Text()
	if terr != nil {
		return "体解不成 UTF-8（" + strconv.Itoa(len(response.Body)) + " 字节，内容不打印）"
	}
	shape := liveProbeShape("", text)
	if strings.HasPrefix(shape, "非 JSON") {
		return "非 JSON（" + strconv.Itoa(len(text)) + " 字节，内容不打印）"
	}
	return "体形状 " + shape
}

// liveProbeTaskWord 是一条任务在某一遍拉表里的面值快照（四格都是上游自己的业务读数 ⇒ 可落档）。
type liveProbeTaskWord struct {
	name, group                      string
	state, currstep, process, enable string
}

// liveProbeTableWords 只读拉一遍表并交回 id → 快照。
//
// 拉不到时交回 **nil**，并把这一句念出来：调用方的差集在 nil 那一档**不成立**，
// 那与"表没动"是两回事（这一族在第十九、廿二趟各撞过一次）。
func liveProbeTableWords(
	ctx context.Context, opening SignInOpening, sink *liveProbeSink, label string,
) map[int64]liveProbeTaskWord {
	matrix, perr := AppTaskMatrix(ctx, opening.Source)
	if perr != nil {
		sink.say("拉表[%s]失败：%s ⇒ 这一遍的差集**不成立**", label, sink.mask(perr.Error()))
		return nil
	}
	pending := matrix.Pending()
	sink.say("拉表[%s]：%d 枪 / 去重前 %d / 去重后 %d / 待做 %d / 没读到的组 %d",
		label, matrix.Fetches, matrix.Raw, len(matrix.Items), len(pending), len(matrix.Failures))
	words := make(map[int64]liveProbeTaskWord, len(matrix.Items))
	for _, item := range matrix.Items {
		id, ok := taskIDOf(item)
		if !ok {
			continue
		}
		words[id] = liveProbeTaskWord{
			name: taskNameOf(item), group: taskGroupOf(item),
			state: liveProbeFieldWord(item.State), currstep: liveProbeFieldWord(item.CurrStep),
			process: liveProbeFieldWord(item.Process), enable: liveProbeFieldWord(item.Enable),
		}
	}
	return words
}

// liveProbeSayWordDiff 逐条念两遍之间的变化，并按"本趟有没有为它发过枪"分两类。
//
// 那第二类（**没动却变了**）就是 R24 剩下那一格的证据：它意味着复查里那些 `没认`
// 至少有一部分是**滞后**，而不是"这一类任务打卡永不翻档"。
func liveProbeSayWordDiff(
	sink *liveProbeSink, before, after map[int64]liveProbeTaskWord, fired map[int64]bool,
) {
	if before == nil || after == nil {
		sink.say("⚠ 两遍里有一遍没读出来 ⇒ 差集不成立，这一格仍空（**不等于**「表没动」）")
		return
	}
	ids := make([]int64, 0, len(after))
	for id := range after {
		ids = append(ids, id)
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })

	var changed, changedUntouched, appeared, gone int
	for _, id := range ids {
		newer := after[id] // ids 取自 after ⇒ 这一格必在
		older, hadOld := before[id]
		if !hadOld {
			appeared++
			sink.say("只在跑后出现 #%d [%s] %s（state=%s）⇒ 昨天这张表里没有这一条",
				id, newer.group, newer.name, newer.state)
			continue
		}
		diffs := liveProbeWordFields(older, newer)
		if len(diffs) == 0 {
			continue
		}
		changed++
		tail := "本趟为它发过枪"
		if !fired[id] {
			tail = "本趟**没动它**"
			changedUntouched++
		}
		sink.say("变了 #%d [%s] %s ⇒ %s ｜ %s", id, newer.group, newer.name,
			strings.Join(diffs, "、"), tail)
	}
	for id, older := range before {
		if _, still := after[id]; !still {
			gone++
			sink.say("只在跑前有 #%d [%s] %s（state=%s）⇒ 跑后那张表里读不到它了",
				id, older.group, older.name, older.state)
		}
	}
	sink.say("差集读数：两遍都在的 %d 条里**变了 %d 条**，其中本趟没动过的 %d 条"+
		"（那 %d 条就是 R24 那一问的读数：不是这一趟打的功劳）；只在跑后出现 %d 条、只在跑前 %d 条。",
		len(after)-appeared, changed, changedUntouched, changedUntouched, appeared, gone)
}

// liveProbeWordFields 比对快照的四格，交回"哪一格从什么变成什么"。
//
// 只比这四格：`state` / `currstep` / `process` / `enable` 是这一族今天唯二会动的面值
// （名字与组是静态的，比对它们只会淹没读数）。
func liveProbeWordFields(older, newer liveProbeTaskWord) []string {
	pairs := []struct {
		name          string
		before, after string
	}{
		{"state", older.state, newer.state},
		{"currstep", older.currstep, newer.currstep},
		{"process", older.process, newer.process},
		{"enable", older.enable, newer.enable},
	}
	out := make([]string, 0, len(pairs))
	for _, pair := range pairs {
		if pair.before != pair.after {
			out = append(out, pair.name+" "+pair.before+"→"+pair.after)
		}
	}
	return out
}

// liveProbeWorldRecording 是 [liveProbeWorldWith] 的那一份世界 + 挂在客户端外面的记录器。
//
// 两个消费者共用它，是因为"记录器要拿到同一份掩码表"这件事容易漏：漏了的探针会把
// 未登记的串直接吐进日志（掩码表只在 [liveProbeSink.see] 见过的那些串上生效）。
func liveProbeWorldRecording(
	t *testing.T,
) (TaskMatrixRun, core.AccountID, *liveProbeSink, *liveProbeRecorder) {
	t.Helper()
	var recorder *liveProbeRecorder
	run, account, sink, _ := liveProbeWorldWith(t,
		func(client core.HTTPClient, s *liveProbeSink) core.HTTPClient {
			recorder = newLiveProbeRecorder(s)
			return &liveProbeRecordingClient{inner: client, recorder: recorder}
		})
	return run, account, sink, recorder
}

// TestLiveProbeRunAccountFull 跑整趟生产入口，并收上面说的那三格读数。
func TestLiveProbeRunAccountFull(t *testing.T) {
	run, account, sink, recorder := liveProbeWorldRecording(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	// ── 动手前：第一遍只读拉表（R24 的那半边要靠这一份快照）──
	before := liveProbeTableWords(ctx, opening, sink, "动手前")

	// ── 生产入口本身。它自己会把每一行播报进 sink（与界面同一句文本）。──
	started := time.Now()
	result, rerr := run.RunAccount(ctx, account)
	if rerr != nil {
		sink.say("⚠ 整趟 RunAccount 交回错误：%s（下面那些读数仍照念 —— 挂在哪一步本身就是判据）",
			sink.mask(rerr.Error()))
	}
	sink.say("小结：%s", result.SummaryLine())
	for _, note := range result.Notes {
		sink.say("Note：%s", sink.mask(note))
	}
	sink.say("清理结局：%s 原因=%q ｜ 记账枚数 %d（其中上游另存的产物 %d 枚）｜ 上传计数 %d",
		liveProbeCleanupWord(result), sink.mask(result.CleanupReason),
		len(result.CloudFileIDs), result.CloudProductFiles, result.Counts.Uploaded)

	// 逐条处置与逐条复查（播报里都有，但那是"人话行"；这两段给的是**面值**，
	// 后面那份差集要按 id 对照，缺它就归不了因）。
	fired := make(map[int64]bool, len(result.Steps))
	for _, step := range result.Steps {
		switch step.Kind {
		case StepClick, StepRunner:
			fired[step.TaskID] = true
		}
		sink.say("处置 #%d [%s] %s ⇒ %s runner=%q 上传=%d 原因=%q 错误=%q",
			step.TaskID, step.Group, step.Name, step.Kind, step.Runner, step.Uploads,
			sink.mask(step.Reason), sink.mask(step.Err))
	}
	for _, verified := range result.Verified {
		sink.say("复查 #%d [%s] %s ⇒ %s%s%s", verified.TaskID, verified.Group, verified.Name,
			verified.Class, liveProbePlanSuffix("该去哪", verified.Label),
			liveProbePlanSuffix("备注", verified.Note))
	}
	sink.say("计数：读 %d / 待做 %d / 打卡 %d / 玩法 %d / 失败 %d / 缺口 %d / 规则跳过 %d / 需人工 %d "+
		"｜ 复查四格：当场认 %d / 没认 %d / 本来就是 %d / 仍开着 %d ｜ 拉表 %d 枪（没读到 %d 组）+ 复查 %d 枪",
		result.Counts.Read, result.Counts.Pending, result.Counts.Clicked, result.Counts.Ran,
		result.Counts.Failed, result.Counts.ToolGap, result.Counts.RuleSkipped,
		result.Counts.NeedsHuman,
		result.Counts.DoneNow, result.Counts.NotAcknowledged, result.Counts.AlreadyWas,
		result.Counts.StillOpen,
		result.Fetches, result.ReadFailures, result.VerifiedAgain)

	// ── 动手后：第三遍独立只读拉表 + 差集 ──
	after := liveProbeTableWords(ctx, opening, sink, "跑后回读")
	liveProbeSayWordDiff(sink, before, after, fired)
	recorder.sayTally()
	sink.say("耗时 %v ⇒ 三条读数怎么用：① 播报那些行就是界面上的原文；② 差集里「本趟没动它却变了」"+
		"那几条是 R24「没认 = 滞后」的读数；③ 传输层账本里 `doTask` 那一发是 R26 至今唯一的一手证据"+
		"（逐字要落进 docs/capture/，成文里只写键、值长度与 code）。",
		time.Since(started).Round(time.Second))
}

// TestLiveProbeMrpDoTaskShape 只补一问：**这一枪的响应里 `code` 是数字还是字符串**。
//
// 为什么不复用上面那一趟：整趟真跑（`TestLiveProbeRunAccountFull`）已经拿到过这一口的
// 形状读数（HTTP 200 + 顶层两键），但那份读数**答不了类型** —— 探针的打印机在
// 2026-09-26 之前把 JSON 数字 `0` 与字符串 `"0"` 打成同一个样子，而 R26 要求补的那一句
// 判定（"不判 `code`" ⇒ 改成判）恰好取决于它。打印机修好了 ⇒ 这一发只为把那一格
// 用**新打印机**复读一次。
//
// ⚠ 它**只发这一枪**（一发写），不带打卡、不带玩法、不拉表 ⇒ 它是整趟里那一发的
// 同一条请求，而不是"再跑一遍"（凭据边界与停止条件见 `docs/capture-guidelines.md`）。
// 出处：`internal/upstream/app_task_click.go` 文件头「零实拍」那一段，
// 判据登记在 `docs/pending-decisions.md` R26。
func TestLiveProbeMrpDoTaskShape(t *testing.T) {
	run, account, sink, recorder := liveProbeWorldRecording(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	executor := newTaskExecutor(run, opening, account)
	deviceID, derr := executor.accountDeviceID(ctx)
	if derr != nil {
		t.Fatalf("换不到 H5 `deviceId`（这一枪的必填参数）：%s", sink.mask(derr.Error()))
	}
	// 长度可以落，值不行：它是本仓上报设备指纹换来的那枚 id。
	sink.say("这一发带的 deviceId 长度 %d（值不打印）", len(deviceID))

	before := recorder.shots
	eerr := executor.entryReport(ctx, deviceID)
	sink.say("入口上报交回：%v", liveProbeErrWord(sink, eerr))
	// **按路径**数，不按总枪数：`entryReport` 走的是 [do]，而会话是**惰性**换的 ——
	// 第一次用时才补上换票那几枪（本轮实测：这一发之前先出了 3 枪会话侧的）。
	// 拿总数当"只发了 1 枪"的判据会把这句念成"多打了 3 发"，那是假红。
	hits := recorder.countFor(liveProbeMrpDoTaskPath)
	sink.say("这一趟 `doTask` 出了 %d 发（预期正好 1）｜整趟 %d 枪", hits, recorder.shots-before)
	if hits != 1 {
		sink.say("⚠ 上面那句不是 1 ⇒ 这一族的调用点比成文里写的多，要回查是谁发的")
	}
	sink.say("读数怎么用：上面那行 `枪[..] …/page/doTask …` 里的 `code=` 带不带「(数字)」，" +
		"决定 R26 那句判定怎么落（数字 ⇒ 与 `task/click` 同一份解析器；字符串 ⇒ 要单独解）。")
}

// liveProbeErrWord 把一个 *core.Error 念成掩码后的文本（nil 念成"没有错误"）。
func liveProbeErrWord(sink *liveProbeSink, err *core.Error) string {
	if err == nil {
		return "没有错误"
	}
	return sink.mask(err.Error())
}
