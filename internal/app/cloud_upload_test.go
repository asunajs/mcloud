package app

// 上传编排（#35）的用例：四枪的**顺序**、三态结局、以及"失败了还要不要把 fileId 交回"。
//
// # 这一族断言的是什么
//
// 四枪里三枪零实拍（见 `internal/upstream/app_cloud_upload.go` 文件头）⇒ 这里
// **不钉**"上游真的会回这些字段"，钉的是**编排层自己定的那几条规矩**：
//
//  1. **该少的枪必须少** —— 秒传不发 PUT/complete、`exist` 一枪都不发、
//     空列表不 batchTrash。多发的表现不是报错，是**在用户网盘里多写一次**、
//     或者把别人的文件移进回收站。
//  2. **该多的枪必须多** —— create 成功后无论后面走到哪一步，fileId 都要交回
//     （这是本层与旧实现**刻意不一致**的那一处，理由写在其文件头）。
//  3. **两枪的 contentHash 必须是同一枚**，且等于 PUT 上去那串字节的 sha256。
//
// ⚠ 随机源固定（[seqRandom]）⇒ 文件名与内容都能逐字断言：`UploadRandom` 先取
// 32 字节当内容、再取 4 字节起名，所以实得内容 `"0001…1f"`、名字 `asign-20212223.png`。
// 顺序反了不会报错，只会名字与内容都换一串 —— 这条断言就是钉那个顺序的。

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	uploadPutURLSentinel = "https://synth-oss.example.invalid/put/SYNTH-TOKEN"
	uploadFileIDSentinel = "SYNTH-FID"
	uploadAuthValue      = "Basic U1lOVEhFVElDLUFVVEg="
)

// uploadUploader 用固定随机源与一个脚本客户端搭起编排层。
func uploadUploader(responses ...string) (CloudUploader, *scriptedUpstream) {
	client := &scriptedUpstream{responses: responses}
	return CloudUploader{
		Up:       upstream.NewUpstream(client),
		Rng:      &seqRandom{},
		Auth:     uploadAuthValue,
		DeviceID: "DEVICEID-SENTINEL",
	}, client
}

// TestUploadRandomSendsThreeShotsInOrder ⭐ 正常那一支：create → PUT → complete，恰好三枪。
//
// 反向改坏它：把 complete 挪到 PUT 前面 ⇒ 三条 URL 断言全红；
// 把 `UploadContent` 里那句 hash 改成每次现算 ⇒ hash 一致性红。
func TestUploadRandomSendsThreeShotsInOrder(t *testing.T) {
	uploader, client := uploadUploader(
		`{"code":"0000","success":true,"data":{"exist":false,"rapidUpload":false,`+
			`"fileId":"SYNTH-FID","uploadId":"SYNTH-UID","partInfos":[{"uploadUrl":"`+uploadPutURLSentinel+`"}]}}`,
		``,
		`{"code":"0000","success":true}`,
	)
	receipt, perr := uploader.UploadRandom(context.Background(), CloudUploadOptions{})
	if perr != nil {
		t.Fatalf("三枪应当走通: %v", perr.Message())
	}
	if receipt.Outcome != CloudUploadDone {
		t.Errorf("结局实得 %v", receipt.Outcome)
	}
	if receipt.FileID != uploadFileIDSentinel {
		t.Errorf("fileId 实得 %q", receipt.FileID)
	}
	// 随机源固定 ⇒ 内容与名字可逐字断言（见文件头那条"顺序"）。
	if receipt.Name != "asign-20212223.png" {
		t.Errorf("默认文件名实得 %q", receipt.Name)
	}
	if len(client.requests) != 3 {
		t.Fatalf("应恰好三枪，实得 %d: %v", len(client.requests), client.urls())
	}
	if client.requests[0].Method != core.MethodPost ||
		!strings.HasSuffix(client.requests[0].URL, "/hcy/file/create") {
		t.Errorf("第一枪应是 create，实得 %v %s", client.requests[0].Method, client.requests[0].URL)
	}
	if client.requests[1].Method != core.MethodPut || client.requests[1].URL != uploadPutURLSentinel {
		t.Errorf("第二枪应是 PUT 到预签名 URL，实得 %v %s", client.requests[1].Method, client.requests[1].URL)
	}
	if client.requests[2].Method != core.MethodPost ||
		!strings.HasSuffix(client.requests[2].URL, "/hcy/file/complete") {
		t.Errorf("第三枪应是 complete，实得 %v %s", client.requests[2].Method, client.requests[2].URL)
	}

	// PUT 的 body 就是内容本身，且 create/complete 两处的 hash 是同一枚、
	// 且确实等于这串字节的 sha256（三处任一一枚算错，上游都会说"内容不完整"）。
	content := string(client.requests[1].Body)
	if content == "" || len(content) != 64 {
		t.Fatalf("随机内容应是 64 个十六进制字符，实得 %d", len(content))
	}
	wantHash := core.SHA256Hex([]byte(content))
	createBody := decodeUploadBody(t, client.requests[0].Body)
	completeBody := decodeUploadBody(t, client.requests[2].Body)
	if createBody["contentHash"] != wantHash || completeBody["contentHash"] != wantHash {
		t.Errorf("两枪的 contentHash 必须都等于 sha256(实发字节)：%v / %v",
			createBody["contentHash"], completeBody["contentHash"])
	}
	if createBody["size"] != float64(len(content)) {
		t.Errorf("size 应是字节数 %d，实得 %v", len(content), createBody["size"])
	}
	if createBody["name"] != receipt.Name {
		t.Error("create 发的名字必须与收据上那一个一致")
	}
	if completeBody["fileId"] != uploadFileIDSentinel || completeBody["uploadId"] != "SYNTH-UID" {
		t.Errorf("complete 没带上 create 给的两个 id：%v", completeBody)
	}
	// 默认档：两枪都走 android（旧 TS 的默认），且 complete **不**偷偷换档。
	for i, request := range []core.HTTPRequest{client.requests[0], client.requests[2]} {
		if got := headerValue(request, "x-yun-app-channel"); got != string(upstream.CloudChannelAndroid) {
			t.Errorf("第 %d 枪的渠道档实得 %q", i, got)
		}
	}
}

