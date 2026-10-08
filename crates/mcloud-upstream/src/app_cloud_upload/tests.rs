//! 上传族**前五枪**协议层的用例（第四枪 `batchTrash` 在 [`crate::cloud_trash`]）。
//!
//! # 这一族钉什么、不钉什么
//!
//! 四枪里只有 `file/create` 有实拍，而那两条实拍是**建目录**那一型
//! （`type:"folder"`，`uploadId` / `partInfos` 都是 `null`）⇒ 它们能钉的只有
//! ① URL、② 信封形状（`code` 是字符串 `"0000"`、载荷在 `data`）。
//! ⚠ 文件型 / `PUT` / `complete` / `batchTrash` 自 2026-09-25 起有**真跑**（R27），
//! 但仍**无 har 实拍**；建目录那一型 2026-09-26 由 R7 那一发探针补上真打
//! （成文见 `../mcloud/docs/capture/app-cloud-folder-trash-samples.txt`）。
//!
//! # ⭐ 2026-10-07 起：相册自动备份那一支**有真机抓包了**
//!
//! `../mcloud-capture/docs/capture/app-album-backup-samples.txt`（通路 A ＋ mihomo TUN
//! 按进程接管，六发明文）＋ 同端手动上传那一支 `app-upload-samples.txt` §3/§5/§7。
//! ⇒ 本文件从"只能钉形状"变成**能逐发钉读数**，新钉的四格是：
//!
//! 1. **两支两个键集**：备份支 `parentPath` 在 / `parentFileId` 不在、
//!    `fileRenameMode` = `refuse_duplicate_content`；手动支反过来；
//! 2. **三档 op**：手动 `1/100`、照片备份 `2/200`、快照 `0/0`；
//! 3. **三套头**（20 / 25 / 12）：`backup/*` 那两口**没有** op 两枚、
//!    而**有** `pragma` / `service_identifier_key` 那 8 枚里的几枚；
//!    两发 `PUT` 有 `upload-complete` / `upload-draft-interop-version` 与那两枚 op 头；
//! 4. **`completeSnapshot` 的 `data` 是 `null` 也算成功** —— 与 `parse_file_create`
//!    的关键差别（拿错解析器会把**每一发成功**读成失败）。
//!
//! ⚠ 抓包**没给的**（不许在用例里编）：`userRegion` 的省/市（本仓无来源 ⇒ 这一版不发）、
//! 25 套那 8 枚的**值**（抓包只给了头名）、快照 `fileId` 为什么是 64 位、ZIP 里装什么。
//!
//! 所以全部用例钉的是三类**可控**的事：
//!
//!  1. **字面量别在翻译中丢失** —— `application/oct-stream`（少一个 `e`，**手动支**的体里）
//!     与 `application/octet-stream`（备份支的体里、以及 `PUT` 的头里）是**三处不同的东西**；
//!     三档 client-info 串整串照抄；`x-yun-device-id` 的值**就是**设备信息串。
//!  2. **同名头只能有一枚** —— [`HttpRequest::header`] 是**追加**，而旧 TS 是对象赋值
//!     （覆盖）。android 的备份档要把 base 那枚 `x-yun-op-type: 1` **摘掉**再设成 `2`。
//!     ⚠ 判「恰好一枚」一律用 [`header_count`]（大小写不敏感）：用 `find` 那种写法，
//!     「同一枚出现两次」会**永远通过**。
//!  3. **「没给」不能念成「给了零值」** —— `data` 缺失、`fileId` / `contentHash` 缺席、
//!     预签名 URL 缺席，三格都得是**可区分**的错误，而不是「成功但空」。
//!
//! ⚠ 刻意**不钉**的：`exist:true` 之后该不该继续（那是编排层的判断，且零样本）、
//! `uploadId` 为 `null` 时 `complete` 会怎样（未测试）、`PUT` 的响应形状（零样本）、
//! `PUT` 的「不可重试」（本仓 [`HttpRequest`] 没有那一格，见 `upload_put_request`）、
//! 快照 `count` 的语义（抓包 §9 明写无读数）。
//!
//! # 凭据边界
//!
//! 预签名 URL 一律用 `https://synth-oss.example.invalid/…`（`.invalid` 是保留域，
//! 永远不会被解析成真主机），测试**不发任何请求**。

use super::*;
use crate::cloud_trash::{CLOUD_CHANNEL_WEB, CLOUD_CLIENT_INFO_WEB, CLOUD_CONTENT_TYPE};

/// 合成的预签名 URL（外站）。
const SYNTH_PUT_URL: &str = "https://synth-oss.example.invalid/put/SYNTH-TOKEN";
/// 合成的预签名 URL（命中 `yun.139.com` 后缀那一侧）。
const SYNTH_KD_URL: &str = "https://personal-kd-njs.yun.139.com/hcy/put/SYNTH-TOKEN";
/// 快照 `createSnapshot` 实拍那枚 `fileId` 的**长度**（str64，不是照片那支的 33）。
const SYNTH_SNAPSHOT_FILE_ID: &str =
    "SNAP0123456789abcdef0123456789abcdef0123456789abcdef0123456789ab";
/// 合成的快照 `uploadId`（实拍 str33）。
const SYNTH_SNAPSHOT_UPLOAD_ID: &str = "SYNTH-SNAPSHOT-UPLOAD-ID-01234567";

// ── 小工具 ──────────────────────────────────────────────────────────────

/// 取一枚头（大小写不敏感 —— 上游与 reqwest 都不保证大小写一致）。
fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// 数同名头的**枚数**（大小写不敏感）。
fn header_count(req: &HttpRequest, name: &str) -> usize {
    req.headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .count()
}

fn body_json(req: &HttpRequest) -> Value {
    let text = req.body_str().expect("这一族的体应是 UTF-8 文本");
    serde_json::from_str(text).expect("体应是合法 JSON")
}

/// 体的键集合（**排序后**拼成一串）—— 与声明序无关，只比集合。
fn sorted_keys(body: &Value) -> String {
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("体应是 JSON 对象")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys.join(",")
}

fn probe_request() -> HttpRequest {
    HttpRequest::post("https://example.invalid/x")
}

fn create_input() -> CloudFileCreateInput {
    CloudFileCreateInput {
        name: "asign-synth.png".into(),
        size: 64,
        content_hash: "a".repeat(64),
        ..Default::default()
    }
}

/// **备份支**（相册自动备份）的输入：`parent_path` 有值 ⇒ 走 `parentPath` 那一支。
fn backup_create_input() -> CloudFileCreateInput {
    CloudFileCreateInput {
        parent_path: Some("SYNTH".into()),
        rename_mode: CloudRenameMode::RefuseDuplicateContent,
        parallel_upload: true,
        local_created_at: Some("2026-10-07T08:59:04.999Z".into()),
        op: CloudOpType::Backup,
        ..create_input()
    }
}

/// **手动支**：`parent_path: None` ⇒ 走 `parentFileId` 那一支。
fn manual_create_input() -> CloudFileCreateInput {
    CloudFileCreateInput {
        rename_mode: CloudRenameMode::ForceRename,
        ..create_input()
    }
}

fn snapshot_create_input() -> CloudSnapshotCreateInput {
    CloudSnapshotCreateInput {
        file_extension: "zip".into(),
        count: 5,
        size: 619,
        content_hash: "c".repeat(64),
        timestamp_ms: 1_759_808_345_332,
        channel: CloudChannel::Android,
    }
}

fn snapshot_complete_input() -> CloudSnapshotCompleteInput {
    CloudSnapshotCompleteInput {
        file_id: SYNTH_SNAPSHOT_FILE_ID.into(),
        upload_id: SYNTH_SNAPSHOT_UPLOAD_ID.into(),
        count: 5,
        content_hash: "c".repeat(64),
        size: 619,
        timestamp_ms: 1_759_808_345_332,
    }
}

/// 解析器的入参是**字节**（快照那两口，契约如此）⇒ 用这个把测试里的 JSON 字面量转过去。
fn as_bytes(body: &str) -> &[u8] {
    body.as_bytes()
}

fn response(status: u16, body: &str) -> mcloud_core::HttpResponse {
    mcloud_core::HttpResponse {
        status,
        body: body.to_string(),
        headers: Vec::new(),
    }
}

const ALL_CHANNELS: [CloudChannel; 3] =
    [CloudChannel::Android, CloudChannel::Web, CloudChannel::Pc];
/// 三档 op（2026-10-07 抓包：手动 `1/100`、照片备份 `2/200`、快照 `0/0`）。
const ALL_OPS: [CloudOpType; 3] = [
    CloudOpType::Upload,
    CloudOpType::Backup,
    CloudOpType::Snapshot,
];

// ══════════════════════════ 常量：素材字面量 ══════════════════════════

/// ⭐ 三档 client-info 串整串照抄（旧 TS `constants.ts:21-27`）。
///
/// 这一条把**字面量本身**钉住：只跟常量比对的话，把常量「顺手修好」也照样绿。
/// 三档的版本号各不相同（App `12.6.0` / Web `7.15.3` / PC `8.3.3.20250521`）。
#[test]
fn client_info_strings_are_the_legacy_literals_verbatim() {
    assert_eq!(
        CLOUD_CLIENT_INFO_ANDROID,
        "1|127.0.0.1|1|12.6.0|Xiaomi|22041216C||02-00-00-00-00-00|android 14|1080X2360|zh||||032|0|"
    );
    assert_eq!(
        CLOUD_CLIENT_INFO_WEB,
        "||9|7.15.3|firefox|139.0|||linux unknow||zh-CN|||"
    );
    assert_eq!(
        CLOUD_CLIENT_INFO_PC,
        "||11|8.3.3.20250521|PC|TGktMjAyNDA5MTcxMDA2||| Windows 11 (11.0)|1920X1020|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||"
    );
    assert_ne!(CLOUD_CLIENT_INFO_ANDROID, CLOUD_CLIENT_INFO_WEB);
    assert_ne!(CLOUD_CLIENT_INFO_WEB, CLOUD_CLIENT_INFO_PC);
}

