package app

// 活体探针：`hcy/file/list` 那一口到底吃哪几枚头（**只读，一发写都没有**）。
//
// # 这一族当初要收的那一格，以及它现在的答案
//
// 本仓在 `personal-kd-njs.yun.139.com` 上有两套装配，而它们的结果**不一致**：
//
//   - **写侧那一套**（[upstream.CloudFileOpHeaders]：`User-Agent` + `deviceid` +
//     `Authorization` + `x-yun-api-version` / `x-yun-app-channel` / `x-yun-op-type`
//     + 渠道三枚）—— `file/create`、`complete`、`recyclebin/batchTrash` 都**真打通过**
//     （2026-09-25 起多趟），同一台主机、同一个 `Authorization` 整串；
//   - **`file/list` 那一套**（iOS 那 21 枚 `WithAppHeaders` + `Authorization` +
//     `x-yun-module-type:100` + `x-yun-op-type:1`）—— 2026-09-26 三连拒，
//     原话是 `success=false: 请求头不合法(00010003)`
//     （AI 头像找回链、邮箱探针的回读、AI 相机探针的根目录计数，三个调用点各撞一次）。
//
// 后果不是"探针少一行读数"：`file/list` 是**按名字找产物**的唯一途径 ⇒
// AI 头像那条链的临时文件找回、以及"传上去的东西找不找得回"（R19 / R20）都挂在它上面。
//
// ⭐ **2026-09-26 三趟之后收口了：承重的是 `x-yun-client-info` 的那一串值。**
// 体（本仓那五键）与两套装配本身都不卡；把设备指纹串换成网盘侧那一串，
// iOS 那 21 枚立刻回 `code:"0000"` + 31 条，反方向则一字不差地回出 `(00010003)`。
// 落点与逐档读数见 `internal/upstream/app_ai_avatar.go` 的 [upstream.CloudFileListRequest]
// 与 `docs/pending-decisions.md` R16。
//
// # 为什么"是头还是体"当时不能只靠读代码分开（也是本文件留三个函数的理由）
//
// 那句错误措辞指头，而这一口的**体**也确实与实拍不同（本仓按旧 TS 发五键，
// `test/fixtures/http/ref.json` 那两条是另一个客户端发的十余键）⇒ 两件事同时可疑。
// 夹具能给的只有头名清单（实拍 19 枚，值全 `<REDACTED>`）：与本仓装配一比，
// **实拍有而本仓没有**的那一类里最扎眼的是 `x-yun-device-id`。
// 一比只能提出假设；判据只有把体固定住、只动一头，各发一发。
//
// 三个函数就是那三步，**每一步都只回答上一步留下的那一问**：
//
//   - [TestLiveProbeCloudFileListHeaders]：装配之间挑一个（补一枚头名 / 整套换）；
//   - [TestLiveProbeCloudFileListLeaveOneOut]：在那套通了的装配里**逐枚摘**，
//     问"哪几枚承重"（当时的答案是五枚：`Authorization` / `Content-Type` /
//     `x-yun-api-version` / `x-yun-app-channel` / `x-yun-client-info`）；
//   - [TestLiveProbeCloudFileListClientInfoValue]：承重的那五枚里，
//     iOS 那套**也全有** ⇒ 只剩"同一枚头的值"这一格，本函数把它单独问出来。
//
// 修好之后这三个函数不再是"找病因"，而是**复诊**：生产那一发（第一个函数的甲档）
// 应当读到条目，而第三个函数 A 档应当仍能把 `(00010003)` 造出来 ——
// 那一档是"这一枚值仍然承重"的证据，哪天它不红了，说明上游改了口径或本仓改回了设备串。
//
// # 边界
//
//   - 所有档都是**只读**（列根目录），不写用户网盘 ⇒ 不带额外开关，给了那四个 env 就跑；
//   - 头名可以念（值一律不行）：见过的凭据都进 [liveProbeSink] 的掩码表，
//     这一族打印头**清单**时只取名字；两串 client-info 里嵌着设备指纹，
//     所以本文件只念它们的**段数**，不念值。

