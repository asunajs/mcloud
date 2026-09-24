package upstream

// 红包派对（云机）族 · 云机实例与安装的测试。
//
// ⚠ **这七条口全部零实拍样本**（夹具 39 条里 `cpactiv.buy.139.com` 上只有十条，
// 不含这七个路径名）。所以本文件里**没有一条**"逐字对拍夹具"的用例 ——
// 那会是一种撒谎：把测试写成"对过上游"的样子。
//
// 这里钉的是**有出处的那些约定**，出处分两档、档次不一样：
//   - **旧 TS 的字面量**（`api.ts:116-172` 的路径与 body 键名、`task.ts:193-368`
//     的响应取法）—— 一份跑过的实现，所以本文件把它的**拼法逐字钉住**：
//     改这些键要先去改实现，不是改测试；
//   - **我们自己的纪律**（读不出来交 nil 不交结论、六条口都不带幂等键、
//     发送层判信封）—— 与上游认什么无关。
//
// ⚠ 上面第一档**不等于**"上游就在这个地址"：旧 TS 是"曾经可用"的证据。
// 真跑出 404 时要改的是那七个常量，然后回这里同步 —— 而不是反过来把断言放宽。

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 路径与 body：六个零样本口共有的约定
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneInstallPathsMatchLegacyTS 七条口的 URL 逐字钉住旧 TS。
//
// 这一条**曾经**钉的是"六条各挂入口根下、没有目录段"（那时候连旧 TS 都没查）。
// 现在值取自 `api.ts:116-172`：`/thirdapi/huawei/`、`/user/`、`/redpacket/`、
// `/app/activity/` 四段前缀各归各的，**不是**一个前缀套七条。
//
// 反向改坏它：把任意一条改回裸尾段、或"顺手统一成 `redpacket/`"⇒ 该条红。
// ⚠ 这一条钉的是"与旧 TS 一致"，不是"上游认这个地址"（后者零实拍）。
func TestCloudPhoneInstallPathsMatchLegacyTS(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	for _, c := range []struct{ name, want string }{
		{"relationship", "thirdapi/huawei/thirdQueryUserRelationshipV2"},
		{"configAppList", "redpacket/configAppList"},
		{"gethwToken", "user/gethwToken"},
		{"instanceApkInstallStatus", "app/activity/instanceApkInstallStatus"},
		{"apkInstall", "app/activity/apkInstall"},
		{"appInstallTaskStatus", "app/activity/appInstallTaskStatus"},
		{"userInstallApp", "redpacket/userInstallApp"},
	} {
		var (
			request core.HTTPRequest
			perr    *core.Error
		)
		switch c.name {
		case "relationship":
			request, perr = CloudPhoneRelationshipRequest(auth, "", map[string]any{"status": "ACTIVE"})
		case "configAppList":
			request, perr = CloudPhoneConfigAppListRequest(auth, "", nil)
		case "gethwToken":
			request, perr = CloudPhoneGetHwTokenRequest(auth, "", nil)
		case "instanceApkInstallStatus":
			request, perr = CloudPhoneInstanceApkInstallStatusRequest(auth, "", nil)
		case "apkInstall":
			request, perr = CloudPhoneApkInstallRequest(auth, "", nil)
		case "appInstallTaskStatus":
			request, perr = CloudPhoneAppInstallTaskStatusRequest(auth, "", nil)
		case "userInstallApp":
			request, perr = CloudPhoneUserInstallAppRequest(auth, "", nil)
		}
		if perr != nil {
			t.Fatalf("%s 构造失败: %v", c.name, perr)
		}
		if want := CloudPhoneBaseURL + "/" + c.want; request.URL != want {
			t.Errorf("%s 的 URL 实得 %q，应为 %q（旧 TS api.ts 那一串）", c.name, request.URL, want)
		}
		if request.Method != core.MethodPost {
			t.Errorf("%s 应是 POST，实得 %s", c.name, request.Method)
		}
		if !strings.Contains(string(request.Body), `"`+CloudPhoneVersion+`"`) {
			t.Errorf("%s 的 body 应带 version，实得 %s", c.name, request.Body)
		}
	}
}

// TestCloudPhoneInstallBodiesMatchLegacyTS ⭐ 七条口的 body 逐字钉住旧 TS 的键名。
//
// 这一档为什么值得逐字：上一版把 body 整个交回调用方，结果是**全仓没有任何一处成文**
// 写着这些口要发哪些键 —— 接线的人只能自己猜一遍，而猜错的表现是上游报"参数非法"
// 或者（更糟）**照单收下**一个空 body。
//
// 反向改坏它：把 `platformType` 改成字符串、把 `apkList` 从数组改成单值、
// 或给"装成了"那次记账补上 `existed:0` ⇒ 三条各红一处。
func TestCloudPhoneInstallBodiesMatchLegacyTS(t *testing.T) {
	for _, c := range []struct{ name, want string }{
		{"relationship", `{"pageNumber":1,"pageSize":10,"status":"ACTIVE","version":"SYS_CONFIG_Y"}`},
		{"configAppList", `{"platformType":1,"version":"SYS_CONFIG_Y"}`},
		{"hwToken", `{"version":"SYS_CONFIG_Y"}`},
		{"installStatus", `{"apkList":["com.example.synthetic"],"hwToken":"SYNTH-HW-199",` +
			`"instanceIdList":["SYNTH-INST-199"],"version":"SYS_CONFIG_Y"}`},
		{"apkInstall", `{"appId":"SYNTH-PKG-199","hwToken":"SYNTH-HW-199",` +
			`"instanceId":"SYNTH-INST-199","version":"SYS_CONFIG_Y"}`},
		{"installTaskStatus", `{"hwToken":"SYNTH-HW-199","taskId":"SYNTH-TASK-199",` +
			`"version":"SYS_CONFIG_Y"}`},
		// 记账：existed **不给就不发这个键**（旧 TS 用 isNil 判，两支是两种话）。
		{"report", `{"appId":"7","platformType":1,"taskId":"9","version":"SYS_CONFIG_Y"}`},
		{"reportExisted", `{"appId":"7","existed":1,"platformType":1,"taskId":"9",` +
			`"version":"SYS_CONFIG_Y"}`},
	} {
		var (
			body map[string]any
			got  string
		)
		switch c.name {
		case "relationship":
			body = CloudPhoneRelationshipBody(CloudPhoneRelationshipActive, 0, 0)
		case "configAppList":
			body = CloudPhoneConfigAppListBody()
		case "hwToken":
			body = CloudPhoneEmptyBody()
		case "installStatus":
			body = CloudPhoneInstallStatusBody("SYNTH-HW-199", "SYNTH-INST-199", "com.example.synthetic")
		case "apkInstall":
			body = CloudPhoneApkInstallBody("SYNTH-INST-199", "SYNTH-PKG-199", "SYNTH-HW-199")
		case "installTaskStatus":
			body = CloudPhoneInstallTaskStatusBody("SYNTH-HW-199", "SYNTH-TASK-199")
		case "report":
			body = CloudPhoneInstallReportBody("9", "7", nil)
		case "reportExisted":
			existed := int64(1)
			body = CloudPhoneInstallReportBody("9", "7", &existed)
		}
		request, perr := CloudPhoneRequest("probe", body, cloudPhoneSyntheticAuth(), "")
		if perr != nil {
			t.Fatalf("%s 构造失败: %v", c.name, perr)
		}
		got = string(request.Body)
		if got != c.want {
			t.Errorf("%s 的 body 实得 %s，应为 %s", c.name, got, c.want)
		}
	}
}

