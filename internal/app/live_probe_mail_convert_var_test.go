package app

// 活体探针：⑥（`disk:convertPdfFromYunFile`）**另存的那一份**，它的 `fileId` 能不能不列目录就拿到。
//
// # 为什么这一发现在才问
//
// 同一天早先那一发（`live_probe_mail_pdf_r36_test.go`）把"转换产物在不在网盘里"答成了
// **在**：根目录那一页不多第二份，但**深度 3** 的只读走查在两枚测试号里都念出了
// `/139邮箱/我的附件/PDF转换` 下的 `asign-pdf-<ms>.docx`，其中一枚的名字里的毫秒数
// **正好等于**带写那一发上传素材时用的那个时间戳 ⇒ ⑥ 确实另存一份，且这份**不在**
// 链尾那条 `batchTrash` 的账上（素材被移走了，产物还在）。
//
// 于是 R36 剩下的那一问从"有没有残留"变成"**要不要接清理、接得上吗**"：
// 删得要 `fileId`，而 ⑥ 的响应本仓**只判 `code`、不读 `var`**（旧 TS 同样不读）。
// 本发只问一件事：**`var` 里有没有那一枚产物 `fileId`**。
//
// # 判据怎么读才不越界
//
//   - **绝不打印 `var` 里的任何值**：这一族的响应体里塞过会话凭据（`ParseMailSession`
//     那一口的失败文本因此被要求"先抹再拼"），所以只念**键路径 + 值形态 + 长度**；
//     产物与素材的 `fileId` 一律走 [liveProbeSink.see]（只念长度）。
//   - "有没有"用**相等比对**回答：先把目录里那一枚产物的 `fileId` 拿到，再拿它去和
//     `var` 的每一枚叶子值比，只念"**哪一条路径与它相等**"。零样本处不猜语义。
//   - 目录那一头按**名字精确匹配**取产物（`asign-pdf-<ms>.docx`，干名与素材同一个
//     毫秒数）：`asign-` 前缀是我们自己造的 ⇒ 这是"认我们这一趟造的那一枚"，
//     不是"按前缀扫一片去删"（后者仍是禁止的，见 R36）。
//   - 链尾把**素材 + 产物一起**交给一次 `batchTrash`，然后回读那个目录：
//     产物名字消失 ⇒ "列目录取 fileId 再删"这条路走得通（那才是接清理的依据）。
//
// 边界：① 转换可能**晚于**这一次比对才落盘 ⇒ 只读那一头最多轮询若干次，念的是
// "在这个等待窗口里有没有"。窗口从 60 秒加到约 7 分钟，是因为 60 秒那一档**已经**
// 出现过一次"⑥ 回成功、目录里没有"的读数（第一趟的产物是在写入后约 22 分钟那一次
// 只读走查里第一次看到的）——等待窗口要按"产物可能什么时候出现"设，不按"跑得快"设；
// ② 每个目录只看第一页（与生产同档 `pageSize`）；
// ③ 回收站里的不算（`batchTrash` 的语义就是移进回收站，所以"名字还在"与"已删"
// 的分别只在**列目录还读不读得到**这一层）。

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// 那三枚目录名是**上游自己的系统目录**（2026-09-26 只读走查在两枚测试号里同型读到），
	// 写死在探针侧只为把 `fileId` 一级级导航出来；**不进**生产层（生产不认路径，只认 fileId）。
	liveProbeMailFolderRoot     = "139邮箱"
	liveProbeMailFolderAttach   = "我的附件"
	liveProbeMailFolderPDFTrans = "PDF转换"

	liveProbeConvertPolls     = 20
	liveProbeConvertPollDelay = 20 * time.Second
)

// liveProbeLeafValue 是响应里的一枚**叶子**：路径、形态、原值（原值只在内存里用于相等比对）。
type liveProbeLeafValue struct {
	path  string
	kind  string
	value any
}

