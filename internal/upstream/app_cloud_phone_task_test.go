package upstream

// 红包派对（云机）族 · 任务清单与任务动作的测试。
//
// 取材：夹具第 32/36 条（两条清单口）、第 30/33 条（规则文本）、第 38 条（公告清单）、
// 第 31/34 条（埋点）。`userSign` / `userCompleteTask` / `userBrowse` **零样本** ⇒
// 那三条只钉"我们自己写下的那一形状"（路径 + body 键名）与信封判定，
// 不去假装对过上游。
//
// 凭据纪律同 `app_cloud_phone_test.go`：`jumpLink` 实拍带着 token ⇒
// 测试里换成不可用的合成地址；手机号一律合成尾号。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 两条清单口
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneTaskListRequestsDifferOnlyByPath ⭐ **参数相同而 URL 不同**：
// 第 32 与第 36 条的 body 逐字相同（都是 26 字节那一句），却是两条口。
//
// 反向改坏它：有人"顺手合并成一条"（反正 body 一样）⇒ 路径那两条断言红。
// 这是 `AGENTS.md` §1.12 事实②的反面情形，合一条就等于丢掉登录态那一版。
func TestCloudPhoneTaskListRequestsDifferOnlyByPath(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	plain, perr := CloudPhoneConfigTaskListRequest(auth, cloudPhoneSyntheticReferer)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	logged, perr := CloudPhoneConfigTaskLoginListRequest(auth, cloudPhoneSyntheticReferer)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	for name, request := range map[string]core.HTTPRequest{"configTaskList": plain, "configTaskLoginList": logged} {
		if want := `{"version":"SYS_CONFIG_Y"}`; string(request.Body) != want {
			t.Errorf("%s 的 body 应逐字等于第 32/36 条，实得 %s", name, request.Body)
		}
		if len(request.Body) != 26 {
			t.Errorf("%s 的实拍 content-length 是 26，实得 %d", name, len(request.Body))
		}
	}
	if want := CloudPhoneBaseURL + "/redpacket/configTaskList"; plain.URL != want {
		t.Errorf("实得 %q，应为 %q", plain.URL, want)
	}
	if want := CloudPhoneBaseURL + "/redpacket/configTaskLoginList"; logged.URL != want {
		t.Errorf("实得 %q，应为 %q", logged.URL, want)
	}
	if plain.URL == logged.URL {
		t.Error("两条口被合成了一条")
	}
}

// cloudPhoneCapturedTaskListBody 是第 36 条响应的**忠实裁剪版**：
// 键名与取值逐字照抄，只删掉与本层判据无关的键（`configType` / `createTime` /
// `updateTime` / `sort` / `taskLogo` / `taskExplain` / `aiAppLink` / `aiH5Link` /
// `iconLink` / `iconUrl`）与重复的 43 天签到格。
//
// ⚠ 裁剪的是**条数与无关键**，不是形状：留下的每一格都能在夹具里指到那一条。
const cloudPhoneCapturedTaskListBody = `{"header":{"appId":"12345681","errMsg":"OK","status":"200"},
"data":{
 "configTaskDailyList":[{"id":"6","taskCode":"DAILY_1","taskName":"每日使用云机","taskType":"DAILY",
   "prizeName":"每日使用云机","prizeType":1,"prizeAmount":"20","prizeAmountMany":"20",
   "periodCanNum":1,"userCanCompleteNum":1,"userCompleteNum":0,"userStatus":0}],
 "configTaskNoviceList":[{"id":"3","taskCode":"NOVICE_1","taskName":"领取30GB定向流量","taskType":"NOVICE",
   "prizeName":"领取30GB定向流量","prizeType":1,"prizeAmount":"30","prizeAmountMany":"30",
   "periodCanNum":1,"userCanCompleteNum":1,"userCompleteNum":0,"userStatus":0,
   "jumpLink":"https://synthetic.invalid/path?token=SYNTH-IN-LINK-199"},
  {"id":"4","taskCode":"NOVICE_2","taskName":"浏览玩机教程","taskType":"NOVICE",
   "prizeName":"浏览玩机教程","prizeType":1,"prizeAmount":"30","prizeAmountMany":"30",
   "periodCanNum":1,"userCanCompleteNum":0,"userCompleteNum":1,"userStatus":1,
   "jumpLink":"https://synthetic.invalid/path?token=SYNTH-IN-LINK-199"}],
 "configTaskMonthlyList":[{"id":"8","taskCode":"MONTHLY_2","taskName":"在云手机里安装应用","taskType":"MONTHLY",
   "prizeName":"在云手机里安装应用","prizeType":1,"prizeAmount":"30","prizeAmountMany":"30",
   "periodCanNum":3,"userCanCompleteNum":3,"userCompleteNum":0,"userStatus":0},
  {"id":"9","taskCode":"MONTHLY_3","taskName":"趣味答题挑战","taskType":"MONTHLY",
   "prizeName":"趣味答题挑战","prizeType":1,"prizeAmount":"30","prizeAmountMany":"30",
   "periodCanNum":3,"userCanCompleteNum":0,"userCompleteNum":3,"userStatus":1}],
 "configTaskBuyList":[],
 "configTaskSignList":[{"date":"2026-06-30","day":30,"isToday":1,"isWeek":0,"signAmount":"20","status":0},
  {"date":"2026-07-05","day":5,"isToday":0,"isWeek":1,"signAmount":"20","status":2}],
 "totalSignTimes":0,"weixinPrize":"1","secondConfirmStatus":0}}`

