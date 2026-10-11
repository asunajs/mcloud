//! 直播 WebSocket 的 **MSync 报文层** —— 手写 proto3 编解码。
//!
//! 对应 Go 参考的 `internal/upstream/live_msync.go`，也就是原项目
//! `packages/core/src/live-room/proto.ts`（proto3 **内联字符串**，无 `.proto`
//! 文件、无 codegen，运行时用 protobufjs parse）。
//!
//! 本模块是**纯层**：零 IO、零网络、零异步。它只做「结构体 ↔ wire 字节」。
//!
//! # 为什么手写，而不引 protobuf / prost
//!
//! 1. 真正要动的只有**三个**消息（`MSync` / `Provision` / 几个入站包装），
//!    而 proto3 的 wire format 只有四种 wire type —— 引一整套运行时换来的是
//!    用不到的能力（反射、JSON 映射、descriptor）；
//! 2. 手写之后**对拍面反而更硬**：可以逐字节比 protobufjs 的输出
//!    （`test/golden/live-room-vectors.json` 的 `msyncFrames`，由原实现产出）。
//!
//! # 四条最容易写错、而且**错了不会报错**的规则
//!
//! 1. **singular 字段取默认值时不写**。proto3 的 `version = 0` / `crypto = 0` /
//!    `deviceId = 0` / 空串 / 空 bytes 一律**整条不出现**。写上去不是"多几个字节"：
//!    protobufjs 照样解得开，只有逐字节比才发现不一样。
//! 2. ⚠⚠ **`repeated` 标量默认 packed**（wire type **2**，长度前缀 + 连写的
//!    varint），而不是"重复出现 wire type 0"。`encryptType = [0]` 的编码是
//!    `5a 01 00` —— **`repeated` 里的 0 要写**（它不适用第 1 条）。
//! 3. **字段按编号升序出现**。protobufjs 的 Writer 按字段定义顺序写，而那份
//!    schema 的字段号恰好是升序 —— 所以"升序"是它的实际输出形状。
//! 4. ⚠⚠ **`pov` 是第 1 条的反例**：原实现写 `BigInt(0)`，protobufjs 把它当
//!    `Long` **对象**（非 null）⇒ **零值也写**（`38 00`）。同一份报文里
//!    `crypto: 0` / `deviceId: 0` 是普通 number，一律不写。
//!    这一条是**逐字节对拍抓出来的**（第一版实现漏了它，弱断言全绿）。
//!    见 [`MsyncMessage::pov`]。
//!
//! # 字段号来源
//!
//! 全部逐个照抄 `proto.ts` 的 `PROTO_DEF`（权威），不按名字猜。参考实现里
//! 每个消息的字段都带编号注释，本文件保持同一口径。
//!
//! # 与 Go 参考实现的唯一有意差异
//!
//! Go 侧用**指针**表达 proto3 的"不出现 vs 写出去"；Rust 侧用 `Option<T>`。
//! 语义一一对应：`None` ⇒ 字段不写，`Some(0)` ⇒ 字段写出去（varint 两个字节）。

use mcloud_crypto::Random;

// ─────────────────────────────────────────────────────────────────────────────
// 常量（逐字对齐 proto.ts / 黄金向量的 constants 段）
// ─────────────────────────────────────────────────────────────────────────────

/// **直播间**形态的 `guid.appKey`（带 `#`，与 [`SDK_APP_KEY`] 不是一个值）。
pub const LIVE_APP_KEY: &str = "cmos#ngvlcs";

/// **非直播间**形态的 `guid.appKey`（原实现的另一条分支）。
///
/// 两条分支的差别不只是 appKey：`guid.name` 也从 `cmlive_c_<phone>` 变成
/// `cmlive_<roomId>`。所以判断依据是**有没有 phone**，而不是"是不是在直播"
/// —— 照抄原实现，别自己另立一条判据。
pub const SDK_APP_KEY: &str = "cmos_js_sdk";

/// `guid.domain`。
pub const LIVE_DOMAIN: &str = "cmosim.com";

/// `MSync.userAgent`（原项目硬编码的 iPad UA 前缀，**不是** HTTP 头）。
///
/// ⚠ 它比 `live_consts::LIVE_USER_AGENT`（HTTP 头那个）**短一截**：少了
/// ` (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN`。
/// 两个值长得像但不是一个 —— 互搬之后没有任何本地断言会变红。
pub const LIVE_MSYNC_USER_AGENT: &str =
    "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15";

/// `MSync.deviceInfo` 的 **48 字节**常量（96 个 hex 字符）。
///
/// ⚠ 它是**应用级常量**：原项目把它硬编码在源码里，每个客户端发出去的都是
/// 同一串 —— 所以它不是设备指纹，也不该被"每台设备换一个"。照抄。
///
/// ⚠ 字节数别按"看起来多长"估：`len(hex) / 2` = 96 / 2 = **48**。这个数在
/// 别处被写成过 40（同一个常量、同一个文件里两种写法），而"40 字节"读起来
/// 同样合理 ⇒ 只能数。
pub const LIVE_DEVICE_INFO_HEX: &str = "830652084dda69b6b957e2c3a7bd9b18db65922e29051b1b342ccf15bebc0fdbcc96f61dadab12f3cab1e66cb28ec05b";

/// `identification` 的固定前缀（原项目 `genIdentification`）。
pub const LIVE_IDENTIFICATION_PREFIX: &str = "cmos_im";

/// `auth` 的前缀：原实现是 `"$t$" + accessToken`，**不是**裸 token。
pub const LIVE_AUTH_PREFIX: &str = "$t$";

// ── MSync.Command 枚举（proto.ts 逐字）────────────────────────────────────

/// `SYNC` —— 服务端推弹幕用的命令。
pub const CMD_SYNC: u8 = 0;
/// `UNREAD` —— **必须主动补发**的那一条：PROVISION 响应之后不发它，
/// 服务端不推弹幕。
pub const CMD_UNREAD: u8 = 1;
/// `NOTICE` —— 另一条服务端推送路径。
pub const CMD_NOTICE: u8 = 2;
/// `PROVISION` —— 握手命令，也是**唯一**带 payload 的那一帧。
pub const CMD_PROVISION: u8 = 3;
/// `LOGOUT`。
pub const CMD_LOGOUT: u8 = 4;
/// `HEARTBEAT`。
pub const CMD_HEARTBEAT: u8 = 5;
/// `ROUTE`。
pub const CMD_ROUTE: u8 = 6;
/// `EVT_BROADCAST`。
pub const CMD_EVT_BROADCAST: u8 = 7;

