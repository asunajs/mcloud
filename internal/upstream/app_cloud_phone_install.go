// 红包派对（云机）族 · 云机实例与安装：`thirdQueryUserRelationshipV2`、
// `configAppList`、`gethwToken`、`instanceApkInstallStatus`、`apkInstall`、
// `appInstallTaskStatus`、`userInstallApp`。
//
// ⚠⚠⚠ **本文件这些口全部零实拍样本** ⚠⚠⚠
// 这些路径名在 `test/fixtures/http/红包派对.json` 的 39 条里 grep **命中 0**
// （2026-09-24 逐条核过；夹具里 `cpactiv.buy.139.com` 上只有十条：
// `redpacket/{configTaskList,configTaskLoginList,userAccountInfo,userToastInfo}`、
// `activity/common/ruleInfo`、`rule/config/list`、`user/tokenValidate`、`bilog/upload`）。
//
// # 形状有两份出处，而且它们给的层次不一样
//
// **出处一**：`docs/business-feature-inventory.md` §2.4 第 375 行那半句 —— 它给的是
// **动作顺序**与两个值（`ACTIVE`/`PENDING`、`lockStatus==="Y"`），
// 给不出 URL 的目录段、body 的键名、响应的键名。
//
// **出处二**（2026-09-24 补，本文件 URL 与 body 键名的来源）：旧 TS
// `packages/core/src/tasks/cloud-phone-redpack/api.ts:116-172` —— 它把每一条的
// **完整路径**与**请求体键名**都写成了代码（是一份跑过的实现，不是注释）：
//
//	/thirdapi/huawei/thirdQueryUserRelationshipV2  {pageNumber,pageSize,status}
//	/user/gethwToken                              {}
//	/redpacket/configAppList                      {platformType:1}
//	/app/activity/instanceApkInstallStatus        {hwToken,apkList[],instanceIdList[]}
//	/app/activity/apkInstall                      {instanceId,appId,hwToken}
//	/app/activity/appInstallTaskStatus            {taskId,hwToken}
//	/redpacket/userInstallApp                     {taskId,appId,platformType:1[,existed]}
//
// ⇒ **取态按接口真相的权威顺序**：抓包样本 > 旧 TS > 我的推测。夹具里那十条把
// 前缀钉成了 `redpacket/` `user/` `activity/` `rule/` `bilog/`，与旧 TS 在同一条上
// 逐字相同（`user/tokenValidate`、`redpacket/configTaskLoginList`…）⇒ 旧 TS 给的
// 目录段**与实拍同一套拼法**，所以本文件采用它，**不再**填"裸尾段"。
//
// ⚠ 这**不等于**这几条已核实：旧 TS 是"曾经可用"的证据，不是"今天上游还认"的证据。
// **响应侧仍然零出处** —— 旧 TS 只声明了它自己要读的三两个键
// （`hwToken?`、`list?`、`data.subscribeDetails?`），载荷读不出的口照旧只交 `data`
// 原文（[CloudPhoneData]），不因为请求侧有了出处就去猜响应键名。
//
// # 为什么这些口比本族其它口更该谨慎
//
// `apkInstall` 是**真往用户的云机上装 APK**（成文 §0 一页总览第 39 行给它写的是
// "真给云机装 APK"，产出那一列是**现金**）。它同时满足两件最坏的事：
// 副作用在别人设备上、且**零样本**。⇒ 本层只提供构造与发送，
// **不**在这里加"看起来更安全"的自动判断（比如"没锁屏就装"）：
// 那种判断没有任何证据支持，写出来只会让下一层以为已经有人把关了。
// 门槛与二次确认属编排层与 WebUI（`internal/capabilities/registry.go` 的 `RunInBatch`
// 那一档由接线的人判，本文件不主张）。
//
// # `userInstallApp`：本文件把它补上了（2026-09-24）
//
// 上一版这里写的是"成文的端点清单没点它的名 ⇒ 不实现"。那句话在**只有出处一**
// 的时候成立；现在旧 TS `api.ts:163-172` 给了完整路径与 body 键名
// （`{taskId, appId, platformType:1}` + 可选 `existed`）⇒ 它不再是"零样本叠零样本"，
// 而是与上面六条同一档（请求侧有出处、响应侧零样本），于是按同一口径实现：
// [CloudPhoneUserInstallAppRequest]。
// ⚠ 旧 TS 的 `installApp` 链里它是**记账**（装完/已装都要发一次），不是查询：
// 装了就算登记过，本层不替编排判断该不该发。
//
// # 成文记着的两个已知缺陷（登记在此，处置不在本层）
//
//   - 开源 Go 参考实现把 `configTaskList` / `userSign` 那一条做成了
//     **`taskID` 从未进请求体**（见 `docs/caiyun-go-feature-inventory.md` 第 72 行
//     （表内编号 160）与第 224 行，以及 `docs/caiyun-go-parameters-and-decisions.md`
//     第 166 行的表 T-4），处置是"先修再搬"。
//     ⇒ 本族所有带任务维度的口都把 body 交回调用方，**没有**任何一条写成空 body
//     （见 `app_cloud_phone_task.go` 的 `CloudPhoneUserSignRequest`）。
//   - 旧 TS 的安装链是**先记账后动作**（同成文第 380 行）：`userCompleteTask` 先发，
//     后面装失败任务也已登记完成。本层不改顺序，改在编排层。
package upstream

