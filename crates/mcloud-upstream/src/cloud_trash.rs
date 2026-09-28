//! 网盘写操作的**收尾那一枪**：`hcy/recyclebin/batchTrash`。
//!
//! 对应参考实现 `internal/upstream/app_cloud_upload.go` 的
//! `CloudBatchTrashRequest` / `ParseCloudFileBatchTrash`；证据是
//! `mcloud/docs/capture/app-cloud-recyclebin-samples.txt`（活体探针真打，非推断）
//! 与 `app-cloud-folder-trash-samples.txt`。
//!
//! # 为什么这一枪值得单独一层
//!
//! 写网盘的任务（P3 那一批）会在用户网盘里**造东西**：传文件、建目录。
//! 「造出来的东西，失败也要收走」这件事本仓已经有**账本**
//! （`mcloud_engine::artifacts`：记账 / 销账 / 交回未清项），
//! 但账本只回答「还剩哪几枚」，**不回答「怎么把它们弄走」**。
//! 那一半就是本模块 —— 少了它，产物账本只是一个越攒越长的清单。
//!
//! # 请求形状
//!
//! `POST {PERSONAL_KD}/hcy/recyclebin/batchTrash`，体只有一枚键：
//!
//! ```json
//! {"fileIds": ["<id>", "<id>"]}
//! ```
//!
//! ⚠ **空列表不发**。旧实现把这个判断放在调用侧
//! （`after.ts:133` 的 `if (files && files.length > 0)`），
//! 参考实现把它挪进了构造函数。这里沿用后者：
//! 「请求层能挡住的少发一枪」不该指望每个调用方都记得挡 ——
//! 而忘了挡的表现是发一枪 `{"fileIds":[]}` 出去，上游回什么**没有样本**。
//!
//! # 回执形状
//!
//! 成功只有三格：`code` 是**字符串** `"0000"`（不是数字 `0`）、
//! `success` 是**布尔** `true`、`msg` **键可以缺席**（不是空串）。
//! 「`msg` 缺席」这一档对本模块有直接意义：读它的代码必须能在没有
//! 这一格时活下来。
//!
//! # ⚠ `success` 缺席 = 失败，不是「没失败就是成功」
//!
//! 旧实现读的是 `result?.success` —— 那个 `result` 是 `tryRequest` 元组里的
//! **整个响应对象**，所以它读的其实是**顶层** `success`，而且**读不到就当失败**
//! （`undefined` 是假值）。这里保持同一口径：那一格缺席 ⇒ 回错误。
//!
//! 反过来写（「没有 `success:false` 就算成功」）会把「上游换了回执形状」
//! 这一类故障**洗白成删除成功**，而后果是产物一直留在网盘里、账本却已销账 ——
//! 这正是本仓最忌讳的「不发错、不报错，只是行为不对」。
//!
//! # 重复点名：今天没有「多记一条」的读数
//!
//! 清理链**会**重复点名同一枚 `fileId`（任务重复触发、两条链上传了同一枚产物、
//! 用户连点两次清理），所以「重试安不安全」是这一口必须有的口径。
//! 2026-09-26 第卅 / 卅一趟真跑：对同一枚点名两次 ⇒ 上游回**同一句**成功，
//! 回收站里**条数不变**、那一枚的 `trashedAt` **逐毫秒仍是第一发那一条**。
//!
//! ⚠ **但它不是「幂等」**。那两趟的边界是：只测了「入站后一分钟内再点名」、
//! 只测了 `batchTrash` 这一口、只测了**当天**；对象只覆盖
//! **空目录 / 单个文件 / 两层 1 枚的非空目录**三型（三层子树未测）。
//! 所以能说的只有「重复点名不产生第二条、也不回失败」，
//! **不能**扩成一般性保证，更不能拿它去**省掉**清理链的记账 ——
//! 那是「少发一枪」的优化，另一回事。
//!
//! # 头画像：web 档，且与上传那几枪**无关**
//!
//! `apis/file.ts:114` 写死 `channelSrc: "10000034"`（web 档），
//! 而同一族的上传用的是 android 档。也就是说旧实现里
//! **「用 PC 档上传、用 web 档删除」是常态** —— 照抄，不「顺手统一」。
//!
//! ⚠ `x-yun-op-type` 在 web 档是 `1`：那个 `op*100` 的 `x-yun-sub-op-type`
//! **只对 android 档生效**（`apis/file.ts:17-40` 两个分支里 `opType` 只被
//! android 分支读）。所以「backup 在 web 档会怎样」的答案是「和 upload 一样」
//! —— 那是从代码形状读出来的，**不是实测**。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};