// TestCloudPhoneInstallRequestsCarryCallerKeys ⭐ 传进去的键一定进 body。
//
// 这是本文件最要紧的一条：六条口一个键名都不知道，所以**任何**一次静默丢参数
// 都表现为"上游按没传来处理、我们看见的是请求成功了"。
// 参考实现那个 `taskID 从未进请求体` 的 bug（表 T-4）就是这个形状。
func TestCloudPhoneInstallRequestsCarryCallerKeys(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	data := map[string]any{"instanceId": "199", "packageName": "com.example.synthetic"}
	for name, build := range map[string]func(map[string]any) (core.HTTPRequest, *core.Error){
		"relationship": func(d map[string]any) (core.HTTPRequest, *core.Error) {
			return CloudPhoneRelationshipRequest(auth, "", d)
		},
		"configAppList": func(d map[string]any) (core.HTTPRequest, *core.Error) {
			return CloudPhoneConfigAppListRequest(auth, "", d)
		},
		"gethwToken": func(d map[string]any) (core.HTTPRequest, *core.Error) {
			return CloudPhoneGetHwTokenRequest(auth, "", d)
		},
		"instanceApkInstallStatus": func(d map[string]any) (core.HTTPRequest, *core.Error) {
			return CloudPhoneInstanceApkInstallStatusRequest(auth, "", d)
		},
		"apkInstall": func(d map[string]any) (core.HTTPRequest, *core.Error) {
			return CloudPhoneApkInstallRequest(auth, "", d)
		},
		"appInstallTaskStatus": func(d map[string]any) (core.HTTPRequest, *core.Error) {
			return CloudPhoneAppInstallTaskStatusRequest(auth, "", d)
		},
	} {
		request, perr := build(data)
		if perr != nil {
			t.Fatalf("%s 构造失败: %v", name, perr)
		}
		body := string(request.Body)
		for _, key := range []string{"instanceId", "packageName"} {
			if !strings.Contains(body, `"`+key+`"`) {
				t.Errorf("%s 的 body 里丢了 %s：%s", name, key, body)
			}
		}
		// ⚠ 六条都**不带**幂等键：本层不假装上游能去重，
		// 也就没人能据此对 `apkInstall` 做自动重试。
		if request.IdempotencyKey != nil {
			t.Errorf("%s 不该被本层安上幂等键（上游不认它，重试等于再装一次）", name)
		}
	}
	// 传进来的 map 不被写脏。
	if _, perr := CloudPhoneApkInstallRequest(auth, "", data); perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if _, dirty := data["version"]; dirty {
		t.Error("调用方的 map 被写脏了")
	}
}

// TestCloudPhoneInstallRequestsDoNotGuessKeys ⭐ 反向的那一半：nil data 时
// body 里**只有** version —— 本层不替调用方补任何键名。
//
// 反向改坏它：给 `apkInstall` 猜一个 `apkUrl` / `packageName` ⇒ 这一条红
// （多一个键的表现是上游可能报"参数非法"，也可能**照单收下**一个我们凭空写的值）。
func TestCloudPhoneInstallRequestsDoNotGuessKeys(t *testing.T) {
	request, perr := CloudPhoneApkInstallRequest(cloudPhoneSyntheticAuth(), "", nil)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"version":"SYS_CONFIG_Y"}`; string(request.Body) != want {
		t.Errorf("nil data 时应逐字只剩 version（与第 32/36/37 条那一形状相同），实得 %s", request.Body)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 实例列表：双层嵌套与"读不出来"
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudPhoneInstanceListLayers ⭐ 取层的写法逐字钉住旧 TS（`task.ts:193-195`）。
//
//	nested = (data.data 是对象) ? data.data : data
//	list   = nested.subscribeDetails ?? []
//
// `Nested` 这一位的全部意义：外层取与内层取**两种都没实拍**，
// 而走错层的表现是"列表空了"——与"真的没有实例"在只交列表的版本里长得一模一样。
//
// 反向改坏它：把三元倒过来（先看浅层）⇒ 第三条红；把 `Nested` 写死 false ⇒ 第二条红；
// 把键名换回 `list`（上一版凭空造的那一个）⇒ 全部红。
func TestParseCloudPhoneInstanceListLayers(t *testing.T) {
	// 浅层：`data.subscribeDetails`。
	shallow, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"subscribeDetails":[`+
			`{"subsProdInstances":[{"lockStatus":"Y"}]},`+
			`{"subsProdInstances":[{"lockStatus":"N"}]}]}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if shallow.Nested {
		t.Error("从浅层取到的不该标 Nested")
	}
	if len(shallow.Instances) != 2 {
		t.Fatalf("应两项，实得 %d", len(shallow.Instances))
	}
	if got := shallow.Instances[0]; got.LockStatus == nil || *got.LockStatus != CloudPhoneLockStatusLocked {
		t.Errorf("lockStatus 实得 %v", got.LockStatus)
	}
	// 深层：`data.data.subscribeDetails`（旧 TS 优先走的那一层）。
	deep, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"data":{"subscribeDetails":[`+
			`{"instanceId":"SYNTH-INST-199","subsProdInstances":[{"lockStatus":"Y"}]}]}}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if !deep.Nested {
		t.Error("从深层取到的必须标 Nested（否则接线时看不见走了哪条路）")
	}
	if len(deep.Instances) != 1 {
		t.Fatalf("实得 %d 项", len(deep.Instances))
	}
	if id := deep.Instances[0].InstanceID; id == nil || *id != "SYNTH-INST-199" {
		t.Errorf("instanceId 实得 %v", id)
	}
	// 两层都给 ⇒ **取深层**（旧 TS 那个三元就是这意思：`data.data` 是对象时浅层被整格跳过）。
	both, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{`+
			`"subscribeDetails":[{"instanceId":"SYNTH-SHALLOW"}],`+
			`"data":{"subscribeDetails":[{"instanceId":"SYNTH-DEEP"}]}}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if !both.Nested || len(both.Instances) != 1 ||
		*both.Instances[0].InstanceID != "SYNTH-DEEP" {
		t.Errorf("应取深层那一份，实得 %#v", both)
	}
	// 哪种都没有 ⇒ 空列表、不报错。
	for name, body := range map[string]string{
		"data 缺失":           `{"header":{"status":"200"}}`,
		"data 是对象但没列表":      `{"header":{"status":"200"},"data":{"anything":1}}`,
		"data.data 是对象但没列表": `{"header":{"status":"200"},"data":{"data":{"a":1}}}`,
		// ⚠ 这一档是**已知读不到**的那一支：旧 TS 的 `Array.isArray(data.data)`
		// 兼容的是"data.data 直接是列表"，而本层信封取法只认对象 ⇒ 落到浅层、读成空。
		"data.data 是数组（本层读不到）": `{"header":{"status":"200"},"data":{"data":[{"instanceId":"SYNTH-199"}]}}`,
	} {
		out, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：只读口不该报错: %v", name, perr)
			continue
		}
		if out.Instances != nil {
			t.Errorf("%s：应交 nil，实得 %d 项", name, len(out.Instances))
		}
	}
	// 信封不过 ⇒ 必须报错。
	if _, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"云机查不到"},"data":{}}`)); perr == nil {
		t.Error("status 非 200 应报错")
	}
}