import (
	"context"
	"encoding/json"

	"github.com/catlair/mcloud/internal/core"
)

// 这些口的路径。**零实拍 ⇒ 值取自旧 TS**（`api.ts:116-172`，逐字照抄它的目录段）。
//
// 上一版这里填的是"裸尾段"，理由是成文 §2.4 只给了尾段名。那句话没错，
// 但它只描述了**成文**的覆盖面：旧 TS 的实现里路径是完整的，
// 而它给的前缀与夹具里那十条**实拍钉住的前缀同一套拼法**
// （`user/tokenValidate`、`redpacket/configTaskLoginList`）⇒ 按接口真相的
// 权威顺序（样本 > 旧 TS > 推测）改用旧 TS 的那一串。
//
// ⚠ 真跑出 404 时改这里，一次改一条：`/thirdapi/huawei/` 与 `/app/activity/`
// 这两段在夹具里**从未出现过**，它们是"旧 TS 这么发过"而非"上游这么认"。
const (
	cloudPhoneRelationshipPath        = "thirdapi/huawei/thirdQueryUserRelationshipV2"
	cloudPhoneConfigAppListPath       = "redpacket/configAppList"
	cloudPhoneGetHwTokenPath          = "user/gethwToken"
	cloudPhoneInstanceApkInstallPath  = "app/activity/instanceApkInstallStatus"
	cloudPhoneApkInstallPath          = "app/activity/apkInstall"
	cloudPhoneAppInstallTaskStatusPth = "app/activity/appInstallTaskStatus"
	// cloudPhoneUserInstallAppPath 是**记账**那一条（装完/已装都要发，见文件头）。
	cloudPhoneUserInstallAppPath = "redpacket/userInstallApp"
)

// ─────────────────────────────────────────────────────────────────────────────
// 请求体：键名按旧 TS `api.ts:116-172` 逐字（`platformType` 那一格它固定发 `1`）
// ─────────────────────────────────────────────────────────────────────────────
//
// ⚠ 这些函数**只是把旧 TS 的字面量搬过来**，不是"我们推导出的必填集"：
// 上游缺了某个键会怎样、多给一个会怎样，全部**未测试**。
// 之所以在此刻固化它们，是因为上一版把 body 整个交回调用方，
// 结果是接线时**没有任何一处成文写着这些口要发哪些键** —— 那比"照旧 TS 发"更容易出错。
// 要发别的键（比如上游改版）：`data` 参数仍然照旧合并进来并**覆盖**同名键。

// CloudPhoneRelationshipBody 是列云机那一枪的 body：`{pageNumber,pageSize,status}`。
//
// `status` 取 [CloudPhoneRelationshipActive] / [CloudPhoneRelationshipPending]。
// 旧 TS 固定发第 1 页、每页 10 条（`api.ts:118`）⇒ 本仓照抄，**但分页这件事是可疑的**：
// 一页 10 条意味着"云机多于 10 台就看不见"，而旧实现没有翻页。
// 这里不擅自改成大页码（那会改变上游的返回），把 `page`/`pageSize` 交回调用方才是
// 接线时要不要翻的决定，见两个参数。
func CloudPhoneRelationshipBody(status string, page, pageSize int) map[string]any {
	if page <= 0 {
		page = 1
	}
	if pageSize <= 0 {
		pageSize = 10
	}
	return map[string]any{
		"pageNumber": page,
		"pageSize":   pageSize,
		"status":     status,
	}
}

// CloudPhoneEmptyBody 是"旧 TS 发 `{}`"那两条（`gethwToken`、`configTopicList` 类）的显式写法。
//
// 为什么要有这个名字而不是直接传 `nil`：`nil` 在 [CloudPhoneRequest] 里与"空 map"
// 等价（都只剩 `version` 那一键），但读代码的人分不出"作者想过这里没键"还是"忘了传"。
func CloudPhoneEmptyBody() map[string]any { return map[string]any{} }

// CloudPhoneConfigAppListBody 是取可装应用清单那一枪的 body：`{platformType:1}`。
func CloudPhoneConfigAppListBody() map[string]any {
	return map[string]any{"platformType": CloudPhonePlatformType}
}