use crate::endpoints::RECYCLEBIN_BATCH_TRASH;

// ══════════════════════════════ 常量 ══════════════════════════════

/// 这一族成功回执的业务码。**字符串**，不是数字。
pub const CODE_OK: &str = "0000";

/// 删除那一口固定用的渠道档（web）。
///
/// ⚠ 它与同一族上传用的 **android 档 `10000023`** 不是同一个值 ——
/// 旧实现里两档并存是**刻意**的（见模块头那一段）。
pub const CLOUD_CHANNEL_WEB: &str = "10000034";

/// web 档的 `x-yun-client-info` —— **素材字面量，整串照抄**。
///
/// 别按「我们这一台设备」重拼：三档的版本号各不相同
/// （App `12.6.0` / Web `7.15.3` / PC `8.3.3.20250521`），而它跟仓库里那几个
/// `12.6.0` 常量只是**恰好同号**，不同族。
pub const CLOUD_CLIENT_INFO_WEB: &str = "||9|7.15.3|firefox|139.0|||linux unknow||zh-CN|||";

/// 这一族带体请求的 content-type。
pub const CLOUD_CONTENT_TYPE: &str = "application/json;charset=UTF-8";

// ══════════════════════════════ 请求 ══════════════════════════════

/// 给网盘那一族的写请求补上头画像（web 档）。
///
/// # 哪些头**不**在这里设，以及为什么
///
/// - `authorization` / `deviceid`：由发送层（`crate::client::ReqwestClient`）
///   按域名与 profile 注入。在这儿再设一遍**不会覆盖**，而是发出去
///   **两根同名头**（reqwest 的 `.header()` 是追加），而上游读哪一根未测试
///   —— 这就是本仓 `content-type` 曾经踩过的那个坑。
/// - `User-Agent`：本仓的 UA 归 profile（`effective_user_agent()`），
///   在这儿加会是**第二根**。⚠ 这里与参考实现有一处**已知分歧**：
///   参考实现给这一族写死 `CaiyunAndroidWebUA`，本仓用的是 profile 的 UA。
///   要复现参考行为，应在**组装层**把 profile 的 UA 设成那一串。
///   上游今天不校验这一格；哪天开始校验，错的一方是**静默失效**，不会报错。
///
/// # 本函数只是**转发**（这一族的头画像只有一处实现）
///
/// 那一族是「渠道 × 操作类型」两维的：`create` / `complete` 可能走 android 或 PC 档，
/// 而删除固定 web 档 —— 三档两份实现的写法迟早分叉，所以参数化的那一份落在
/// [`crate::app_cloud_upload::cloud_file_op_headers`]，本函数是它在
/// `(web, upload)` 这一档上的转发。
///
/// ⚠ 改这一族的头**只改那一个函数**：在这里补一枚头会变成「只有删除那一枪有」，
/// 而旧实现里这几口共用同一个 `getFileOpHeader`。
pub fn cloud_file_op_headers_web(req: HttpRequest) -> HttpRequest {
    crate::app_cloud_upload::cloud_file_op_headers(
        req,
        crate::app_cloud_upload::CloudChannel::Web,
        crate::app_cloud_upload::CloudOpType::Upload,
    )
}

/// 构造「批量移进回收站」这一枪（**写**）。
///
/// # 空列表 ⇒ 回错误，**不发请求**
///
/// 见模块头那一段：这一格由请求层挡住，调用方不必记得挡。
///
/// ⚠ 回 `Error::Config` 而**不是**静默返回一个空请求：后者会让调用方
/// 以为已经清理过了，而账本那边会照着销账。
/// 「没有东西要清」是**正常状态**，但它该由调用方自己判
/// （账本的待清清单为空），而不是靠这里发一枪空请求来"成功"。
pub fn batch_trash_request(file_ids: &[String]) -> Result<HttpRequest> {
    if file_ids.is_empty() {
        return Err(Error::Config(
            "batchTrash 收到空文件列表：这一枪不该发（旧实现在这里根本不发请求）".into(),
        ));
    }
    let body = serde_json::json!({ "fileIds": file_ids });
    Ok(cloud_file_op_headers_web(
        HttpRequest::post(RECYCLEBIN_BATCH_TRASH).json_body(&body),
    ))
}

// ══════════════════════════════ 回执 ══════════════════════════════

