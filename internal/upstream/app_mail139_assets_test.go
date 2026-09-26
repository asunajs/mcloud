package upstream

// 139 邮箱**资产侧**那三口（⑤⑥⑦⑧）的**协议层**用例（玩法 1004 / 1014 / 1020 的前置）。
//
// # 这一族一个样本都没有，所以本文件钉的全是"形状"，没有一条钉"上游认不认"
//
// ⑤⑥⑦⑧ 在 `har/` 与夹具里零命中（文件头那句扫描记录），形状唯一出处是旧 TS
// `apis/mail139.ts:73-160` ⇒ 这里断言的是"我们发出去的**就是那份**字节"，
// 而"上游收到之后认不认"由**真跑**答（`internal/app/live_probe_test.go`），不由本文件答。
// ⚠ 反过来说：如果将来一次真跑证明这一族要的是另一种头 / 另一种体，**改的是实现与这里的
// 期望**，不是把这一条用例删掉装作没钉过。
//
// 五类断言：
//
//  1. **URL 与查询串逐字** —— 参数**顺序**、两枚空值参数（`behaviorData=` 与 `k=`）、
//     末尾的 `comefrom=166`。反向改坏：把 `comefrom` 换成 `Mail139SourceID` ⇒ 红。
//  2. **两个 host 不许合并** —— ⑤⑥ 在 `html5.`、⑦ 在 `appmail.`、⑧ 在裸 `mail.`，
//     而 ③④ 那两发 404 打的正是 `mail.` 这一台 ⇒ "同一片域"不是"同一台"。
//  3. **`sid` 走编码** —— 带 `+` / `&` 的值必须变成 `%2B` / `%26`，否则一份请求
//     会**静默**变成另一份（同 ③ 那一格；对实拍那种 hex 形状是恒等变换）。
//  4. **XML 体的标签族** —— `fileRealSize` 与 `convertType` 是 `<int>` 而不是 `<string>`；
//     ⑥ 的五键顺序；`fileName` / `fileId` 过转义。
//  5. **凭据只进不出，而类别要留住** —— 四口每一次出口的错误文本都不许含 `sid` / `rmkey`，
//     含时仍按原类别交回（`Network` 不能洗成 `Internal`）。
//
// # 凭据边界
//
// 全部用合成哨兵串。⚠ `mailSynthRMKey`（上一族那份）**故意**带 `+ / =` 三枚保留字符：
// 它是用来证明"头里裸拼、URL 里编码"这两条路各自行为正确的。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	mailAssetCguid = "012345678"
	mailAssetRnd   = "0.123456789012345"
	// mailAssetSidReserved 专门用来钉编码那一格：`+` 在查询串里会被解成空格，
	// `&` 会截断参数 ⇒ 不编码等于换了一份请求。
	mailAssetSidReserved = "synth+SID&tail=1"
)