// CloudPhoneInstallStatusBody 是"这台机装过没有"那一枪的 body。
//
// 旧 TS 把两个单值包成**长度为 1 的数组**（`api.ts:146`：`apkList:[packageName]`、
// `instanceIdList:[instanceId]`）⇒ 这是批量口，本仓只按它自己用过的那一档发一次一个。
// ⚠ "数组里塞多个会怎样"零样本，别自作主张合并成一次多装。
func CloudPhoneInstallStatusBody(hwToken, instanceID, packageName string) map[string]any {
	return map[string]any{
		"hwToken":        hwToken,
		"apkList":        []string{packageName},
		"instanceIdList": []string{instanceID},
	}
}

// CloudPhoneApkInstallBody 是**真装 APK** 那一枪的 body：`{instanceId,appId,hwToken}`。
//
// ⚠ 副作用最大的一条：改的是**用户那台云机**。键值都照旧 TS 原样送出，
// `appId` 在旧 TS 里是 `number|string`（来自 `configAppList` 的响应）⇒ 这里收 `any`，
// 由调用方保证类型：**不要**在这里做"能转成数字就转"的好事，
// 那会把上游当字符串用的那一格变成数字（本族有过 `platformType` 数字、
// `taskId` 也可能是字符串的先例 ⇒ 无样本时保持原样是唯一不丢信息的选择）。
func CloudPhoneApkInstallBody(instanceID string, appID any, hwToken string) map[string]any {
	return map[string]any{
		"instanceId": instanceID,
		"appId":      appID,
		"hwToken":    hwToken,
	}
}

// CloudPhoneInstallTaskStatusBody 是轮询"装完没有"那一枪的 body：`{taskId,hwToken}`。
func CloudPhoneInstallTaskStatusBody(hwToken, installTaskID string) map[string]any {
	return map[string]any{
		"taskId":  installTaskID,
		"hwToken": hwToken,
	}
}

// CloudPhoneInstallReportBody 是记账那一枪的 body：`{taskId,appId,platformType:1[,existed]}`。
//
// `existed` 是可选位（旧 TS `api.ts:169` 用 `isNil` 判：不传就**不发这个键**）⇒
// 这里传 `nil` 就整键省略。为什么省略而不是发 `0`：
// 上游看到的是"没这个字段"还是"existed=0"，两格**零样本**，
// 而旧实现明确只在前者那一支发；发 `0` 是把未知当已知。
func CloudPhoneInstallReportBody(taskID any, appID any, existed *int64) map[string]any {
	body := map[string]any{
		"taskId":       taskID,
		"appId":        appID,
		"platformType": CloudPhonePlatformType,
	}
	if existed != nil {
		body["existed"] = *existed
	}
	return body
}

// CloudPhoneData 是"载荷形状零样本"那一类口的解析结果：**只交 `data` 原文**。
//
// # 为什么不声明字段
//
// 这一族的安装类响应，成文连一个键名都没给（它只描述了动作顺序）。
// 声明一个 `Installed *bool` 之类的字段，表现是"永远读到 nil"，
// 而 nil 会被下一层读成"没装"⇒ 于是**真的发一次安装**。
// 交原文至少不会假装知道有什么键。
//
// ⚠ 拿到原文**不许**直接进日志或结论：这些 map 里可能有 token 类字段
// （本族另一条口 `gethwToken` 的名字就暗示了这件事）。
type CloudPhoneData map[string]any

// ParseCloudPhoneData 判信封并交出 `data` 原文。
//
// 信封不过 ⇒ 报错（含"没有 header 段"、"status 不是字符串 `"200"`"两种，
// 见 [EnsureCloudPhoneOK]）。`data` 缺失或不是对象 ⇒ nil。
func ParseCloudPhoneData(envelope *Envelope) (CloudPhoneData, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return nil, perr
	}
	if data == nil {
		return nil, nil
	}
	return CloudPhoneData(data), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 列云机实例：`thirdQueryUserRelationshipV2`
// ─────────────────────────────────────────────────────────────────────────────

// 成文 §2.4 第 375 行点名的两枚状态值（旧实现对**同一张口查两次**再聚合）。
//
// ⚠ 它们是**上游字面量**，不是我们的枚举名；大写与拼写照抄。
// "装完会不会新出现第三种关系状态（比如 EXPIRED）"无从知道 ⇒ 别把这两枚当全集。
const (
	// CloudPhoneRelationshipActive 是 `ACTIVE`。
	CloudPhoneRelationshipActive = "ACTIVE"
	// CloudPhoneRelationshipPending 是 `PENDING`。
	CloudPhoneRelationshipPending = "PENDING"
)

// CloudPhoneLockStatusLocked 是成文给出的那一个锁屏字面量：`"Y"`。
//
// ⚠ 它**没有**实拍：来源是成文 §2.4 第 375 行那句"锁屏 `lockStatus==="Y"` 优先跳过"。
// `"Y"` 之外的取值（`"N"`？空？根本没这个键？）同样零样本 ⇒ 本层不做布尔化：
// 读成 `LockStatus *string` 交原文，"等于 Y 就跳过"这个判断留给拿到样本的编排层。
const CloudPhoneLockStatusLocked = "Y"

