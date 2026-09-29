//! 分享外链那一族：`getOutLink` / `delOutLink`（玩法 434 的本体）。
//!
//! # 它是**两次写**，而且是可回收的那种
//!
//! 建一条公开外链、再**立刻**把它删掉 —— 旧 TS `runners.ts:68-88` 就是这个形状
//! （取 `ctx.state.files` 的**第一个**建外链，上游给了 `linkID` 就马上再发一口删）。
//! 与"传文件"那几档正相反：那一档**生产**文件，这一档**消费**别人生产的文件。
//!
//! ⚠ 因此它有一个**旧实现自带的脆弱点**：同一趟里排在它前面的上传任务必须真的
//! 传上来过东西，否则`coIDLst` 是空的 —— 那时候该**不发**，而不是发一条空外链。
//!
//! # 两台主机、两套头
//!
//! `yun.139.com/orchestration/…` 这套头与网盘文件操作那一族
//! （`personal-kd-njs.yun.139.com`，见 `app_cloud_upload::cloud_file_op_headers`）
//! **不是同一套** —— 它有自己的一批 `mcloud-*` / `x-svctype`，还多一个 `caller: web`。
//! ⇒ 这里独立一份，不"顺手复用"那一族。
//!
//! 逐字出自 `packages/core/src/apis/share.ts`。
//!
//! # ⚠ 信封也不同
//!
//! 签到族是 `{code, msg, result}`，而这一族是 `{code, data:{…}}`
//! （`code` 在 TS 的类型里写作 **string**）。⇒ 不能拿 `SignInEnvelope` 解析。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};

/// 这一族的主机是 **`yun.139.com`**（不带 `user-njs` / `personal-kd-njs` 前缀），
/// 与 `BASE_USER_YUN`（`user-njs.yun.139.com`）**不是同一台**。
///
/// ⚠ 两台主机别互搬：`/orchestration/…` 这一族只在裸 `yun.139.com` 上。
const BASE_YUN: &str = "https://yun.139.com";

/// `getOutLink` 的路径。
pub const GET_OUT_LINK_PATH: &str = "/orchestration/personalCloud-rebuild/outlink/v1.0/getOutLink";
/// `delOutLink` 的路径。
pub const DEL_OUT_LINK_PATH: &str = "/orchestration/personalCloud-rebuild/outlink/v1.0/delOutLink";

/// `getOutLink` 那一发的头（`apis/share.ts` 逐字）。
///
/// ⚠ 这些值**不是"某个真实客户端"**，是 TS 里钉死的一组常量；照抄它是因为
/// "换一组值上行不行"没有样本。特别地 `x-deviceinfo` / `x-yun-client-info`
/// 里那串 `||9|7.14.4|edge||||linux unknow||zh-CN|||` **中间的竖线数量也算内容**。
pub const OUTLINK_WEB_HEADERS: [(&str, &str); 14] = [
    ("caller", "web"),
    ("cms-device", "default"),
    ("mcloud-channel", "1000101"),
    ("mcloud-version", "7.14.4"),
    ("x-deviceinfo", "||9|7.14.4|edge||||linux unknow||zh-CN|||"),
    ("x-huawei-channelsrc", "10000034"),
    ("x-svctype", "1"),
    ("x-yun-api-version", "v1"),
    ("x-yun-app-channel", "10000034"),
    ("x-yun-channel-source", "10000034"),
    (
        "x-yun-client-info",
        "||9|7.14.4|edge||||linux unknow||zh-CN|||||",
    ),
    ("x-yun-module-type", "100"),
    ("x-yun-svc-type", "1"),
    ("content-type", "application/json"),
];

/// `delOutLink` 那一发**只带头**（TS 没给它 headers）—— 但它仍然要过网盘那套
/// 通用头，否则拿不到账号上下文。这里用 `getOutLink` 那一套的下标 0..13 裁剪版：
/// `content-type` 必须留，其余照旧。
fn del_out_link_headers() -> Vec<(&'static str, &'static str)> {
    OUTLINK_WEB_HEADERS.to_vec()
}

/// 构造 `getOutLink`。
///
/// `co_id_lst` 是**待分享的文件 id**。⚠ 空表 ⇒ 报错**不发**：
/// 发一条没有内容的外链，等于拿用户的账号去建一个指向空集的东西 ——
/// 而那**是一次真的写**（会多一条外链记录）。
pub fn get_out_link_request(
    account: &str,
    co_id_lst: &[String],
    dedicated_name: &str,
) -> Result<HttpRequest> {
    if co_id_lst.is_empty() {
        return Err(Error::Config(
            "getOutLink 需要至少一个文件 id（空表会建一条指向空集的外链，那是白写一次）"
                .to_string(),
        ));
    }
    let body = serde_json::json!({
        "getOutLinkReq": {
            "subLinkType": 0,
            "encrypt": 1,
            "coIDLst": co_id_lst,
            "caIDLst": [],
            "pubType": 1,
            "dedicatedName": dedicated_name,
            "period": 1,
            "periodUnit": 1,
            "viewerLst": [],
            "extInfo": {
                "isWatermark": 0,
                "shareChannel": "3001",
            },
            "commonAccountInfo": {
                "account": account,
                "accountType": 1,
            },
        }
    });
    let mut req = HttpRequest::post(format!("{BASE_YUN}{GET_OUT_LINK_PATH}")).json_body(&body);
    for (k, v) in OUTLINK_WEB_HEADERS {
        req = req.header(k, v);
    }
    Ok(req)
}