// TestUploadRapidUploadSkipsPutAndComplete ⭐ 秒传只发一枪，但**仍交回 fileId**。
//
// 这一条钉的是"少发两枪 ≠ 不用清理"：网盘里确实有那一个文件，
// 漏记账的表现是它永远留在用户网盘里。
func TestUploadRapidUploadSkipsPutAndComplete(t *testing.T) {
	uploader, client := uploadUploader(
		`{"code":"0000","success":true,"data":{"exist":false,"rapidUpload":true,` +
			`"fileId":"SYNTH-FID","uploadId":null,"partInfos":null}}`)
	receipt, perr := uploader.UploadRandom(context.Background(), CloudUploadOptions{})
	if perr != nil {
		t.Fatalf("秒传不是错误: %v", perr.Message())
	}
	if receipt.Outcome != CloudUploadRapid {
		t.Errorf("结局实得 %v", receipt.Outcome)
	}
	if receipt.FileID != uploadFileIDSentinel {
		t.Error("秒传也必须交回 fileId，否则清理那一族找不到它")
	}
	if len(client.requests) != 1 {
		t.Errorf("秒传应只发一枪，实得 %d: %v", len(client.requests), client.urls())
	}
}

// TestUploadExistingFileSendsNothingAndKeepsNoID ⭐ `exist:true` ⇒ 不记账。
//
// 反过来写（把已存在的 id 也交回去）的后果是**删用户自己的文件**：
// 那一个同名对象不是这一趟造的。
func TestUploadExistingFileSendsNothingAndKeepsNoID(t *testing.T) {
	uploader, client := uploadUploader(
		`{"code":"0000","success":true,"data":{"exist":true,"rapidUpload":false,"fileId":"SYNTH-OTHER"}}`)
	receipt, perr := uploader.UploadRandom(context.Background(), CloudUploadOptions{})
	if perr != nil {
		t.Fatalf("已存在不是错误: %v", perr.Message())
	}
	if receipt.Outcome != CloudUploadExists || receipt.FileID != "" {
		t.Errorf("实得 %+v", receipt)
	}
	if len(client.requests) != 1 {
		t.Errorf("已存在时不该再发任何一枪，实得 %v", client.urls())
	}
}