/// `MSync.Version` 的 `MSYNC_V1 = 0`（原实现恒用它，0 不写进报文）。
pub const MSYNC_V1: u8 = 0;
/// `MSync.Version` 的 `MSYNC_V2 = 1`。
pub const MSYNC_V2: u8 = 1;

// ── Provision 里的取值（proto.ts 注释：`7 = OS_JS`、`1 = WIFI`）────────────

/// `Provision.osType`：`OS_JS`。
pub const PROVISION_OS_TYPE_JS: u32 = 7;
/// `Provision.networkType`：`WIFI`。
pub const PROVISION_NETWORK_WIFI: u32 = 1;
/// `Provision.appSign`。
pub const PROVISION_APP_SIGN: &str = "cmlive_cmos_js_sdk";
/// `Provision.version`（**不带** `v` 前缀，与 HTTP 头的 `v23.0.2` 不同）。
pub const PROVISION_VERSION: &str = "23.0.2";
/// `Provision.deviceName`。
pub const PROVISION_DEVICE_NAME: &str = "webim";
/// `msyncInbound` 里 `kind = "notice"` 那一条对应的命令。
///
/// 它只是把 [`CMD_NOTICE`] 换个读法，放在这里是为了让驱动层不必同时引两套命名。
pub const NOTICE_KIND: &str = "notice";

/// 返回 `deviceInfo` 的字节形态（**48 字节**）。
///
/// ⚠ 它是**应用级常量**（不是设备指纹）：原项目把它硬编码在源码里，每个客户端
/// 发出去的都是同一串，照抄即可。
///
/// 每次调用都重新解码 hex 而不是缓存一份全局切片：返回的是**可变 Vec**，
/// 共享一份等于让任何一个调用方都能改掉下一个请求的报文（同 Go 侧
/// `LiveDeviceInfo` 那条"必须拷贝"的理由）。
///
/// hex 常量写错属于**开发期错误**，不是运行时状况 —— 所以这里直接 `expect`
/// 当场炸，而不是发一个 0 字节的 `deviceInfo` 出去（那会被服务端静默拒掉）。
pub fn live_device_info() -> Vec<u8> {
    hex::decode(LIVE_DEVICE_INFO_HEX)
        .expect("LIVE_DEVICE_INFO_HEX 必须是合法 hex（常量写错是开发期错误）")
}

/// 把毫秒编成 MSync 的 `timestamp`（**8 字节大端**）。
///
/// ⚠ 不是 protobuf 的 varint：原项目 `genTimestamp` 是 `BigInt(Date.now())`
/// 逐字节右移填 8 字节。写成 varint 会让服务端认不出时间，而**没有任何本地
/// 断言会变红** —— 所以这里由黄金向量的逐字节断言钉住。
pub fn live_timestamp_bytes(millis: i64) -> Vec<u8> {
    let mut out = [0u8; 8];
    let mut v = millis as u64;
    for i in (0..8).rev() {
        out[i] = v as u8;
        v >>= 8;
    }
    out.to_vec()
}

/// 生成 `identification`：`cmos_im` 前缀 + 16 随机字节。
///
/// `rng` 注入而不是内部取全局随机源 —— 否则 fixture 回放与黄金向量对拍无法复现。
pub fn live_identification(rng: &mut dyn Random) -> Vec<u8> {
    let mut random = [0u8; 16];
    rng.fill_bytes(&mut random);
    let mut out = Vec::with_capacity(LIVE_IDENTIFICATION_PREFIX.len() + 16);
    out.extend_from_slice(LIVE_IDENTIFICATION_PREFIX.as_bytes());
    out.extend_from_slice(&random);
    out
}

/// 返回一段字节的前 `n` 字节 hex，供"解不开"时打日志。
///
/// 放在这里而不是调用点：解不开的现场**最需要**的就是这段 hex，而每个调用点
/// 各写一遍 `hex::encode(&data[..64])` 时，最容易漏的恰好是"data 比 64 短"
/// 那个越界。
pub fn hex_preview(data: &[u8], n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let end = data.len().min(n);
    hex::encode(&data[..end])
}

// ─────────────────────────────────────────────────────────────────────────────
// 消息形状
// ─────────────────────────────────────────────────────────────────────────────

/// 对应 `cmos.pb.JID`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jid {
    /// `appKey = 1`。
    pub app_key: String,
    /// `name = 2`。
    pub name: String,
    /// `domain = 3`。
    pub domain: String,
    /// `clientResource = 4`。
    pub client_resource: String,
}

/// 对应 `cmos.pb.MSync`。
///
/// 只声明**我们真的会读或写**的字段：`compressAlgorimth`(4) / `crypto`(5) /
/// `encryptKey`(12) / `srcIdc`(16) / `dstIdc`(17) / `tos`(18) 在原实现里从不
/// 赋值，写出来只会让人以为它们在参与协议。
///
/// 没有 `device_id` 字段：原实现恒给 `0` ⇒ proto3 规则下**整条不出现**，
/// 加一个"恒 0 的字段"只会让人以为它是可调的。解码时也照样跳过。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MsyncMessage {
    /// `version = 1`（[`MSYNC_V1`] = 0）；0 时**不写进报文**。
    pub version: u8,
    /// `guid = 2`。
    pub guid: Jid,
    /// `auth = 3`，取值是 `"$t$" + accessToken`（前缀不能省）。
    pub auth: String,
    /// `userAgent = 6`。
    pub user_agent: String,
    /// `command = 8`，取 [`CMD_SYNC`] / [`CMD_UNREAD`] / [`CMD_NOTICE`] /
    /// [`CMD_PROVISION`] 等。
    pub command: u8,
    /// `payload = 9`，只有 PROVISION 那一帧有（[`Provision`] 的编码字节）。
    pub payload: Vec<u8>,
    /// `encryptType = 11`，原实现恒为 `[0]`（`ENCRYPT_NONE`）。
    ///
    /// ⚠ 它是 repeated ⇒ **packed 编码**，而且**值为 0 也要写**（见模块文档第 2 条）。
    pub encrypt_type: Vec<u32>,
    /// `deviceInfo = 13`，取值是 [`live_device_info`]（**48 字节**）。
    pub device_info: Vec<u8>,
    /// `timestamp = 14`，**8 字节大端**毫秒（不是 protobuf 的 varint 时间戳）。
    pub timestamp: Vec<u8>,
    /// `identification = 15`，取值是 `cmos_im` + 16 随机字节。
    pub identification: Vec<u8>,
    /// `pov = 7`。
    ///
    /// ⚠⚠ 本文件里**唯一**需要 `Option` 的标量，理由值得记牢：原实现写的是
    /// `pov: BigInt(0)`，而 protobufjs 把 `BigInt` 转成 `Long` **对象** ——
    /// 对象非 null，于是"有值"判断成立，**零值也会被写出去**（实测 `38 00`；
    /// 同一份报文里的 `crypto: 0` / `deviceId: 0` 是普通 number，一律**不写**）。
    ///
    /// ```text
    /// None      ⇒ 字段不出现（0 字节）
    /// Some(0)   ⇒ 字段写出去（`38 00`，2 字节）
    /// ```
    ///
    /// 这一条只有**逐字节**对拍才看得见：`38 00` 多一个少一个，protobufjs
    /// 两边都解得开、字段也读得出 0。黄金向量的 `msyncFrames` 三条都钉着它。
    ///
    /// 生产路径用 [`live_msync_frame`]，它已经把这一项填好。
    pub pov: Option<u64>,
}