// CloudPhoneRelationshipRequest 构造列云机实例那一枪。
//
// 路径、body 键名、以及响应的读法都有旧 TS 出处（见文件头与
// [ParseCloudPhoneInstanceList]）；**零实拍**这一点不变。
// ⚠ 调用方要么用 [CloudPhoneRelationshipBody] 拼 body，要么自己传一份同样键名的 map ——
// 传 `nil` 会发出只有 `version` 的空 body，旧 TS 从不这么发。
// 调用方需要发**两枪**（`ACTIVE` + `PENDING` 各一次）再合并，那是编排的活。
func CloudPhoneRelationshipRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneRelationshipPath, data, auth, referer)
}

// CloudPhoneInstance 是一台云机实例。
//
// # 两个键的出处，以及一处**上一版读错了的地方**
//
// 上一版只声明 `lockStatus`，而且是从**条目顶层**读。旧 TS 的读法
// （`task.ts:274-283`）是 `item.subsProdInstances[0].lockStatus` —— 锁屏位在
// **子数组的第一项**里，条目顶层根本没有这个键。⇒ 那一版的表现在真样本下是
// "永远读到 nil"，而 nil 会被下一层读成"没锁屏"（旧实现里"不是 Y"就等于可选）：
// 一个本应跳过的锁屏云机会被拿去装 APK。现已按旧 TS 改正。
//
// ⚠ 这仍是**旧 TS 的形状**，不是实拍：`subsProdInstances` 有多项时会怎样（旧实现
// 只看 `[0]`）、`lockStatus` 的取值集合，都**未测试**。
type CloudPhoneInstance struct {
	// InstanceID 是 `instanceId` —— 安装链后面三条口都要用它（旧 TS `task.ts:279`）。
	// 缺失 ⇒ nil，而旧实现在此情况下**放弃这件任务**（"需手动完成 (云机锁屏)"那一路）。
	InstanceID *string
	// LockStatus 是 `subsProdInstances[0].lockStatus`。
	//
	// ⚠ 语义只有一句成文描述（`"Y"` ⇒ 锁屏、旧实现优先跳过），其它取值的含义**未验证**；
	// 子数组缺失或为空同样读成 nil（旧实现把 nil 当"未锁屏"用 —— 那是**编排层**的判断，
	// 本层不布尔化）。
	LockStatus *string
	// Raw 是该条目的其余键（键名与个数**零实拍** ⇒ 只交原文）。
	Raw map[string]any
}

// CloudPhoneInstanceList 是聚合前的单次结果。
type CloudPhoneInstanceList struct {
	// Instances 是实例列表。
	Instances []CloudPhoneInstance
	// Nested 报告本次是从 `data.data` 那一层取到的（成文点名要兼容的双层嵌套）。
	//
	// ⚠ 为什么留这一位：外层 `data` 与内层 `data` **到底哪一层是列表**，
	// 旧实现的答案是"两种都兼容"，而我们两种都没样本。留一个布尔
	// 是为了接线时**看得见**这次走的是哪条路（走错了的表现是"列表空了"，
	// 而"空列表"与"取错了层"在只交 Instances 的版本里长得一模一样）。
	Nested bool
}

// ParseCloudPhoneInstanceList 解析实例列表。
//
// # 取列表的写法是旧 TS 那一条（`task.ts:193-195`），逐字对齐：
//
//	nested = (data.data 是对象) ? data.data : data
//	list   = nested.subscribeDetails ?? []
//
// 这就是成文 §2.4 第 375 行那句"兼容 `data.data` 双层嵌套"的**全部**内容：
// 外层与内层都只找一个键 `subscribeDetails`，区别只在往哪一层找。
//
// 上一版在这里找的是 `list` / `records` / `instances` 三个**凭空造的键名**，
// 注释里也承认"零样本、总得有一处给人改"。旧 TS 给了真键名 ⇒ 就地改正，
// 不留那三个：多找一个猜的键，表现是"某天上游真返回了 `data.list`（那是
// `configAppList` 的键）"时把**应用清单当成云机列表**去装机。
//
// 两种都拿不到 ⇒ `Instances` 为 nil、不报错（"没云机"与"读错了层"在这一层
// 仍然分不出来，那正是 [CloudPhoneInstanceList.Nested] 存在的理由）。
//
// ⚠ **一种读不到的形状**：`data` 本身直接是数组。它过得了
// [EnsureCloudPhoneOK]（信封只看 `header.status`），但会被取段的那一步压成 nil
// ⇒ 本函数在这种情况下交"空列表"，**不是**报错。真抓到样本发现是这种形状时，
// 要改的是取段那一层（[EnsureCloudPhoneOK] 的返回类型），不是在这里补一个 map 兜底。
func ParseCloudPhoneInstanceList(envelope *Envelope) (CloudPhoneInstanceList, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneInstanceList{}, perr
	}
	raw, nested := cloudPhoneInstanceArray(data)
	out := CloudPhoneInstanceList{Nested: nested}
	for _, item := range raw {
		obj, _ := item.(map[string]any)
		out.Instances = append(out.Instances, CloudPhoneInstance{
			InstanceID: cloudPhoneString(obj, "instanceId"),
			LockStatus: cloudPhoneFirstInstanceLock(obj),
			Raw:        obj,
		})
	}
	return out, nil
}

