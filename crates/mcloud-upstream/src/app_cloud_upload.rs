//! 网盘「传一个文件上去」这一族的**前三枪**（第四枪在 [`crate::cloud_trash`]）。
//!
//! ```text
//! ① POST {PERSONAL_KD}/hcy/file/create    声明「我要传一个文件」，拿 fileId / uploadId
//! ② PUT  <uploadUrl>                      发**原始字节**（预签名直传）
//! ③ POST {PERSONAL_KD}/hcy/file/complete  告诉上游这段传完了
//! ```
//!
//! 对应参考实现 `internal/upstream/app_cloud_upload.go`（711 行，本文件逐条移植）。
//! 旧实现是 `packages/core/src/apis/file.ts`（四口）+ `utils/file.ts`（编排），
//! 调用点在 `runners.ts:43/95/115/318` 与 `after.ts:135`（清理）。
//!
//! # ⚠ 第四枪不在这里
//!
//! `hcy/recyclebin/batchTrash` 已经在 [`crate::cloud_trash`] 落过（带它自己的测试），
//! 本文件**不再实现一遍**。两处共用的那份「文件操作头」也只有**一处实现**：
//! [`cloud_file_op_headers`]（按渠道 × 操作类型参数化），而
//! [`crate::cloud_trash::cloud_file_op_headers_web`] 只是它在 (web, upload) 这一档上的**转发**。
//!
//! # 证据分两半（2026-09-25 之后：这一族已经从「零真打」变成「端到端跑通过」）
//!
//! - **四枪里只有 `file/create` 有实拍，而且实拍的是「建目录」那一型**
//!   （`../mcloud/test/fixtures/http/3d.json` 两条：`type:"folder"`，响应 `data.uploadId` 与
//!   `data.partInfos` 都是 `null`、`exist:true`）。⇒ 它只钉得住三件事：
//!   ① URL 与**信封形状**（`{code:"0000"(字符串), message, success, data}`，载荷在 `data`）；
//!   ② `data.fileId` 这一格确实存在；③ 这台主机收这个客户端发的请求。
//! - **「上传文件」那一型、`PUT`、`complete`、`batchTrash` 至今无 har 实拍**，但
//!   **不是「一个真打都没有过」**：2026-09-25 起 `app-task` 的 106 / 522 / beiyong1
//!   那几条玩法用这一族端到端跑通（传进去 + 链尾 `batchTrash` 收走，见 R27 与
//!   `mcloud/docs/capture/`）。⚠ 仍**零样本**的是 `exist:true` 与 `rapidUpload:true`
//!   那两支（每次都传随机名 + 随机内容 ⇒ 撞同名与撞内容哈希碰不上）。
//! - ⭐ `batchTrash` 打在**非空目录**上的行为（**R7**）已于 2026-09-26 真跑闭合：
//!   四趟都是「上游认了 + 内容跟着消失 + 没有退回到根目录」；逐字读数与边界见
//!   `../mcloud/docs/capture/app-cloud-folder-trash-samples.txt`。
//!
//! 所以本层落码的口径是：**有真跑的写真跑、只有实拍的写实拍、两者都没有的标「零样本」**，
//! 测试只钉「发得出 + 形状对」。
//!
//! # 预签名 `uploadUrl` 与凭据同类：**绝不进日志、夹具、错误文本**
//!
//! `create` 回的那个 URL 是**带签名的写入入口**（本仓从未抓到过长什么样）。所以：
//! ① 它只作为 [`CloudFileCreated::upload_url`] 交回调用方，本文件的任何函数都**不**把它
//! 拼进 `Error` 的文本；② 夹具与测试里只出现 `SYNTH-*` 合成值。
//!
//! # 与同一台主机上那一口 `hcy/file/list` 用了**两套画像**（R16 / R27，2026-09-26 收口）
//!
//! 那一口用 iOS 的 profile（`WithAppHeaders` 那一整族 `x-yun-*`），本族**没有 har 实拍**
//! （只有 2026-09-25 起的真跑）⇒ 按**旧 TS** 发：主 http 实例的静态头加
//! [`cloud_file_op_headers`] 那三档。同一账号同一趟里两套并存是**已知不一致**，不是笔误。
//!
//! 谁对，探针已经答过（三趟只读、体逐字不变）：**两套今天都通**，而「通不通」既不取决于
//! 装配、也不取决于渠道号的值 —— 唯一承重的是 `x-yun-client-info` 的**值**：设备指纹串被
//! 这台主机判 `请求头不合法(00010003)`，网盘侧那一串判「请求成功」。
//!
//! # 与参考实现的三处**已知分歧**（本仓口径，逐条都有理由）
//!
//! 1. **`User-Agent` 不在这里设**。参考实现给这一族写死
//!    `CaiyunAndroidWebUA`（见 [`crate::smid::CAIYUN_ANDROID_WEB_UA`]），PC 档覆盖成裸
//!    `Mozilla/5.0`，`PUT` 那一枪也是它。本仓的 UA 归 [`crate::ClientProfile`] 的
//!    `effective_user_agent()`，而 `ReqwestClient` **无条件**先设一枚 ⇒ 在这儿再设就是
//!    **第二根同名头**（上游读哪一根未测试，正是本仓 `content-type` 踩过的那个坑）。
//!    要复现参考行为：**组装层**把 profile 的 UA 设成 `CAIYUN_ANDROID_WEB_UA`。
//! 2. **`authorization` / `deviceid` 不在这里设**：由发送层按域名与 profile 注入。
//!    在请求构造里再设一遍**不会覆盖**，而是发出去两根同名头 —— 见
//!    [`cloud_file_op_headers`] 的说明。
//! 3. **渠道是 Rust 枚举** ⇒ 参考实现那个「未识别的渠道值走 web 兜底」的 `default:` 分支
//!    在本层**不可表达**（理由见 [`CloudChannel`]）。
//!
//! ⚠ 另有一处**刻意收紧**：参考实现的 `Envelope.EnsureOK` 把「没有 `code` 这一格」也判成功
//! （有些接口回裸数组），而本层只认字符串 `"0000"`、缺格即失败 —— 与
//! [`crate::cloud_trash::parse_batch_trash`] 同一口径（那一口有 2026-09-26 的真机回执钉着）。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, HttpResponse, Result};

