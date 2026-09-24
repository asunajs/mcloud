package app

// 网盘"传一个文件"的编排：四枪的**顺序**与三态结局（#35）。
//
// 对应旧实现 `packages/core/src/utils/file.ts` 的 `uploadFile` / `uploadRandomFile` /
// `deleteFiles`（三个函数的全部逻辑都在这里，请求形状在 `internal/upstream`）。
//
// # 这一族今天**没有调用点**
//
// 等它的是四条要上传的玩法（106 / 113 / 522 / 1014，见
// `docs/pending-decisions.md` P1 与 `internal/app/task_matrix_dispatch.go` 的
// `taskMatrixToolGaps`）与 `after` 的清理那一半。把它们接上之前，这一层是
// "能编译、被测试钉住形状、但一次都没真打过上游"的代码 —— 这是**登记过**的状态，
// 不是"做完了一半"。
//
// # ⚠ 四枪里三枪零实拍：这一层只保证"顺序与判据照旧实现"
//
// `file/create` 的**文件型**、`PUT`、`complete`、`batchTrash` 在本仓所有归档里
// 一条样本都没有（唯一实拍是"建目录"那一型）。所以：
//
//   - 下面每个分支的**判据**都有旧 TS 的字面出处（写在各自注释里），
//     但"上游真的会回这些字段吗"未验证；
//   - 真跑一次的代价是**往用户网盘写文件**（且失败路径可能留下孤儿文件），
//     所以要不要跑由用户拍板（P1 第三档）。要核它：拿一个测试号跑
//     "传一个小文件、再 batchTrash 掉"，把四枪的请求体与响应体都抓下来。
//
// # 与旧实现**刻意不一致**的一处：complete 失败时仍交回 fileId
//
// 旧实现只在"全部成功"与"秒传"两支把 id 记进 `ctx.state.files`
// （`utils/file.ts:60/93`），complete 失败那一支直接 `return false` ——
// 也就是"**网盘里已经有一个坑了，但没人知道它的 id**"，清理那一族靠 id 删，
// 于是那个文件留在用户网盘里。本层在**所有**分支都尽量把已知的 FileID 交回
// （见 [CloudUploadReceipt]），错误照回。这不算改业务语义：它只多交一个信息，
// 让调用方能选择清理，而不删就什么都不少做。