// cloudPhoneInstanceArray 按上面那个"先看 `data.data` 是不是对象"的写法找列表。
//
// 第二个返回值报告**取的是哪一层**（true = 深层）。
func cloudPhoneInstanceArray(data map[string]any) ([]any, bool) {
	if data == nil {
		return nil, false
	}
	if inner, ok := data["data"].(map[string]any); ok {
		if list, ok := inner["subscribeDetails"].([]any); ok {
			return list, true
		}
	}
	list, _ := data["subscribeDetails"].([]any)
	return list, false
}

// cloudPhoneFirstInstanceLock 读 `subsProdInstances[0].lockStatus`（旧 TS 的那一条）。
//
// 子数组缺失、为空、首项不是对象 ⇒ 一律 nil。**不**在这一层把 nil 折成 "N"：
// 旧实现把"不是 Y"当成可装，那是它的判断，而它没有任何样本支持（见类型注释）。
func cloudPhoneFirstInstanceLock(item map[string]any) *string {
	if item == nil {
		return nil
	}
	list, ok := item["subsProdInstances"].([]any)
	if !ok || len(list) == 0 {
		return nil
	}
	first, _ := list[0].(map[string]any)
	return cloudPhoneString(first, "lockStatus")
}

// ─────────────────────────────────────────────────────────────────────────────
// 装 APK 那四步（configAppList → gethwToken → instanceApkInstallStatus → apkInstall
// → appInstallTaskStatus）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneConfigAppListRequest 构造"取可装应用清单"那一枪。
//
// ⚠ **零样本**，且成文里它只出现了一个名字（§2.4 第 375 行安装链第三步）：
// 没有任何键名信息 ⇒ body 交回调用方、载荷只交原文。
// 它的 body 走 [CloudPhoneRequest]（带 `version`）只是**按统一形状推**，未核实。
func CloudPhoneConfigAppListRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneConfigAppListPath, data, auth, referer)
}

// CloudPhoneGetHwTokenRequest 构造"取那个 token"那一枪。
//
// ⚠ **零样本**。成文里它只是安装链上的第四环（名字暗示与华为侧的装机通道有关，
// 但**这只是名字**，没有任何成文或样本说明 `hw` 是什么、这个 token 给谁用）。
// 载荷里必然含凭据 ⇒ [CloudPhoneData] 那段"不许进日志"的警告在这一条上最要紧。
func CloudPhoneGetHwTokenRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneGetHwTokenPath, data, auth, referer)
}

// CloudPhoneInstanceApkInstallStatusRequest 构造"这台机装过没有"那一枪。
//
// ⚠ **零样本**。成文 §2.4 第 375 行只说它的读法："已装则 `userInstallApp{existed:1}`"
// ⇒ 它是安装前的**幂等预检**（旧实现靠它跳过重复安装）。
// ⚠ "已装"这一位在响应里**叫什么**没有成文，所以载荷只交原文：
// 把某个键名写死后读成 nil，下一层就会以为"没装过"，于是**真的再装一次**。
func CloudPhoneInstanceApkInstallStatusRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneInstanceApkInstallPath, data, auth, referer)
}

// CloudPhoneApkInstallRequest 构造"往云机装 APK"那一枪。**这是本族副作用最大的一条**。
//
// ⚠ **零样本**，且它改变的是**用户那台云机**上的东西，不是活动页里的一个计数。
// 三点必须让接线的人看见：
//  1. 这一枪**没有**幂等键可带（本族的 body 形状我们一无所知，
//     而 `core.HTTPRequest.IdempotencyKey` 那个头是**我们自己的传输层**用的，
//     上游不认 ⇒ 重试等于再装一次。[HTTPRequest.IsRetryable] 那条口径因此对本族
//     **不放松**：写请求不带幂等键就不许自动重试）；
//  2. 旧实现的前面有 `instanceApkInstallStatus` 这道预检、后面有轮询，
//     本层**不**替它把关（见文件头：加了没证据的判断更危险）；
//  3. 超时之后**不该**重发：装没装成只有再查一次 `appInstallTaskStatus` 说得出，
//     而"再查"与"再装"是两件事。
func CloudPhoneApkInstallRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneApkInstallPath, data, auth, referer)
}

