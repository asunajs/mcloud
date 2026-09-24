// 红包派对（云机）族 · 答题与 RCS：`configTopicList`、`userTopicAnswer`、
// `getRcsToken`。
//
// ⚠⚠⚠ **本文件三条口全部零实拍样本** ⚠⚠⚠
// `test/fixtures/http/红包派对.json` 的 39 条里，这三个路径名 grep **命中 0**
// （2026-09-24 逐条核过；连 `topic` 这个词在整份夹具里也只出现在一条无关的
// `jumpUrl` 上）。形状的唯一出处是 `docs/business-feature-inventory.md` §2.4：
//
//	第 376 行 答题类：`configTopicList` 取 `[0]`，查**内置 15 条 FAQ 题库**
//	          （`KNOWN_ANSWERS`，改版即静默失效）→ `userTopicAnswer
//	          {answer:"A"|"B"|"C"|"D"}`（最多重试 15×150ms 直到题目在库里）
//	第 377 行 `DAILY_1`：`getRcsToken` 返回值丢弃
//	          + info("需手动完成 … (需云机内实际使用)") + **`return true`**（假成功，§1.4）
//
// ⇒ 这两句给得出：题目是**一张列表**（旧实现取 `[0]`）、答案键叫 `answer` 且取值是
// `"A"|"B"|"C"|"D"`（`userBrowse {taskCode}` 之后成文第二次、也是最后一次给出 body 键名）。
// 给不出的是：列表在 `data` 的哪个键下、题目里哪一格是题干/选项、
// `getRcsToken` 的 body 与返回。**缺的都不猜**（理由见各函数注释）。
//
// # `getRcsToken` 那一处最容易搬错
//
// 夹具里有两条 `getRcsToken`（第 8、28 条），但它们在
// `cloud.139.com/uniphone/user/getRcsToken` 上 —— **另一个主机、另一个活动**，
// 响应的 `header.appId` 是 `3099801`（本族实拍是 `12345681`，见
// `app_cloud_phone.go` 的 `CloudPhoneAppID`）。它们的 body 是
// `{targetSourceId, token}`，`targetSourceId` 取 `"001005"` 或 `"0"`。
// ⇒ 那两条**不能**当本族的样本用：键名看着像，域却不是一回事；
// 本族这一枪的 body 今天**没人见过**。`targetSourceId` 只作为"改这一行时先看这里"
// 的线索记在下面那个常量旁边，**没有**写进任何请求。
//
// # 信封、鉴权、`version`
//
// 都在 [`app_cloud_phone.go`]（app_cloud_phone.go），本文件不重复。
// 三条口都走 [CloudPhoneRequest]（body 带 `version`）—— ⚠ 这是按 §2.4 第 370 行
// 那个统一形状**推**的，实拍支持它的是 `redpacket/` 那四条，不含本文件任何一条。
package upstream

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
)

// 三条口的路径。⚠ **零样本 ⇒ 前缀全是推的**，处置与
// `cloudPhoneUserSignPath` 那三行同一处（404 时改这一行，别改头、别改 body）：
//
//   - `redpacket/` 那一段：实拍里 `user*` / `config*` 这一类动作口都挂它下面
//     （`redpacket/userToastInfo`、`redpacket/configTaskLoginList`），答题两条同属一类。
//   - `user/` 那一段：本族唯一实拍的 `user/tokenValidate`（第 29 条）挂它下面，
//     而**别的域**那两条 `getRcsToken` 也叫 `user/getRcsToken` —— 两处线索指向同一段，
//     但那是另一个主机的证据，**不足以当成核实过**。
const (
	cloudPhoneConfigTopicListPath = "redpacket/configTopicList"
	cloudPhoneUserTopicAnswerPath = "redpacket/userTopicAnswer"
	cloudPhoneGetRcsTokenPath     = "user/getRcsToken"
)

// ─────────────────────────────────────────────────────────────────────────────
// 答题类：configTopicList → userTopicAnswer
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneTopicList 是 `configTopicList` 的题目列表，**每项交原文**。
//
// # 为什么不声明字段
//
// 成文对这一口只说了两件事：它是一张列表、旧实现取 `[0]`。
// 题干在哪一格、选项是数组还是 `optionA`…`optionD` 四个键、题目有没有 id
// —— 一个都不知道。声明成固定字段表的话，读不到就是 nil，
// 而 nil 在下一层长成的样子是"这道题没有内容"⇒ 于是答了一个它自己也没把握的选项。
// 交原文至少不会假装知道有什么键。
//
// ⚠ 为什么是**切片类型**而不是结构体：这一口唯一确定的产物就是"一串题目"，
// 多包一层结构体只会塞进一个没人读的字段。唯一想留的第二项信息（列表取自哪一层）
// 放进了 [ParseCloudPhoneTopicList] 的第二个返回值，理由和安装类那次一样：
// "列表为空"与"取错了层"在只交列表的版本里长得一模一样。
type CloudPhoneTopicList []map[string]any

