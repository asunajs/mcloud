package upstream

import (
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/golden"
)

// liveRoomVectors 是 test/golden/live-room-vectors.json 的形状。
//
// ⚠ 这份向量与别的几份**生成方式不同**：不是抓包值，而是**原项目实现跑出来的值**
// （`scripts/gen-live-room-vectors.cjs` 用 protobufjs + node:crypto 算）。
// 理由见该脚本的文件头：MSync 的 wire 字节、两个签名头、口令提取这三段
// 在 2026-09-22 那轮抓包里**都拿不到**（未开播 ⇒ `chatRoomId: null` ⇒ WS 结构性不存在），
// 所以对拍基准只能是原实现。
type liveRoomVectors struct {
	Constants struct {
		AppKey         string   `json:"APP_KEY"`
		LiveAppKey     string   `json:"LIVE_APP_KEY"`
		Domain         string   `json:"DOMAIN"`
		DeviceInfoHex  string   `json:"DEVICE_INFO_HEX"`
		UserAgent      string   `json:"USER_AGENT"`
		AccessToken    string   `json:"ACCESS_TOKEN"`
		Phone          string   `json:"PHONE"`
		RoomID         string   `json:"ROOM_ID"`
		ProtoTypesUsed []string `json:"protoTypesUsed"`
	} `json:"constants"`
	MSyncFrames  []msyncFrameVector `json:"msyncFrames"`
	MSyncInbound []struct {
		Name          string `json:"name"`
		Kind          string `json:"kind"`
		Text          string `json:"text"`
		PayloadBase64 string `json:"payloadBase64"`
		PayloadHex    string `json:"payloadHex"`
	} `json:"msyncInbound"`
	MemberApplySignatures []struct {
		Name              string `json:"name"`
		Timestamp         string `json:"timestamp"`
		AccessToken       string `json:"accessToken"`
		DeviceInfoHex     string `json:"deviceInfoHex"`
		TimestampSuffix10 string `json:"timestampSuffix10"`
		SystemMD5         string `json:"systemMd5"`
		TokenMD5          string `json:"tokenMd5"`
	} `json:"memberApplySignatures"`
	KoulingExtract []struct {
		Name   string   `json:"name"`
		Text   string   `json:"text"`
		Expect []string `json:"expect"`
	} `json:"koulingExtract"`
	XiaohongshuExtract []struct {
		Name   string   `json:"name"`
		HTML   string   `json:"html"`
		Expect []string `json:"expect"`
	} `json:"xiaohongshuExtract"`
}

// msyncFrameVector 是 `msyncFrames` 的一条。
//
// 提成具名类型而不是内联匿名结构：`provision` 那一条要单独被第二个用例取出来
// （它的 payload 是唯一非空的那份），内联的话两个用例得各写一遍字段表，
// 改一个字段就有一次漏改的机会。
type msyncFrameVector struct {
	Name              string `json:"name"`
	Command           int    `json:"command"`
	TimestampMillis   int64  `json:"timestampMillis"`
	IdentificationHex string `json:"identificationHex"`
	Resource          string `json:"resource"`
	DeviceUUID        string `json:"deviceUuid"`
	DeviceInfoHex     string `json:"deviceInfoHex"`
	Base64            string `json:"base64"`
	Hex               string `json:"hex"`
	PayloadHex        string `json:"payloadHex"`
	Expect            struct {
		Version   int    `json:"version"`
		Auth      string `json:"auth"`
		UserAgent string `json:"userAgent"`
		Command   int    `json:"command"`
		// ⚠ DeviceID **没有**断言，是有意的：原实现把 `deviceId` 硬编码成 0
		// （`proto.ts` 里 `deviceId: 0`），proto3 默认值 ⇒ **整条不写**；
		// Go 侧因此**没有**这个字段。这条"不写"由
		// [TestLiveMSyncEncodesToOriginalBytes] 的**逐字节**比较覆盖
		// （多写一个字节就不等），不需要、也无法在解码侧再断一次。
		// 同一个理由适用于向量里的 `crypto`（也是恒 0，Go 侧连声明都没有）。
		DeviceID          int    `json:"deviceId"`
		EncryptType       []int  `json:"encryptType"`
		DeviceInfoHex     string `json:"deviceInfoHex"`
		IdentificationHex string `json:"identificationHex"`
		GUID              struct {
			AppKey         string `json:"appKey"`
			Name           string `json:"name"`
			Domain         string `json:"domain"`
			ClientResource string `json:"clientResource"`
		} `json:"guid"`
	} `json:"expect"`
	ExpectProvision struct {
		OSType       int    `json:"osType"`
		Version      string `json:"version"`
		NetworkType  int    `json:"networkType"`
		AppSign      string `json:"appSign"`
		CompressType []int  `json:"compressType"`
		EncryptType  []int  `json:"encryptType"`
		DeviceUUID   string `json:"deviceUuid"`
		DeviceName   string `json:"deviceName"`
		Resource     string `json:"resource"`
		Auth         string `json:"auth"`
		ProvinceCode string `json:"provinceCode"`
	} `json:"expectProvision"`
}