import (
	"context"
	"sort"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// fileListProbePageSize 与生产路径那一档一致（旧 TS 固定 100）。
//
// 探针要换的只有头 ⇒ 体的字节必须逐字相同，否则"体也跟着变了"就混进了对照。
const fileListProbePageSize = 100

// TestLiveProbeCloudFileListHeaders 问的第一层：**装配之间**挑一个（URL 与体逐字相同，只动头）。
//
//   - 甲 **现装配**（生产路径那份）—— 2026-09-26 第一次跑它红（`(00010003)`），
//     那一格当场确认"可重复"；上游层改掉 `x-yun-client-info` 的值之后**重跑已过**（31 条）。
//   - 乙 现装配 **+** `x-yun-device-id: <真 deviceId>` —— 只补那一枚"实拍有、我们没有"的头名；
//   - 丙 现装配 **+** `deviceid: <真 deviceId>` —— 写侧那族用的是这个头名（不是 `x-yun-*`）；
//   - 丁 **整把换成写侧那套已在这台主机上跑通过的装配**（web 档 + `x-yun-module-type:100`）；
//   - 戊 / 己 从甲里**摘掉** `x-yun-uni` / `x-yun-client-info` —— "加一枚没用"之后，
//     摘是唯一还能保持体不变的对照方向。
//
// 第一次跑的读数：甲乙丙戊都回 `04000003 / 请求头不合法(00010003)`，
// 己回的是**不带子码**的 `请求头不合法`（那一格当时只是记下，第三轮才用上），
// 丁回 `code:"0000"` + 31 条。⇒ 单补一枚头名不够，"整套换"才通。
// 改掉那枚值之后重跑：甲乙丙丁戊**全通**，只有己（摘掉这枚）红 —— 头名与装配本来就不是变量。
func TestLiveProbeCloudFileListHeaders(t *testing.T) {
	run, account, sink, client := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	authorization := opening.Credential.Authorization()
	sink.see(authorization)
	sink.see(opening.Credential.Phone())

	deviceID, derr := run.DeviceIDFor(ctx, account)
	if derr != nil || deviceID == "" {
		// 乙丙丁三档都以那枚 deviceId 为对照物 ⇒ 拿不到就只留甲，并把"为什么只有一档"念出来。
		reason := "库里是空串"
		if derr != nil {
			reason = sink.mask(derr.Error())
		}
		sink.say("deviceId 拿不到（%s）⇒ 乙丙丁没有对照物，只发甲（现装配）", reason)
		deviceID = ""
	} else {
		sink.say("deviceId：换到了（长度 %d，值不打印）", len(deviceID))
	}

	base, berr := upstream.CloudFileListRequest(
		opening.Profile, UserTimestamp(run.Clock), authorization, "/", fileListProbePageSize)
	if berr != nil {
		t.Fatalf("甲那一发的请求就构造不出来：%s", berr.Error())
	}
	sink.say("甲：POST %s，体 %d 字节（五键，与生产同一段代码），头 %d 枚：%s",
		base.URL, len(base.Body), len(base.Headers), fileListProbeHeaderNames(base))

	arms := []struct {
		word        string
		req         core.HTTPRequest
		needsDevice bool
	}{
		{"甲 现装配（生产那份）", base, false},
		{"乙 +x-yun-device-id", fileListProbeWithHeader(base, "x-yun-device-id", deviceID), true},
		{"丙 +deviceid", fileListProbeWithHeader(base, "deviceid", deviceID), true},
		{"丁 换成写侧那套装配", fileListProbeWriteProfile(base, authorization, deviceID), true},
		// 戊己 是**第二轮**才加上来的：第一轮的结果是"补一枚头没用、整套换才通"，
		// 那一格只到"是这一套画像的问题"，**分不出是哪一枚**。摘掉法比加上去法更便宜：
		// 甲那份少一枚头仍是一次可比的读数（体没动）。
		{"戊 摘掉 x-yun-uni", fileListProbeWithoutHeader(base, "x-yun-uni"), false},
		{"己 摘掉 x-yun-client-info", fileListProbeWithoutHeader(base, "x-yun-client-info"), false},
	}
	for _, arm := range arms {
		if arm.needsDevice && deviceID == "" {
			sink.say("── %s：跳过（没有 deviceId 就没有对照物）", arm.word)
			continue
		}
		sink.say("── %s：头 %d 枚：%s", arm.word,
			len(arm.req.Headers), fileListProbeHeaderNames(arm.req))
		fileListProbeShoot(ctx, sink, client, arm.word, arm.req)
	}

	sink.say("读数怎么用（第一趟修法落地前：甲乙丙戊己全红、只有丁通 ⇒ 收窄到第二、三个函数；" +
		"改掉那枚值之后重跑：甲乙丙丁戊全通，只有己红）：" +
		"乙或丙绿而甲红 ⇒ 缺的是那一枚头名；只有丁绿 ⇒ 是这一套画像的问题，但仍分不出哪一枚；" +
		"四档全红 ⇒ 嫌疑回到体。⚠ 这几句是**用法**，只有当场念到的读数算数")
}

// fileListProbeShoot 发一发，把**形状**与"我们的解析器读出几条"分开念。
//
// 刻意不省解析那一趟：`success=false` 那句错误原文（00010003）走的是
// [upstream.Envelope.EnsureOK]，而"上游认了、我们的解析器读成 0 条"是另一种失败 ——
// 两者在界面上都表现为"没找到文件"，探针必须能分开它们。
//
// 返回值是"通没通"，让调用方能拿它当**闸门**：摘一枚头的对照只有在基准绿的时候才是读数。
func fileListProbeShoot(
	ctx context.Context, sink *liveProbeSink, client core.HTTPClient, word string,
	request core.HTTPRequest,
) bool {
	response, terr := client.Send(ctx, request)
	if terr != nil {
		sink.say("%s：传输层失败：%s", word, sink.mask(terr.Error()))
		return false
	}
	sink.dump(word+" 响应", response)
	envelope, eerr := upstream.ParseEnvelope(response.Body)
	if eerr != nil {
		sink.say("%s：信封解不出来：%s", word, sink.mask(eerr.Error()))
		return false
	}
	list, lerr := upstream.ParseCloudFileList(envelope)
	if lerr != nil {
		sink.say("%s：我们的解析器读不出：%s ⇒ 这一档**没通**（不是「上游回了、但目录是空的」）",
			word, sink.mask(lerr.Error()))
		return false
	}
	sink.say("%s：解析器读出 %d 条，下一页游标=%s ⇒ 这一档**通**",
		word, len(list.Items), fileListProbeCursorLabel(list.NextPageCursor))
	return true
}

// fileListProbeWithHeader 加一枚头。
//
// 先按大小写不敏感摘同名再加：甲那份今天不带这两枚，但"今天不带"不是这一档的前提 ——
// 同名两枚会让读数说不清是哪一根线起作用（同一顾虑见 `setCloudHeader` 那段）。
func fileListProbeWithHeader(request core.HTTPRequest, name, value string) core.HTTPRequest {
	if value == "" {
		return request
	}
	request.Headers = fileListProbeDropHeaders(request.Headers, name)
	return request.WithHeader(name, value)
}

// fileListProbeWithoutHeader 摘掉一枚头再发 —— 第一轮"加一枚没用"，那一格只到
// "整套画像不对"，而**摘**是唯一还能保持体不变的对照方向。
func fileListProbeWithoutHeader(request core.HTTPRequest, name string) core.HTTPRequest {
	request.Headers = fileListProbeDropHeaders(request.Headers, name)
	return request
}

// TestLiveProbeCloudFileListLeaveOneOut 在**绿的那一套**上逐枚摘头，问"哪一枚是承重的"。
//
// 第一轮（上面那个函数）的读数是"整套换过去才通，单补一枚没用" ⇒ 那一格只到
// "这一口吃写侧那套客户端画像"，仍**指不出是哪一枚**。这第二轮把方向反过来：
// 基准 = 丁那一套（必须当场再绿一次，否则下面每一档都不算读数），
// 然后每次只摘掉**一枚**头，其它一字不动（体照旧）。
//
// 读数怎么用：摘了还绿 ⇒ 那一枚**不是**承重（将来可以从生产装配里省掉）；
// 摘了就红 ⇒ 承重。两种读数对"要不要把这一整套抄进生产路径"都够用，
// 而只有后者会限制改动面。
//
// ⭐ 2026-09-26 那趟的读数：**承重五枚** = `Authorization`（04000005 认证失败）、
// `Content-Type`（04000018 请求体媒体格式不支持）、`x-yun-api-version`（**HTTP 404**，
// 它是路由不是装饰）、`x-yun-app-channel`、`x-yun-client-info`（后两枚回 04000003，
// **不带**子码）；**不承重六枚**（摘了仍通）= `User-Agent`、`deviceid`、
// `x-huawei-channelSrc`、`x-yun-device-id`、`x-yun-module-type`、`x-yun-op-type`。
//
// ⚠ 这一轮只到"哪几枚**在**"，**不**回答"哪一枚的值对" —— 而 iOS 那套五枚全有却仍被拒，
// 所以真因落在值上，那一问由下面第三个函数单独问。
func TestLiveProbeCloudFileListLeaveOneOut(t *testing.T) {
	run, account, sink, client := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	authorization := opening.Credential.Authorization()
	sink.see(authorization)
	sink.see(opening.Credential.Phone())

	deviceID, derr := run.DeviceIDFor(ctx, account)
	if derr != nil {
		sink.say("deviceId 读失败：%s", sink.mask(derr.Error()))
		deviceID = ""
	}
	if deviceID == "" {
		// 基准那一套里 `deviceid` 是 AiAvatarHeaders 在值非空时才加的 ⇒
		// 拿不到它，"摘哪一枚"这套对照就少了一根轴，读出来的"承重"名单不可信。
		sink.say("deviceId 是空串 ⇒ 基准少一枚头，本轮逐枚对照不发（先补库里的这一格再跑）")
		return
	}

	base, berr := upstream.CloudFileListRequest(
		opening.Profile, UserTimestamp(run.Clock), authorization, "/", fileListProbePageSize)
	if berr != nil {
		t.Fatalf("请求就构造不出来：%s", berr.Error())
	}
	green := fileListProbeWriteProfile(base, authorization, deviceID)
	names := fileListProbeHeaderNameList(green)
	sink.say("基准（写侧那一套）：头 %d 枚：%s", len(names), strings.Join(names, " "))
	if !fileListProbeShoot(ctx, sink, client, "基准 全套", green) {
		// 基准不绿时后面每一档的"红"都不说明任何事 —— 直接停，别让 11 行红读数冒充结论。
		t.Fatalf("基准没绿 ⇒ 后面每一档的读数都不成立（这一族今天的状态与第一轮不同，先看上面那行）")
	}
	for _, name := range names {
		arm := fileListProbeWithoutHeader(green, name)
		sink.say("── 摘掉 %s（剩下 %d 枚）", name, len(arm.Headers))
		if fileListProbeShoot(ctx, sink, client, "摘掉 "+name, arm) {
			sink.say("摘掉 %s：仍**通** ⇒ 这一枚不是承重", name)
		} else {
			sink.say("摘掉 %s：红 ⇒ 这一枚**承重**（本轮唯一的判据是「摘了就红」这一条）", name)
		}
	}
}

// TestLiveProbeCloudFileListClientInfoValue 问的是**值**而不是头名。
//
// # 它当初要问出来的那一格
//
// 第二轮"摘一枚头"的读数把范围收成了五枚承重头
// （2026-09-26 那趟：`Authorization`→04000005、`Content-Type`→04000018、
// `x-yun-api-version`→**HTTP 404**、`x-yun-app-channel` 与 `x-yun-client-info`→04000003），
// 而 iOS 那套装配**五枚全有**却仍被拒 ⇒ 剩下的差别只能落在"同一枚头的值"上。
//
// 最可疑的是 `x-yun-client-info`：两轮的错误原话其实**不是一句**——
// iOS 那套回的是 `请求头不合法(00010003)`（带子码），
// 而把已通的那套里的这枚摘掉回的是 `请求头不合法`（不带子码）。
// "带了但不认"与"压根没带"分得开 ⇒ 换值对照就按这条分界做。
//
// ⭐ 那一趟的读数（四档，2026-09-26）：同一套 iOS 头**只**把这一枚换成网盘侧那一串 ⇒
// **通**（31 条）；再补 `x-yun-device-id` 也通（说明它既不承重也不挡）；
// 反方向（已通的写侧那套 + 设备指纹串）回的正是 `(00010003)`；
// 而把 `x-yun-app-channel` 换成 iOS 的渠道号仍**通** ⇒ 那一枚只认"在不在"、不认值，
// 所以旧 TS 的 `10214200`（`api.ts:88`）本仓**不照抄**（没有读数支持第三个取值）。
//
// # 修好之后这几档是"复诊"：基准 = **生产那份**
//
//   - 基准 生产那份 —— 应当**读到条目**；红了就是这一口又挡死三条链；
//   - A 生产 + 换成**设备指纹串** —— 应当回 `(00010003)`：这是"改回旧写法会怎样"的
//     反向读数，也是本文件唯一能当场**造出旧症状**的一档；
//   - B 生产 + **摘掉**这一枚 —— 应当回不带子码的那句 ⇒ 子码分界仍在；
//   - C 写侧那套 + 换成设备指纹串 —— 应当也回 `(00010003)` ⇒ 承重的是**值**，
//     与用哪套装配无关（这一档要是绿了，说明真因不在值上，R16 得重新打开）。
//
// ⚠ 两串 client-info 都是**设备指纹**（设备那一串里嵌 deviceId 与 deviceShort），
// 所以本函数一个字都不打印它们的值：只念"换成哪一串"和读数。
func TestLiveProbeCloudFileListClientInfoValue(t *testing.T) {
	run, account, sink, client := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	authorization := opening.Credential.Authorization()
	sink.see(authorization)
	sink.see(opening.Credential.Phone())

	deviceID, derr := run.DeviceIDFor(ctx, account)
	if derr != nil {
		sink.say("deviceId 读失败：%s", sink.mask(derr.Error()))
		deviceID = ""
	}
	if deviceID == "" {
		sink.say("deviceId 是空串 ⇒ C 档（写侧那套装配）凑不齐，只发基准与 A/B")
	}

	production, berr := upstream.CloudFileListRequest(
		opening.Profile, UserTimestamp(run.Clock), authorization, "/", fileListProbePageSize)
	if berr != nil {
		t.Fatalf("请求就构造不出来：%s", berr.Error())
	}
	deviceClientInfo := opening.Profile.Device().ClientInfo()
	sink.see(deviceClientInfo)
	sink.see(deviceID)
	// 生产那一份**必须**带这一枚（上游层是覆盖式改写的），否则换值档没有对照物。
	webClientInfo, hasWeb := fileListProbeHeaderValue(production, "x-yun-client-info")
	if !hasWeb {
		t.Fatal("生产那一份里没有 x-yun-client-info ⇒ 复诊问不成（先看上游层那一行覆盖有没有被改坏）")
	}
	if webClientInfo == deviceClientInfo {
		// 两份素材串万一哪天被人改成同一串，A/C 两档就变成空操作，读数会**假绿**。
		t.Fatal("两份 client-info 素材串相等 ⇒ A/C 两档是空操作，本轮读数不成立")
	}
	sink.say("两串 client-info 的段数：设备那一串 %d / 网盘那一串 %d（值不打印，已登记进掩码表）",
		strings.Count(deviceClientInfo, "|"), strings.Count(webClientInfo, "|"))

	if !fileListProbeShoot(ctx, sink, client, "基准 生产那份", production) {
		// 基准红**不**停下：后面三档在"这一口又坏了"的世界里照样有意义（要看的正是子码在不在）。
		sink.say("基准红 ⇒ 生产路径今天就不通；下面三档用来分「值不认（带子码）」与「别的事」")
	}

	type arm struct {
		word string
		req  core.HTTPRequest
	}
	arms := []arm{
		{"A 生产 + 换成设备指纹串（反向：旧症状应当还能造出来）",
			fileListProbeWithHeader(production, "x-yun-client-info", deviceClientInfo)},
		{"B 生产 + 摘掉这枚（子码分界应当还在）",
			fileListProbeWithoutHeader(production, "x-yun-client-info")},
	}
	if deviceID != "" {
		arms = append(arms, arm{"C 写侧装配 + 换成设备指纹串（承重的是值，不是装配）",
			fileListProbeWithHeader(
				fileListProbeWriteProfile(production, authorization, deviceID),
				"x-yun-client-info", deviceClientInfo)})
	}
	for _, a := range arms {
		sink.say("── %s：头 %d 枚", a.word, len(a.req.Headers))
		fileListProbeShoot(ctx, sink, client, a.word, a.req)
	}
	sink.say("读数怎么用：基准通而 A 回 (00010003) ⇒ 这一枚值仍承重、修法没坏；" +
		"基准红而 B 回不带子码的那句 ⇒ 病不在值上，R16 得重新打开。" +
		"⚠ 这两句是**用法**，只有当场念到的读数算数")
}

// fileListProbeHeaderValue 按名（大小写不敏感）取**第一枚**的值。
//
// 本函数只用它取 web 那一串 client-info 来喂换值档；取到的值一律不打印。
func fileListProbeHeaderValue(request core.HTTPRequest, name string) (string, bool) {
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1], true
		}
	}
	return "", false
}