/// 解析 `batchTrash` 的回执。
///
/// 判据两条，**缺一不可**：
///
/// 1. `code == "0000"`（字符串）—— 非 0 码照常回错误；
/// 2. 顶层有一个 `success: true`。
///
/// ⚠ 第 2 条是「**要求它存在**」，不是「它不为 false 就算过」。
/// 见模块头：那一格缺席必须算失败。
///
/// `msg` 缺席是**正常**的（真实成功回执里就没有这一格）—— 所以这里不读它。
pub fn parse_batch_trash(body: &str) -> Result<()> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("batchTrash 回执不是合法 JSON：{e}")))?;

    let code = v.get("code").and_then(Value::as_str).unwrap_or_default();
    if code != CODE_OK {
        return Err(Error::Upstream {
            code: code.to_string(),
            message: format!("batchTrash 失败：{}", upstream_msg(&v)),
        });
    }

    match v.get("success") {
        Some(Value::Bool(true)) => Ok(()),
        Some(other) => Err(Error::Upstream {
            code: code.to_string(),
            message: format!(
                "batchTrash 回执里 success 不是 true（{other}）：{}",
                upstream_msg(&v)
            ),
        }),
        None => Err(Error::Parse(
            "batchTrash 回执里没有 success 这一格 —— 按失败处置（旧实现读不到就当失败）".into(),
        )),
    }
}