// TestCloudPhoneInstanceLockStatusIsNested ⭐ 锁屏位在 `subsProdInstances[0]` 里，
// **不在条目顶层**。
//
// 这一条是本轮改正的核心：上一版从顶层读 `lockStatus`，而旧 TS 读的是
// `item.subsProdInstances[0].lockStatus`（`task.ts:274-283`）。顶层那一格**不存在**
// ⇒ 永远读到 nil ⇒ 下一层按旧实现的脾气把 nil 当"没锁屏"，于是
// **往一台锁屏的云机上装 APK**。
//
// 反向改坏它：把 `cloudPhoneFirstInstanceLock` 改回读顶层 ⇒ 前两档红。
func TestCloudPhoneInstanceLockStatusIsNested(t *testing.T) {
	out, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"subscribeDetails":[`+
			// ① 正常：顶层有一个**同名干扰键**，锁屏位仍须从子数组读
			`{"instanceId":"SYNTH-A","lockStatus":"N","subsProdInstances":[{"lockStatus":"Y"}]},`+
			// ② 子数组为空 ⇒ nil（旧实现把 nil 当"可装"，那是它的判断）
			`{"instanceId":"SYNTH-B","subsProdInstances":[]},`+
			// ③ 根本没有子数组 ⇒ nil
			`{"instanceId":"SYNTH-C"},`+
			// ④ 子数组首项不是对象 ⇒ nil
			`{"instanceId":"SYNTH-D","subsProdInstances":["不是对象"]}]}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(out.Instances) != 4 {
		t.Fatalf("应四项，实得 %d", len(out.Instances))
	}
	first := out.Instances[0]
	if first.LockStatus == nil || *first.LockStatus != "Y" {
		t.Errorf("①：锁屏位应从 subsProdInstances[0] 读，实得 %v（读到顶层那个 N 就是本轮那个 bug）",
			first.LockStatus)
	}
	if first.InstanceID == nil || *first.InstanceID != "SYNTH-A" {
		t.Errorf("①：instanceId 实得 %v", first.InstanceID)
	}
	for i := 1; i < 4; i++ {
		if out.Instances[i].LockStatus != nil {
			t.Errorf("②③④ 第 %d 项：读不到应是 nil，实得 %q", i+1, *out.Instances[i].LockStatus)
		}
	}
}

// TestCloudPhoneInstanceKeepsUnknownKeysAsRaw ⭐ 一项里的其余键原样交出、
// 而读不出来的格子是 nil **不是** 兜底值。
//
// "没锁屏"是一条**结论**：读成 nil→"N" 就会让下一层理直气壮地往那台机上装东西。
//
// 反向改坏它：给 LockStatus 补一个缺省 "N" ⇒ 第一档红。
func TestCloudPhoneInstanceKeepsUnknownKeysAsRaw(t *testing.T) {
	out, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"subscribeDetails":[`+
			`{"instanceName":"SYNTH-199","subsProdInstances":[{"lockStatus":"Y"}]},`+
			`{"instanceName":"SYNTH-199-x"}]}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	withLock, noLock := out.Instances[0], out.Instances[1]
	if withLock.Raw == nil || withLock.Raw["instanceName"] != "SYNTH-199" {
		t.Errorf("其余键该原样交出，实得 %#v", withLock.Raw)
	}
	if noLock.LockStatus != nil {
		t.Errorf("没给锁屏位应是 nil，实得 %q（nil 与 \"N\" 是两件事）", *noLock.LockStatus)
	}
	if noLock.Raw == nil {
		t.Error("对象项即使只读出一格，Raw 也该交出原文")
	}
	// ⚠ 非对象的一项：整项读成"什么都不知道"（三格全 nil、Raw nil）而**不报错**
	// —— 与题目列表那一条的处置**刻意不同**，理由见两处注释。
	broken, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"subscribeDetails":["不是对象"]}}`))
	if perr != nil {
		t.Fatalf("只读口不该报错: %v", perr)
	}
	if len(broken.Instances) != 1 || broken.Instances[0].LockStatus != nil ||
		broken.Instances[0].InstanceID != nil || broken.Instances[0].Raw != nil {
		t.Errorf("非对象项应读成一格都不知道的空实例，实得 %#v", broken.Instances)
	}
}

// TestCloudPhoneRelationshipStatusesAreUpstreamLiterals `ACTIVE` / `PENDING`
// 与 `lockStatus` 的 `"Y"` 都是**上游字面量**（成文逐字给过），不许顺手改成小写或枚举名。
//
// ⚠ 同时钉住"这两枚不是全集"这件事：本层没有任何一处对第三个值做判断。
func TestCloudPhoneRelationshipStatusesAreUpstreamLiterals(t *testing.T) {
	if CloudPhoneRelationshipActive != "ACTIVE" || CloudPhoneRelationshipPending != "PENDING" {
		t.Errorf("两枚关系状态应逐字照抄，实得 %q / %q",
			CloudPhoneRelationshipActive, CloudPhoneRelationshipPending)
	}
	if CloudPhoneLockStatusLocked != "Y" {
		t.Errorf("锁屏位应逐字是 Y，实得 %q", CloudPhoneLockStatusLocked)
	}
}

// TestParseCloudPhoneDataRefusesToAssumeSuccess ⭐ 零样本载荷那类口的统一读法：
// 只判信封、只交原文，**不**声明任何字段。
//
// 反向改坏它：在这里加一个 `Installed *bool` 之类的"方便字段"⇒
// 它永远是 nil，而 nil 会被读成"没装"⇒ 于是真的再装一次。这条测试盯的就是
// "载荷类型今天只有一格：原文 map"。
func TestParseCloudPhoneDataRefusesToAssumeSuccess(t *testing.T) {
	data, perr := ParseCloudPhoneData(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"anything":"SYNTH-199"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if data["anything"] != "SYNTH-199" {
		t.Errorf("原文应交出，实得 %#v", data)
	}
	// data 缺失 ⇒ nil、不报错。
	missing, perr := ParseCloudPhoneData(cloudPhoneEnvelope(t, `{"header":{"status":"200"}}`))
	if perr != nil || missing != nil {
		t.Errorf("应是 nil/不报错，实得 %#v / %v", missing, perr)
	}
	// 信封不过 ⇒ 报错，两种失败分得开。
	for name, body := range map[string]string{
		"status 非 200": `{"header":{"status":"500","errMsg":"装机通道忙"},"data":{"a":1}}`,
		"没有 header":    `{"data":{"a":1}}`,
		"status 是数字":   `{"header":{"status":200},"data":{"a":1}}`,
	} {
		if _, perr := ParseCloudPhoneData(cloudPhoneEnvelope(t, body)); perr == nil {
			t.Errorf("%s：应报错", name)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// TestAppCloudPhoneInstancesRoundTrip 列实例这一条走完整"发 + 解"。
//
// 载荷用**深层**那一形状（`data.data.subscribeDetails`）：它同时过到取层的两条分支
// （是对象 ⇒ 进深层）与 `Nested=true` 那一位，而 body 走 [CloudPhoneRelationshipBody]
// —— 这一条因此是"键名 → 发送 → 读数"整链唯一一条贯通的。
func TestAppCloudPhoneInstancesRoundTrip(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"appId":"12345681","errMsg":"OK","status":"200"},`+
			`"data":{"data":{"subscribeDetails":[`+
			`{"instanceId":"SYNTH-INST-199","subsProdInstances":[{"lockStatus":"Y"}]},`+
			`{"instanceId":"SYNTH-INST-198","subsProdInstances":[{"lockStatus":"N"}]}`+
			`]}}}`))}
	auth := cloudPhoneSyntheticAuth()
	got, perr := NewUpstream(client).AppCloudPhoneInstances(nil, auth, "",
		CloudPhoneRelationshipBody(CloudPhoneRelationshipActive, 1, 10))
	if perr != nil {
		t.Fatalf("调用失败: %v", perr)
	}
	if !got.Nested || len(got.Instances) != 2 {
		t.Fatalf("实得 %#v", got)
	}
	if id := got.Instances[1].InstanceID; id == nil || *id != "SYNTH-INST-198" {
		t.Errorf("第二台的 instanceId 实得 %v", id)
	}
	if len(client.requests) != 1 {
		t.Fatalf("一次调用一条请求（两查聚合归编排层），实得 %d 条", len(client.requests))
	}
	body := string(client.requests[0].Body)
	for _, want := range []string{`"status":"` + CloudPhoneRelationshipActive + `"`, `"pageNumber":1`} {
		if !strings.Contains(body, want) {
			t.Errorf("body 里应有 %s，实得 %s", want, body)
		}
	}
}