use crate::cloud_trash::{CLOUD_CHANNEL_WEB, CLOUD_CLIENT_INFO_WEB, CLOUD_CONTENT_TYPE, CODE_OK};
use crate::endpoints::{AUTH_DOMAINS, PERSONAL_KD};

// ══════════════════════════════ 渠道与操作类型 ══════════════════════════════

/// 「哪个端在传」—— 三档渠道（旧 TS `apis/file.ts:3-40` + `constants.ts:66-70`）。
///
/// ⚠ 参考实现里它是**字符串枚举**，可以传任意串，未识别的值落 web 分支；本层用 Rust 枚举
/// ⇒ 「未识别的渠道」在类型上不可表达。这是**刻意**的：那一档在旧 TS 里是 `default:` 兜底，
/// 而本仓三档都有出处（android `10000023` / web `10000034` / PC `10200153`），
/// 让第四档可以拼出来只会多一处「传错端能静默通过」的地方。
/// 代价是：真出现第四档时，这里**编译不过**而不是静默按 web 发 —— 那是可接受的一侧。
///
/// ⚠ 与 iOS App 的渠道号 `10000020` **不是一个值族**：那是登录族的渠道号，
/// 这里是旧 TS 文件操作三档里的 android 档。别互相搬。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudChannel {
    /// 旧 TS 的**默认档**（`getFileOpHeader` 的 `channelSrc` 缺省值，`apis/file.ts:14`）
    /// —— 也就是说 `uploadRandomFile` 那条链默认按安卓端传。
    #[default]
    Android,
    /// 网页端档。旧实现的**删除**那一口固定用它（`apis/file.ts:114` 的
    /// `channelSrc: "10000034"`），与 [`crate::cloud_trash`] 一致。
    Web,
    /// PC 客户端档（`huaweiChannelSrc` = `10200153`）。旧实现里玩法 113 在上传前先
    /// `refreshAuthToken` 再按 PC 档传 —— 那是那个玩法自己的要求，不是这一族的默认。
    Pc,
}

impl CloudChannel {
    /// 这一档的 `x-yun-app-channel` / `x-huawei-channelSrc` 值。
    pub fn value(self) -> &'static str {
        match self {
            CloudChannel::Android => CLOUD_CHANNEL_ANDROID,
            CloudChannel::Web => CLOUD_CHANNEL_WEB,
            CloudChannel::Pc => CLOUD_CHANNEL_PC,
        }
    }
}

/// android 档的渠道号（旧 TS `apis/file.ts`）。
pub const CLOUD_CHANNEL_ANDROID: &str = "10000023";
/// PC 档的渠道号（旧实现 `huaweiChannelSrc`）。
pub const CLOUD_CHANNEL_PC: &str = "10200153";

/// 操作类型。旧 TS 只有两档（`"backup" | "upload"`）。
///
/// ⚠ 它**只对 android 档有效果**：另两档不看这个参数（`apis/file.ts:17-40` 的两个分支里
/// `opType` 只被 android 分支读）。所以「backup 在 web 档会怎样」这一格的答案是
/// 「和 upload 一样」—— 那是从代码形状读出来的，**不是实测**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudOpType {
    /// 普通上传：`x-yun-op-type: 1`、`x-yun-sub-op-type: 100`。
    #[default]
    Upload,
    /// 备份上传：`op-type: 2`、`sub-op-type: 200`。旧 TS `task-expansion.ts:12` 用这一档。
    Backup,
}

/// 把 [`CloudOpType`] 折成 android 档那两个码（`opCode` 与 `opCode * 100`）。
///
/// 单独一个函数是为了让「其它值 ⇒ upload」这一格可断言：参考实现那边
/// `CloudOpType` 是字符串类型、空串就是 upload，而本层是枚举 ⇒ 默认值由
/// `#[derive(Default)]` 表达（`CloudOpType::Upload`）。
fn android_op_codes(op: CloudOpType) -> (i64, i64) {
    let op_code: i64 = match op {
        CloudOpType::Backup => 2,
        CloudOpType::Upload => 1,
    };
    (op_code, op_code * 100)
}

// ══════════════════════════════ 请求体里的固定字面量 ══════════════════════════════

/// `create` 的 `type`：这一族只传文件。
///
/// ⚠ 实拍那两条是 `"folder"`（建目录），「这一口认不认 `file`」由那两条**证不了**。
/// ⇒ 它由 2026-09-25 起的真跑证实（106 / 522 / beiyong1 那几条玩法每次都发这一型，
/// 且链尾删得掉，见 R27）。
pub const CLOUD_FILE_TYPE_FILE: &str = "file";

/// `create` 的**建目录**那一型（见 [`folder_create_request`]）。
///
/// ⚠ 它的出处是**实拍**（`3d.json` 两条），不是真跑：本仓的生产链到今天**一处都不建目录**。
pub const CLOUD_FILE_TYPE_FOLDER: &str = "folder";