/// 取上游那句 `msg`，缺席时给一句**可判别**的占位。
///
/// ⚠ 占位不写成空串：空串在日志里与「上游返回了空 msg」长得一样，
/// 而这两件事的排查方向完全不同（一个是我们的解析，一个是对面的回执）。
fn upstream_msg(v: &Value) -> &str {
    v.get("msg")
        .and_then(Value::as_str)
        .unwrap_or("（上游没给 msg）")
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 取一个头（大小写不敏感 —— 上游与 reqwest 都不保证大小写一致）。
    fn header(req: &HttpRequest, name: &str) -> Option<String> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("id-{i}")).collect()
    }

    // ── 请求 ────────────────────────────────────────────────────────────

    /// ⭐ 空列表**根本不发**：不是"发一枪空请求然后算成功"。
    ///
    /// 这条锁的是"清理链在无产物时不会打出无样本的请求"，
    /// 而不是一个格式细节：忘了挡就会发 `{"fileIds":[]}` 出去，
    /// 而上游回什么**没有样本**。
    #[test]
    fn empty_list_is_rejected_before_sending() {
        let err = batch_trash_request(&[]).expect_err("空列表必须被挡住");
        assert!(
            matches!(err, Error::Config(_)),
            "应是 Config，实际：{err:?}"
        );
    }

    #[test]
    fn body_carries_only_file_ids() {
        let req = batch_trash_request(&ids(2)).expect("应能构造");
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, RECYCLEBIN_BATCH_TRASH);
        let body: Value =
            serde_json::from_str(req.body_str().expect("应有体")).expect("体应是 JSON");
        assert_eq!(body, serde_json::json!({ "fileIds": ["id-0", "id-1"] }));
        // ⚠ 体里只有这一枚键。多塞一枚"看着有用"的字段等于凭空发明协议。
        assert_eq!(body.as_object().unwrap().len(), 1, "体里只该有 fileIds");
    }

    /// ⭐ 头画像逐枚对拍，**并且**断言那几枚凭证头不在里面。
    ///
    /// 后者不是洁癖：发送层会按域名再注入一次，而 reqwest 的 `.header()`
    /// 是**追加** ⇒ 这里设了就会发出去两根同名头，上游读哪一根未测试。
    /// 表现是"不报错、只是行为不对"，正是本仓最忌讳的那类。
    #[test]
    fn web_header_recipe_is_exact_and_omits_credentials() {
        let req = batch_trash_request(&ids(1)).expect("应能构造");

        assert_eq!(header(&req, "x-yun-api-version").as_deref(), Some("v1"));
        assert_eq!(
            header(&req, "x-yun-app-channel").as_deref(),
            Some("10000034")
        );
        assert_eq!(header(&req, "x-yun-op-type").as_deref(), Some("1"));
        assert_eq!(
            header(&req, "x-huawei-channelSrc").as_deref(),
            Some("10000034")
        );
        assert_eq!(
            header(&req, "x-yun-client-info").as_deref(),
            Some(CLOUD_CLIENT_INFO_WEB)
        );
        // 这一枚的值**就是**设备信息串（旧 TS 的笔误，但上游收到的就是它）
        assert_eq!(
            header(&req, "x-yun-device-id").as_deref(),
            Some(CLOUD_CLIENT_INFO_WEB)
        );
        assert_eq!(
            header(&req, "Content-Type").as_deref(),
            Some("application/json;charset=UTF-8")
        );

        // ⚠ web 档**没有** x-yun-sub-op-type —— 那一枚只对 android 档生效。
        // 顺手"补上"它等于把 android 档的画像混进来。
        assert_eq!(
            header(&req, "x-yun-sub-op-type"),
            None,
            "web 档不该有 sub-op-type"
        );

        for owned in ["authorization", "deviceid", "user-agent"] {
            assert_eq!(
                header(&req, owned),
                None,
                "{owned} 由发送层注入，这里设了会变成两根同名头"
            );
        }
    }

    // ── 回执 ────────────────────────────────────────────────────────────

    /// ⭐ 真实成功回执：三格里**没有 `msg`**。
    ///
    /// 逐字形状来自 2026-09-26 第卅趟探针第一次念出的那三格
    /// （见 `app-cloud-recyclebin-samples.txt` §13）。
    #[test]
    fn parse_accepts_the_real_success_receipt() {
        let real = r#"{"code":"0000","success":true}"#;
        assert!(parse_batch_trash(real).is_ok(), "真实成功回执必须过");
    }

    /// ⭐⭐ 本模块最要紧的一条：`success` **缺席**必须算失败。
    ///
    /// 反例是"没有 `success:false` 就当成功"——那样写会把"上游换了回执形状"
    /// 洗白成删除成功：产物还在网盘里，账本却已销账。
    /// 旧实现读 `result?.success`、读不到就当失败（`undefined` 是假值），
    /// 这里保持同一口径。
    #[test]
    fn parse_requires_the_success_key_to_be_present() {
        let err = parse_batch_trash(r#"{"code":"0000"}"#)
            .expect_err("success 缺席必须算失败，不能当成功");
        assert!(matches!(err, Error::Parse(_)), "应是 Parse，实际：{err:?}");
        assert!(
            err.to_string().contains("success"),
            "错误里要指出缺的是哪一格：{err}"
        );
    }

    #[test]
    fn parse_rejects_success_false() {
        let err = parse_batch_trash(r#"{"code":"0000","success":false,"msg":"拒绝"}"#)
            .expect_err("success:false 必须算失败");
        assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");
        assert!(
            err.to_string().contains("拒绝"),
            "上游那句 msg 要带出来：{err}"
        );
    }

    #[test]
    fn parse_rejects_non_zero_code_and_surfaces_msg() {
        let err = parse_batch_trash(r#"{"code":"1001","msg":"参数错误"}"#)
            .expect_err("非 0 码必须算失败");
        match err {
            Error::Upstream { ref code, .. } => assert_eq!(code, "1001"),
            other => panic!("应是 Upstream，实际：{other:?}"),
        }
        assert!(err.to_string().contains("参数错误"), "{err}");
    }

    /// 非 0 码那一支也要能在**没有 `msg`** 时活下来。
    ///
    /// 真机回执里 `msg` 缺席是常态（成功那一发就没有），
    /// 而失败那一发「也缺席」这一档今天零样本 —— 但读它的代码
    /// 必须两种都活。
    #[test]
    fn parse_survives_a_missing_msg() {
        let err = parse_batch_trash(r#"{"code":"1001"}"#).expect_err("非 0 码算失败");
        assert!(err.to_string().contains("1001"), "码要带出来：{err}");
    }

    #[test]
    fn parse_rejects_non_json() {
        let err =
            parse_batch_trash("<html>502 Bad Gateway</html>").expect_err("非 JSON 必须算失败");
        assert!(matches!(err, Error::Parse(_)), "实际：{err:?}");
    }

    /// 数字形态的 `0` **不算** `"0000"`。
    ///
    /// 这一条对应仓库里那条既有口径：本仓两种信封都存在
    /// （登录族/网盘族是字符串 `"0000"`，签到族是数字 `0`），
    /// 拿错信封会**永远判失败**。这里把"我们只认字符串那一型"钉住，
    /// 表现成"上游真换了形态"时测试先红，而不是线上静默全败。
    #[test]
    fn numeric_zero_is_not_accepted() {
        let err = parse_batch_trash(r#"{"code":0,"success":true}"#)
            .expect_err("数字 0 不是这一族的成功码");
        assert!(matches!(err, Error::Upstream { .. }), "实际：{err:?}");
    }
}
