//! 上传族**前三枪**协议层的用例（第四枪在 [`crate::cloud_trash`]）。
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
//! 所以全部用例钉的是三类**可控**的事：
//!
//!  1. **字面量别在翻译中丢失** —— `application/oct-stream`（少一个 `e`，在**请求体**里）
//!     与 `application/octet-stream`（正常拼写，在 **PUT 的头**里）是两处不同的东西；
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
//! `PUT` 的「不可重试」（本仓 [`HttpRequest`] 没有那一格，见 `upload_put_request`）。
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

fn response(status: u16, body: &str) -> mcloud_core::HttpResponse {
    mcloud_core::HttpResponse {
        status,
        body: body.to_string(),
        headers: Vec::new(),
    }
}

const ALL_CHANNELS: [CloudChannel; 3] =
    [CloudChannel::Android, CloudChannel::Web, CloudChannel::Pc];
const ALL_OPS: [CloudOpType; 2] = [CloudOpType::Upload, CloudOpType::Backup];

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

/// ⭐ 两处 `content-type` 是**两个不同的东西**，不许「顺手统一」。
#[test]
fn the_two_content_types_stay_different() {
    // 体里那一枚少一个 `e`（旧 TS 的字面量）；PUT 头里那一枚拼写正常。
    assert_eq!(CLOUD_CREATE_CONTENT_TYPE, "application/oct-stream");
    assert_eq!(CLOUD_PUT_CONTENT_TYPE, "application/octet-stream");
    assert_ne!(CLOUD_CREATE_CONTENT_TYPE, CLOUD_PUT_CONTENT_TYPE);

    let body = body_json(&file_create_request(create_input()).expect("应能构造"));
    assert_eq!(
        body["contentType"],
        serde_json::json!(CLOUD_CREATE_CONTENT_TYPE)
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
    }
}

// ══════════════════════════ 第一枪：create 的体 ══════════════════════════

/// ⭐ 十键齐全，且两处「看着像拼错」的字面量在位。
///
/// 反向改坏它的样子：① 把体的 `contentType` 改成 `application/octet-stream`
/// （「顺手修个拼写」）⇒ 红；② 把 `fileRenameMode` 改成 `refuse`（那是**建目录**那一型的值）
/// ⇒ 红；③ 少发 `parallelUpload` ⇒ 键集合断言红。
#[test]
fn file_create_body_is_the_legacy_ten_keys() {
    let req = file_create_request(create_input()).expect("应能构造");
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
        serde_json::json!(CLOUD_RENAME_MODE_AUTO)
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

/// ⭐ 两型的重名策略**不许互相搬**：文件型 `auto_rename`，目录型 `refuse`。
///
/// 参考实现明确警告过：`refuse` 只在**目录型**上出现过（实拍与真跑都是），
/// 「同名文件用 refuse 会怎样」**零样本**。
#[test]
fn rename_modes_are_not_interchangeable() {
    let file_body = body_json(&file_create_request(create_input()).expect("应能构造")).to_string();
    let folder_body = body_json(
        &folder_create_request(
            "",
            "asign-synth-dir",
            CloudChannel::Android,
            CloudOpType::Upload,
        )
        .expect("应能构造"),
    )
    .to_string();

    assert!(file_body.contains(CLOUD_RENAME_MODE_AUTO));
    assert!(
        !file_body.contains(CLOUD_RENAME_MODE_REFUSE),
        "refuse 只在目录型上出现过，别搬去传文件"
    );
    assert!(folder_body.contains(CLOUD_RENAME_MODE_REFUSE));
    assert!(
        !folder_body.contains(CLOUD_RENAME_MODE_AUTO),
        "auto_rename 是文件那一型的值，别搬来建目录"
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

/// ⭐ `PUT` 那一枪：方法、三枚头、**裸字节**体。
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
    assert_eq!(header(&req, "origin"), Some("https://yun.139.com"));
    assert_eq!(
        header(&req, "referer"),
        Some("https://yun.139.com/"),
        "旧 TS 带结尾斜杠"
    );
}

/// 这一枪**不套** `getFileOpHeader`，也不自己设凭据 / UA。
///
/// 旧 TS 的 `putFile` 只发那三枚头加实例静态头。
#[test]
fn upload_put_carries_no_file_op_headers_or_credentials() {
    let req = upload_put_request(SYNTH_PUT_URL, b"SYNTH");
    for name in [
        "x-yun-api-version",
        "x-yun-app-channel",
        "x-yun-op-type",
        "x-yun-client-info",
        "x-huawei-channelSrc",
        "authorization",
        "deviceid",
        "user-agent",
    ] {
        assert_eq!(header_count(&req, name), 0, "{name} 不该出现在 PUT 那一枪");
    }
    assert_eq!(
        req.headers.len(),
        3,
        "这一枪只有三枚头：content-type / origin / referer，实得 {:?}",
        req.headers
    );
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
