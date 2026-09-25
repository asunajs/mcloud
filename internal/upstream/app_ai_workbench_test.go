package upstream

// 玩法 **1028「AI 工作台」**四枪的**协议层**用例。
//
// # 这一族零实拍，所以本文件钉的全是形状
//
// ①② 在 `har/` 里零命中（文件头那句扫描记录），③ 与 585 同 URL 但**另一套头与体**
// ⇒ 本文件断言的是"我们发出去的就是旧 TS 那一份"，而"上游认不认"由真跑答
// （`internal/app/live_probe_test.go` 那一族），不由本文件答。
//
// # 三类最贵的错，本文件各有一组用例对着
//
//  1. **把 585 那一套搬过来**：钉头集合（本族只有七枚、**没有** `Origin` / `x-yun-tid` / 设备头）、
//     钉 `inputTime` 的**本地**格式、钉 SSE 读法的五格与 585 相反之处（含"同一段样本两套读数不同"）。
//  2. **把秘密写进文本**：① 的 DES 密文与解出的手机号、② body 里的 RSA 密文、
//     ③ 的 Basic 整串 —— 三口的错误出口各钉一条"含凭据的响应/错误进来，干净文本出去"，
//     且**类别不许被洗**。
//  3. **把判据读松或读紧**：② 的门是**严格**数字相等（`code===0`、`taskStatus===1`、
//     `cloudStatus===0`）⇒ 字符串形态 `"0"` / `"1"` 必须**不**成立；
//     而"拿不到状态"必须落成 `EnvelopeOK=false`（旧实现那一档是"照样问一句"，不是"跳过"）。
//
// # 凭据边界
//
// 全部用合成值：手机号 `19900001111`（199 段合成号）、token `SYNTH-AI-TOKEN`、
// 会话沿用上一族的 `mailSynthSID` / `mailSynthRMKey`。
// DES 密文由 **crypto-js**（旧实现用的那一枚库）现场加密得到，不是本仓自己解给自己看：
// 四枚向量连同生成方式记在 [TestParseAIWorkbenchUserTokenMatchesCryptoJSVectors]。

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// aiWorkbenchSynthPhone 是合成手机号（199 段，非真实号段）。
const aiWorkbenchSynthPhone = "19900001111"

// aiWorkbenchSynthToken 是 ① 回的那枚裸 token（合成）。
const aiWorkbenchSynthToken = "SYNTH-AI-TOKEN"

// aiWorkbenchCiphertext 是 `DES-ECB/PKCS7(key="139_mail", "19900001111")` 的 base64。
//
// 由 crypto-js 3.x 现场加密得到（与旧实现 `rt.crypto.desDecrypt` 同一枚库、同一组参数），
// ⚠ 不是 Go 这边自产自销：那样只能证明"我的加密和我的解密互逆"，证明不了"和上游那一份一致"。
const aiWorkbenchCiphertext = "KlRGp7lbaaIYgR3idlxaQw=="

// aiWorkbenchSession 是这一族用的那份邮箱会话。
func aiWorkbenchSession() MailSession {
	return mailAssetSession(mailSynthSID, mailSynthRMKey)
}

// aiWorkbenchAuth 手工拼一份"① 已经派生出来"的凭据（发送层用例用）。
func aiWorkbenchAuth() AIWorkbenchDomainAuth {
	return AIWorkbenchDomainAuth{
		Phone:         core.NewSecretValue(aiWorkbenchSynthPhone),
		Token:         core.NewSecretValue(aiWorkbenchSynthToken),
		Authorization: core.NewSecretValue(AppAuthorization(aiWorkbenchSynthPhone, aiWorkbenchSynthToken)),
		UserDomainID:  "1990000000000000001",
	}
}

