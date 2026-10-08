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
//! # ⭐ 2026-10-07：相册自动备份那一支是**六枪**，本文件现在管五枪
//!
//! 真机抓包（`../mcloud-capture/docs/capture/app-album-backup-samples.txt`，
//! iOS App 12.6.0，通路 A ＋ mihomo TUN 按进程接管）把这一族补成：
//!
//! ```text
//! ① POST {PERSONAL_KD}/hcy/file/create            ← 本文件
//! ② PUT  <uploadUrl>（`ot=personal`，op 2/200）   ← 本文件
//! ③ POST {PERSONAL_KD}/hcy/file/complete          ← 本文件
//! ④ POST {PERSONAL_KD}/hcy/backup/createSnapshot  ← 本文件（历史样本里从没出现过）
//! ⑤ PUT  <uploadUrl>（`ot=system`，op 0/0）       ← 本文件
//! ⑥ POST {PERSONAL_KD}/hcy/backup/completeSnapshot← 本文件
//! ```
//!
//! 它与**同端手动上传**（`../mcloud-capture/docs/capture/app-upload-samples.txt` §3/§5/§7）
//! **共用同一组端点**，差别全在**档位**上（三处，抓包 §3 那张表）：
//!
//! | 格子 | 手动支 | 备份支（相册） |
//! |---|---|---|
//! | 定位目录 | `parentFileId`（str33） | **`parentPath`**（str5）—— **有它就没有 `parentFileId`** |
//! | 查重策略 | `force_rename` | **`refuse_duplicate_content`** |
//! | 照片那一发 `PUT` | `1 / 100` | **`2 / 200`**；快照那一发 `PUT` 是 **`0 / 0`** |
//!
//! ⚠ **同一台主机、同一个 `/hcy/` 前缀，真机上是三套头（20 / 25 / 12）**，
//! 差集逐枚见抓包 §7：`file/create`·`complete` 走 **20**（多 `cache-control` 与
//! 那两枚 op 头），`backup/*` 走 **25**（多 `pragma` / `service_identifier_key` /
//! `x-DeviceInfo` / `x-DeviceName` / `x-huawei-channelSrc` / `x-SvcType` / `x-UserAgent` /
//! `x-yun-user-timestamp` 这 8 枚、**不带** op 那两枚），两发 EOS `PUT` 走 **12**
//! （零 mcloud 凭据）。三套的装配**都在本文件**，一律走 `set_cloud_header`（覆盖式）。
//!
//! ⚠ 分套**不是按业务链路分的**（同一条备份链里 20 与 25 并存）——
//! **按什么分，抓包 §7 明写"仍无读数"**，不许写成「SDK 层 vs 界面层」。
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

use crate::app_login::{APP_DEVICE_NAME_B64, APP_X_USER_AGENT, APP_X_YUN_USER_AGENT};
use crate::cloud_trash::{CLOUD_CHANNEL_WEB, CLOUD_CLIENT_INFO_WEB, CLOUD_CONTENT_TYPE, CODE_OK};
use crate::endpoints::{
    AUTH_DOMAINS, BACKUP_COMPLETE_SNAPSHOT, BACKUP_CREATE_SNAPSHOT, PERSONAL_KD,
};

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

/// 操作类型。旧 TS 只有两档（`"backup" | "upload"`），2026-10-07 的真机抓包补上第三档。
///
/// ⚠ 它**只对 android 档有效果**：另两档不看这个参数（`apis/file.ts:17-40` 的两个分支里
/// `opType` 只被 android 分支读）。所以「backup 在 web 档会怎样」这一格的答案是
/// 「和 upload 一样」—— 那是从代码形状读出来的，**不是实测**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudOpType {
    /// 普通上传：`x-yun-op-type: 1`、`x-yun-sub-op-type: 100`。
    ///
    /// ⚠ 这也是**手动上传**那一支的读数（抓包 `app-upload-samples.txt` §5）——
    /// 旧 TS 的默认档，不是"随便挑的零值"。
    #[default]
    Upload,
    /// 备份上传（照片那一支）：`op-type: 2`、`sub-op-type: 200`。
    /// 旧 TS `task-expansion.ts:12` 用这一档；抓包 `app-album-backup-samples.txt` §2 逐字确认。
    Backup,
    /// 备份**快照**那一支（`backup/createSnapshot` 之后那发 `ot=system` 的 `PUT`）：
    /// `op-type: 0`、`sub-op-type: 0`。
    ///
    /// ⭐ 第三档是 2026-10-07 抓包**新读到的**（`app-album-backup-samples.txt` §4/§6）：
    /// 同一端三档并存（手动 `1/100`、照片备份 `2/200`、快照 `0/0`）。
    /// 它在 `android_op_codes` 里天然折出 `0` 与 `0*100 = 0`。
    ///
    /// ⚠ 两个 `backup/*` **控制口**（`createSnapshot` / `completeSnapshot`）**不带**这两枚头
    /// —— 抓包 §6/§7 明写。所以「这一档」只在快照那发 `PUT` 上用，见
    /// `snapshot_op_headers` 那段说明。
    Snapshot,
}

/// 把 [`CloudOpType`] 折成 android 档那两个码（`opCode` 与 `opCode * 100`）。
///
/// 单独一个函数是为了让「其它值 ⇒ upload」这一格可断言：参考实现那边
/// `CloudOpType` 是字符串类型、空串就是 upload，而本层是枚举 ⇒ 默认值由
/// `#[derive(Default)]` 表达（`CloudOpType::Upload`）。
///
/// ⚠ [`CloudOpType::Snapshot`] 折出来是 **`(0, 0)`** —— 这是**真机读数**
/// （抓包 §4：快照那发 `PUT` 是 `x-yun-op-type: 0` / `x-yun-sub-op-type: 0`），
/// 不是"没填所以是 0"。别把它当成"零值兜底"顺手改成 `1/100`：
/// 上游收到的就是两个 0。
fn android_op_codes(op: CloudOpType) -> (i64, i64) {
    let op_code: i64 = match op {
        CloudOpType::Backup => 2,
        CloudOpType::Upload => 1,
        CloudOpType::Snapshot => 0,
    };
    (op_code, op_code * 100)
}

/// 重名/查重档。**手动支与备份支不是同一个值**（2026-10-07 真机抓包，逐字）。
///
/// 出处：`app-album-backup-samples.txt` §3 那张两支对照表 ——
/// 手动支 `force_rename`（str12），备份支 `refuse_duplicate_content`（str24）。
///
/// ⚠ 它与本模块旧 TS 那个 `auto_rename`（[`CLOUD_RENAME_MODE_AUTO`]）**都不是一回事**：
/// 移动端实拍两支里**没有** `auto_rename`，那个值是旧 TS `apis/file.ts:65` 的写法
/// （PC 抓包也是 `force_rename`）。见 [`CLOUD_RENAME_MODE_AUTO`] 那条说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudRenameMode {
    /// `force_rename`：撞同名就改名，**照样传**。手动上传那一支用它。
    ///
    /// 零值取它而不是 [`Self::RefuseDuplicateContent`] 是**故意的**：
    /// 「拒绝重复内容」是一条会**主动失败**的策略（上游按内容哈希判重，撞上就不建），
    /// 谁忘了显式声明而静默落到它上面，表现是"任务莫名失败"，而不是"多传了一个文件"。
    /// 两害相权，零值放在**不会主动拒绝**的那一侧。
    #[default]
    ForceRename,
    /// `refuse_duplicate_content`：**按内容哈希拒绝重复**。相册自动备份那一支用它
    /// （同一张照片重传时不产生第二个对象）。
    RefuseDuplicateContent,
}