// TestAppCloudPhoneApkInstallJudgesTheEnvelope ⭐ 副作用最大那一条**必须**判信封。
func TestAppCloudPhoneApkInstallJudgesTheEnvelope(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	failing := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"云机忙"},"data":{"errorMsg":"当前无空闲实例"}}`))}
	if perr := NewUpstream(failing).AppCloudPhoneApkInstall(nil, auth, "",
		map[string]any{"someId": "199"}); perr == nil {
		t.Error("HTTP 200 而 status 非 200 时必须报错 —— 这一条改变的是用户那台云机")
	}
	if n := len(failing.requests); n != 1 {
		t.Errorf("应恰好一条，实得 %d", n)
	}
	passing := &scriptedClient{response: core.NewResponse(200,
		[]byte(`{"header":{"status":"200"},"data":{}}`))}
	if perr := NewUpstream(passing).AppCloudPhoneApkInstall(nil, auth, "", nil); perr != nil {
		t.Errorf("成功信封不该报错: %v", perr)
	}
	// ⚠ 但**别把"没报错"读成"装好了"**：第一手读数在轮询那一条上。
}

// TestAppCloudPhoneInstallReadersJudgeTheEnvelope 四条只读装机口的统一契约。
func TestAppCloudPhoneInstallReadersJudgeTheEnvelope(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	bad := core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"活动结束了"},"data":{"token":"SYNTH-LEAK-CHECK-199"}}`))
	for name, call := range map[string]func(client *scriptedClient) *core.Error{
		"configAppList": func(client *scriptedClient) *core.Error {
			_, perr := NewUpstream(client).AppCloudPhoneConfigAppList(nil, auth, "", nil)
			return perr
		},
		"gethwToken": func(client *scriptedClient) *core.Error {
			_, perr := NewUpstream(client).AppCloudPhoneGetHwToken(nil, auth, "", nil)
			return perr
		},
		"instanceApkInstallStatus": func(client *scriptedClient) *core.Error {
			_, perr := NewUpstream(client).AppCloudPhoneInstanceApkInstallStatus(nil, auth, "", nil)
			return perr
		},
		"appInstallTaskStatus": func(client *scriptedClient) *core.Error {
			_, perr := NewUpstream(client).AppCloudPhoneAppInstallTaskStatus(nil, auth, "", nil)
			return perr
		},
		"instances": func(client *scriptedClient) *core.Error {
			_, perr := NewUpstream(client).AppCloudPhoneInstances(nil, auth, "", nil)
			return perr
		},
	} {
		client := &scriptedClient{response: bad}
		perr := call(client)
		if perr == nil {
			t.Errorf("%s：status 非 200 必须报错", name)
			continue
		}
		// ⚠ 载荷里那一位**长得像凭据**：错误文案不许把它带出去
		//（[EnsureCloudPhoneOK] 只取那三处 msg 位，不取整包原文）。
		if strings.Contains(perr.Message(), "SYNTH-LEAK-CHECK") {
			t.Errorf("%s：错误文案里出现了上游载荷原文：%q", name, perr.Message())
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 补：注释里承诺过而上面没钉到的三档 + 拼不出 body 时不留请求 + 凭据不外泄
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudPhoneInstanceListDeepEmptyLayerStillReportsNested ⭐ `Nested`
// 这一位**只在空列表上才有意义**（函数注释原话：走错层的表现是"列表空了"）。
//
// 上面那条只钉到"深层给了两项 ⇒ nested=true"；这一档钉"深层给了**空数组**"：
//   - `data.subscribeDetails: []` ⇒ nil + nested=false（浅层确实是空的）；
//   - `data.data.subscribeDetails: []` ⇒ nil + **true**（列表在深层，只是这次没内容）。
//
// 两者都交 nil 列表、都不报错 —— 能把"没实例"与"取错了层"分开的只有这一位。
// 反向改坏它：把"深层是对象"那一档的命中条件从 `ok` 改成 `len(...) > 0`
// （"空列表就不算命中这一层"）⇒ 本条红。
func TestParseCloudPhoneInstanceListDeepEmptyLayerStillReportsNested(t *testing.T) {
	deep, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"data":{"subscribeDetails":[]}}}`))
	if perr != nil {
		t.Fatalf("空数组不该报错: %v", perr)
	}
	if deep.Instances != nil {
		t.Errorf("空列表该交 nil，实得 %d 项", len(deep.Instances))
	}
	if !deep.Nested {
		t.Error("深层取到的空列表必须标 Nested=true")
	}
	shallow, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"subscribeDetails":[]}}`))
	if perr != nil || shallow.Instances != nil || shallow.Nested {
		t.Errorf("浅层空数组应是 nil/false，实得 %#v / %v", shallow, perr)
	}
}