// CloudPhoneAppInstallTaskStatusRequest 构造"装完没有"那一枪。
//
// ⚠ **零样本**。成文 §2.4 第 375 行给了轮询上限：`appInstallTaskStatus` **≤60×1s**。
// 那六个 0 与那个 1s 是**旧实现的常量**、不是上游的承诺；
// 上限与间隔都归编排层，本层只造请求（一条请求回答不了"装了几次"）。
//
// ⚠ **这一条是唯一可以安全反复打的**：它只读。成文没这么说，
// 是从"它被用来轮询"那半句推的（未验证）。
func CloudPhoneAppInstallTaskStatusRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneAppInstallTaskStatusPth, data, auth, referer)
}

// CloudPhoneUserInstallAppRequest 构造**记账**那一枪（`userInstallApp`）。
//
// 旧 TS 在安装链里发它两次，条件不同（`task.ts:315` / `:359`）：
// 预检到"已装" ⇒ 带 `existed:1`；真的装成了 ⇒ **不带** `existed` 这个键。
// ⇒ 这一位不是"可选参数"，是**两种不同的话**，别在调用方填 0 把它省掉。
// 载荷只判信封（`data` 读法零出处）。
func CloudPhoneUserInstallAppRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneUserInstallAppPath, data, auth, referer)
}

// ─────────────────────────────────────────────────────────────────────────────
// 安装链的响应读法（出处：旧 TS `task.ts:296-368` 里那些 `?.` 取值链）
// ─────────────────────────────────────────────────────────────────────────────
//
// ⚠ 这一段是**旧实现的读法**，不是上游承诺的形状。它值钱的程度恰好等于旧 TS
// 值钱的程度：那份代码跑过，所以键名不会是凭空造的；但活动改版后的今天
// 它还认不认，**未测试**。所以每条读法都遵守同一条纪律：**读不到就交 nil**，
// 不补默认值 —— 补出来的默认值会被编排层当成读数。

// 安装链上的三枚状态值，全部是**上游字面量**（旧 TS 用 `===` 比过它们）。
//
// ⚠ 读法见 [cloudPhoneChainInt]：这一族**只收数字、不收数字串**，与旧 TS 的
// `installStatus === 2` 同宽。它故意比包内通用的 [optInt64Field] 窄 ——
// 放宽到"字符串 2 也算"会**改变旧实现的行为**（旧实现在那一格会判"没装过"、
// 于是真的再装一次），而上游到底给不给字符串这一档**零样本**：
// 没有样本时，与旧实现同宽是唯一能说清的选择。
const (
	// CloudPhoneInstallStatusInstalled 是 `instanceApkInstallStatus` 的"这台机装过了"。
	CloudPhoneInstallStatusInstalled int64 = 2
	// CloudPhoneInstallTaskStatusDone 是 `appInstallTaskStatus` 的"装完了"。
	CloudPhoneInstallTaskStatusDone int64 = 2
	// CloudPhoneInstallTaskStatusFailed 是 `appInstallTaskStatus` 的"装砸了"。
	//
	// ⚠ 旧实现在这一档**直接 break 出轮询**、不报错也不记账（`task.ts:347`），
	// 于是"装失败"与"轮询 60 次没结果"在日志里长得一模一样。新实现要区分 ⇒
	// 判据交编排层，本层只保证这一位读得出来。
	CloudPhoneInstallTaskStatusFailed int64 = 3
)

// ParseCloudPhoneHwToken 读 `data.hwToken`（旧 TS `task.ts:296`）。
//
// ⚠ 返回值**是凭据**：不许进日志、错误文案、测试断言，也不进 fixture。
// 缺失 ⇒ nil（旧实现在此情况下放弃这件任务，见 `task.ts:297-300`）。
func ParseCloudPhoneHwToken(envelope *Envelope) (*string, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return nil, perr
	}
	return cloudPhoneString(data, "hwToken"), nil
}

// CloudPhoneApp 是 `configAppList` 里一个可装应用。
//
// 四个键都按旧 TS 的取法点名（`task.ts:288-303, 320, 366`）：
// `packageName` 是预检与安装的包名、`id` 进**记账**那一枪的 `appId`、
// `packageId` 进**安装**那一枪的 `appId`（⚠ 同一个键名、两枪取不同的值 ——
// 装的时候传 `id` 会把另一个包塞进云机）、`apkName` 只用于日志。
//
// ⚠ `id` / `packageId` 的**类型**旧 TS 没保证（它写 `app.id as number`，
// 又写 `String(app.packageId)`）⇒ 这里原样交 `any`，由调用方决定要不要转，
// 转换只发生在它那一层，且必须能看懂转的是什么。
// 落地的形态：本仓信封走 `UseNumber`（[ParseEnvelope]）⇒ 数字到这里是 `json.Number`，
// 拿它当 `float64` 断言会一直失败；要字符串用 `.String()`，那**不丢位**。
type CloudPhoneApp struct {
	PackageName *string
	ID          any
	PackageID   any
	ApkName     *string
	// Raw 是该条目的其余键（**零实拍**）。
	Raw map[string]any
}