/// **上传**那一型的重名策略（`apis/file.ts:65`）。实拍两条是 `"refuse"`（建目录用 refuse）
/// ⇒ 两档各有出处，不是猜的。
pub const CLOUD_RENAME_MODE_AUTO: &str = "auto_rename";

/// **建目录**那一型的重名策略：同名目录已存在时上游不另起一个，而是在响应里给
/// `data.exist:true` + **已有那枚** `fileId`。
///
/// ⭐ 这一句 2026-09-26 起**有本仓自己的读数**：同一个目录名连发两发 `file/create`
/// ⇒ 第二发回 `code:"0000" + success:true`（与第一发逐字同）、`exist:true`、
/// `fileId` 与第一发**同一枚**，而 `file/list` 那一页里这个整名仍只 1 枚
/// （三趟一致，短标 `[refuse=回已有那枚]`；出处 `app-cloud-folder-trash-samples.txt` §8）。
///
/// ⚠ 上面那档读的是**目录型**。「同名**文件**也用 refuse 会怎样」**仍零样本**：本族的
/// 文件型走 [`CLOUD_RENAME_MODE_AUTO`]，refuse 只在目录型上出现过 —— 别把这一枚搬去传文件。
pub const CLOUD_RENAME_MODE_REFUSE: &str = "refuse";

/// `contentHashAlgorithm`：`SHA256`（大写，`create` 与 `complete` 两处都用它）。
pub const CLOUD_HASH_ALGORITHM: &str = "SHA256";

/// **请求体里那个 `contentType` 键**的值：`application/oct-stream`（少一个 `e`）。
///
/// ⚠ 这是旧 TS 的字面量（`apis/file.ts:68`），看着像拼错，但**上游收到过这一串**。
/// 与 [`CLOUD_PUT_CONTENT_TYPE`]（`PUT` 那一条，拼写正常）**不是一处**：
/// 一个是 MIME 键的值、一个是真实 HTTP 头。改任何一个都等于改一次接口 ⇒ 照抄。
pub const CLOUD_CREATE_CONTENT_TYPE: &str = "application/oct-stream";

/// `PUT` 那一枪**真实的** `content-type` 头（`apis/file.ts:101`）。
pub const CLOUD_PUT_CONTENT_TYPE: &str = "application/octet-stream";

/// `PUT` 那一枪的 `origin`（旧 TS `apis/file.ts:99`）。
pub const CLOUD_PUT_ORIGIN: &str = "https://yun.139.com";

/// `PUT` 那一枪的 `referer`（旧 TS `apis/file.ts:100`，**带结尾斜杠**）。
pub const CLOUD_PUT_REFERER: &str = "https://yun.139.com/";

/// 两型的 `create` 都在这一台上。
const FILE_CREATE_PATH: &str = "/hcy/file/create";

/// `complete` 的路径。
const FILE_COMPLETE_PATH: &str = "/hcy/file/complete";

/// 三档 client-info 串（旧 TS `constants.ts:21-27`）。
///
/// ⚠ 它们是**素材字面量**：整串照抄，别按「我们这一台设备」重拼。三档的版本号各不相同
/// （App `12.6.0` / Web `7.15.3` / PC `8.3.3.20250521`），与仓库里那几个 `12.6.0`
/// 常量（`IOSAppVersion` / `TASK_CLIENT_VERSION`）只是**恰好同号**。
///
/// ⚠ web 那一枚的**唯一定义**在 [`crate::cloud_trash::CLOUD_CLIENT_INFO_WEB`]（第四枪先落的
/// 地），这里引用它而不是抄第二份 —— 两处各写一份，早晚会分叉。
pub const CLOUD_CLIENT_INFO_ANDROID: &str =
    "1|127.0.0.1|1|12.6.0|Xiaomi|22041216C||02-00-00-00-00-00|android 14|1080X2360|zh||||032|0|";
/// PC 档的设备信息串（见 [`CLOUD_CLIENT_INFO_ANDROID`] 那条）。
pub const CLOUD_CLIENT_INFO_PC: &str = "||11|8.3.3.20250521|PC|TGktMjAyNDA5MTcxMDA2||| Windows 11 (11.0)|1920X1020|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||";

// ══════════════════════════════ 请求头：三档 `getFileOpHeader` ══════════════════════════════