// TestParseCloudPhoneInstanceListDataThatIsAnArrayIsEmptyNotError 兑现函数注释里
// 那条**明写了两个方向**的承诺："`data` 本身直接是数组"这一形状过得了信封判据
// （它只看 `header.status`），但会被取段那一步压成 nil ⇒ 交**空列表**，**不是**报错。
//
// 为什么值得单独钉：注释同时写了"要改的是取段那一层（[EnsureCloudPhoneOK] 的返回类型），
// **不是**在这里补一个 map 兜底"。钉住"今天交空而不报错"，才能让下一次改动
// 要么保留它、要么连取段那层一起改，而不是在这里偷偷叠一层兜底。
//
// 反向改坏它：在 cloudPhoneInstanceArray 之前补一句"`data` 是数组就当列表"⇒ 本条红。
func TestParseCloudPhoneInstanceListDataThatIsAnArrayIsEmptyNotError(t *testing.T) {
	out, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":[{"lockStatus":"Y"},{"lockStatus":"N"}]}`))
	if perr != nil {
		t.Fatalf("注释承诺这一形状不报错（只读口），实得 %v", perr)
	}
	if out.Instances != nil || out.Nested {
		t.Errorf("应是空列表 + 未嵌套，实得 %#v", out)
	}
	// 同一形状交给载荷口：`data` 不是对象 ⇒ 同样只交 nil、不报错。
	data, perr := ParseCloudPhoneData(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":[{"anything":1}]}`))
	if perr != nil || data != nil {
		t.Errorf("实得 %#v / %v（data 不是对象时不报错，交 nil）", data, perr)
	}
}

// TestParseCloudPhoneInstallRejectsForeignEnvelopes ⭐ **两套信封不通用**的安装侧。
//
// [ParseCloudPhoneData] 与 [ParseCloudPhoneInstanceList] 只认本族那份
// `{version,header:{status,errMsg,respMsg},data}`；同一份夹具里另两族的判据是 `code`
// （`ycloud/newyear` 是**数字** 0、`ai` 是**字符串** `"0000"` + `success`），
// 而通用判据 [Envelope.EnsureOK] 对"没有 code"是**通过**的 ⇒ 混解是一条静默通路。
//
// 这一档在 `gethwToken` 上最要紧：那一口的返回**必然含凭据**，把别族的响应当成它，
// 下一层拿到的就是一个"长得像 token 的字符串"，而它来自另一个域。
//
// 反向改坏它：把 [EnsureCloudPhoneOK] 换成 `envelope.EnsureOK()` ⇒ 前两档红。
func TestParseCloudPhoneInstallRejectsForeignEnvelopes(t *testing.T) {
	for name, foreign := range map[string]string{
		"newyear 那族": `{"code":0,"msg":"success","result":{"canAmount":"210"}}`,
		"ai 那族":      `{"code":"0000","message":"请求成功","success":true,"data":{"token":"SYNTH-LOOKS-LIKE-TOKEN"}}`,
	} {
		envelope := cloudPhoneEnvelope(t, foreign)
		if _, perr := ParseCloudPhoneData(envelope); perr == nil {
			t.Errorf("%s：不是本族的信封，绝不能判成成功", name)
		} else if perr.Kind() != core.KindParse {
			t.Errorf("%s：信封不认识应归 Parse，实得 %v", name, perr.Kind())
		}
		if _, perr := ParseCloudPhoneInstanceList(envelope); perr == nil {
			t.Errorf("%s：实例列表口同样要报错", name)
		}
	}
	// 反方向：本族那份交给别族的解析器（差的就是判据字段那一位）。
	mine := cloudPhoneEnvelope(t, `{"header":{"status":"200"},"data":{"token":"SYNTH-199"}}`)
	if _, perr := PlayAIParseRecognize(mine); perr == nil {
		t.Error("本族信封喂给 ai 那族也该报错（那一族找的是 code/success）")
	}
	if _, perr := PlayAIParseUserStatus(mine); perr == nil {
		t.Error("本族信封喂给 newyear 那族同样该报错")
	}
}

// TestCloudPhoneInstallRequestsFailWithoutLeavingABody ⭐ 拼不出 body 时不许留下半条请求。
//
// 与答题那条**不同**：这一族的构造是先 `core.NewRequestPost(URL)` 再序列化，
// 所以报错时 URL **已经填上了**（`core.HTTPRequest` 是值语义，错误分支原样交回）。
// 真正钉得住的是两件事：① 交回来的那条**没有 body**；② 发送层拿到错误**一条都不发**。
//
// 反向改坏它：把 [CloudPhoneRawRequest] 的错误分支改成"序列化失败就当没 body 继续发"
// ⇒ 第二条红，而它的后果是真的往那台云机上打了一枪装机请求。
func TestCloudPhoneInstallRequestsFailWithoutLeavingABody(t *testing.T) {
	unencodable := map[string]any{"apkUrl": make(chan int)}
	request, perr := CloudPhoneApkInstallRequest(cloudPhoneSyntheticAuth(), "", unencodable)
	if perr == nil {
		t.Fatal("body 拼不出来应报错")
	}
	if len(request.Body) != 0 {
		t.Errorf("报错时不许留下一条带 body 的请求，实得 %s", request.Body)
	}
	client := &scriptedClient{response: core.NewResponse(200,
		[]byte(`{"header":{"status":"200"},"data":{}}`))}
	if perr := NewUpstream(client).AppCloudPhoneApkInstall(nil, cloudPhoneSyntheticAuth(), "",
		unencodable); perr == nil {
		t.Error("发送层应把构造错误原样交回")
	}
	if n := len(client.requests); n != 0 {
		t.Errorf("body 拼不出来时一条都不该发，实得 %d 条", n)
	}
}

// TestAppCloudPhoneInstallKeepsTheTokenOutOfErrorsAndURLs ⭐ 凭据只进请求头。
//
// `CloudPhoneAuth.Token` 是活动站凭据（本族口口都带它，它还是 `sign` 算式的一段）；
// `gethwToken` 的返回**必然也是凭据**。钉三条：
//   - 错误文案里不许出现它，也不许出现它的派生值 `sign`（`Message` 会进日志）；
//   - URL 里不许出现它（这一族没有查询串，最容易被"顺手把 token 拼进 URL 方便排查"破坏）；
//   - 但它**必须**待在 `token` 头里（否则整族鉴权与签名都白算）。
//
// 反向改坏它：把 URL 改成 `…?token=` + auth.Token ⇒ 第二条红。
func TestAppCloudPhoneInstallKeepsTheTokenOutOfErrorsAndURLs(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"鉴权过期"},"data":{"errorMsg":"请重新登录"}}`))}
	_, perr := NewUpstream(client).AppCloudPhoneGetHwToken(nil, auth, "", nil)
	if perr == nil {
		t.Fatal("status 非 200 应报错")
	}
	if strings.Contains(perr.Message(), auth.Token) || strings.Contains(perr.Message(), auth.Sign()) {
		t.Errorf("错误文案里出现了凭据或它的派生值：%q", perr.Message())
	}
	sent := client.requests[0]
	if strings.Contains(sent.URL, auth.Token) {
		t.Errorf("URL 里不该出现 token：%q", sent.URL)
	}
	if got := headerValue(sent.Headers, "token"); got != auth.Token {
		t.Errorf("token 只该待在请求头里，实得 %q", got)
	}
	if got := headerValue(sent.Headers, "sign"); got == auth.Token || got == "" {
		t.Errorf("sign 不该退化成 token 本身，实得 %q", got)
	}
}

// TestParseCloudPhoneInstallRefusesHeaderWithoutStatus ⭐ "有 header、但**压根没给 status**"
// 这一支在装机/实例这两口上同样不许当成功。
//
// 为什么补这一档：这一族**没有 `code` 那一位**，通用信封的宽松判据找的正是 `code`
// （找不到就算过）⇒ 少一格守卫，"云机接口没权限"就会被读成"实例列表为空"，
// 而装机流程看到空列表的反应是"没有可装机的实例"，不是"登录过期了"。
// 上面几条钉的是"status 非 200"与"别族的信封"，都没走到"这一位不存在"。
//
// 反向改坏它：把 [EnsureCloudPhoneOK] 里 `if status == nil` 那一支改成"交 data、不报错"
// ⇒ 本条三条断言全红。
func TestParseCloudPhoneInstallRefusesHeaderWithoutStatus(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	for name, body := range map[string]string{
		"header 在但没 status": `{"header":{"appId":"12345681","errMsg":"OK"},"data":{"list":[{"lockStatus":"Y"}]}}`,
		"status 是数字":        `{"header":{"status":200},"data":{"list":[{"lockStatus":"Y"}]}}`,
		"status 是 null":     `{"header":{"status":null},"data":{"data":[]}}`,
		"整个 header 没有":      `{"data":{"list":[{"lockStatus":"Y"}]}}`,
	} {
		envelope := cloudPhoneEnvelope(t, body)
		if data, perr := ParseCloudPhoneData(envelope); perr == nil {
			t.Errorf("%s：载荷口不能判成成功，实得 %#v", name, data)
		} else if perr.Kind() != core.KindParse {
			t.Errorf("%s：信封不认识应归 Parse，实得 %v", name, perr.Kind())
		}
		list, perr := ParseCloudPhoneInstanceList(envelope)
		if perr == nil {
			t.Errorf("%s：实例口不能判成成功，实得 %d 项 nested=%v", name, len(list.Instances), list.Nested)
			continue
		}
		// 报错时不许留下一副"看起来读好了"的列表 —— 那正是把失败伪装成空列表的形状。
		if list.Instances != nil || list.Nested {
			t.Errorf("%s：报错时该交零值，实得 %#v", name, list)
		}
	}
	// 发送层：装机那一族的每一条都走同一个解析器，钉一口真调用即可防"改一处绕过"。
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"appId":"12345681","errMsg":"OK"},"data":{"data":[{"lockStatus":"Y"}]}}`))}
	got, perr := NewUpstream(client).AppCloudPhoneInstances(nil, auth, "", nil)
	if perr == nil {
		t.Fatalf("发送层把没给 status 读成了成功，实得 %d 项", len(got.Instances))
	}
	if perr.Kind() != core.KindParse {
		t.Errorf("实得 Kind=%v", perr.Kind())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 安装链的响应读法（2026-09-24：按旧 TS `task.ts:288-368` 那几条取值链补齐）
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudPhoneInstanceListIgnoresLegacyGuessedKeys ⭐ 反向的那一半：
// `list` / `records` / `instances` 三个**上一版凭空造的键**现在必须**不读**。
//
// 为什么值得钉：旧 TS 里 `data.list` 是**另一张口**（`configAppList`）的键。
// 留着这一档当兜底的表现是"把可装应用清单当成云机列表"，
// 而下一层拿着它去装机 —— 那不是读不到，是**读错了还很有把握**。
func TestParseCloudPhoneInstanceListIgnoresLegacyGuessedKeys(t *testing.T) {
	for name, body := range map[string]string{
		"list":      `{"header":{"status":"200"},"data":{"list":[{"instanceId":"SYNTH-APP-199"}]}}`,
		"records":   `{"header":{"status":"200"},"data":{"records":[{"instanceId":"SYNTH-APP-199"}]}}`,
		"instances": `{"header":{"status":"200"},"data":{"instances":[{"instanceId":"SYNTH-APP-199"}]}}`,
	} {
		out, perr := ParseCloudPhoneInstanceList(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Fatalf("%s：只读口不该报错: %v", name, perr)
		}
		if out.Instances != nil {
			t.Errorf("%s：这一族不认的键不许读成实例，实得 %d 项 %#v", name, len(out.Instances), out.Instances)
		}
	}
}

// TestParseCloudPhoneHwTokenReadsTheLegacyChain `data.hwToken`（`task.ts:296`）。
//
// ⚠ 这一位是**凭据**：本条断言只比"读没读到"，绝不把值写进错误文案；
// 顺带钉"信封不过时报错里也不带载荷"。
func TestParseCloudPhoneHwTokenReadsTheLegacyChain(t *testing.T) {
	got, perr := ParseCloudPhoneHwToken(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"hwToken":"SYNTH-HW-199","extra":"x"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got == nil || *got != "SYNTH-HW-199" {
		t.Errorf("实得 %v", got)
	}
	// 缺键 ⇒ nil（旧实现在此放弃这件任务，不兜底成空串：空串发出去是一条真请求）。
	missing, perr := ParseCloudPhoneHwToken(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"anything":1}}`))
	if perr != nil || missing != nil {
		t.Errorf("缺键应是 nil/不报错，实得 %#v / %v", missing, perr)
	}
	// 数字不算字符串（与 [cloudPhoneString] 同一条纪律）。
	number, perr := ParseCloudPhoneHwToken(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"hwToken":199}}`))
	if perr != nil || number != nil {
		t.Errorf("数字型 hwToken 应交 nil（这一档零样本，不猜），实得 %#v / %v", number, perr)
	}
	bad, perr := ParseCloudPhoneHwToken(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"通道忙"},"data":{"hwToken":"SYNTH-LEAK-199"}}`))
	if perr == nil {
		t.Fatal("status 非 200 必须报错")
	}
	if bad != nil || strings.Contains(perr.Message(), "SYNTH-LEAK") {
		t.Errorf("报错时应交 nil 且不带载荷原文，实得 %#v / %q", bad, perr.Message())
	}
}

// TestParseCloudPhoneAppListKeepsIDAndPackageApart ⭐ 一条里最容易用反的两格。
//
// 旧 TS 装 APK 用 `app.packageId`（`task.ts:322` 还 `String()` 了一道），
// 记账用 `app.id`（`:315`、`:359`）⇒ **两格不能互换**。
// 本条给两格填**不同的值**，互换即红。
//
// 其余约定：`data.list` 缺失或空 ⇒ nil 不报错；非对象项读成"一格都不知道"；
// `id`/`packageId` 原样交 `any`（不做数字↔字符串转换 —— 旧 TS 自己都没定）。
func TestParseCloudPhoneAppListKeepsIDAndPackageApart(t *testing.T) {
	apps, perr := ParseCloudPhoneAppList(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"list":[`+
			`{"id":7,"packageId":"SYNTH-PKG-199","packageName":"com.example.synthetic","apkName":"合成应用"},`+
			`{"packageName":""},`+
			`"不是对象"]}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(apps) != 3 {
		t.Fatalf("应三项（非对象项也占一格，不许静默丢），实得 %d", len(apps))
	}
	first := apps[0]
	if first.PackageName == nil || *first.PackageName != "com.example.synthetic" {
		t.Errorf("packageName 实得 %v", first.PackageName)
	}
	if first.ApkName == nil || *first.ApkName != "合成应用" {
		t.Errorf("apkName 实得 %v", first.ApkName)
	}
	// 两格各归各：值不同，且类型按原样交出去（数字仍是数字、字符串仍是字符串）。
	// ⚠ 形态：本仓信封走 `UseNumber`（[ParseEnvelope]）⇒ 数字到这里是 `json.Number`，
	// 不是 `float64`。这一格刻意不做任何转换（连"顺手化成字符串"都没有）：
	// 旧 TS 在它上面写的是 `as number`，而 19 位 id 过一遍 double 会丢末几位。
	if id, ok := first.ID.(json.Number); !ok || id.String() != "7" {
		t.Errorf("`id` 应原样交数字（json.Number），实得 %#v（%T）", first.ID, first.ID)
	}
	if pkg, ok := first.PackageID.(string); !ok || pkg != "SYNTH-PKG-199" {
		t.Errorf("`packageId` 应原样交字符串，实得 %#v（%T）", first.PackageID, first.PackageID)
	}
	// 空字符串是**读数**，不是缺失。
	if apps[1].PackageName == nil || *apps[1].PackageName != "" {
		t.Errorf("给了空串就该读到空串，实得 %#v", apps[1].PackageName)
	}
	if apps[2].PackageName != nil || apps[2].Raw != nil || apps[2].ID != nil {
		t.Errorf("非对象项应读成一格都不知道，实得 %#v", apps[2])
	}
	// 缺失/空数组 ⇒ nil、不报错。
	for name, body := range map[string]string{
		"没有 list":  `{"header":{"status":"200"},"data":{"anything":1}}`,
		"list 是空":  `{"header":{"status":"200"},"data":{"list":[]}}`,
		"list 不是组": `{"header":{"status":"200"},"data":{"list":{"a":1}}}`,
	} {
		out, perr := ParseCloudPhoneAppList(cloudPhoneEnvelope(t, body))
		if perr != nil || out != nil {
			t.Errorf("%s：应是 nil/不报错，实得 %#v / %v", name, out, perr)
		}
	}
	if _, perr := ParseCloudPhoneAppList(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"应用下架了"}}`)); perr == nil {
		t.Error("status 非 200 应报错")
	}
}