impl CloudRenameMode {
    /// 这一档在请求体 `fileRenameMode` 里的字面量。
    pub fn value(self) -> &'static str {
        match self {
            CloudRenameMode::ForceRename => CLOUD_RENAME_MODE_FORCE,
            CloudRenameMode::RefuseDuplicateContent => CLOUD_RENAME_MODE_REFUSE_DUPLICATE_CONTENT,
        }
    }
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
///
/// ⚠⚠ **移动端真机抓包里没有这个值**（`app-upload-samples.txt` §3 手动支是
/// `force_rename`、`app-album-backup-samples.txt` §3 备份支是 `refuse_duplicate_content`）
/// ⇒ 它是**旧 TS 的写法**，本层文件型已经不用它（改用 [`CloudRenameMode`]）。
/// 常量**刻意留着**：它是「本层与旧 TS 在这一格上不一致」的证据本身，
/// 删掉之后下一个人只会看到 `force_rename`，不知道曾经写的是 `auto_rename`。
pub const CLOUD_RENAME_MODE_AUTO: &str = "auto_rename";

/// 手动上传那一支的查重档：`force_rename`（抓包 `app-upload-samples.txt` §3，str12）。
///
/// ⚠ 与 [`CLOUD_RENAME_MODE_AUTO`] **不是同一个值** —— 上游收到过的是这一枚。
pub const CLOUD_RENAME_MODE_FORCE: &str = "force_rename";

/// 相册自动备份那一支的查重档：`refuse_duplicate_content`（抓包
/// `app-album-backup-samples.txt` §3，str24）——「按内容哈希拒绝重复」。
pub const CLOUD_RENAME_MODE_REFUSE_DUPLICATE_CONTENT: &str = "refuse_duplicate_content";

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

/// **备份支** `create` 体里那个 `contentType` 键的值：`application/octet-stream`（拼写正常）。
///
/// # ⚠ 两支两个读数（这是本文件里最容易"顺手统一"掉的一格）
///
/// - **手动支**（`parent_path: None`）走 [`CLOUD_CREATE_CONTENT_TYPE`]
///   = `application/oct-stream`（**少一个 `e`**，旧 TS `apis/file.ts:68` 的字面量）；
/// - **备份支**（`parent_path: Some`）走这一枚 = `application/octet-stream`
///   —— 2026-10-07 真机抓包里 App 发的就是**正确拼写**（`app-album-backup-samples.txt` §3）。
///
/// 两支**两个读数**，所以是两个常量、两条分支，不许合并成一个：
/// 合并就等于替上游"修"一次拼写，而那一枪今天还在用旧值跑。
///
/// ⚠ 它与 [`CLOUD_PUT_CONTENT_TYPE`] **字符串恰好相同、却是两个东西**：
/// 一个是**请求体里 `contentType` 键的值**，一个是**真实 HTTP 头**。
/// 别拿这一枚去替 [`CLOUD_CREATE_CONTENT_TYPE`]，也别把三处合成一个常量
/// —— 两两的出处都不同（旧 TS / 抓包 / 旧 TS 的头）。
pub const CLOUD_CREATE_CONTENT_TYPE_OCTET: &str = "application/octet-stream";

/// `PUT` 那一枪的 `origin`（旧 TS `apis/file.ts:99`）。
///
/// ⚠⚠ **本版不再发这一枚**：2026-10-07 的两发 EOS `PUT`（照片 + 快照）逐枚数下来是
/// **12 枚、没有 `origin` / `referer`**（`app-album-backup-samples.txt` §4）。
/// 常量留着是记录「旧 TS 加过这两枚 CORS 头」这一处**已知分歧**，
/// 不是"待补回去"。要恢复旧行为，得先有一次"带上它们更对"的读数。
pub const CLOUD_PUT_ORIGIN: &str = "https://yun.139.com";

/// `PUT` 那一枪的 `referer`（旧 TS `apis/file.ts:100`，**带结尾斜杠**）。
/// 本版**不发**，理由见 [`CLOUD_PUT_ORIGIN`]。
pub const CLOUD_PUT_REFERER: &str = "https://yun.139.com/";

/// 数据面那一枪的 `upload-complete`（移动端独有，抓包 §4/§5）。
///
/// ⚠ `?1` 里那个问号**是值的一部分**（形状像 HTTP Uploadable 草案那一族）。
/// 服务端读不读它**没有读数**（抓包 §5 明写"这一轮只登记它出现了"）⇒ 照抄。
pub const CLOUD_PUT_UPLOAD_COMPLETE: &str = "?1";

/// 数据面那一枪的 `upload-draft-interop-version`（移动端独有，抓包 §4/§5）。
/// 读不读同样**无读数** ⇒ 照抄。
pub const CLOUD_PUT_UPLOAD_DRAFT_INTEROP_VERSION: &str = "6";

/// `Accept-Language` —— 25 套与 12 套都有这一枚。
///
/// 值取自**同主机同族**的 `client-profiles-ios-app.md` §3.3.3（`Accept-Language: zh-CN`）。
/// ⚠ 相册备份那一篇抓包**只给了头名、没给值**（§7 那张差集表）⇒ 这里不是"抓包读数"，
/// 是"同一台主机、同一个客户端在另一份通路 B 读数里的值"。
pub const CLOUD_ACCEPT_LANGUAGE: &str = "zh-CN";