// aiWorkbenchParams 给一份三格齐全的 ③ 入参。
func aiWorkbenchParams() AIWorkbenchChatParams {
	return AIWorkbenchChatParams{
		UserID:    "1990000000000000001",
		Dialogue:  "你好",
		InputTime: AIWorkbenchInputTime(time.Date(2026, 6, 29, 12, 10, 43, 123_000_000, time.UTC)),
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ① 枪：queryUserToken + DES 派生
// ─────────────────────────────────────────────────────────────────────────────

// TestAIWorkbenchQueryUserTokenRequestShape 钉 ① 的 URL、七枚头与**那个空体**。
//
// 反向改坏：把体写成 ⑤ 那一份三键 XML ⇒ 红；补 `authorization` ⇒ 红；
// 把 `func` 换成 `disk:surplusNum` ⇒ 红（两口同 URL 同头，只在这一格分开）。
func TestAIWorkbenchQueryUserTokenRequestShape(t *testing.T) {
	request, perr := AIWorkbenchQueryUserTokenRequest(aiWorkbenchSession(), mailAssetCguid, mailAssetRnd)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	wantURL := "https://html5.mail.10086.cn/mw2/file/disk" +
		"?func=disk:queryUserToken&sid=" + mailSynthSID +
		"&behaviorData=&rnd=" + mailAssetRnd + "&cguid=" + mailAssetCguid + "&k=&comefrom=166"
	if request.URL != wantURL {
		t.Errorf("URL 实得 %q，期望 %q", request.URL, wantURL)
	}
	if string(request.Body) != "<object></object>" {
		t.Errorf("体实得 %q，期望旧实现那一份空体 <object></object>", string(request.Body))
	}
	if _, has := mailHeader(request, "authorization"); has {
		t.Error("这一族走原生 fetch ⇒ 不许顺手补 authorization")
	}
	if n := mailHeaderCount(request, "cookie"); n != 1 {
		t.Errorf("cookie 枚数实得 %d，期望 1", n)
	}
	if got, _ := mailHeader(request, "cookie"); got != "RMKEY="+mailSynthRMKey+"; Os_SSo_Sid="+mailSynthSID {
		t.Errorf("cookie 实得 %q，顺序或两枚值与旧实现不符", got)
	}
	for name, want := range map[string]string{
		"content-type":     "application/xml;charset=UTF-8",
		"accept":           "*/*",
		"origin":           "https://html5.mail.10086.cn",
		"referer":          "https://html5.mail.10086.cn/",
		"x-requested-with": "cn.cj.pe",
	} {
		if got, _ := mailHeader(request, name); got != want {
			t.Errorf("%s 实得 %q，期望 %q", name, got, want)
		}
	}
	if got, _ := mailHeader(request, "user-agent"); !strings.HasPrefix(got, "Mozilla/5.0 (Linux; Android 14;") {
		t.Errorf("① 用的是 WebView 那枚 UA（与 ②③ 的桌面 UA 不同），实得 %q", got)
	}
}

// TestParseAIWorkbenchUserTokenMatchesCryptoJSVectors 钉 DES 那一格与**参考实现同库**的读数。
//
// 四枚向量各挡一种偏：11 字节（两整块）、9 字节（跨块）、13 字节、以及**多字节 UTF-8**
// （中文 ⇒ 验明文按 UTF-8 收，不是 latin-1）。
// 反向改坏：把 [AIWorkbenchDESKey] 换一枚 ⇒ 四条全红；把 PKCS7 收尾删掉 ⇒ 红。
func TestParseAIWorkbenchUserTokenMatchesCryptoJSVectors(t *testing.T) {
	for _, c := range []struct{ cipher, want string }{
		{"KlRGp7lbaaIYgR3idlxaQw==", "19900001111"},
		{"KlRGp7lbaaKBOvXI8BwzWQ==", "199000011"},
		{"KlRGp7lbaaIWqLlHMvDSHQ==", "1990000111222"},
		{"MO0bFfGIDrM=", "你好"},
	} {
		got, err := core.DESECBPKCS7Decrypt(AIWorkbenchDESKey, c.cipher)
		if err != nil {
			t.Fatalf("%s: %v", c.cipher, err)
		}
		if got != c.want {
			t.Errorf("解出 %q，期望 %q", got, c.want)
		}
	}
}

// TestParseAIWorkbenchUserTokenDerivesBasicAuth 钉 ① 成功档的三格派生。
//
// `Authorization` 要**逐字**等于 `Basic base64("mobile:<手机号>:<token>")`：
// 这一格错一位上游就认不出是谁，而它**没有**任何一列可"看着像"。
// 反向改坏：把 `mobile` 换成 `pc` ⇒ 红；少一层 `var.data.data` ⇒ 红。
func TestParseAIWorkbenchUserTokenDerivesBasicAuth(t *testing.T) {
	body := `{"code":"S_OK","var":{"data":{"data":{` +
		`"account":"` + aiWorkbenchCiphertext + `","token":"` + aiWorkbenchSynthToken + `",` +
		`"userDomainId":"1990000000000000001","provCode":"19"
}}}}`
	auth, perr := ParseAIWorkbenchUserToken(core.NewResponse(200, []byte(body)))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if auth.Phone.Value() != aiWorkbenchSynthPhone {
		t.Errorf("手机号实得 %q", auth.Phone.Value())
	}
	if want := AppAuthorization(aiWorkbenchSynthPhone, aiWorkbenchSynthToken); auth.Authorization.Value() != want {
		t.Errorf("Authorization 实得 %q，期望 %q", auth.Authorization.Value(), want)
	}
	raw, derr := base64.StdEncoding.DecodeString(strings.TrimPrefix(auth.Authorization.Value(), "Basic "))
	if derr != nil {
		t.Fatalf("Authorization 不是合法 base64: %v", derr)
	}
	if parts := strings.Split(string(raw), ":"); len(parts) != 3 ||
		parts[0] != "mobile" || parts[1] != aiWorkbenchSynthPhone || parts[2] != aiWorkbenchSynthToken {
		t.Errorf("Authorization 的三段实得 %#v", parts)
	}
	if auth.UserDomainID != "1990000000000000001" {
		t.Errorf("userDomainId 实得 %q", auth.UserDomainID)
	}
	if !auth.OK() {
		t.Error("有手机号时 OK() 应为真")
	}
}

// TestParseAIWorkbenchUserTokenAcceptsSingleQuoteJSON 钉宽松解法那一遍**在本族也成立**。
//
// 出处：旧实现 `parseLooseJson` 与资产侧同一写法（`'` → `"`）。这一口回的是**双引号**还是
// 单引号 ⇒ 零样本，所以两档都得能过；只留严解那一遍，遇到单引号就变成"①一直失败"。
func TestParseAIWorkbenchUserTokenAcceptsSingleQuoteJSON(t *testing.T) {
	body := `{'code':'S_OK','var':{'data':{'data':{'account':'` + aiWorkbenchCiphertext +
		`','token':'` + aiWorkbenchSynthToken + `'
}}}}`
	auth, perr := ParseAIWorkbenchUserToken(core.NewResponse(200, []byte(body)))
	if perr != nil {
		t.Fatalf("单引号那一档解不开: %v", perr.Message())
	}
	if auth.UserDomainID != "" {
		t.Errorf("没回 userDomainId 时应交回空串，实得 %q", auth.UserDomainID)
	}
	if !auth.OK() {
		t.Error("有手机号与 token 时 OK() 应为真")
	}
}

// TestParseAIWorkbenchUserTokenFailuresSayWhatWithoutEchoingAnything 钉三种失败各自的人话，
// 且**都不 echo** 提交进去或解出来的任何东西。
//
// 反向改坏：把"缺 account"那一支改成静默回零值（旧实现就是 `return undefined`）⇒ 红。
func TestParseAIWorkbenchUserTokenFailuresSayWhatWithoutEchoingAnything(t *testing.T) {
	cases := []struct {
		name  string
		body  string
		wants []string
	}{
		{"上游没认", `{"code":"E_OK","summary":"synth-summary"}`, []string{"code=E_OK", "synth-summary"}},
		{"没 code", `{"var":{"data":{"data":{"account":"AA=="}}}}`, []string{"没有 code"}},
		{"缺 account", `{"code":"S_OK","var":{"data":{"data":{"token":"T"}}}}`,
			[]string{"缺 account=true", "缺 token=false"}},
		{"解不开", `{"code":"S_OK","var":{"data":{"data":{"account":"@@@","token":"T"}}}}`,
			[]string{"account 解不开"}},
		{"形状不对", `{"code":"S_OK","var":"不是对象"}`, []string{"缺 account=true", "缺 token=true"}},
		{"不是 JSON", `not json at all`, []string{"解不开"}},
	}
	for _, c := range cases {
		_, perr := ParseAIWorkbenchUserToken(core.NewResponse(200, []byte(c.body)))
		if perr == nil {
			t.Fatalf("%s：应失败却成功了", c.name)
		}
		text := perr.Error()
		for _, want := range c.wants {
			if !strings.Contains(text, want) {
				t.Errorf("%s：错误文本缺 %q（实得 %q）", c.name, want, text)
			}
		}
		for _, forbidden := range []string{aiWorkbenchCiphertext, aiWorkbenchSynthPhone, "KlRG"} {
			if strings.Contains(text, forbidden) {
				t.Errorf("%s：错误文本泄了 %q", c.name, forbidden)
			}
		}
	}
}

// TestAppAIWorkbenchUserTokenRedactsSession 钉 ① 的出口不泄会话。
//
// 假客户端交回的错误文本**故意含** `sid` 与 `rmkey`（`internal/io` 真会把 URL 原样嵌进
// 传输层错误里，而 ① 的 URL 带 `sid`）。⚠ 按坑表 #168：脱敏用例必须打在**会 echo** 的那一层。
func TestAppAIWorkbenchUserTokenRedactsSession(t *testing.T) {
	client := &sseScriptClient{status: 0, sendErr: fmt.Errorf(
		`Post "https://html5.mail.10086.cn/…?sid=%s": dial tcp: lookup failed（rmkey=%s）`,
		mailSynthSID, mailSynthRMKey)}
	_, perr := NewUpstream(client).AppAIWorkbenchUserToken(
		context.Background(), aiWorkbenchSession(), mailAssetCguid, mailAssetRnd)
	if perr == nil {
		t.Fatal("应失败")
	}
	if strings.Contains(perr.Error(), mailSynthSID) || strings.Contains(perr.Error(), mailSynthRMKey) {
		t.Errorf("错误文本泄了会话：%s", perr.Error())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ②④ 枪：状态查询（RSA）
// ─────────────────────────────────────────────────────────────────────────────

// TestAIWorkbenchStatusRequestCarriesOnlyCiphertext 钉 ② 的"身份全在 body 那一枚密文里"。
//
// 四格断言：URL、四枚头（**没有** authorization）、body 里没有明文手机号、
// 密文是合法 base64 且**两次不同**（PKCS#1 v1.5 的随机填充）。
// 反向改坏：给它补 `Authorization` ⇒ 红；把明文塞进 body ⇒ 红。
func TestAIWorkbenchStatusRequestCarriesOnlyCiphertext(t *testing.T) {
	first, perr := AIWorkbenchStatusRequest(
		core.NewSecretValue(aiWorkbenchSynthPhone), AIWorkbenchTaskID, AIWorkbenchSourceChannel,
		&seqRng{b: 1})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	second, perr := AIWorkbenchStatusRequest(
		core.NewSecretValue(aiWorkbenchSynthPhone), AIWorkbenchTaskID, AIWorkbenchSourceChannel,
		&seqRng{b: 2})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if want := "https://m.mcloud.139.com/ycloud/signin/public/getUserTaskAndCloudStatusExternal"; first.URL != want {
		t.Errorf("URL 实得 %q，期望 %q", first.URL, want)
	}
	if got := mailHeaderNames(first); len(got) != 4 {
		t.Errorf("头应为四枚（content-type/origin/referer/user-agent），实得 %v", got)
	}
	if _, has := mailHeader(first, "authorization"); has {
		t.Error("② 的出处里没有 authorization：它的身份全在 body 那一枚 RSA 密文里")
	}
	if got, _ := mailHeader(first, "origin"); got != "https://appmail.mail.10086.cn" {
		t.Errorf("origin 实得 %q", got)
	}
	if got, _ := mailHeader(first, "user-agent"); got != AIWorkbenchDesktopUserAgent {
		t.Errorf("user-agent 实得 %q，期望桌面那枚", got)
	}
	if first.TimeoutMS != nil {
		t.Errorf("② 不该带请求级超时（出处没有），实得 %v", *first.TimeoutMS)
	}
	var decoded map[string]any
	if uerr := json.Unmarshal(first.Body, &decoded); uerr != nil {
		t.Fatalf("body 不是 JSON: %v", uerr)
	}
	token, _ := decoded["token"].(string)
	if token == "" {
		t.Fatalf("body 缺 token：%s", string(first.Body))
	}
	if strings.Contains(string(first.Body), aiWorkbenchSynthPhone) {
		t.Error("body 里出现了明文手机号")
	}
	raw, derr := base64.StdEncoding.DecodeString(token)
	if derr != nil {
		t.Fatalf("token 不是 base64: %v", derr)
	}
	if len(raw) != 256 {
		t.Errorf("RSA 密文应 256 字节（2048 位公钥），实得 %d", len(raw))
	}
	other, _ := decoded["taskId"]
	if n, _ := other.(float64); n != 1028 {
		t.Errorf("taskId 实得 %v", other)
	}
	if ch, _ := decoded["sourceChannel"].(string); ch != "10175" {
		t.Errorf("sourceChannel 实得 %q", ch)
	}
	var secondDecoded map[string]any
	_ = json.Unmarshal(second.Body, &secondDecoded)
	if secondDecoded["token"] == token {
		t.Error("两次 RSA 密文相同 ⇒ 随机填充没生效")
	}
}

// TestAIWorkbenchStatusRequestRefusesMissingInputs 钉四格护栏（每格都对应一种"必被拒的一发"）。
func TestAIWorkbenchStatusRequestRefusesMissingInputs(t *testing.T) {
	rng := &seqRng{b: 3}
	phone := core.NewSecretValue(aiWorkbenchSynthPhone)
	cases := []struct {
		name    string
		phone   core.SecretValue
		taskID  int64
		channel string
		rng     core.Random
	}{
		{"缺手机号", core.NewSecretValue(""), 1028, "10175", rng},
		{"缺随机源", phone, 1028, "10175", nil},
		{"taskId 为零", phone, 0, "10175", rng},
		{"缺渠道号", phone, 1028, "", rng},
	}
	for _, c := range cases {
		_, perr := AIWorkbenchStatusRequest(c.phone, c.taskID, c.channel, c.rng)
		if perr == nil {
			t.Errorf("%s：应拒发", c.name)
			continue
		}
		if strings.Contains(perr.Error(), aiWorkbenchSynthPhone) {
			t.Errorf("%s：错误文本泄了手机号：%s", c.name, perr.Error())
		}
	}
}

// TestParseAIWorkbenchTaskStatusGateIsStrictNumbers 钉那道门是**严格数字**相等。
//
// 表里每一行都是旧实现那一句 `cs?.code === 0 && cs.result` 与
// `taskStatus === 1 && cloudStatus === 0` 的一个真实边界：字符串形态**不成立**
// （JS 的 `===` 不做隐式转换 ⇒ `"0" !== 0`）。
// ⚠ 反过来"拿不到状态"这一档必须落 `EnvelopeOK=false` 而**不是** `GatePass=true`：
// 旧实现那一档的行为是"照样问一句"，念成"前置已满足"就是把没做说成跳过。
func TestParseAIWorkbenchTaskStatusGateIsStrictNumbers(t *testing.T) {
	cases := []struct {
		name       string
		body       string
		envelopeOK bool
		hasResult  bool
		gate       bool
		taskRead   string
		cloudRead  string
		codeRead   string
	}{
		{"门成立", `{"code":0,"result":{"taskStatus":1,"cloudStatus":0}}`, true, true, true, "1", "0", "0"},
		{"cloudStatus 是 1", `{"code":0,"result":{"taskStatus":1,"cloudStatus":1}}`, true, true, false, "1", "1", "0"},
		{"taskStatus 是 0", `{"code":0,"result":{"taskStatus":0,"cloudStatus":0}}`, true, true, false, "0", "0", "0"},
		{"两枚是字符串", `{"code":0,"result":{"taskStatus":"1","cloudStatus":"0"}}`, true, true, false, "1", "0", "0"},
		{"code 是字符串", `{"code":"0","result":{"taskStatus":1,"cloudStatus":0}}`, false, true, false, "", "", "0"},
		{"code 非零", `{"code":500,"result":{"taskStatus":1,"cloudStatus":0}}`, false, true, false, "", "", "500"},
		{"没有 result", `{"code":0}`, false, false, false, "", "", "0"},
		// ⭐ 探针真跑回来的那一句（taskId=1028 今天不在这号的任务集里）：
		// `code` 是**字符串** ⇒ 与上面"code 是字符串"同档，但它是**真实**形态。
		{"真跑的 501", `{"code":"501","msg":"任务不存在"}`, false, false, false, "", "", "501"},
		{"result 是空对象", `{"code":0,"result":{}}`, true, true, false, "", "", "0"},
		{"result 是 null", `{"code":0,"result":null}`, false, false, false, "", "", "0"},
		{"两枚是负零与小数", `{"code":0,"result":{"taskStatus":1.0,"cloudStatus":0.0}}`, true, true, true,
			"1.0", "0.0", "0"},
	}
	for _, c := range cases {
		read, perr := ParseAIWorkbenchTaskStatus(core.NewResponse(200, []byte(c.body)))
		if perr != nil {
			t.Fatalf("%s: %v", c.name, perr.Message())
		}
		if read.EnvelopeOK != c.envelopeOK || read.HasResult != c.hasResult || read.GatePass != c.gate {
			t.Errorf("%s：三格实得 envelope=%v has=%v gate=%v，期望 %v/%v/%v",
				c.name, read.EnvelopeOK, read.HasResult, read.GatePass,
				c.envelopeOK, c.hasResult, c.gate)
		}
		if read.TaskStatus != c.taskRead || read.CloudStatus != c.cloudRead || read.CodeRaw != c.codeRead {
			t.Errorf("%s：读数实得 %q/%q/%q，期望 %q/%q/%q", c.name,
				read.CodeRaw, read.TaskStatus, read.CloudStatus, c.codeRead, c.taskRead, c.cloudRead)
		}
	}
}

// TestParseAIWorkbenchTaskStatusRealBodiesKeepGateClosed ⭐ 三份**真跑回来的体**：门全不成立。
//
// 出处：2026-09-26 活体探针三趟（`internal/app/live_probe_workbench_test.go`，
// 读数落档在 `docs/capture/app-ai-workbench-samples.txt`）。这一条与上一条表**分工不同**：
// 上一条是"门的规则"（造出来的形状），这一条是"上游今天真实吐出来的那三串字节"。
//
// ⚠⚠ 它钉住的是一件很不舒服的事：**真实响应的 `code` / `taskStatus` / `cloudStatus`
// 三格全是字符串**（`{"code":"0",...,"taskStatus":"1","cloudStatus":"0"}`），
// 而旧实现的门是 JS 的 `===`（严格相等、不做隐式转换）⇒
// **这一族今天永远走"仍发对话"那一档**，连 106 那种读数上是"1/0"的也一样。
// 所以本仓**不放宽**：放宽等于由我们替用户决定"这一条少发一次写"，
// 而它与旧实现的行为也不再一致。要动这一格 ⇒ 先过 `docs/pending-decisions.md` 本族那一问。
func TestParseAIWorkbenchTaskStatusRealBodiesKeepGateClosed(t *testing.T) {
	for _, probed := range []struct {
		name      string
		body      string
		hasResult bool
		codeRead  string
	}{
		{
			// taskId=1028 与 999999 今天真回的那一句（探针三次全部如此）。
			name:      "1028 今天真回的那一句",
			body:      `{"code":"501","msg":"任务不存在"}`,
			hasResult: false,
			codeRead:  "501",
		},
		{
			// ②′-对照枪 taskId=585：result 有，三格全是字符串。
			name:      "对照 585：result 有、三格字符串",
			body:      `{"code":"0","msg":"success","result":{"cloudNum":"20","cloudStatus":"1","taskStatus":"1"}}`,
			hasResult: true,
			codeRead:  "0",
		},
		{
			// taskId=106：两枚状态**字面**上是 1/0（数字门要的那两格），但它们是字符串。
			// ⚠ 这一档是整个决定的核心：读数看着"该跳过对话"，门按出处**不成立**。
			name:      "对照 106：字面像门成立，类型不让",
			body:      `{"code":"0","msg":"success","result":{"cloudNum":"6","cloudStatus":"0","taskStatus":"1"}}`,
			hasResult: true,
			codeRead:  "0",
		},
	} {
		read, perr := ParseAIWorkbenchTaskStatus(core.NewResponse(200, []byte(probed.body)))
		if perr != nil {
			t.Fatalf("%s：真跑回来的体不该解不开：%s", probed.name, perr.Message())
		}
		if read.EnvelopeOK || read.GatePass {
			t.Errorf("%s：字符串形态的 code 不该让门成立，实得 envelope=%v gate=%v",
				probed.name, read.EnvelopeOK, read.GatePass)
		}
		if read.HasResult != probed.hasResult {
			t.Errorf("%s：result 有无实得 %v，期望 %v", probed.name, read.HasResult, probed.hasResult)
		}
		if read.CodeRaw != probed.codeRead {
			t.Errorf("%s：code 读数要**原样带出**（不归一成数字），实得 %q 期望 %q",
				probed.name, read.CodeRaw, probed.codeRead)
		}
		// ⚠ `EnvelopeOK=false` 时两枚状态读数**故意留空**：门那条路已经不看了，
		// 念出来只会让播报出现"taskStatus=1、门=不过"这种读者要费劲对齐的组合。
		// 探针那一趟正是这个形状 ⇒ 这里把它钉住，别将来"顺手补上"。
		if read.TaskStatus != "" || read.CloudStatus != "" {
			t.Errorf("%s：门那条路不成立时不该再交两枚状态，实得 %q/%q",
				probed.name, read.TaskStatus, read.CloudStatus)
		}
	}
}

// TestParseAIWorkbenchTaskStatusRejectsNonJSON 钉"解不开才算失败"那一格。
func TestParseAIWorkbenchTaskStatusRejectsNonJSON(t *testing.T) {
	_, perr := ParseAIWorkbenchTaskStatus(core.NewResponse(200, []byte("<html>风控</html>")))
	if perr == nil || !strings.Contains(perr.Error(), "解不开") {
		t.Fatalf("应报解不开，实得 %v", perr)
	}
	_, perr = ParseAIWorkbenchTaskStatus(core.NewResponse(200, []byte(`[1,2]`)))
	if perr == nil || !strings.Contains(perr.Error(), "不是对象") {
		t.Fatalf("应报不是对象，实得 %v", perr)
	}
}

// TestAppAIWorkbenchTaskStatusKeepsKindAndRedersSecrets 钉出口既留住类别又不泄三格秘密。
func TestAppAIWorkbenchTaskStatusKeepsKindAndRedersSecrets(t *testing.T) {
	client := &sseScriptClient{status: 500, body: "boom", echoErr: true}
	_, perr := NewUpstream(client).AppAIWorkbenchTaskStatus(
		context.Background(), aiWorkbenchAuth(), AIWorkbenchTaskID, AIWorkbenchSourceChannel, &seqRng{b: 9})
	if perr == nil {
		t.Fatal("非 2xx 应失败")
	}
	text := perr.Error()
	for _, forbidden := range []string{aiWorkbenchSynthPhone, aiWorkbenchSynthToken, "Basic ", "bW9iaWxl"} {
		if strings.Contains(text, forbidden) {
			t.Errorf("错误文本泄了 %q：%s", forbidden, text)
		}
	}
	if perr.Kind() != core.KindUpstream {
		t.Errorf("类别应是 Upstream（不能被洗成 Internal），实得 %v", perr.Kind())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ③ 枪：对话请求
// ─────────────────────────────────────────────────────────────────────────────

// TestAIWorkbenchChatRequestHeadersAreTheLegacySeven 钉 ③ 的**头集合本身**（七枚，一枚不多）。
//
// 这一条存在的唯一目的：拦住"复用 585 那套 [playAIAppHeaders]"。那一套会给这条请求
// 补上 `Origin` / `x-yun-tid` / `x-DeviceInfo` / `x-yun-uni` 与一枚签名时间戳 ——
// 而 1028 的出处（`buildAiChatV2Headers`）里**一枚都没有**。
// 反向改坏：改用 585 那个构造函数 ⇒ 头枚数与三枚具体值同时红。
func TestAIWorkbenchChatRequestHeadersAreTheLegacySeven(t *testing.T) {
	request, perr := AIWorkbenchChatRequest(aiWorkbenchAuth().Authorization,
		AIWorkbenchSourceChannel, aiWorkbenchParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if got, want := len(request.Headers), 7; got != want {
		t.Errorf("头枚数实得 %d（%v），期望 %d", got, mailHeaderNames(request), want)
	}
	for name, want := range map[string]string{
		"x-yun-api-version": "v1",
		"x-yun-client-info": "4g||30|||||||1440/900|zh-CN||||",
		"x-yun-app-channel": "10175",
		"Content-Type":      "application/json",
		"Accept":            "text/event-stream",
		"User-Agent":        AIWorkbenchDesktopUserAgent,
		"Authorization":     aiWorkbenchAuth().Authorization.Value(),
	} {
		got, has := mailHeader(request, name)
		if !has {
			t.Errorf("缺头 %s", name)
			continue
		}
		if got != want {
			t.Errorf("%s 实得 %q，期望 %q", name, got, want)
		}
	}
	for _, mustNot := range []string{"origin", "referer", "x-yun-tid", "x-yun-uni", "x-deviceinfo"} {
		if _, has := mailHeader(request, mustNot); has {
			t.Errorf("多了一枚 %s ⇒ 已经变成另一条请求（那是 585 的头集合）", mustNot)
		}
	}
	// `content-type` 只有一枚，且大小写是出处那一份（`WithJSONBody` 补的那枚已被换掉）。
	if n := mailHeaderCount(request, "content-type"); n != 1 {
		t.Errorf("content-type 枚数实得 %d，期望 1", n)
	}
	if request.TimeoutMS == nil || *request.TimeoutMS != 60_000 {
		t.Errorf("③ 应带 60s 上限（旧实现那枚 AbortController），实得 %v", request.TimeoutMS)
	}
	if request.URL != "https://ai.yun.139.com/api/outer/assistant/chat/v2/add" {
		t.Errorf("URL 实得 %q", request.URL)
	}
}

// TestAIWorkbenchChatBodyIsTheLegacyShapeByteForByte 钉 ③ 的体字节。
//
// 与 585 那一套同 URL、**不同体**：外层多 `continuationInfo:null`、少 `applicationId`；
// `dialogueInput` 是另一组 14 键、`attachment` 是空对象。
// 反向改坏：把 `commands` 写成数字 ⇒ 红；把 `inputTime` 换成 UTC+Z 那一档 ⇒ 红。
func TestAIWorkbenchChatBodyIsTheLegacyShapeByteForByte(t *testing.T) {
	request, perr := AIWorkbenchChatRequest(aiWorkbenchAuth().Authorization,
		AIWorkbenchSourceChannel, aiWorkbenchParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	want := `{"applicationType":"chat","continuationInfo":null,"dialogueInput":{` +
		`"aiWritingSetting":{},"attachment":{},"command":null,"commandType":1,"commands":"000",` +
		`"dialogue":"你好","dialogueType":"0","enableAiSearch":false,"enableAllNetworkSearch":false,` +
		`"enableForceNetworkSearch":false,"extInfo":"{}","inputTime":"2026-06-29 12:10:43",` +
		`"prompt":"","resourceId":"","resourceType":"0",` +
		`"toolSetting":{"imageToolSetting":{"enableLlmDescribe":false}},` +
		`"versionInfo":{"h5Version":"2.5.0","pcVersion":"1.7.0"}},"sessionId":"",` +
		`"sourceChannel":"10175","userId":"1990000000000000001"}`
	if string(request.Body) != want {
		t.Errorf("体实得\n%s\n期望\n%s", string(request.Body), want)
	}
}

// TestAIWorkbenchInputTimeIsLocalClockComponents 钉 `inputTime` 的格式**与出处同形**。
//
// 旧实现取的是 `new Date()` 的**本地分量**，Go 这边 `Format` 也不换算时区 ⇒
// 同一个时刻在两个时区下会得到不同的串，这是**出处的行为**（585 那一套才固定 UTC）。
func TestAIWorkbenchInputTimeIsLocalClockComponents(t *testing.T) {
	when := time.Date(2026, 1, 2, 3, 4, 5, 600_000_000, time.FixedZone("synth+8", 8*3600))
	if got := AIWorkbenchInputTime(when); got != "2026-01-02 03:04:05" {
		t.Errorf("实得 %q，期望打印这一时刻自己的挂钟分量（无偏移、无毫秒）", got)
	}
	if got := AIWorkbenchInputTime(time.Date(2026, 12, 31, 23, 59, 58, 0, time.UTC)); got != "2026-12-31 23:59:58" {
		t.Errorf("实得 %q", got)
	}
	// 与 585 那一套**不同形**这一句要能被断言，否则将来一定有人合并。
	if AIWorkbenchInputTime(when) == PlayAIChatCameraInputTime(when) {
		t.Error("两族的 inputTime 相同 ⇒ 其中一族的出处被丢了")
	}
}

// TestAIWorkbenchChatRequestRefusesMissingInputs 钉 ③ 的四格护栏。
func TestAIWorkbenchChatRequestRefusesMissingInputs(t *testing.T) {
	good := aiWorkbenchParams()
	for _, c := range []struct {
		name   string
		mutate func(p *AIWorkbenchChatParams)
	}{
		{"缺 userId", func(p *AIWorkbenchChatParams) { p.UserID = "" }},
		{"缺 dialogue", func(p *AIWorkbenchChatParams) { p.Dialogue = "" }},
		{"缺 inputTime", func(p *AIWorkbenchChatParams) { p.InputTime = "" }},
	} {
		params := good
		c.mutate(&params)
		if _, perr := AIWorkbenchChatRequest(aiWorkbenchAuth().Authorization,
			AIWorkbenchSourceChannel, params); perr == nil {
			t.Errorf("%s：应拒发", c.name)
		}
	}
	if _, perr := AIWorkbenchChatRequest(core.NewSecretValue(""),
		AIWorkbenchSourceChannel, good); perr == nil {
		t.Error("空 Authorization 应拒发")
	} else if strings.Contains(perr.Error(), "Basic") && strings.Contains(perr.Error(), aiWorkbenchSynthPhone) {
		t.Errorf("错误文本泄了凭据：%s", perr.Error())
	}
	if _, perr := AIWorkbenchChatRequest(aiWorkbenchAuth().Authorization, "", good); perr == nil {
		t.Error("空 sourceChannel 应拒发")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ③ 枪：本族自己的 SSE 累加器
// ─────────────────────────────────────────────────────────────────────────────

// TestParseAIWorkbenchSSEFiveBehavioursDifferFromPlayAI 逐格钉那五处与 585 相反的读法。
//
// 每一格都在同一段样本上被断言，且**顺带**跟 585 的 [ParsePlayAIChatSSE] 比一次：
// 两边读数不同才算钉住（如果两套读法结果相同，合并就没人会发现，那一半实现会静默变坏）。
func TestParseAIWorkbenchSSEFiveBehavioursDifferFromPlayAI(t *testing.T) {
	// 样本里同时含：不带前缀的裸 JSON、`[DONE]`、非 JSON 行、两条 outContent 路径、
	// 后到的 sessionId、以及**末尾没有换行的那一整条事件**（旧实现丢弃它）。
	text := strings.Join([]string{
		`{"code":"0000","message":"请求成功","data":{"sessionId":"S-1","outContent":"甲"}}`,
		`data: {"code":"1","data":{"flowResult":{"outContent":"乙"},"sessionId":"S-2"}}`,
		`data: [DONE]`,
		`这一段不是 JSON`,
		`data: {"data":{"outContent":"丙"}}`,
		`{"data":{"dialogueId":"D-9"}}`,
		`{"data":{"outContent":"这一条没有换行收尾，旧实现丢掉它"}}`,
	}, "\n")
	read := ParseAIWorkbenchSSE(text)
	// 4 = 七行里**以换行收尾**的那六行中：① 裸 JSON、② `data:` + flowResult、
	// ⑤ `data:` + 直挂 outContent、⑥ 裸 JSON（只有 dialogueId）各计一次；
	// ③ `[DONE]` 与 ④ 非 JSON 不计，⑦ 没有换行收尾 ⇒ 整条丢弃。
	if read.Events != 4 {
		t.Errorf("事件数实得 %d，期望 4（裸 JSON 也计、[DONE] 与非 JSON 不计、末段丢弃）", read.Events)
	}
	if read.Answer != "甲乙丙" {
		t.Errorf("累加实得 %q，期望三条都进、末条被丢", read.Answer)
	}
	if read.SessionID != "S-2" {
		t.Errorf("sessionId 实得 %q，期望后到的覆盖先到的", read.SessionID)
	}
	if read.DialogueID != "D-9" || read.LastCode != "1" || read.LastMessage != "请求成功" {
		t.Errorf("三格读数实得 %q/%q/%q", read.DialogueID, read.LastCode, read.LastMessage)
	}
	// 与 585 那一套的对比：它只认 `data:` 前缀的行、且倒序取**一条**判成败。
	other := ParsePlayAIChatSSE(text)
	if other.Events == read.Events || other.Answer == read.Answer {
		t.Errorf("两套读法读数相同（events %d/%d、answer %q/%q）⇒ 合并已经发生，本条存在的目的落空",
			other.Events, read.Events, other.Answer, read.Answer)
	}
}

// TestParseAIWorkbenchSSECopiesTheLegacyAccumulatorOnRealStreamShape 钉"正序、不跳末条"。
//
// 585 那两条实拍里 `data.outContent` 逐行皆空、末行是 `finishReason:"stop"` 的空对象 ——
// ⚠ **那是 585 的读数**，不许拿来解释本族（文件头第二节）。本族按旧实现：
// 末条若有内容**照样累加**，且不看 `finishReason`。
func TestParseAIWorkbenchSSECopiesTheLegacyAccumulatorOnRealStreamShape(t *testing.T) {
	text := "data: {\"data\":{\"flowResult\":{\"outContent\":\"第一片\"}}}\n" +
		"data: {\"data\":{\"flowResult\":{\"outContent\":\"第二片\"},\"finishReason\":\"stop\"}}\n" +
		"data: {\"data\":{\"flowResult\":{\"outContent\":\"末片也加\"}}}\n"
	read := ParseAIWorkbenchSSE(text)
	if read.Answer != "第一片第二片末片也加" {
		t.Errorf("实得 %q", read.Answer)
	}
	if read.Events != 3 {
		t.Errorf("事件数实得 %d", read.Events)
	}
}

// TestParseAIWorkbenchSSEEmptyAndNoNewlineOnlyText 钉两个"什么都没读到"的边界。
//
// 只有一行且**没有**换行收尾 ⇒ 旧实现的 buffer 永不 flush ⇒ 一次事件都不计。
// 这一格是 `eventCount === 0` 那一档最容易踩的坑：一段"看着有内容"的响应被读成零事件，
// 而旧实现同样读成零 ⇒ 跟着，不"修正"（修正就不是照抄参考实现了，§1.9）。
func TestParseAIWorkbenchSSEEmptyAndNoNewlineOnlyText(t *testing.T) {
	if read := ParseAIWorkbenchSSE(""); read.Events != 0 || read.Answer != "" {
		t.Errorf("空文本实得 %+v", read)
	}
	only := `{"data":{"outContent":"没有换行"}}`
	if read := ParseAIWorkbenchSSE(only); read.Events != 0 {
		t.Errorf("无换行收尾的一行应计 0 次，实得 %+v", read)
	}
	// 同样的内容加上换行 ⇒ 计一次。这一对比就是"丢弃末段"那条规则的断言。
	if read := ParseAIWorkbenchSSE(only + "\n"); read.Events != 1 || read.Answer != "没有换行" {
		t.Errorf("加换行后应计一次并累加，实得 %+v", read)
	}
}

// TestAppAIWorkbenchChatReadsBodyOnFailureAndRedersCredential 钉 ③ 的发送口：
// 非 2xx **保留**响应体摘录，同时把 Basic 整串洗掉。
//
// ⚠ 用例打在会 echo 的假客户端上（坑表 #168）：响应体里**故意**装着那一枚 Basic 串，
// 于是"截断摘录"与"脱敏"两件事在同一次调用里互相拉扯，这才测得到真实行为。
func TestAppAIWorkbenchChatReadsBodyOnFailureAndRedersCredential(t *testing.T) {
	auth := aiWorkbenchAuth()
	body := "上游把提交进来的东西 echo 了：" + auth.Authorization.Value() + " 以及 " + aiWorkbenchSynthPhone
	client := &sseScriptClient{status: 403, body: body, echoErr: true}
	_, perr := NewUpstream(client).AppAIWorkbenchChat(context.Background(), auth, aiWorkbenchParams())
	if perr == nil {
		t.Fatal("非 2xx 应失败")
	}
	text := perr.Error()
	if !strings.Contains(text, "HTTP 403") {
		t.Errorf("错误文本缺状态码：%s", text)
	}
	for _, forbidden := range []string{auth.Authorization.Value(), "Basic ", aiWorkbenchSynthPhone,
		aiWorkbenchSynthToken} {
		if strings.Contains(text, forbidden) {
			t.Errorf("错误文本泄了 %q：%s", forbidden, text)
		}
	}
	if len(client.requests) != 1 {
		t.Errorf("③ 应只发一发，实得 %d", len(client.requests))
	}
}

// TestAppAIWorkbenchChatSuccessPathAndTransportErrorCategory 钉两档：2xx 读 SSE、
// 传输层失败只报类别且不造摘录。
func TestAppAIWorkbenchChatSuccessPathAndTransportErrorCategory(t *testing.T) {
	auth := aiWorkbenchAuth()
	client := &sseScriptClient{status: 200,
		body: "data: {\"data\":{\"flowResult\":{\"outContent\":\"模型那一句\"}}}\n"}
	read, perr := NewUpstream(client).AppAIWorkbenchChat(context.Background(), auth, aiWorkbenchParams())
	if perr != nil {
		t.Fatalf("2xx 不该失败: %v", perr.Message())
	}
	if read.Answer != "模型那一句" || read.Events != 1 {
		t.Errorf("读数实得 %+v", read)
	}
	// 旧实现的成败判据在 runner 里（`eventCount===0 || !fullText`）⇒ 发送口**不**判成败，
	// 这里钉的就是"零事件也是 nil 错误"这一格，别让本层偷偷长出判据。
	empty := &sseScriptClient{status: 200, body: "data: [DONE]\n"}
	zero, perr := NewUpstream(empty).AppAIWorkbenchChat(context.Background(), auth, aiWorkbenchParams())
	if perr != nil {
		t.Fatalf("零事件不是错误: %v", perr.Message())
	}
	if zero.Events != 0 || zero.Answer != "" {
		t.Errorf("零事件读数实得 %+v", zero)
	}
	// 连状态码都没有 ⇒ 只报类别、不凭空造摘录。
	dropped := &sseScriptClient{status: 0, echoErr: false, sendErr: core.NewNetwork("拨不上")}
	_, perr = NewUpstream(dropped).AppAIWorkbenchChat(context.Background(), auth, aiWorkbenchParams())
	if perr == nil || !strings.Contains(perr.Error(), "拨不上") {
		t.Fatalf("传输层失败应把原话交回，实得 %v", perr)
	}
	if strings.Contains(perr.Error(), "响应摘录") {
		t.Errorf("没有响应时不许造摘录：%s", perr.Error())
	}
}