func loadLiveRoomVectors(t *testing.T) liveRoomVectors {
	t.Helper()
	raw, err := golden.Load("live-room-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	var out liveRoomVectors
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("黄金向量不是合法 JSON: %v", err)
	}
	if len(out.MSyncFrames) == 0 || len(out.KoulingExtract) == 0 {
		t.Fatal("黄金向量缺段：msyncFrames / koulingExtract 至少各要有一条")
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 生产常量 ↔ 向量 `constants` 段
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomConstantsMatchVectors 把**生产常量**显式钉到向量的 `constants` 段上。
//
// 为什么需要单独一条 —— `constants` 段看着"已经被覆盖了"，其实只覆盖了一半：
//
//   - **间接钉住的**：`LiveAppKey` / `SDKAppKey` / `LiveDomain` / `LiveMSyncUserAgent`
//     （`TestLiveMSyncEncodesToOriginalBytes` 用它们拼报文再比字节 ⇒ 改错就红）、
//     `Provision*` / `LiveIdentificationPrefix`（同一条链 / provision payload 那条）。
//   - ⚠⚠ **没人钉的**：`LiveDeviceInfoHex` —— 编码用例取的是**向量里的**
//     `frame.DeviceInfoHex`，**不是**这个常量。于是常量改了**没有任何用例会红**，
//     而它**真的在生产路径上**（`live_room_drive.go` 算 member/apply 的 `systemMd5`、
//     `live_msync.go` 填 `MSync.deviceInfo`）。
//     ⇒ 这正是"看着有覆盖、其实没有"的那一类，只能**显式**钉。
//
// ⚠ 它同时钉住"48 字节"这个数：文件里两处曾把这个常量写成 40 字节，
// 而"40 字节"读起来同样合理（见 [LiveDeviceInfoHex] 的注释）。
//
// 反向验证：把 `LiveDeviceInfoHex` 末尾改一个字符（或把 `LiveDomain` 改成
// `cmosim.cn`），本用例红。
func TestLiveRoomConstantsMatchVectors(t *testing.T) {
	vectors := loadLiveRoomVectors(t)
	c := vectors.Constants

	checks := []struct {
		name string
		got  string
		want string
	}{
		{"APP_KEY", SDKAppKey, c.AppKey},
		{"LIVE_APP_KEY", LiveAppKey, c.LiveAppKey},
		{"DOMAIN", LiveDomain, c.Domain},
		{"USER_AGENT", LiveMSyncUserAgent, c.UserAgent},
		{"DEVICE_INFO_HEX", LiveDeviceInfoHex, c.DeviceInfoHex},
	}
	for _, chk := range checks {
		if chk.want == "" {
			t.Errorf("向量 constants 缺 %s —— 段名对不上时本用例会静默变成空比较", chk.name)
			continue
		}
		if chk.got != chk.want {
			t.Errorf("%s 与黄金向量不一致：\n  Go   = %q\n  向量 = %q", chk.name, chk.got, chk.want)
		}
	}

	// 设备串的**字节数**：96 个 hex 字符 = 48 字节（不是 40）。
	if got := len(LiveDeviceInfo()); got != 48 {
		t.Errorf("deviceInfo 期望 48 字节，实得 %d", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// MSync：**逐字节**对拍
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveMSyncEncodesToOriginalBytes 是这条链上最硬的一条断言。
//
// 判据是**字节相等**（不是"解回来对得上"）—— 理由见 live_msync.go 文件头：
// 漏写 packed、多写默认值、字段顺序不对，这三类错**都能被 protobufjs 解开**，
// 只有逐字节比才看得见。
//
// 反向验证：把 [appendPackedUint32] 换成 [appendScalar]（wire type 0），
// 或把 `EncodeMSync` 里 `EncryptType` 那一行删掉，本用例立刻红。
func TestLiveMSyncEncodesToOriginalBytes(t *testing.T) {
	vectors := loadLiveRoomVectors(t)
	c := vectors.Constants

	for _, frame := range vectors.MSyncFrames {
		t.Run(frame.Name, func(t *testing.T) {
			// identification = 前缀（ASCII，不是 hex 字面量）+ 16 随机字节。
			// 这里把**前缀按字节**拼在 hex 串前面，而不是把前缀当 hex 解 ——
			// 后者会把 `cmos_im` 当成非法 hex 直接报错（它本来也不是 hex）。
			identification, err := hex.DecodeString(hex.EncodeToString([]byte(LiveIdentificationPrefix)) + frame.IdentificationHex)
			if err != nil {
				t.Fatalf("identification 不是合法 hex: %v", err)
			}
			deviceInfo, err := hex.DecodeString(frame.DeviceInfoHex)
			if err != nil {
				t.Fatalf("deviceInfo 不是合法 hex: %v", err)
			}
			phone := c.Phone
			if frame.Name == "sync-no-phone" {
				phone = ""
			}
			guidName := "cmlive_c_" + phone
			appKey := LiveAppKey
			if phone == "" {
				guidName = "cmlive_" + c.RoomID
				appKey = SDKAppKey
			}
			payload, err := hex.DecodeString(frame.PayloadHex)
			if err != nil {
				t.Fatalf("payload 不是合法 hex: %v", err)
			}

			got := EncodeMSync(MSyncMessage{
				Version:   0,
				GUID:      JID{AppKey: appKey, Name: guidName, Domain: LiveDomain, ClientResource: frame.Resource},
				Auth:      "$t$" + c.AccessToken,
				UserAgent: LiveMSyncUserAgent,
				Command:   frame.Command,
				Payload:   payload,
				// ⚠ 恒为 [0]：**值为 0 也要写**（packed repeated 不适用 proto3 的默认值省略）。
				EncryptType: []uint32{0},
				DeviceInfo:  deviceInfo,
				Timestamp:   LiveTimestampBytes(frame.TimestampMillis),
				// ⚠⚠ 恒为**非 nil 的 0**：原实现给 `BigInt(0)`，protobufjs 当 `Long`
				// 对象处理 ⇒ 零值也写（`38 00`）。给 nil 的话这里会少 2 字节，
				// 而 protobufjs 两边都解得开 —— 只有逐字节比才看得见。
				Pov:            new(uint64),
				Identification: identification,
			})

			if gotHex := hex.EncodeToString(got); gotHex != frame.Hex {
				t.Fatalf("MSync 字节与原实现不一致\n期望 %s\n实得 %s", frame.Hex, gotHex)
			}
		})
	}
}

// TestLiveMSyncProvisionPayloadMatchesOriginal 单独钉 PROVISION 的 payload。
//
// 它是**唯一**带 payload 的那一帧，而 payload 里有两处 repeated（compressType /
// encryptType）与一处"空串不写"（province_code）—— 三个容易错的点都在这一小段里。
func TestLiveMSyncProvisionPayloadMatchesOriginal(t *testing.T) {
	vectors := loadLiveRoomVectors(t)
	c := vectors.Constants

	// 直接在切片上取，不复制一份匿名结构 —— 复制只会制造"两份字段列表要同时改"的机会。
	var frame *msyncFrameVector
	for i := range vectors.MSyncFrames {
		if vectors.MSyncFrames[i].Name == "provision" {
			frame = &vectors.MSyncFrames[i]
			break
		}
	}
	if frame == nil {
		t.Fatal("黄金向量里没有 provision 帧")
	}

	got := EncodeProvision(Provision{
		OSType:       ProvisionOSTypeJS,
		Version:      ProvisionVersion,
		NetworkType:  ProvisionNetwork,
		AppSign:      ProvisionAppSign,
		CompressType: []uint32{0},
		EncryptType:  []uint32{0},
		DeviceUUID:   frame.DeviceUUID,
		DeviceName:   ProvisionDeviceName,
		Resource:     frame.Resource,
		Auth:         "$t$" + c.AccessToken,
		ProvinceCode: "",
	})
	if gotHex := hex.EncodeToString(got); gotHex != frame.PayloadHex {
		t.Fatalf("Provision 字节与原实现不一致\n期望 %s\n实得 %s", frame.PayloadHex, gotHex)
	}

	// 再解一遍：编码对不代表**解码**对（两个方向是两套代码）。
	decoded, err := DecodeProvision(got)
	if err != nil {
		t.Fatalf("DecodeProvision 失败: %v", err)
	}
	expect := frame.ExpectProvision
	if int(decoded.OSType) != expect.OSType || decoded.Version != expect.Version ||
		int(decoded.NetworkType) != expect.NetworkType || decoded.AppSign != expect.AppSign ||
		decoded.DeviceUUID != expect.DeviceUUID || decoded.DeviceName != expect.DeviceName ||
		decoded.Resource != expect.Resource || decoded.Auth != expect.Auth ||
		decoded.ProvinceCode != expect.ProvinceCode {
		t.Fatalf("DecodeProvision 与期望字段不一致: %+v", decoded)
	}
	if len(decoded.CompressType) != 1 || decoded.CompressType[0] != 0 {
		t.Fatalf("compressType 期望 [0]，实得 %v", decoded.CompressType)
	}
	if len(decoded.EncryptType) != 1 || decoded.EncryptType[0] != 0 {
		t.Fatalf("encryptType 期望 [0]，实得 %v", decoded.EncryptType)
	}
}

// TestLiveMSyncDecodesOriginalFrames 用原实现的字节喂**我们自己的**解码器。
func TestLiveMSyncDecodesOriginalFrames(t *testing.T) {
	vectors := loadLiveRoomVectors(t)

	for _, frame := range vectors.MSyncFrames {
		t.Run(frame.Name, func(t *testing.T) {
			raw, err := base64.StdEncoding.DecodeString(frame.Base64)
			if err != nil {
				t.Fatalf("黄金向量 base64 解不开: %v", err)
			}
			got, err := DecodeMSync(raw)
			if err != nil {
				t.Fatalf("DecodeMSync 失败: %v", err)
			}
			expect := frame.Expect
			if got.Version != expect.Version {
				t.Errorf("version 期望 %d，实得 %d", expect.Version, got.Version)
			}
			if got.GUID.AppKey != expect.GUID.AppKey || got.GUID.Name != expect.GUID.Name ||
				got.GUID.Domain != expect.GUID.Domain || got.GUID.ClientResource != expect.GUID.ClientResource {
				t.Errorf("guid 期望 %+v，实得 %+v", expect.GUID, got.GUID)
			}
			if got.Auth != expect.Auth {
				t.Errorf("auth 期望 %q，实得 %q", expect.Auth, got.Auth)
			}
			if got.UserAgent != expect.UserAgent {
				t.Errorf("userAgent 期望 %q，实得 %q", expect.UserAgent, got.UserAgent)
			}
			if got.Command != expect.Command {
				t.Errorf("command 期望 %d，实得 %d", expect.Command, got.Command)
			}
			if hex.EncodeToString(got.DeviceInfo) != expect.DeviceInfoHex {
				t.Errorf("deviceInfo 期望 %s，实得 %s", expect.DeviceInfoHex, hex.EncodeToString(got.DeviceInfo))
			}
			if hex.EncodeToString(got.Identification) != expect.IdentificationHex {
				t.Errorf("identification 期望 %s，实得 %s",
					expect.IdentificationHex, hex.EncodeToString(got.Identification))
			}
			// pov 是"零值也被写出来"的那一项：解出来必须是**非 nil 的 0**。
			if got.Pov == nil {
				t.Error("pov 丢了 —— 原实现的报文里有 `38 00`")
			} else if *got.Pov != 0 {
				t.Errorf("pov 期望 0，实得 %d", *got.Pov)
			}
			if len(got.EncryptType) != len(expect.EncryptType) {
				t.Fatalf("encryptType 期望 %v，实得 %v", expect.EncryptType, got.EncryptType)
			}
			for i := range expect.EncryptType {
				if int(got.EncryptType[i]) != expect.EncryptType[i] {
					t.Errorf("encryptType[%d] 期望 %d，实得 %d", i, expect.EncryptType[i], got.EncryptType[i])
				}
			}
			if len(got.Payload) == 0 && frame.PayloadHex != "" {
				t.Error("payload 丢了")
			}
		})
	}
}

// TestLiveMSyncInboundTextExtraction 钉"三级兜底"里的每一级。
//
// 三种形态（CommSyncUL 包装 / NoticePayload 包装 / 裸 MessageBody）**各一条**：
// 少写任何一级兜底，对应的那一条会红 —— 而它红的意义是
// "服务端换个包装我们就收不到弹幕了"，那正是原实现补兜底时踩过的坑。
func TestLiveMSyncInboundTextExtraction(t *testing.T) {
	vectors := loadLiveRoomVectors(t)

	for _, item := range vectors.MSyncInbound {
		t.Run(item.Name, func(t *testing.T) {
			payload, err := base64.StdEncoding.DecodeString(item.PayloadBase64)
			if err != nil {
				t.Fatalf("payload base64 解不开: %v", err)
			}
			command := CmdSync
			if item.Kind == "notice" {
				command = CmdNotice
			}
			got, err := ExtractDanmakuText(command, payload)
			if err != nil {
				t.Fatalf("ExtractDanmakuText 失败: %v", err)
			}
			if got != item.Text {
				t.Fatalf("弹幕文本期望 %q，实得 %q", item.Text, got)
			}
			// 提取出来的文本必须能被口令提取器吃下 —— 两段是串起来的。
			if codes := ExtractKouling(got); len(codes) == 0 {
				t.Fatalf("从 %q 里解不出任何口令", got)
			}
		})
	}
}

// TestExtractDanmakuTextSkipsControlFrames 钉"控制帧不算异常"。
//
// 反向验证：把 `command != CmdSync && command != CmdNotice` 那个判断删掉，
// 心跳帧会被当 SYNC 去解，症状是"每一帧都报一次解码失败"。
func TestExtractDanmakuTextSkipsControlFrames(t *testing.T) {
	for _, command := range []int{CmdUnread, CmdProvision, 5, 6, 7} {
		got, err := ExtractDanmakuText(command, []byte{0x0a, 0x01, 0x41})
		if err != nil {
			t.Fatalf("command=%d 不该报错: %v", command, err)
		}
		if got != "" {
			t.Fatalf("command=%d 期望空文本，实得 %q", command, got)
		}
	}
	// 空 payload 也不该报错。
	if got, err := ExtractDanmakuText(CmdSync, nil); err != nil || got != "" {
		t.Fatalf("空 payload 期望 (\"\", nil)，实得 (%q, %v)", got, err)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// member/apply 的两个签名头
// ─────────────────────────────────────────────────────────────────────────────

// TestMemberApplySignaturesMatchOriginal 对拍原实现算出的两个 md5。
//
// 反向验证：把 [TokenMD5] 里的时间戳位置从"第 10 个字符之后"改成末尾，
// 三条用例**全红** —— 这正是那个形状唯一可被钉住的地方。
func TestMemberApplySignaturesMatchOriginal(t *testing.T) {
	vectors := loadLiveRoomVectors(t)

	for _, item := range vectors.MemberApplySignatures {
		t.Run(item.Name, func(t *testing.T) {
			if got := TimestampSuffix10(item.Timestamp); got != item.TimestampSuffix10 {
				t.Fatalf("末 10 位期望 %q，实得 %q", item.TimestampSuffix10, got)
			}
			systemMD5, err := SystemMD5(item.DeviceInfoHex, item.Timestamp)
			if err != nil {
				t.Fatalf("SystemMD5 失败: %v", err)
			}
			if systemMD5 != item.SystemMD5 {
				t.Errorf("systemMd5 期望 %s，实得 %s", item.SystemMD5, systemMD5)
			}
			if got := TokenMD5(item.AccessToken, item.Timestamp); got != item.TokenMD5 {
				t.Errorf("tokenMd5 期望 %s，实得 %s", item.TokenMD5, got)
			}
		})
	}
}

// TestSystemMD5UsesDecodedBytesNotHex 钉住"第一段是 48 字节而不是 96 个 hex 字符"。
//
// 这一条只能靠**与 hex 形态算出不同结果**来证明：如果实现里误用了字符串，
// 那么"用 hex 串"与"用字节"会得到同一个值 —— 而它们本来必须不同。
func TestSystemMD5UsesDecodedBytesNotHex(t *testing.T) {
	fromBytes, err := SystemMD5(LiveDeviceInfoHex, "1784790184674")
	if err != nil {
		t.Fatalf("SystemMD5 失败: %v", err)
	}
	fromHexString := core.MD5Hex([]byte(LiveDeviceInfoHex + "4790184674"))
	if fromBytes == fromHexString {
		t.Fatal("用解码字节与用 hex 串算出了同一个值 —— 说明没有解码")
	}
}

// TestSystemMD5RejectsBadHex 钉住"坏 hex 当场报错，而不是算一个别的签名出来"。
func TestSystemMD5RejectsBadHex(t *testing.T) {
	if _, err := SystemMD5("zzzz", "1784790184674"); err == nil {
		t.Fatal("非法 hex 应当报错")
	}
}

// TestTokenMD5ShortTokenDoesNotPanic 钉住短 token 的边界。
//
// JS 的 `slice(0,10)` / `slice(10)` 对短串分别是"整串"与"空串"；
// Go 的切片会 panic。黄金向量里那条 `short-token` 就是为了钉这个边界。
func TestTokenMD5ShortTokenDoesNotPanic(t *testing.T) {
	vectors := loadLiveRoomVectors(t)
	found := false
	for _, item := range vectors.MemberApplySignatures {
		if item.Name != "short-token" {
			continue
		}
		found = true
		if got := TokenMD5(item.AccessToken, item.Timestamp); got != item.TokenMD5 {
			t.Fatalf("短 token 的 tokenMd5 期望 %s，实得 %s", item.TokenMD5, got)
		}
	}
	if !found {
		t.Fatal("黄金向量里没有 short-token 那条用例")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口令提取
// ─────────────────────────────────────────────────────────────────────────────

// TestExtractKoulingMatchesOriginal 对拍 34 条口令用例。
//
// 输入全部有据可依：14 条逐字搬自原项目 `__tests__/kouling.test.ts`，
// 其余来自 2026-09-17 实拍弹幕（`test/fixtures/http/直播wss.json` 的 38 条唯一文本）。
//
// 反向验证：
//   - 把 [jsSpaceClass] 换成 Go 的 `\s` ⇒ `live-ideographic-space` 红；
//   - 把 [IsQuestion] 的疑问句过滤删掉 ⇒ 10 条 `live-question-*` 红；
//   - 把格式 4 的 `len(result) == 0` 前提去掉 ⇒ `live-real-*` 那几条红。
func TestExtractKoulingMatchesOriginal(t *testing.T) {
	vectors := loadLiveRoomVectors(t)

	for _, item := range vectors.KoulingExtract {
		t.Run(item.Name, func(t *testing.T) {
			got := ExtractKouling(item.Text)
			if !equalStrings(got, item.Expect) {
				t.Fatalf("文本 %q\n期望 %v\n实得 %v", item.Text, item.Expect, got)
			}
		})
	}
}

// TestExtractKoulingFromXiaohongshuMatchesOriginal 对拍小红书主页抽取。
func TestExtractKoulingFromXiaohongshuMatchesOriginal(t *testing.T) {
	vectors := loadLiveRoomVectors(t)

	for _, item := range vectors.XiaohongshuExtract {
		t.Run(item.Name, func(t *testing.T) {
			got := ExtractKoulingFromXiaohongshuHTML(item.HTML)
			if !equalStrings(got, item.Expect) {
				t.Fatalf("HTML %q\n期望 %v\n实得 %v", item.HTML, item.Expect, got)
			}
		})
	}
}

// TestIsKoulingHeadDetectsContentOnNextFrame 钉跨帧拼接的**触发条件**。
//
// 反向验证：把 [koulingHeadOnlyRe] 的 `$` 锚点去掉，`口令1：AAA` 也会被
// 判成"头" ⇒ 紧接着的那条弹幕会被整条当口令收下（假阳性）。
func TestIsKoulingHeadDetectsContentOnNextFrame(t *testing.T) {
	heads := []string{"复制口令1：", "口令2:", "口令：", " 复制口令： "}
	for _, text := range heads {
		if !IsKoulingHead(text) {
			t.Errorf("%q 应当被判为口令头", text)
		}
	}
	notHeads := []string{"口令1：AAA", "口令是AAA", "口令是什么", "没有随机口令", ""}
	for _, text := range notHeads {
		if IsKoulingHead(text) {
			t.Errorf("%q 不该被判为口令头", text)
		}
	}
}

// TestExtractKoulingKeepsKnownFalsePositive 把"已知假阳性"钉成**行为契约**。
//
// `口令是相伴什么` 会解出 `相伴什么`（实拍弹幕里真有这一条）。这不是缺陷修复点：
// 修严它会连带把 `口令是带着云盘去上学` 这类**真口令**一起滤掉，
// 而真口令被滤掉的代价（错过整场）远大于假阳性（一次 200108 + 一次重听）。
//
// 这条测试存在的意义是：**谁哪天"顺手修好"了它，这里会红**，而红的时候
// 他会看到上面这段话。
func TestExtractKoulingKeepsKnownFalsePositive(t *testing.T) {
	got := ExtractKouling("口令是相伴什么")
	if !equalStrings(got, []string{"相伴什么"}) {
		t.Fatalf("已知假阳性变了：期望 [相伴什么]，实得 %v —— 若是有意收紧，"+
			"请同时确认 '口令是带着云盘去上学' 仍然解得出来", got)
	}
	if real := ExtractKouling("口令是带着云盘去上学"); !equalStrings(real, []string{"带着云盘去上学"}) {
		t.Fatalf("真口令被误伤：实得 %v", real)
	}
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
