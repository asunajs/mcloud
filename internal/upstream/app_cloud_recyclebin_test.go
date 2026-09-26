package upstream

// 回收站那一族只读口（`/hcy/recyclebin/list`）的**协议层**用例。
//
// # 这一族能钉什么
//
// 出处只有一份，而且是**代码**不是流量：App 端 JS 包
// （`docs/capture/apiServer.js` 的 `getVirDirInfo`）里那一句请求形状。
// 所以这里钉的是"我们对那一份出处的转写有没有走样"：URL、体里的键集合、
// 空游标那一档是"带空串"还是"省略键"（出处是 `item.nextPageCursor || ""` ⇒ 前者），
// 以及**头画像沿用同族 batchTrash**这一件（它是我自己选的，不是出处要求的，
// 所以钉成"与 [CloudBatchTrashRequest] 同一档"，将来要改必须显式改两处）。
//
// ⚠ 这里**没有**任何一条断言在钉"上游回什么"：响应侧零样本（那一份 JS 里 `getVirDirInfo`
// 只有定义、无调用点，一个字段都没读），第一次读数由活体探针
// `internal/app/live_probe_recycle_bin_test.go` 收。因此也**刻意不落**响应解析器 ——
// 拿没读过的形状写结构体，等于把猜测固化成接口层。
//
// # 凭据边界
//
// 沿用上一族的合成哨兵（`uploadAuthSentinel` / `uploadDeviceSentinel`）。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// TestCloudRecycleBinListRequestIsAppJSLiteralShape ⭐ 钉"体里只有 pageInfo 那一枚键"。
//
// 反向改坏它的样子：把 `pageSize` 提到顶层、或"顺手"补一枚 `fileIds`/`type` ⇒ 红。
func TestCloudRecycleBinListRequestIsAppJSLiteralShape(t *testing.T) {
	request, perr := CloudRecycleBinListRequest(uploadAuthSentinel, uploadDeviceSentinel, "")
	if perr != nil {
		t.Fatalf("构造回收站列口失败: %v", perr.Message())
	}
	if request.URL != BasePersonalKD+"/hcy/recyclebin/list" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	if request.Method != core.MethodPost {
		t.Errorf("方法实得 %v（出处是 sendRequest 的 json 档 ⇒ POST）", request.Method)
	}
	obj := bodyKeys(t, request.Body)
	if got := strings.Join(sortedKeys(obj), ","); got != "pageInfo" {
		t.Errorf("体顶层键集合实得 %s（那一份 JS 里只有 pageInfo 一枚）", got)
	}
	page, ok := obj["pageInfo"].(map[string]any)
	if !ok {
		t.Fatalf("pageInfo 不是对象，实得 %T", obj["pageInfo"])
	}
	if got := strings.Join(sortedKeys(page), ","); got != "pageCursor,pageSize" {
		t.Errorf("pageInfo 键集合实得 %s", got)
	}
	// 空游标那一档：出处是 `item.nextPageCursor || ""` ⇒ **带着空串发**，不是省略键。
	// 反向改坏它：写成 `if cursor != "" { … }` 会让第一页发成"只有一个 pageSize 的 pageInfo"，
	// 而"上游怎么看那一枚缺席的键"今天零样本 ⇒ 不许自行发明。
	cursor, present := page["pageCursor"]
	if !present {
		t.Error("第一页也必须带 pageCursor（出处是空串，不是省略）")
	}
	if cursor != "" {
		t.Errorf("空游标实得 %v", cursor)
	}
	// 页长在那个包里写死 100（旁边留着 `//item.pageSize` ⇒ 本来可调）。
	width, isNum := page["pageSize"].(float64)
	if !isNum || width != CloudRecycleBinPageSize {
		t.Errorf("pageSize 实得 %v（出处是数字 100，不是字符串）", page["pageSize"])
	}
}