/// 对应 `cmos.pb.Provision`（PROVISION 那一帧的 payload）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provision {
    /// `osType = 1`，取值 [`PROVISION_OS_TYPE_JS`]。
    pub os_type: u32,
    /// `version = 2`，取值 [`PROVISION_VERSION`]。
    pub version: String,
    /// `networkType = 3`，取值 [`PROVISION_NETWORK_WIFI`]。
    pub network_type: u32,
    /// `appSign = 4`，取值 [`PROVISION_APP_SIGN`]。
    pub app_sign: String,
    /// `compressType = 5`，恒为 `[0]`（`COMPRESS_NONE`），同样 packed + 要写 0。
    pub compress_type: Vec<u32>,
    /// `encryptType = 6`，恒为 `[0]`（`ENCRYPT_NONE`）。
    pub encrypt_type: Vec<u32>,
    /// `deviceUuid = 9`。
    pub device_uuid: String,
    /// `deviceName = 12`，取值 [`PROVISION_DEVICE_NAME`]。
    pub device_name: String,
    /// `resource = 13`。
    pub resource: String,
    /// `auth = 14`（与 MSync 的 `auth` 同值）。
    pub auth: String,
    /// `province_code = 15`，原实现给的是**空串** ⇒ 整个字段不出现。
    pub province_code: String,
}

/// 对应 `MessageBody.Content`（只用 `type = 1` 与 `text = 2`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageContent {
    /// `type = 1`。
    pub content_type: i32,
    /// `text = 2`。
    pub text: String,
}

/// 对应 `cmos.pb.MessageBody`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageBody {
    /// `type = 1`。
    pub body_type: i32,
    /// `contents = 4`。
    pub contents: Vec<MessageContent>,
}

// ─────────────────────────────────────────────────────────────────────────────
// proto3 wire 基础
// ─────────────────────────────────────────────────────────────────────────────

/// proto3 的四种 wire type（3 / 4 是已废弃的 group，一律拒）。
const WIRE_VARINT: u32 = 0;
const WIRE_FIXED64: u32 = 1;
const WIRE_BYTES: u32 = 2;
const WIRE_FIXED32: u32 = 5;

/// 编解码错误。
///
/// 不引 `thiserror`：本模块的公开 API 只有三个 `Result`，而 Go 侧的
/// `fmt.Errorf` 已经被拍平成字符串。这里保留一个结构化变体 + 一个自由描述，
/// 够上层判断"是格式问题还是语义问题"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsyncError {
    /// wire 格式不合法（varint 溢出 / 长度越界 / 未知 wire type / 字段号 0）。
    Wire(String),
    /// 嵌套消息解不开，带上外层字段名，避免只看到一句"被截断"。
    Nested {
        /// 出问题的字段名（如 `guid` / `contents`）。
        field: &'static str,
        /// 内层错误描述。
        reason: String,
    },
}

impl std::fmt::Display for MsyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(msg) => write!(f, "protobuf wire 错误: {msg}"),
            Self::Nested { field, reason } => write!(f, "{field} 解码失败: {reason}"),
        }
    }
}

impl std::error::Error for MsyncError {}

/// 一个已扫出来的字段。
///
/// 做成"先扫一遍、再按字段号分派"而不是每个消息手写一个循环：三个入站包装
/// （`CommSyncUL` / `NoticePayload` / `MessageBody`）都要走同一套跳过逻辑，
/// 各写一份的话，某一份漏处理一个 wire type 的表现是**静默解出空串** ——
/// 而那正好和"这条弹幕没有文本"长得一模一样。
struct ProtoField<'a> {
    num: u32,
    wire: u32,
    varint: u64,
    raw: &'a [u8],
}

impl ProtoField<'_> {
    /// 解出字符串（`bytes` 字段按 UTF-8 读；非法字节走 `from_utf8_lossy`,
    /// 与 protobufjs 的宽松解码一致 —— 弹幕里出现半个汉字不该让整帧报错）。
    fn as_str(&self) -> String {
        String::from_utf8_lossy(self.raw).into_owned()
    }
}