// liveProbeFlatten 递归摊平响应，遇到 map/slice 继续走，其余记成叶子。
//
// 路径里的键名**可以**念（`code` / `var` / `result` 这类形状信息就是本发的判据），
// 值**不可以**（这一族塞过票）。数字是 [json.Number]（`core.ParseJSON` 开了 UseNumber）。
func liveProbeFlatten(path string, value any, leaves *[]liveProbeLeafValue, keys *[]string) {
	switch typed := value.(type) {
	case map[string]any:
		names := make([]string, 0, len(typed))
		for key := range typed {
			names = append(names, key)
		}
		sort.Strings(names)
		for _, key := range names {
			child := path + "." + key
			*keys = append(*keys, child)
			liveProbeFlatten(child, typed[key], leaves, keys)
		}
	case []any:
		for i, element := range typed {
			liveProbeFlatten(fmt.Sprintf("%s[%d]", path, i), element, leaves, keys)
		}
	default:
		if path == "" {
			return
		}
		*leaves = append(*leaves, liveProbeLeafValue{path: path, kind: fmt.Sprintf("%T", typed), value: typed})
	}
}

// liveProbeLeafKindWord 念一枚叶子的**形态**：字符串与数字只念长度（值可能是凭据，
// 也可能是将来对上的那枚 fileId），布尔可以念字面（`true` / `false` 装不下一张票）。
func liveProbeLeafKindWord(leaf liveProbeLeafValue) string {
	switch typed := leaf.value.(type) {
	case string:
		return fmt.Sprintf("字符串 长度 %d", len([]rune(typed)))
	case json.Number:
		return fmt.Sprintf("数字 长度 %d", len([]rune(typed.String())))
	case bool:
		return fmt.Sprintf("布尔 %v", typed)
	case nil:
		return "null"
	default:
		return leaf.kind
	}
}

// liveProbeLeafByPath 按路径取一枚叶子的原文（只给 `code` 这种**业务码**用；`var.*` 不走这里）。
func liveProbeLeafByPath(leaves []liveProbeLeafValue, path string) (string, bool) {
	for _, leaf := range leaves {
		if leaf.path == path {
			return liveProbeLeafText(leaf)
		}
	}
	return "", false
}

// liveProbeLeafText 交回一枚叶子的**原文**，只用于内存里的相等比对（不进日志）。
//
// 数字也参与比对：`fileId` 今天读到的是串，但这一族"回串 / 回数"两档都见过（坑表 #176），
// 只比字符串会让"相等"这一格假阴性。
func liveProbeLeafText(leaf liveProbeLeafValue) (string, bool) {
	switch typed := leaf.value.(type) {
	case string:
		return typed, true
	case json.Number:
		return typed.String(), true
	}
	return "", false
}

// liveProbeMailBodyValue 解这一族两种写法（标准 JSON 与单引号那份旧写法）。
//
// ⚠ 与本仓上游层 `parseMailJSONText`（非导出）同一取向：探针不复用它，也不为探针把它
// 导出 —— 导出面是给生产判据用的，这里只需要"解得开就摊平"。
func liveProbeMailBodyValue(text string) (any, bool) {
	if value, ok := core.ParseJSON(text); ok {
		return value, true
	}
	return core.ParseJSON(strings.ReplaceAll(text, "'", `"`))
}

// liveProbeChildID 在一个目录的读数里按**精确名字**找一枚，找不到交回空串。
func liveProbeChildID(items []liveProbeRawItem, name string) string {
	for _, item := range items {
		if item.name == name {
			return item.fileID
		}
	}
	return ""
}

