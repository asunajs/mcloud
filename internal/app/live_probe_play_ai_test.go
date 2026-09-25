package app

// 585「AI 相机」那一族的**活体探针**（与 `live_probe_test.go` 里邮箱那一族同一套边界）。
//
// 三条边界照旧，一条都不松：默认跳过（四个 env 全给才动）、只读**库副本**、
// 所有打印过 [liveProbeSink.mask]。本文件**没有断言** —— 它是取读数的仪器，
// 判据落在 `docs/status.md` 与 `docs/pending-decisions.md`（R37）。
//
// # 这一族要收的五格（每一格都只在真跑时才答）
//
//   - **① 用哪一枚凭据**（这一格被真跑否过两次：带签到页那张票 ⇒ `01000002 鉴权失败`；
//     带 Basic 第三段那枚裸 authToken ⇒ **同样** `01000002`。实拍里 ①② 逐字同一枚整串）。
//     所以探针**先把这一格念出来再发枪**：库里没有整串时这一趟直接停在那里，
//     别让"两枪都没动"被读成"上游还在拒"。纠正全程成文在 `task_matrix_play_ai.go` 文件头。
//   - **② 的真机读数**：har 里已有**两条**完整 SSE 实拍（47 行与 386 行，登记在
//     `docs/capture/app-play-ai-samples.txt` §8）⇒ "上游回什么形状"这一格有出处了。
//     **本程序自己发出去那一发**也有读数了（2026-09-26 两趟：`档=data 行、code=0000`，
//     第二趟带 `模型回话=` 那一句）⇒ 这一格收口，现场在 `docs/status.md`。
//     探针仍走生产那一趟（[taskExecutor.runAICamera]），播报行本身就带
//     "命中哪一档 + data 行数 + 模型回话 / 截断后的原文"⇒ 这一格不用另发一枪。
//     ⚠ 两趟的 `data 行数` 不同（33 / 41）而问句与样图同一份 ⇒ **行数不是稳定的量**，
//     别把"这次多少行"当判据；它只说明"这一趟上游切了这么多片"。
//     ⚠ ② 只发**一次**：再问一句等于让模型多答一次，而它不答"这一族还有别的什么问题"。
//   - **样图前缀比不比**（R37）：调用点选的是 `data:image/jpg;base64,…` 那一档
//     （唯一有实拍支撑的形态），旧 TS 传的是裸 base64 ⇒ 对照档只补发 ①，
//     且**凭据与生产那一发同一枚**（上一趟两格一起变，结果两问都没收到答案）。
//     ⚠ 第二发的读数**不能单独归因**给样图形态：同一趟里第二发还多吃了"同名 / 限流"
//     那一类可能，所以两句要一起看，探针把这句话也打出来。
//   - **`userId` 是否与票同源**（R37）：本仓用库里那份 `user_domain_id`，旧实现从票里解。
//     探针在**同一趟**里把**生产路径那张票**的载荷解出来（一次 base64url，不发请求），
//     只念"载荷里有哪些键 / 那一格的长度 / 与库里那份是否相等"，两个值都不进日志。
//   - **① 到底写不写网盘 + 链尾删不删得掉**：`uploadToCloud` 我们恒 `true`（实拍那一档），
//     而"真跑之后用户网盘里多没多一张图"没人核过。所以探针在前后各读一次根目录那一页
//     做**粗读**，并且**自己清**（探针不调整趟 [TaskMatrixRun.RunAccount]，
//     那条链尾的 `batchTrash` 不会自动跑）。

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// TestLiveProbeAICameraFamily 让 585 那两枪各出一次真实反馈（**含写网盘**）。
func TestLiveProbeAICameraFamily(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())
	executor := newTaskExecutor(run, opening, account)

	// ── 前置：库里那份 userDomainId 到底有没有
	//
	// 没有 ⇒ 这一层**一枪都不发**（"填错会打到别人的账号上"那道守卫）。那一档本身
	// 也是读数：它说明这个号的库里缺哪一格，而 R37 剩下的三格今天取不到。
	domain := opening.Profile.UserDomainID()
	if domain == nil || *domain == "" {
		sink.say("库里没有 userDomainId ⇒ ①② 都不该发（这一层的守卫挡住了），剩下的三格今天没有读数")
		t.FailNow()
	}
	sink.say("库里那份 userDomainId：长度 %d（值不打印；它是标识不是凭据，同一纪律）", len(*domain))

	// ── 前置：两枪共用的那枚凭据在不在
	//
	// 前两趟真跑分别卡在这两枚"看着都对"的凭据上：签到页那张票 ⇒ `01000002 鉴权失败`、
	// Basic 第三段那枚裸 authToken ⇒ **同样** `01000002`（纠正全程成文在
	// `task_matrix_play_ai.go` 文件头第一条）。所以这一格要先念：
	// 它为空 ⇒ 这一层的守卫让两枪都不发，而**不该**被读成"上游还在拒我们"。
	authorization := opening.Credential.Authorization()
	sink.see(authorization)
	if authorization == "" {
		sink.say("①② 的凭据：库里没有 Authorization 整串 ⇒ 两枪都不该发（本工具侧缺格）")
		t.FailNow()
	}
	sink.say("①② 的凭据：同一个 Authorization 整串，长度 %d（值与内容都不打印）",
		len(authorization))

	// ── R37 之一：票的载荷里有没有同一格（**不发请求**，只解已经拿到的那张票）
	//
	// ⚠ 这一发**不是**在测"① 用哪张票"—— 本族两枪都不带票（见上面那一格）。它只回答
	// R37 那一句"旧实现从票里解出来的 `userId`，与库里那份同不同源"，而那张票要拿到
	// 只能走生产路径的会话源（通用前置打卡用的就是它）。
	// 走 [do] 而不是自己建会话：拿的是**生产路径同一张**票（池里那一枚），
	// 比出来的"相等"说的才是"这一趟用的那两枚同不同源"。
	_, perr = do(ctx, executor.src, func(s SignInSession) (struct{}, *core.Error) {
		sink.see(s.jwt)
		keys, value, found := liveProbeJWTPayload(s.jwt)
		sink.say("票的载荷：键 %s（只有键名，值不外带）", keys)
		if !found {
			sink.say("载荷里**没有** userDomainId 这一格 ⇒ 旧实现那句 `parseJwtUserDomainId` " +
				"今天在这张票上取不到东西（这本身就是读数）")
			return struct{}{}, nil
		}
		sink.see(value)
		sink.say("载荷里那一格：长度 %d、与库里那份**相等=%v**（两个值都不打印）"+
			"⇒ R37 这一格的答案只到\"这一趟这两枚同不同源\"，不是\"恒等\"", len(value), value == *domain)
		return struct{}{}, nil
	})
	if perr != nil {
		sink.say("拿票这一步失败：%s ⇒ R37 那一格今天没有读数", sink.mask(perr.Error()))
	}

	// ── 链尾回读的"前"（粗读：根目录那一页的项数）
	before := liveProbeCloudRootCount(ctx, run, opening, sink, "① 之前")

	// ── 生产路径那一趟：①(data URL) → ②（播报与记账与真跑完全一致）
	//
	// 这一趟的播报行（`AI 相机对话已回 …：档=…、data 行数=…`）就是本族**第一条**
	// ② 的真实读数 ⇒ 不再另发对照枪。
	item := liveProbeTaskItem(585, "AI 相机：拍照问一句")
	record := &TaskStepRecord{TaskID: 585}
	beforeIDs := len(executor.cloudFileIDs)
	fired, problems := executor.runAICamera(ctx, item, record)
	sink.say("①② 走完整条玩法（585）：发过枪=%v 失败 %d 条 记账上传 %d 次 新增 fileId %d 枚",
		fired, len(problems), record.Uploads, len(executor.cloudFileIDs)-beforeIDs)
	for _, problem := range problems {
		sink.say("①② 原因：%s", sink.mask(problem))
	}

	// ── R37 之二：样图**裸 base64** 那一档（旧 TS 的形态）与上面那一发对照。只补发 ①。
	//
	// ⚠ 凭据必须与生产那一发**同一枚**（库里那串 `Basic …` 整串，直发、不经过会话源）：
	// 上一趟的教训是"样图形态"与"凭据形态"两格同时变，结果两问都没收到答案。
	recognized, rerr := run.Upstream.PlayAIRecognize(
		ctx, opening.Profile, UserTimestamp(run.Clock), authorization, randomHex(run.Random, 8),
		upstream.PlayAIRecognizeParams{
			UserID:   *domain,
			DataURL:  upstream.PlayAIChatCameraSampleBase64,
			ImageExt: upstream.PlayAIRecognizeImageExt,
		})
	if rerr != nil {
		sink.say("① 对照（裸 base64）失败：%s", sink.mask(rerr.Error()))
		sink.say("① 对照的边界：这一句**不能单独归因**给样图形态 —— 同一趟里第二发还可能是" +
			"同名/限流那两类。要分开得换一天或换个号只发这一发")
	} else {
		sink.say("① 对照（裸 base64）：成功。fileId %s、taskId %s、hasText=%s、推荐问句 %d 条（值不打印）",
			liveProbeOptLen(recognized.FileID), liveProbeOptLen(recognized.TaskID),
			liveProbeOptBool(recognized.HasText), len(recognized.Recommend))
		if recognized.FileID != nil && *recognized.FileID != "" {
			sink.see(*recognized.FileID)
			executor.cloudFileIDs = append(executor.cloudFileIDs, *recognized.FileID)
		} else {
			sink.say("① 对照说成功却**没给 fileId** ⇒ 这就是 R37 里\"删不掉\"那一档：" +
				"网盘里可能多了一张图而账上没有，链尾清不到")
		}
	}

	// ── 链尾清理（探针自己清：整趟那条清理枪在 RunAccount 末尾，这里没跑到）
	uploader, uerr := executor.cloudUploader(ctx)
	switch {
	case uerr != nil:
		sink.say("清理没做：拿不到上传器（%s）⇒ 网盘里那 %d 份要手工清",
			sink.mask(uerr.Error()), len(executor.cloudFileIDs))
	default:
		sent, derr := uploader.DeleteCloudFiles(ctx, executor.cloudFileIDs)
		switch {
		case derr != nil:
			sink.say("链尾清理失败（发过枪=%v）：%s ⇒ 可能留着 %d 份，要手工清",
				sent, sink.mask(derr.Error()), len(executor.cloudFileIDs))
		case sent:
			sink.say("链尾清理：一次 batchTrash，覆盖这一趟记下的 %d 份（id 不打印）",
				len(executor.cloudFileIDs))
		default:
			sink.say("链尾清理：没有要删的 ⇒ 两发 ① 都没写出 fileId（与\"① 失败\"是两档，别并成一句）")
		}
	}
	after := liveProbeCloudRootCount(ctx, run, opening, sink, "清理之后")
	if before >= 0 && after >= 0 && before != after {
		sink.say("粗读对照：根目录那一页 %d 项 ⇒ %d 项。⚠ 差值**不**等于\"我们造了几份\"："+
			"这一页可能同时有别人的写入，且只覆盖第一页（回收站里的不算）", before, after)
	}
	sink.say("边界：① 若说成功却没给 fileId，那张图**不在**上面那条清理链上（R37）" +
		"⇒ 这一份读数不代表\"什么都没留下\"")
}

