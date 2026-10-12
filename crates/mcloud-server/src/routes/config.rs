//! `GET /api/config/export` —— 把配置导出成一个下载文件（WebUI 那一侧）。
//!
//! # 它和 CLI 那条的关系
//!
//! 同一个"导出"，两个入口 ✓：CLI 是 `mcloud config export`（无头 / 无公网的机器 ✓），
//! WebUI 是这条路由（有界面的机器上点一下就拿到 ✓）。**实现只有一份** ——
//! 文档形状、导出哪几类、稳定排序、敏感标记全在 [`crate::config_doc`] ✓，
//! 这里只负责"取当前登录用户 → 调它 → 套上下载响应头" ✓。
//!
//! ⚠ **不许**在这个文件里另写一份 collect / 形状：两份必然漂移，而漂移的表现是
//! "界面导出的文件导不进去"，那要等用户真的换了机器才现形 ✗。
//!
//! # ⚠⚠ 视角：WebUI 导的是**当前登录用户自己的**，CLI 导的是**本机管理员**那份
//!
//! 这是 `AGENTS.md`「CLI 与 WebUI 的视角分工」那条的直接落地 ✓：
//!
//! | | 谁在用 | 导谁的那份 |
//! |---|---|---|
//! | WebUI | 不同设备上的不同登录用户（多用户共享一份后端 ✓） | **当前会话的 owner**（[`CurrentUser`] ✓） |
//! | CLI | 本机那一个人（无头 / 只想敲命令 ✓） | `legacy_owner`（= 管理员 ✓） |
//!
//! 所以这里**一律**用 `cur.0`，**没有**任何"选谁"的参数 ✓ ——
//! 请求里根本不存在"别人的 owner"这个输入面，于是"别人的东西"连**查询入口**
//! 都没有（归属错误一律 404 那条口径在这里表现为：根本问不到 ✓）。
//! 数据面那一层也一样：`config_doc::collect` 取账号走 `accounts_for(owner)`
//! （归属过滤的唯一入口 ✓），两张按账号 id 存的表再按**这些**账号去读 ✓
//! ⇒ 别人的账号 / 状态 / 设备指纹结构上进不了这份文件 ✓。
//!
//! # 为什么是只读的 GET
//!
//! 它不改任何东西（连机器级那枚设备指纹也是**只读**读出来的 ✓，见
//! `LoginService::pc_device_id_opt`）⇒ 一次普通的 GET ✓，可以被浏览器直接下载 ✓。
//!
//! ⚠ **导入没有界面**（用户口径 2026-10-12：「导入不要做界面」）—— 它是**覆盖式**
//! 操作，一键覆盖风险高 ⇒ 界面只放导出，导入走 CLI `mcloud config import <文件>` ✓。
//!
//! # 响应头
//!
//! * `Content-Type: application/json; charset=utf-8` —— 就是一份 JSON 文档 ✓；
//! * `Content-Disposition: attachment; filename="…"` —— 文件名带**用户名与时间戳**
//!   （不这样的话，同一个目录下导两次就得靠人改名才能区分 ✗）。
//!   ⚠ 两段都给：ASCII 那份（`filename=`，用户名里的引号 / 换行已被换掉，见
//!   [`crate::config_doc::ascii_filename_part`]）与 RFC 5987 那份
//!   （`filename*=UTF-8''…`，中文名靠它 ✓），浏览器优先用后者 ✓。

use axum::extract::{Extension, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::auth_gate::CurrentUser;
use crate::config_doc::{self, ExportDoc};
use crate::error::ApiError;
use crate::ServerState;

pub fn routes() -> Router<ServerState> {
    Router::new().route("/api/config/export", get(export))
}

/// 把"取文档"与"拼下载响应"分开，便于单测直接对着 `ExportDoc` 断言
/// （**不必**穿过 HTTP 那一层就能核"导出了哪几类"）。
async fn build(state: &ServerState, owner: &mcloud_core::UserId) -> Result<ExportDoc, ApiError> {
    config_doc::collect(&state.handle, owner)
        .await
        .map_err(|e| ApiError(mcloud_core::Error::Storage(format!("导出配置失败：{e}"))))
}

pub async fn export(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> Result<Response, ApiError> {
    let doc = build(&state, &cur.0).await?;
    let json = config_doc::to_json(&doc)
        .map_err(|e| ApiError(mcloud_core::Error::Storage(format!("导出配置失败：{e}"))))?;

    // 用户名取**登录用户自己那条记录**；读不到（会话还在、用户被删了）就回落成 owner id ✓
    // —— 回落而不是报错：导出这件事本身与他叫什么名字无关 ✓。
    let name = state
        .handle
        .store
        .user(&cur.0)
        .await?
        .map(|u| u.name)
        .unwrap_or_else(|| cur.0.to_string());

    // 时间戳只来自注入的 `Clock`（仓库约定：时间只有这一个来源 ✓）。
    let stamp = mcloud_core::beijing_format(&*state.handle.clock, "%Y%m%d-%H%M%S");
    let ascii = format!(
        "mcloud-config-{}-{stamp}.json",
        config_doc::ascii_filename_part(&name)
    );
    let utf8 = format!(
        "mcloud-config-{}-{stamp}.json",
        config_doc::percent_encode_filename(&name)
    );

    let mut resp = Response::new(axum::body::Body::from(json));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{utf8}");
    // 两段都过了字符白名单 ⇒ 这里不会失败；真失败也只丢这个头，不该把导出带崩 ✓。
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 门：**文件名里必须有用户名与时间戳**，且两段（ASCII / RFC 5987）都在。
    ///
    /// 这里是纯字符串判据，不起服务：起服务那一条在 `tests/routes.rs`
    ///（那里要连"归属隔离"一起验 ✓）。
    #[test]
    fn filename_carries_the_user_and_a_timestamp() {
        let stamp = "20261012-163000";
        let ascii = format!(
            "mcloud-config-{}-{stamp}.json",
            config_doc::ascii_filename_part("张三")
        );
        let utf8 = format!(
            "mcloud-config-{}-{stamp}.json",
            config_doc::percent_encode_filename("张三")
        );
        assert_eq!(ascii, "mcloud-config-owner-20261012-163000.json");
        assert!(utf8.contains("%E5%BC%A0%E4%B8%89"), "{utf8}");
        assert!(!utf8.contains('"'), "引号必须被编码掉：{utf8}");
    }
}