/// 给文件操作那一族补头 —— **这一族唯一的一份实现**（三档渠道 × 两种操作类型）。
///
/// 它做两件事，顺序**有意如此**：
///
/// 1. 先发 base 三枚（`apis/file.ts:15`，三档都有）：`x-yun-api-version: v1`、
///    `x-yun-app-channel: <渠道>`、`x-yun-op-type: 1`。
/// 2. 再按渠道叠上那一档自己的头。
///
/// # 哪些头**不**在这里设，以及为什么
///
/// - `authorization` / `deviceid`（以及 `jwttoken`）：由发送层
///   （[`crate::client::ReqwestClient`]）按域名与 profile 注入。在这儿再设一遍
///   **不会覆盖**，而是发出去**两根同名头**（reqwest 的 `.header()` 是追加），
///   而上游读哪一根未测试 —— 这就是本仓 `content-type` 曾经踩过的那个坑。
///   参考实现的 `AiAvatarHeaders` 收 `authorization` / `deviceID` 两个开关；
///   本仓把这两个开关挪到了发送层，语义不变（空串 = 不发那一枚）。
/// - `User-Agent`：同上，归 profile（见模块头「已知分歧」第 1 条）。参考实现的 PC 档
///   要把它覆盖成裸 `Mozilla/5.0`、android/web 档保持 `CaiyunAndroidWebUA` ——
///   在本仓要在**组装层**用 `ClientProfile::user_agent` 表达。
/// - `content-type`：**恒发**（参考实现把它放在主 http 实例的静态头里、与有没有体无关），
///   值是 [`CLOUD_CONTENT_TYPE`]。⚠ 它必须**最后**覆盖式地设：`x-huawei-channelSrc`
///   这些都不带 `content-type`，而万一哪一档又补了一枚，append 语义就会发出两根线。
///
/// # op 只对 android 档有影响
///
/// PC 档与 web 档都不读 `op` ⇒ 「backup 在这两档」与 upload **逐枚头相同**。
/// 那是从旧代码分支形状读出来的（不是实测），所以测试里把这两档各跑一遍备用例钉住。
///
/// ⚠ 备份档要**覆盖** base 的那一枚 `x-yun-op-type: 1` ⇒ 走 `set_cloud_header`。
/// 表现成两号线不是「难看」而已：旧实现发的是 `2`，而 append 会同时发 `1` 与 `2`
/// （上游读哪一枚未测试），备份玩法的判定就会飘。
pub fn cloud_file_op_headers(
    req: HttpRequest,
    channel: CloudChannel,
    op: CloudOpType,
) -> HttpRequest {
    let channel_src = channel.value();
    // base 三枚：三档都有（`apis/file.ts:15`）。
    let req = req
        .header("x-yun-api-version", "v1")
        .header("x-yun-app-channel", channel_src)
        .header("x-yun-op-type", "1");
    let req = match channel {
        CloudChannel::Pc => req
            .header("x-yun-market-source", "001")
            .header("x-yun-module-type", "100")
            .header("x-yun-svc-type", "1")
            .header("x-yun-client-info", CLOUD_CLIENT_INFO_PC),
        CloudChannel::Android => {
            let (op_code, sub_op_code) = android_op_codes(op);
            let req = req
                .header("x-huawei-channelSrc", channel_src)
                .header("x-yun-client-info", CLOUD_CLIENT_INFO_ANDROID)
                .header("x-yun-sub-op-type", sub_op_code.to_string());
            set_cloud_header(req, "x-yun-op-type", &op_code.to_string())
        }
        // ⚠ `x-yun-device-id` 在旧 TS 里被赋成**设备信息串**（`apis/file.ts:41`），
        // 看着像笔误，但上游收到过的就是这一串 ⇒ 照抄，别「修对」。
        CloudChannel::Web => req
            .header("x-huawei-channelSrc", channel_src)
            .header("x-yun-client-info", CLOUD_CLIENT_INFO_WEB)
            .header("x-yun-device-id", CLOUD_CLIENT_INFO_WEB),
    };
    set_cloud_header(req, "Content-Type", CLOUD_CONTENT_TYPE)
}

/// 覆盖式地设一枚头（先按大小写不敏感摘掉同名，再追加）。
///
/// 存在的理由：**旧 TS 的 `{...base, k: v}` 是覆盖**，而 [`HttpRequest::header`] 是**追加**
/// —— 不写成覆盖，「改一个头的值」会静默变成「加一根线」，而上游读哪一根未测试。
///
/// 它必须**留在链的最后一步**：先 set 再 header 同名会又把那一枚加回来。
fn set_cloud_header(req: HttpRequest, name: &str, value: &str) -> HttpRequest {
    let mut req = req;
    req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    req.header(name, value)
}

/// 参考实现那条「按主机后缀决定 `PUT` 要不要带 `authorization`」的规则
/// （旧 TS `shared/src/http/base.ts:10` 的 `AUTH_DOMAINS`；后缀表复用
/// [`crate::endpoints::AUTH_DOMAINS`]，不另抄一份）。
///
/// 解析不出主机（相对路径、坏 URL）⇒ **不发**：这是「宁可缺一枚头」的一侧，
/// 因为把凭据发到一个认不出的主机上**不可回滚**，而缺一枚头的表现是这一枪失败。
///
/// ⚠ 本仓没有「预签名 URL 长什么样」的样本 ⇒ 这条规则对 `PUT` 那一枪的实际效果**未验证**。
///
/// # ⚠ 真正上线的那条规则是发送层的 [`crate::endpoints::accepts_basic`]，不是本函数
///
/// [`crate::client::ReqwestClient`] 自己按域名注入 `authorization`（走 `accepts_basic`，
/// 要求**域名边界**上命中），所以请求构造**不能**再设一枚 —— 那会变成两根同名线。
/// 本函数是参考实现那条判据的逐字移植，供调用方与测试判断「某一台主机会不会拿到凭据」。
///
/// 两条规则只在**边界格**上不同：本函数用裸后缀（`evilyun.139.com` 也算命中，那是
/// Go `strings.HasSuffix` 的行为），`accepts_basic` 要求 `.` 边界（不算）。两边都
/// **不命中**的（外站域名、坏 URL、相对路径）行为一致；而真上线的是更严的那一条
/// —— 那一侧是「少发一枚凭据」，可回滚。
pub fn sends_authorization_to(raw_url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(raw_url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    AUTH_DOMAINS.iter().any(|suffix| host.ends_with(suffix))
}

// ══════════════════════════════ 第一枪：POST /hcy/file/create ══════════════════════════════

/// 「要传什么」的调用方输入。
///
/// 刻意做成一个参数结构体而不是六个位置参数：这一枪有**六个可选格**，
/// 六个 `String`/`i64` 连排会让「传错顺序」变成一个能编译通过的 bug。
/// ⚠ 参考实现的 `Channel` / `Op` 零值是**空串**（等于「按默认」）；本层是枚举，
/// 零值由 `#[derive(Default)]` 表达（[`CloudChannel::Android`] / [`CloudOpType::Upload`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudFileCreateInput {
    /// 目标目录 id；空 ⇒ 按旧实现补 `"/"`（根目录）。
    /// 旧 TS `apis/file.ts:57`：`parentFileId: parentFileId || "/"`。
    pub parent_file_id: String,
    /// 文件名。旧 TS 的默认值是 `asign-<8 hex><ext||.png>`，那是**编排层**的事
    /// （见 `utils/file.ts:30`），不在这一层。
    pub name: String,
    /// 字节数。⚠ 上游拿它和 `PUT` 的实际长度对不对得上**未测试**；
    /// 旧实现按「字符串就取 length、字节就取 byteLength」算，两者对 ASCII 内容同值。
    pub size: i64,
    /// 内容的 SHA256 十六进制（小写）。旧实现用 `sha256(file)`，且**同一个串**在
    /// `complete` 那一枪再发一次 ⇒ 两枪必须一致，见 [`file_complete_request`]。
    pub content_hash: String,
    /// 渠道档；零值 = [`CloudChannel::Android`]（旧 TS 的默认参数）。
    pub channel: CloudChannel,
    /// 操作类型；零值 = [`CloudOpType::Upload`]。
    pub op: CloudOpType,
}