/// ⭐ 三处 `content-type` 是**三个不同的东西**，不许「顺手统一」。
///
/// 2026-10-07 起是**三处**（原先两处）：
/// 手动支体里那一枚（少一个 `e`，旧 TS）、备份支体里那一枚（抓包的正确拼写）、
/// `PUT` 头里那一枚（拼写正常）。
#[test]
fn the_three_content_types_stay_different() {
    // 手动支的体里那一枚少一个 `e`（旧 TS 的字面量）。
    assert_eq!(CLOUD_CREATE_CONTENT_TYPE, "application/oct-stream");
    // 备份支的体里那一枚是抓包的正确拼写（`app-album-backup-samples.txt` §3）。
    assert_eq!(CLOUD_CREATE_CONTENT_TYPE_OCTET, "application/octet-stream");
    // `PUT` 头里那一枚拼写正常 —— 与上一枚**字符串相同、却是两个东西**。
    assert_eq!(CLOUD_PUT_CONTENT_TYPE, "application/octet-stream");
    assert_ne!(CLOUD_CREATE_CONTENT_TYPE, CLOUD_CREATE_CONTENT_TYPE_OCTET);

    // 手动支走旧值。
    let manual = body_json(&file_create_request(manual_create_input()).expect("应能构造"));
    assert_eq!(
        manual["contentType"],
        serde_json::json!(CLOUD_CREATE_CONTENT_TYPE)
    );

    // 备份支走抓包那个正确拼写（反向改坏：把两支合并成一个常量 ⇒ 这一条红）。
    let backup = body_json(&file_create_request(backup_create_input()).expect("应能构造"));
    assert_eq!(
        backup["contentType"],
        serde_json::json!(CLOUD_CREATE_CONTENT_TYPE_OCTET)
    );

    let put = upload_put_request(SYNTH_PUT_URL, b"SYNTH");
    assert_eq!(header(&put, "content-type"), Some(CLOUD_PUT_CONTENT_TYPE));
}

// ══════════════════════════ 头画像：三档 × 两 op ══════════════════════════

/// android / upload 档逐枚对拍，**并且**断言不该有的头一枚都没有。
#[test]
fn android_upload_header_recipe_is_exact() {
    let req = cloud_file_op_headers(probe_request(), CloudChannel::Android, CloudOpType::Upload);

    assert_eq!(header(&req, "x-yun-api-version"), Some("v1"));
    assert_eq!(header(&req, "x-yun-app-channel"), Some("10000023"));
    assert_eq!(header(&req, "x-huawei-channelSrc"), Some("10000023"));
    assert_eq!(header(&req, "x-yun-op-type"), Some("1"));
    assert_eq!(header(&req, "x-yun-sub-op-type"), Some("100"));
    assert_eq!(
        header(&req, "x-yun-client-info"),
        Some(CLOUD_CLIENT_INFO_ANDROID)
    );
    assert_eq!(header(&req, "Content-Type"), Some(CLOUD_CONTENT_TYPE));

    // android 档**没有** PC 那三枚，也没有 web 的 device-id。
    for name in [
        "x-yun-market-source",
        "x-yun-module-type",
        "x-yun-svc-type",
        "x-yun-device-id",
    ] {
        assert_eq!(header(&req, name), None, "{name} 不该出现在 android 档");
    }
    // 凭据与 UA 归发送层（见模块头「已知分歧」）。
    for name in ["authorization", "deviceid", "user-agent", "jwttoken"] {
        assert_eq!(
            header_count(&req, name),
            0,
            "{name} 由发送层注入，这里设了会变成两根同名头"
        );
    }
}

/// ⭐⭐ 备份档换的是**两**枚码，而且 base 那枚 `x-yun-op-type: 1` 必须被**摘掉**。
///
/// 反向改坏它的样子：把 `set_cloud_header` 写成 `header`（追加）⇒ 这一条红，
/// 表现是同时发出 `1` 与 `2` 两根线，上游读哪一根未测试 ⇒ 备份玩法的判定会飘。
#[test]
fn android_backup_switches_op_and_sub_op_codes() {
    let req = cloud_file_op_headers(probe_request(), CloudChannel::Android, CloudOpType::Backup);

    assert_eq!(header(&req, "x-yun-op-type"), Some("2"));
    assert_eq!(header(&req, "x-yun-sub-op-type"), Some("200"));
    assert_eq!(
        header_count(&req, "x-yun-op-type"),
        1,
        "覆盖式：base 那枚 1 必须被摘掉"
    );
    assert!(
        !req.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-yun-op-type") && v == "1"),
        "旧实现备份档发的是 2，不是 1 与 2 两根线"
    );
}

/// ⭐⭐ 三档 op 各折出抓包里那两枚值：手动 `1/100`、照片备份 `2/200`、快照 `0/0`。
///
/// 快照那一档是 2026-10-07 抓包**新读到的**（`app-album-backup-samples.txt` §4）：
/// `0` 与 `0*100 = 0`。反向改坏它的样子：把 `Snapshot` 顺手归进 `Upload`（"反正都是 0 附近"）
/// ⇒ 这一条红，而真机上照片那一发是 `2/200`、快照那一发是 `0/0`，混了就有一发是错的。
#[test]
fn three_op_tiers_fold_to_the_captured_codes() {
    for (op, want_op, want_sub, why) in [
        (
            CloudOpType::Upload,
            "1",
            "100",
            "手动上传（app-upload-samples §5）",
        ),
        (
            CloudOpType::Backup,
            "2",
            "200",
            "照片备份（app-album-backup-samples §2）",
        ),
        (
            CloudOpType::Snapshot,
            "0",
            "0",
            "快照那一发 PUT（app-album-backup-samples §4）",
        ),
    ] {
        let req = cloud_file_op_headers(probe_request(), CloudChannel::Android, op);
        assert_eq!(header(&req, "x-yun-op-type"), Some(want_op), "{why}");
        assert_eq!(header(&req, "x-yun-sub-op-type"), Some(want_sub), "{why}");
        assert_eq!(header_count(&req, "x-yun-op-type"), 1, "{why}");
    }
}

/// ⭐ `CloudRenameMode` 的两枚字面量**逐字**是抓包读数，且零值是 `ForceRename`。
///
/// 零值为什么是它：`RefuseDuplicateContent` 是一条**会主动拒绝**的策略
/// （上游按内容哈希判重），谁忘了显式声明而静默落上去，表现是"任务莫名失败"。
#[test]
fn rename_mode_values_are_the_two_capture_readings() {
    assert_eq!(CloudRenameMode::ForceRename.value(), "force_rename");
    assert_eq!(
        CloudRenameMode::RefuseDuplicateContent.value(),
        "refuse_duplicate_content"
    );
    assert_ne!(
        CloudRenameMode::ForceRename.value(),
        CloudRenameMode::RefuseDuplicateContent.value()
    );
    assert_eq!(
        CloudRenameMode::default(),
        CloudRenameMode::ForceRename,
        "零值必须落在不会主动拒绝的那一侧"
    );
    // ⚠ 旧 TS 那个 `auto_rename` 在移动端两支实拍里**都不存在** —— 常量留着是为了
    // 记住这处不一致，不是"还有第三档可用"。
    assert_eq!(CLOUD_RENAME_MODE_AUTO, "auto_rename");
    assert_ne!(CLOUD_RENAME_MODE_AUTO, CloudRenameMode::ForceRename.value());
}

/// PC 档：另加四枚、且 **op 不被读**（backup 与 upload 逐枚相同）。
#[test]
fn pc_header_recipe_is_exact_and_ignores_op() {
    for op in ALL_OPS {
        let req = cloud_file_op_headers(probe_request(), CloudChannel::Pc, op);
        assert_eq!(header(&req, "x-yun-api-version"), Some("v1"));
        assert_eq!(header(&req, "x-yun-app-channel"), Some("10200153"));
        assert_eq!(
            header(&req, "x-yun-op-type"),
            Some("1"),
            "PC 档不读 op ⇒ {op:?} 也是 1"
        );
        assert_eq!(header(&req, "x-yun-market-source"), Some("001"));
        assert_eq!(header(&req, "x-yun-module-type"), Some("100"));
        assert_eq!(header(&req, "x-yun-svc-type"), Some("1"));
        assert_eq!(
            header(&req, "x-yun-client-info"),
            Some(CLOUD_CLIENT_INFO_PC)
        );
        assert_eq!(header(&req, "Content-Type"), Some(CLOUD_CONTENT_TYPE));
        for name in [
            "x-huawei-channelSrc",
            "x-yun-sub-op-type",
            "x-yun-device-id",
            "authorization",
            "deviceid",
            "user-agent",
        ] {
            assert_eq!(header_count(&req, name), 0, "{name} 不属于 PC 档");
        }
    }
}

/// ⭐⭐ web 档：**逐枚含顺序**与 `cloud_trash` 那条转发路径完全相同。
///
/// 这一条同时是 [`crate::cloud_trash::cloud_file_op_headers_web`] 的**行为不变**回归：
/// 改动前它自己逐枚设的就是下面这一串（连顺序都一样），现在它转发到本函数。
#[test]
fn web_header_recipe_is_exact_and_matches_cloud_trash() {
    let req = cloud_file_op_headers(probe_request(), CloudChannel::Web, CloudOpType::Upload);

    let expected = vec![
        ("x-yun-api-version".to_string(), "v1".to_string()),
        (
            "x-yun-app-channel".to_string(),
            CLOUD_CHANNEL_WEB.to_string(),
        ),
        ("x-yun-op-type".to_string(), "1".to_string()),
        (
            "x-huawei-channelSrc".to_string(),
            CLOUD_CHANNEL_WEB.to_string(),
        ),
        (
            "x-yun-client-info".to_string(),
            CLOUD_CLIENT_INFO_WEB.to_string(),
        ),
        (
            "x-yun-device-id".to_string(),
            CLOUD_CLIENT_INFO_WEB.to_string(),
        ),
        ("Content-Type".to_string(), CLOUD_CONTENT_TYPE.to_string()),
    ];
    assert_eq!(
        req.headers, expected,
        "web 档那一串（含顺序）必须与改动前相同"
    );

    // ⚠ 这一枚的值**就是**设备信息串（旧 TS 的笔误，但上游收到的就是它）。
    assert_eq!(
        header(&req, "x-yun-device-id"),
        header(&req, "x-yun-client-info")
    );

    // web 档**没有** `x-yun-sub-op-type`（那一枚只对 android 档生效），也没有 PC 那几枚。
    for name in [
        "x-yun-sub-op-type",
        "x-yun-market-source",
        "x-yun-module-type",
        "x-yun-svc-type",
    ] {
        assert_eq!(header_count(&req, name), 0, "{name} 不属于 web 档");
    }

    // 交叉验证：第四枪那条转发路径产出的头必须与这里逐字相同。
    let via_trash = crate::cloud_trash::cloud_file_op_headers_web(probe_request());
    assert_eq!(via_trash.headers, req.headers);
}