// TestUploadKeepsFileIDWhenLaterShotsFail ⭐ 本层与旧实现**刻意不同**的那一条。
//
// 旧实现在 PUT / complete 失败时 `return false` 而**不记账**（`utils/file.ts:70/89`），
// 于是"网盘里已经有一个坑、但没人知道它的 id"。这里两种失败都交回 FileID。
//
// 反向改坏它：把 `receipt.FileID` 的赋值挪到成功之后 ⇒ 这两条红。
func TestUploadKeepsFileIDWhenLaterShotsFail(t *testing.T) {
	t.Run("PUT 失败", func(t *testing.T) {
		uploader, _ := uploadUploader()
		failing := &failingUploadClient{after: 2}
		uploader.Up = upstream.NewUpstream(failing)
		receipt, perr := uploader.UploadRandom(context.Background(), CloudUploadOptions{})
		if perr == nil {
			t.Fatal("PUT 非 2xx 应当是错误")
		}
		if receipt.FileID != uploadFileIDSentinel {
			t.Error("PUT 失败时也必须交回 fileId：占位已经成功，那是清理唯一的依据")
		}
		if len(failing.requests) != 2 {
			t.Errorf("PUT 失败后不该再发 complete，实得 %v", failing.urls())
		}
	})
	t.Run("complete 失败", func(t *testing.T) {
		uploader, _ := uploadUploader()
		failing := &failingUploadClient{after: 3}
		uploader.Up = upstream.NewUpstream(failing)
		receipt, perr := uploader.UploadRandom(context.Background(), CloudUploadOptions{})
		if perr == nil {
			t.Fatal("complete 业务失败应当是错误")
		}
		if receipt.FileID != uploadFileIDSentinel {
			t.Error("complete 失败时同样要交回 fileId")
		}
		if receipt.Outcome.String() != "未上传" {
			t.Errorf("出错时结局必须是零值「未上传」，实得 %v", receipt.Outcome)
		}
	})
}

// TestUploadWithoutURLFailsLoudly 钉"缺 uploadUrl"不会被念成成功。
//
// 这是零实拍最可能撞上的那一格：上游改了字段名（比如挪到 `data.uploadUrl`），
// 兜底写法（当空串继续）会让链条"跑完但什么都没传"。
func TestUploadWithoutURLFailsLoudly(t *testing.T) {
	uploader, client := uploadUploader(
		`{"code":"0000","success":true,"data":{"exist":false,"fileId":"SYNTH-FID","uploadId":"SYNTH-UID"}}`)
	receipt, perr := uploader.UploadRandom(context.Background(), CloudUploadOptions{})
	if perr == nil {
		t.Fatal("没有 uploadUrl 必须是错误")
	}
	if receipt.FileID != uploadFileIDSentinel {
		t.Error("这一支也要交回 fileId（占位已经成功）")
	}
	if len(client.requests) != 1 {
		t.Errorf("缺 URL 时不该发后两枪，实得 %v", client.urls())
	}
	if !strings.Contains(perr.Message(), "uploadUrl") {
		t.Errorf("错误文本要指名缺哪一格，实得 %q", perr.Message())
	}
}

// TestDeleteCloudFilesGuardsEmptyList 钉"没有要清理的东西"与"清理成功"是两档。
func TestDeleteCloudFilesGuardsEmptyList(t *testing.T) {
	uploader, client := uploadUploader()
	sent, perr := uploader.DeleteCloudFiles(context.Background(), nil)
	if perr != nil {
		t.Fatalf("空列表不该报错: %v", perr.Message())
	}
	if sent {
		t.Error("空列表不该发请求（旧实现同样在调用侧挡：after.ts:133）")
	}
	if len(client.requests) != 0 {
		t.Errorf("实得 %v", client.urls())
	}

	uploader2, client2 := uploadUploader(`{"code":"0000","success":true,"result":{}}`)
	sent, perr = uploader2.DeleteCloudFiles(context.Background(), []string{"SYNTH-A"})
	if !sent || perr != nil {
		t.Fatalf("非空列表该发并且成功：sent=%v err=%v", sent, perr)
	}
	if len(client2.requests) != 1 ||
		!strings.HasSuffix(client2.requests[0].URL, "/hcy/recyclebin/batchTrash") {
		t.Errorf("删除那一枪实得 %v", client2.urls())
	}
	// 删除固定 web 档，与上传用的档位无关（旧 TS `apis/file.ts:114`）。
	if got := headerValue(client2.requests[0], "x-yun-app-channel"); got != string(upstream.CloudChannelWeb) {
		t.Errorf("删除应走 web 档，实得 %q", got)
	}
}

// TestDeleteReportsUpstreamFailure 钉"上游没给 success"不能念成删除成功。
func TestDeleteReportsUpstreamFailure(t *testing.T) {
	uploader, _ := uploadUploader(`{"code":"0000","success":false,"message":"回收站已满"}`)
	sent, perr := uploader.DeleteCloudFiles(context.Background(), []string{"SYNTH-A"})
	if !sent {
		t.Error("这一枪确实发出去了，sent 该是 true")
	}
	if perr == nil {
		t.Fatal("success:false 必须是错误")
	}
}

