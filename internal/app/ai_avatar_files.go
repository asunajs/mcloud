package app

// 从**图片 URL** 倒推"网盘里那一个文件"—— AI 头像那条链的收尾两函数（纯函数）。
//
// 对应旧实现 `tasks/ai-avatar/task.ts:12-36`（`extractImageFileName` 与
// `collectAiAvatarFileIds`）。
//
// # 这条链为什么需要"按文件名找人"
//
// `POST /market/rabactive/image/upload` 会把"原噪点图 + 合成图"两张**写进用户网盘**，
// 回一个 id。旧实现随后要把这两个文件当临时文件清掉，但它拿到的只有 id，
// 而**清理要的是网盘侧的 `fileId`** —— 两串的形态并不相同（一个是活动的作品 id，
// 一个是网盘对象 id）。于是那条链绕了三枪：
// `getWorkList`（作品 → 图片 URL）→ `catalogFullPath`（活动目录 → 父 `fileId`）
// → `file/list`（列目录 → `name` 与 `fileId`），**用图片 URL 的文件名**去撞目录里的 `name`。
//
// ⚠ 这个做法的真实风险写在这里而不是别处：**同名即错配**。用户网盘里如果本来就
// 有一张同名的图，这里会把它的 `fileId` 当成活动产物交出去 —— 而清理那一族是**真删**。
// 本仓今天**没有接上清理**：`hcy/recyclebin/batchTrash` 的请求构造与发送口已于
// 2026-09-24 落码（`internal/upstream/app_cloud_upload.go` 的 `CloudBatchTrashRequest`
// / `AppCloudBatchTrash`，编排侧 `cloud_upload.go` 的 `DeleteCloudFiles`），
// 但它**零实拍**（R27）且**没有调用点**（要不要真跑见 `docs/pending-decisions.md` P1）。
// 所以这里只负责"找回并交回 id"，**不删任何东西**。
// 接清理之前必须先解决这一格（怎么解：`file/create` 的响应里就有 `fileId`，
// 拿到**文件型**那一发的实拍之后应当改成"直接用上传响应给的 id"，把这三枪整个绕开 ——
// 该口的实拍也在 P1 那一次真跑的范围里）。
//
// # 为什么这两件事值得单独一个文件
//
// 它们是两个链上都会用的**纯**判断（生成链收尾用、将来清理链也用），
// 而且失效方式都是"匹配不上但没人报错"：
// 文件名正则漏一档 ⇒ 少找一个文件；URL 与 `name` 的比较放宽一档 ⇒ 删错文件。
// 混在编排里写的话，两侧都只会在真跑时才发现，而那时代价落在用户网盘上。

import (
	"regexp"
	"sort"
	"strings"

	"github.com/catlair/mcloud/internal/upstream"
)

// 图片扩展名的判定，逐字对齐旧实现的 `/\.(?:jpe?g|png)$/i`。
//
// ⚠ 三档就够，是因为上游只会给这三种形态（旧实现见过的全部产物）。
// 出现 `.webp` 时这里判 false ⇒ 那一个文件**找不到**（而不是找错）。
// 这一档**未验证**：AI 头像的产物今天是 `.jpg` 还是别的，本仓零实拍。
// 放宽成"任意扩展名"是把安全侧的判断当性能问题处理，不做。
var aiAvatarImageExt = regexp.MustCompile(`(?i)\.(jpe?g|png)$`)

// aiAvatarImageFileName 从图片 URL 里取出**文件名**（含扩展名）。
//
// 逐步对齐旧实现：按 `/` 切、取最后一段、再按 `?` 或 `#` 截断、然后验扩展名。
//
// ⚠ 顺序很重要：**先截查询串再验扩展名**。反过来写的表现是
// `…/a.jpg?token=x` 被判成"不是图片"（旧实现不会），于是那一个临时文件永远找不回来。
// 反过来也有一种情形会出错：`?` 出现在路径段之前时（`…/a?b/c`），
// 旧实现拿的是**最后一段**再截，所以它取到 `c` —— 这里保持一致，不去"修正"它：
// 那串 URL 由上游生成，形态不是我们能改的，而"修正"会让匹配结果与旧项目不一致。
//
// 第二个返回值是"像不像图片文件名"。返回 `("", true)` 不存在，返回 `("x", false)` 时
// 第一个值**不作数** —— 用 bool 而不是 `""`/`nil` 区分"没给 URL"与"给了但不匹配"，
// 因为这两档在调用方是同一处置（跳过），区分它们只会多一个测不到的分支。
func aiAvatarImageFileName(url string) (string, bool) {
	if url == "" {
		return "", false
	}
	// 旧式 `split("/").pop()`：末尾是 `/` 时最后一段是空串 ⇒ 不算文件名。
	last := url[strings.LastIndexByte(url, '/')+1:]
	if cut := strings.IndexAny(last, "?#"); cut >= 0 {
		last = last[:cut]
	}
	if last == "" || !aiAvatarImageExt.MatchString(last) {
		return "", false
	}
	return last, true
}

// collectAiAvatarFileIDs 把"活动侧的作品 id"翻译成"网盘侧待清理的文件 id"。
//
// 三步（与旧实现一致）：① 只看在 `uploadIDs` 里出现过的作品；
// ② 取它们两格图片 URL（`imgUrl` 与 `qrImgUrl` —— 一次生成会产两张）的文件名；
// ③ 拿这批名字去网盘列表里撞，撞上的取 `fileId` 去重。
//
// 返回**已排序**且去重：旧实现返回的是 Set 的插入序，而这里的顺序会进
// `SummaryLine` 与将来的清理请求 —— 插入序取决于 `getWorkList` 的返回顺序，
// 那是不稳定的。排完序才能断言"两次跑同一份响应得到同一串"。
//
// ⚠ 上游 `qrImgUrl` 有时与 `imgUrl` 同图（未验证），那时两格撞的是同一个文件，
// 靠 `fileId` 去重收掉 —— 不去重就是同一个文件登记两遍，清理时会多删一次。
func collectAiAvatarFileIDs(
	uploadIDs []string,
	works []upstream.AiAvatarWork,
	files []upstream.CloudFile,
) []string {
	targetIDs := make(map[string]struct{}, len(uploadIDs))
	for _, id := range uploadIDs {
		targetIDs[id] = struct{}{}
	}
	targetNames := make(map[string]struct{}, 2*len(works))
	for _, work := range works {
		if _, ok := targetIDs[work.WorkID]; !ok {
			continue
		}
		for _, url := range []*string{work.ImgURL, work.QrImgURL} {
			if url == nil {
				continue
			}
			if name, ok := aiAvatarImageFileName(*url); ok {
				targetNames[name] = struct{}{}
			}
		}
	}
	if len(targetNames) == 0 {
		return []string{}
	}
	seen := make(map[string]struct{}, len(files))
	out := make([]string, 0, len(files))
	for _, file := range files {
		if _, ok := targetNames[file.Name]; !ok {
			continue
		}
		if _, dup := seen[file.FileID]; dup {
			continue
		}
		seen[file.FileID] = struct{}{}
		out = append(out, file.FileID)
	}
	sort.Strings(out)
	return out
}