// TestParseCloudPhoneTaskListReadsTheCapture ⭐ 清单里每一格判据都指到第 36 条。
//
// 反向改坏它：
//   - 把 `prizeAmount` 当元读（或在本层除 100）⇒ 20/30 那两条断言红；
//   - 把四张表合成一张（"反正键名像"）⇒ `Buy` 那条（实拍是**空数组**）红；
//   - 把 `id` 按数字收 ⇒ `TaskID` 是字符串那一条红。
func TestParseCloudPhoneTaskListReadsTheCapture(t *testing.T) {
	got, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, cloudPhoneCapturedTaskListBody))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(got.Daily) != 1 || len(got.Novice) != 2 || len(got.Monthly) != 2 {
		t.Fatalf("四张表的条数实得 %d/%d/%d", len(got.Daily), len(got.Novice), len(got.Monthly))
	}
	// ⚠ `configTaskBuyList` 实拍**恒是空数组**：条数是 0 也要读成"给了、是空的"。
	if got.Buy == nil {
		t.Error("Buy 表实拍给了空数组 ⇒ 应是空切片而不是 nil")
	}
	if len(got.Buy) != 0 {
		t.Errorf("Buy 表实拍是空数组，实得 %d 条", len(got.Buy))
	}
	item := got.Daily[0]
	if item.TaskID == nil || *item.TaskID != "6" {
		t.Errorf("id 实拍是字符串，实得 %v", item.TaskID)
	}
	if item.TaskCode == nil || *item.TaskCode != string(CloudPhoneTaskCodeDaily1) {
		t.Errorf("taskCode 实得 %v", item.TaskCode)
	}
	if item.TaskType == nil || *item.TaskType != "DAILY" {
		t.Errorf("taskType 实得 %v", item.TaskType)
	}
	// ⭐ 单位是**分**：实拍 `"20"` 同条目里那句"可获得0.2元"就是旁证。
	if item.PrizeAmount == nil || *item.PrizeAmount != 20 {
		t.Errorf("prizeAmount 应是分（20），实得 %v", item.PrizeAmount)
	}
	if item.PrizeAmountMany == nil || *item.PrizeAmountMany != 20 {
		t.Errorf("prizeAmountMany 实得 %v", item.PrizeAmountMany)
	}
	if item.PrizeType == nil || *item.PrizeType != 1 {
		t.Errorf("prizeType 实得 %v", item.PrizeType)
	}
	// 三个额度读数各自交出，本层**不替你算**它们之间的关系。
	if item.PeriodCanNum == nil || *item.PeriodCanNum != 1 ||
		item.UserCanCompleteNum == nil || *item.UserCanCompleteNum != 1 ||
		item.UserCompleteNum == nil || *item.UserCompleteNum != 0 {
		t.Errorf("额度三位实得 %v / %v / %v", item.PeriodCanNum, item.UserCanCompleteNum,
			item.UserCompleteNum)
	}
	if item.UserStatus == nil || *item.UserStatus != CloudPhoneUserStatusUnfinished {
		t.Errorf("userStatus 实得 %v", item.UserStatus)
	}
	if item.JumpLink != nil {
		t.Error("DAILY_1 实拍**没有** jumpLink 这个键 ⇒ 该是 nil")
	}
	// 同一条目里"有 jumpLink"与"没有"两种都实拍过。
	if got.Novice[0].JumpLink == nil || !strings.HasPrefix(*got.Novice[0].JumpLink, "https://synthetic.invalid") {
		t.Errorf("NOVICE_1 的 jumpLink 实得 %v", got.Novice[0].JumpLink)
	}
	// 已完成那一档：`userStatus` 1 配 `userCompleteNum` 1。
	if got.Novice[1].UserStatus == nil || *got.Novice[1].UserStatus != CloudPhoneUserStatusCompleted {
		t.Errorf("NOVICE_2 应是已完成，实得 %v", got.Novice[1].UserStatus)
	}
	// MONTHLY_3 是实拍里唯一"额度用光"的样子：0/3 + 已完成。
	answered := got.Monthly[1]
	if answered.UserCanCompleteNum == nil || *answered.UserCanCompleteNum != 0 ||
		answered.UserCompleteNum == nil || *answered.UserCompleteNum != 3 {
		t.Errorf("MONTHLY_3 的额度读数实得 %v / %v", answered.UserCanCompleteNum, answered.UserCompleteNum)
	}
	// 旁证两位：weixinPrize 实拍是**字符串** "1"，totalSignTimes 是数字 0。
	if got.WeixinPrize == nil || *got.WeixinPrize != "1" {
		t.Errorf("weixinPrize 实拍是字符串，实得 %v", got.WeixinPrize)
	}
	if got.TotalSignTimes == nil || *got.TotalSignTimes != 0 {
		t.Errorf("totalSignTimes 实拍给了 0 ⇒ 该读到 0 而不是 nil，实得 %v", got.TotalSignTimes)
	}
	if len(got.SignList) != 2 {
		t.Errorf("签到日历实得 %d 项", len(got.SignList))
	}
}

// TestCloudPhoneTaskListSecondConfirmStatusOnlyOnLoginList ⭐ 两条清单响应的键集合
// **只差这一位**：第 36 条有 `secondConfirmStatus`、第 32 条没有。
//
// 这就是它必须是指针的全部理由：把两条都填成 0，
// 下一层会以为"未登录态也明确说了二次确认是关"，而上游那句话**根本没说**。
//
// 反向改坏它：`optInt64Field` 换成缺省 0 ⇒ 第二条红。
func TestCloudPhoneTaskListSecondConfirmStatusOnlyOnLoginList(t *testing.T) {
	logged, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, cloudPhoneCapturedTaskListBody))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if logged.SecondConfirmStatus == nil || *logged.SecondConfirmStatus != 0 {
		t.Errorf("第 36 条给了 secondConfirmStatus: 0 ⇒ 该读到 0，实得 %v", logged.SecondConfirmStatus)
	}
	before, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t,
		strings.Replace(cloudPhoneCapturedTaskListBody, `,"secondConfirmStatus":0`, "", 1)))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if before.SecondConfirmStatus != nil {
		t.Errorf("第 32 条**没有**这一位 ⇒ 该是 nil，实得 %d", *before.SecondConfirmStatus)
	}
	// 其余读数不受影响（少一个键不该把整份清单读歪）。
	if len(before.Daily) != 1 || before.WeixinPrize == nil {
		t.Errorf("少一个键把别的读数也带歪了：%#v", before)
	}
}