fn read_varint(data: &[u8]) -> Result<(u64, usize), MsyncError> {
    let mut value: u64 = 0;
    for (i, &b) in data.iter().enumerate() {
        if i >= 10 {
            return Err(MsyncError::Wire(format!(
                "protobuf varint 超过 10 字节（第 {} 字节 = 0x{b:02x}）",
                i + 1
            )));
        }
        // 64 位最多 10 字节；第 10 字节只允许低 1 位有效。
        if i == 9 && b > 1 {
            return Err(MsyncError::Wire(format!(
                "protobuf varint 溢出（第 10 字节 = 0x{b:02x}）"
            )));
        }
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(MsyncError::Wire("protobuf varint 被截断".to_owned()))
}

fn scan_proto_fields(data: &[u8]) -> Result<Vec<ProtoField<'_>>, MsyncError> {
    let mut fields = Vec::with_capacity(16);
    let mut i = 0usize;
    while i < data.len() {
        let (key, n) = read_varint(&data[i..])?;
        i += n;
        let num = u32::try_from(key >> 3)
            .map_err(|_| MsyncError::Wire(format!("protobuf 字段号超出 u32（key = {key}）")))?;
        let wire = u32::try_from(key & 7)
            .map_err(|_| MsyncError::Wire(format!("protobuf wire type 超出 u32（key = {key}）")))?;
        if num == 0 {
            return Err(MsyncError::Wire(format!(
                "protobuf 字段号为 0（key = {key}）"
            )));
        }
        let mut field = ProtoField {
            num,
            wire,
            varint: 0,
            raw: &[],
        };
        match wire {
            WIRE_VARINT => {
                let (value, n) = read_varint(&data[i..])?;
                i += n;
                field.varint = value;
            }
            WIRE_FIXED64 => {
                if data.len() - i < 8 {
                    return Err(MsyncError::Wire(format!("字段 {num} 的 64 位定长值被截断")));
                }
                i += 8;
            }
            WIRE_BYTES => {
                let (length, n) = read_varint(&data[i..])?;
                i += n;
                let length = usize::try_from(length).map_err(|_| {
                    MsyncError::Wire(format!("字段 {num} 声明长度超出 usize（{length}）"))
                })?;
                if data.len() - i < length {
                    return Err(MsyncError::Wire(format!(
                        "字段 {num} 声明长度 {length}，实际只剩 {} 字节",
                        data.len() - i
                    )));
                }
                field.raw = &data[i..i + length];
                i += length;
            }
            WIRE_FIXED32 => {
                if data.len() - i < 4 {
                    return Err(MsyncError::Wire(format!("字段 {num} 的 32 位定长值被截断")));
                }
                i += 4;
            }
            other => {
                // group（3 / 4）在 proto3 里已废弃，protobufjs 也不会产出；
                // 认不出来就报错，别"跳过 8 字节试试"。
                return Err(MsyncError::Wire(format!(
                    "不支持的 protobuf wire type {other}（字段 {num}）"
                )));
            }
        }
        fields.push(field);
    }
    Ok(fields)
}

fn append_varint(dst: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        dst.push(value as u8 | 0x80);
        value >>= 7;
    }
    dst.push(value as u8);
}

fn append_tag(dst: &mut Vec<u8>, num: u32, wire: u32) {
    append_varint(dst, u64::from(num) << 3 | u64::from(wire));
}

/// 写一个 singular 标量，**取零值时不写**（proto3 规则 1）。
fn append_scalar(dst: &mut Vec<u8>, num: u32, value: u64) {
    if value == 0 {
        return;
    }
    append_tag(dst, num, WIRE_VARINT);
    append_varint(dst, value);
}

/// 写一个 singular 标量，**零值也写**。
///
/// 只有 `pov` 用它 —— 原实现写 `BigInt(0)`，protobufjs 当 `Long` 对象处理，
/// 于是"有值"判断成立、零值被写出去（见 [`MsyncMessage::pov`]）。
/// 不要拿它去写别的字段：其余标量给的是普通 number，零值一律不写。
fn append_scalar_forced(dst: &mut Vec<u8>, num: u32, value: u64) {
    append_tag(dst, num, WIRE_VARINT);
    append_varint(dst, value);
}

/// 写一个 singular 字符串，空串不写。
fn append_string(dst: &mut Vec<u8>, num: u32, value: &str) {
    if value.is_empty() {
        return;
    }
    append_tag(dst, num, WIRE_BYTES);
    append_varint(dst, value.len() as u64);
    dst.extend_from_slice(value.as_bytes());
}

/// 写一个 singular bytes，空字节不写。
fn append_bytes(dst: &mut Vec<u8>, num: u32, value: &[u8]) {
    if value.is_empty() {
        return;
    }
    append_tag(dst, num, WIRE_BYTES);
    append_varint(dst, value.len() as u64);
    dst.extend_from_slice(value);
}

/// 写一个嵌套消息。**空消息也要写**（`tag + 长度 0`）—— 它是"字段被设置了"
/// 的表达，与"没设置"不是一回事（同 proto3 的 presence 语义）。
fn append_message(dst: &mut Vec<u8>, num: u32, body: &[u8]) {
    append_tag(dst, num, WIRE_BYTES);
    append_varint(dst, body.len() as u64);
    dst.extend_from_slice(body);
}

/// 写一个 **packed** 的 repeated uint32。
///
/// ⚠ 与 [`append_scalar`] 的两处相反：① wire type 是 2 不是 0；② **元素为 0
/// 也要写**。非空数组一律产出 `tag + 长度 + 值`（哪怕只有一个 0）。
///
/// 数组空时**整条不出现** —— 空 repeated 与"没设置"在 proto3 里等价。
fn append_packed_uint32(dst: &mut Vec<u8>, num: u32, values: &[u32]) {
    if values.is_empty() {
        return;
    }
    let mut body = Vec::with_capacity(values.len());
    for &v in values {
        append_varint(&mut body, u64::from(v));
    }
    append_message(dst, num, &body);
}

// ─────────────────────────────────────────────────────────────────────────────
// 编码
// ─────────────────────────────────────────────────────────────────────────────

/// 编码 `cmos.pb.JID`（字段 1–4 全是 string）。
pub fn encode_jid(jid: &Jid) -> Vec<u8> {
    let mut out = Vec::new();
    append_string(&mut out, 1, &jid.app_key);
    append_string(&mut out, 2, &jid.name);
    append_string(&mut out, 3, &jid.domain);
    append_string(&mut out, 4, &jid.client_resource);
    out
}