import (
	"context"
	"encoding/hex"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 合成文件的默认档（旧 TS `utils/file.ts:30` 的 `asign-${randomHex(4)}${ext||".png"}`）。
const (
	// CloudUploadNamePrefix 是临时文件的名字前缀。旧实现**一直**用它 ⇒
	// 用户在网盘里看到 `asign-xxxxxxxx.png` 就是这套脚本留下的东西。
	CloudUploadNamePrefix = "asign-"
	// CloudUploadDefaultExt 是没指定扩展名时的那一枚（含点）。
	CloudUploadDefaultExt = ".png"
	// cloudRandomNameHexLen 是文件名里那 8 个十六进制位的来源（`randomHex(4)` = 4 字节）。
	cloudRandomNameHexLen = 4
	// cloudRandomContentBytes 是随机文件的内容长度（`randomHex(32)` = 32 字节 ⇒ 64 个字符）。
	//
	// ⚠ 内容本身是**十六进制文本**而不是随机字节：旧实现把 `randomHex(32)` 这个
	// **字符串**当文件内容发出去，于是 body 是 64 字节 ASCII、`size` 也是 64。
	// 两件事不能混：若这里改成发 32 个随机字节，`size` 与 sha256 全变 ⇒
	// 那是改接口，不是改风格。
	cloudRandomContentBytes = 32
)

// CloudUploadOutcome 是这一趟上传的三种**非错误**结局。
//
// 三档分开而不是一个 bool，是因为旧实现的 `return false` 里混了两种完全不同的事：
// "文件已存在所以什么都没做"与"传失败了"。它们在**记账与清理**上是两侧：
// 前者**不该**去删（那个文件不是我们建的），后者**该**删（网盘里可能已经有坑）。
type CloudUploadOutcome int

const (
	// CloudUploadNothing 是**零值**：这一趟没有产出任何可确认的结果（出错时就是这一档）。
	//
	// ⚠ 它排第一**不是**随手：让零值等于"已上传"会让"忘了赋值"表现成
	// "报了一次成功"，而这一族的成败决定要不要往清理那一族交 id ——
	// 零值念成成功 = 一个文件留在用户网盘里没人删。
	CloudUploadNothing CloudUploadOutcome = iota
	// CloudUploadDone 是走完 create → PUT → complete 三枪。
	CloudUploadDone
	// CloudUploadRapid 是秒传：create 回 `rapidUpload:true`，
	// 旧实现**跳过 PUT 与 complete**（`utils/file.ts:57-61`），只把 fileId 记账。
	// ⚠ 因此这一档**仍然要清理** —— 网盘里确实有这一个文件。
	CloudUploadRapid
	// CloudUploadExists 是 create 回 `exist:true`：旧实现直接返回且不上传
	// （`utils/file.ts:52-55`）。这一档 [CloudUploadReceipt.FileID] 为**空**，
	// 因为那个同名文件不是本次造的 ⇒ 把它交给删除那一族就是删用户自己的东西。
	CloudUploadExists
)

// String 给人话档名，用于小结与日志。
func (o CloudUploadOutcome) String() string {
	switch o {
	case CloudUploadDone:
		return "已上传"
	case CloudUploadRapid:
		return "秒传"
	case CloudUploadExists:
		return "已存在"
	case CloudUploadNothing:
		return "未上传"
	}
	return "未知结局"
}

// CloudUploadReceipt 是这一次上传的**收据**。
//
// `FileID` 非空 ⇒ 网盘里存在一个可以（也该）被清理的条目；为空 ⇒ 没有新对象。
// 出错时它**也可能非空**（见文件头那一处刻意不一致）。
type CloudUploadReceipt struct {
	Outcome CloudUploadOutcome
	FileID  string
	// Name 是实际发出去的文件名（调用方排错时要对得上网盘里那一格）。
	Name string
}

// CloudUploadOptions 是编排层的可调参数。零值 = 旧实现 `uploadRandomFile` 的那一档。
type CloudUploadOptions struct {
	// ParentFileID 空 ⇒ 根目录（上游口径由 upstream 层补 "/"）。
	ParentFileID string
	// Name 空 ⇒ 现造 `asign-<8 hex><Ext>`。
	Name string
	// Ext 只在 [CloudUploadOptions.Name] 为空时参与拼名；空 ⇒ `.png`。
	Ext string
	// Channel 空 ⇒ [upstream.CloudChannelAndroid]（旧 TS 的默认档）。
	Channel upstream.CloudChannel
	// Op 空 ⇒ [upstream.CloudOpUpload]。⚠ 只有 android 档读它。
	Op upstream.CloudOpType
}

// CloudUploader 是"能往网盘传文件"这件事的注入点：一个上游口 + 一个随机源。
//
// 做成小结构体而不是四个参数，是因为调用方（将来的 106/113/522 那三条 runner）
// 每一趟都要传同一串 `authorization` / `deviceID` 两次以上 —— 让它们各传各的
// 会出现"某一枪用了另一枪的凭据"这种编译得过的错。
type CloudUploader struct {
	Up   *upstream.Upstream
	Rng  core.Random
	Auth string
	// DeviceID 是 `deviceid` 头。旧实现**每一枪都带**它（主 http 实例的静态头），
	// 取不到 deviceId 时整条链根本不会启动（`init-account.ts:31-35`）⇒
	// 这里不接空串，由调用方在更早的地方拦。
	DeviceID string
}

// UploadContent 上传一份**给定内容**（**写**：会在用户网盘里造一个文件）。
//
// 四枪顺序不可换：`create`（占位，回 uploadUrl/uploadId）→ `PUT`（写字节）→
// `complete`（声明"传完了"，服务端按 `contentHash` 校验）。秒传与"已存在"两支
// 少发后几枪，见 [CloudUploadOutcome]。
func (u CloudUploader) UploadContent(
	ctx context.Context, content []byte, opts CloudUploadOptions,
) (CloudUploadReceipt, *core.Error) {
	name := opts.Name
	if name == "" {
		name = CloudUploadNamePrefix + randomHex(u.Rng, cloudRandomNameHexLen) +
			extensionOrDefault(opts.Ext)
	}
	receipt := CloudUploadReceipt{Name: name}
	// 与旧实现同一枚 hash：`sha256(file)`，且 create 与 complete **复用同一个串**。
	contentHash := core.SHA256Hex(content)

	created, perr := u.Up.AppCloudFileCreate(ctx, u.Auth, u.DeviceID, upstream.CloudFileCreateInput{
		ParentFileID: opts.ParentFileID,
		Name:         name,
		Size:         int64(len(content)),
		ContentHash:  contentHash,
		Channel:      opts.Channel,
		Op:           opts.Op,
	})
	if perr != nil {
		return receipt, perr
	}
	if created.Exist {
		// 旧实现：记一句"文件已存在"然后返回 false（**不记账**，也就不会去删）。
		receipt.Outcome = CloudUploadExists
		return receipt, nil
	}
	if created.RapidUpload {
		// 秒传：后两枪**不发**（旧实现 `return true` 前只做了记账这一件事）。
		receipt.Outcome = CloudUploadRapid
		if created.FileID != nil {
			receipt.FileID = *created.FileID
		}
		return receipt, nil
	}
	if created.FileID != nil {
		receipt.FileID = *created.FileID
	}
	// 旧实现 `utils/file.ts:64`：`!uploadUrl || !fileId` ⇒ 判失败。
	// 这里分开两句错误文本，因为两者的**下一步**不同：
	// 缺 URL 是"这一族的响应形状变了"，缺 fileId 是"占位没成但也没报错"。
	// ⚠ 下面所有错误路径都**不设** Outcome：它的零值是"未上传"（见那个 iota 的注释）。
	// 出错却报"已上传"的表现是调用方把这一趟记成一次成功上传。
	if created.UploadURL == nil || *created.UploadURL == "" {
		return receipt, core.NewParse("file/create 没给 uploadUrl：无法 PUT（零实拍的一格，形状可能变了）")
	}
	if created.FileID == nil || *created.FileID == "" {
		return receipt, core.NewParse("file/create 没给 fileId：即使传完也无从记账与清理")
	}
	if perr := u.Up.AppCloudUploadPut(ctx, *created.UploadURL, content, u.Auth, u.DeviceID); perr != nil {
		// 这一支**保留 FileID**：占位已经成功 ⇒ 网盘里已经有那一个坑了。
		return receipt, perr
	}
	// complete 用**同一档渠道**：旧实现在这里有一个"没给就退回 android"的默认
	// （`utils/file.ts:84`），而那个默认会让"create 用 web、complete 用 android"
	// 悄悄发生。这里不复制那个默认 —— 两枪同一档是可断言的，跨档不是。
	if perr := u.Up.AppCloudFileComplete(
		ctx, u.Auth, u.DeviceID, receipt.FileID, stringOf(created.UploadID), contentHash,
		channelOrDefault(opts.Channel), opts.Op,
	); perr != nil {
		// 同上：字节的 PUT 已经成功，这一格失败意味着网盘里有一个"没完成"的对象 ⇒
		// fileId 必须交回，否则它就是那个删不掉的孤儿。
		return receipt, perr
	}
	receipt.Outcome = CloudUploadDone
	return receipt, nil
}

// UploadRandom 传一个内容为十六进制文本的小文件（旧 `uploadRandomFile`）。
//
// 它存在的理由只有一条：那四条玩法要的是"**有过一次上传动作**"而不是
// "文件内容有意义"，所以内容越短越省事（64 字节）。
func (u CloudUploader) UploadRandom(
	ctx context.Context, opts CloudUploadOptions,
) (CloudUploadReceipt, *core.Error) {
	return u.UploadContent(ctx, []byte(randomHex(u.Rng, cloudRandomContentBytes)), opts)
}

// DeleteCloudFiles 把一批文件移进回收站（**写**，且不可回滚）。
//
// 第一个返回值是"**这一枪发没发**"：空列表 ⇒ 不发（旧实现也是在调用侧挡的，
// `after.ts:133`，并且分成两句日志）。把它做成返回值而不是让调用方自己判长度，
// 是为了让"没有要清理的东西"与"清理成功了"在日志里仍是两件事。
func (u CloudUploader) DeleteCloudFiles(
	ctx context.Context, fileIDs []string,
) (bool, *core.Error) {
	if len(fileIDs) == 0 {
		return false, nil
	}
	return true, u.Up.AppCloudBatchTrash(ctx, u.Auth, u.DeviceID, fileIDs)
}

// extensionOrDefault 补旧实现那个 `ext || ".png"`。
//
// ⚠ 它**只补前导点以外的形状**：传 `"jpg"`（不带点）会拼出 `asign-xxxxjpg`。
// 旧实现也是这样（那是模板串不是 path.Ext），而这一格零实拍 ⇒ 不"顺手修正"，
// 但也不为它写断言（见测试文件里那条"未断言"声明）。
func extensionOrDefault(ext string) string {
	if ext == "" {
		return CloudUploadDefaultExt
	}
	return ext
}

// channelOrDefault 与 upstream 层同一条默认（那里的是方法，这里是编排层的参数）。
func channelOrDefault(channel upstream.CloudChannel) upstream.CloudChannel {
	if channel == "" {
		return upstream.CloudChannelAndroid
	}
	return channel
}

// randomHex 产 `n` 个随机字节的十六进制（旧 TS `randomHex(n)` 的语义：**参数是字节数**，
// 所以 `randomHex(4)` 是 8 个字符）。
//
// ⚠ 那两个数搞混的表现是"名字短一半"或"内容长一倍"，都不会报错。
func randomHex(rng core.Random, n int) string {
	buf := make([]byte, n)
	rng.FillBytes(buf)
	return hex.EncodeToString(buf)
}

// stringOf 把 `*string` 折成"nil 当作空串"。
//
// 只有 `uploadId` 用得上：旧实现不判它空就带进 complete（那一格 null 会怎样零样本），
// 所以这里保持"照发"，不把它变成错误。
func stringOf(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}