// TestCloudRecycleBinListUsesBatchTrashChannel ⭐ 钉"头画像沿用同族那一档"（web + op*100）。
//
// 这一枚断言钉的是**我自己的选择**：出处只给了 URL 与体，头画像没人给过。
// 选"与 batchTrash 同一档"的理由是那两份实现里 list 与 batchTrash 都出自同一个请求包装，
// 而 batchTrash 那一档本仓已真打通过几十次。把它钉住，是为了"将来换画像必须显式改两处"，
// 而不是让它悄悄跟着别的函数的默认值漂走。
func TestCloudRecycleBinListUsesBatchTrashChannel(t *testing.T) {
	list, perr := CloudRecycleBinListRequest(uploadAuthSentinel, uploadDeviceSentinel, "SYNTH-CURSOR")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	trash, perr := CloudBatchTrashRequest(uploadAuthSentinel, uploadDeviceSentinel, []string{"SYNTH-ID"})
	if perr != nil {
		t.Fatalf("构造 batchTrash 失败: %v", perr.Message())
	}
	// 头面按 [CloudFileOpHeaders] **实际发出去的那几枚**列。⚠ 别照着旧 TS 源码里的属性名
	// （`channelSrc`）写断言：那一族落到线上是 `x-yun-app-channel` + `x-huawei-channelSrc`
	// 两枚，写成 `channelSrc` 会"两边都抓到 0 枚 ⇒ 相等 ⇒ 通过"，即假绿。
	for _, name := range []string{"authorization", "deviceid", "x-yun-user-agent",
		"x-yun-sub-op-type", "content-type", "x-yun-api-version", "x-yun-app-channel",
		"x-huawei-channelSrc", "x-yun-client-info", "x-yun-device-id"} {
		gotList, gotTrash := countHeader(list.Headers, name), countHeader(trash.Headers, name)
		if gotList != gotTrash {
			t.Errorf("%s 枚数与 batchTrash 不一致：list=%d trash=%d", name, gotList, gotTrash)
		}
		// 值也要一致：这一档画像里"有几枚"与"填的是什么"是两格（PC 档就靠覆盖 User-Agent 区分）。
		if v, w := headerValue(list.Headers, name), headerValue(trash.Headers, name); v != w {
			t.Errorf("%s 的值与 batchTrash 不一致：list=%q trash=%q", name, v, w)
		}
	}
	// 上面那条循环抓不到"两边都没这一枚"那一档 ⇒ 这里正面钉 list **确实**带了渠道那一枚，
	// 且值就是 batchTrash 写死的 web 档（否则整条比较可以在"两串空头"上自洽通过）。
	if got := countHeader(list.Headers, "x-yun-app-channel"); got != 1 {
		t.Errorf("x-yun-app-channel 应恰好一枚，实得 %d", got)
	}
	if got := headerValue(list.Headers, "x-yun-app-channel"); got != string(CloudChannelWeb) {
		t.Errorf("渠道档实得 %q（沿用 batchTrash 那一份写死的 web 档）", got)
	}
	// 非空游标要原样带上（翻页那一格由探针第一次读数后再定键名）。
	page := bodyKeys(t, list.Body)["pageInfo"].(map[string]any)
	if page["pageCursor"] != "SYNTH-CURSOR" {
		t.Errorf("非空游标被改写，实得 %v", page["pageCursor"])
	}
}