// fileListProbeWriteProfile 把甲那份的**头整把换成写侧那套**，URL 与体一个字不动。
//
// 先清空 `Headers` 是这一档的全部意义：两套装配不能叠着发，叠上去之后
// "到底是哪一枚起作用"又分不开了。`x-yun-module-type:100` 是旧 TS 在这一口上带的那枚、
// 实拍那 19 枚里也有 ⇒ 换装配时把它留着，免得连这一枚一起当成变量。
func fileListProbeWriteProfile(
	request core.HTTPRequest, authorization, deviceID string,
) core.HTTPRequest {
	swapped := request
	swapped.Headers = nil
	return upstream.CloudFileOpHeaders(
		swapped, authorization, deviceID, upstream.CloudChannelWeb, upstream.CloudOpUpload).
		WithHeader("x-yun-module-type", "100")
}

// fileListProbeHeaderNames 念出头**名**清单（只名不值）—— 这一族要收的就是"带了哪几枚"。
func fileListProbeHeaderNames(request core.HTTPRequest) string {
	return strings.Join(fileListProbeHeaderNameList(request), " ")
}

// fileListProbeHeaderNameList 是清单的**结构化**那一版：摘一枚头的对照要按它逐枚展开，
// 所以不能只有一份拼好的字符串。排序是为了让两趟读数逐行可比（map 顺序会变）。
func fileListProbeHeaderNameList(request core.HTTPRequest) []string {
	names := make([]string, 0, len(request.Headers))
	for _, pair := range request.Headers {
		names = append(names, pair[0])
	}
	sort.Strings(names)
	return names
}

func fileListProbeDropHeaders(pairs []core.HeaderPair, name string) []core.HeaderPair {
	out := make([]core.HeaderPair, 0, len(pairs))
	for _, pair := range pairs {
		if !strings.EqualFold(pair[0], name) {
			out = append(out, pair)
		}
	}
	return out
}

// fileListProbeCursorLabel 把 `*string` 念成"缺 / 有（长度）"。
//
// `nextPageCursor` 实拍两条里都有这一格、值可能是 `null`，而"有没有下一页游标"决定
// "这一页 100 条是不是根目录的全部"（本族**不翻页**，理由见 [upstream.CloudFileList]）。
// 长度而不是值：游标是上游给的不透明串，本仓纪律是成文里不出现它的原文。
func fileListProbeCursorLabel(value *string) string {
	if value == nil {
		return "缺"
	}
	if *value == "" {
		return "有，空串"
	}
	return "有，长度 " + strings.Repeat("·", min(len(*value), 12))
}