/// `parent_file_id || "/"` —— 两型 `file/create`（文件、目录）共用同一个退路。
fn parent_file_id_or_root(parent_file_id: &str) -> &str {
    if parent_file_id.is_empty() {
        "/"
    } else {
        parent_file_id
    }
}

fn file_create_url() -> String {
    format!("{PERSONAL_KD}{FILE_CREATE_PATH}")
}

/// 构造 `POST /hcy/file/create`（**写**：建一个待上传的文件位）。
///
/// body **十键**逐字来自旧 TS `apis/file.ts:56-70`。三件要说清的：
///
/// - **单分片**：`partInfos` 只有一项、`parallelUpload:false`。旧实现也是这样，所以
///   「大于某个尺寸要不要分片」这一格**两版都没答案**（未测试）。本仓的调用方只传
///   几十字节的合成内容，落在这一档里。
/// - **键序**与旧 TS 不同：`serde_json` 的默认 `Map` 按字典序排（Go 的
///   `map[string]any` 同样按字典序 marshal）。实拍那条 `file/create` 的请求体
///   **恰好也是字典序**（`{"fileRenameMode":"refuse","name":…,"parentFileId":…,"type":"folder"}`），
///   所以「上游是否依赖声明序」从样本里**读不出来** ⇒ 不加「必须按声明序」的断言，
///   也不认为无所谓（测试钉的是键集合，不是顺序）。
/// - `exist` / `rapidUpload` 两格的语义见 [`CloudFileCreated`]。
///
/// # 缺 `name` / `contentHash` ⇒ 回错误，**不发**
///
/// 参考实现回的是它那边的「内部不一致」档（`core.NewInternal`）；本仓没有那一档 ——
/// 这一格是**调用方给的输入不成立**，所以落 [`Error::Config`]，与
/// [`crate::cloud_trash::batch_trash_request`] 的空列表同一档。
/// 旧 TS 没有这一道闸（它总是先 sha256、先起名），所以这两格在实拍里从不为空；
/// 在这里挡住是因为**返回错误比发一个空名请求更便宜**：后者会在用户网盘里留下一个
/// 没有名字的文件位，而清理那一族靠 `fileId` 认它。
pub fn file_create_request(input: CloudFileCreateInput) -> Result<HttpRequest> {
    if input.name.is_empty() {
        return Err(Error::Config(
            "file/create 缺 name：旧实现总有一个 asign-* 名字，空名这一格未测试；发出去只会在用户网盘里留一个没有名字的文件位".into(),
        ));
    }
    if input.content_hash.is_empty() {
        return Err(Error::Config(
            "file/create 缺 contentHash：旧实现总是先 sha256 再发，而 complete 那一枪要靠它 —— 少了它两枪就对不上".into(),
        ));
    }
    let CloudFileCreateInput {
        parent_file_id,
        name,
        size,
        content_hash,
        channel,
        op,
    } = input;
    let body = serde_json::json!({
        "parentFileId": parent_file_id_or_root(&parent_file_id),
        "name": name,
        "type": CLOUD_FILE_TYPE_FILE,
        "size": size,
        "fileRenameMode": CLOUD_RENAME_MODE_AUTO,
        "contentHash": content_hash,
        "contentHashAlgorithm": CLOUD_HASH_ALGORITHM,
        "contentType": CLOUD_CREATE_CONTENT_TYPE,
        "parallelUpload": false,
        "partInfos": [{
            "parallelHashCtx": { "partOffset": 0 },
            "partNumber": 1,
            "partSize": size,
        }],
    });
    Ok(cloud_file_op_headers(
        HttpRequest::post(file_create_url()).json_body(&body),
        channel,
        op,
    ))
}

// ══════════════════════════════ 第一枪的**另一型**：建目录 ══════════════════════════════