// ParseCloudPhoneTopicList 判信封并交出题目列表；第二个返回值报告列表
// 是不是从 `data.data` 那一层取到的（成文本族另一条口点名要兼容的双层嵌套；
// ⚠ 这一口有没有那层嵌套**未验证**，留这一位是为了"空列表"与"取错层"分得开）。
//
// # 取列表的顺序（两种都试，先浅后深）
//
//  1. `data.list` / `data.records` / `data.topics` —— ⚠ 三个键名**全部零样本**，
//     列在这里只因为"总得有一处给人改"。真跑出空列表时改这一行，
//     不要回来改顺序，也不要因此去答一个空题目。
//  2. `data.data` 是数组。
//
// 都拿不到 ⇒ 返回 nil 列表、不报错（信封过了但读不出列表，是形状未知，不是失败）。
//
// ⚠ 与安装类那处同样的读不到的形状：`data` 本身直接是数组时会被取段压成 nil
// （[EnsureCloudPhoneOK] 只看 `header.status`）⇒ 真要支持得先改取段那一层。
func ParseCloudPhoneTopicList(envelope *Envelope) (CloudPhoneTopicList, bool, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return nil, false, perr
	}
	var raw []any
	nested := false
	for _, key := range []string{"list", "records", "topics"} {
		if list, ok := data[key].([]any); ok {
			raw = list
			break
		}
	}
	if raw == nil {
		if inner, ok := data["data"].([]any); ok {
			raw, nested = inner, true
		}
	}
	topics := make(CloudPhoneTopicList, 0, len(raw))
	for _, item := range raw {
		if obj, ok := item.(map[string]any); ok {
			topics = append(topics, obj)
			continue
		}
		// 列表里出现了非对象的一项：⚠ **不静默吞掉**。整份列表判为读不出
		// （交 nil），因为"跳过一项继续"在这里的表现是答错题。
		return nil, false, core.NewParse("红包派对题目列表里有一项不是对象，形状不认识")
	}
	if len(topics) == 0 {
		return nil, nested, nil
	}
	return topics, nested, nil
}

// CloudPhoneTopicAnswer 是一道题的答案选项。**上游字面量**，不是我们的枚举名。
//
// 取值 `"A"|"B"|"C"|"D"` 有成文出处（§2.4 第 376 行逐字写了
// `userTopicAnswer {answer:"A"|"B"|"C"|"D"}`），和本族另一条口
// `userBrowse {taskCode}` 是同一证据等级 —— 这是成文给过的最后两个键名/取值。
//
// ⚠ 但**大写**这一件仍然只是照抄那一句：实拍 0 条 ⇒ 上游会不会接受小写、
// 有没有 `E`（多选题会不会出现），都不知道。⇒ [CloudPhoneTopicAnswer.Valid]
// 只认那四个，别把"没认出来"当成"上游也不认"。
type CloudPhoneTopicAnswer string

// 四个选项。⚠ 值是**大写**上游字面量。
const (
	CloudPhoneTopicAnswerA CloudPhoneTopicAnswer = "A"
	CloudPhoneTopicAnswerB CloudPhoneTopicAnswer = "B"
	CloudPhoneTopicAnswerC CloudPhoneTopicAnswer = "C"
	CloudPhoneTopicAnswerD CloudPhoneTopicAnswer = "D"
)

// String 返回上游字面量（拼 body 用）。
func (a CloudPhoneTopicAnswer) String() string { return string(a) }

// Valid 报告这一格是不是那四个选项之一。
//
// ⚠ 空值**不合法**：成文的旧实现是"题目不在库里就重试"，
// 也就是说"答不上来"是一个真实存在的状态。把它折成一个默认选项（比如永远答 `A`）
// 是本层最不该做的事 —— 那等于每次都用一个没依据的选项去赌。
func (a CloudPhoneTopicAnswer) Valid() bool {
	switch a {
	case CloudPhoneTopicAnswerA, CloudPhoneTopicAnswerB, CloudPhoneTopicAnswerC, CloudPhoneTopicAnswerD:
		return true
	default:
		return false
	}
}