// TestUploadOutcomeZeroValueIsNothing ⭐ 零值必须是"未上传"。
//
// 反向改坏它：把 `CloudUploadDone` 排到 iota 起点 ⇒ 这一条红，而那一改的真实后果是
// "任何忘记赋值的路径都报成功" —— 于是清理那一族少拿一个 id。
func TestUploadOutcomeZeroValueIsNothing(t *testing.T) {
	var zero CloudUploadOutcome
	if zero != CloudUploadNothing {
		t.Errorf("零值应是 CloudUploadNothing，实得 %v", zero)
	}
	if zero.String() != "未上传" {
		t.Errorf("零值的人话实得 %q", zero.String())
	}
	for _, o := range []CloudUploadOutcome{CloudUploadDone, CloudUploadRapid, CloudUploadExists} {
		if o == CloudUploadNothing {
			t.Error("档位撞了")
		}
	}
}

// TestUploadHonoursExplicitOptions 钉"显式参数不被默认值覆盖"。
func TestUploadHonoursExplicitOptions(t *testing.T) {
	uploader, client := uploadUploader(
		`{"code":"0000","success":true,"data":{"exist":false,"fileId":"SYNTH-FID",`+
			`"uploadId":"SYNTH-UID","partInfos":[{"uploadUrl":"`+uploadPutURLSentinel+`"}]}}`,
		``, `{"code":"0000","success":true}`,
	)
	if _, perr := uploader.UploadContent(context.Background(), []byte("SYNTH-CONTENT"),
		CloudUploadOptions{
			ParentFileID: "SYNTH-DIR", Name: "given.bin", Ext: ".ignored",
			Channel: upstream.CloudChannelPC, Op: upstream.CloudOpBackup,
		}); perr != nil {
		t.Fatalf("跑通不该报错: %v", perr.Message())
	}
	body := decodeUploadBody(t, client.requests[0].Body)
	if body["name"] != "given.bin" || body["parentFileId"] != "SYNTH-DIR" {
		t.Errorf("显式参数被改写了：%v", body)
	}
	// PC 档不读 opType ⇒ 两枪的头应当一致（这是旧实现的形状，不是实测）。
	first := headerValue(client.requests[0], "x-yun-op-type")
	third := headerValue(client.requests[2], "x-yun-op-type")
	if first != "1" || third != "1" {
		t.Errorf("PC 档 backup 仍发 op-type 1（旧实现如此），实得 %q / %q", first, third)
	}
	if got := headerValue(client.requests[0], "User-Agent"); got != "Mozilla/5.0" {
		t.Errorf("PC 档该覆盖 UA，实得 %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// failingUploadClient 按顺序回放：前 `after-1` 枪成功，第 `after` 枪失败。
//
// 用它是因为"失败发生在第几枪"本身就是这条链的判据：PUT 失败不该再发 complete。
type failingUploadClient struct {
	after     int
	requests  []core.HTTPRequest
	done      int
	responses []string
}

func (c *failingUploadClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	c.done++
	if c.done == c.after {
		return core.NewResponse(503, []byte(`{"code":"9"}`)), nil
	}
	// 成功档：create 回一份能往下走的 data，其余回"成功信封"。
	if strings.HasSuffix(request.URL, "/hcy/file/create") {
		return core.NewResponse(200, []byte(
			`{"code":"0000","success":true,"data":{"exist":false,"fileId":"SYNTH-FID",`+
				`"uploadId":"SYNTH-UID","partInfos":[{"uploadUrl":"`+uploadPutURLSentinel+`"}]}}`)), nil
	}
	return core.NewResponse(200, []byte(`{"code":"0000","success":true}`)), nil
}

func (c *failingUploadClient) urls() []string {
	out := make([]string, 0, len(c.requests))
	for _, r := range c.requests {
		out = append(out, r.URL)
	}
	return out
}

// decodeUploadBody 把请求体解成键值表。
func decodeUploadBody(t *testing.T, body []byte) map[string]any {
	t.Helper()
	var obj map[string]any
	if err := json.Unmarshal(body, &obj); err != nil {
		t.Fatalf("请求体不是 JSON 对象: %v（实得 %s）", err, body)
	}
	return obj
}