/// 构造 `delOutLink`。
///
/// ⚠ `link_ids` 空表 ⇒ 报错**不发**（同一理由：没有东西可删就别发写请求）。
pub fn del_out_link_request(account: &str, link_ids: &[String]) -> Result<HttpRequest> {
    if link_ids.is_empty() {
        return Err(Error::Config(
            "delOutLink 需要至少一个 linkID（空表是白发一次写请求）".to_string(),
        ));
    }
    let body = serde_json::json!({
        "delOutLinkReq": {
            "linkIDs": link_ids,
            "commonAccountInfo": {
                "account": account,
                "accountType": 1,
            },
        }
    });
    let mut req = HttpRequest::post(format!("{BASE_YUN}{DEL_OUT_LINK_PATH}")).json_body(&body);
    for (k, v) in del_out_link_headers() {
        req = req.header(k, v);
    }
    Ok(req)
}

/// 这一族的信封：`{code, data}`。
///
/// ⚠ 与签到族的 `{code, msg, result}` **不是同一个**，别互用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutLinkEnvelope {
    /// `code`。TS 的类型写作 **string**，但抓到的可能是数字 ⇒ 两种都收。
    pub code: String,
    /// `data` 原样保留。`None` = 响应里没有这一格。
    pub data: Option<Value>,
}

impl OutLinkEnvelope {
    pub fn parse(body: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(body)
            .map_err(|e| Error::Parse(format!("外链响应不是 JSON：{e}")))?;
        let code = match v.get("code") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            // 缺 `code` ⇒ 当成失败码，但**把原文带上**：这一族零实拍，
            // 我们不知道"成功"长什么样，所以宁可说得出来。
            _ => {
                return Err(Error::Parse(format!(
                    "外链响应没有 `code` 这一格：{}",
                    truncate(body)
                )))
            }
        };
        Ok(Self {
            code,
            data: v.get("data").cloned(),
        })
    }

    /// 成没成。这一族的成功码口径出自 TS 的判读（`code` 那边只当字符串比）。
    pub fn is_ok(&self) -> bool {
        self.code == "0"
    }

    pub fn ensure_ok(&self, what: &str) -> Result<()> {
        if self.is_ok() {
            return Ok(());
        }
        Err(Error::Upstream {
            code: self.code.clone(),
            message: format!("{what}失败"),
        })
    }
}

fn truncate(s: &str) -> String {
    let t: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        format!("{t}…")
    } else {
        t
    }
}

/// `getOutLink` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutLink {
    /// `data.getOutLinkRes.getOutLinkResSet[*].linkID` —— **删除那一口要的就是它们**。
    pub link_ids: Vec<String>,
    /// 每条外链的地址。零实拍 ⇒ 只按 TS 的类型取 `linkUrl` 那一格（有就带上）。
    pub urls: Vec<String>,
}

/// 解析 `getOutLink`。
///
/// ⚠ **零实拍**：这一族的响应形状只有 TS 的类型声明（`GetOutLinkResponse`），
/// 没有一个真实响应体。所以这里**松**着解析（层缺一层就当空），
/// 但**绝不**把"没拿到 linkID"当成成功 —— 那个由 [`Self::ensure_found`] 判。
pub fn parse_get_out_link(env: &OutLinkEnvelope) -> Result<OutLink> {
    env.ensure_ok("建外链（getOutLink）")?;
    let set = env
        .data
        .as_ref()
        .and_then(|d| d.get("getOutLinkRes"))
        .and_then(|r| r.get("getOutLinkResSet"))
        .and_then(Value::as_array);
    let mut out = OutLink::default();
    for item in set.into_iter().flatten() {
        if let Some(id) = item.get("linkID").and_then(value_to_string) {
            out.link_ids.push(id);
        }
        if let Some(url) = item.get("linkUrl").and_then(value_to_string) {
            out.urls.push(url);
        }
    }
    Ok(out)
}

impl OutLink {
    /// 上游回成功了、但**一条 linkID 都没有** ⇒ 当失败。
    ///
    /// ⚠ 这一条是这一族最要紧的判据：外链是**写**出来的，
    /// "回 `code:0` 但没给 linkID" 意味着**有一条我们建了却删不掉的外链**。
    /// 把它读成成功，那条外链就永远留在用户的分享记录里。
    pub fn ensure_found(&self) -> Result<()> {
        if self.link_ids.is_empty() {
            return Err(Error::Parse(
                "getOutLink 回了成功但没给任何 linkID ⇒ 可能有一条建了却删不掉的外链".to_string(),
            ));
        }
        Ok(())
    }
}

/// 解析 `delOutLink`。
pub fn parse_del_out_link(env: &OutLinkEnvelope) -> Result<()> {
    env.ensure_ok("删外链（delOutLink）")
}

/// 数字/字符串都收（上游这两种都给过）。
fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