// TestCloudRecycleBinListRoundTripsThroughSendLayer 钉"构造 → 发送 → 信封解得动"这一串接线。
//
// ⚠ 它**不**证明上游接受，也**不**证明响应里有 `items`：合成体按"同族信封"那一档写
// （`code` 是字符串、载荷在 `data`），因为那一档是本族**有实拍**的部分。
// 探针拿到真实形状后如果要改这里的合成体，改的是**夹具**，不是断言的强度。
func TestCloudRecycleBinListRoundTripsThroughSendLayer(t *testing.T) {
	client := &scriptedClient{response: core.HTTPResponse{
		Status: 200,
		Body: core.Body([]byte(
			`{"code":"0000","success":true,"message":"请求成功","data":{"items":[]}}`)),
	}}
	request, perr := CloudRecycleBinListRequest(uploadAuthSentinel, uploadDeviceSentinel, "")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	envelope, cerr := NewUpstream(client).Call(context.Background(), request)
	if cerr != nil {
		t.Fatalf("发送层闭环失败: %v", cerr.Message())
	}
	payload, oerr := envelope.EnsureOK()
	if oerr != nil {
		t.Fatalf("信封按同族那一档解不开: %v", oerr.Message())
	}
	if _, ok := payload.(map[string]any); !ok {
		t.Errorf("data 不是对象，实得 %T", payload)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应恰好一发，实得 %d", len(client.requests))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应侧（形状取自 2026-09-26 第廿七趟真跑，值全部换成合成号）
// ─────────────────────────────────────────────────────────────────────────────

// recycleBinSynthPage 是实拍那一页的**形状**：folder 型带 `trashedDirStatistics` 对象、
// `thumbnailUrls:null`；file 型反过来（统计是 `null`、缩略图是数组）。
//
// ⚠ 值的出处：`remainingTime` 的 `14` / `0` 两档与 `trashedReason:"userTrashed"` 是实拍念到的；
// `category` / `style` / `userTags` 里的内容**没有成文**，这里一律写 SYNTH 占位 ——
// 本层也不读它们（见 [CloudRecycleBinEntry] 那条字段级说明）。
const recycleBinSynthPage = `{"code":"0000","message":"请求成功","success":true,"data":{` +
	`"items":[` +
	`{"category":"SYNTH-CAT","createdAt":"2026-09-26T15:15:55.000+08:00","fileId":"SYNTH-DIR-ID",` +
	`"mediaMetaInfo":null,"name":"asign-binlistdir-synth0001","remainingTime":14,"size":0,` +
	`"thumbnailUrls":null,"trashedAt":"2026-09-26T15:15:56.161+08:00",` +
	`"trashedDirStatistics":{"subDirCount":0,"subFileCount":0,"totalSize":0},` +
	`"trashedReason":"userTrashed","type":"folder","updatedAt":"2026-09-26T15:15:56.161+08:00",` +
	`"userTags":[{"key":"SYNTH-K","value":"SYNTH-V"}]},` +
	`{"category":"SYNTH-CAT","createdAt":"2026-09-12T14:10:49.970+08:00","fileId":"SYNTH-FILE-ID",` +
	`"mediaMetaInfo":null,"name":"asign-synth.png","remainingTime":0,"size":64,` +
	`"thumbnailUrls":[{"style":"SYNTH","url":"https://synth-read.example.invalid/thumb"}],` +
	`"trashedAt":"2026-09-12T14:10:49.970+08:00","trashedDirStatistics":null,` +
	`"trashedReason":"userTrashed","type":"file","updatedAt":"2026-09-26T03:44:12.871+08:00",` +
	`"userTags":[]}` +
	`],"nextPageCursor":"SYNTH-CURSOR-STRING"}}`

// TestParseCloudRecycleBinPageReadsRealShape ⭐ 钉"实拍形状交回来是什么"。
//
// 反向改坏它的样子：把 `trashedDirStatistics` 的判空改成"null 也给一个零值对象" ⇒
// 第二条的 `DirStats != nil` 红（那一格把"这是文件"读成了"空目录"）；
// 把 `nextPageCursor` 读成"缺席也给空串"而不看 HasCursorField ⇒ 后面两条红。
func TestParseCloudRecycleBinPageReadsRealShape(t *testing.T) {
	page, perr := ParseCloudRecycleBinPage(envelopeOf(t, recycleBinSynthPage))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if len(page.Items) != 2 {
		t.Fatalf("应解出两条，实得 %d", len(page.Items))
	}
	if page.NextPageCursor != "SYNTH-CURSOR-STRING" || !page.HasCursorField {
		t.Errorf("游标实得 %q / 键在否 %v", page.NextPageCursor, page.HasCursorField)
	}
	dir, file := page.Items[0], page.Items[1]
	if dir.Name == nil || *dir.Name != "asign-binlistdir-synth0001" {
		t.Errorf("folder 的 name 实得 %v", dir.Name)
	}
	if dir.DirStats == nil {
		t.Fatal("folder 型该有目录统计（file 型才是 null，两档不能糊）")
	}
	if dir.DirStats.SubFileCount == nil || *dir.DirStats.SubFileCount != 0 {
		t.Errorf("subFileCount 实得 %v（实拍这一格是 0，尽管删之前目录里有 1 枚文件）",
			dir.DirStats.SubFileCount)
	}
	if file.DirStats != nil {
		t.Errorf("file 型的 null 被编造成了对象：%+v", file.DirStats)
	}
	if file.Size == nil || *file.Size != 64 {
		t.Errorf("file 的 size 实得 %v", file.Size)
	}
	if dir.RemainingTime == nil || *dir.RemainingTime != 14 ||
		file.RemainingTime == nil || *file.RemainingTime != 0 {
		t.Errorf("remainingTime 两档实得 %v / %v", dir.RemainingTime, file.RemainingTime)
	}
	if file.TrashedAt == nil || *file.TrashedAt != "2026-09-12T14:10:49.970+08:00" {
		t.Errorf("trashedAt 实得 %v", file.TrashedAt)
	}
}

// TestParseCloudRecycleBinPageSeparatesNullCursorFromAbsentCursor ⭐ 两档必须分得开。
//
// 实拍里 `nextPageCursor` **存在且为 null**（= 没有下一页）；"这一枚键缺席"是第三种情况
// （形状变了）。两者都读成"没有下一页"会让一次上游改版静默变成"回收站只有一页"。
func TestParseCloudRecycleBinPageSeparatesNullCursorFromAbsentCursor(t *testing.T) {
	nullCase := `{"code":"0000","success":true,"data":{"items":[],"nextPageCursor":null}}`
	page, perr := ParseCloudRecycleBinPage(envelopeOf(t, nullCase))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if !page.HasCursorField || page.NextPageCursor != "" {
		t.Errorf("null 游标应读成「键在、没有下一页」，实得 键在=%v 值=%q",
			page.HasCursorField, page.NextPageCursor)
	}
	if len(page.Items) != 0 {
		t.Errorf("空 items 应给 0 条，实得 %d", len(page.Items))
	}
	absentCase := `{"code":"0000","success":true,"data":{"items":[]}}`
	absent, perr := ParseCloudRecycleBinPage(envelopeOf(t, absentCase))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if absent.HasCursorField {
		t.Error("键缺席被读成「键在」⇒ 与 null 那一档糊成一格")
	}
}

// TestParseCloudRecycleBinPageDoesNotFoldShapeChangeIntoEmptyBin 钉"`data` 缺失/不是对象 ⇒ 报错"。
//
// 反向：把它兜成空的一页 ⇒ 这一条红，而且上层会把"这一口改版了"念成"回收站是空的"。
func TestParseCloudRecycleBinPageDoesNotFoldShapeChangeIntoEmptyBin(t *testing.T) {
	for _, body := range []string{
		`{"code":"0000","success":true}`,
		`{"code":"0000","success":true,"data":[]}`,
		`{"code":"0000","success":true,"data":"SYNTH"}`,
	} {
		if _, perr := ParseCloudRecycleBinPage(envelopeOf(t, body)); perr == nil {
			t.Errorf("这一份不该解得出来：%s", body)
		}
	}
}

// TestParseCloudRecycleBinPageAcceptsStringNumbers 钉本层的**宽松**，并标明它不是上游事实。
//
// `asInt64` 两种形态都认（数字与 `"64"` 这样的串）。⚠ "上游会不会回串"今天**未测试**
// —— 实拍那两条里 `size` / `remainingTime` 都是数字。这一条只保证本层不会因为形态
// 换了就整页报错，**不**支持把"串"写进任何成文当判据。
func TestParseCloudRecycleBinPageAcceptsStringNumbers(t *testing.T) {
	body := `{"code":"0000","success":true,"data":{"items":[{` +
		`"fileId":"SYNTH-ID","name":"asign-synth.png","type":"file",` +
		`"size":"64","remainingTime":"14"}],"nextPageCursor":null}}`
	page, perr := ParseCloudRecycleBinPage(envelopeOf(t, body))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if len(page.Items) != 1 || page.Items[0].Size == nil || *page.Items[0].Size != 64 {
		t.Fatalf("串形态的数字没读出来：%+v", page.Items)
	}
	if page.Items[0].RemainingTime == nil || *page.Items[0].RemainingTime != 14 {
		t.Errorf("remainingTime 实得 %v", page.Items[0].RemainingTime)
	}
}

// TestAppCloudRecycleBinListRoundTripsRealShapeThroughSendLayer 把"构造 → 发送 → 解析"
// 串成一发的闭环（**不**证明上游接受，只证明接线通）。响应那一份是上面合成的实拍形状。
func TestAppCloudRecycleBinListRoundTripsRealShapeThroughSendLayer(t *testing.T) {
	client := &scriptedClient{response: core.HTTPResponse{
		Status: 200, Body: core.Body([]byte(recycleBinSynthPage)),
	}}
	page, perr := NewUpstream(client).AppCloudRecycleBinList(
		context.Background(), uploadAuthSentinel, uploadDeviceSentinel, "")
	if perr != nil {
		t.Fatalf("闭环失败: %v", perr.Message())
	}
	if len(page.Items) != 2 || page.NextPageCursor != "SYNTH-CURSOR-STRING" {
		t.Errorf("闭环解出来的那一页实得 %+v", page)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应恰好一发，实得 %d", len(client.requests))
	}
	if got := countHeader(client.requests[0].Headers, "authorization"); got != 1 {
		t.Errorf("这一型也走 [CloudFileOpHeaders]，authorization 应恰好一枚，实得 %d", got)
	}
}