/// ⭐ 三档 × 两 op：**任何**同名头都不许出现两枚（旧 TS 是覆盖语义）。
#[test]
fn every_header_appears_at_most_once() {
    for channel in ALL_CHANNELS {
        for op in ALL_OPS {
            let req = cloud_file_op_headers(probe_request(), channel, op);
            for (name, _) in &req.headers {
                assert_eq!(
                    header_count(&req, name),
                    1,
                    "{channel:?}/{op:?} 的 {name} 出现了多枚 —— 旧 TS 的 `{{...base, k: v}}` 是覆盖"
                );
            }
            assert_eq!(header_count(&req, "content-type"), 1);
            assert_eq!(header(&req, "content-type"), Some(CLOUD_CONTENT_TYPE));
        }
    }
}

/// 这一族**从不**发签到那几枚头，也从不发 `jwttoken`。
///
/// 反向改坏它的样子：图省事改用签到族的 `h5()` 那套 ⇒ 这一条红，
/// 而且会把 `activityid` / 签到页 `referer` 渗进来 —— 那正是这一族从不发的头。
#[test]
fn headers_carry_no_jwt_or_signin_fields() {
    for channel in ALL_CHANNELS {
        for op in ALL_OPS {
            let req = cloud_file_op_headers(probe_request(), channel, op);
            for name in [
                "jwttoken",
                "activityid",
                "referer",
                "appversion",
                "showloading",
                "x-yun-tid",
            ] {
                assert_eq!(
                    header_count(&req, name),
                    0,
                    "{name} 不该由这一族发出（{channel:?}/{op:?}）"
                );
            }
        }
    }
}

/// ⭐ 三个请求构造器都**不自己设凭据 / UA**：那是发送层的活。
///
/// 这一条不是洁癖：发送层会按域名与 profile 再注入一次，而 reqwest 的 `.header()`
/// 是**追加** ⇒ 这里设了就会发出去两根同名头，上游读哪一根未测试。
/// 表现是「不报错、只是行为不对」，正是本仓最忌讳的那类。
#[test]
fn request_builders_leave_credentials_to_the_sending_layer() {
    let requests = vec![
        file_create_request(create_input()).expect("应能构造"),
        file_create_request(backup_create_input()).expect("应能构造"),
        folder_create_request(
            "",
            "asign-synth-dir",
            CloudChannel::Android,
            CloudOpType::Upload,
        )
        .expect("应能构造"),
        file_complete_request(
            "SYNTH-FID",
            "SYNTH-UID",
            &"a".repeat(64),
            CloudChannel::Android,
            CloudOpType::Upload,
        )
        .expect("应能构造"),
        snapshot_create_request(snapshot_create_input()).expect("应能构造"),
        snapshot_complete_request(snapshot_complete_input()).expect("应能构造"),
    ];
    for req in &requests {
        for name in ["authorization", "deviceid", "user-agent"] {
            assert_eq!(
                header_count(req, name),
                0,
                "{} 由发送层注入，这里设了会变成两根同名头",
                name
            );
        }
        // `cookie`（reqwest 的 cookie store 管）与 `x-yun-uni`（本仓没有它的来源）
        // 同样不该由**请求构造**层发：前者会与 store 里那一枚重名，
        // 后者是个 19 位真 userDomainId，编不出来。
        for name in ["cookie", "x-yun-uni"] {
            assert_eq!(header_count(req, name), 0, "{name} 不该由请求构造层发");
        }
    }
}

// ══════════════════════════ 第一枪：create 的体 ══════════════════════════

/// ⭐ **手动支**十键齐全，且两处「看着像拼错」的字面量在位。
///
/// 反向改坏它的样子：① 把体的 `contentType` 改成 `application/octet-stream`
/// （「顺手修个拼写」）⇒ 红；② 把 `fileRenameMode` 改成 `refuse`（那是**建目录**那一型的值）
/// ⇒ 红；③ 少发 `parallelUpload` ⇒ 键集合断言红。
///
/// ⚠ `fileRenameMode` 的期望值是 **`force_rename`**（抓包 `app-upload-samples.txt` §3），
/// **不是**旧 TS 那个 `auto_rename` —— 移动端实拍里没有后者（见 `CLOUD_RENAME_MODE_AUTO`）。
#[test]
fn file_create_body_is_the_legacy_ten_keys() {
    let req = file_create_request(manual_create_input()).expect("应能构造");
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://personal-kd-njs.yun.139.com/hcy/file/create"
    );

    let body = body_json(&req);
    assert_eq!(
        sorted_keys(&body),
        "contentHash,contentHashAlgorithm,contentType,fileRenameMode,name,parallelUpload,parentFileId,partInfos,size,type"
    );
    assert_eq!(body["type"], serde_json::json!(CLOUD_FILE_TYPE_FILE));
    assert_eq!(
        body["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_FORCE)
    );
    assert_eq!(
        body["contentHashAlgorithm"],
        serde_json::json!(CLOUD_HASH_ALGORITHM)
    );
    assert_eq!(
        body["contentType"],
        serde_json::json!("application/oct-stream")
    );
    assert_eq!(body["size"], serde_json::json!(64));
    assert_eq!(
        body["parallelUpload"],
        serde_json::json!(false),
        "parallelUpload 应是**布尔** false"
    );
    // 空 parentFileId ⇒ 补 "/"（旧 TS `apis/file.ts:57`）。
    assert_eq!(body["parentFileId"], serde_json::json!("/"));

    // 单分片：一项、partNumber 1、partSize 与 size 同一个值。
    let parts = body["partInfos"]
        .as_array()
        .expect("partInfos 应是一项数组");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["partNumber"], serde_json::json!(1));
    assert_eq!(parts[0]["partSize"], serde_json::json!(64));
    assert_eq!(
        parts[0]["parallelHashCtx"]["partOffset"],
        serde_json::json!(0)
    );
}

/// ⭐⭐ **备份支**（相册自动备份）的键集：抓包的 12 键**减去 `userRegion`**。
///
/// 抓包 `app-album-backup-samples.txt` §3 那 12 键是：
/// `contentType`、`contentHash`、`contentHashAlgorithm`、`fileRenameMode`、`localCreatedAt`、
/// `name`、`parallelUpload`、`parentPath`、`partInfos`、`size`、`type`、**`userRegion`**。
/// ⚠ 第 12 枚 `userRegion`（`{provinceCode, cityCode}`）本仓**没有省/市来源**
/// ⇒ 这一版**不发**（`file_create_request` 的文档里写了理由）⇒ 这里钉的是**11 键**，
/// 并且**显式断言 `userRegion` 不在** —— 不是"忘了它"，是"知道它在抓包里、也知道我们缺来源"。
#[test]
fn backup_create_body_is_the_captured_keys_minus_user_region() {
    let req = file_create_request(backup_create_input()).expect("应能构造");
    assert_eq!(
        req.url, "https://personal-kd-njs.yun.139.com/hcy/file/create",
        "两支同一口"
    );

    let body = body_json(&req);
    assert_eq!(
        sorted_keys(&body),
        "contentHash,contentHashAlgorithm,contentType,fileRenameMode,localCreatedAt,name,parallelUpload,parentPath,partInfos,size,type"
    );

    // ⭐ 定位目录：`parentPath` 在、`parentFileId` **不在**（抓包 §3）。
    assert_eq!(body["parentPath"], serde_json::json!("SYNTH"));
    assert!(
        body.get("parentFileId").is_none(),
        "备份支按路径定位 ⇒ **不发** parentFileId（两支二选一，抓包 §3）"
    );

    // ⭐ 三处档位读数。
    assert_eq!(
        body["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_REFUSE_DUPLICATE_CONTENT)
    );
    assert_eq!(
        body["parallelUpload"],
        serde_json::json!(true),
        "备份支的 parallelUpload 是**布尔 true**（抓包 §3）"
    );
    assert_eq!(
        body["contentType"],
        serde_json::json!(CLOUD_CREATE_CONTENT_TYPE_OCTET),
        "备份支这一枚是抓包里的**正确拼写**（与手动支那个少一个 e 的不是同一个读数）"
    );

    // 其余各格照旧。
    assert_eq!(body["type"], serde_json::json!(CLOUD_FILE_TYPE_FILE));
    assert_eq!(
        body["contentHashAlgorithm"],
        serde_json::json!(CLOUD_HASH_ALGORITHM)
    );
    assert_eq!(body["size"], serde_json::json!(64));
    assert_eq!(
        body["localCreatedAt"],
        serde_json::json!("2026-10-07T08:59:04.999Z")
    );
    let parts = body["partInfos"]
        .as_array()
        .expect("partInfos 应是一项数组");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["partNumber"], serde_json::json!(1));
    assert_eq!(parts[0]["partSize"], serde_json::json!(64));
    // ⭐ 单片时移动端**照样带** `parallelHashCtx{partOffset}`（抓包 §3）。
    assert_eq!(
        parts[0]["parallelHashCtx"]["partOffset"],
        serde_json::json!(0)
    );

    // ⚠ 抓包里有、本仓无来源 ⇒ 这一版不发（不是"漏了"）。
    assert!(
        body.get("userRegion").is_none(),
        "userRegion 抓包里有、本仓没有省/市来源 ⇒ 这一版不发（别编一个空值占位）"
    );
}