// ParseCloudPhoneAppList 读 `data.list`（旧 TS `task.ts:288`）。
//
// `data.list` 缺失或不是数组 ⇒ 空列表、不报错（旧实现把空列表当错误处理，
// 那是编排的判断：它要靠"空"决定放弃这件任务，而不是靠异常）。
func ParseCloudPhoneAppList(envelope *Envelope) ([]CloudPhoneApp, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return nil, perr
	}
	list, _ := data["list"].([]any)
	apps := make([]CloudPhoneApp, 0, len(list))
	for _, item := range list {
		obj, _ := item.(map[string]any)
		apps = append(apps, CloudPhoneApp{
			PackageName: cloudPhoneString(obj, "packageName"),
			ID:          obj["id"],
			PackageID:   obj["packageId"],
			ApkName:     cloudPhoneString(obj, "apkName"),
			Raw:         obj,
		})
	}
	if len(apps) == 0 {
		return nil, nil
	}
	return apps, nil
}

// ParseCloudPhoneInstallStatus 读"这台机装过没有"。
//
// 旧 TS 的取法（`task.ts:310-313`，逐字对照）：
//
//	items = (statusData.data 是数组) ? statusData.data : statusData.data.list ?? []
//	installStatus = items[0].instanceList[0].installStatus
//
// `statusData.data` 就是本层 [EnsureCloudPhoneOK] 交出的那一份 `data` ⇒
// 落下来只有两条：`data` 直接是数组，或取 `data.list`。
//
// ⚠ **第一条在本层读不到**：信封取 `data` 走的是 [cloudPhoneSection]，
// 非对象一律成 nil ⇒ "data 是数组"那种响应到这里就是空列表（读成"没装过"）。
// 要支持那一档改的是**信封层**（[EnsureCloudPhoneOK] 的返回类型），
// 不是在这里再补一次 `data["data"]` —— 那是另一个键，补了会把形状读歪。
// 旧 TS 自己也没样本说它是数组（本口**零实拍**），所以这里只登记、不实现。
//
// 内层再下两层 `instanceList[0]`；任一段读不到 ⇒ nil，**不报错**：
// 旧实现读成 `undefined` 时走的是"没装过"那一支，而那是**它**的判断。
//
// ⚠ 这一位的 nil 在编排层是最危险的一格：nil 当"没装过" ⇒ 真的再发一次安装。
func ParseCloudPhoneInstallStatus(envelope *Envelope) (*int64, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return nil, perr
	}
	items, _ := data["list"].([]any)
	if len(items) == 0 {
		return nil, nil
	}
	first, _ := items[0].(map[string]any)
	instances, _ := first["instanceList"].([]any)
	if len(instances) == 0 {
		return nil, nil
	}
	entry, _ := instances[0].(map[string]any)
	return cloudPhoneChainInt(entry, "installStatus"), nil
}

// ParseCloudPhoneInstallSubmit 读 `apkInstall` 那一枪的回执：`{success, taskId}`。
//
// 旧 TS（`task.ts:328-332`）：`data.success` 不真 ⇒ **跳过这个应用**（不报错）；
// 真 ⇒ 取 `data.taskId` 去轮询。这里两样都交出去，让调用方能分清
// "上游说没成"与"上游没说"（`Success == nil`）—— 后者按旧实现会被当成"没成"，
// 而那是把"读不到"当"读到否"，本层不替它圆这个谎。
func ParseCloudPhoneInstallSubmit(envelope *Envelope) (CloudPhoneInstallSubmit, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneInstallSubmit{}, perr
	}
	return CloudPhoneInstallSubmit{
		Success: cloudPhoneChainBool(data, "success"),
		TaskID:  cloudPhoneString(data, "taskId"),
	}, nil
}

// CloudPhoneInstallSubmit 是安装回执。
//
// ⚠ `TaskID` 在旧 TS 里过的是 `String(data.taskId ?? "")` ⇒ 数字型 taskId
// 会被转成十进制字符串。这里同样收字符串，但**不猜数字那一档**：
// 读到数字就交 nil（真抓到样本发现是数字，改 [cloudPhoneString] 的取法，
// 而不是在这里加一次浮点转换 —— 那会把"没这个键"与"键是数字"压成同一格）。
type CloudPhoneInstallSubmit struct {
	Success *bool
	TaskID  *string
}

// ParseCloudPhoneInstallTaskStatus 读轮询那一枪的 `data.taskStatus`。
//
// 读不到 ⇒ nil（旧实现把 nil 当"继续轮询"，也就是**只认 2 和 3 两个终止值**）。
func ParseCloudPhoneInstallTaskStatus(envelope *Envelope) (*int64, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return nil, perr
	}
	return cloudPhoneChainInt(data, "taskStatus"), nil
}