// TestLiveProbeMailConvertVarFileID 传素材 → ⑥（自己发，为了拿得到原始响应）→
// 在 `/139邮箱/我的附件/PDF转换` 里找到那一枚产物 → 比对 `var` 有没有它的 fileId →
// 一次 `batchTrash` 收素材 + 产物 → 回读那个目录。
func TestLiveProbeMailConvertVarFileID(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	executor := newTaskExecutor(run, opening, account)

	session, fired, problems := executor.mailSessionFor(ctx)
	if len(problems) > 0 {
		sink.say("①② 那一档没过（发过枪=%v）：%s ⇒ ⑥ 没有会话可发，这一趟停在这里",
			fired, sink.mask(problems[0]))
		return
	}
	sink.say("邮箱会话已取到（sid/rmkey 只念长度：sid=%d rmkey=%d）",
		len([]rune(session.SID.Value())), len([]rune(session.RMKey.Value())))

	// ⑤ 是**只读**的一口（转换权益剩余次数）。本发把它打在 ⑥ **前后**各一次：
	// 第一趟（10:08）⑥ 回 `S_OK` 且三分钟后目录里多出一枚产物，第二趟（10:59）同样回成功、
	// 目录里却**没有**这一趟的那一枚 ⇒ "⑥ 认了但没产出"需要一个解释，而 ⑤ 是本仓手里
	// 唯一一口"上游自己说转换还剩几次"。⚠ 这两格本仓生产层今天**没人读**（`AppMailSurplusNum`
	// 只有上游层与测试），这里念值不越界：它是计数与状态码，不是凭据。
	surplusCguid, surplusRnd := mailDiskRandoms(executor.run.Random)
	if count, status, serr := run.Upstream.AppMailSurplusNum(
		ctx, session, surplusCguid, surplusRnd); serr != nil {
		sink.say("⑤ 转换权益（⑥ 之前）没读到：%s", sink.mask(serr.Error()))
	} else {
		sink.say("⑤ 转换权益（⑥ 之前）：benefitSurplusCount=%q benefitStatus=%q",
			liveProbeClip(count, 24), liveProbeClip(status, 24))
	}

	ts, ok := executor.nowMillis()
	if !ok {
		sink.say("时钟没接 ⇒ 造不出素材名，不发写枪")
		return
	}
	uploader, uerr := executor.cloudUploader(ctx)
	if uerr != nil {
		sink.say("拿不到上传器：%s", sink.mask(uerr.Error()))
		return
	}
	fileName, content := mailPDFAsset(ts)
	receipt, perr := uploader.UploadContent(ctx, content, CloudUploadOptions{Name: fileName})
	if receipt.FileID != "" {
		sink.see(receipt.FileID)
		executor.cloudFileIDs = append(executor.cloudFileIDs, receipt.FileID)
	}
	if perr != nil {
		sink.say("素材没传上去：%s ⇒ ⑥ 不发", sink.mask(perr.Error()))
		return
	}
	if receipt.FileID == "" {
		sink.say("素材没有 fileId（同名已存在 / 秒传没回 id）⇒ ⑥ 不发")
		return
	}
	sink.say("素材已上传：%s（毫秒数 %d ⇒ 产物若另存，名字里的毫秒数就是它）", fileName, ts)

	cguid, rnd := mailDiskRandoms(executor.run.Random)
	request, perr := upstream.MailConvertPDFRequest(
		session, cguid, rnd, receipt.Name, receipt.FileID, int64(len(content)),
		upstream.MailPDFConvertType)
	if perr != nil {
		sink.say("⑥ 请求构造失败：%s", sink.mask(perr.Error()))
		return
	}
	response, perr := run.Upstream.Raw(ctx, request)
	if perr != nil {
		sink.say("⑥ 传输失败：%s", sink.mask(perr.Error()))
		return
	}
	sink.say("⑥ 原始响应：状态 %d，体 %d 字节", response.Status, len(response.Body))
	body, parsed := liveProbeMailBodyValue(string(response.Body))
	if !parsed {
		sink.say("⑥ 的体解不开（标准 JSON 与单引号两种都试过）⇒ 只能念字节数，" +
			"`var` 那一格**仍是空白**")
		return
	}
	var leaves []liveProbeLeafValue
	var keys []string
	liveProbeFlatten("", body, &leaves, &keys)
	sink.say("⑥ 响应键路径（只念路径与形态，`var` 的值不进日志）：%s", strings.Join(keys, " / "))
	for _, leaf := range leaves {
		// `code` / `summary` **可以**念值：生产那一口失败时打的就是这两枚
		// （`AppMailConvertPDF` → `assetSummaryHint`，截断 120），所以"能念"不是探针侧新开的口子。
		// `var.*` 一律只念形态：这一族的成功响应里装过凭据（② 那口回的就是 sid/rmkey）。
		if leaf.path == ".code" || leaf.path == ".summary" {
			text, _ := liveProbeLeafText(leaf)
			sink.say("  叶 %s = %q", leaf.path, liveProbeClip(text, 120))
			continue
		}
		sink.say("  叶 %s = %s", leaf.path, liveProbeLeafKindWord(leaf))
	}
	// 生产的判据是 `code == "S_OK"`（`AppMailConvertPDF`，取自旧 TS 那一句）。本发**并行**念一遍
	// 相等判断：不是为了改生产（它已经在做这件事），是为了把"上游那句原文"与"我们认不认"
	// 放在同一行里对上 —— 第二趟读到 `1010010111 / 用户权益已经使用完` 时才看得出这一判据真的在挡。
	if code, hasCode := liveProbeLeafByPath(leaves, ".code"); hasCode {
		if code == "S_OK" {
			sink.say("生产判据（`code==\"S_OK\"`）：**认**这一发")
		} else {
			sink.say("生产判据（`code==\"S_OK\"`）：**不认**这一发（实得 %q）⇒ "+
				"玩法 1014 会带着上游那句原文报失败", liveProbeClip(code, 60))
		}
	}
	afterCguid, afterRnd := mailDiskRandoms(executor.run.Random)
	if count, status, serr := run.Upstream.AppMailSurplusNum(
		ctx, session, afterCguid, afterRnd); serr != nil {
		sink.say("⑤ 转换权益（⑥ 之后）没读到：%s", sink.mask(serr.Error()))
	} else {
		sink.say("⑤ 转换权益（⑥ 之后）：benefitSurplusCount=%q benefitStatus=%q",
			liveProbeClip(count, 24), liveProbeClip(status, 24))
	}

	// ── 到那个目录里找产物（按名字精确对上素材那一枚）──
	candidates := []string{
		strings.TrimSuffix(receipt.Name, ".pdf") + ".docx",
		strings.TrimSuffix(receipt.Name, ".pdf") + ".pdf",
	}
	productParent, parentOK := liveProbeProductFolder(run, opening, sink)
	// `var.directoryId` 在成功那一发**非空**（2026-09-26 一号第二趟：长度 33 的串），而 ⑥ 的
	// `summary` 上游自己说了"转存至 …139邮箱-我的附件-PDF转换" ⇒ 它很可能就是那一枚目录的
	// fileId。若相等成立，生产层接清理时可以**按它导航**，不必把三枚目录名写死（那是上游的
	// 系统目录名，写进生产层等于把"改名就失效"绑在我们身上）。
	if dirID, has := liveProbeLeafByPath(leaves, ".var.directoryId"); has && parentOK {
		if dirID == productParent {
			sink.say("相等：`var.directoryId` **就是**产物所在目录的 fileId ⇒ 接清理时按它导航，" +
				"不必写死那三枚上游目录名")
		} else {
			sink.say("相等：`var.directoryId`（长度 %d）**不等于**导航到的那枚目录 fileId（长度 %d）"+
				"⇒ 不能按它导航，目录名那一格仍是问句",
				len([]rune(dirID)), len([]rune(productParent)))
		}
	}
	productFileID := ""
	if !parentOK {
		sink.say("没导航到 %s/%s/%s ⇒ 找不到产物那一枚，只能问「`var` 里有没有一枚像 fileId 的格」",
			liveProbeMailFolderRoot, liveProbeMailFolderAttach, liveProbeMailFolderPDFTrans)
	}
	for attempt := 1; attempt <= liveProbeConvertPolls && productFileID == ""; attempt++ {
		if attempt > 1 {
			time.Sleep(liveProbeConvertPollDelay)
		}
		if !parentOK {
			break
		}
		items, okItems := liveProbeRawListing(run, opening, sink, productParent)
		if !okItems {
			sink.say("第 %d 次：那个目录**读不出来** ⇒ 这一轮不算「找不到」", attempt)
			continue
		}
		for _, name := range candidates {
			if id := liveProbeChildID(items, name); id != "" {
				productFileID = id
				sink.see(id)
				sink.say("第 %d 次：读到产物 %s（该目录这一页 %d 项）", attempt, name, len(items))
				break
			}
		}
		if productFileID == "" {
			ours := 0
			for _, item := range items {
				if strings.HasPrefix(item.name, CloudUploadNamePrefix) {
					ours++
				}
			}
			sink.say("第 %d 次：那个目录这一页 %d 项（其中我方前缀 %d 枚），**没有**这一趟那一枚",
				attempt, len(items), ours)
		}
	}

	// ── 主判据：`var` 里有没有等于产物 fileId 的那一格 ──
	if productFileID == "" {
		sink.say("`var` 与产物的相等比对**做不了**（手里没有产物 fileId 当对照）⇒ 这一格仍空")
	} else {
		matched := 0
		for _, leaf := range leaves {
			text, comparable := liveProbeLeafText(leaf)
			if comparable && text != "" && text == productFileID {
				matched++
				sink.say("相等：`%s` 这一格**就是**产物那一枚的 fileId（值不打印）", leaf.path)
			}
		}
		if matched == 0 {
			sink.say("相等：`var` 里**没有任何一格**等于产物 fileId ⇒ 产物 fileId **只能**" +
				"靠列目录 + 名字精确对上（清理链要接它就得先列那个目录）")
		} else {
			sink.say("⇒ 接清理时**不必**列目录：⑥ 的响应里就有那一枚（本发只念相等，不念值）")
		}
	}

	// ── 链尾：素材 + 产物一次 batchTrash，再回读那个目录 ──
	ids := make([]string, 0, 2)
	for _, id := range executor.cloudFileIDs {
		ids = append(ids, id)
	}
	if productFileID != "" {
		ids = append(ids, productFileID)
	}
	sent, derr := uploader.DeleteCloudFiles(ctx, ids)
	switch {
	case derr != nil:
		sink.say("链尾清理失败（发过枪=%v）：%s", sent, sink.mask(derr.Error()))
	case sent:
		sink.say("链尾清理：一次 batchTrash 覆盖 %d 枚（素材 + 产物，值不打印）", len(ids))
	default:
		sink.say("链尾清理：没有要删的")
	}
	if parentOK && productFileID != "" {
		items, okItems := liveProbeRawListing(run, opening, sink, productParent)
		if !okItems {
			sink.say("清理后回读：那个目录读不出来 ⇒ 只到「清理发过枪」这一格")
		} else {
			still := false
			for _, name := range candidates {
				if liveProbeChildID(items, name) != "" {
					still = true
				}
			}
			if still {
				sink.say("清理后回读：产物那一枚**还在**那个目录里（列目录读得到）⇒ " +
					"删不动，「接清理」这条路的判据是反的")
			} else {
				sink.say("清理后回读：那一枚**不再**被列出（该目录这一页 %d 项）⇒ "+
					"列目录取 fileId 再删这条路走得通", len(items))
			}
		}
	}
	sink.say("读数怎么用：`var` 有 fileId ⇒ 清理挂在 ⑥ 之后一行就够；没有而" +
		"列目录删得动 ⇒ 清理要先导航到那个目录（三枚名字是上游系统目录，写死在生产层要按 §1.5 登记）；" +
		"两者都没有 ⇒ 只能报「这一趟有一枚残留」，不猜。")
}