/// ⭐ **手动支**：`parent_path: None` ⇒ 仍然发 `parentFileId`、**不发** `parentPath`。
///
/// 反向改坏它的样子：把两支合并成"总是发 `parentPath`"（或总是两枚都发）
/// ⇒ 这一条红。抓包 §3 明写两支**二选一**（"有它就没有 `parentFileId`"）。
#[test]
fn manual_create_still_locates_by_parent_file_id() {
    let req = file_create_request(CloudFileCreateInput {
        parent_file_id: "SYNTH-DIR".into(),
        rename_mode: CloudRenameMode::ForceRename,
        ..create_input()
    })
    .expect("应能构造");
    let body = body_json(&req);

    assert_eq!(body["parentFileId"], serde_json::json!("SYNTH-DIR"));
    assert!(
        body.get("parentPath").is_none(),
        "手动支按 id 定位 ⇒ 不发 parentPath"
    );
    assert_eq!(
        body["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_FORCE)
    );
    assert!(
        body.get("localCreatedAt").is_none(),
        "local_created_at 是 Option：None ⇒ 这一键**不在**（不是空串）"
    );
}

/// `local_created_at` 是"`Some` 才发"：手动支也能带它（抓包两支都有这一枚）。
#[test]
fn local_created_at_is_sent_only_when_given() {
    let with = file_create_request(CloudFileCreateInput {
        local_created_at: Some("2026-10-06T22:54:22.999Z".into()),
        ..manual_create_input()
    })
    .expect("应能构造");
    assert_eq!(
        body_json(&with)["localCreatedAt"],
        serde_json::json!("2026-10-06T22:54:22.999Z")
    );

    let without = file_create_request(manual_create_input()).expect("应能构造");
    assert!(body_json(&without).get("localCreatedAt").is_none());
}

/// ⭐ 空的 `parent_path` 是**拒发**（不是"补根"）：它不是根，也不是一个能定位的目录。
#[test]
fn empty_parent_path_is_refused() {
    let err = file_create_request(CloudFileCreateInput {
        parent_path: Some(String::new()),
        ..backup_create_input()
    })
    .expect_err("空 parentPath 应当拒发");
    assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
    assert!(err.to_string().contains("parentPath"), "{err}");
}

/// 钉「空 ⇒ 根、非空 ⇒ 原样」。
#[test]
fn file_create_keeps_an_explicit_parent_folder() {
    let req = file_create_request(CloudFileCreateInput {
        parent_file_id: "SYNTH-DIR".into(),
        ..create_input()
    })
    .expect("应能构造");
    assert_eq!(
        body_json(&req)["parentFileId"],
        serde_json::json!("SYNTH-DIR")
    );
}

/// ⭐ 缺 `name` / `contentHash` ⇒ **不发**，而且是 [`Error::Config`]。
///
/// 旧 TS 没有这一道闸（它总是先 sha256、先起名），所以这两格在实拍里从不为空。
/// 在这里挡住是因为**返回错误比发一个空名请求更便宜**：后者会在用户网盘里留下一个
/// 没有名字的文件位，而清理那一族靠 `fileId` 认它。
#[test]
fn file_create_refuses_empty_name_or_hash() {
    let nameless = CloudFileCreateInput {
        name: String::new(),
        ..create_input()
    };
    let err = file_create_request(nameless).expect_err("空 name 应当拒发");
    assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
    assert!(err.to_string().contains("name"), "{err}");

    let hashless = CloudFileCreateInput {
        content_hash: String::new(),
        ..create_input()
    };
    let err = file_create_request(hashless).expect_err("空 contentHash 应当拒发");
    assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
    assert!(err.to_string().contains("contentHash"), "{err}");
}

/// 零值渠道落到旧 TS 的默认档 android；显式档不被覆盖。
#[test]
fn file_create_defaults_to_the_android_channel() {
    let req = file_create_request(create_input()).expect("应能构造");
    assert_eq!(header(&req, "x-yun-app-channel"), Some("10000023"));
    assert_eq!(header(&req, "x-huawei-channelSrc"), Some("10000023"));
    assert_eq!(header(&req, "x-yun-op-type"), Some("1"));
    assert_eq!(header(&req, "x-yun-sub-op-type"), Some("100"));

    let web = file_create_request(CloudFileCreateInput {
        channel: CloudChannel::Web,
        ..create_input()
    })
    .expect("应能构造");
    assert_eq!(header(&web, "x-yun-app-channel"), Some(CLOUD_CHANNEL_WEB));
    assert_eq!(header(&web, "x-yun-device-id"), Some(CLOUD_CLIENT_INFO_WEB));
}

/// 备份档要一路走到请求头上（`1/100` 与 `2/200` 两档都钉住）。
#[test]
fn file_create_honours_the_backup_op() {
    let req = file_create_request(CloudFileCreateInput {
        op: CloudOpType::Backup,
        ..create_input()
    })
    .expect("应能构造");
    assert_eq!(header(&req, "x-yun-op-type"), Some("2"));
    assert_eq!(header(&req, "x-yun-sub-op-type"), Some("200"));
}

// ══════════════════════════ 第一枪的另一型：建目录 ══════════════════════════

/// ⭐ 钉「这一型只有四枚键」。
///
/// 这一条**不是** [`file_create_body_is_the_legacy_ten_keys`] 的复制品：它的价值正好在
/// 「少那六枚」。反向改坏它的样子：把两型的 builder 合成一个、按 `Type` 开关 ⇒
/// `size` / `contentHash` / `partInfos` 会从文件型漏进目录型（键集合红）。
#[test]
fn folder_create_body_is_the_legacy_four_keys() {
    let req = folder_create_request(
        "",
        "asign-synth-dir",
        CloudChannel::Android,
        CloudOpType::Upload,
    )
    .expect("应能构造");
    assert_eq!(
        req.url, "https://personal-kd-njs.yun.139.com/hcy/file/create",
        "两型同一口"
    );

    let body = body_json(&req);
    assert_eq!(sorted_keys(&body), "fileRenameMode,name,parentFileId,type");
    assert_eq!(body["type"], serde_json::json!(CLOUD_FILE_TYPE_FOLDER));
    assert_eq!(
        body["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_REFUSE)
    );
    assert_eq!(body["parentFileId"], serde_json::json!("/"));

    // 文件型那六枚**一枚都不该在**。
    for key in [
        "size",
        "contentHash",
        "contentHashAlgorithm",
        "contentType",
        "parallelUpload",
        "partInfos",
    ] {
        assert!(
            body.get(key).is_none(),
            "folder 型不应带 {key}（那是文件型的键）"
        );
    }

    // 显式给父目录 ⇒ 不补根。
    let with_parent = folder_create_request(
        "SYNTH-PARENT",
        "asign-synth-dir",
        CloudChannel::Web,
        CloudOpType::Upload,
    )
    .expect("应能构造");
    assert_eq!(
        body_json(&with_parent)["parentFileId"],
        serde_json::json!("SYNTH-PARENT")
    );
    assert_eq!(
        header(&with_parent, "x-yun-app-channel"),
        Some(CLOUD_CHANNEL_WEB)
    );

    // 空名 ⇒ 拒发（发出去只会在用户网盘里留一个无名目录，而清理那一族靠 id 认它）。
    let err = folder_create_request("/", "", CloudChannel::Android, CloudOpType::Upload)
        .expect_err("空 name 应当拒发");
    assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
}

/// ⭐ **三型**的重名策略**不许互相搬**：手动支 `force_rename`、备份支
/// `refuse_duplicate_content`、目录型 `refuse`。
///
/// ⚠ 判据一律用**精确取值**（`body["fileRenameMode"] == …`），**不用 `contains`**：
/// `refuse_duplicate_content` 里含有子串 `refuse` ⇒ 拿子串比会把两支判成同一档，
/// 而那正是这一条要防的事。
///
/// 参考实现明确警告过：`refuse` 只在**目录型**上出现过（实拍与真跑都是），
/// 「同名**文件**用 refuse 会怎样」**零样本**。
#[test]
fn rename_modes_are_not_interchangeable() {
    let manual = body_json(&file_create_request(manual_create_input()).expect("应能构造"));
    let backup = body_json(&file_create_request(backup_create_input()).expect("应能构造"));
    let folder = body_json(
        &folder_create_request(
            "",
            "asign-synth-dir",
            CloudChannel::Android,
            CloudOpType::Upload,
        )
        .expect("应能构造"),
    );

    assert_eq!(
        manual["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_FORCE)
    );
    assert_eq!(
        backup["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_REFUSE_DUPLICATE_CONTENT)
    );
    assert_eq!(
        folder["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_REFUSE)
    );

    // 三档两两不等（子串关系不算相等 —— 这里比的是**整串**）。
    assert_ne!(manual["fileRenameMode"], backup["fileRenameMode"]);
    assert_ne!(backup["fileRenameMode"], folder["fileRenameMode"]);
    assert_ne!(manual["fileRenameMode"], folder["fileRenameMode"]);

    // 文件型（两支）都**不**该带目录型那个 `refuse`。
    for body in [&manual, &backup] {
        assert_ne!(
            body["fileRenameMode"],
            serde_json::json!(CLOUD_RENAME_MODE_REFUSE),
            "refuse 只在目录型上出现过，别搬去传文件"
        );
    }
    // 目录型也不该带文件型那两档。
    assert_ne!(
        folder["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_FORCE)
    );
    assert_ne!(
        folder["fileRenameMode"],
        serde_json::json!(CLOUD_RENAME_MODE_AUTO)
    );
}

// ══════════════════════════ 第三枪：complete ══════════════════════════

/// ⭐ `complete` 的体四键，且 `create` 与 `complete` 用**同一个串**。
#[test]
fn create_and_complete_share_one_hash_string() {
    // 大小写混着：也钉「不做归一化、不改大小写」。
    let hash = format!("{}{}", "b".repeat(32), "C".repeat(32));
    let create = file_create_request(CloudFileCreateInput {
        content_hash: hash.clone(),
        ..create_input()
    })
    .expect("应能构造");
    let complete = file_complete_request(
        "SYNTH-FID",
        "SYNTH-UID",
        &hash,
        CloudChannel::Android,
        CloudOpType::Upload,
    )
    .expect("应能构造");

    let create_body = body_json(&create);
    let complete_body = body_json(&complete);
    assert_eq!(create_body["contentHash"], serde_json::json!(hash));
    assert_eq!(complete_body["contentHash"], serde_json::json!(hash));
    assert_eq!(
        create_body["contentHashAlgorithm"], complete_body["contentHashAlgorithm"],
        "两枪的算法名必须同一枚"
    );
    assert_eq!(
        complete_body["contentHashAlgorithm"],
        serde_json::json!("SHA256")
    );
}

/// `complete` 的四键与「三个位置参数逐格对位」。
#[test]
fn complete_body_is_the_legacy_four_keys() {
    let req = file_complete_request(
        "SYNTH-FID",
        "SYNTH-UID",
        "SYNTH-HASH",
        CloudChannel::Android,
        CloudOpType::Upload,
    )
    .expect("应能构造");
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://personal-kd-njs.yun.139.com/hcy/file/complete"
    );

    let body = body_json(&req);
    assert_eq!(
        sorted_keys(&body),
        "contentHash,contentHashAlgorithm,fileId,uploadId"
    );
    // ⚠ 三个 `&str` 位置参数换错顺序**能编译通过** ⇒ 用不重样的哨兵值逐格钉住。
    assert_eq!(body["fileId"], serde_json::json!("SYNTH-FID"));
    assert_eq!(body["uploadId"], serde_json::json!("SYNTH-UID"));
    assert_eq!(body["contentHash"], serde_json::json!("SYNTH-HASH"));
}

/// ⭐ 缺 `fileId` 或 `contentHash` ⇒ 回错误，**不发空请求**。
///
/// 「不发错、不报错，只是行为不对」正是静默发一枪空请求的样子：上游可能回成功，
/// 而没有任何东西被完成。
#[test]
fn complete_refuses_empty_file_id_or_hash() {
    for (file_id, hash) in [("", "SYNTH-HASH"), ("SYNTH-FID", "")] {
        let err = file_complete_request(
            file_id,
            "SYNTH-UID",
            hash,
            CloudChannel::Android,
            CloudOpType::Upload,
        )
        .expect_err("缺依据应当拒发");
        assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
        let text = err.to_string();
        assert!(
            text.contains("fileId") && text.contains("contentHash"),
            "错误里要说明缺的是哪两格之一：{text}"
        );
    }
}

/// `uploadId` 为空**照样发**（空串），旧实现不判空。
#[test]
fn complete_sends_an_empty_upload_id_as_empty_string() {
    let req = file_complete_request(
        "SYNTH-FID",
        "",
        "SYNTH-HASH",
        CloudChannel::Android,
        CloudOpType::Upload,
    )
    .expect("空 uploadId 今天不该拒发（旧实现 stringOf(null) ⇒ 空串）");
    let body = body_json(&req);
    assert_eq!(body["uploadId"], serde_json::json!(""));
    assert!(
        body.get("uploadId").is_some(),
        "这一格**在**，只是空串 —— 不是缺键，也不是 null"
    );
}

// ══════════════════════════ 第二枪：PUT ══════════════════════════

/// ⭐ `PUT` 那一枪：方法、**裸字节**体、抓包 12 套里本层装配的那几枚。
///
/// 抓包 `app-album-backup-samples.txt` §4 把两发 EOS `PUT` 数成 12 枚；本层只装配
/// **有读数**的 6 枚（`content-type` / `accept-language` / `upload-complete` /
/// `upload-draft-interop-version` / 两枚 op 头），其余归传输层或发送层 ——
/// 逐枚去向写在 [`upload_put_request_with_op`] 的文档里。
#[test]
fn upload_put_is_a_raw_byte_put() {
    // 故意混进非 UTF-8 字节：它证明这一枪装的是原始字节，不是文本。
    let content: &[u8] = &[0x00, 0xff, 0xfe, 0x80, b'a'];
    let req = upload_put_request(SYNTH_PUT_URL, content);

    assert_eq!(req.method, "PUT");
    assert_eq!(req.url, SYNTH_PUT_URL, "URL 必须原样转发（不解析、不改写）");
    assert!(req.has_body());
    assert_eq!(req.body.as_deref(), Some(content), "必须原样是那串字节");
    assert_eq!(
        req.body_str(),
        None,
        "二进制体读不成文本 —— 这**不是**「没有体」"
    );

    assert_eq!(header(&req, "content-type"), Some(CLOUD_PUT_CONTENT_TYPE));
    assert_eq!(
        header_count(&req, "content-type"),
        1,
        "这一枪自带 content-type，发送层不会再补一枚"
    );

    // ⚠ 本版**不发** `origin` / `referer`（旧 TS 有，抓包那 12 枚里没有）。
    // 反向改坏它的样子：把两枚加回来 ⇒ 这一条红 —— 那正是"这一处与旧 TS 的分歧"
    // 被无声抹掉的样子。
    assert_eq!(
        header_count(&req, "origin"),
        0,
        "抓包的两发 PUT 里没有 origin（旧 TS 的 CORS 头）"
    );
    assert_eq!(
        header_count(&req, "referer"),
        0,
        "抓包的两发 PUT 里没有 referer"
    );
}

/// ⭐⭐ 数据面那两枚**档位头**：三档逐档对拍，且**恰好一枚**。
///
/// 抓包 §4/§5：照片那发 `2/200`、快照那发 `0/0`、手动那发 `1/100`。
#[test]
fn upload_put_carries_the_two_op_codes_for_each_tier() {
    for (op, want_op, want_sub, why) in [
        (CloudOpType::Upload, "1", "100", "手动上传那一发"),
        (CloudOpType::Backup, "2", "200", "照片备份那一发"),
        (CloudOpType::Snapshot, "0", "0", "快照那一发（ot=system）"),
    ] {
        let req = upload_put_request_with_op(SYNTH_PUT_URL, b"SYNTH", op);
        assert_eq!(header(&req, "x-yun-op-type"), Some(want_op), "{why}");
        assert_eq!(header(&req, "x-yun-sub-op-type"), Some(want_sub), "{why}");
        assert_eq!(header_count(&req, "x-yun-op-type"), 1, "{why}");
        assert_eq!(header_count(&req, "x-yun-sub-op-type"), 1, "{why}");
    }
}

/// ⭐ 12 套里那四枚**移动端独有**的头：`upload-complete: ?1`、
/// `upload-draft-interop-version: 6`、`accept-language`，加上拼写正常的 `content-type`。
///
/// ⚠ `?1` 里那个问号**是值的一部分**（形状像 HTTP Uploadable 草案那一族）——
/// 谁"顺手"把它去掉，这一条就红。
#[test]
fn upload_put_carries_the_mobile_only_headers() {
    let req = upload_put_request_with_op(SYNTH_PUT_URL, b"SYNTH", CloudOpType::Snapshot);

    assert_eq!(header(&req, "upload-complete"), Some("?1"));
    assert_eq!(header(&req, "upload-draft-interop-version"), Some("6"));
    assert_eq!(header(&req, "accept-language"), Some(CLOUD_ACCEPT_LANGUAGE));
    assert_eq!(header(&req, "content-type"), Some(CLOUD_PUT_CONTENT_TYPE));
    // 常量本身也钉住（免得只跟常量比对，"顺手改常量"也照样绿）。
    assert_eq!(CLOUD_PUT_UPLOAD_COMPLETE, "?1");
    assert_eq!(CLOUD_PUT_UPLOAD_DRAFT_INTEROP_VERSION, "6");

    // ⚠ 这一枪**零 mcloud 凭据**（抓包 §7）：授权整套在查询串上。
    for name in [
        "authorization",
        "cookie",
        "x-yun-uni",
        "x-yun-client-info",
        "x-huawei-channelSrc",
        "x-yun-api-version",
        "x-yun-app-channel",
        "deviceid",
        "user-agent",
    ] {
        assert_eq!(
            header_count(&req, name),
            0,
            "{name} 不属于数据面这一族（零 mcloud 凭据）"
        );
    }
    // 也不套 20/25 那两套里的任何一枚（`x-yun-op-type` / `sub-op-type` 除外）。
    assert_eq!(header_count(&req, "Pragma"), 0);
    assert_eq!(header_count(&req, "cache-control"), 0);
}

/// ⭐ 0 字节也是**带体**（发出 `content-length: 0`），不是无体。
#[test]
fn upload_put_keeps_an_empty_body_as_a_body() {
    let req = upload_put_request(SYNTH_PUT_URL, &[]);
    assert!(
        req.has_body(),
        "Some(vec![]) 与 None 语义不同：判据只看 is_some()"
    );
    assert_eq!(req.body.as_deref(), Some(&[][..]));
    assert_eq!(req.body_str(), Some(""));
}

/// 凭据规则的两侧：外站不带、命中四段后缀才带、坏 URL / 相对路径一律不带。
#[test]
fn authorization_criterion_follows_the_legacy_suffix_rule() {
    for (_why, url, want) in [
        ("外站预签名 URL 不带凭据", SYNTH_PUT_URL, false),
        ("命中 yun.139.com 后缀", SYNTH_KD_URL, true),
        (
            "mcloud.139.com 也是四段之一",
            "https://mcloud.139.com/x",
            true,
        ),
        (
            "feixin.10086.cn 也是四段之一",
            "https://feixin.10086.cn/x",
            true,
        ),
        (
            "mail.10086.cn 也是四段之一",
            "https://mail.10086.cn/x",
            true,
        ),
        (
            "坏 URL 一律不带（宁可缺凭据，不可错发）",
            "::not a url::",
            false,
        ),
        ("相对路径没有主机 ⇒ 不带", "/hcy/put/SYNTH", false),
        (
            "后缀必须落在主机结尾才行",
            "https://yun.139.com.evil.example/x",
            false,
        ),
    ] {
        assert_eq!(sends_authorization_to(url), want, "{url}");
    }
}

/// ⭐ 本函数（参考实现的裸后缀）比**真上线**那条规则更松：差异只在边界格。
///
/// [`crate::endpoints::accepts_basic`] 要求域名边界 ⇒ `evilyun.139.com` 不算命中；
/// 而 Go 的 `strings.HasSuffix` 算命中。真正把凭据发出去的是**更严**的那一条
/// （发送层），这一侧是「少发一枚凭据」，可回滚。
#[test]
fn authorization_criterion_is_looser_than_the_sending_layer() {
    let sneaky = "https://evilyun.139.com/hcy/put/SYNTH";
    assert!(
        sends_authorization_to(sneaky),
        "参考实现那条（裸后缀）会命中"
    );
    assert!(
        !crate::endpoints::accepts_basic("evilyun.139.com"),
        "发送层那条（域名边界）不命中 —— 真上线的、更严的是它"
    );
}

/// `PUT` 的结局只看状态码，且错误文本**不带** URL 与响应体原文。
#[test]
fn ensure_put_ok_only_looks_at_the_status() {
    for status in [200u16, 204, 299] {
        assert!(
            ensure_put_ok(&response(status, "")).is_ok(),
            "2xx 就是成了（响应不是 JSON 也算）"
        );
    }
    for status in [300u16, 403, 500] {
        let err =
            ensure_put_ok(&response(status, "SYNTH-BODY-TOKEN")).expect_err("非 2xx 必须算失败");
        assert!(matches!(err, Error::Network(_)), "实际：{err:?}");
        let text = err.to_string();
        assert!(text.contains(&status.to_string()), "{text}");
        assert!(
            !text.contains("SYNTH-BODY-TOKEN"),
            "错误文本不带响应体原文（上游错误体常回显凭据）：{text}"
        );
    }
}

// ══════════════════════════ 回执：create / complete ══════════════════════════

/// ⭐ 载荷在 `data`、`code` 是字符串 `"0000"`（合成值，形状取自实拍那两条的字段集合）。
#[test]
fn parse_file_create_reads_the_data_payload() {
    let body = r#"{"code":"0000","message":"请求成功","success":true,"data":{"exist":false,"fileId":"SYNTH-FID","fileName":"asign-synth.png","formInfo":null,"parentFileId":"/","partInfos":[{"uploadUrl":"https://synth-oss.example.invalid/put"}],"rapidUpload":false,"transferToken":null,"type":"file","uploadId":"SYNTH-UID"}}"#;
    let created = parse_file_create(body).expect("应能解析");

    assert_eq!(created.file_id.as_deref(), Some("SYNTH-FID"));
    assert_eq!(created.upload_id.as_deref(), Some("SYNTH-UID"));
    assert_eq!(
        created.upload_url.as_deref(),
        Some("https://synth-oss.example.invalid/put"),
        "取的是 partInfos[0].uploadUrl"
    );
    assert_eq!(created.exist, Some(false), "exist:false 该读成明确的 false");
    assert!(!created.rapid_upload);
}

/// ⭐⭐ `owned_by_us()` —— "这一发是不是我们造的"那三态，**七种情形逐条钉住** ✓。
///
/// ⚠⚠ 其中第 ① 条是**回归钉子**：2026-10-05 真机，上游把"没撞同名"从 `exist:false`
/// 改成了 **`exist:null`** ✗ ⇒ 只看 `exist` 那一格的写法把**每一发** create 都判成
/// "判不了"（86 发全废 ✗：106 / 113 / 522 / 615 与 434 的补传一起 ✗）。
/// 判据因此是"**有没有开出新的上传会话**"✓ —— 下面每一条都在钉这个判据的一角 ✓。
#[test]
fn owned_by_us_is_a_three_state_with_a_session_based_null() {
    let mk = |data: &str| {
        let body =
            format!(r#"{{"code":"0000","message":"请求成功","success":true,"data":{data}}}"#);
        parse_file_create(&body).expect("应能解析").owned_by_us()
    };

    // ① **2026-10-05 的实拍形状**：`exist:null` + 新上传会话 ⇒ **是我们的** ✓（回归钉子）
    assert_eq!(
        mk(
            r#"{"exist":null,"fileId":"F","partInfos":[{"uploadUrl":"https://x.invalid/put"}],"rapidUpload":false,"uploadId":"U"}"#
        ),
        Some(true),
        "⚠ exist:null 但开出了新上传会话 ⇒ 那个 fileId 是我们刚造的（2026-10-05 实拍 ✓）"
    );
    // ② 老形状：`exist:false` ⇒ 我们的 ✓
    assert_eq!(
        mk(
            r#"{"exist":false,"fileId":"F","partInfos":[{"uploadUrl":"https://x.invalid/put"}],"uploadId":"U"}"#
        ),
        Some(true)
    );
    // ③ `exist:true` ⇒ **撞同名，不是我们的**（优先级最高，哪怕带了会话也不认 ✗）
    assert_eq!(
        mk(
            r#"{"exist":true,"fileId":"F","partInfos":[{"uploadUrl":"https://x.invalid/put"}],"uploadId":"U"}"#
        ),
        Some(false),
        "撞同名那一枚是用户自己的文件 ⇒ 绝不进清理名单"
    );
    // ④ `exist:null` **且没有 uploadUrl**（`refuse` 语义那一档的形状）⇒ 判不了 ✓
    assert_eq!(
        mk(r#"{"exist":null,"fileId":"F","partInfos":null,"rapidUpload":false,"uploadId":"U"}"#),
        None,
        "没有上传会话 ⇒ 判不了 ⇒ 绝不删"
    );
    // ⑤ `exist:null` **且没有 uploadId** ⇒ 判不了 ✓
    assert_eq!(
        mk(
            r#"{"exist":null,"fileId":"F","partInfos":[{"uploadUrl":"https://x.invalid/put"}],"uploadId":null}"#
        ),
        None
    );
    // ⑥ `exist` 整格**缺席**、但有会话 ⇒ 我们的 ✓（形状再变一格也不至于全废 ✓）
    assert_eq!(
        mk(r#"{"fileId":"F","partInfos":[{"uploadUrl":"https://x.invalid/put"}],"uploadId":"U"}"#),
        Some(true)
    );
    // ⑦ 秒传：`exist:null` + `rapidUpload:true` + uploadId ⇒ 我们的 ✓
    //（内容是随机字节 ⇒ 秒传命中的也是"这一发造出来的"那个 ✓）
    assert_eq!(
        mk(r#"{"exist":null,"fileId":"F","partInfos":null,"rapidUpload":true,"uploadId":"U"}"#),
        Some(true)
    );
}

/// 钉「实拍那两条的形状交回来是什么」：`exist:true`、`uploadId:null`、`partInfos:null`。
///
/// 这一条的价值在于「以后有人拿 `file/create` 复用来建目录」时，本层不会 panic，
/// 也**不**把 `null` 编造成空串。
#[test]
fn parse_file_create_handles_the_captured_folder_shape() {
    let body = r#"{"code":"0000","success":true,"message":"请求成功","data":{"exist":true,"fileId":"SYNTH-DIR","formInfo":null,"parentFileId":"/","partInfos":null,"rapidUpload":false,"type":"folder","uploadId":null}}"#;
    let created = parse_file_create(body).expect("folder 型不该报错");

    assert_eq!(created.exist, Some(true), "exist:true 该读出来");
    assert_eq!(created.file_id.as_deref(), Some("SYNTH-DIR"));
    assert_eq!(created.upload_id, None, "null 不该被编造成空串");
    assert_eq!(created.upload_url, None, "partInfos:null ⇒ 没有 URL");
}

/// ⭐ 三格「没有」的处置：`data` 缺失 / 不是对象 / `partInfos` 为空数组。
#[test]
fn parse_file_create_distinguishes_missing_from_empty() {
    // `data` 整个缺失 ⇒ 解析错误（旧实现是 `createResult.data` 直接解构会抛）。
    let err = parse_file_create(r#"{"code":"0000","success":true}"#)
        .expect_err("没有 data 应当报错，而不是交回一个全空结构");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");
    assert!(err.to_string().contains("data"), "{err}");

    // `data` 不是对象。
    let err = parse_file_create(r#"{"code":"0000","success":true,"data":[]}"#)
        .expect_err("data 是数组应当报错");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");

    // `partInfos` 是空数组 ⇒ `upload_url` 为 None，而不是空串。
    let created = parse_file_create(
        r#"{"code":"0000","success":true,"data":{"partInfos":[],"fileId":"SYNTH"}}"#,
    )
    .expect("应能解析");
    assert_eq!(created.upload_url, None);
    assert_eq!(created.file_id.as_deref(), Some("SYNTH"));

    // `partInfos` 第一项不是对象 ⇒ 同样交回 None，不 panic。
    let created =
        parse_file_create(r#"{"code":"0000","success":true,"data":{"partInfos":["nope"]}}"#)
            .expect("应能解析");
    assert_eq!(created.upload_url, None);

    // `uploadUrl` 是 null ⇒ None。
    let created = parse_file_create(
        r#"{"code":"0000","success":true,"data":{"partInfos":[{"uploadUrl":null}]}}"#,
    )
    .expect("应能解析");
    assert_eq!(created.upload_url, None);

    // ⭐ `success:false` 优先于载荷 ⇒ 必须是错误（哪怕 data 齐、code 是 "0000"）。
    let err = parse_file_create(
        r#"{"code":"0000","success":false,"message":"容量不足","data":{"fileId":"SYNTH"}}"#,
    )
    .expect_err("success:false 必须是错误");
    assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");
    assert!(
        err.to_string().contains("容量不足"),
        "上游原话要带出来：{err}"
    );

    // `exist` 是数字 ⇒ **None**（形状变了，且这一格决定"能不能删"，不能兜成 false）；
    // `rapidUpload` 是数字 ⇒ 读成 false（兜成 false 只是多走一遍 PUT，无害）。
    let weird = parse_file_create(
        r#"{"code":"0000","success":true,"data":{"exist":1,"rapidUpload":"true"}}"#,
    )
    .expect("应能解析");
    assert_eq!(
        weird.exist, None,
        "exist:1 不是布尔 ⇒ 必须如实回 None（兜成 false 会删用户的文件）"
    );
    assert!(!weird.rapid_upload, "rapidUpload:\"true\" 不该被念成 true");
}

/// ⭐ 非 `"0000"` 的码走错误分支，且错误里带着那个码与上游原话。
#[test]
fn parse_file_create_rejects_non_zero_code_and_surfaces_msg() {
    for body in [
        r#"{"code":"1","success":true,"message":"失败"}"#,
        r#"{"code":"1001","msg":"参数错误"}"#,
    ] {
        let err = parse_file_create(body).expect_err("非 0000 应是失败");
        match err {
            Error::Upstream { ref code, .. } => {
                assert!(code == "1" || code == "1001", "码要带出来，实得 {code}");
            }
            other => panic!("应是 Upstream，实际：{other:?}"),
        }
        let text = err.to_string();
        assert!(
            text.contains("失败") || text.contains("参数错误"),
            "两个键（message / msg）都要认：{text}"
        );
    }
}

/// ⭐ 数字 `0` **不算** `"0000"`（这一族的 `code` 是**字符串**）。
///
/// 拿错信封（签到族那个数字 `code` 的解析器）会让这一族**永远判失败**；
/// 反过来认数字又会让「上游换了信封」静默通过。与
/// [`crate::cloud_trash::parse_batch_trash`] 同一口径。
#[test]
fn parse_file_create_rejects_a_numeric_code() {
    let err = parse_file_create(r#"{"code":0,"success":true,"data":{"fileId":"SYNTH"}}"#)
        .expect_err("数字 0 不是这一族的成功码");
    assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");

    // `code` 整个缺席 ⇒ 也**不**算成功（参考实现把这一格判成功，本层刻意收紧）。
    let err = parse_file_create(r#"{"success":true,"data":{"fileId":"SYNTH"}}"#)
        .expect_err("缺 code 这一格不该念成成功");
    assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");
}

#[test]
fn parse_file_create_rejects_non_json() {
    let err = parse_file_create("<html>502 Bad Gateway</html>").expect_err("非 JSON 必须算失败");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");
}

/// 这一族**不声明** `complete` 的字段：成功只看信封。
#[test]
fn parse_file_complete_only_judges_the_envelope() {
    assert!(parse_file_complete(r#"{"code":"0000","success":true}"#).is_ok());
    assert!(
        parse_file_complete(r#"{"code":"0000"}"#).is_ok(),
        "这一族不要求 success 存在"
    );
    assert!(
        parse_file_complete(r#"{"code":"0000","success":true,"data":{"anything":1}}"#).is_ok(),
        "data 的形状这一口今天不读（零样本），不该因此报错"
    );

    let err = parse_file_complete(r#"{"code":"0000","success":false,"message":"校验失败"}"#)
        .expect_err("success:false 应是错误");
    assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");
    assert!(err.to_string().contains("校验失败"), "{err}");

    let err = parse_file_complete(r#"{"code":"90001","msg":"未登录"}"#).expect_err("非 0 码应失败");
    assert!(err.to_string().contains("90001"), "{err}");
    assert!(err.to_string().contains("未登录"), "{err}");
}

// ══════════════════ 快照那一支：backup/createSnapshot · completeSnapshot ══════════════════
//
// 依据：`../mcloud-capture/docs/capture/app-album-backup-samples.txt` §6（2026-10-07 真机）。
// ⚠ 这两口在历史样本里**从没出现过**，所以下面每一格都标了"读数 / 无读数"。

/// ⭐⭐ `createSnapshot` 的体**恰好 7 键**，且 `partInfos[0]` **没有** `parallelHashCtx`。
///
/// 反向改坏它的样子：把照片那一支的 `partInfos` 构造函数搬过来
/// （它带 `parallelHashCtx{partOffset}`）⇒ 键集合断言红。
/// 抓包 §6 括号里逐字写了这一格与照片那一支不同。
#[test]
fn snapshot_create_body_is_the_captured_seven_keys() {
    let req = snapshot_create_request(snapshot_create_input()).expect("应能构造");
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://personal-kd-njs.yun.139.com/hcy/backup/createSnapshot"
    );

    let body = body_json(&req);
    assert_eq!(
        sorted_keys(&body),
        "contentHash,contentHashAlgorithm,count,fileExtension,partInfos,size,timestamp"
    );
    assert_eq!(body["fileExtension"], serde_json::json!("zip"));
    assert_eq!(body["count"], serde_json::json!(5));
    assert_eq!(body["size"], serde_json::json!(619));
    assert_eq!(
        body["contentHashAlgorithm"],
        serde_json::json!(CLOUD_HASH_ALGORITHM)
    );
    assert_eq!(
        body["timestamp"],
        serde_json::json!(1_759_808_345_332_i64),
        "13 位毫秒，**不是**微秒"
    );

    let parts = body["partInfos"]
        .as_array()
        .expect("partInfos 应是一项数组");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["partNumber"], serde_json::json!(1));
    assert_eq!(parts[0]["partSize"], serde_json::json!(619));
    assert!(
        parts[0].get("parallelHashCtx").is_none(),
        "快照那一支的 partInfos **没有** parallelHashCtx（抓包 §6 明写与照片那一支不同）"
    );
    // 照片那一支有、这一支没有的其余几枚。
    for key in [
        "parentFileId",
        "parentPath",
        "name",
        "type",
        "parallelUpload",
    ] {
        assert!(body.get(key).is_none(), "快照那一支不该带 {key}");
    }
}

/// ⭐⭐ **25 套头**：`backup/*` 那两口**没有** op 两枚，而**有**那 8 枚里的几枚。
///
/// 抓包 §7 的差集表是判据本身：
/// `create` 有而 `backup/*` 没有的是 `cache-control` / `x-yun-op-type` / `x-yun-sub-op-type`；
/// `backup/*` 有而 `create` 没有的是 `pragma` / `service_identifier_key` / `x-DeviceInfo` /
/// `x-DeviceName` / `x-huawei-channelSrc` / `x-SvcType` / `x-UserAgent` / `x-yun-user-timestamp`。
///
/// ⚠ 那 8 枚里本层**只装配 5 枚**（`x-DeviceInfo` 与 `x-yun-user-timestamp` 本仓没有来源，
/// 见 [`snapshot_op_headers`] 那张表）—— 所以这里断言的是"**在的那几枚在、
/// 不在的那几枚明说不在**"，不是"8 枚齐了"。
#[test]
fn snapshot_headers_are_the_twenty_five_set() {
    for req in [
        snapshot_create_request(snapshot_create_input()).expect("应能构造"),
        snapshot_complete_request(snapshot_complete_input()).expect("应能构造"),
    ] {
        // ⭐ 这一套**不带** op 两枚（与 20 套、与数据面 12 套都不同）。
        for name in ["x-yun-op-type", "x-yun-sub-op-type", "cache-control"] {
            assert_eq!(
                header_count(&req, name),
                0,
                "{name} 是 20 套那一边的，backup/* 这两口不带"
            );
        }

        // ⭐ 那 8 枚里本层装配的几枚。
        assert_eq!(header(&req, "Pragma"), Some("no-cache"));
        assert_eq!(
            header(&req, "service_identifier_key"),
            Some(CLOUD_SERVICE_IDENTIFIER_CLOUD_DISK)
        );
        assert_eq!(
            CLOUD_SERVICE_IDENTIFIER_CLOUD_DISK, "service_identifier_cloudDisk",
            "这一枚是**按主机查表**查出来的（client-profiles-ios-app.md §3.3.3）"
        );
        assert_eq!(header(&req, "x-SvcType"), Some("1"));
        assert_eq!(header(&req, "x-huawei-channelSrc"), Some("10000023"));
        assert_eq!(header(&req, "x-DeviceName"), Some(APP_DEVICE_NAME_B64));
        assert_eq!(header(&req, "x-UserAgent"), Some(APP_X_USER_AGENT));
        assert_eq!(header(&req, "x-yun-user-agent"), Some(APP_X_YUN_USER_AGENT));

        // ⚠ 本仓**没有来源**的两枚：明说不在（不是"忘了"）。
        // `x-DeviceInfo` 要一个设备 id、`x-yun-user-timestamp` 要一个 16 位微秒戳，
        // 而这一层的入参里都没有。
        for name in ["x-DeviceInfo", "x-yun-user-timestamp", "x-yun-client-info"] {
            assert_eq!(
                header_count(&req, name),
                0,
                "{name} 本仓没有来源（入参里没有设备 id / 微秒戳）⇒ 这一版不发"
            );
        }

        // 其余各格。
        assert_eq!(header(&req, "x-yun-api-version"), Some("v2"));
        assert_eq!(header(&req, "x-yun-app-channel"), Some("10000023"));
        assert_eq!(header(&req, "x-yun-net-type"), Some("1"));
        assert_eq!(header(&req, "x-yun-svc-type"), Some("1"));
        assert_eq!(header(&req, "Accept-Language"), Some(CLOUD_ACCEPT_LANGUAGE));
        assert_eq!(header(&req, "Content-Type"), Some(CLOUD_CONTENT_TYPE));
        assert_eq!(
            header_count(&req, "content-type"),
            1,
            "覆盖式：不许出现两根 content-type"
        );
        // 同名头一枚都不许有两根。
        for (name, _) in &req.headers {
            assert_eq!(header_count(&req, name), 1, "{name} 出现了多枚");
        }
    }
}

/// ⭐ 25 套里的渠道那两枚跟着入参走；`complete` 那一发（契约里没有 `channel`）按默认档。
#[test]
fn snapshot_headers_follow_the_channel_argument() {
    let web = snapshot_create_request(CloudSnapshotCreateInput {
        channel: CloudChannel::Web,
        ..snapshot_create_input()
    })
    .expect("应能构造");
    assert_eq!(header(&web, "x-yun-app-channel"), Some(CLOUD_CHANNEL_WEB));
    assert_eq!(header(&web, "x-huawei-channelSrc"), Some(CLOUD_CHANNEL_WEB));

    // `completeSnapshot` 的契约里没有 channel ⇒ 落 `CloudChannel::default()`（android）。
    let complete = snapshot_complete_request(snapshot_complete_input()).expect("应能构造");
    assert_eq!(
        header(&complete, "x-yun-app-channel"),
        Some(CloudChannel::default().value())
    );
}

/// ⭐ `completeSnapshot` 的体**恰好 7 键**，且各格逐字对位。
#[test]
fn snapshot_complete_body_is_the_captured_seven_keys() {
    let req = snapshot_complete_request(snapshot_complete_input()).expect("应能构造");
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://personal-kd-njs.yun.139.com/hcy/backup/completeSnapshot"
    );

    let body = body_json(&req);
    assert_eq!(
        sorted_keys(&body),
        "contentHash,contentHashAlgorithm,count,fileId,size,timestamp,uploadId"
    );
    assert_eq!(body["fileId"], serde_json::json!(SYNTH_SNAPSHOT_FILE_ID));
    assert_eq!(
        body["uploadId"],
        serde_json::json!(SYNTH_SNAPSHOT_UPLOAD_ID)
    );
    assert_eq!(body["count"], serde_json::json!(5));
    assert_eq!(body["size"], serde_json::json!(619));
    assert_eq!(
        body["contentHashAlgorithm"],
        serde_json::json!(CLOUD_HASH_ALGORITHM)
    );
    assert_eq!(body["timestamp"], serde_json::json!(1_759_808_345_332_i64));
    // 快照那两口**不带** `fileExtension` / `partInfos`（那是 create 那一枪的键）。
    for key in ["fileExtension", "partInfos"] {
        assert!(body.get(key).is_none(), "completeSnapshot 不该带 {key}");
    }
}

/// ⭐ 缺依据 ⇒ 拒发，而且是 [`Error::Config`]。
///
/// 这一条与 `file_create` / `file_complete` 同一口径：静默发一枪空请求的表现是
/// 「上游回了成功、但没有任何东西被建/被完成」。
#[test]
fn snapshot_builders_refuse_missing_evidence() {
    // create：缺 contentHash。
    let err = snapshot_create_request(CloudSnapshotCreateInput {
        content_hash: String::new(),
        ..snapshot_create_input()
    })
    .expect_err("空 contentHash 应当拒发");
    assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
    assert!(err.to_string().contains("contentHash"), "{err}");

    // create：count < 1（那是"声明一个空快照"，这一格没有读数）。
    for count in [0_i64, -1] {
        let err = snapshot_create_request(CloudSnapshotCreateInput {
            count,
            ..snapshot_create_input()
        })
        .expect_err("count < 1 应当拒发");
        assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
        assert!(err.to_string().contains("count"), "{err}");
    }

    // complete：`fileId` / `contentHash` 各自缺一发就拒。
    let hash = "c".repeat(64);
    for (file_id, hash_value) in [("", hash.as_str()), (SYNTH_SNAPSHOT_FILE_ID, "")] {
        let err = snapshot_complete_request(CloudSnapshotCompleteInput {
            file_id: file_id.into(),
            content_hash: hash_value.into(),
            ..snapshot_complete_input()
        })
        .expect_err("缺依据应当拒发");
        assert!(matches!(err, Error::Config(_)), "实际：{err:?}");
    }
    // ⚠ 但 `uploadId` **空串不算缺** —— 2026-10-07 **真机读数**：任务在 `移动` 号上真跑，
    // `createSnapshot` 回的 `data.uploadId` 是 `null`（秒传那一支，抓包篇 §9 记的"仍未读"）。
    // 与 `file/complete` 同口径：`stringOf(null)` 就是空串 ⇒ 照发 ✓。
    assert!(
        snapshot_complete_request(CloudSnapshotCompleteInput {
            upload_id: String::new(),
            ..snapshot_complete_input()
        })
        .is_ok(),
        "空 uploadId 要能构造（真机秒传那一支回的就是 null）"
    );
}

/// ⭐⭐ `parse_snapshot_create`：`data` 4 键解出三枚 + `rapidUpload`。
///
/// 合成值的**形状**逐格照抓包 §6：`fileId` str64（⚠ 不是照片那支的 33）、
/// `uploadId` str33、`uploadUrl` 取 `partInfos[0]`、`rapidUpload` 布尔。
#[test]
fn parse_snapshot_create_reads_the_four_key_data() {
    let body = format!(
        r#"{{"code":"0000","message":"请求成功","success":true,"data":{{"fileId":"{fid}","rapidUpload":false,"uploadId":"{uid}","partInfos":[{{"partNumber":1,"partSize":null,"uploadUrl":"https://synth-oss.example.invalid/put","cdnUploadUrl":null,"etag":null,"parallelHashCtx":{{"h":null,"partOffset":null}}}}]}}}}"#,
        fid = SYNTH_SNAPSHOT_FILE_ID,
        uid = SYNTH_SNAPSHOT_UPLOAD_ID,
    );
    let created = parse_snapshot_create(as_bytes(&body)).expect("应能解析");

    assert_eq!(created.file_id.as_deref(), Some(SYNTH_SNAPSHOT_FILE_ID));
    assert_eq!(
        created.file_id.as_deref().map(str::len),
        Some(64),
        "抓包 §6：快照的 fileId 是 **str64**（照片那支是 33）"
    );
    assert_eq!(created.upload_id.as_deref(), Some(SYNTH_SNAPSHOT_UPLOAD_ID));
    assert_eq!(
        created.upload_url.as_deref(),
        Some("https://synth-oss.example.invalid/put"),
        "取的是 partInfos[0].uploadUrl"
    );
    assert!(!created.rapid_upload);

    // 秒传那一格读得出来（但**没有**任何"允许缺 uploadUrl"的例外，见解析器文档）。
    let rapid = parse_snapshot_create(as_bytes(
        r#"{"code":"0000","success":true,"data":{"fileId":"F","rapidUpload":true,"uploadId":"U","partInfos":[{"uploadUrl":"https://x.invalid/put"}]}}"#,
    ))
    .expect("应能解析");
    assert!(rapid.rapid_upload);
}

/// ⭐⭐ 缺格一律 Err：`data` 缺席、`data` 不是对象、三枚必需格各缺一次。
///
/// 这一条钉的是「**缺了就走不下去**」那三格 —— 与 `parse_file_create` 的
/// "交回 `None` 让上层判"刻意不同（见 [`parse_snapshot_create`] 的文档）。
#[test]
fn parse_snapshot_create_errors_when_a_required_key_is_missing() {
    // `data` 缺席。
    let err = parse_snapshot_create(as_bytes(r#"{"code":"0000","success":true}"#))
        .expect_err("没有 data 应当报错");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");
    assert!(err.to_string().contains("data"), "{err}");

    // `data` 不是对象。
    let err = parse_snapshot_create(as_bytes(r#"{"code":"0000","success":true,"data":[]}"#))
        .expect_err("data 是数组应当报错");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");

    // 三枚必需格：各缺一次，且 `null` / 空串 / 数字都算缺。
    for (why, data) in [
        (
            "缺 fileId ⇒ completeSnapshot 发不出去",
            r#"{"rapidUpload":false,"uploadId":"U","partInfos":[{"uploadUrl":"https://x.invalid/put"}]}"#,
        ),
        (
            "缺 uploadId ⇒ completeSnapshot 发不出去",
            r#"{"fileId":"F","rapidUpload":false,"partInfos":[{"uploadUrl":"https://x.invalid/put"}]}"#,
        ),
        (
            "缺 uploadUrl ⇒ 没有地方 PUT 那 619 字节",
            r#"{"fileId":"F","rapidUpload":false,"uploadId":"U","partInfos":[{"partSize":null}]}"#,
        ),
        (
            "partInfos 整个是 null ⇒ 同样没有 uploadUrl",
            r#"{"fileId":"F","rapidUpload":false,"uploadId":"U","partInfos":null}"#,
        ),
        (
            "fileId 是 null（不是「没给」，是给了个空）⇒ 也算缺",
            r#"{"fileId":null,"rapidUpload":false,"uploadId":"U","partInfos":[{"uploadUrl":"https://x.invalid/put"}]}"#,
        ),
        (
            "uploadUrl 是空串 ⇒ 也算缺",
            r#"{"fileId":"F","rapidUpload":false,"uploadId":"U","partInfos":[{"uploadUrl":""}]}"#,
        ),
    ] {
        let body = format!(r#"{{"code":"0000","success":true,"data":{data}}}"#);
        let err = parse_snapshot_create(as_bytes(&body)).expect_err(why);
        assert!(matches!(err, Error::Parse(_)), "{why}；实际：{err:?}");
    }
}

/// ⭐⭐ **秒传那一支**（`rapidUpload:true`）：`uploadId` / `uploadUrl` 可以缺席。
///
/// ⚠ 这一格是 **2026-10-07 的真机读数**：任务在 `移动` 号上真跑，`createSnapshot`
/// 回的 `data.uploadId` 是 `null` —— 正是抓包篇 §9 明写"仍未读"的那一支 ❓。
/// 旧解析器按"三格必须齐"写死 ⇒ 那一支直接判失败 ✗（真跑一次的报错原文就是
/// `data.uploadId 缺席 / null / 空串 / 不是字符串（实得 Some(Null)）`）。
#[test]
fn parse_snapshot_create_allows_missing_upload_evidence_on_rapid_upload() {
    let body = r#"{"code":"0000","success":true,"data":{"fileId":"F","rapidUpload":true,"uploadId":null,"partInfos":[{"uploadUrl":null}]}}"#;
    let created = parse_snapshot_create(as_bytes(body)).expect("秒传那一支要能解出来");
    assert!(created.rapid_upload, "这一支就是 rapidUpload:true");
    assert_eq!(created.file_id.as_deref(), Some("F"));
    assert_eq!(created.upload_id, None, "null 就是没有，不是空串");
    assert_eq!(created.upload_url, None, "秒传没有 PUT 要发");
    // ⚠ 非秒传那一支**仍然严**：uploadId 缺席 ⇒ Err（那一支没有 PUT 就没有下文）。
    let strict = r#"{"code":"0000","success":true,"data":{"fileId":"F","rapidUpload":false,"uploadId":null,"partInfos":[{"uploadUrl":"https://x.invalid/put"}]}}"#;
    assert!(
        parse_snapshot_create(as_bytes(strict)).is_err(),
        "非秒传缺 uploadId 仍要报错"
    );
}

/// ⭐⭐ `parse_snapshot_complete`：**`data: null` 算成功** —— 与 `parse_file_create` 的关键差别。
///
/// 抓包 §6：这一口只回 59 字节、`data` 是 `null`（交完就完，服务端不回对象快照），
/// 而 `file/complete` 回的是 29 键对象。
/// ⚠ 拿 `parse_file_create` 来解这一口会把**每一发成功**读成失败 ——
/// 所以这一条同时把两个解析器在**同一个体**上的结果摆在一起（一个 Ok、一个 Err）。
#[test]
fn parse_snapshot_complete_treats_null_data_as_success_unlike_file_create() {
    // 抓包那一发的形状（59 字节那一条）。
    let body = r#"{"code":"0000","message":"请求成功","success":true,"data":null}"#;

    assert!(
        parse_snapshot_complete(as_bytes(body)).is_ok(),
        "data 是 null 也算成功（这是与 parse_file_create 的关键差别）"
    );
    let err = parse_file_create(body)
        .expect_err("同一个体交给 file/create 的解析器必须失败 —— 它要的是 data 是对象");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");

    // `data` 整个缺席也放过（这一口不读载荷）。
    assert!(parse_snapshot_complete(as_bytes(r#"{"code":"0000","success":true}"#)).is_ok());
    assert!(parse_snapshot_complete(as_bytes(r#"{"code":"0000"}"#)).is_ok());
    // `data` 是别的形状也一样不读。
    assert!(parse_snapshot_complete(as_bytes(
        r#"{"code":"0000","success":true,"data":{"anything":1}}"#
    ))
    .is_ok());
}

/// 失败码 / `success:false` / 非 JSON / 非 UTF-8 都必须报错（与 `parse_file_complete` 同口径）。
#[test]
fn parse_snapshot_complete_only_judges_the_envelope() {
    let err = parse_snapshot_complete(as_bytes(
        r#"{"code":"0000","success":false,"message":"校验失败"}"#,
    ))
    .expect_err("success:false 应是错误");
    assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");
    assert!(err.to_string().contains("校验失败"), "{err}");

    let err = parse_snapshot_complete(as_bytes(r#"{"code":"90001","msg":"未登录"}"#))
        .expect_err("非 0 码应失败");
    assert!(err.to_string().contains("90001"), "{err}");

    let err = parse_snapshot_complete(as_bytes("<html>502 Bad Gateway</html>"))
        .expect_err("非 JSON 必须算失败");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");

    // ⚠ 非 UTF-8 要报错，**不能**被 `from_utf8_lossy` 洗成"信封形状变了"：
    // 那两种病的修法完全不同（一个是上游换了回执，一个是这一发拿到的根本不是 JSON）。
    let err = parse_snapshot_complete(&[0x00, 0xff, 0xfe, 0x80]).expect_err("非 UTF-8 必须算失败");
    assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");
    assert!(err.to_string().contains("UTF-8"), "{err}");
}

/// ⭐ 两个快照解析器都**不自己设凭据**，且错误文本里**不带**预签名 URL 原文。
///
/// 预签名 URL 是"带签名的写入入口"，与凭据同类（见模块头那一段）⇒
/// 缺 `uploadUrl` 的那条错误只说明**缺了哪一格**，不回显任何值。
#[test]
fn snapshot_parsers_never_echo_the_presigned_url() {
    let body = r#"{"code":"0000","success":true,"data":{"fileId":"F","uploadId":"U","partInfos":[{"uploadUrl":"https://synth-oss.example.invalid/put/SYNTH-TOKEN"}]}}"#;
    let created = parse_snapshot_create(as_bytes(body)).expect("应能解析");
    // 解出来的那一格**可以**带值（调用方要用它发 PUT），但错误文本里不许有。
    assert_eq!(
        created.upload_url.as_deref(),
        Some("https://synth-oss.example.invalid/put/SYNTH-TOKEN")
    );

    let err = parse_snapshot_create(as_bytes(
        r#"{"code":"0000","success":true,"data":{"fileId":"F","uploadId":"U","partInfos":null}}"#,
    ))
    .expect_err("缺 uploadUrl 应当报错");
    assert!(
        !err.to_string().contains("SYNTH-TOKEN"),
        "错误文本不许回显预签名入口：{err}"
    );
    assert!(
        !err.to_string().contains("https://"),
        "错误文本不许带 URL 原文：{err}"
    );
}