/// 25 套里的 `service_identifier_key` —— `service_identifier_cloudDisk`。
///
/// 出处：`client-profiles-ios-app.md` §3.3.3（`personal-kd-njs.yun.139.com` 这一族的
/// 读数；同一份文档 §3.3.1 写明这一枚**与主机一一对应**，不是端常量）。
/// ⚠ 相册备份那一篇抓包**没给值**（只给了头名）⇒ 这是按主机查表查出来的那一枚。
/// ⚠ 别拿 `app_login::APP_SERVICE_IDENTIFIER`（`service_identifier_userDomain`）来替：
/// 那是**登录主机** `user-njs` 的值，发错的表现是路由到别的服务。
pub const CLOUD_SERVICE_IDENTIFIER_CLOUD_DISK: &str = "service_identifier_cloudDisk";

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
/// 零值由 `#[derive(Default)]` 表达（[`CloudChannel::Android`] / [`CloudOpType::Upload`] /
/// [`CloudRenameMode::ForceRename`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudFileCreateInput {
    /// 目标目录 id；空 ⇒ 按旧实现补 `"/"`（根目录）。
    /// 旧 TS `apis/file.ts:57`：`parentFileId: parentFileId || "/"`。
    ///
    /// ⚠ **只在 [`Self::parent_path`] 是 `None` 时发**（见那一格的说明）。
    pub parent_file_id: String,
    /// 目标目录**路径**。`Some` ⇒ 走**备份支**：只发 `parentPath`、**不发** `parentFileId`。
    ///
    /// ⭐ 这是两支唯一的"怎么定位目录"读数（抓包 `app-album-backup-samples.txt` §3）：
    /// 手动支 `parentFileId`（str33）、备份支 `parentPath`（str5），**有它就没有另一枚**。
    /// 上游按路径定位（同名目录存在就落进去），不需要先 `file/list` 查 id ——
    /// 那是这一支省掉的一步。
    ///
    /// ⚠ 抓包**只给了长度（5）与"它在"**，值没打（是真实目录名）⇒ 本层原样转发，
    /// 不解析、不改写、也不判它是不是绝对路径。
    ///
    /// ⚠ 这一支的 `contentType` 也随之换档（[`CLOUD_CREATE_CONTENT_TYPE_OCTET`]），
    /// 两支两个读数。
    pub parent_path: Option<String>,
    /// 文件名。旧 TS 的默认值是 `asign-<8 hex><ext||.png>`，那是**编排层**的事
    /// （见 `utils/file.ts:30`），不在这一层。
    pub name: String,
    /// 字节数。⚠ 上游拿它和 `PUT` 的实际长度对不对得上**未测试**；
    /// 旧实现按「字符串就取 length、字节就取 byteLength」算，两者对 ASCII 内容同值。
    pub size: i64,
    /// 内容的 SHA256 十六进制（小写）。旧实现用 `sha256(file)`，且**同一个串**在
    /// `complete` 那一枪再发一次 ⇒ 两枪必须一致，见 [`file_complete_request`]。
    ///
    /// ⚠ 快照那一支也用它（`backup/createSnapshot` 的 `contentHash` 是**快照 zip** 的哈希，
    /// 不是照片的），见 [`CloudSnapshotCreateInput`]。
    pub content_hash: String,
    /// 重名/查重档；零值 = [`CloudRenameMode::ForceRename`]（见那一档的说明）。
    pub rename_mode: CloudRenameMode,
    /// 是否并行上传（体里那一枚 `parallelUpload`，**布尔**，不是字符串）。
    ///
    /// ⚠ 移动端实拍**两支都是 `true`**（抓包 `app-upload-samples.txt` §3 与
    /// `app-album-backup-samples.txt` §3 同值），而旧 TS 写的是 `false`。
    /// 本层**不替调用方定**（契约把这一格交出来了）：两支各按自己的读数传。
    pub parallel_upload: bool,
    /// 本地创建时刻（ISO 形状，抓包 `app-upload-samples.txt` §3：str24，
    /// 形如 `<日期>T<时刻>.999Z`）。`Some` 才发这一键。
    ///
    /// ⚠ 它**不在**旧 TS 的十键里 —— 是移动端两支**都有**、PC 没有的一枚。
    /// 本层**没有时钟**（上游层不碰时间源）⇒ 由调用方给；
    /// 「这一枚上游读不读」**无读数**（抓包只记了形状与长度）。
    pub local_created_at: Option<String>,
    /// 渠道档；零值 = [`CloudChannel::Android`]（旧 TS 的默认参数）。
    ///
    /// ⚠ 真机 iOS App 的渠道是 `10000020`（`app_login::APP_CHANNEL`，抓包 §7 那套 25 头里
    /// `x-yun-app-channel` / `x-huawei-channelSrc` 都是它），而 [`CloudChannel::Android`]
    /// 是旧 TS 的 `10000023` —— **两处口径不一致，且这一档今天表达不出来**。
    /// 本层照参数走（旧 TS 那三档在真机上跑通过），要复现真机得先给 `CloudChannel` 加一档。
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
/// # 两支、两种键集（2026-10-07 真机抓包，`app-album-backup-samples.txt` §3）
///
/// 这一口被**同一端的两种调用方**共用，而两支援的是**不同的键**：
///
/// | 格子 | 手动支（[`CloudFileCreateInput::parent_path`] = `None`） | 备份支（`Some`） |
/// |---|---|---|
/// | 定位目录 | `parentFileId`（空 ⇒ `"/"`） | **`parentPath`**，**不发** `parentFileId` |
/// | 查重 | `fileRenameMode` = 调用方给的档 | 同左（备份支传 `refuse_duplicate_content`） |
/// | `contentType` | [`CLOUD_CREATE_CONTENT_TYPE`]（少一个 `e`，旧 TS） | [`CLOUD_CREATE_CONTENT_TYPE_OCTET`]（抓包的正确拼写） |
/// | `localCreatedAt` | `Some` 才发 | 同左 |
/// | `parallelUpload` | 调用方给 | 调用方给（抓包两支都是 `true`） |
///
/// ⇒ 键集是**运行时**由 `parent_path` 与 `local_created_at` 决定的：
/// 手动支 10 键（+ `localCreatedAt`）、备份支 11 键（+ `localCreatedAt`）。
/// ⚠ 抓包里两支都是 **12 键**，差的第 12 枚是 `userRegion` —— 本仓**没有省/市的来源**
/// ⇒ 这一版**不发**（见下面那一段），这是本文件与抓包**已知的一处缺口**。
///
/// # ⚠ 本版**不发** `userRegion`
///
/// 抓包两支都带 `userRegion: {provinceCode str2, cityCode str3}`
/// （`app-album-backup-samples.txt` §3）。本仓**没有任何省/市来源**
/// （账号落盘里没有这一列，也没有"从 IP 反查"那种能力）⇒ 按纪律**不发这一键**，
/// 也**不编**一个 `""`/`"00"` 去占位：编出来的值会让上游按错的归属地建档，
/// 而那是**不可回滚**的。要补这一格得先有一个真实来源（抓包 `app-upload-samples.txt`
/// §8 也没写它从哪来）。
///
/// # 单分片
///
/// `partInfos` 只有一项、`parallelHashCtx.partOffset` = 0。抓包两支**移动端都带**
/// `parallelHashCtx{partOffset}`（单片也一样），PC 那发只有 `{partNumber, partSize}`
/// ⇒ 本层按移动端发。`h` 那一格在移动端请求侧**无读数**（`app-upload-samples.txt` §3 明写），
/// 所以不发。
///
/// # 键序
///
/// 与旧 TS 不同：`serde_json` 的默认 `Map` 按字典序排（Go 的 `map[string]any` 同样按
/// 字典序 marshal）。实拍那条 `file/create` 的请求体**恰好也是字典序**
/// （`{"fileRenameMode":"refuse","name":…,"parentFileId":…,"type":"folder"}`），
/// 所以「上游是否依赖声明序」从样本里**读不出来** ⇒ 不加「必须按声明序」的断言，
/// 也不认为无所谓（测试钉的是键集合，不是顺序）。
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
///
/// `parent_path: Some("")` 同样拒发：空路径既不是"根"（根要的是 `parentFileId` 那条退路），
/// 也不是一个能定位的目录 —— 发出去的表现是"上游按空路径建了一个我们不知道在哪的文件"。
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
        parent_path,
        name,
        size,
        content_hash,
        rename_mode,
        parallel_upload,
        local_created_at,
        channel,
        op,
    } = input;

    // 定位目录：**两支二选一**（抓包 §3）。`parentPath` 那一支连 `contentType` 一起换档
    // —— 两个读数各写各的，见 `CLOUD_CREATE_CONTENT_TYPE_OCTET`。
    let (locate_key, locate_value, content_type) = match parent_path {
        Some(path) => {
            if path.is_empty() {
                return Err(Error::Config(
                    "file/create 的 parentPath 是空串：空路径既不是根（根走 parentFileId 那条退路），也不是一个能定位的目录 —— 发出去只会在用户网盘里留一个我们不知道落在哪的文件位".into(),
                ));
            }
            ("parentPath", path, CLOUD_CREATE_CONTENT_TYPE_OCTET)
        }
        None => (
            "parentFileId",
            parent_file_id_or_root(&parent_file_id).to_string(),
            CLOUD_CREATE_CONTENT_TYPE,
        ),
    };

    // ⚠ 用 `Map` 逐枚插、而不是一个大 `json!`：`parentPath` / `parentFileId` 与
    // `localCreatedAt` 三格是**条件存在**的（"缺席"与"空串"在这一口不是一回事），
    // 写成 `json!` 再 `remove` 会让"哪些键会缺席"变成读代码才看得见的事。
    let mut body = serde_json::Map::new();
    body.insert("name".into(), Value::String(name));
    body.insert("type".into(), Value::String(CLOUD_FILE_TYPE_FILE.into()));
    body.insert("size".into(), Value::from(size));
    body.insert(
        "fileRenameMode".into(),
        Value::String(rename_mode.value().into()),
    );
    body.insert("contentHash".into(), Value::String(content_hash));
    body.insert(
        "contentHashAlgorithm".into(),
        Value::String(CLOUD_HASH_ALGORITHM.into()),
    );
    body.insert("contentType".into(), Value::String(content_type.into()));
    body.insert("parallelUpload".into(), Value::Bool(parallel_upload));
    body.insert(locate_key.into(), Value::String(locate_value));
    if let Some(created_at) = local_created_at {
        body.insert("localCreatedAt".into(), Value::String(created_at));
    }
    body.insert(
        "partInfos".into(),
        serde_json::json!([{
            "parallelHashCtx": { "partOffset": 0 },
            "partNumber": 1,
            "partSize": size,
        }]),
    );

    Ok(cloud_file_op_headers(
        HttpRequest::post(file_create_url()).json_body(&Value::Object(body)),
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
/// 而文件型带 `size` / `contentHash` / `partInfos` 等六枚，重名策略也与目录型不同
/// （目录型 `refuse`，文件型看 [`CloudRenameMode`]）。
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

/// 构造那一发 `PUT`（**原始字节**，不是 JSON）。`op` 决定那两枚档位头的**值**。
///
/// # 头：抓包的 **12 套**里，本层只装配**有读数的**那几枚
///
/// 2026-10-07 真机抓包（`app-album-backup-samples.txt` §4）把两发 EOS `PUT`（照片 + 快照）
/// 逐枚数成 **12 枚**：
///
/// ```text
/// accept、accept-encoding、accept-language、connection、content-length、
/// content-type、host、upload-complete: ?1、upload-draft-interop-version: 6、
/// user-agent、x-yun-op-type、x-yun-sub-op-type
/// ```
///
/// 逐枚去向（**这是"12 套"在本层为什么只落 6 枚的全部理由**）：
///
/// | 枚 | 谁发 |
/// |---|---|
/// | `content-type` | **本层**（[`CLOUD_PUT_CONTENT_TYPE`]）|
/// | `accept-language` | **本层**（[`CLOUD_ACCEPT_LANGUAGE`]；⚠ 这一发的值抓包**没给**，见该常量）|
/// | `upload-complete` | **本层**（[`CLOUD_PUT_UPLOAD_COMPLETE`]）|
/// | `upload-draft-interop-version` | **本层**（[`CLOUD_PUT_UPLOAD_DRAFT_INTEROP_VERSION`]）|
/// | `x-yun-op-type` / `x-yun-sub-op-type` | **本层**（由 `op` 折出，见 `android_op_codes`）|
/// | `host` / `content-length` | 传输层（reqwest 按 URL 与体长自己发）|
/// | `accept-encoding` / `connection` | 传输层（reqwest 的默认行为；本层不声明）|
/// | `accept` | **本层不发**：抓包只给了头名，**值无读数** —— 编一个 `*/*` 属于推断 |
/// | `user-agent` | 发送层（[`crate::ClientProfile::effective_user_agent`]）—— 在这儿设会是**第二根同名头** |
///
/// # ⚠ 本版**不发** `origin` / `referer`（与旧 TS 的一处已知分歧）
///
/// 旧 TS 的 `putFile` 发 `content-type` / `origin` / `referer` 三枚
/// （`apis/file.ts:98-104`）。而真机那 12 枚里**没有** `origin` / `referer`
/// ⇒ 本版按抓包**不发**。两枚常量（[`CLOUD_PUT_ORIGIN`] / [`CLOUD_PUT_REFERER`]）留着，
/// 是为了把这条分歧留在代码里，而不是让下一个人以为"从来就没有过"。
///
/// ⚠ 这一处与 `AGENTS.md` 那条「`mcloud-sign` 优先于抓包」**表面上冲突**：
/// 判据是**同一条链上真机自己在跑什么** —— 照片那一发真机不带这两枚也回 200
/// （抓包 §4），而旧 TS 那两枚是 CORS 头，对一个非浏览器客户端没有语义。
/// 若哪天这一枪开始 403，**第一处要怀疑的就是这里**（把两枚加回来再试一次即可）。
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
/// 抓包 §7 也印证了这一支「**零 mcloud 凭据**」（授权整套在查询串上）。
///
/// ⚠ `content` 为空（0 字节）也是**带体**：`Some(vec![])` 与 `None` 语义不同
/// （前者发出 `content-length: 0`），判据只看 [`HttpRequest::has_body`]。
pub fn upload_put_request_with_op(
    upload_url: &str,
    content: &[u8],
    op: CloudOpType,
) -> HttpRequest {
    let (op_code, sub_op_code) = android_op_codes(op);
    let req = HttpRequest::put(upload_url).bytes_body(content);
    // 覆盖式（`set_cloud_header`）而不是追加：这一族的三套头全靠"多哪几枚、少哪几枚"，
    // 追加语义会让"改一个值"静默变成"多一根线"（见 `set_cloud_header` 的说明）。
    let req = set_cloud_header(req, "content-type", CLOUD_PUT_CONTENT_TYPE);
    let req = set_cloud_header(req, "accept-language", CLOUD_ACCEPT_LANGUAGE);
    let req = set_cloud_header(req, "upload-complete", CLOUD_PUT_UPLOAD_COMPLETE);
    let req = set_cloud_header(
        req,
        "upload-draft-interop-version",
        CLOUD_PUT_UPLOAD_DRAFT_INTEROP_VERSION,
    );
    let req = set_cloud_header(req, "x-yun-op-type", &op_code.to_string());
    set_cloud_header(req, "x-yun-sub-op-type", &sub_op_code.to_string())
}

/// 手动上传那一支的 `PUT`（旧签名，**其它调用方在用**）。
///
/// 转调 [`upload_put_request_with_op`] 并传 [`CloudOpType::Upload`]（`1 / 100`）——
/// 那就是手动支的读数（抓包 `app-upload-samples.txt` §5）。
///
/// ⚠ **备份支不能再用这个函数**：照片那一发要 `2 / 200`、快照那一发要 `0 / 0`，
/// 而这里写死 `Upload`。要按档位发就用带 `op` 的那个。
pub fn upload_put_request(upload_url: &str, content: &[u8]) -> HttpRequest {
    upload_put_request_with_op(upload_url, content, CloudOpType::Upload)
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

// ══════════════════════════ 快照那一支：backup/createSnapshot · completeSnapshot ══════════════════════════
//
// 2026-10-07 真机抓包（`app-album-backup-samples.txt` §6）—— **这两口在历史样本里从没出现过**，
// 所以下面每一句都写清"哪一格有读数、哪一格没有"。
//
// 它在链上的位置：照片那三枪（create → PUT(personal) → complete）跑完之后，
// 备份支再补一发**快照**：`createSnapshot` → PUT(system) → `completeSnapshot`。
// 快照声明的是"这一批备份里有几枚、总共多少字节、内容哈希是多少"，
// 体是 ZIP（`fileExtension: "zip"`，抓包 §6 由魔数 `PK\x03\x04` 认出）。
// ⚠ **ZIP 里装的是什么，抓包明写"没解"**（§9）⇒ 本层不猜、不生成、只转发字节。

/// 25 套（`backup/createSnapshot` · `backup/completeSnapshot`）的**静态**头。
///
/// # 25 套 = 共有 17 枚 + 那 8 枚；**不带** `cache-control` 与两枚 op 头
///
/// 抓包 §7 那张差集表（同一台主机 `personal-kd-njs.yun.139.com`、同一个 `/hcy/` 前缀）：
///
/// - `backup/*` **有**而 `file/create` 没有的 8 枚：`pragma`、`service_identifier_key`、
///   `x-DeviceInfo`、`x-DeviceName`、`x-huawei-channelSrc`、`x-SvcType`、`x-UserAgent`、
///   `x-yun-user-timestamp`；
/// - `file/create` **有**而 `backup/*` 没有的 3 枚：`cache-control`、`x-yun-op-type`、
///   `x-yun-sub-op-type`。
///
/// ⇒ **同一端的两个口用的是两套装配**，而分套**不是按业务链路分的**
/// （同一条备份链里 20 与 25 并存）。**按什么分，抓包 §7 明写"仍无读数"** ——
/// 不许在这里写成「SDK 层 vs 界面层」。
///
/// # ⚠ 8 枚里本层只装配 5 枚，另外 3 枚**本仓没有来源**
///
/// | 枚 | 本层 | 依据 |
/// |---|---|---|
/// | `pragma` | `no-cache` | `client-profiles-ios-app.md` §3.3.3（同主机同族）|
/// | `service_identifier_key` | `service_identifier_cloudDisk` | 同上 §3.3.3（**按主机查表**，见 [`CLOUD_SERVICE_IDENTIFIER_CLOUD_DISK`]）|
/// | `x-SvcType` | `1` | 同上 §3.3.3 |
/// | `x-huawei-channelSrc` | `channel.value()` | 同上 §3.3.3（与 `x-yun-app-channel` 同值）|
/// | `x-UserAgent` / `x-yun-user-agent` / `x-DeviceName` | [`crate::app_login`] 那三个常量 | 本仓已有的 App 设备画像 |
/// | `x-DeviceInfo` | ❌ **不发** | 它的值要一个**设备 id**（`app_login::app_device_info` 的入参），而 [`CloudSnapshotCreateInput`] 里没有这一格 |
/// | `x-yun-user-timestamp` | ❌ **不发** | 它是**每请求**的 16 位微秒戳；这一层的入参里只有一个 13 位毫秒的 `timestamp_ms`，**换算关系无读数** |
///
/// ⚠ 相册备份那一篇抓包**只给了这 8 枚的头名、没给值**（§7 是差集表）⇒ 上面每一枚的值
/// 都不是"那一发抓包的读数"，而是"同一台主机、同一个客户端在**另一份**读数里的值"。
/// 这一条要在成文与汇报里说清楚，别把 `zh-CN` / `service_identifier_cloudDisk`
/// 说成"抓包里读到的"。
///
/// ⚠ `x-yun-client-info`（17 枚共有之一）本层**不发**：真机那一枚是 143 字符的
/// iPad 设备串（§3.3.3），要设备 id 才拼得出来 —— 与 `x-DeviceInfo` 同一条理由。
/// 这里**不**退回 `cloud_file_op_headers` 用的那两枚旧 TS 设备串：那是**20 套**的读数，
/// 搬进 25 套就是把两套装配混起来。
///
/// # 渠道那一格：真机是 `10000020`，本层只能表达 `10000023`
///
/// 抓包 §7 明写 25 套里有 `x-huawei-channelSrc`，而 §3.3.3 给出这一族的值是
/// **`10000020`**（`app_login::APP_CHANNEL`）—— 那是 [`CloudChannel`] **表达不出来**的一档
/// （它只有旧 TS 的 android `10000023` / web `10000034` / PC `10200153`）。
/// ⇒ 本层照调用方给的档发，并在 [`CloudSnapshotCreateInput::channel`] 上点名这处不一致。
fn snapshot_op_headers(req: HttpRequest, channel: CloudChannel) -> HttpRequest {
    let channel_src = channel.value();
    let req = set_cloud_header(req, "Pragma", "no-cache");
    let req = set_cloud_header(
        req,
        "service_identifier_key",
        CLOUD_SERVICE_IDENTIFIER_CLOUD_DISK,
    );
    // §3.3.3：`hcy/*` 网盘族的 `x-yun-api-version` 是 **v2**（登录链路才是 v1）。
    let req = set_cloud_header(req, "x-yun-api-version", "v2");
    let req = set_cloud_header(req, "x-yun-app-channel", channel_src);
    let req = set_cloud_header(req, "x-huawei-channelSrc", channel_src);
    let req = set_cloud_header(req, "x-yun-net-type", "1");
    let req = set_cloud_header(req, "x-yun-svc-type", "1");
    let req = set_cloud_header(req, "x-SvcType", "1");
    let req = set_cloud_header(req, "x-UserAgent", APP_X_USER_AGENT);
    let req = set_cloud_header(req, "x-yun-user-agent", APP_X_YUN_USER_AGENT);
    let req = set_cloud_header(req, "x-DeviceName", APP_DEVICE_NAME_B64);
    let req = set_cloud_header(req, "Accept-Language", CLOUD_ACCEPT_LANGUAGE);
    // ⚠ `Content-Type` 必须**最后**覆盖式地设（与 `cloud_file_op_headers` 同一条理由）：
    // `json_body` 自己不带头，而发送层会在"调用方没声明"时补一枚默认值 ⇒ 两枚同名线。
    set_cloud_header(req, "Content-Type", CLOUD_CONTENT_TYPE)
}

/// 「要建一个什么快照」的调用方输入（抓包 §6 的**请求 7 键**）。
///
/// ⚠ 这里**没有** `content`（快照 ZIP 的字节）：建快照那一枪只声明形状，
/// 真正的字节走 [`upload_put_request_with_op`]（`op` 传 [`CloudOpType::Snapshot`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudSnapshotCreateInput {
    /// `fileExtension`：快照体的扩展名。实拍是 **`zip`**（抓包 §6，
    /// 与 PUT 真实体的魔数 `PK\x03\x04` 对上）。
    pub file_extension: String,
    /// `count`：这一批**几枚**。实拍 `5`，而同一窗口只录到 **1** 发照片 `PUT`
    /// ⇒ 抓包 §9 明写「另外 4 枚去哪了**无读数**」（可能走了没录到的那一批、
    /// 也可能走了秒传不发 `PUT`）。**本层只转发这个数，不校验它与发数一致**。
    pub count: i64,
    /// `size`：快照体的字节数。实拍 `619`（= PUT 的 `content-length`）。
    pub size: i64,
    /// `contentHash`：**快照体**（那个 ZIP）的 SHA256 十六进制。
    /// ⚠ 与照片那一枪的 `contentHash` **不是同一个值**（一个哈希照片、一个哈希清单）。
    pub content_hash: String,
    /// `timestamp`：**13 位毫秒**（抓包 §6 逐字标了 `int(13 位)`）。
    /// ⚠ 别与 25 套里那枚 `x-yun-user-timestamp`（16 位微秒）混起来 ——
    /// 同一个客户端里两个时间单位（`app-sms-login-samples.txt` §3 也点过这一格）。
    /// 本层**没有时钟** ⇒ 由调用方给。
    pub timestamp_ms: i64,
    /// 渠道档。⚠ 真机这一族是 `10000020`，而 [`CloudChannel`] 表达不出来
    /// （见 `snapshot_op_headers` 最后那一段）⇒ 本层照参数发，要复现真机得先加一档。
    pub channel: CloudChannel,
}

/// `backup/createSnapshot` 回的那一格 `data`（抓包 §6 的**响应 4 键**）。
///
/// # 为什么三格是 `Option`，而解析器却对缺格回 Err
///
/// 类型与 [`CloudFileCreated`] 同形（三格 `Option` + 一枚 `bool`），是为了让**读的人**
/// 一眼看出"协议不保证每一格都在"；而 [`parse_snapshot_create`] 在**缺格时回 Err**
/// —— 理由见那个函数（缺 `uploadUrl` 就没法 PUT、缺 `fileId`/`uploadId` 就发不出
/// `completeSnapshot`，三格缺任何一枚这一支都走不下去）。
/// ⇒ 解析**成功**时三格必然是 `Some`；`Option` 不是"可能没有"的许可证，
/// 而是"这一层不替上游承诺"。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudSnapshotCreated {
    /// `data.fileId`。实拍是 **str64**（⚠ 照片那一支是 str33）。
    /// **为什么快照的 id 是 64 位，抓包 §6 明写"无读数"** —— 不许写成"内容寻址"。
    pub file_id: Option<String>,
    /// `data.uploadId`（实拍 str33）。
    pub upload_id: Option<String>,
    /// `data.partInfos[0].uploadUrl`（实拍 str583）—— 预签名写入入口，
    /// **绝不进日志 / 错误文本 / 夹具**（见模块头那一段）。
    pub upload_url: Option<String>,
    /// `data.rapidUpload`。实拍 `false`。
    /// ⚠ 秒传那一支（`true` 时有没有 `PUT`、`uploadId` 是否 null）**移动端无读数**
    /// （抓包 §9）⇒ 本层**不给自己开例外**，见 [`parse_snapshot_create`]。
    pub rapid_upload: bool,
}

/// 构造 `POST /hcy/backup/createSnapshot`（**写**：声明这一批备份的形状）。
///
/// 体 **7 键**逐字来自抓包 §6：
///
/// ```text
/// fileExtension        str3       # "zip"
/// count                int        # 5
/// contentHash          str64
/// contentHashAlgorithm str6       # "SHA256"
/// size                 int        # 619
/// partInfos[]          长度=1     # {partNumber, partSize}
/// timestamp            int(13 位) # epoch 毫秒
/// ```
///
/// ⭐ 与照片那一支的关键差别：`partInfos[0]` **没有 `parallelHashCtx`**
/// （抓包 §6 括号里逐字写了"与照片那一支不同"）⇒ 这里**不发**那一格。
/// 顺手"统一"成一个 `partInfos` 构造函数就会把它加回去，而那是另一发请求的形状。
///
/// # 缺 `contentHash` / `count < 1` ⇒ 回错误，**不发**
///
/// 与 [`file_create_request`] 同一口径（[`Error::Config`]）：
/// 快照声明的就是"这一批的内容是什么、有几枚"，这两格缺了它就不是一个快照。
/// ⚠ `count` 的**语义**（它到底数的是什么）抓包 §9 明写无读数 ——
/// 这里只挡住 `0`/负数（那是"声明一个空快照"，而空快照这一格**没有任何读数**），
/// **不**校验它与实际发数一致。
pub fn snapshot_create_request(input: CloudSnapshotCreateInput) -> Result<HttpRequest> {
    if input.content_hash.is_empty() {
        return Err(Error::Config(
            "backup/createSnapshot 缺 contentHash：快照声明的就是这一批的内容哈希，而 completeSnapshot 那一枪要用同一个串 —— 少了它两枪就对不上".into(),
        ));
    }
    if input.count < 1 {
        return Err(Error::Config(format!(
            "backup/createSnapshot 的 count 是 {}（<1）：那是「声明一个空快照」，而这一格没有任何读数 —— 不发",
            input.count
        )));
    }
    let CloudSnapshotCreateInput {
        file_extension,
        count,
        size,
        content_hash,
        timestamp_ms,
        channel,
    } = input;
    let body = serde_json::json!({
        "fileExtension": file_extension,
        "count": count,
        "contentHash": content_hash,
        "contentHashAlgorithm": CLOUD_HASH_ALGORITHM,
        "size": size,
        // ⚠ 单分片、**没有** `parallelHashCtx`（抓包 §6）。
        "partInfos": [{ "partNumber": 1, "partSize": size }],
        "timestamp": timestamp_ms,
    });
    Ok(snapshot_op_headers(
        HttpRequest::post(BACKUP_CREATE_SNAPSHOT).json_body(&body),
        channel,
    ))
}

/// 解析 `backup/createSnapshot` 的回执（抓包 §6：923 字节、`data` **4 键**）。
///
/// # 与 [`parse_file_create`] 的差别：**这里缺格一律 Err**
///
/// `data` 缺席 ⇒ Err（这一族的载荷在 `data`，缺席说明信封形状变了）；
/// `data` 不是对象 ⇒ Err；`fileId` / `uploadId` / `uploadUrl` 三格**缺任何一枚 ⇒ Err**：
///
/// - 缺 `uploadUrl` ⇒ 没有地方 PUT 那 619 字节（这一支走不下去）；
/// - 缺 `fileId` / `uploadId` ⇒ `completeSnapshot` 那一枪发不出去
///   （它的 7 键里这两枚是全部依据）。
///
/// ⇒ 三格都不"兜成 None 让调用方自己判"：`None` 在上层会变成
/// 「这一枪成了但没给 URL」那种**两种病混在一起**的读法，
/// 而这一支的下一步对三格的要求**完全相同**（都得有）。
///
/// # ⚠ 秒传那一支：本层**不给自己开例外**
///
/// 抓包 §9 明写「移动端 `rapidUpload:true` 时有没有 `PUT`、响应 `uploadId` 是否 null ——
/// **仍未读**」。所以这里**不**写「`rapidUpload:true` 就允许缺 `uploadUrl`」那种例外：
/// 开了它，真遇上那一支会**静默少发一发 PUT**（表现是"快照建了但没传上去"），
/// 而不开它只会**明确报错**（可回滚）。要开例外得先有一次真机读数。
pub fn parse_snapshot_create(body: &[u8]) -> Result<CloudSnapshotCreated> {
    const WHAT: &str = "backup/createSnapshot";
    let envelope = CloudEnvelope::parse_bytes(body, WHAT)?;
    envelope.ensure_ok(WHAT)?;
    let payload = envelope.payload().ok_or_else(|| {
        Error::Parse(
            "backup/createSnapshot 的回执里没有 data 这一格 —— 这一族的载荷在 data（不是 result），缺席说明信封形状变了，不能兜成一个全空结构"
                .into(),
        )
    })?;
    let Some(obj) = payload.as_object() else {
        return Err(Error::Parse(format!(
            "backup/createSnapshot 的 data 不是对象，实得 {payload}"
        )));
    };
    let file_id = required_str(obj, "fileId", WHAT)?;
    // ⚠ **先读 `rapidUpload`，再决定另外两格要不要**（2026-10-07 真机读数改的，见下）。
    let rapid_upload = cloud_flag(obj, "rapidUpload");
    let upload_id = obj
        .get("uploadId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // `partInfos` 是数组，取**第一项**的 `uploadUrl`（与 `parse_file_create` 同一条路径）。
    let upload_url = obj
        .get("partInfos")
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .and_then(|first| first.get("uploadUrl"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if !rapid_upload {
        if upload_id.is_none() {
            return Err(Error::Parse(
                "backup/createSnapshot 的 data.uploadId 缺席 / null / 空串，而 rapidUpload 不是 true —— 这一格是这一支继续走下去的依据（没有它就没法 PUT，也没法 complete）".into(),
            ));
        }
        if upload_url.is_none() {
            return Err(Error::Parse(
                "backup/createSnapshot 没给出 data.partInfos[0].uploadUrl（缺席 / null / 空串），而 rapidUpload 不是 true —— 没有它这一支就没地方 PUT 那一段字节，不能当成「成了但没 URL」放过去".into(),
            ));
        }
    }
    Ok(CloudSnapshotCreated {
        file_id: Some(file_id),
        upload_id,
        upload_url,
        rapid_upload,
    })
}

/// 「结一个快照」的调用方输入（抓包 §6 的**请求 7 键**）。
///
/// ⚠ 这里**没有** `channel`（契约如此）。[`snapshot_complete_request`] 因此按
/// [`CloudChannel::default()`]（android 档）装配头 —— **与 `create` 那一发必须同档**，
/// 调用方若给 `create` 传了别的档，这一格会不一致（见那个函数的说明）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudSnapshotCompleteInput {
    /// `fileId`：`createSnapshot` 回的那一枚（实拍 str64）。
    pub file_id: String,
    /// `uploadId`：`createSnapshot` 回的那一枚（实拍 str33）。
    pub upload_id: String,
    /// `count`：与 `createSnapshot` 那一枪**同一个数**（旧实现就是同一个变量）。
    pub count: i64,
    /// `contentHash`：与 `createSnapshot` 那一枪**同一个串**。
    pub content_hash: String,
    /// `size`：与 `createSnapshot` 那一枪**同一个数**。
    pub size: i64,
    /// `timestamp`：13 位毫秒。⚠ 与 `createSnapshot` 那一枪是否必须**同值**没有读数
    /// ⇒ 本层原样转发调用方给的值，不替它复用。
    pub timestamp_ms: i64,
}

/// 构造 `POST /hcy/backup/completeSnapshot`（**写**：告诉上游这一批传完了）。
///
/// 体 **7 键**逐字来自抓包 §6：`{fileId, uploadId, count, contentHash,
/// contentHashAlgorithm, size, timestamp}`。
///
/// # 缺 `fileId` / `uploadId` / `contentHash` ⇒ 回错误，**不发**
///
/// 与 [`file_complete_request`] 同一口径：这三格是"完成哪一发"的全部依据，
/// 静默发一枪空请求的表现是「上游回了成功、但没有任何东西被完成」。
///
/// # ⚠ 渠道那一格（契约里没有 `channel`）
///
/// [`CloudSnapshotCompleteInput`] 没有渠道入参 ⇒ 这里按 [`CloudChannel::default()`]
/// （[`CloudChannel::Android`]）装配 25 套头。**同一支链上的两发必须同档**，
/// 所以调用方给 [`snapshot_create_request`] 传的 `channel` 应当也是它。
/// 要支持跨档得先扩契约（加一格 `channel`）—— 本层**不**去猜。
pub fn snapshot_complete_request(input: CloudSnapshotCompleteInput) -> Result<HttpRequest> {
    // ⚠ `uploadId` **允许空串**：真机读数（2026-10-07）里 `createSnapshot` 会回
    // `uploadId: null`（`rapidUpload` 那一支）—— 与 `file/complete` 同一条口径
    // （旧实现 `stringOf(null)` 就是空串，见 [`file_complete_request`]）✓。
    // `fileId` / `contentHash` 缺了才不发：它们是"完成哪一发"的另一半依据。
    if input.file_id.is_empty() || input.content_hash.is_empty() {
        return Err(Error::Config(
            "backup/completeSnapshot 缺 fileId / contentHash：没有它们这一枪就说不清在完成哪一发（本层不发空请求）".into(),
        ));
    }
    let CloudSnapshotCompleteInput {
        file_id,
        upload_id,
        count,
        content_hash,
        size,
        timestamp_ms,
    } = input;
    let body = serde_json::json!({
        "fileId": file_id,
        "uploadId": upload_id,
        "count": count,
        "contentHash": content_hash,
        "contentHashAlgorithm": CLOUD_HASH_ALGORITHM,
        "size": size,
        "timestamp": timestamp_ms,
    });
    Ok(snapshot_op_headers(
        HttpRequest::post(BACKUP_COMPLETE_SNAPSHOT).json_body(&body),
        CloudChannel::default(),
    ))
}

/// 解析 `backup/completeSnapshot` 的回执：**只判「这一枪成不成」**。
///
/// # ⚠⚠ 与 [`parse_file_create`] 的关键差别：`data` 是 `null` **也算成功**
///
/// 抓包 §6：这一口只回 **59 字节**、`data` 是 **`null`** ——
/// 与 `file/complete`（回 29 键对象快照）**不是一套**。所以这一口**必须另写一个解析器**：
/// [`parse_file_create`] 在"缺 `data`"与"`data` 不是对象"两处都会 Err
/// （它要的是对象快照），拿它来解这一口会把**每一发成功**都读成失败。
///
/// 这里**只读信封**：`code` 必须是字符串 `"0000"`，`success:false` 否决，
/// `data` 是 `null` / 缺席 / 任何形状都**不读**（`file/complete` 的解析器也是这个口径，
/// 区别只在名字与"这一口有实拍证明 data 是 null"）。
///
/// ⚠ `data` 是 `null` 时 `CloudEnvelope::payload` 会回 `Some(Value::Null)` ——
/// 也就是说"缺席"与"null"在这一口都合法，本函数**不看这一格**，两个都放过。
pub fn parse_snapshot_complete(body: &[u8]) -> Result<()> {
    let envelope = CloudEnvelope::parse_bytes(body, "backup/completeSnapshot")?;
    envelope.ensure_ok("backup/completeSnapshot")
}

/// 取一格**必需**的非空字符串；缺席 / `null` / 非字符串 / 空串一律 Err。
///
/// ⚠ 与 [`opt_str_at`] 刻意分开：那一枚是"可选的、缺了交回 `None`"，
/// 这一枚是"缺了这一枪就走不下去"。两个语义写成一个函数，
/// 早晚会有人把必需的那一格当可选用（表现是上游回了成功、而下一步没有依据）。
fn required_str(obj: &serde_json::Map<String, Value>, key: &str, what: &str) -> Result<String> {
    match obj.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        other => Err(Error::Parse(format!(
            "{what} 的 data.{key} 缺席 / null / 空串 / 不是字符串（实得 {other:?}）—— 这一格是这一支继续走下去的依据，不能兜成 None"
        ))),
    }
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

    /// 从**原始字节**解析（快照那两个解析器收的是 `&[u8]`，见它们的签名）。
    ///
    /// ⚠ 这一族的两口控制面回执都是 JSON 文本，但**非 UTF-8 的回执必须报错而不是
    /// 被 `from_utf8_lossy` 洗成"形状变了"**：后者会让人以为"上游换了信封"，
    /// 而真相是"这一发拿到的根本不是 JSON"（比如一个网关的 HTML 错误页、
    /// 或一段被压缩过的体）—— 两种病的修法完全不同。
    fn parse_bytes(body: &[u8], what: &str) -> Result<Self> {
        let text = std::str::from_utf8(body).map_err(|e| {
            Error::Parse(format!(
                "{what} 的回执不是合法 UTF-8（{e}）—— 说明这一发拿到的不是这一族的 JSON 信封"
            ))
        })?;
        Self::parse(text)
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

impl CloudFileCreated {
    /// 这一发**是不是我们造的那个文件**（能不能进清理名单）—— 三态。
    ///
    /// * `Some(true)`：是我们的 ⇒ 可以记账、链尾可以清 ✓
    /// * `Some(false)`：**撞上用户已有的同名文件**（上游按 `refuse` 语义把那一枚交回来了）
    ///   ⇒ **绝不**记账、**绝不**删 ✗
    /// * `None`：判不了 ⇒ 同样**绝不**删 ✗
    ///
    /// # ⚠⚠ 2026-10-05 真机：上游把"没撞同名"从 `false` 改成了 **`null`**
    ///
    /// 那天 86 发 `create` **全废** ✗ —— 只看 [`Self::exist`] 那一格的话，`null` 落进
    /// "判不了"，于是 106 / 113 / 522 / 615 与 434 的补传**一起**被拒 ✗。
    /// 实拍回执（键序照抄，脱敏后）：
    ///
    /// ```json
    /// {"success":true,"code":"0000","message":"请求成功","data":{
    ///   "parentFileId":"/","fileId":"FmjnKrVH…","type":"file",
    ///   "fileName":"mcloud-task-expansion-245a9182ccf640de.bin","rapidUpload":false,
    ///   "uploadId":"2~Ok3eN2Hay6jBPHxO_UOMup26SS-pmeo","partInfos":[{"uploadUrl":"https://…"}],
    ///   "exist":null,"formInfo":null,"transferToken":null}}
    /// ```
    ///
    /// ⇒ 判据换成"**这一发有没有开出一个新的上传会话**"：`uploadId` 有、且
    /// （`uploadUrl` 有 **或** 秒传）⇒ 上游确实为我们新开了一发 ⇒ 那个 `fileId` 是新的 ✓。
    /// 而撞同名那档（`refuse`）按本模块文档是 `partInfos` 全 `null`、**没有** uploadUrl ✓
    /// ⇒ 两档分得开 ✓。`exist:true` 仍然直接判"不是我们的"（优先级更高 ✓）。
    pub fn owned_by_us(&self) -> Option<bool> {
        match self.exist {
            Some(true) => Some(false),
            Some(false) => Some(true),
            // ⚠ `null` / 缺席：**不能**一律当"我们的" ✗ —— 判据是"有没有新上传会话" ✓。
            None => {
                let fresh_session =
                    self.upload_id.is_some() && (self.upload_url.is_some() || self.rapid_upload);
                fresh_session.then_some(true)
            }
        }
    }
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