// liveProbeCloudRootCount 读根目录**第一页**的项数（只念数量，名字不念：
// 文件名里可能带用户内容）。返回 -1 表示这一发没做成。
//
// ⚠ 三条边界与邮箱探针那一段完全相同：只第一页、回收站里的不算、差值不自动等于缺陷。
func liveProbeCloudRootCount(
	ctx context.Context, run TaskMatrixRun, opening SignInOpening, sink *liveProbeSink, label string,
) int {
	page, lerr := run.Upstream.AppCloudFileList(ctx, opening.Profile,
		UserTimestamp(run.Clock), opening.Credential.Authorization(), "/", 100)
	if lerr != nil {
		sink.say("%s：根目录那一页没读到（%s）⇒ 这一格只剩 batchTrash 那句上游原话",
			label, sink.mask(lerr.Error()))
		return -1
	}
	sink.say("%s：根目录这一页共 %d 项（只到\"第一页\"，回收站里的不算）", label, len(page.Items))
	return len(page.Items)
}

// liveProbeJWTPayload 把票的第二段（base64url 的载荷）解成 JSON，回**键名清单**与
// `userDomainId` 那一格的值。
//
// 只在探针里用：生产代码**没有**"解 JWT 载荷"那一口（全仓零实现，成文见
// `task_matrix_play_ai.go` 文件头），所以这一份是"为了回答 R37 而临时读一次"，
// 不许被将来当成"我们实现了同源取法"的证据。
// 解不开**不猜**：回一句解不开的形状，那本身就是 R37 的读数。
func liveProbeJWTPayload(jwt string) (keys string, value string, found bool) {
	segments := strings.Split(jwt, ".")
	if len(segments) < 2 {
		return "票不是三段式（实得 " + strconv.Itoa(len(segments)) + " 段）", "", false
	}
	raw, err := base64.RawURLEncoding.DecodeString(segments[1])
	if err != nil {
		// 有的票载荷带填充 ⇒ 再试一次带填充的解法，两种都不行才算"解不开"。
		raw, err = base64.URLEncoding.DecodeString(segments[1])
		if err != nil {
			return "载荷不是 base64url", "", false
		}
	}
	var root map[string]any
	if json.Unmarshal(raw, &root) != nil {
		return "载荷不是 JSON 对象", "", false
	}
	names := make([]string, 0, len(root))
	for name := range root {
		names = append(names, name)
	}
	sort.Strings(names)
	joined := strings.Join(names, " ")
	// 载荷里那一格可能是字符串也可能是数字（两种形态本仓都没样本）⇒ 都摊成串再比。
	switch typed := root["userDomainId"].(type) {
	case string:
		return joined, typed, true
	case json.Number:
		return joined, typed.String(), true
	case float64:
		return joined, strconv.FormatFloat(typed, 'f', -1, 64), true
	}
	return joined, "", false
}

// liveProbeOptLen 把"可能没给"的那格念成**长度**（值不进日志）。
func liveProbeOptLen(value *string) string {
	if value == nil {
		return "没给"
	}
	if *value == "" {
		return "空串"
	}
	return "长度 " + strconv.Itoa(len(*value))
}

// liveProbeOptBool 分开"`false` 是真实取值"与"上游没给这一格"。
func liveProbeOptBool(value *bool) string {
	if value == nil {
		return "没给"
	}
	return strconv.FormatBool(*value)
}