/// 把一条 MSync 编成 wire 字节。
///
/// 字段顺序**必须**是编号升序 —— 这是 protobufjs 的实际输出形状，也是
/// `test/golden/live-room-vectors.json` 逐字节钉住的东西。
pub fn encode_msync(m: &MsyncMessage) -> Vec<u8> {
    let mut out = Vec::new();
    // 1 version：0 不写。
    append_scalar(&mut out, 1, u64::from(m.version));
    // 2 guid：**空消息也要写** —— 原实现总是 provide 一个 JID 对象。
    append_message(&mut out, 2, &encode_jid(&m.guid));
    // 3 auth。
    append_string(&mut out, 3, &m.auth);
    // 4 compressAlgorimth / 5 crypto：原实现不赋值（恒 0）⇒ 不写。
    // 6 userAgent。
    append_string(&mut out, 6, &m.user_agent);
    // 7 pov：⚠ **零值也要写**（原实现给的是 `BigInt(0)`，protobufjs 当对象处理）。
    // 所以这里走"显式判 None"而不是 `append_scalar` 的"取零就跳过"。
    if let Some(pov) = m.pov {
        append_scalar_forced(&mut out, 7, pov);
    }
    // 8 command：0（SYNC）不写。
    append_scalar(&mut out, 8, u64::from(m.command));
    // 9 payload：空不写。
    append_bytes(&mut out, 9, &m.payload);
    // 10 deviceId：恒 0 ⇒ 不写。
    // 11 encryptType：packed，且恒为 [0]（0 也要写）。
    append_packed_uint32(&mut out, 11, &m.encrypt_type);
    // 12 encryptKey：原实现不赋值 ⇒ 不写。
    append_bytes(&mut out, 13, &m.device_info);
    append_bytes(&mut out, 14, &m.timestamp);
    append_bytes(&mut out, 15, &m.identification);
    out
}

/// 构造一条**直播侧**的 MSync 报文（恒定项一次填好）。
///
/// # 为什么要这个构造函数，而不是让调用方自己拼结构体
///
/// 这条报文里有 **六项**是原实现写死的：`version=0` / `userAgent` /
/// `pov = Some(0)` / `encrypt_type = [0]` / `device_info` / `auth` 的 `$t$`
/// 前缀。漏掉任何一项**都不会被服务端当场拒掉**（报文体仍然合法），但报文就与
/// 原客户端不同了 —— 而"哪里不同"要到开播时才发现。
///
/// 最阴的是 [`MsyncMessage::pov`]：它忘了填就是**静默少 2 个字节**，弱断言
/// （"解回来 command 对不对"）全绿。
///
/// 会变的只有四项：`guid.client_resource`（`js_<毫秒>`）、`command`、`payload`、
/// `timestamp` / `identification`。
///
/// # `phone` 决定走哪条分支
///
/// 有 phone ⇒ 直播间形态（[`LIVE_APP_KEY`] + `cmlive_c_<phone>`）；
/// 没有 ⇒ 非直播间形态（[`SDK_APP_KEY`] + `cmlive_<room_id>`）。
/// 判据是**有没有 phone**，照抄原实现，别自己另立一条。
pub fn live_msync_frame(
    access_token: &str,
    phone: Option<&str>,
    room_id: &str,
    command: u8,
    payload: Vec<u8>,
    now_millis: i64,
    rng: &mut dyn Random,
) -> MsyncMessage {
    let (app_key, name) = match phone {
        Some(phone) => (LIVE_APP_KEY, format!("cmlive_c_{phone}")),
        None => (SDK_APP_KEY, format!("cmlive_{room_id}")),
    };
    MsyncMessage {
        version: MSYNC_V1,
        guid: Jid {
            app_key: app_key.to_owned(),
            name,
            domain: LIVE_DOMAIN.to_owned(),
            client_resource: format!("js_{now_millis}"),
        },
        auth: format!("{LIVE_AUTH_PREFIX}{access_token}"),
        user_agent: LIVE_MSYNC_USER_AGENT.to_owned(),
        command,
        payload,
        encrypt_type: vec![0],
        device_info: live_device_info(),
        timestamp: live_timestamp_bytes(now_millis),
        identification: live_identification(rng),
        // ⚠ 非 None 的零值 —— 见 [`MsyncMessage::pov`]，这一项**不能**省。
        pov: Some(0),
    }
}

/// 把 PROVISION 的 payload 编成 wire 字节。
pub fn encode_provision(p: &Provision) -> Vec<u8> {
    let mut out = Vec::new();
    append_scalar(&mut out, 1, u64::from(p.os_type));
    append_string(&mut out, 2, &p.version);
    append_scalar(&mut out, 3, u64::from(p.network_type));
    append_string(&mut out, 4, &p.app_sign);
    append_packed_uint32(&mut out, 5, &p.compress_type);
    append_packed_uint32(&mut out, 6, &p.encrypt_type);
    // 7 encryptKey / 8 status / 10 isManualLogin / 11 password：不赋值 ⇒ 不写。
    append_string(&mut out, 9, &p.device_uuid);
    append_string(&mut out, 12, &p.device_name);
    append_string(&mut out, 13, &p.resource);
    append_string(&mut out, 14, &p.auth);
    append_string(&mut out, 15, &p.province_code);
    out
}