// TestCloudPhoneSignListStatusIsAnotherDictionary ⭐ 签到日历里的 `status`
// 与任务条目里的 `userStatus` **不是同一本字典**。
//
// 实拍：`2` 在这几位上出现在**未来七天**（2026-07-01 起，而响应日期是 2026-06-30），
// 读起来是"还没到那一天"；而成文给 `userStatus` 那五档里的 `2` 标的是"未定义"。
// ⇒ 这里刻意用裸 `*int64`：谁哪天"顺手统一成 [CloudPhoneUserStatus]"，这一条红。
func TestCloudPhoneSignListStatusIsAnotherDictionary(t *testing.T) {
	got, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, cloudPhoneCapturedTaskListBody))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	today, future := got.SignList[0], got.SignList[1]
	if today.Status == nil || *today.Status != 0 {
		t.Errorf("今天那格实拍是 0，实得 %v", today.Status)
	}
	if future.Status == nil || *future.Status != 2 {
		t.Errorf("未来那格实拍是 2，实得 %v", future.Status)
	}
	// 同一格里的其余读数（含**字符串**形态的金额位）。
	if today.Date == nil || *today.Date != "2026-06-30" {
		t.Errorf("date 实得 %v", today.Date)
	}
	if today.IsToday == nil || *today.IsToday != 1 {
		t.Errorf("isToday 实得 %v", today.IsToday)
	}
	if today.SignAmount == nil || *today.SignAmount != 20 {
		t.Errorf("signAmount 实拍是字符串 \"20\"（分），实得 %v", today.SignAmount)
	}
	if future.IsWeek == nil || *future.IsWeek != 1 {
		t.Errorf("isWeek 实得 %v", future.IsWeek)
	}
	// 反证：字段类型是裸 int64 —— 这两行编译不过就说明有人把它换成了档位类型。
	var raw int64
	if future.Status != nil {
		raw = *future.Status
	}
	if raw != 2 {
		t.Errorf("实得 %d", raw)
	}
}

// TestCloudPhoneTaskItemsShapeChangeIsVisibleNotFatal ⭐ 承诺过的那一条：
// 上游把某张表从数组换成对象 ⇒ **不报错**、交 nil。
//
// 为什么不是错误（也不是"改成报错"）：这是只读清单口，"读不出来"要由编排层落成
// "这一格不知道"，而不是落成一次接口故障（先例 [ParseNoticeTaskInfo]）。
// ⚠ 代价也写在这儿：形状变了**不会有任何断言变红**，只表现为"清单是空的" ——
// 所以这条测试钉的就是"今天的行为是 nil 而非 0 条而非报错"，改坏会红。
func TestCloudPhoneTaskItemsShapeChangeIsVisibleNotFatal(t *testing.T) {
	for name, body := range map[string]string{
		"表成了对象":   `{"header":{"status":"200"},"data":{"configTaskDailyList":{"6":{}}}}`,
		"表成了字符串":  `{"header":{"status":"200"},"data":{"configTaskDailyList":"x"}}`,
		"表是 null": `{"header":{"status":"200"},"data":{"configTaskDailyList":null}}`,
		"这个键没有":   `{"header":{"status":"200"},"data":{}}`,
	} {
		got, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：只读口不该报错: %v", name, perr)
			continue
		}
		if got.Daily != nil {
			t.Errorf("%s：应交 nil（让调用方看得见\"没读到\"），实得 %d 条", name, len(got.Daily))
		}
	}
	// data 整个缺失 ⇒ 全空结果、不报错。
	empty, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, `{"header":{"status":"200"}}`))
	if perr != nil {
		t.Fatalf("不该报错: %v", perr)
	}
	if empty.Daily != nil || empty.SignList != nil || empty.SecondConfirmStatus != nil {
		t.Errorf("应是零值结果，实得 %#v", empty)
	}
	// 而信封不过必须报错（这与"载荷读不出"是两件事）。
	if _, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"清单挂了"},"data":{}}`)); perr == nil {
		t.Error("非 200 应报错")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 三条零样本的任务动作
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneUserSignRequestCarriesTheTaskDimension ⭐ 挡住那个已记录的 bug。
//
// `docs/caiyun-go-feature-inventory.md` 第 72 行（表内编号 160）与
// `docs/caiyun-go-parameters-and-decisions.md` 第 166 行的表 T-4 记着：
// 参考实现的 `userSign` 收了 `taskID` 却发**空 body** ⇒ 签到永远打在服务端默认项。
// ⇒ 这一条钉的是"传进去的键一定出现在 body 里"，
// 而**不是**某个键名（键名零样本，本层不猜，见函数注释）。
//
// 反向改坏它：把 body 写死成只有 `version`（正是那个缺陷）⇒ 第一条红。
func TestCloudPhoneUserSignRequestCarriesTheTaskDimension(t *testing.T) {
	request, perr := CloudPhoneUserSignRequest(cloudPhoneSyntheticAuth(), cloudPhoneSyntheticReferer,
		map[string]any{"taskCode": CloudPhoneTaskCodeDaily1.String(), "someId": "199"})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	body := string(request.Body)
	for _, key := range []string{"taskCode", "someId", "version"} {
		if !strings.Contains(body, `"`+key+`"`) {
			t.Errorf("body 里少了 %s（参考实现那个缺陷的形状就是这个样子）：%s", key, body)
		}
	}
	if want := CloudPhoneBaseURL + "/redpacket/userSign"; request.URL != want {
		t.Errorf("URL 实得 %q（⚠ 前缀是推的，404 时改常量那一行）", request.URL)
	}
	// 传进来的 map **不该被改**（同一份上下文会发好几枪）。
	data := map[string]any{"someId": "199"}
	if _, perr := CloudPhoneUserSignRequest(cloudPhoneSyntheticAuth(), "", data); perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if _, dirty := data["version"]; dirty {
		t.Error("调用方的 map 被写脏了（version 该拼在副本上）")
	}
}

// TestCloudPhoneUserBrowseBodyPinsTheDocumentedKey ⭐ `taskCode` 是成文
// §2.4 第 374 行逐字给过的键名（`userBrowse {taskCode}`）。
//
// 反向改坏它：把它拼成 `taskcode` / `code` ⇒ 逐字那条红。
func TestCloudPhoneUserBrowseBodyPinsTheDocumentedKey(t *testing.T) {
	request, perr := CloudPhoneUserBrowseRequest(cloudPhoneSyntheticAuth(), cloudPhoneSyntheticReferer,
		CloudPhoneTaskCodeMonthly1)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"taskCode":"MONTHLY_1","version":"SYS_CONFIG_Y"}`; string(request.Body) != want {
		t.Errorf("body 实得\n%s\n应为 %s", request.Body, want)
	}
	if want := CloudPhoneBaseURL + "/redpacket/userBrowse"; request.URL != want {
		t.Errorf("URL 实得 %q", request.URL)
	}
}