func mailAssetSession(sid, rmkey string) MailSession {
	return MailSession{SID: core.NewSecretValue(sid), RMKey: core.NewSecretValue(rmkey)}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑤ 枪：surplusNum（只读）
// ─────────────────────────────────────────────────────────────────────────────

// TestMailSurplusNumRequestShape 钉 ⑤ 的 URL、八枚头与那三键体。
//
// 反向改坏：URL 少一枚 `&k=` ⇒ 红；体里把 `channel` 挪到最前 ⇒ 红；
// 给它补上 `authorization` ⇒ 红（旧 TS 这一族走原生 fetch，上游收到过的那一份没有它）。
func TestMailSurplusNumRequestShape(t *testing.T) {
	request, perr := MailSurplusNumRequest(mailAssetSession(mailSynthSID, mailSynthRMKey),
		mailAssetCguid, mailAssetRnd)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	wantURL := "https://html5.mail.10086.cn/mw2/file/disk" +
		"?func=disk:surplusNum&sid=" + mailSynthSID +
		"&behaviorData=&rnd=" + mailAssetRnd + "&cguid=" + mailAssetCguid + "&k=&comefrom=166"
	if request.URL != wantURL {
		t.Errorf("URL 实得 %q，期望 %q", request.URL, wantURL)
	}
	wantBody := "<object>\n  <string name=\"appType\">1</string>\n" +
		"  <string name=\"appVer\">10.0.3</string>\n" +
		"  <string name=\"channel\">10242000</string>\n</object>"
	if string(request.Body) != wantBody {
		t.Errorf("体实得 %q，期望 %q", string(request.Body), wantBody)
	}
	if _, has := mailHeader(request, "authorization"); has {
		t.Error("这一族旧 TS 走原生 fetch ⇒ 上游收到过的那一份不带 authorization，不许顺手补")
	}
	// `content-type` 用的是**这一族自己那枚**（无空格、大写 UTF-8），不是 ② 实拍那枚。
	if got, _ := mailHeader(request, "content-type"); got != "application/xml;charset=UTF-8" {
		t.Errorf("content-type 实得 %q，期望旧 TS 那一枚（无空格）", got)
	}
	// Cookie 里两枚**都**在，且顺序照旧 TS：`RMKEY=…; Os_SSo_Sid=…`。
	wantCookie := "RMKEY=" + mailSynthRMKey + "; Os_SSo_Sid=" + mailSynthSID
	if got, _ := mailHeader(request, "cookie"); got != wantCookie {
		t.Errorf("cookie 实得 %q，期望 %q", got, wantCookie)
	}
	for name, want := range map[string]string{
		"x-requested-with": "cn.cj.pe",
		"origin":           "https://html5.mail.10086.cn",
		"referer":          "https://html5.mail.10086.cn/",
		"accept":           "*/*",
	} {
		if got, _ := mailHeader(request, name); got != want {
			t.Errorf("%s 实得 %q，期望 %q", name, got, want)
		}
	}
	if got, _ := mailHeader(request, "user-agent"); !strings.HasPrefix(got, "Mozilla/5.0 (Linux; Android 14;") {
		t.Errorf("user-agent 应是旧 TS 那枚 Android WebView 串，实得 %q", got)
	}
}

// TestMailDiskQueryEncodesReservedSID 钉"带保留字符的 sid 不会静默换一份请求"。
//
// 反向改坏：把 [mailDiskQuery] 里两处 [PercentEncode] 换成裸拼 ⇒ 三条断言同时红。
// ⚠ 实拍那枚 `sid` 是 hex 形态（探针量到长度 38），所以这一格在真跑里**看不出来** ——
// 它防的是"哪天猫上换一个字符集"。
func TestMailDiskQueryEncodesReservedSID(t *testing.T) {
	request, perr := MailSurplusNumRequest(mailAssetSession(mailAssetSidReserved, mailSynthRMKey),
		mailAssetCguid, mailAssetRnd)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if strings.Contains(request.URL, "sid="+mailAssetSidReserved) {
		t.Errorf("sid 被裸拼进查询串了：%s", request.URL)
	}
	if !strings.Contains(request.URL, "sid=synth%2BSID%26tail%3D1") {
		t.Errorf("sid 未按 RFC3986 编码，实得 %s", request.URL)
	}
	// 头里那一枚**是**裸值（旧 TS 就那样发）：编码只作用于 URL，不作用于 Cookie。
	if got, _ := mailHeader(request, "cookie"); !strings.Contains(got, "RMKEY="+mailSynthRMKey) {
		t.Errorf("cookie 里的 RMKEY 应保持原值，实得 %q", got)
	}
}

// TestMailAssetRequestsRejectMissingInputs 钉"缺参数就**不发**"，且错误文本不含凭据。
//
// 反向改坏：把 `cguid == ""` 那一格改成"照样拼空串" ⇒ 第一条红；
// 把错误文案改成 `fmt.Sprintf("…%s…", session.SID.Value())` ⇒ 三条全红。
func TestMailAssetRequestsRejectMissingInputs(t *testing.T) {
	t.Run("缺随机参数", func(t *testing.T) {
		_, perr := MailSurplusNumRequest(mailAssetSession(mailSynthSID, mailSynthRMKey), "", mailAssetRnd)
		if perr == nil {
			t.Fatal("缺 cguid 应报错不发")
		}
		if strings.Contains(perr.Error(), mailSynthSID) || strings.Contains(perr.Error(), mailSynthRMKey) {
			t.Errorf("错误文本漏出凭据：%s", perr.Error())
		}
		if !strings.Contains(perr.Error(), "rnd") {
			t.Errorf("应点名缺的是哪两枚，实得 %s", perr.Error())
		}
	})
	t.Run("缺会话", func(t *testing.T) {
		_, perr := MailConvertPDFRequest(mailAssetSession("", mailSynthRMKey),
			mailAssetCguid, mailAssetRnd, "a.pdf", "fid", 10, 1)
		if perr == nil {
			t.Fatal("缺 sid 应报错不发")
		}
		if strings.Contains(perr.Error(), mailSynthRMKey) {
			t.Errorf("错误文本漏出 rmkey：%s", perr.Error())
		}
	})
	t.Run("缺 fileId", func(t *testing.T) {
		_, perr := MailConvertPDFRequest(mailAssetSession(mailSynthSID, mailSynthRMKey),
			mailAssetCguid, mailAssetRnd, "a.pdf", "", 10, 1)
		if perr == nil || !strings.Contains(perr.Error(), "fileId") {
			t.Fatalf("空 fileId 应报错并点名它，实得 %v", perr)
		}
	})
	t.Run("负的 fileRealSize", func(t *testing.T) {
		_, perr := MailConvertPDFRequest(mailAssetSession(mailSynthSID, mailSynthRMKey),
			mailAssetCguid, mailAssetRnd, "a.pdf", "fid", -1, 1)
		if perr == nil || !strings.Contains(perr.Error(), "-1") {
			t.Fatalf("负数应报错并把值念出来，实得 %v", perr)
		}
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑥ 枪：convertPdfFromYunFile（写）
// ─────────────────────────────────────────────────────────────────────────────

// TestMailConvertPDFRequestBody 钉五键顺序、`<int>` 标签族与两处转义。
//
// 反向改坏：把 `fileRealSize` 写成 `<string>` ⇒ 红；把 `convertType` 挪到 `fileName` 前 ⇒ 红；
// 把 `escapeXMLText` 摘掉 ⇒ 第二条红（`&` 会把体打成两个节点）。
func TestMailConvertPDFRequestBody(t *testing.T) {
	request, perr := MailConvertPDFRequest(mailAssetSession(mailSynthSID, mailSynthRMKey),
		mailAssetCguid, mailAssetRnd, "a&b.pdf", "fid<>1", 1234, 3)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if !strings.Contains(request.URL, "func=disk:convertPdfFromYunFile") {
		t.Errorf("func 不对：%s", request.URL)
	}
	wantBody := "<object>\n  <string name=\"fileName\">a&amp;b.pdf</string>\n  " +
		"<int name=\"fileRealSize\">1234</int>\n  " +
		"<string name=\"fileId\">fid&lt;&gt;1</string>\n  " +
		"<int name=\"convertType\">3</int>\n  " +
		"<string name=\"appType\">1</string><string name=\"appVer\">10.0.3</string>" +
		"<string name=\"channel\">10242000</string>\n</object>"
	if string(request.Body) != wantBody {
		t.Errorf("体实得 %q，期望 %q", string(request.Body), wantBody)
	}
}

// TestMailConvertPDFKeepsOutOfRangeConvertType 钉"四档越界不判、原样落进体"。
//
// 这一条**故意**钉一个看起来不该过的形状：旧 TS 的类型域是 `1|2|3|4`，但**没有任何成文**
// 说过 0 或 9 该被本仓挡下。挡它等于我们从零样本里编一条规则（AGENTS.md §1.7）。
// 反向改坏：加一条 `if convertType < 1 || convertType > 4 { 报错 }` ⇒ 这一条红。
func TestMailConvertPDFKeepsOutOfRangeConvertType(t *testing.T) {
	request, perr := MailConvertPDFRequest(mailAssetSession(mailSynthSID, mailSynthRMKey),
		mailAssetCguid, mailAssetRnd, "a.pdf", "fid", 1, 9)
	if perr != nil {
		t.Fatalf("越界的 convertType 不该被本层挡下: %v", perr.Message())
	}
	if !strings.Contains(string(request.Body), "<int name=\"convertType\">9</int>") {
		t.Errorf("值应原样落进体，实得 %q", string(request.Body))
	}
}

// TestAppMailConvertPDFJudgement 钉"只有 S_OK 算认"，且宽严两种解法都走得到。
//
// 单引号那一档对这一族**有出处**（旧 TS 的 `parseLooseJson` 就是为 `RmWeb` / `mw2` 写的），
// 与 ② 那一发的"外推"不同 —— 那一格的说明在 `app_mail139.go` 的 [parseMailJSONText] 头注。
//
// `wantDir` 那一列钉的是 2026-09-26 实拍的形状：成功那一发 `var.directoryId` 非空、
// `var.fileId` **空串**。反向改坏它：把读 `directoryId` 改成读 `fileId` ⇒ 空串 ⇒ 这一条红。
func TestAppMailConvertPDFJudgement(t *testing.T) {
	cases := []struct {
		name      string
		body      string
		wantError bool
		wantText  string
		wantDir   string
	}{
		{"标准 JSON 的 S_OK", `{"code":"S_OK","var":{"taskId":"1"}}`, false, "", ""},
		{"单引号的 S_OK", `{'code':'S_OK'}`, false, "", ""},
		{
			"实拍两发 S_OK 的三格：目录 id 有、fileId 空",
			`{'code':'S_OK','summary':'已成功转存至：移动云盘个人云>139邮箱-我的附件-PDF转换',` +
				`'var':{'directoryId':'DIR-33','fileId':'','html5Url':'https://example'}}`,
			false, "", "DIR-33",
		},
		{
			"非 S_OK 念上游给的两枚字符串",
			`{"code":"E_LIMIT","summary":"权益用完了"}`,
			true, "E_LIMIT", "",
		},
		{
			"实拍：权益用完那一发（code 是十位数字串）",
			`{"code":"1010010111","summary":"用户权益已经使用完",` +
				`"var":{"directoryId":"","fileId":"","html5Url":""}}`,
			true, "用户权益已经使用完", "",
		},
		{"解不开只说字节数与状态", `not json at all`, true, "解不开", ""},
		{"顶层不是对象", `[{"code":"S_OK"}]`, true, "不是对象", ""},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			client := &mailFailingClient{result: core.HTTPResponse{
				Status: 200, Headers: []core.HeaderPair{{"content-type", "text/javascript"}},
				Body: core.Body(c.body)}}
			receipt, got := NewUpstream(client).AppMailConvertPDF(context.Background(),
				mailAssetSession(mailSynthSID, mailSynthRMKey),
				mailAssetCguid, mailAssetRnd, "a.pdf", "fid", 1, 1)
			if c.wantError {
				if got == nil {
					t.Fatal("应回错误")
				}
				if receipt.DirectoryID != "" {
					t.Errorf("没认的那一发不该交回目录 id，实得 %q", receipt.DirectoryID)
				}
				if c.wantText != "" && !strings.Contains(got.Error(), c.wantText) {
					t.Errorf("错误文本应含 %q，实得 %s", c.wantText, got.Error())
				}
			} else if got != nil {
				t.Fatalf("S_OK 应认，实得 %v", got.Message())
			} else if receipt.DirectoryID != c.wantDir {
				t.Errorf("目录 id 应为 %q，实得 %q", c.wantDir, receipt.DirectoryID)
			}
			if got != nil {
				if strings.Contains(got.Error(), mailSynthSID) || strings.Contains(got.Error(), mailSynthRMKey) {
					t.Errorf("错误文本漏出凭据：%s", got.Error())
				}
				if strings.Contains(got.Error(), c.body) {
					t.Errorf("不该把响应体整块摊进错误文本：%s", got.Error())
				}
			}
		})
	}
}

// TestAppMailConvertPDFFailsOnStatus 钉"非 2xx 是错误"，且 `summary` 会被念出来。
//
// 这一条走的是 [Upstream.Raw] 那一档（非 2xx 直接成错误），断的是"错误还在、
// 状态码也在"—— 反向改坏：把 `Raw` 换成不看状态的直发 ⇒ 这一条红。
func TestAppMailConvertPDFFailsOnStatus(t *testing.T) {
	client := &mailFailingClient{result: core.HTTPResponse{Status: 500, Body: core.Body(`{"code":"E_X"}`)}}
	receipt, got := NewUpstream(client).AppMailConvertPDF(context.Background(),
		mailAssetSession(mailSynthSID, mailSynthRMKey),
		mailAssetCguid, mailAssetRnd, "a.pdf", "fid", 1, 1)
	if got == nil {
		t.Fatal("500 应交回错误")
	}
	if receipt.DirectoryID != "" {
		t.Errorf("非 2xx 那一发不该交回目录 id，实得 %q", receipt.DirectoryID)
	}
	if !strings.Contains(got.Error(), "500") {
		t.Errorf("应带 HTTP 状态，实得 %s", got.Error())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑦ 枪：getComposeId
// ─────────────────────────────────────────────────────────────────────────────

// TestMailComposeIDRequestShape 钉"另一台主机、另一套头、空体"。
//
// 三个不许被同族惯例带跑的格子：① host 是 `appmail.`（不是 `html5.` 也不是裸 `mail.`）；
// ② `Cookie` 里**只有** `RMKEY`，没有 `Os_SSo_Sid`（⑤⑥ 有，这一口没有）；
// ③ 体是**空串**而不是 `{}` 或"没有 body"（旧 TS 传的就是 `body: ""`）。
func TestMailComposeIDRequestShape(t *testing.T) {
	request, perr := MailComposeIDRequest(mailAssetSession(mailSynthSID, mailSynthRMKey))
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	wantURL := "https://appmail.mail.10086.cn/RmWeb/mail?func=mbox:getComposeId&sid=" + mailSynthSID
	if request.URL != wantURL {
		t.Errorf("URL 实得 %q，期望 %q", request.URL, wantURL)
	}
	if string(request.Body) != "" {
		t.Errorf("体应是空串，实得 %q", string(request.Body))
	}
	cookie, _ := mailHeader(request, "cookie")
	if cookie != "RMKEY="+mailSynthRMKey {
		t.Errorf("这一口只带 RMKEY 一枚，实得 %q", cookie)
	}
	if names := strings.Join(mailHeaderNames(request), ","); names != "accept,content-type,cookie,user-agent" {
		t.Errorf("头集合实得 %q，期望那四枚", names)
	}
}

// TestParseMailComposeIDReadsBothShapes 钉旧实现那两种 `var` 读法各自都接得住。
//
// 反向改坏：只留字符串那一支 ⇒ 两条对象档红；只留 `.m` ⇒ `.id` 那一档红。
func TestParseMailComposeIDReadsBothShapes(t *testing.T) {
	cases := []struct {
		name string
		body string
		want string
	}{
		{"var 是字符串", `{"code":"S_OK","var":"CID-1"}`, "CID-1"},
		{"var 对象带 m", `{'code':'S_OK','var':{'m':'CID-2','id':'CID-9'}}`, "CID-2"},
		{"var 对象只有 id", `{"code":"S_OK","var":{"id":"CID-3"}}`, "CID-3"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, perr := ParseMailComposeID(core.HTTPResponse{Status: 200, Body: core.Body(c.body)})
			if perr != nil {
				t.Fatalf("应接住: %v", perr.Message())
			}
			if got != c.want {
				t.Errorf("实得 %q，期望 %q", got, c.want)
			}
		})
	}
}

// TestParseMailComposeIDFailures 钉三种失败各自的文本形状，且都不放大载荷。
func TestParseMailComposeIDFailures(t *testing.T) {
	t.Run("非 S_OK 念 code 与 summary", func(t *testing.T) {
		_, perr := ParseMailComposeID(core.HTTPResponse{
			Status: 200, Body: core.Body(`{"code":"E_AUTH","summary":"会话过期"}`)})
		if perr == nil {
			t.Fatal("应报错")
		}
		for _, want := range []string{"E_AUTH", "会话过期"} {
			if !strings.Contains(perr.Error(), want) {
				t.Errorf("应含 %q，实得 %s", want, perr.Error())
			}
		}
	})
	t.Run("S_OK 但 var 两档都没有 ⇒ 交回键名", func(t *testing.T) {
		_, perr := ParseMailComposeID(core.HTTPResponse{
			Status: 200, Body: core.Body(`{"code":"S_OK","var":{"other":"x"}}`)})
		if perr == nil {
			t.Fatal("应报错")
		}
		if !strings.Contains(perr.Error(), "code,var") {
			t.Errorf("应按键名升序交出顶层键，实得 %s", perr.Error())
		}
	})
	t.Run("没有 code 念成人话", func(t *testing.T) {
		_, perr := ParseMailComposeID(core.HTTPResponse{Status: 200, Body: core.Body(`{"var":"CID"}`)})
		if perr == nil || !strings.Contains(perr.Error(), "没有 code") {
			t.Fatalf("实得 %v", perr)
		}
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑧ 枪：mailsms/sms/backup
// ─────────────────────────────────────────────────────────────────────────────

// TestMailBackupRequestShape 钉它与 ③ 同处置（同主机、authorization 两档），而与 ⑤⑥⑦ 不同。
func TestMailBackupRequestShape(t *testing.T) {
	session := mailAssetSession(mailSynthSID, mailSynthRMKey)
	withAuth, perr := MailBackupRequest(session, mailSynthAuth)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if want := "https://mail.10086.cn/mailsms/sms/backup?sid=" + mailSynthSID; withAuth.URL != want {
		t.Errorf("URL 实得 %q，期望 %q", withAuth.URL, want)
	}
	if got, _ := mailHeader(withAuth, "authorization"); got != mailSynthAuth {
		t.Errorf("默认应带上 authorization，实得 %q", got)
	}
	if string(withAuth.Body) != "{}" {
		t.Errorf("体应是 {}（旧 TS 那一发传的就是空对象），实得 %q", string(withAuth.Body))
	}
	if _, has := mailHeader(withAuth, "cookie"); has {
		t.Error("这一口旧 TS 不拼 RMKEY cookie，别从同族搬过来")
	}
	withoutAuth, perr := MailBackupRequest(session, "")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if _, has := mailHeader(withoutAuth, "authorization"); has {
		t.Error("传空就该一个 authorization 都不带")
	}
}

// TestAppMailBackupRedactsTransportError 钉"URL 里那枚 sid 不外泄，而类别还在"。
//
// 这一条与 ③ 那条同族（`TestAppMailAutoLoginRedactsURLSecrets`）：传输层错误原样嵌 URL，
// 而这一族的 URL 里就是凭据。反向改坏：摘掉 [redactError] ⇒ 第一条红；
// 把重建写成 [core.NewNetwork] 一盖 ⇒ 第二条红（90001 会被洗成网络错）。
func TestAppMailBackupRedactsTransportError(t *testing.T) {
	session := mailAssetSession(mailSynthSID, mailSynthRMKey)
	request, perr := MailBackupRequest(session, mailSynthAuth)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	client := &mailFailingClient{
		err: core.NewNetwork(`Post "` + request.URL + `": ` + mailSynthErr)}
	got := NewUpstream(client).AppMailBackup(context.Background(), session, mailSynthAuth)
	if got == nil {
		t.Fatal("传输层错误应交回")
	}
	if strings.Contains(got.Error(), mailSynthSID) {
		t.Errorf("错误文本漏出 sid：%s", got.Error())
	}
	if got.Kind() != core.KindNetwork {
		t.Errorf("类别应保持 Network，实得 %v", got.Kind())
	}
}

// TestAppMailBackupIgnoresBody 钉"2xx 一律算发过了，本层不解析体"。
//
// ⚠ 这一条**不是**"任务完成了"：那一问由编排层复查读数答（文件头与 [Upstream.AppMailBackup]
// 头注都写了）。这里钉的是"别在这一层加推断"—— 反向改坏：在这里补一句 `code` 判定 ⇒ 红。
func TestAppMailBackupIgnoresBody(t *testing.T) {
	client := &mailFailingClient{result: core.HTTPResponse{
		Status: 200, Body: core.Body("<html>not json</html>")}}
	if got := NewUpstream(client).AppMailBackup(context.Background(),
		mailAssetSession(mailSynthSID, mailSynthRMKey), mailSynthAuth); got != nil {
		t.Fatalf("2xx + 非 JSON 体在本层不该报错，实得 %v", got.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑤ 的响应读数
// ─────────────────────────────────────────────────────────────────────────────

// TestParseMailSurplusNumReadings 钉两枚读数，以及"没有就交回空串而不是猜 0"。
//
// 空串那一档是**有意的**：这一口没有"必须有什么"的判据在手（形状零样本），
// 把缺键写成 `0` 会让界面上"权益为零"与"上游没说话"长得一样（AGENTS.md §1.7）。
func TestParseMailSurplusNumReadings(t *testing.T) {
	t.Run("两枚都在", func(t *testing.T) {
		count, status, perr := ParseMailSurplusNum(core.HTTPResponse{Status: 200, Body: core.Body(
			`{'code':'S_OK','var':{'benefitSurplusCount':'7','benefitStatus':'1'}}`)})
		if perr != nil {
			t.Fatalf("应解得开: %v", perr.Message())
		}
		if count != "7" || status != "1" {
			t.Errorf("实得 %q / %q，期望 7 / 1", count, status)
		}
	})
	t.Run("var 缺席 ⇒ 空串", func(t *testing.T) {
		count, status, perr := ParseMailSurplusNum(core.HTTPResponse{
			Status: 200, Body: core.Body(`{"code":"S_OK"}`)})
		if perr != nil {
			t.Fatalf("解得开就不该报错: %v", perr.Message())
		}
		if count != "" || status != "" {
			t.Errorf("缺键应交回空串，实得 %q / %q", count, status)
		}
	})
	t.Run("解不开 ⇒ 错误带状态与字节数", func(t *testing.T) {
		_, _, perr := ParseMailSurplusNum(core.HTTPResponse{Status: 404, Body: core.Body("<html/>")})
		if perr == nil {
			t.Fatal("应报错")
		}
		for _, want := range []string{"404", "7 字节"} {
			if !strings.Contains(perr.Error(), want) {
				t.Errorf("应含 %q，实得 %s", want, perr.Error())
			}
		}
		if strings.Contains(perr.Error(), mailSynthSID) {
			t.Errorf("错误文本不该含凭据：%s", perr.Error())
		}
	})
}

// TestAppMailSurplusNumRedactsTransportError 钉 ⑤ 同样过脱敏（它的 URL 里也有 sid）。
func TestAppMailSurplusNumRedactsTransportError(t *testing.T) {
	session := mailAssetSession(mailSynthSID, mailSynthRMKey)
	request, perr := MailSurplusNumRequest(session, mailAssetCguid, mailAssetRnd)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	client := &mailFailingClient{
		err: core.NewNetwork(`Post "` + request.URL + `": ` + mailSynthErr)}
	_, _, got := NewUpstream(client).AppMailSurplusNum(context.Background(), session, mailAssetCguid, mailAssetRnd)
	if got == nil {
		t.Fatal("传输层错误应交回")
	}
	if strings.Contains(got.Error(), mailSynthSID) || strings.Contains(got.Error(), mailSynthRMKey) {
		t.Errorf("错误文本漏出凭据：%s", got.Error())
	}
}