/// 构造 PROVISION 那一帧的 payload（恒定项一次填好）。
///
/// `device_uuid` 由调用方给（原实现用毫秒字符串，但那是它的选择，不是协议约束）。
pub fn live_provision_payload(access_token: &str, resource: &str, device_uuid: &str) -> Vec<u8> {
    encode_provision(&Provision {
        os_type: PROVISION_OS_TYPE_JS,
        version: PROVISION_VERSION.to_owned(),
        network_type: PROVISION_NETWORK_WIFI,
        app_sign: PROVISION_APP_SIGN.to_owned(),
        // ⚠ 两个 repeated 都恒为 [0]：**0 也要写**（packed）。
        compress_type: vec![0],
        encrypt_type: vec![0],
        device_uuid: device_uuid.to_owned(),
        device_name: PROVISION_DEVICE_NAME.to_owned(),
        resource: resource.to_owned(),
        auth: format!("{LIVE_AUTH_PREFIX}{access_token}"),
        // 原实现给的是空串 ⇒ 整个字段不出现。别"顺手填个省份"。
        province_code: String::new(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// 解码
// ─────────────────────────────────────────────────────────────────────────────

/// 解 `cmos.pb.JID`。
pub fn decode_jid(data: &[u8]) -> Result<Jid, MsyncError> {
    let fields = scan_proto_fields(data)?;
    let mut jid = Jid::default();
    for f in fields {
        match f.num {
            1 => jid.app_key = f.as_str(),
            2 => jid.name = f.as_str(),
            3 => jid.domain = f.as_str(),
            4 => jid.client_resource = f.as_str(),
            // 认不出的字段直接跳过（proto3 的前向兼容要求）。
            _ => {}
        }
    }
    Ok(jid)
}

/// 解一条 MSync 报文。
///
/// 认不出的字段**直接跳过**（proto3 的前向兼容要求：新版本加字段不该让老客户端炸）。
pub fn decode_msync(data: &[u8]) -> Result<MsyncMessage, MsyncError> {
    let fields = scan_proto_fields(data)?;
    let mut out = MsyncMessage::default();
    for f in fields {
        match f.num {
            1 => out.version = f.varint as u8,
            2 => {
                out.guid = decode_jid(f.raw).map_err(|e| MsyncError::Nested {
                    field: "guid",
                    reason: e.to_string(),
                })?;
            }
            3 => out.auth = f.as_str(),
            6 => out.user_agent = f.as_str(),
            7 => {
                // ⚠ 必须**读**回来，不能当未知字段跳过：编码侧"零值也写"那一支
                // 只有解码侧跟着读，往返断言才闭得上（见 [`MsyncMessage::pov`]）。
                out.pov = Some(f.varint);
            }
            8 => out.command = f.varint as u8,
            9 => out.payload = f.raw.to_vec(),
            11 => {
                // packed（wire 2）。⚠ 也容忍非 packed 的重复 varint（wire 0）：
                // 我们发出去的是 packed，但服务端**没有实拍**，两种都认比只认一种稳。
                if f.wire == WIRE_VARINT {
                    out.encrypt_type.push(f.varint as u32);
                } else {
                    let values = decode_packed_uint32(f.raw).map_err(|e| MsyncError::Nested {
                        field: "encryptType",
                        reason: e.to_string(),
                    })?;
                    out.encrypt_type.extend(values);
                }
            }
            13 => out.device_info = f.raw.to_vec(),
            14 => out.timestamp = f.raw.to_vec(),
            15 => out.identification = f.raw.to_vec(),
            _ => {}
        }
    }
    Ok(out)
}

fn decode_packed_uint32(data: &[u8]) -> Result<Vec<u32>, MsyncError> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let (value, n) = read_varint(&data[i..])?;
        i += n;
        out.push(value as u32);
    }
    Ok(out)
}

/// 解 PROVISION 那一帧的 payload。
pub fn decode_provision(data: &[u8]) -> Result<Provision, MsyncError> {
    let fields = scan_proto_fields(data)?;
    let mut out = Provision::default();
    for f in fields {
        match f.num {
            1 => out.os_type = f.varint as u32,
            2 => out.version = f.as_str(),
            3 => out.network_type = f.varint as u32,
            4 => out.app_sign = f.as_str(),
            5 => {
                out.compress_type =
                    decode_packed_uint32(f.raw).map_err(|e| MsyncError::Nested {
                        field: "compressType",
                        reason: e.to_string(),
                    })?;
            }
            6 => {
                out.encrypt_type = decode_packed_uint32(f.raw).map_err(|e| MsyncError::Nested {
                    field: "encryptType",
                    reason: e.to_string(),
                })?;
            }
            9 => out.device_uuid = f.as_str(),
            12 => out.device_name = f.as_str(),
            13 => out.resource = f.as_str(),
            14 => out.auth = f.as_str(),
            15 => out.province_code = f.as_str(),
            _ => {}
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// 入站：从推送帧里取弹幕文本
// ─────────────────────────────────────────────────────────────────────────────

/// 解 `cmos.pb.MessageBody`。
pub fn decode_message_body(data: &[u8]) -> Result<MessageBody, MsyncError> {
    let fields = scan_proto_fields(data)?;
    let mut out = MessageBody::default();
    for f in fields {
        match f.num {
            1 => out.body_type = f.varint as i32,
            4 => {
                let content = decode_content(f.raw).map_err(|e| MsyncError::Nested {
                    field: "contents",
                    reason: e.to_string(),
                })?;
                out.contents.push(content);
            }
            _ => {}
        }
    }
    Ok(out)
}

fn decode_content(data: &[u8]) -> Result<MessageContent, MsyncError> {
    let fields = scan_proto_fields(data)?;
    let mut out = MessageContent::default();
    for f in fields {
        match f.num {
            1 => out.content_type = f.varint as i32,
            2 => out.text = f.as_str(),
            _ => {}
        }
    }
    Ok(out)
}

/// 把 [`MessageBody`] 编回 wire 字节 —— [`decode_message_body`] 的逆。
///
/// # 为什么它出现在**生产**代码里
///
/// 入站那一侧（[`extract_danmaku_text`] 的三级兜底）目前一条实拍都没有，全部
/// 来自原项目 —— 也就是说它是这条链上**最需要测试、也最没法靠抓包验证**的一段。
///
/// 而它只能在 `mcloud-upstream` 里被测：入站帧的构造要用到本模块那几个未导出的
/// protobuf 写助手。没有这个导出函数的话，驱动层测试就只能**硬编码一段
/// base64** —— 那种测试通过时只证明"我抄的字节与实现一致"，两处都错时照样绿。
///
/// ⇒ 它是为了让上层能**造出**入站帧而存在的，不是给发送用的。
pub fn encode_message_body(body: &MessageBody) -> Vec<u8> {
    let mut out = Vec::new();
    append_scalar(&mut out, 1, body.body_type as u64);
    for content in &body.contents {
        let mut item = Vec::new();
        append_scalar(&mut item, 1, content.content_type as u64);
        append_string(&mut item, 2, &content.text);
        // ⚠ repeated 元素用 `append_message`（**空元素也写**）——
        // 一个只有 `type` 没有 `text` 的 content 在 proto3 里是"被设置了"。
        append_message(&mut out, 4, &item);
    }
    out
}

/// 从 `Meta` 里取 `payload`（字段 6）。
fn decode_meta_payload(data: &[u8]) -> Result<Option<Vec<u8>>, MsyncError> {
    let fields = scan_proto_fields(data)?;
    for f in fields {
        if f.num == 6 {
            return Ok(Some(f.raw.to_vec()));
        }
    }
    Ok(None)
}

/// 取 `CommSyncUL.meta.payload`（字段 1 → 6）。
pub fn decode_comm_sync_ul_meta_payload(data: &[u8]) -> Result<Option<Vec<u8>>, MsyncError> {
    let fields = scan_proto_fields(data)?;
    for f in fields {
        if f.num == 1 {
            return decode_meta_payload(f.raw);
        }
    }
    Ok(None)
}

/// 取 `NoticePayload.meta.payload`（字段 2 → 6）。
///
/// ⚠ 与 [`decode_comm_sync_ul_meta_payload`] 的字段号**不同**（这里 `meta` 是 2、
/// 那边是 1）—— 两个消息长得像，抄错字段号的表现是"NOTICE 这条路永远解不出文本"。
pub fn decode_notice_meta_payload(data: &[u8]) -> Result<Option<Vec<u8>>, MsyncError> {
    let fields = scan_proto_fields(data)?;
    for f in fields {
        if f.num == 2 {
            return decode_meta_payload(f.raw);
        }
    }
    Ok(None)
}

// ─────────────────────────────────────────────────────────────────────────────
// 弹幕的**消息类型**：哪一条才可能是口令
// ─────────────────────────────────────────────────────────────────────────────

/// `MessageBody.ext`（字段 5，`repeated KeyValue`）里那个键：消息类型。
pub const EXT_KEY_MSG_TYPE: &str = "msgType";

/// `msgType` 的实拍取值 —— **观测**，不是文档。
///
/// # 依据
///
/// 2026-09-23 实拍（`mcloud-capture/har/live-room-raw-2026-09-23.json`：两条 WS 连接
/// id 632 / 1127，服务端共推了 17 条带文本的帧）。**没有**任何上游文档或 H5 源码可对：
/// 抓包里直播间那三个 JS 产物只有 102 字节的壳，`docs/capture/app-live-ws-samples.txt`
/// 也只把 `msgType` 当 `ext` 的**例子**列了一次（那一例正好是 `4`）。
/// 那一批帧（脱敏后）留在 `test/fixtures/live-ws-danmaku-2026-09-23.json`。
///
/// | 取值 | 实拍文本 | 判 |
/// |---|---|---|
/// | `2` | `加入直播间` | **事件**：入场提示 |
/// | `4` | `小红花口令：一键9图出圈` | **口令** —— 17 条里唯一真的那一种 |
/// | `5` | `点赞` | **事件**：点赞 |
/// | `6` | `中国移动云盘` | **事件**：系统提示（文本**就是主播昵称本身**） |
/// | `7` | `送了1个火箭` / `送了1个棒棒糖` / `送了1个鲜花` | **事件**：礼物 |
///
/// ⚠⚠ `1` / `3` / `0` **一次都没观测到** ⇒ 表里没有它们，判据也**不**把它们当事件
/// （见 [`is_event_message`]）。这是刻意的，见那里关于"两个方向的代价不对称"的说明。
pub const MSG_TYPE_JOIN: u64 = 2;
/// 口令。实拍里 `小红花口令：…` 全是它。
pub const MSG_TYPE_KOULING: u64 = 4;
/// 点赞。
pub const MSG_TYPE_LIKE: u64 = 5;
/// 系统提示（实拍文本恰是主播昵称 `中国移动云盘`）。
pub const MSG_TYPE_SYSTEM: u64 = 6;
/// 礼物（`送了1个…`）。
pub const MSG_TYPE_GIFT: u64 = 7;

/// 已知**不是口令**的那几个 `msgType`（[`is_event_message`] 的判据表）。
pub const EVENT_MSG_TYPES: [u64; 4] =
    [MSG_TYPE_JOIN, MSG_TYPE_LIKE, MSG_TYPE_SYSTEM, MSG_TYPE_GIFT];

/// 这条推送是不是**弹幕流里的事件消息**（入场 / 点赞 / 系统 / 礼物），而不是人发的话。
///
/// # 判据只有一条：上游 `ext.msgType` 落在已知的**事件**取值上
///
/// ⚠ **不要**换成形状猜测（"中文四个字就算口令"这类）：实拍里事件文本
/// （`加入直播间` 五个字、`点赞` 两个字、`中国移动云盘` 六个字）与真口令
/// （`一键9图出圈` 五个字）在**字形上分不开**，只有 `msgType` 分得开。
///
/// ⚠ **也不要**按"发信人是不是主播"过滤：口令是**谁都能用**的，而实拍里
/// `小红花口令：…` 那 4 条的发信人都是普通观众（`cmlive_c_<手机号>` / `wechat10086_…`），
/// **没有一条**来自主播账号。
///
/// # ⚠ 未知取值一律**放行**（返回 `false`）
///
/// `None`（这一帧没带 `msgType`）与表外的取值都当作"可能是口令"。
/// 两个方向的代价**不对称**：
///
/// - 严过头 ⇒ 把**真口令**滤掉。口令只在直播那一小时有效，
///   **没有第二次机会**（这一条写在 `live_kouling.rs` 的模块头）。
/// - 松一点 ⇒ 多投一条废口令。代价是一次 `200108`，
///   而实拍证明那一枪**零消耗**（什么都不扣）。
///
/// ⇒ 所以这是一张**事件黑名单**，不是"口令白名单"。
pub fn is_event_message(msg_type: Option<u64>) -> bool {
    matches!(msg_type, Some(t) if EVENT_MSG_TYPES.contains(&t))
}

/// 一帧推送解出来的弹幕：**文本** + **上游的消息类型**。
///
/// 两样一起返回而不是分两次解：`msgType` 就在同一个 `MessageBody` 里，
/// 分两次解要扫两遍字节，而且两次解出来的可能**不是同一条**（多 `meta` 的帧上会分叉）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DanmakuFrame {
    /// `MessageBody.contents[0].text`（空串 = 这一帧没有文本）。
    pub text: String,
    /// `MessageBody.ext` 里 `msgType` 的取值（`None` = 这一帧没带这个键）。
    pub msg_type: Option<u64>,
}

/// 从一帧推送里取出弹幕文本（**多个 body 拼成一条**），外加它的 `msgType`。
///
/// 这是原实现那套**三级兜底**的等价物（`live-room-kouling.ts` 的 `onMessage`）：
///
/// ```text
/// SYNC(0)   → CommSyncUL.meta.payload → MessageBody.contents[0].text
///              ↳ 解不开就退到「整段 payload 直接当 MessageBody」
/// NOTICE(2) → NoticePayload.meta.payload → MessageBody.contents[0].text
///              ↳ 同样退一级
/// ```
///
/// # 为什么必须保留兜底
///
/// 服务端推的是哪一层**没有实拍**（`chatRoomId: null` ⇒ WS 结构性不存在）。
/// 原实现是先写死了 SYNC 那一层、后来撞上解不出文本才补的兜底 —— 也就是说，
/// 这一级的存在本身就是"猜错过一次"的证据。纯协议实现如果只留一条路，等于把
/// 那次踩坑重新踩一遍。
///
/// 返回空文本表示"这一帧没有文本"（不是错误）：心跳、无 payload 的控制帧
/// 都会走到这里，而它们**本来就不该被当成异常**。
///
/// # ⚠ 一处**已知**缺口：payload 里有多条 `meta` 时只读第一条
///
/// `decode_notice_meta_payload` 取的是**第一个** `payload.2`，而实拍里 payload
/// 可以带**多条** meta（17 条带文本的帧里有 2 条如此：`conn 1127` 的 `msg 8` / `msg 58`），
/// 那条弹幕在**靠后的** meta 里 ⇒ 这里读不到它（返回空文本）。
/// TS 那边用 protobufjs 解 singular 字段，取的是**最后一条**，反而读得到。
/// ⚠ 这**不是**本轮改的那件事（本轮只排除事件消息），所以**没动**它 ——
/// 见 `test/fixtures/live-ws-danmaku-2026-09-23.json` 的 `$knownGap`。
pub fn extract_danmaku_frame(command: u8, payload: &[u8]) -> Result<DanmakuFrame, MsyncError> {
    if payload.is_empty() {
        return Ok(DanmakuFrame::default());
    }
    if command != CMD_SYNC && command != CMD_NOTICE {
        return Ok(DanmakuFrame::default());
    }

    let outer = if command == CMD_SYNC {
        decode_comm_sync_ul_meta_payload(payload)
    } else {
        decode_notice_meta_payload(payload)
    };
    match &outer {
        // ⚠ 用 `&outer` + `ref` 借用而不是按值解构：`outer` 下面还要再用一次
        // （兜底失败时把外层错误报上去）。按值解构会把 `Vec<u8>` move 走，
        // 那段 `match outer` 就编译不过。
        Ok(Some(inner)) if !inner.is_empty() => {
            if let Some(frame) = content_frame(inner) {
                return Ok(frame);
            }
        }
        _ => {}
    }

    // 兜底：把整段 payload 当 MessageBody 再解一次。
    if let Some(frame) = content_frame(payload) {
        return Ok(frame);
    }
    match outer {
        // 外层解不开、兜底也没解出文本 —— 这时才把外层的错误报上去：
        // 它至少比"空文本"多一条线索（前 64 字节 hex 由调用方打）。
        Err(err) => Err(err),
        Ok(_) => Ok(DanmakuFrame::default()),
    }
}

/// 只要文本那一半（[`extract_danmaku_frame`] 的旧签名）。
///
/// ⚠ 拿它做"这条是不是口令"的判断是**不够的** —— 事件消息也有文本。
/// 生产路径（`live_ws` 的状态机）用的是 [`extract_danmaku_frame`] +
/// [`is_event_message`]；这个只留给出入不需要消息类型的地方与既有测试。
pub fn extract_danmaku_text(command: u8, payload: &[u8]) -> Result<String, MsyncError> {
    Ok(extract_danmaku_frame(command, payload)?.text)
}

/// 把 `MessageBody.contents`（repeated）里的文本**拼成一条**，外加 `msgType`。
///
/// # ⚠ 为什么不是"只取第一条"
///
/// 抓包实拍：**一条弹幕可以有多个 body，文本要拼起来**
/// （`../mcloud-back/docs/capture/app-live-ws-samples.txt:534-537`：
/// `{"payload":{"bodies":[{"msg":"小红花口令：…","type":"txt"}]}, …}`
/// —— 文件里紧跟着那句「一条弹幕可以有**多个** body，要拼起来」）。
/// 只取第一条的表现是：`小红花口令：` 与 `一键9图出圈` 落在两个 body 上时，
/// 前半段被丢掉 ⇒ **一条口令都解不出来**，而丢掉的那一侧没有第二次机会。
///
/// # 拼接**不加分隔符**
///
/// body 是同一条消息被切开的碎片（实拍那个例子里 `小红花口令：` 就是前缀本身），
/// 加一个空格会把 `小红花口令：` + `一键9图出圈` 拼成 `小红花口令： 一键9图出圈`
/// —— 那不是实拍见过的形状。首尾相接才是（`:141` 那条整条就是这个结果）。
fn content_frame(data: &[u8]) -> Option<DanmakuFrame> {
    let body = decode_message_body(data).ok()?;
    if body.contents.is_empty() {
        return None;
    }
    let text: String = body.contents.iter().map(|c| c.text.as_str()).collect();
    Some(DanmakuFrame {
        text,
        msg_type: msg_type_of(data),
    })
}

/// 从 `MessageBody.ext`（字段 5）里读 `msgType`。
///
/// # 为什么单独扫一遍，而不是把它加进 [`MessageBody`]
///
/// [`MessageBody`] 是公开结构、还配着 [`encode_message_body`] 的往返断言。
/// 往里加一个 `ext` 字段意味着编码侧也得跟着写（否则往返就对不上），
/// 而口令这条链**只需要读一个键**。加字段是改协议层的形状，读一个键不是。
///
/// # 判据落在**字段号**上，不落在 `ValueType` 上
///
/// `KeyValue` 是 `{key = 1(string), type = 2(ValueType), oneof value {varintValue = 3 …}}`
/// （`proto.ts` 逐字）。实拍里 `msgType` 的 `type` 是 **`INT(2)`**（入场/点赞/系统/口令）
/// 或 **`LLINT(4)`**（礼物那一批），值都在**字段 3**。
/// ⇒ 拿 `type` 当门会让 `LLINT` 那 4 条整批漏掉（表现是"礼物被当成口令"那一类
/// 反过来的一半：本来该滤掉的没滤掉）。
fn msg_type_of(data: &[u8]) -> Option<u64> {
    let fields = scan_proto_fields(data).ok()?;
    for field in fields {
        if field.num != 5 || field.wire != WIRE_BYTES {
            continue;
        }
        let mut key: Option<String> = None;
        let mut value: Option<u64> = None;
        for kv in scan_proto_fields(field.raw).ok()? {
            match kv.num {
                1 if kv.wire == WIRE_BYTES => key = Some(kv.as_str()),
                3 if kv.wire == WIRE_VARINT => value = Some(kv.varint),
                _ => {}
            }
        }
        if key.as_deref() == Some(EXT_KEY_MSG_TYPE) {
            return value;
        }
    }
    None
}

#[cfg(test)]
mod tests;