/// 构造 `POST /hcy/file/create` 的**建目录**那一型（**写**）。
///
/// # 为什么不复用 [`file_create_request`] 加一个 `Type` 字段
///
/// 两型的**请求体不是同一份**：实拍那两条（`../mcloud/test/fixtures/http/3d.json`）只有四枚键
/// —— `{"fileRenameMode":"refuse","name":…,"parentFileId":…,"type":"folder"}`，
/// 而文件型带 `size` / `contentHash` / `partInfos` 等六枚，且重名策略是 `auto_rename`。
/// 做成「一个 builder + 类型开关」会让「这一型该发哪几枚键」变成运行时才知道的事，
/// 而那是这一口最容易出错的一格。
///
/// # 今天的消费者只有一支探针
///
/// 本仓生产链**一处都不建目录**（清理那一族只删自己传上去的文件）。这一口的存在是为了
/// 问 R7 那一格：`batchTrash` 打在**非空目录**上到底算什么 —— 那需要先造出一个非空目录，
/// 而这一步就是「造」。⚠ 别把它当成「链上缺一环」补进编排。
///
/// 响应与文件型同一份信封、同一个解析器（[`parse_file_create`]）。
pub fn folder_create_request(
    parent_file_id: &str,
    name: &str,
    channel: CloudChannel,
    op: CloudOpType,
) -> Result<HttpRequest> {
    if name.is_empty() {
        return Err(Error::Config(
            "file/create（folder）缺 name：实拍两条总有一个目录名，空名这一格未测试；发出去只会在用户网盘里留一个无名目录".into(),
        ));
    }
    let body = serde_json::json!({
        "fileRenameMode": CLOUD_RENAME_MODE_REFUSE,
        "name": name,
        "parentFileId": parent_file_id_or_root(parent_file_id),
        "type": CLOUD_FILE_TYPE_FOLDER,
    });
    Ok(cloud_file_op_headers(
        HttpRequest::post(file_create_url()).json_body(&body),
        channel,
        op,
    ))
}

// ══════════════════════════════ 第二枪：PUT <uploadUrl>（预签名直传） ══════════════════════════════

/// 构造那一发 `PUT`（**原始字节**，不是 JSON）。
///
/// # 为什么这一枪不带 hcy 那套文件操作头
///
/// 旧 TS 的 `putFile` **只发**三枚头（`content-type` / `origin` / `referer`，
/// `apis/file.ts:98-104`）加主实例的静态头 —— 它**不套** `getFileOpHeader`。
/// 所以这里没有 `x-yun-*`，也**不该**有：顺手「统一」会把这一枪的画像改成另一套。
///
/// # 为什么不带幂等键、也不重试
///
/// 重放同一条 `PUT` 的语义**未验证**，而代价是用户网盘里多一段坏文件。
/// ⚠ 本仓的 [`HttpRequest`] 没有「可不可重试」这一格（参考实现那边是
/// `IsRetryable`），所以这条口径**无法在类型上表达**：重试策略在调用方，
/// 而调用方的正确做法是**不重试**这一枪。
///
/// # 凭据不在这里设
///
/// `authorization` 是否带上由**发送层按主机**决定（[`sends_authorization_to`] 那条规则，
/// 上线的是 [`crate::endpoints::accepts_basic`]）；`deviceid` 由 profile 注入。
/// 在这里设一枚会在命中时变成两根同名头 —— 见 [`cloud_file_op_headers`] 那段。
///
/// ⚠ `content` 为空（0 字节）也是**带体**：`Some(vec![])` 与 `None` 语义不同
/// （前者发出 `content-length: 0`），判据只看 [`HttpRequest::has_body`]。
pub fn upload_put_request(upload_url: &str, content: &[u8]) -> HttpRequest {
    HttpRequest::put(upload_url)
        .bytes_body(content)
        .header("content-type", CLOUD_PUT_CONTENT_TYPE)
        .header("origin", CLOUD_PUT_ORIGIN)
        .header("referer", CLOUD_PUT_REFERER)
}

/// `PUT` 那一枪的结局：**只看状态码**。
///
/// 预签名 URL 的响应**不是** hcy 那套 JSON 信封（本仓零样本，连状态码都没抓到过），
/// 交给 [`parse_file_complete`] 那一类解析器只会得到「响应不是 JSON」，
/// 而那句话会掩盖真正的失败。⇒ 2xx 就是成了，非 2xx 就是错了。
///
/// # 与 [`crate::session::ensure_http_ok`] 刻意不同：错误文本不带响应体原文
///
/// 参考实现的 `Raw` + `errorFromStatus` 明写「不带响应体原文：上游的错误体里常常
/// 直接回显凭据」。`ensure_http_ok` 会把响应体前 200 字符带进错误 —— 那是它那条链
/// 的口径，不适用于这一枪：它打的是**带签名的写入入口**。所以这里只留状态码，
/// **既不拼 URL 也不拼响应体**。⚠ 这一条今天是成立的，改 [`HttpResponse`] 的用法时回看。
pub fn ensure_put_ok(resp: &HttpResponse) -> Result<()> {
    if (200..300).contains(&resp.status) {
        return Ok(());
    }
    Err(Error::Network(format!(
        "预签名 PUT 失败，HTTP {}（这一枪不带幂等键 ⇒ 不重试；错误文本刻意不含那条 URL 与响应体原文）",
        resp.status
    )))
}

// ══════════════════════════════ 第三枪：POST /hcy/file/complete ══════════════════════════════

