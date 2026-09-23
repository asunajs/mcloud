// 直播 WebSocket 的 MSync 报文 —— **手写 proto3 编解码**。
//
// 对应原项目 `packages/core/src/live-room/proto.ts`（proto3 **内联字符串**、
// 无 `.proto` 文件、无 codegen，用 protobufjs 在运行时 parse）。
//
// # 为什么手写，而不引 protobuf 依赖
//
// ① 本仓库 Go 侧**零第三方新增**（AGENTS.md §3.4）；
// ② 真正要动的只有**三个**消息（MSync / Provision / 几个入站包装），
//
//	而 proto3 的 wire format 只有四种 wire type —— 引一整套 protobuf 运行时
//	换来的是一堆用不到的能力（反射、JSON 映射、descriptor）；
//
// ③ 手写之后**对拍面反而更硬**：可以逐字节比 protobufjs 的输出
//
//	（test/golden/live-room-vectors.json 的 `msyncFrames`，由原实现产出）。
//
// # 四条最容易写错、而且**错了不会报错**的规则
//
//  1. **singular 字段取默认值时不写**。proto3 的 `version = 0` / `crypto = 0` /
//     `deviceId = 0` / 空串 / 空 bytes 一律**整条不出现**。写上去不是"多几个字节"：
//     protobufjs 照样解得开，只有逐字节比才发现不一样。
//  2. ⚠⚠ **`repeated` 标量默认 packed**（wire type **2**，长度前缀 + 连写的 varint），
//     而不是"重复出现 wire type 0"。`encryptType = [0]` 的编码是 `5a 01 00`
//     —— **`repeated` 里的 0 要写**（它不适用第 1 条）。
//     实测：`live_room_vectors_test.go` 里那条逐字节断言就是为它准备的。
//  3. **字段按编号升序出现**。protobufjs 的 Writer 按字段定义顺序写，
//     而那份 schema 的字段号恰好是升序 —— 所以"升序"是它的实际输出形状。
//  4. ⚠⚠ **`pov` 是第 1 条的反例**：原实现写 `BigInt(0)`，protobufjs 把它当
//     `Long` **对象**（非 null）⇒ **零值也写**（`38 00`）。同一份报文里
//     `crypto: 0` / `deviceId: 0` 是普通 number，一律不写。
//     这一条是**逐字节对拍抓出来的**（第一版实现漏了它，弱断言全绿）。
//     见 [MSyncMessage.Pov]。
//
// # 来源与冻结基准
//
// 常量（`cmos#ngvlcs` / `cmosim.com` / 48 字节 `DEVICE_INFO` / userAgent）逐字取自
// 原项目 `proto.ts`；向量由 `scripts/gen-live-room-vectors.cjs` 用**原实现**跑出。
package upstream