// TestParseCloudPhoneCompleteTaskMissingStatusIsNotSuccess ⭐⭐
// **本层刻意偏离旧实现**的那一条。
//
// 成文 §2.4 第 379 行：旧实现的判据是 `!isNil(data.data?.status) && status !== 1`
// ⇒ **status 缺省时它算成功**。本层不跟：缺这一格就是 nil。
// 先例与理由见 [ParseNoticeTaskInfo] 与 [MsgPushObtain.ObtainCode]
// （"上游没给这一槽 ≠ 给了空槽"）；这里偏离的是**旧实现**，不是本仓口径。
//
// 反向改坏它：把 nil 折成"成功"（跟旧实现对齐）⇒ 第一档红；
// 把 `2` 折进 Completed ⇒ 第四档红。
func TestParseCloudPhoneCompleteTaskMissingStatusIsNotSuccess(t *testing.T) {
	for name, body := range map[string]string{
		"status 缺失（旧实现在这里判成功）": `{"header":{"status":"200"},"data":{}}`,
		"data 整段缺失":  `{"header":{"status":"200"}}`,
		"status 是空串": `{"header":{"status":"200"},"data":{"status":""}}`,
		"status 是对象": `{"header":{"status":"200"},"data":{"status":{"a":1}}}`,
	} {
		got, perr := ParseCloudPhoneCompleteTask(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：只读判据不该报错: %v", name, perr)
			continue
		}
		if got.Status != nil {
			t.Errorf("%s：缺的那格该是 nil 而不是任何档位，实得 %v", name, *got.Status)
		}
	}
	for _, c := range []struct {
		body string
		want CloudPhoneUserStatus
	}{
		{`{"header":{"status":"200"},"data":{"status":1}}`, CloudPhoneUserStatusCompleted},
		{`{"header":{"status":"200"},"data":{"status":0}}`, CloudPhoneUserStatusUnfinished},
		{`{"header":{"status":"200"},"data":{"status":2}}`, CloudPhoneUserStatusUndefined2},
		{`{"header":{"status":"200"},"data":{"status":4}}`, CloudPhoneUserStatusTomorrow},
	} {
		got, perr := ParseCloudPhoneCompleteTask(cloudPhoneEnvelope(t, c.body))
		if perr != nil {
			t.Fatalf("%s 解析失败: %v", c.body, perr)
		}
		if got.Status == nil || *got.Status != c.want {
			t.Errorf("实得 %v，应为 %v", got.Status, c.want)
		}
	}
	// `2` 那一档不被折进任何别的档（它既不是"已完成"也不是"未完成"）。
	two, perr := ParseCloudPhoneCompleteTask(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"status":2}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if *two.Status != CloudPhoneUserStatusUndefined2 || !two.Status.Known() {
		t.Errorf("2 应是**有名有姓**的未定义档，实得 %v", *two.Status)
	}
	// 信封不过 ⇒ 报错（"没打上"与"打上但状态未知"是三件事里的两件）。
	if _, perr := ParseCloudPhoneCompleteTask(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"记账失败"},"data":{"status":1}}`)); perr == nil {
		t.Error("信封不过时应报错，不能交出一个看起来像成功的读数")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 两条配置口与一条埋点口（有实拍，且**不带** version）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneRuleInfoAndRuleConfigListBodiesMatchCapture ⭐ 逐字对第 30/33、38 条，
// 并且钉住"这两条**没有** `version`"这件事。
//
// 反向改坏它：把这两条也走统一 body 拼法（[CloudPhoneRequest]）⇒
// 两个 `not` 断言当场红，而这一族的表现形式是"上游照样回 200"。
func TestCloudPhoneRuleInfoAndRuleConfigListBodiesMatchCapture(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	rule, perr := CloudPhoneRuleInfoRequest(auth, cloudPhoneSyntheticReferer,
		CloudPhoneActivityCodeRedpacketRule)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"activityCode":"STORAGE_REDPACKET_RULE"}`; string(rule.Body) != want {
		t.Errorf("ruleInfo 的 body 实得 %s，应为 %s", rule.Body, want)
	}
	if len(rule.Body) != 41 {
		t.Errorf("实拍第 30/33 条 content-length 是 41，实得 %d", len(rule.Body))
	}
	if want := CloudPhoneBaseURL + "/activity/common/ruleInfo"; rule.URL != want {
		t.Errorf("URL 实得 %q", rule.URL)
	}
	list, perr := CloudPhoneRuleConfigListRequest(auth, cloudPhoneSyntheticReferer, CloudPhoneRuleCodes)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"codes":"STORAGE_REDPACKET_POP,STORAGE_REDPACKET_ROLL"}`; string(list.Body) != want {
		t.Errorf("rule/config/list 的 body 实得 %s，应为 %s", list.Body, want)
	}
	if len(list.Body) != 56 {
		t.Errorf("实拍第 38 条 content-length 是 56（与 body 一致），实得 %d", len(list.Body))
	}
	if want := CloudPhoneBaseURL + "/rule/config/list"; list.URL != want {
		t.Errorf("URL 实得 %q", list.URL)
	}
	for name, request := range map[string]core.HTTPRequest{"ruleInfo": rule, "rule/config/list": list} {
		if strings.Contains(string(request.Body), CloudPhoneVersion) {
			t.Errorf("%s 的 body 里凭空多了 version（实拍这两条**没有**这个键）：%s", name, request.Body)
		}
	}
}

// TestParseCloudPhoneRuleInfoReadsTheCapture 规则文本三位（第 30 条）。
//
// 反向改坏它：在本层清洗 HTML（实拍是带标签的一段）⇒ 首字符那条红；
// 把 title 缺省折成"活动规则"这种自造名 ⇒ 第二条红。
func TestParseCloudPhoneRuleInfoReadsTheCapture(t *testing.T) {
	got, perr := ParseCloudPhoneRuleInfo(cloudPhoneEnvelope(t,
		`{"header":{"appId":"12345681","errMsg":"OK","status":"200"},`+
			`"data":{"code":"STORAGE_REDPACKET_RULE","title":"红包派对-活动规则",`+
			`"ruleText":"<div class=\"content\">一、活动时间：</span>2026年1月1日-2026年6月30日</div>"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.Code == nil || *got.Code != CloudPhoneActivityCodeRedpacketRule {
		t.Errorf("code 实得 %v", got.Code)
	}
	if got.Title == nil || *got.Title != "红包派对-活动规则" {
		t.Errorf("title 实得 %v", got.Title)
	}
	if got.RuleText == nil || !strings.HasPrefix(*got.RuleText, "<div") {
		t.Errorf("ruleText 应**原样**交出（谁展示谁转义），实得 %v", got.RuleText)
	}
	// 三位全缺 ⇒ 全 nil、不报错（这是纯展示内容，不该打断任何链）。
	empty, perr := ParseCloudPhoneRuleInfo(cloudPhoneEnvelope(t, `{"header":{"status":"200"}}`))
	if perr != nil {
		t.Fatalf("不该报错: %v", perr)
	}
	if empty != (CloudPhoneRuleText{}) {
		t.Errorf("应是零值，实得 %#v", empty)
	}
	if _, perr := ParseCloudPhoneRuleInfo(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"规则没了"},"data":{}}`)); perr == nil {
		t.Error("信封不过时应报错")
	}
}

// TestParseCloudPhoneRuleConfigListReadsBothItems 公告清单（第 38 条两项）。
//
// 反向改坏它：`data.list` 换成对象 ⇒ 交 nil（与清单口同一处置，最后一档红）；
// 把 `readStatus` 缺省折成 0 ⇒ 第二项红（那一项实拍是 1，另一项才是 0）。
func TestParseCloudPhoneRuleConfigListReadsBothItems(t *testing.T) {
	got, perr := ParseCloudPhoneRuleConfigList(cloudPhoneEnvelope(t,
		`{"header":{"appId":"12345681","errMsg":"OK","status":"200"},"data":{"list":[`+
			`{"code":"STORAGE_REDPACKET_POP","id":"5","status":0,"readStatus":1,"markNum":0,`+
			`"title":"红包派对-弹窗通知","ruleText":"<div class=\"title\">【重要公告】</div>",`+
			`"createTime":"2024-11-29 10:34:08"},`+
			`{"code":"STORAGE_REDPACKET_ROLL","id":"6","status":0,"readStatus":0,"markNum":0,`+
			`"title":"红包派对-滚动通知","ruleText":"<div>第二条</div>"}]}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(got.Items) != 2 {
		t.Fatalf("应两项，实得 %d", len(got.Items))
	}
	first, second := got.Items[0], got.Items[1]
	if first.Code == nil || *first.Code != "STORAGE_REDPACKET_POP" {
		t.Errorf("code 实得 %v", first.Code)
	}
	if first.AnnID == nil || *first.AnnID != "5" {
		t.Errorf("id 实拍是字符串，实得 %v", first.AnnID)
	}
	if first.Title == nil || *first.Title != "红包派对-弹窗通知" {
		t.Errorf("title 实得 %v", first.Title)
	}
	if first.ReadStatus == nil || *first.ReadStatus != 1 {
		t.Errorf("弹窗那条 readStatus 实拍是 1，实得 %v", first.ReadStatus)
	}
	if second.ReadStatus == nil || *second.ReadStatus != 0 {
		t.Errorf("滚动那条是 0，实得 %v", second.ReadStatus)
	}
	if first.Status == nil || *first.Status != 0 || first.MarkNum == nil || *first.MarkNum != 0 {
		t.Errorf("status/markNum 实得 %v / %v", first.Status, first.MarkNum)
	}
	if second.RuleText == nil || !strings.Contains(*second.RuleText, "<div>") {
		t.Errorf("ruleText 应原样交出，实得 %v", second.RuleText)
	}
	// list 不是数组 ⇒ nil、不报错。
	for name, body := range map[string]string{
		"list 是对象": `{"header":{"status":"200"},"data":{"list":{"5":{}}}}`,
		"没有 list":  `{"header":{"status":"200"},"data":{}}`,
	} {
		out, perr := ParseCloudPhoneRuleConfigList(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：不该报错: %v", name, perr)
			continue
		}
		if out.Items != nil {
			t.Errorf("%s：应交 nil，实得 %d 项", name, len(out.Items))
		}
	}
}

// TestCloudPhoneBilogBodiesMatchCapture ⭐ 逐字对第 31/34 条（105 与 119 字节）。
//
// ⚠ 第 34 条实拍那一位 `extra2` 是**带尾号的掩码手机号**，按仓库隐私红线不能转述
// （连"前三位 + 星号 + 后四位"这种形态也不行）⇒ 换成不带尾号的 `19****`。
// 键名、顺序、`activityId` 那串 base64 都逐字保留。
//
// 反向改坏它：把 `activityId` "顺手解码成明文"⇒ 两条红；
// 给这一条补 `version` ⇒ 红（实拍两条都没有）；
// 无条件发出 `extra1`/`extra2` 两个键 ⇒ "缺键"与"这一位差多少字节"两条断言同时红。
func TestCloudPhoneBilogBodiesMatchCapture(t *testing.T) {
	const extra2Value = "19****"
	auth := cloudPhoneSyntheticAuth()
	home, perr := CloudPhoneBilogUploadRequest(auth, cloudPhoneSyntheticReferer, CloudPhoneBilogFields{
		Action: "首页-活动页曝光", Extra1: "红包派对首页",
	})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	wantHome := `{"action":"首页-活动页曝光","activityId":"` + CloudPhoneBilogActivityID +
		`","extra1":"红包派对首页"}`
	if string(home.Body) != wantHome {
		t.Errorf("第 31 条 body 实得\n%s\n应为\n%s", home.Body, wantHome)
	}
	if len(home.Body) != 105 {
		t.Errorf("实拍 content-length 是 105，实得 %d", len(home.Body))
	}
	login, perr := CloudPhoneBilogUploadRequest(auth, cloudPhoneSyntheticReferer, CloudPhoneBilogFields{
		Action: "登录-登录成功", Extra1: "单点登录", Extra2: extra2Value,
	})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if !strings.Contains(string(login.Body), `"extra2":"`+extra2Value+`"`) {
		t.Errorf("extra2 该按给的顺序进 body，实得 %s", login.Body)
	}
	// 实拍第 34 条 content-length 是 119，那一格 `extra2` 是 11 字节的掩码手机号；
	// 换成 6 字节的不带尾号掩码 ⇒ 整条短 5 字节。114 这个数是"实拍 119 减掉那 5 字节"，
	// 不是拍脑袋：**别为了凑 119 把真实形态的号码写回测试里**。
	if len(login.Body) != 119-5 {
		t.Errorf("实拍 content-length 119（减掉掩码短的那 5 字节）应为 114，实得 %d", len(login.Body))
	}
	// 与夹具无关的**结构**钉法：同一条请求只多/少 extra2 这一位，差值恰是那一段的字节数。
	loginNoExtra, perr := CloudPhoneBilogUploadRequest(auth, cloudPhoneSyntheticReferer,
		CloudPhoneBilogFields{Action: "登录-登录成功", Extra1: "单点登录"})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if delta, want := len(login.Body)-len(loginNoExtra.Body), len(`,"extra2":"`+extra2Value+`"`); delta != want {
		t.Errorf("给 extra2 相对不给，body 应多 %d 字节，实得 %d", want, delta)
	}
	if strings.Contains(string(home.Body), "extra2") || strings.Contains(string(loginNoExtra.Body), "extra2") {
		t.Error("没给 extra2 就不该出现这个键（实拍两条的键集合正差这一位）")
	}
	for name, request := range map[string]core.HTTPRequest{"31": home, "34": login} {
		if strings.Contains(string(request.Body), CloudPhoneVersion) {
			t.Errorf("%s 的 body 里凭空多了 version：%s", name, request.Body)
		}
		if want := CloudPhoneBaseURL + "/bilog/upload"; request.URL != want {
			t.Errorf("URL 实得 %q", request.URL)
		}
	}
	// 载荷读数：实拍两条都是 `{"ret":1}`。
	got, perr := ParseCloudPhoneBilogRet(cloudPhoneEnvelope(t,
		`{"data":{"ret":1},"header":{"appId":"12345681","errMsg":"OK",`+
			`"requestId":"2071905980258680833","status":"200"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.Ret == nil || *got.Ret != 1 {
		t.Errorf("ret 实得 %v", got.Ret)
	}
	// ⚠ ret 缺 ⇒ nil：它是不是成功判据**未验证**（没有任何非 1 的样本可对照）。
	missing, perr := ParseCloudPhoneBilogRet(cloudPhoneEnvelope(t, `{"header":{"status":"200"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if missing.Ret != nil {
		t.Errorf("缺位应是 nil，实得 %d", *missing.Ret)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 任务码 + 发送层判信封
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneTaskCodesAreUpstreamLiterals 九个实拍码逐字照抄。
//
// ⚠ 这里**不**断言"一共只有这九个"：任务表随活动期次换（实现文件头记了为什么
// 不做"码 → 分支"的映射表）。它挡的是"有人把某个码顺手改成小写或下划线"。
func TestCloudPhoneTaskCodesAreUpstreamLiterals(t *testing.T) {
	want := []string{"DAILY_1", "NOVICE_1", "NOVICE_2", "NOVICE_3",
		"MONTHLY_1", "MONTHLY_2", "MONTHLY_3", "MONTHLY_4", "MONTHLY_5"}
	got := []CloudPhoneTaskCode{CloudPhoneTaskCodeDaily1, CloudPhoneTaskCodeNovice1,
		CloudPhoneTaskCodeNovice2, CloudPhoneTaskCodeNovice3, CloudPhoneTaskCodeMonthly1,
		CloudPhoneTaskCodeMonthly2, CloudPhoneTaskCodeMonthly3, CloudPhoneTaskCodeMonthly4,
		CloudPhoneTaskCodeMonthly5}
	for i, code := range got {
		if code.String() != want[i] {
			t.Errorf("第 %d 个码应逐字为 %q，实得 %q", i, want[i], code.String())
		}
	}
}

// TestAppCloudPhoneWritesJudgeTheEnvelope ⭐ 三条写口都必须判信封。
//
// [Upstream.Call] 只在**非 2xx** 上回错误，而这一族的成败位在 `header.status`；
// 不判就等于"上游回了 status 500，我们报告任务已登记/已签到/已浏览"。
//
// 反向改坏它：把任一处的 `EnsureCloudPhoneOK` 去掉 ⇒ 对应那一档红。
func TestAppCloudPhoneWritesJudgeTheEnvelope(t *testing.T) {
	notOK := core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"活动已结束"},"data":{"errorMsg":"活动已结束"}}`))
	ok := core.NewResponse(200, []byte(`{"header":{"status":"200"},"data":{"status":1}}`))
	auth := cloudPhoneSyntheticAuth()
	for _, c := range []struct {
		name string
		send func(client *scriptedClient) *core.Error
	}{
		{"userSign", func(client *scriptedClient) *core.Error {
			return NewUpstream(client).AppCloudPhoneUserSign(nil, auth, "",
				map[string]any{"someId": "199"})
		}},
		{"userBrowse", func(client *scriptedClient) *core.Error {
			return NewUpstream(client).AppCloudPhoneUserBrowse(nil, auth, "", CloudPhoneTaskCodeMonthly1)
		}},
		{"userCompleteTask", func(client *scriptedClient) *core.Error {
			_, perr := NewUpstream(client).AppCloudPhoneUserCompleteTask(nil, auth, "",
				map[string]any{"someId": "199"})
			return perr
		}},
	} {
		failing := &scriptedClient{response: notOK}
		if perr := c.send(failing); perr == nil {
			t.Errorf("%s：status 非 200 时必须报错（不判信封就是把失败报成成功）", c.name)
		}
		passing := &scriptedClient{response: ok}
		if perr := c.send(passing); perr != nil {
			t.Errorf("%s：成功信封不该报错: %v", c.name, perr)
		}
		if n := len(passing.requests); n != 1 {
			t.Errorf("%s：应恰好发一条，实得 %d", c.name, n)
		}
	}
}

// TestCloudPhoneActivityHomeURLIsCredentialFree referer 候选地址里不许有凭据。
//
// 实拍第 9 条那条活动页 URL 是带 token 的；这里只留**去凭据**的那一版。
func TestCloudPhoneActivityHomeURLIsCredentialFree(t *testing.T) {
	if strings.Contains(strings.ToLower(CloudPhoneActivityHomeURL), "token") {
		t.Errorf("referer 候选地址里不该含 token：%q", CloudPhoneActivityHomeURL)
	}
	if !strings.HasPrefix(CloudPhoneActivityHomeURL, CloudPhoneOrigin) {
		t.Errorf("应与实拍 Origin 同主机，实得 %q", CloudPhoneActivityHomeURL)
	}
}

// TestAppCloudPhoneTaskListReadersHitTheirOwnPaths ⭐ 两条清单口在**发送层**也各走各的。
//
// 上面那条钉的是构造函数；这一条钉的是 `AppCloudPhoneConfigTaskList` 与
// `AppCloudPhoneConfigTaskLoginList` 共用 [cloudPhoneTaskListAt] 时**传对了构造器**。
// 这一档不钉就是静默的：两条口共用一份解析、只差函数值，把两个 builder 写反
// 在今天**没有任何断言会红** —— 而表现是"登录后那张清单永远读成未登录那份"
// （少一位 `secondConfirmStatus`、任务状态全 0），看起来像"用户还没做过任务"。
//
// 反向改坏它：把两个函数里的 builder 互换 ⇒ 两条 URL 断言同时红。
func TestAppCloudPhoneTaskListReadersHitTheirOwnPaths(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	for name, c := range map[string]struct {
		want string
		call func(client *scriptedClient) (CloudPhoneTaskList, *core.Error)
	}{
		"configTaskList": {
			want: CloudPhoneBaseURL + "/redpacket/configTaskList",
			call: func(client *scriptedClient) (CloudPhoneTaskList, *core.Error) {
				return NewUpstream(client).AppCloudPhoneConfigTaskList(nil, auth, "")
			},
		},
		"configTaskLoginList": {
			want: CloudPhoneBaseURL + "/redpacket/configTaskLoginList",
			call: func(client *scriptedClient) (CloudPhoneTaskList, *core.Error) {
				return NewUpstream(client).AppCloudPhoneConfigTaskLoginList(nil, auth, "")
			},
		},
	} {
		client := &scriptedClient{response: core.NewResponse(200, []byte(
			cloudPhoneCapturedTaskListBody))}
		got, perr := c.call(client)
		if perr != nil {
			t.Fatalf("%s 调用失败: %v", name, perr)
		}
		if n := len(client.requests); n != 1 {
			t.Fatalf("%s 一次调用应只发一条（两条各一次是编排层的事），实得 %d 条", name, n)
		}
		if sent := client.requests[0].URL; sent != c.want {
			t.Errorf("%s 实发到 %q，应为 %q", name, sent, c.want)
		}
		// 同一次调用要把载荷读出来（证明走的确实是"发 + 解"这一条链，不是只构造）。
		if len(got.Daily) != 1 || len(got.SignList) != 2 {
			t.Errorf("%s 载荷没读出来：%#v", name, got)
		}
	}
}

// TestParseCloudPhoneTaskListFailureTextCarriesNoPayload ⭐ 载荷里的 `jumpLink` 是带
// token 的 URL（实拍那一格就是）⇒ 信封失败时**只**许带那三处 msg 位，不许把载荷原文
// 拼进错误（`core.Error.Message` 会进日志，而日志会留档）。
//
// 反向改坏它：把 [EnsureCloudPhoneOK] 的文案改成"status + 整份 data 原文"
// （"这样好排查"）⇒ 本条红，而真实样本里被带出去的就是那条 token。
func TestParseCloudPhoneTaskListFailureTextCarriesNoPayload(t *testing.T) {
	const inLink = "SYNTH-TOKEN-IN-JUMPLINK-199"
	// `mobile` 是**我们模型里没有的键**：它只存在于原始 JSON 里，用来验证"错误文案
	// 会不会顺手把载荷原文带出去"。值取不带尾号的合成掩码（隐私红线，见文件头）。
	const inMobile = "19****"
	body := `{"header":{"status":"500","errMsg":"活动已结束"},` +
		`"data":{"configTaskDailyList":[{"id":"6","taskCode":"DAILY_1","mobile":"` + inMobile + `",` +
		`"jumpLink":"https://synthetic.invalid/activityCenter?token=` + inLink + `"}]}}`
	_, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, body))
	if perr == nil {
		t.Fatal("status 非 200 应报错")
	}
	if strings.Contains(perr.Message(), inLink) {
		t.Errorf("错误文案里出现了载荷原文（含 jumpLink 那枚 token）：%q", perr.Message())
	}
	if !strings.Contains(perr.Message(), "活动已结束") {
		t.Errorf("错误该带上游那句 errMsg 原文，实得 %q", perr.Message())
	}
	// 账号标识这一档：只出现在测试输入里，绝不该出现在错误文案里。
	if strings.Contains(perr.Message(), inMobile) {
		t.Errorf("错误文案里出现了账号标识：%q", perr.Message())
	}
}

// TestParseCloudPhoneTaskListRefusesHeaderWithoutStatus ⭐ "有 header、但**压根没给 status**"
// 那一格必须报错，不许顺手当成功。
//
// 为什么单独钉这一档：这一族的信封**没有 `code` 那一位**，而通用信封的宽松判据
// 找的正是 `code`（找不到就算过）⇒ [EnsureCloudPhoneOK] 存在的理由就是堵这个洞。
// 上面几条钉的是"status 给了但不是 200"，走不到"status 这一位不存在"那一支。
//
// 反向改坏它：把 `if status == nil` 那一支改成"交 data、不报错"（没写失败就是成功嘛）
// ⇒ 本条红；而它的真实后果是把"活动已经结束"读成"这张清单恰好是空的"。
func TestParseCloudPhoneTaskListRefusesHeaderWithoutStatus(t *testing.T) {
	for name, body := range map[string]string{
		"header 在但没 status": `{"header":{"appId":"12345681","errMsg":"OK"},` +
			`"data":{"configTaskDailyList":[{"id":"6","taskCode":"DAILY_1"}]}}`,
		"status 是数字":    `{"header":{"status":200},"data":{"configTaskDailyList":[{"id":"6"}]}}`,
		"status 是 null": `{"header":{"status":null},"data":{}}`,
		"整个 header 没有":  `{"data":{"configTaskDailyList":[{"id":"6"}]}}`,
	} {
		list, perr := ParseCloudPhoneTaskList(cloudPhoneEnvelope(t, body))
		if perr == nil {
			t.Errorf("%s：不能判成成功，实得 Daily=%d 项", name, len(list.Daily))
			continue
		}
		if perr.Kind() != core.KindParse {
			t.Errorf("%s：形状不认识应归 Parse，实得 %v", name, perr.Kind())
		}
		if len(list.Daily) != 0 || list.SignList != nil {
			t.Errorf("%s：报错时不该还交出一副读好了的载荷：%+v", name, list)
		}
	}
	// 发送层同样拦住（改一处解析器就绕过的风险，钉在真调用上）。
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"appId":"12345681","errMsg":"OK"},`+
			`"data":{"configTaskDailyList":[{"id":"6","taskCode":"DAILY_1"}]}}`))}
	got, perr := NewUpstream(client).AppCloudPhoneConfigTaskList(nil, cloudPhoneSyntheticAuth(), "")
	if perr == nil {
		t.Fatalf("发送层把没给 status 读成了成功，实得 %d 项", len(got.Daily))
	}
	if perr.Kind() != core.KindParse {
		t.Errorf("实得 Kind=%v", perr.Kind())
	}
	if n := len(client.requests); n != 1 {
		t.Errorf("应只发一条请求，实得 %d 条", n)
	}
}