// cloudPhoneChainInt 取安装链上的整数格：**只收数字，不收数字串**。
//
// 与包内通用的 [optInt64Field] 刻意不同（那个走 [asInt64]，字符串 "2" 也收）。
// 理由与上面三枚常量的注释同一条：旧 TS 用的是 `=== 2` 严格相等，
// 放宽就等于改变旧实现的行为，而这一族**零实拍**，没有样本支持我们放宽。
func cloudPhoneChainInt(obj map[string]any, key string) *int64 {
	if obj == nil {
		return nil
	}
	switch v := obj[key].(type) {
	case json.Number:
		n, err := v.Int64()
		if err != nil {
			return nil
		}
		return &n
	case float64:
		n := int64(v)
		return &n
	case int:
		n := int64(v)
		return &n
	case int64:
		return &v
	}
	return nil
}

// cloudPhoneChainBool 取安装链上的布尔格：只收**真布尔**，其余（含 `"true"`、`1`）⇒ nil。
//
// ⚠ 这一位不能顺手用 [cloudPhoneChainInt] 替：旧 TS 写的是 `if (!data.success)`，
// 在它眼里 `0`、`"0"`、`null` 与缺键**全是假**；本层把"不是布尔"交成 nil，
// 就是为了让调用方看得见"上游这次根本没给这一格"。
func cloudPhoneChainBool(obj map[string]any, key string) *bool {
	if obj == nil {
		return nil
	}
	flag, ok := obj[key].(bool)
	if !ok {
		return nil
	}
	return &flag
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppCloudPhoneInstances 列云机实例（一次一查；`ACTIVE`+`PENDING` 两查聚合归编排层）。
//
// **只读**。
func (u *Upstream) AppCloudPhoneInstances(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneInstanceList, *core.Error) {
	request, perr := CloudPhoneRelationshipRequest(auth, referer, data)
	if perr != nil {
		return CloudPhoneInstanceList{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneInstanceList{}, perr
	}
	return ParseCloudPhoneInstanceList(envelope)
}

// AppCloudPhoneConfigAppList 取可装应用清单。**只读，零样本载荷**。
func (u *Upstream) AppCloudPhoneConfigAppList(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneData, *core.Error) {
	request, perr := CloudPhoneConfigAppListRequest(auth, referer, data)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseCloudPhoneData(envelope)
}

// AppCloudPhoneGetHwToken 取那个 token。**零样本，且返回的是凭据**：
// 交调用方保管，不许进日志/结论/测试断言。
func (u *Upstream) AppCloudPhoneGetHwToken(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneData, *core.Error) {
	request, perr := CloudPhoneGetHwTokenRequest(auth, referer, data)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseCloudPhoneData(envelope)
}

// AppCloudPhoneInstanceApkInstallStatus 查"装过没有"。**只读，零样本载荷**。
//
// ⚠ 它是 `AppCloudPhoneApkInstall` 的**前置**（成文记的旧实现靠它跳过重复安装），
// 但"读到什么才算已装"这一格今天**填不上**（键名零样本）⇒ 判据归编排层，
// 而编排层在补到样本之前不该判。
func (u *Upstream) AppCloudPhoneInstanceApkInstallStatus(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneData, *core.Error) {
	request, perr := CloudPhoneInstanceApkInstallStatusRequest(auth, referer, data)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseCloudPhoneData(envelope)
}

// AppCloudPhoneApkInstall **真的往云机装 APK**。写请求、零样本、无幂等键。
//
// 载荷只判信封、不交字段：装这件事的第一手读数在
// `AppCloudPhoneAppInstallTaskStatus`（轮询那一条）上，不在这一次响应里 ——
// 而这一次响应里有什么，**零样本**。
func (u *Upstream) AppCloudPhoneApkInstall(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) *core.Error {
	request, perr := CloudPhoneApkInstallRequest(auth, referer, data)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = EnsureCloudPhoneOK(envelope)
	return perr
}

// AppCloudPhoneAppInstallTaskStatus 查安装任务状态（轮询用）。**只读，零样本载荷**。
func (u *Upstream) AppCloudPhoneAppInstallTaskStatus(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneData, *core.Error) {
	request, perr := CloudPhoneAppInstallTaskStatusRequest(auth, referer, data)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseCloudPhoneData(envelope)
}

// AppCloudPhoneUserInstallApp 发**记账**那一枪（`userInstallApp`）。**写**。
//
// 与 [Upstream.AppCloudPhoneApkInstall] 的区别要说清：那一条改的是云机，
// 这一条只改活动侧的"这件任务做过没有"。载荷只判信封（`data` 读法零出处）。
func (u *Upstream) AppCloudPhoneUserInstallApp(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) *core.Error {
	request, perr := CloudPhoneUserInstallAppRequest(auth, referer, data)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = EnsureCloudPhoneOK(envelope)
	return perr
}