// CloudPhoneUserTopicAnswerRequest 构造"提交答案"那一枪。
//
// ⚠ **零样本**：夹具命中 0。`answer` 这个键名与四个取值有成文出处（见上），
// 但这句成文里**只有答案、没有题目标识** ⇒ 一个只带 `answer` 的 body
// 说不清答的是哪一道题。这正是下面两处成文记下来的那类缺陷的形状：
// `docs/caiyun-go-feature-inventory.md` 第 72 行（表内编号 160）与第 224 行、
// `docs/caiyun-go-parameters-and-decisions.md` 第 166 行（表 T-4）都写着
// "`userSign` 收了 `taskID` 但 body 是空的，taskID 从未进请求体
// ⇒ 签到永远打在服务端默认项"，处置是**先修再搬**。
// ⇒ 所以这一枪多接一个 `data`：把题目标识**补进 `data`** 再发，
// 键名由接线方按新抓的样本填。⚠ 猜一个键名的代价是静默 ——
// 上游照样回 `"200"`，答错的却是服务端那道默认题。
//
// `data` 为 nil 就是成文那一条 body（`{"answer":"A","version":…}`）。
// ⚠ 传进来的 map **不会被改**（`answer` 写在副本上）：调用方常常拿同一份
// 上下文重试 15 次（成文记的那个 `15×150ms`），共用 map 会让上一轮的键留在下一轮里。
func CloudPhoneUserTopicAnswerRequest(
	auth CloudPhoneAuth, referer string, answer CloudPhoneTopicAnswer, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	if !answer.Valid() {
		return core.HTTPRequest{}, core.NewParse("红包派对答案不认识：只认 A/B/C/D，空值也不行")
	}
	body := make(map[string]any, len(data)+1)
	for k, v := range data {
		body[k] = v
	}
	body["answer"] = answer.String()
	return CloudPhoneRequest(cloudPhoneUserTopicAnswerPath, body, auth, referer)
}

// ─────────────────────────────────────────────────────────────────────────────
// DAILY_1：getRcsToken
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneGetRcsTokenRequest 构造 `getRcsToken` 那一枪。
//
// ⚠ **零样本**。成文 §2.4 第 377 行对这一口说的**全是用法**：它在 `DAILY_1`
// 分支里被调一次、**返回值丢弃**、然后打一句"需手动完成（需云机内实际使用）"。
// 也就是说：连旧实现都没读这个返回。⇒ body 键名一个都不知 ⇒ 交回调用方。
//
// ⚠ 旁边那条**别的域**的线索（文件头有完整交代）：`cloud.139.com/uniphone`
// 上那两条同名的口 body 是 `{targetSourceId, token}`，`targetSourceId` 取
// `"001005"` / `"0"`。这一族的 `targetSourceId` 应该也不是同一个值
// （本族的 sourceId 实拍是 `001216`，见 `CloudPhoneSpecTokenSourceID`）——
// **两句都是推的**，所以这里既不写键名也不写值。
func CloudPhoneGetRcsTokenRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneGetRcsTokenPath, data, auth, referer)
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppCloudPhoneConfigTopicList 取题目列表。**只读**。
//
// 返回的 `nested` 见 [ParseCloudPhoneTopicList]。列表为空**不是**错误（见同处的解释）。
func (u *Upstream) AppCloudPhoneConfigTopicList(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneTopicList, bool, *core.Error) {
	request, perr := CloudPhoneRequest(cloudPhoneConfigTopicListPath, data, auth, referer)
	if perr != nil {
		return nil, false, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, false, perr
	}
	return ParseCloudPhoneTopicList(envelope)
}

// AppCloudPhoneUserTopicAnswer 提交一道题的答案。**写请求**。
//
// ⚠ 返回值只交 `data` 原文：成文没说这一口的响应里有什么，
// 而**答对没有**是下一轮重试的唯一依据 ⇒ 判据归编排层，
// 本层不替它把"信封过了"读成"答对了"（那是同一族的宽松默认，见
// `app_cloud_phone_task.go` 的 `CloudPhoneCompleteTask`）。
func (u *Upstream) AppCloudPhoneUserTopicAnswer(
	ctx context.Context, auth CloudPhoneAuth, referer string,
	answer CloudPhoneTopicAnswer, data map[string]any,
) (CloudPhoneData, *core.Error) {
	request, perr := CloudPhoneUserTopicAnswerRequest(auth, referer, answer, data)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseCloudPhoneData(envelope)
}

// AppCloudPhoneGetRcsToken 打 `DAILY_1` 那一枪。**零样本载荷**。
//
// ⚠⚠ **旧实现在这里撒过谎，本层不接这个锅**：成文 §2.4 第 377 行记的是
// 调完这一枪就 `return true`（当成任务完成），而它自己把返回值丢了、
// 也承认这件事"需云机内实际使用"。§1.4 把这一条列进了**假成功**清单。
// ⇒ 本层的口径：这一枪**打过**不等于任务**完成过**。
// 返回 `data` 原文（可能是凭据，别进日志），完成与否由拿到样本的人判。
func (u *Upstream) AppCloudPhoneGetRcsToken(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneData, *core.Error) {
	request, perr := CloudPhoneGetRcsTokenRequest(auth, referer, data)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseCloudPhoneData(envelope)
}