/// 构造 `POST /hcy/file/complete`（**写**：告诉上游这段传完了）。
///
/// body **四键**逐字（`apis/file.ts:90`）。`contentHash` 必须与 `create` 那一枪
/// **同一个串** —— 旧实现传的就是同一个变量（`utils/file.ts:80-86`），
/// 服务端比对的正是「`PUT` 上去的字节」与「这两处声明的哈希」，
/// **比对不上会怎样未测试**。
///
/// ⚠ `upload_id` 为空**照样发**（空串）：旧实现不判它空就带进 complete，而它的编排层
/// 把 `null` 折成空串（参考实现 `stringOf`）。「那一格是 `null` 会怎样」**零样本**。
///
/// ⚠ 这一枪的 `channelSrc` 在旧实现里**默认换成 android 档**（`utils/file.ts:84` 的
/// `options.channelSrc || "10000023"`）—— 也就是「create 用 web 档、complete 用 android 档」
/// 是旧实现的真实行为（当调用方没显式给 channelSrc 时）。本层不做那个默认，
/// 由调用方把同一个 channel 传两遍；要复刻旧行为请在编排层显式传。
///
/// # 缺 `fileId` / `contentHash` ⇒ 回错误，**不发**
///
/// 这两个是这一枪的**全部依据**：静默发一枪空请求的表现是「上游回了成功、
/// 但没有任何东西被完成」，那是本仓最忌讳的「不发错、不报错，只是行为不对」。
pub fn file_complete_request(
    file_id: &str,
    upload_id: &str,
    content_hash: &str,
    channel: CloudChannel,
    op: CloudOpType,
) -> Result<HttpRequest> {
    if file_id.is_empty() || content_hash.is_empty() {
        return Err(Error::Config(
            "file/complete 缺 fileId 或 contentHash：这两个是这一枪的全部依据（旧实现不判空就发，本层不发空请求）".into(),
        ));
    }
    let body = serde_json::json!({
        "fileId": file_id,
        "uploadId": upload_id,
        "contentHash": content_hash,
        "contentHashAlgorithm": CLOUD_HASH_ALGORITHM,
    });
    Ok(cloud_file_op_headers(
        HttpRequest::post(format!("{PERSONAL_KD}{FILE_COMPLETE_PATH}")).json_body(&body),
        channel,
        op,
    ))
}

// ══════════════════════════════ 回执：这一族的信封 ══════════════════════════════

/// 这一族的信封：`{code:"0000"(字符串), message, success, data}`，载荷在 `data`。
///
/// ⚠ **与签到族那个数字 `code` 的信封不是同一个解析器**：这里只认字符串 `"0000"`。
/// 实拍那两条（`3d.json`）逐字就是字符串形态，而本仓 `code:0`（数字）与 `code:"0000"`
/// 两种信封都存在 ⇒ 拿错解析器的表现是**永远判失败**。与
/// [`crate::cloud_trash::parse_batch_trash`] 同一口径（那一口有真机回执钉着）。
///
/// ⚠ 参考实现的 `Envelope` 还认 `result` 作为载荷退路（有些接口回 `{code,msg,result}`）。
/// 本层**只认 `data`**：这一族有实拍证明载荷在 `data`（`3d.json`），
/// 兜到 `result` 会让「信封换了形状」这件事静默消失。
struct CloudEnvelope {
    value: Value,
}

impl CloudEnvelope {
    fn parse(body: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(body)
            .map_err(|e| Error::Parse(format!("这一族的回执不是合法 JSON：{e}")))?;
        Ok(Self { value })
    }

    /// 业务码。缺席 ⇒ 空串 ⇒ 与 [`CODE_OK`] 不等 ⇒ 失败（不兜成成功）。
    fn code(&self) -> &str {
        self.value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// 上游那句人话。两个键都认：`msg` 与 `message`（与参考实现的 `Message()` 同口径）。
    fn message(&self) -> &str {
        ["msg", "message"]
            .iter()
            .find_map(|key| self.value.get(*key).and_then(Value::as_str))
            .unwrap_or("（上游没给 msg）")
    }

    /// 判「这一枪成不成」。
    ///
    /// 两条判据，顺序与参考实现**一致**（`success:false` 优先于 `code`）：
    ///
    /// 1. 顶层 `success:false` ⇒ 失败。这两类错误的代价**不对称**：多报一次失败只是噪声，
    ///    把失败当成功会让调用方拿着一张空结果继续往下跑（参考实现的 `EnsureOK` 同句）。
    /// 2. `code` 必须是字符串 [`CODE_OK`]。
    ///
    /// ⚠ 与 [`crate::cloud_trash::parse_batch_trash`] **刻意不同**：那一口**要求**
    /// 顶层有一个 `success:true`（读不到就当失败，因为旧实现拿它当删除的判据）；
    /// 而这一族旧实现在 `create` / `complete` 上**不读** `success` ⇒ 这里只把
    /// `success:false` 当否决位，不要求它存在。
    fn ensure_ok(&self, what: &str) -> Result<()> {
        if self.value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(Error::Upstream {
                code: self.code().to_string(),
                message: format!("{what} 回 success=false：{}", self.message()),
            });
        }
        let code = self.code();
        if code != CODE_OK {
            return Err(Error::Upstream {
                code: code.to_string(),
                message: format!("{what} 失败：{}", self.message()),
            });
        }
        Ok(())
    }

    /// 载荷 `data`（缺席 ⇒ `None`，**不**兜成空对象）。
    fn payload(&self) -> Option<&Value> {
        self.value.get("data")
    }
}