// TestParseCloudPhoneInstallStatusWalksTheLegacyChain ⭐ 两层下钻 + 严格数字。
//
// 旧 TS：`data.list[0].instanceList[0].installStatus`（`task.ts:311-313`），
// 并且**只认数字 2**（`=== 2`）。
//
// 反向改坏它：
//   - 把 `cloudPhoneChainInt` 换成包内那个"字符串也收"的 [optInt64Field] ⇒ "2" 那档红
//     （表现是"已装"被判成已装，而旧实现在这一格会去装第二遍）；
//   - 少下一层 `instanceList` ⇒ 第一项红；
//   - 把 nil 折成 0 ⇒ 最后一档红（0 会被编排层当成一个真实取值）。
func TestParseCloudPhoneInstallStatusWalksTheLegacyChain(t *testing.T) {
	statuses := map[string]*int64{}
	for name, body := range map[string]string{
		"数字 2":           `{"header":{"status":"200"},"data":{"list":[{"instanceList":[{"installStatus":2}]}]}}`,
		"字符串 2":          `{"header":{"status":"200"},"data":{"list":[{"instanceList":[{"installStatus":"2"}]}]}}`,
		"数字 1":           `{"header":{"status":"200"},"data":{"list":[{"instanceList":[{"installStatus":1}]}]}}`,
		"两层都给":           `{"header":{"status":"200"},"data":{"list":[{"instanceList":[{"installStatus":2},{"installStatus":3}]}]}}`,
		"缺 list":         `{"header":{"status":"200"},"data":{"anything":1}}`,
		"缺 instance":     `{"header":{"status":"200"},"data":{"list":[{"anything":1}]}}`,
		"instanceList 空": `{"header":{"status":"200"},"data":{"list":[{"instanceList":[]}]}}`,
	} {
		got, perr := ParseCloudPhoneInstallStatus(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Fatalf("%s：只读口不该报错: %v", name, perr)
		}
		statuses[name] = got
	}
	if got := statuses["数字 2"]; got == nil || *got != CloudPhoneInstallStatusInstalled {
		t.Errorf("数字 2 应读成 %d，实得 %v", CloudPhoneInstallStatusInstalled, got)
	}
	// ⚠ 字符串那一档**必须**是 nil：与旧实现的 `=== 2` 同宽（见函数注释）。
	if got := statuses["字符串 2"]; got != nil {
		t.Errorf("字符串 \"2\" 应读成 nil（不收数字串），实得 %d", *got)
	}
	if got := statuses["数字 1"]; got == nil || *got != 1 {
		t.Errorf("数字 1 应原样读出，实得 %v", got)
	}
	// 只看首项（旧实现就是 `list[0]` / `instanceList[0]`）。
	if got := statuses["两层都给"]; got == nil || *got != 2 {
		t.Errorf("应取首项那一个 2，实得 %v", got)
	}
	for _, name := range []string{"缺 list", "缺 instance", "instanceList 空"} {
		if statuses[name] != nil {
			t.Errorf("%s：读不到应是 nil 而不是 0，实得 %d", name, *statuses[name])
		}
	}
	if _, perr := ParseCloudPhoneInstallStatus(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"查不到装机状态"},"data":{"list":[{"installStatus":2}]}}`)); perr == nil {
		t.Error("status 非 200 应报错")
	}
}

// TestParseCloudPhoneInstallSubmitKeepsThreeStates ⭐ `success` 的三态分开交。
//
// 旧 TS 写的是 `if (!data.success) continue`（`task.ts:329`）⇒ 在它眼里
// "没给这一格"与"给了 false"是**同一件事**（都跳过）。本层刻意分开：
// nil 是"上游这次没说装没装"，而这句话值得让编排层看得见 ——
// 它决定的是"要不要把这一件记成需人工"。
func TestParseCloudPhoneInstallSubmitKeepsThreeStates(t *testing.T) {
	for name, body := range map[string]string{
		"true":  `{"header":{"status":"200"},"data":{"success":true,"taskId":"SYNTH-TASK-199"}}`,
		"false": `{"header":{"status":"200"},"data":{"success":false,"taskId":"SYNTH-TASK-199"}}`,
		"缺格":    `{"header":{"status":"200"},"data":{"taskId":"SYNTH-TASK-199"}}`,
		"字符串真":  `{"header":{"status":"200"},"data":{"success":"true"}}`,
		"数字任务":  `{"header":{"status":"200"},"data":{"success":true,"taskId":199}}`,
	} {
		got, perr := ParseCloudPhoneInstallSubmit(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Fatalf("%s：只读口不该报错: %v", name, perr)
		}
		switch name {
		case "true":
			if got.Success == nil || !*got.Success {
				t.Errorf("实得 %#v", got.Success)
			}
			if got.TaskID == nil || *got.TaskID != "SYNTH-TASK-199" {
				t.Errorf("taskId 实得 %v", got.TaskID)
			}
		case "false":
			if got.Success == nil || *got.Success {
				t.Errorf("实得 %#v", got.Success)
			}
		case "缺格", "字符串真":
			// ⚠ "true" 这个**字符串**不算真：与旧 JS 的宽松不同，本层只收真布尔。
			// 这一处是**有意偏离**旧实现的（旧实现会把字符串 "true" 当成就），
			// 偏离的理由：字符串"true"在本族零样本，收下它等于给一个没见过的形状背书。
			if got.Success != nil {
				t.Errorf("%s：应读成 nil，实得 %v", name, *got.Success)
			}
		case "数字任务":
			if got.TaskID != nil {
				t.Errorf("数字型 taskId 应交 nil（不猜、不顺手转），实得 %q", *got.TaskID)
			}
		}
	}
	if _, perr := ParseCloudPhoneInstallSubmit(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"无空闲实例"},"data":{"success":true}}`)); perr == nil {
		t.Error("status 非 200 应报错 —— 否则「没装成」会被读成「装了」")
	}
}