// liveProbeProductFolder 从根目录一级级走到 `/139邮箱/我的附件/PDF转换`，交回它的 fileId。
//
// 只读；每一级都念"有没有找到那一枚目录名"，找不到时**不**返回空串冒充"目录不存在"——
// 交给调用方念"导航没走通"（一页 100 项装不下时也会读不到，那是对照面问题不是事实问题）。
func liveProbeProductFolder(
	run TaskMatrixRun, opening SignInOpening, sink *liveProbeSink,
) (string, bool) {
	current := "/"
	for _, name := range []string{liveProbeMailFolderRoot, liveProbeMailFolderAttach} {
		items, okItems := liveProbeRawListing(run, opening, sink, current)
		if !okItems {
			sink.say("导航：目录 %s 那一页读不出来", liveProbeClip(name, 12))
			return "", false
		}
		id := liveProbeChildID(items, name)
		if id == "" {
			sink.say("导航：上一级这一页里没有名为 %q 的那一枚（页内 %d 项）", name, len(items))
			return "", false
		}
		sink.see(id)
		current = id
	}
	items, okItems := liveProbeRawListing(run, opening, sink, current)
	if !okItems {
		sink.say("导航：最后一级读不出来")
		return "", false
	}
	id := liveProbeChildID(items, liveProbeMailFolderPDFTrans)
	if id == "" {
		sink.say("导航：这一页里没有 %q（页内 %d 项）⇒ 产物目录可能还没被上游建出来，或不在这一页",
			liveProbeMailFolderPDFTrans, len(items))
		return "", false
	}
	sink.see(id)
	sink.say("导航：已走到那三级最后一枚（fileId 只念长度）")
	return id, true
}