/// `create` 回的那一格 `data`。
///
/// # 四格各自「缺」意味着什么（旧实现的分叉就在这里）
///
/// - [`Self::exist`] ⇒ 旧实现**直接返回 false 且不上传**（`utils/file.ts:52-55`）。
///   **目录型**这一格 2026-09-26 真跑读到过（同名第二发回 `exist:true` + 已有那枚
///   `fileId`，回执仍是成功码，见 [`CLOUD_RENAME_MODE_REFUSE`]）；
///   而「与**文件**已存在是不是同一格语义」**仍未测试** —— 文件型走
///   [`CLOUD_RENAME_MODE_AUTO`]，本仓在文件型上读到 `exist:true` 的样本数是**零**。
/// - [`Self::rapid_upload`] ⇒ 旧实现**跳过 `PUT` 与 `complete`**、只把 `fileId` 记账
///   （`utils/file.ts:57-61`）。也就是「秒传」这一支**不发后两枪**，所以清理那一族
///   拿到的 id 仍然要删得掉。
/// - [`Self::upload_url`] 与 [`Self::file_id`] 任一为空 ⇒ 旧实现判失败
///   （`utils/file.ts:64-67`）。⚠ **判空是编排层的事**：本层只如实交回 `None`，
///   因为「缺 URL」与「缺 fileId」的下一步不同（一个说明响应形状变了，
///   一个是占位没成但也没报错）。
/// - [`Self::upload_id`] 旧实现**不判空**就带进 `complete` ⇒ 秒传之外 `uploadId`
///   为 `null` 时会发生什么，**零样本**。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudFileCreated {
    /// `data.fileId`。
    pub file_id: Option<String>,
    /// `data.uploadId`。
    pub upload_id: Option<String>,
    /// `data.exist`。**三态**：`Some(true)` 撞同名（那个文件不是我们造的）、
    /// `Some(false)` 是我们造的、`None` 表示这一格**形状不对**（缺席 / `null` / 数字）。
    ///
    /// ⚠ 2026-10-04 复审 `up-1` 从 `bool` 改成 `Option<bool>`：这一格是
    /// **"别删用户文件"的唯一依据**（`task_expansion` / `app_task` 靠它决定
    /// `fileId` 要不要进清理名单），而旧写法把"读不出来"兜成 `false` ⇒
    /// 形状一变就会把**用户自己的文件**当成我们的产物删掉。危险的方向只有那一个，
    /// 所以"读不出来"必须与 `false` 分得开。`rapidUpload` 没有这个问题，仍是 `bool`。
    pub exist: Option<bool>,
    /// `data.rapidUpload`。同上。
    pub rapid_upload: bool,
    /// `data.partInfos[0].uploadUrl` —— 预签名写入入口（**绝不进错误文本**）。
    pub upload_url: Option<String>,
}

/// 解析 `create` 的响应。
///
/// 信封与 rabactive 那六口**不同**：载荷在 `data`（不是 `result`），`code` 是字符串
/// `"0000"`。这两件事实拍已证（`3d.json` 两条 folder 型）。
///
/// ⚠ `data` 缺失 ⇒ 解析失败，**不**兜成「全空的 [`CloudFileCreated`]」：
/// 旧实现是 `createResult.data` 直接解构（缺了会抛），把它翻译成「没有 fileId」
/// 会让上层念成「这一枪发成功了但没给 URL」，那是两种不同的病。
pub fn parse_file_create(body: &str) -> Result<CloudFileCreated> {
    let envelope = CloudEnvelope::parse(body)?;
    envelope.ensure_ok("file/create")?;
    let payload = envelope.payload().ok_or_else(|| {
        Error::Parse(
            "file/create 的回执里没有 data 这一格 —— 这一族的载荷在 data（不是 result），缺席说明信封形状变了，不能兜成一个全空结构"
                .into(),
        )
    })?;
    let Some(obj) = payload.as_object() else {
        return Err(Error::Parse(format!(
            "file/create 的 data 不是对象，实得 {payload}（这一口与 rabactive 六口不同信封：载荷在 data）"
        )));
    };
    let mut created = CloudFileCreated {
        file_id: opt_str_at(obj, "fileId"),
        upload_id: opt_str_at(obj, "uploadId"),
        // ⚠ 与 `rapidUpload` 不同：这一格**不做布尔兜底**，读不出来就是 `None`
        //（它是"别删用户文件"的判据，见字段文档）。
        exist: obj.get("exist").and_then(Value::as_bool),
        rapid_upload: cloud_flag(obj, "rapidUpload"),
        upload_url: None,
    };
    // `partInfos` 是数组，取**第一项**的 `uploadUrl`（旧实现 `partInfos?.[0]?.uploadUrl`）。
    // 实拍那条是 `null`（folder 型没有分片）⇒ 「缺席 / null / 空数组」都是合法形状，交回 None。
    if let Some(first) = obj
        .get("partInfos")
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
    {
        created.upload_url = first
            .get("uploadUrl")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    Ok(created)
}

/// 解析 `complete` 的回执：**只判「这一枪成不成」**。
///
/// 旧实现拿到 complete 的响应后**什么都不读**（`utils/file.ts:88-91` 只看有没有抛错），
/// 所以这里也不声明任何字段 —— 等真有一次实拍能读出 `data` 的形状再加。
pub fn parse_file_complete(body: &str) -> Result<()> {
    let envelope = CloudEnvelope::parse(body)?;
    envelope.ensure_ok("file/complete")
}

/// 取一个**可选**字符串：缺键、`null`、非字符串都回 `None`（**不**编造空串）。
fn opt_str_at(obj: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key)?.as_str().map(str::to_string)
}

/// 读一个布尔格：非布尔（含 `null`、数字、缺席）一律 `false`。
///
/// ⚠ **只给"兜成 false 无害"的格子用**（如 `rapidUpload`：兜成 `false` 只是多走
/// 一遍 PUT，且缺 URL 时会明确失败）。`exist` 不在这里 —— 它的 false 意味着
/// "这个文件是我们的、可以删"，兜错方向就是删用户的文件，见
/// [`CloudFileCreated::exist`]。
///
/// ⚠ 刻意**不**「把 `1` 当 true」：旧 TS 用的是 `if (exist)`，那确实会把 `1` 当真值，
/// 但这一族的响应里这两格在实拍中都是**布尔** ⇒ 出现数字就意味着形状变了，
/// 而「形状变了却念成 false」与「念成 true」一样危险，两档都不如让它可断言。
fn cloud_flag(obj: &serde_json::Map<String, Value>, key: &str) -> bool {
    obj.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests;