import (
	"encoding/hex"
	"fmt"
	"strconv"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量（逐字对齐 proto.ts）
// ─────────────────────────────────────────────────────────────────────────────

const (
	// LiveAppKey 是**直播间**形态的 `guid.appKey`（带 `#`，与 `cmos_js_sdk` 不是一个值）。
	LiveAppKey = "cmos#ngvlcs"
	// SDKAppKey 是**非直播间**形态的 `guid.appKey`（原实现的另一条分支）。
	//
	// 两条分支的差别不只是 appKey：`guid.name` 也从 `cmlive_c_<phone>` 变成
	// `cmlive_<roomId>`。所以判断依据是**有没有 phone**，而不是"是不是在直播"——
	// 照抄原实现，别自己另立一条判据。
	SDKAppKey = "cmos_js_sdk"
	// LiveDomain 是 `guid.domain`。
	LiveDomain = "cmosim.com"
	// LiveMSyncUserAgent 是 `MSync.userAgent`（原项目硬编码的 iPad UA 前缀，**不是** HTTP 头）。
	//
	// ⚠ 它比 [LiveUserAgent]（HTTP 头那个）**短一截**：少了
	// ` (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN`。
	// 两个值长得像但不是一个 —— 互搬之后没有任何本地断言会变红。
	LiveMSyncUserAgent = "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15"
	// LiveDeviceInfoHex 是 `MSync.deviceInfo` 的 **48 字节**常量（96 个 hex 字符）。
	//
	// ⚠ 它是**应用级常量**：原项目把它硬编码在源码里，每个客户端发出去的都是同一串
	// —— 所以它不是设备指纹，也不该被"每台设备换一个"。照抄。
	// ⚠ 字节数别按"看起来多长"估：`len(hex)/2` = 96/2 = **48**。
	// 这个数在别处被写成过 40（同一个常量、同一个文件里两种写法），
	// 而"40 字节"读起来同样合理 ⇒ 只能数。
	LiveDeviceInfoHex = "830652084dda69b6b957e2c3a7bd9b18db65922e29051b1b342ccf15bebc0fdbcc96f61dadab12f3cab1e66cb28ec05b"
	// LiveIdentificationPrefix 是 `identification` 的固定前缀（原项目 `genIdentification`）。
	LiveIdentificationPrefix = "cmos_im"
)

// MSync 的 `Command` 枚举（proto.ts 逐字）。
const (
	// CmdSync 是服务端推弹幕用的命令（`SYNC`）。
	CmdSync = 0
	// CmdUnread 是**必须主动补发**的那一条：PROVISION 响应之后不发它，服务端不推弹幕。
	CmdUnread = 1
	// CmdNotice 是另一条服务端推送路径（`NOTICE`）。
	CmdNotice = 2
	// CmdProvision 是握手命令，也是**唯一**带 payload 的那一帧。
	CmdProvision = 3
)

// Provision 里的 `osType` / `networkType`（proto.ts 注释：`7 = OS_JS`、`1 = WIFI`）。
const (
	ProvisionOSTypeJS = 7
	ProvisionNetwork  = 1
	// ProvisionAppSign 是 `appSign`。
	ProvisionAppSign = "cmlive_cmos_js_sdk"
	// ProvisionVersion 是 `version`（**不带** `v` 前缀，与 HTTP 头的 `v23.0.2` 不同）。
	ProvisionVersion = "23.0.2"
	// ProvisionDeviceName 是 `deviceName`。
	ProvisionDeviceName = "webim"
)

// LiveDeviceInfo 返回 `deviceInfo` 的字节形态（**48 字节**）。
//
// ⚠ 它是**应用级常量**（不是设备指纹）：原项目把它硬编码在源码里，
// 每个客户端发出去的都是同一串，照抄即可。
//
// 每次调用都重新解码 hex 而不是缓存一份全局切片：返回的是**可变切片**，
// 共享一份等于让任何一个调用方都能改掉下一个请求的报文（同
// [core.HTTPRequest] 那条"必须拷贝"的理由）。
func LiveDeviceInfo() []byte {
	out, err := hex.DecodeString(LiveDeviceInfoHex)
	if err != nil {
		// 常量写错是**开发期错误**，不是运行时状况 —— 让它当场炸，
		// 而不是发一个 0 字节的 deviceInfo 出去（那会被服务端静默拒掉）。
		panic("upstream: LiveDeviceInfoHex 不是合法 hex: " + err.Error())
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 消息形状
// ─────────────────────────────────────────────────────────────────────────────

// JID 对应 `cmos.pb.JID`。
type JID struct {
	AppKey         string
	Name           string
	Domain         string
	ClientResource string
}

// MSyncMessage 对应 `cmos.pb.MSync`。
//
// 只声明**我们真的会读或写**的字段：`compressAlgorimth` / `encryptKey` / `srcIdc` /
// `dstIdc` / `tos` 在原实现里从不赋值，写出来只会让人以为它们在参与协议。
type MSyncMessage struct {
	// Version 是 `MSYNC_V1 = 0`；0 时**不写进报文**。
	Version int
	GUID    JID
	// Auth 是 `"$t$" + accessToken`（**不是**裸 token，前缀不能省）。
	Auth      string
	UserAgent string
	// Command 取 [CmdSync] / [CmdUnread] / [CmdNotice] / [CmdProvision]。
	Command int
	// Payload 只有 PROVISION 那一帧有（Provision 的编码字节）。
	Payload []byte
	// EncryptType 原实现恒为 `[0]`（`ENCRYPT_NONE`）。
	//
	// ⚠ 它是 repeated ⇒ **packed 编码**，而且**值为 0 也要写**（见文件头第 2 条）。
	EncryptType []uint32
	// DeviceInfo 是 [LiveDeviceInfo]（**48 字节**）。
	DeviceInfo []byte
	// Timestamp 是 **8 字节大端**毫秒（不是 protobuf 的 varint 时间戳）。
	Timestamp []byte
	// Identification 是 `cmos_im` + 16 随机字节。
	Identification []byte
	// Pov 对应 `uint64 pov = 7`。
	//
	// ⚠⚠ 这个字段是本文件里**唯一**需要指针的标量，理由值得记牢：
	// 原实现写的是 `pov: BigInt(0)`，而 protobufjs 把 `BigInt` 转成 `Long`
	// **对象** —— 对象非 null，于是"有值"判断成立，**零值也会被写出去**
	// （实测 `10 00`；同一份报文里的 `crypto: 0` / `deviceId: 0` 是普通 number，
	// 一律**不写**）。
	//
	// 这一条只有**逐字节**对拍才看得见：`38 00` 多一个少一个，
	// protobufjs 两边都解得开、字段也读得出 0。
	// 黄金向量的 `msyncFrames` 三条都钉着它。
	//
	// nil = 字段不出现；非 nil（含 0）= 写出去。生产路径用 [NewLiveMSync]，
	// 它已经把这一项填好。
	Pov *uint64
}

// Provision 对应 `cmos.pb.Provision`（PROVISION 那一帧的 payload）。
type Provision struct {
	OSType      uint32
	Version     string
	NetworkType uint32
	AppSign     string
	// CompressType 恒为 `[0]`（`COMPRESS_NONE`），同样 packed + 要写 0。
	CompressType []uint32
	EncryptType  []uint32
	DeviceUUID   string
	DeviceName   string
	Resource     string
	Auth         string
	// ProvinceCode 原实现给的是**空串** ⇒ 整个字段不出现。
	ProvinceCode string
}

// ─────────────────────────────────────────────────────────────────────────────
// proto3 wire 基础
// ─────────────────────────────────────────────────────────────────────────────

// proto3 的四种 wire type（3/4 是已废弃的 group，一律拒）。
const (
	wireVarint  = 0
	wireFixed64 = 1
	wireBytes   = 2
	wireFixed32 = 5
)

// protoField 是一个已扫出来的字段。
//
// 做成"先扫一遍、再按字段号分派"而不是每个消息手写一个循环：三个入站包装
// （CommSyncUL / NoticePayload / MessageBody）都要走同一套跳过逻辑，
// 各写一份的话，某一份漏处理一个 wire type 的表现是**静默解出空串** ——
// 而那正好和"这条弹幕没有文本"长得一模一样。
type protoField struct {
	num    int
	wire   int
	varint uint64
	raw    []byte
}

func readVarint(data []byte) (uint64, int, error) {
	var value uint64
	for i := 0; i < len(data); i++ {
		b := data[i]
		// 64 位最多 10 字节；第 10 字节只允许低 1 位有效。
		if i == 9 && b > 1 {
			return 0, 0, fmt.Errorf("protobuf varint 溢出（第 10 字节 = 0x%02x）", b)
		}
		value |= uint64(b&0x7f) << (7 * uint(i))
		if b&0x80 == 0 {
			return value, i + 1, nil
		}
	}
	return 0, 0, fmt.Errorf("protobuf varint 被截断")
}

func scanProtoFields(data []byte) ([]protoField, error) {
	fields := make([]protoField, 0, 16)
	for i := 0; i < len(data); {
		key, n, err := readVarint(data[i:])
		if err != nil {
			return nil, err
		}
		i += n
		num := int(key >> 3)
		wire := int(key & 7)
		if num == 0 {
			return nil, fmt.Errorf("protobuf 字段号为 0（key = %d）", key)
		}
		field := protoField{num: num, wire: wire}
		switch wire {
		case wireVarint:
			value, n, err := readVarint(data[i:])
			if err != nil {
				return nil, err
			}
			i += n
			field.varint = value
		case wireFixed64:
			if len(data)-i < 8 {
				return nil, fmt.Errorf("字段 %d 的 64 位定长值被截断", num)
			}
			i += 8
		case wireBytes:
			length, n, err := readVarint(data[i:])
			if err != nil {
				return nil, err
			}
			i += n
			if uint64(len(data)-i) < length {
				return nil, fmt.Errorf("字段 %d 声明长度 %d，实际只剩 %d 字节", num, length, len(data)-i)
			}
			field.raw = data[i : i+int(length)]
			i += int(length)
		case wireFixed32:
			if len(data)-i < 4 {
				return nil, fmt.Errorf("字段 %d 的 32 位定长值被截断", num)
			}
			i += 4
		default:
			// group（3/4）在 proto3 里已废弃，protobufjs 也不会产出；
			// 认不出来就报错，别"跳过 8 字节试试"。
			return nil, fmt.Errorf("不支持的 protobuf wire type %d（字段 %d）", wire, num)
		}
		fields = append(fields, field)
	}
	return fields, nil
}

func appendVarint(dst []byte, value uint64) []byte {
	for value >= 0x80 {
		dst = append(dst, byte(value)|0x80)
		value >>= 7
	}
	return append(dst, byte(value))
}

func appendTag(dst []byte, num, wire int) []byte {
	return appendVarint(dst, uint64(num)<<3|uint64(wire))
}

// appendScalar 写一个 singular 标量，**取零值时不写**（proto3 规则 1）。
func appendScalar(dst []byte, num int, value uint64) []byte {
	if value == 0 {
		return dst
	}
	dst = appendTag(dst, num, wireVarint)
	return appendVarint(dst, value)
}

// appendScalarForced 写一个 singular 标量，**零值也写**。
//
// 只有 `pov` 用它 —— 原实现写 `BigInt(0)`，protobufjs 当 `Long` 对象处理，
// 于是"有值"判断成立、零值被写出去（见 [MSyncMessage.Pov]）。
// 不要拿它去写别的字段：其余标量给的是普通 number，零值一律不写。
func appendScalarForced(dst []byte, num int, value uint64) []byte {
	dst = appendTag(dst, num, wireVarint)
	return appendVarint(dst, value)
}

// appendString 写一个 singular 字符串，空串不写。
func appendString(dst []byte, num int, value string) []byte {
	if value == "" {
		return dst
	}
	dst = appendTag(dst, num, wireBytes)
	dst = appendVarint(dst, uint64(len(value)))
	return append(dst, value...)
}

// appendBytes 写一个 singular bytes，空字节不写。
func appendBytes(dst []byte, num int, value []byte) []byte {
	if len(value) == 0 {
		return dst
	}
	dst = appendTag(dst, num, wireBytes)
	dst = appendVarint(dst, uint64(len(value)))
	return append(dst, value...)
}

// appendMessage 写一个嵌套消息。**空消息也要写**（`tag + 长度 0`）——
// 它是"字段被设置了"的表达，与"没设置"不是一回事（同 proto3 的 presence 语义）。
func appendMessage(dst []byte, num int, body []byte) []byte {
	dst = appendTag(dst, num, wireBytes)
	dst = appendVarint(dst, uint64(len(body)))
	return append(dst, body...)
}

// appendPackedUint32 写一个 **packed** 的 repeated uint32。
//
// ⚠ 与 [appendScalar] 的两处相反：① wire type 是 2 不是 0；② **元素为 0 也要写**。
// 非空数组一律产出 `tag + 长度 + 值`（哪怕只有一个 0）。
func appendPackedUint32(dst []byte, num int, values []uint32) []byte {
	if len(values) == 0 {
		return dst
	}
	body := make([]byte, 0, len(values))
	for _, v := range values {
		body = appendVarint(body, uint64(v))
	}
	return appendMessage(dst, num, body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 编码
// ─────────────────────────────────────────────────────────────────────────────

func encodeJID(jid JID) []byte {
	var out []byte
	out = appendString(out, 1, jid.AppKey)
	out = appendString(out, 2, jid.Name)
	out = appendString(out, 3, jid.Domain)
	out = appendString(out, 4, jid.ClientResource)
	return out
}

// EncodeMSync 把一条 MSync 编成 wire 字节。
//
// 字段顺序**必须**是编号升序 —— 这是 protobufjs 的实际输出形状，
// 也是 `test/golden/live-room-vectors.json` 逐字节钉住的东西。
func EncodeMSync(m MSyncMessage) []byte {
	var out []byte
	out = appendScalar(out, 1, uint64(m.Version))
	out = appendMessage(out, 2, encodeJID(m.GUID))
	out = appendString(out, 3, m.Auth)
	// 4 compressAlgorimth / 5 crypto：原实现不赋值（恒 0）⇒ 不写。
	out = appendString(out, 6, m.UserAgent)
	// 7 pov：⚠ **零值也要写**（原实现给的是 `BigInt(0)`，protobufjs 当对象处理）。
	// 所以这里走"显式判 nil"而不是 [appendScalar] 的"取零就跳过"。
	if m.Pov != nil {
		out = appendScalarForced(out, 7, *m.Pov)
	}
	out = appendScalar(out, 8, uint64(m.Command))
	out = appendBytes(out, 9, m.Payload)
	// 10 deviceId：恒 0 ⇒ 不写。
	out = appendPackedUint32(out, 11, m.EncryptType)
	// 12 encryptKey：原实现不赋值 ⇒ 不写。
	out = appendBytes(out, 13, m.DeviceInfo)
	out = appendBytes(out, 14, m.Timestamp)
	out = appendBytes(out, 15, m.Identification)
	return out
}

// LiveMSyncFrame 构造一条**直播侧**的 MSync 报文（恒定项一次填好）。
//
// # 为什么要这个构造函数，而不是让调用方自己拼结构体
//
// 这条报文里有 **六项**是原实现写死的：`version=0` / `userAgent` /
// `pov=BigInt(0)` / `encryptType=[0]` / `deviceInfo` / `auth` 的 `$t$` 前缀。
// 漏掉任何一项**都不会被服务端当场拒掉**（报文体仍然合法），
// 但报文就与原客户端不同了 —— 而"哪里不同"要到开播时才发现。
//
// 最阴的是 [MSyncMessage.Pov]：它是指针，忘了填就是**静默少 2 个字节**，
// 弱断言（"解回来 command 对不对"）全绿。
//
// 会变的只有四项：`guid.clientResource`（`js_<毫秒>`）、`command`、
// `payload`、`timestamp` / `identification`。
func LiveMSyncFrame(
	accessToken, phone string,
	command int,
	payload []byte,
	now time.Time,
	rng core.Random,
) MSyncMessage {
	return MSyncMessage{
		Version: 0,
		GUID: JID{
			AppKey: LiveAppKey,
			// 直播间形态的 JID name 用 **phone**（不是 roomId）。
			Name:           "cmlive_c_" + phone,
			Domain:         LiveDomain,
			ClientResource: fmt.Sprintf("js_%d", now.UnixMilli()),
		},
		Auth:           "$t$" + accessToken,
		UserAgent:      LiveMSyncUserAgent,
		Command:        command,
		Payload:        payload,
		EncryptType:    []uint32{0},
		DeviceInfo:     LiveDeviceInfo(),
		Timestamp:      LiveTimestampBytes(now.UnixMilli()),
		Identification: LiveIdentification(rng),
		// ⚠ 非 nil 的零值 —— 见 [MSyncMessage.Pov]，这一项**不能**省。
		Pov: new(uint64),
	}
}

// EncodeProvision 把 PROVISION 的 payload 编成 wire 字节。
func EncodeProvision(p Provision) []byte {
	var out []byte
	out = appendScalar(out, 1, uint64(p.OSType))
	out = appendString(out, 2, p.Version)
	out = appendScalar(out, 3, uint64(p.NetworkType))
	out = appendString(out, 4, p.AppSign)
	out = appendPackedUint32(out, 5, p.CompressType)
	out = appendPackedUint32(out, 6, p.EncryptType)
	// 7 encryptKey / 8 status / 10 isManualLogin / 11 password：不赋值 ⇒ 不写。
	out = appendString(out, 9, p.DeviceUUID)
	out = appendString(out, 12, p.DeviceName)
	out = appendString(out, 13, p.Resource)
	out = appendString(out, 14, p.Auth)
	out = appendString(out, 15, p.ProvinceCode)
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 解码
// ─────────────────────────────────────────────────────────────────────────────

func decodeJID(data []byte) (JID, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return JID{}, err
	}
	var jid JID
	for _, f := range fields {
		switch f.num {
		case 1:
			jid.AppKey = string(f.raw)
		case 2:
			jid.Name = string(f.raw)
		case 3:
			jid.Domain = string(f.raw)
		case 4:
			jid.ClientResource = string(f.raw)
		}
	}
	return jid, nil
}

// DecodeMSync 解一条 MSync 报文。
//
// 认不出的字段**直接跳过**（proto3 的前向兼容要求：新版本加字段不该让老客户端炸）。
func DecodeMSync(data []byte) (MSyncMessage, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return MSyncMessage{}, err
	}
	var out MSyncMessage
	for _, f := range fields {
		switch f.num {
		case 1:
			out.Version = int(f.varint)
		case 2:
			jid, err := decodeJID(f.raw)
			if err != nil {
				return MSyncMessage{}, fmt.Errorf("guid 解码失败: %w", err)
			}
			out.GUID = jid
		case 3:
			out.Auth = string(f.raw)
		case 6:
			out.UserAgent = string(f.raw)
		case 7:
			// ⚠ 必须**读**回来，不能当未知字段跳过：编码侧"零值也写"那一支
			// 只有解码侧跟着读，往返断言才闭得上（见 [MSyncMessage.Pov]）。
			value := f.varint
			out.Pov = &value
		case 8:
			out.Command = int(f.varint)
		case 9:
			out.Payload = f.raw
		case 11:
			// packed（wire 2）。⚠ 也容忍非 packed 的重复 varint（wire 0）：
			// 我们发出去的是 packed，但服务端**没有实拍**，两种都认比只认一种稳。
			if f.wire == wireVarint {
				out.EncryptType = append(out.EncryptType, uint32(f.varint))
			} else {
				values, err := decodePackedUint32(f.raw)
				if err != nil {
					return MSyncMessage{}, fmt.Errorf("encryptType 解码失败: %w", err)
				}
				out.EncryptType = append(out.EncryptType, values...)
			}
		case 13:
			out.DeviceInfo = f.raw
		case 14:
			out.Timestamp = f.raw
		case 15:
			out.Identification = f.raw
		}
	}
	return out, nil
}

func decodePackedUint32(data []byte) ([]uint32, error) {
	var out []uint32
	for i := 0; i < len(data); {
		value, n, err := readVarint(data[i:])
		if err != nil {
			return nil, err
		}
		i += n
		out = append(out, uint32(value))
	}
	return out, nil
}

// DecodeProvision 解 PROVISION 那一帧的 payload。
func DecodeProvision(data []byte) (Provision, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return Provision{}, err
	}
	var out Provision
	for _, f := range fields {
		switch f.num {
		case 1:
			out.OSType = uint32(f.varint)
		case 2:
			out.Version = string(f.raw)
		case 3:
			out.NetworkType = uint32(f.varint)
		case 4:
			out.AppSign = string(f.raw)
		case 5:
			values, err := decodePackedUint32(f.raw)
			if err != nil {
				return Provision{}, fmt.Errorf("compressType 解码失败: %w", err)
			}
			out.CompressType = values
		case 6:
			values, err := decodePackedUint32(f.raw)
			if err != nil {
				return Provision{}, fmt.Errorf("encryptType 解码失败: %w", err)
			}
			out.EncryptType = values
		case 9:
			out.DeviceUUID = string(f.raw)
		case 12:
			out.DeviceName = string(f.raw)
		case 13:
			out.Resource = string(f.raw)
		case 14:
			out.Auth = string(f.raw)
		case 15:
			out.ProvinceCode = string(f.raw)
		}
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 入站：从推送帧里取弹幕文本
// ─────────────────────────────────────────────────────────────────────────────

// MessageContent 对应 `MessageBody.Content`（只用 `type` 与 `text`）。
type MessageContent struct {
	Type int
	Text string
}

// MessageBody 对应 `cmos.pb.MessageBody`。
type MessageBody struct {
	Type     int
	Contents []MessageContent
}

// DecodeMessageBody 解 `MessageBody`。
func DecodeMessageBody(data []byte) (MessageBody, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return MessageBody{}, err
	}
	var out MessageBody
	for _, f := range fields {
		switch f.num {
		case 1:
			out.Type = int(f.varint)
		case 4:
			content, err := decodeContent(f.raw)
			if err != nil {
				return MessageBody{}, fmt.Errorf("contents 解码失败: %w", err)
			}
			out.Contents = append(out.Contents, content)
		}
	}
	return out, nil
}

func decodeContent(data []byte) (MessageContent, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return MessageContent{}, err
	}
	var out MessageContent
	for _, f := range fields {
		switch f.num {
		case 1:
			out.Type = int(f.varint)
		case 2:
			out.Text = string(f.raw)
		}
	}
	return out, nil
}

// EncodeMessageBody 把 [MessageBody] 编回 wire 字节 —— [DecodeMessageBody] 的逆。
//
// # 为什么它出现在**生产**代码里
//
// 入站那一侧（[ExtractDanmakuText] 的三级兜底、[KoulingListener.OnFrame] 的
// 帧分派）目前一条实拍都没有，全部来自原项目 —— 也就是说它是这条链上
// **最需要测试、也最没法靠抓包验证**的一段。
//
// 而它只能在 `internal/upstream` 里被测：入站帧的构造要用到本包那几个
// 未导出的 protobuf 写助手。没有这个导出函数的话，`internal/app` 的驱动层
// 测试就只能**硬编码一段 base64** —— 那种测试通过时只证明"我抄的字节
// 与实现一致"，两处都错时照样绿。
//
// ⇒ 它是为了让上层能**造出**入站帧而存在的，不是给发送用的。
func EncodeMessageBody(body MessageBody) []byte {
	var out []byte
	out = appendScalar(out, 1, uint64(body.Type))
	for _, content := range body.Contents {
		var item []byte
		item = appendScalar(item, 1, uint64(content.Type))
		item = appendString(item, 2, content.Text)
		// ⚠ repeated 元素用 [appendMessage]（**空元素也写**）——
		// 一个只有 `type` 没有 `text` 的 content 在 proto3 里是"被设置了"。
		out = appendMessage(out, 4, item)
	}
	return out
}

// decodeMetaPayload 从 `Meta` 里取 `payload`（字段 6）。
func decodeMetaPayload(data []byte) ([]byte, bool, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return nil, false, err
	}
	for _, f := range fields {
		if f.num == 6 {
			return f.raw, true, nil
		}
	}
	return nil, false, nil
}

// DecodeCommSyncULMetaPayload 取 `CommSyncUL.meta.payload`（字段 1 → 6）。
func DecodeCommSyncULMetaPayload(data []byte) ([]byte, bool, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return nil, false, err
	}
	for _, f := range fields {
		if f.num == 1 {
			return decodeMetaPayload(f.raw)
		}
	}
	return nil, false, nil
}

// DecodeNoticeMetaPayload 取 `NoticePayload.meta.payload`（字段 2 → 6）。
//
// ⚠ 与 [DecodeCommSyncULMetaPayload] 的字段号**不同**（这里 meta 是 2、那边是 1）
// —— 两个消息长得像，抄错字段号的表现是"NOTICE 这条路永远解不出文本"。
func DecodeNoticeMetaPayload(data []byte) ([]byte, bool, error) {
	fields, err := scanProtoFields(data)
	if err != nil {
		return nil, false, err
	}
	for _, f := range fields {
		if f.num == 2 {
			return decodeMetaPayload(f.raw)
		}
	}
	return nil, false, nil
}

// ExtractDanmakuText 从一帧推送里取出**第一条**弹幕文本。
//
// 这是原实现那套**三级兜底**的等价物（`live-room-kouling.ts` 的 onMessage）：
//
//	SYNC(0)   → CommSyncUL.meta.payload → MessageBody.contents[0].text
//	             ↳ 解不开就退到「整段 payload 直接当 MessageBody」
//	NOTICE(2) → NoticePayload.meta.payload → MessageBody.contents[0].text
//	             ↳ 同样退一级
//
// # 为什么必须保留兜底
//
// 服务端推的是哪一层**没有实拍**（2026-09-22 那轮未开播，`chatRoomId: null`
// ⇒ WS 结构性不存在）。原实现是先写死了 SYNC 那一层、后来撞上解不出文本才补的兜底
// —— 也就是说，这一级的存在本身就是"猜错过一次"的证据。
// 纯协议实现如果只留一条路，等于把那次踩坑重新踩一遍。
//
// 返回值 `("", nil)` 表示"这一帧没有文本"（不是错误）：心跳、无 payload 的控制帧
// 都会走到这里，而它们**本来就不该被当成异常**。
func ExtractDanmakuText(command int, payload []byte) (string, error) {
	if len(payload) == 0 {
		return "", nil
	}
	if command != CmdSync && command != CmdNotice {
		return "", nil
	}

	var inner []byte
	var hasInner bool
	var err error
	if command == CmdSync {
		inner, hasInner, err = DecodeCommSyncULMetaPayload(payload)
	} else {
		inner, hasInner, err = DecodeNoticeMetaPayload(payload)
	}
	if err == nil && hasInner && len(inner) > 0 {
		if text, ok := firstContentText(inner); ok {
			return text, nil
		}
	}

	// 兜底：把整段 payload 当 MessageBody 再解一次。
	if text, ok := firstContentText(payload); ok {
		return text, nil
	}
	if err != nil {
		// 外层解不开、兜底也没解出文本 —— 这时才把外层的错误报上去：
		// 它至少比"空文本"多一条线索（前 64 字节 hex 由调用方打）。
		return "", err
	}
	return "", nil
}

func firstContentText(data []byte) (string, bool) {
	body, err := DecodeMessageBody(data)
	if err != nil || len(body.Contents) == 0 {
		return "", false
	}
	return body.Contents[0].Text, true
}

// HexPreview 返回一段字节的前 n 字节 hex，供"解不开"时打日志。
//
// 放在这里而不是调用点：解不开的现场**最需要**的就是这段 hex，
// 而每个调用点各写一遍 `hex.EncodeToString(data[:64])` 时，
// 最容易漏的恰好是"data 比 64 短"那个越界。
func HexPreview(data []byte, n int) string {
	if n <= 0 {
		return ""
	}
	if len(data) > n {
		data = data[:n]
	}
	return hex.EncodeToString(data)
}

// LiveTimestampBytes 把毫秒编成 MSync 的 `timestamp`（**8 字节大端**）。
//
// ⚠ 不是 protobuf 的 varint：原项目 `genTimestamp` 是 `BigInt(Date.now())` 逐字节
// 右移填 8 字节。写成 varint 会让服务端认不出时间，而**没有任何本地断言会变红**
// —— 所以这里由 `test/golden/live-room-vectors.json` 的逐字节断言钉住。
func LiveTimestampBytes(millis int64) []byte {
	out := make([]byte, 8)
	v := uint64(millis)
	for i := 7; i >= 0; i-- {
		out[i] = byte(v)
		v >>= 8
	}
	return out
}

// LiveIdentification 生成 `identification`：`cmos_im` 前缀 + 16 随机字节。
//
// `rng` 注入而不是内部取全局随机源 —— 否则 fixture 回放与黄金向量对拍无法复现。
func LiveIdentification(rng core.Random) []byte {
	random := make([]byte, 16)
	rng.FillBytes(random)
	return append([]byte(LiveIdentificationPrefix), random...)
}

// SockJSSessionID 生成 SockJS 路径里的 13 字符 sessionId。
//
// 对齐原项目 `Math.random().toString(36).substring(2, 15)`：36 进制、取 13 位。
// ⚠ 原实现取的是 `Math.random()` 的小数部分（**不是**安全随机）—— 这里用注入的
// [core.Random] 逐位取 36 进制字符，**不照抄**它的熵源（它那一套没有可复现性，
// 而我们需要的恰好是可复现）。服务端只把它当路径里的一段，不参与鉴权。
func SockJSSessionID(rng core.Random) string {
	const alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
	out := make([]byte, 13)
	for i := range out {
		out[i] = alphabet[core.Below(rng, uint32(len(alphabet)))]
	}
	return string(out)
}

// LiveWSURL 拼 SockJS 的 WebSocket 端点。
//
// 形状（原项目逐字）：`wss://videoim.cmcc-cs.cn:20102/ws/<serverId 0-999>/<sessionId>/websocket`
//
// `serverID` 是 **0–999** 的随机数 —— 它是 SockJS 用来做多节点粘滞的路由段，
// 服务端不认识的值不是错误（SockJS 会回一个 `c[...]` 关闭帧），但也没必要乱给。
func LiveWSURL(serverID uint32, sessionID string) string {
	return fmt.Sprintf("%s/ws/%s/%s/websocket", BaseVideoIMWS, strconv.FormatUint(uint64(serverID), 10), sessionID)
}