// TestParseCloudPhoneInstallTaskStatusTerminators 轮询那一格的三个终止/非终止档。
//
// 旧 TS 只认 `2`（成功）与 `3`（失败），其余一律继续轮询（`task.ts:339-346`）⇒
// 本层只负责把数字读准，**不**内置"其他就是失败"的判断。
func TestParseCloudPhoneInstallTaskStatusTerminators(t *testing.T) {
	for _, c := range []struct {
		name string
		body string
		want *int64
	}{
		{"done", `{"header":{"status":"200"},"data":{"taskStatus":2}}`, int64p(2)},
		{"failed", `{"header":{"status":"200"},"data":{"taskStatus":3}}`, int64p(3)},
		{"running", `{"header":{"status":"200"},"data":{"taskStatus":1}}`, int64p(1)},
		{"字符串", `{"header":{"status":"200"},"data":{"taskStatus":"2"}}`, nil},
		{"缺格", `{"header":{"status":"200"},"data":{"anything":1}}`, nil},
	} {
		got, perr := ParseCloudPhoneInstallTaskStatus(cloudPhoneEnvelope(t, c.body))
		if perr != nil {
			t.Fatalf("%s：不该报错: %v", c.name, perr)
		}
		switch {
		case c.want == nil && got != nil:
			t.Errorf("%s：应读成 nil，实得 %d", c.name, *got)
		case c.want != nil && got == nil:
			t.Errorf("%s：应读出 %d，实得 nil", c.name, *c.want)
		case c.want != nil && *c.want != *got:
			t.Errorf("%s：实得 %d，应为 %d", c.name, *got, *c.want)
		}
	}
	if _, perr := ParseCloudPhoneInstallTaskStatus(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"任务查不到了"}}`)); perr == nil {
		t.Error("status 非 200 应报错")
	}
}

// TestAppCloudPhoneUserInstallAppJudgesEnvelope ⭐ 补上的那条记账口走完整发送。
//
// 钉三件事：URL 挂对、body 逐字（含"不给 existed 就没这个键"）、信封必须判。
// 最后一条最要紧：记账失败**看起来**像成功（HTTP 200），
// 而它的后果是"任务登记没上、下次还发一遍安装"。
func TestAppCloudPhoneUserInstallAppJudgesEnvelope(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	existed := int64(1)
	body := CloudPhoneInstallReportBody("9", "7", &existed)
	ok := core.NewResponse(200, []byte(`{"header":{"status":"200"},"data":{}}`))
	client := &scriptedClient{response: ok}
	if perr := NewUpstream(client).AppCloudPhoneUserInstallApp(nil, auth, "", body); perr != nil {
		t.Fatalf("成功信封不该报错: %v", perr)
	}
	if n := len(client.requests); n != 1 {
		t.Fatalf("应恰好一条，实得 %d", n)
	}
	request := client.requests[0]
	if want := CloudPhoneBaseURL + "/redpacket/userInstallApp"; request.URL != want {
		t.Errorf("URL 实得 %q，应为 %q", request.URL, want)
	}
	if !strings.Contains(string(request.Body), `"existed":1`) {
		t.Errorf("给了 existed 就该发出去，实得 %s", request.Body)
	}
	// 另一支：装成之后的那次记账**不带**这一格。
	client = &scriptedClient{response: ok}
	if perr := NewUpstream(client).AppCloudPhoneUserInstallApp(nil, auth, "",
		CloudPhoneInstallReportBody("9", "7", nil)); perr != nil {
		t.Fatalf("调用失败: %v", perr)
	}
	if sent := string(client.requests[0].Body); strings.Contains(sent, "existed") {
		t.Errorf("existed 该整键省略（nil 不是 0），实得 %s", sent)
	}
	failing := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"任务已过期"},"data":{"taskId":"9"}}`))}
	if perr := NewUpstream(failing).AppCloudPhoneUserInstallApp(nil, auth, "", body); perr == nil {
		t.Error("HTTP 200 而 status 非 200 时必须报错")
	}
}
