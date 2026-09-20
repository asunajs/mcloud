//! HAR → 脱敏 fixture 抽取器。
//!
//! 这是方案里 P0 的"重点投入"：`har/*.har` 是真实流量金矿（兑换、待领取、
//! 完成通知领云朵、合成云朵、签到任务列表、短信登录…），
//! 原项目 36 个 HAR **一个都没被用于测试**。
//!
//! # 为什么在生成侧脱敏
//!
//! HAR 里含可直接复用的登录凭据。fixture 要进版本库，
//! 所以必须在写文件之前脱敏，而不是靠"记得别提交"。
//!
//! # Fail closed
//!
//! 写盘之后会把整份产物再扫一遍（[`mcloud_core::sanitize::scan_for_leaks`]）。
//! **只要发现残留就报错退出**，绝不"先生成了再说"。

use anyhow::{Context, Result, bail};
use mcloud_core::sanitize;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ============================ HAR 结构 ============================

#[derive(Debug, Deserialize)]
struct HarFile {
    log: HarLog,
}

#[derive(Debug, Deserialize)]
struct HarLog {
    #[serde(default)]
    entries: Vec<HarEntry>,
}

#[derive(Debug, Deserialize)]
struct HarEntry {
    request: HarRequest,
    response: HarResponse,
}

#[derive(Debug, Deserialize)]
struct HarRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<HarNameValue>,
    #[serde(default, rename = "postData")]
    post_data: Option<HarPostData>,
}

#[derive(Debug, Deserialize)]
struct HarNameValue {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct HarPostData {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HarResponse {
    status: u16,
    #[serde(default)]
    headers: Vec<HarNameValue>,
    content: HarContent,
}

#[derive(Debug, Deserialize)]
struct HarContent {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
}

// ============================ 报告 ============================

#[derive(Debug, Default)]
pub struct ExtractReport {
    pub har_files: usize,
    pub entries_kept: usize,
    /// 被跳过的条数与原因（预检请求、静态资源、非 JSON 响应、非上游主机、超大响应）。
    pub skipped_preflight: usize,
    pub skipped_static: usize,
    pub skipped_non_json: usize,
    pub skipped_foreign_host: usize,
    pub skipped_oversize: usize,
    pub skipped_duplicate: usize,
    /// 被删除的陈旧夹具数（本次一条都没保留、但上一轮留下同名文件的 HAR）。
    pub removed_stale: usize,
    /// 被主机白名单挡掉的主机（去重，供需要时按需放宽）。
    pub foreign_hosts: std::collections::BTreeSet<String>,
    pub leaks: Vec<String>,
    pub written: Vec<PathBuf>,
}

/// 只保留这些**主机后缀**的流量。
///
/// 夹具的用途是回放 mcloud 上游接口（L2），而 HAR 里混着大量与上游无关的流量：
/// 自家 APM 埋点（`*.cmicapm.com`）、系统与浏览器遥测（`*.microsoft.com`）、
/// 第三方 SDK（`*.tencent.com` / `*.163.com` / `*.alidns.com`）。
/// 它们体量大、对"请求构造 + 响应解析"的验证价值为零，留着只会把夹具从几 MB 撑到几十 MB，
/// 并淹没真正要验证的接口。
///
/// 用后缀匹配而不是逐个枚举主机：上游主机名带环境前缀
/// （`user-njs` / `personal-kd-njs` / `share-kd-njs`），域名后缀才是稳定特征。
/// 挡掉的主机会在报告里列出，真需要时按需放宽即可 —— 默认拒绝、可见地放宽。
const KEPT_HOST_SUFFIXES: &[&str] = &[
    "139.com",        // mcloud.139.com / *.yun.139.com / h.139.com
    "10086.cn",       // caiyun.feixin.10086.cn / *.mail.10086.cn
    "cmcc-cs.cn",     // v.cmcc-cs.cn
    "cmpassport.com", // 短信登录链路
];

/// 单个条目响应体的上限。
///
/// 超限的基本都是响应里内嵌的媒体 / base64 载荷（实测有单条 300 KB+），
/// 对"请求构造 + 响应解析"的验证没有价值，却能让夹具体积翻几倍。
/// 超限**整条跳过**而不是截断：截断会得到一份非法 JSON，反而更难查。
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

/// 同一个接口最多保留几条采样。
///
/// 一次抓包会把同一个接口打几十上百次（实测最多的一个 232 次），
/// 内容却几乎一样 —— 全留下会让夹具从几 MB 涨到几十 MB，而验证价值并不随之增加。
///
/// 取 2 而不是 1：同一接口在一条流程里出现多次是常态，
/// [`FixtureHttpClient`] 对重复记录是"按顺序消费、用完保留最后一条"，
/// 留 2 条才能覆盖到"顺序消费"这个行为本身。
const MAX_SAMPLES_PER_ENDPOINT: usize = 2;

/// 静态资源扩展名 —— 这些响应体对接口回放没价值，只会把 fixture 撑大。
const STATIC_EXTENSIONS: &[&str] = &[
    ".js", ".mjs", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".woff",
    ".woff2", ".ttf", ".map", ".mp4", ".mp3",
];

/// 抽取入口。`har_path` 可以是单个 `.har` 文件或包含 `.har` 的目录。
pub fn extract(har_path: &Path, out_dir: &Path, dry_run: bool) -> Result<ExtractReport> {
    let har_files = collect_har_files(har_path)?;
    if har_files.is_empty() {
        bail!("未找到任何 .har 文件：{}", har_path.display());
    }

    let mut report = ExtractReport {
        har_files: har_files.len(),
        ..Default::default()
    };

    if !dry_run {
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("创建输出目录失败 {}", out_dir.display()))?;
    }

    // 每个接口已保留的采样数。跨 HAR 累计 —— 上限要管的是夹具总量，
    // 而不是"每个文件里各自算一遍"。
    let mut samples: BTreeMap<String, usize> = BTreeMap::new();

    for har in &har_files {
        let text = std::fs::read_to_string(har)
            .with_context(|| format!("读取 HAR 失败 {}", har.display()))?;
        let parsed: HarFile = serde_json::from_str(&text)
            .with_context(|| format!("解析 HAR 失败 {}", har.display()))?;

        let mut entries: Vec<Value> = Vec::new();

        for entry in parsed.log.entries {
            match classify(&entry) {
                Keep::Yes => {}
                Keep::SkipPreflight => {
                    report.skipped_preflight += 1;
                    continue;
                }
                Keep::SkipStatic => {
                    report.skipped_static += 1;
                    continue;
                }
                Keep::SkipNonJson => {
                    report.skipped_non_json += 1;
                    continue;
                }
                Keep::SkipForeignHost => {
                    report.skipped_foreign_host += 1;
                    report.foreign_hosts.insert(host_of(&entry.request.url));
                    continue;
                }
                Keep::SkipOversize => {
                    report.skipped_oversize += 1;
                    continue;
                }
            }

            {
                let key = sanitize::match_key(&entry.request.method, &entry.request.url);
                let seen = samples.entry(key).or_insert(0);
                if *seen >= MAX_SAMPLES_PER_ENDPOINT {
                    report.skipped_duplicate += 1;
                    continue;
                }
                *seen += 1;
            }

            let req_headers = flatten(&entry.request.headers);
            let resp_headers = flatten(&entry.response.headers);
            let req_body = entry
                .request
                .post_data
                .as_ref()
                .and_then(|p| p.text.as_deref())
                .unwrap_or("")
                .as_bytes();
            let resp_body = entry
                .response
                .content
                .text
                .as_deref()
                .unwrap_or("")
                .as_bytes();

            let fixture = sanitize::build_fixture_entry(
                &entry.request.method,
                &entry.request.url,
                &req_headers,
                req_body,
                entry.response.status,
                &resp_headers,
                resp_body,
            );
            entries.push(fixture);
            report.entries_kept += 1;
        }

        // 输出名走 slugify：夹具目录的文件名风格要与仓库其余部分一致（见函数文档）
        let out_file = out_dir.join(format!(
            "{}.json",
            slugify(
                har.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("fixture")
            )
        ));

        if entries.is_empty() {
            // 本次一条都没保留（例如整份流量都落在主机白名单之外）：
            // 把上一轮留下的同名夹具删掉。
            //
            // 不删的话它会一直躺在 `test/fixtures/` 里 —— 而它是在**更早、更宽松的脱敏规则**
            // 下生成的，恰恰是最不该继续留在版本库里的东西。
            // 只删本工具自己会写出的那个文件名，不碰目录里的其他内容。
            if !dry_run && out_file.exists() {
                std::fs::remove_file(&out_file).ok();
                report.removed_stale += 1;
            }
            continue;
        }

        let serialized = serde_json::to_string_pretty(&entries)?;

        // Fail closed：写盘前先自检
        let leaks = sanitize::scan_for_leaks(&serialized);
        if !leaks.is_empty() {
            for leak in &leaks {
                report.leaks.push(format!("{}: {leak}", har.display()));
            }
        }

        if !dry_run && leaks.is_empty() {
            std::fs::write(&out_file, &serialized)
                .with_context(|| format!("写入 fixture 失败 {}", out_file.display()))?;
            report.written.push(out_file);
        } else if !dry_run {
            eprintln!("  跳过写入 {}（存在未脱敏残留）", out_file.display());
        }
    }

    Ok(report)
}

enum Keep {
    Yes,
    SkipPreflight,
    SkipStatic,
    SkipNonJson,
    SkipForeignHost,
    SkipOversize,
}

/// 把 HAR 文件名转成符合仓库命名约定的夹具名（kebab-case）。
///
/// HAR 名是抓包工具按页面标题写出来的，实测混着中文、空格、大写、下划线、点
/// （`sign_in_3任务列表` / `玩 AI 抽支付宝红包` / `mobile.events.data.microsoft.com_2026_06_03`）。
/// 直接拿来当文件名，夹具目录里就会同时出现四五种风格 —— 统一成：
/// **小写、非字母数字折成 `-`、连续 `-` 合并、首尾不留 `-`**。
///
/// 中文**保留**：它本身就是这个名字的信息量所在，折成 `-` 只会剩下一个看不懂的空壳。
fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_dash = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || !ch.is_ascii() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_dash = true;
        }
    }

    out
}

/// URL 的主机名（去 scheme、去端口、去路径）。
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

/// 主机是否属于上游。后缀匹配，未登记的一律不算。
fn is_upstream_host(url: &str) -> bool {
    let host = host_of(url);
    KEPT_HOST_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

fn classify(entry: &HarEntry) -> Keep {
    // 预检请求没有任何业务信息
    if entry.request.method.eq_ignore_ascii_case("OPTIONS") {
        return Keep::SkipPreflight;
    }

    // 非上游主机：见 KEPT_HOST_SUFFIXES 的说明
    if !is_upstream_host(&entry.request.url) {
        return Keep::SkipForeignHost;
    }

    // 静态资源
    let path_only = entry
        .request
        .url
        .split(['?', '#'])
        .next()
        .unwrap_or(&entry.request.url)
        .to_ascii_lowercase();
    if STATIC_EXTENSIONS.iter().any(|ext| path_only.ends_with(ext)) {
        return Keep::SkipStatic;
    }

    // 超大响应：见 MAX_RESPONSE_BYTES 的说明
    let body_len = entry.response.content.text.as_deref().map_or(0, str::len);
    if body_len > MAX_RESPONSE_BYTES {
        return Keep::SkipOversize;
    }

    // 只保留 JSON 响应：非 JSON 的响应体对"请求构造 + 响应解析"的验证没有价值
    let is_json_mime = entry
        .response
        .content
        .mime_type
        .as_deref()
        .is_some_and(|m| m.contains("json"));
    let body_is_json = entry
        .response
        .content
        .text
        .as_deref()
        .is_some_and(|t| serde_json::from_str::<Value>(t).is_ok());

    if is_json_mime || body_is_json {
        Keep::Yes
    } else {
        Keep::SkipNonJson
    }
}

fn flatten(headers: &[HarNameValue]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect()
}

fn collect_har_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        bail!("路径不存在：{}", path.display());
    }

    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(path).with_context(|| format!("读取目录失败 {}", path.display()))?
    {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("har") {
            files.push(p);
        } else if p.is_dir() {
            files.extend(collect_har_files(&p)?);
        }
    }
    files.sort();
    Ok(files)
}

/// 把报告渲染成给人看的文本。
pub fn render_report(report: &ExtractReport, dry_run: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} {} 个 HAR 文件\n",
        if dry_run {
            "[dry-run] 扫描"
        } else {
            "已处理"
        },
        report.har_files
    ));
    out.push_str(&format!("  保留条目 : {}\n", report.entries_kept));
    out.push_str(&format!(
        "  跳过      : 预检 {} / 静态 {} / 非 JSON {} / 非上游主机 {} / 超大响应 {} / 重复采样 {}\n",
        report.skipped_preflight,
        report.skipped_static,
        report.skipped_non_json,
        report.skipped_foreign_host,
        report.skipped_oversize,
        report.skipped_duplicate
    ));
    if report.removed_stale > 0 {
        out.push_str(&format!("  清理陈旧  : {} 个\n", report.removed_stale));
    }
    if !report.foreign_hosts.is_empty() {
        let hosts: Vec<&str> = report.foreign_hosts.iter().map(String::as_str).collect();
        out.push_str(&format!("  非上游主机: {}\n", hosts.join(", ")));
    }
    out.push_str(&format!(
        "  已写出    : {} 个 fixture 文件\n",
        report.written.len()
    ));
    for file in &report.written {
        out.push_str(&format!("    - {}\n", file.display()));
    }
    if report.leaks.is_empty() {
        out.push_str("  脱敏自检  : 通过（无残留）\n");
    } else {
        out.push_str(&format!(
            "  脱敏自检  : 失败，{} 处残留\n",
            report.leaks.len()
        ));
        // 按"残留形状 + 位置"合并计数后再展示。
        //
        // 逐条罗列在真实数据上没有可读性：36 份 HAR 会产出三万余条，
        // 而同一条规则缺失会在每个条目的同一个字段上重复成百上千次。
        // 合并之后剩下的是几十条"该补哪条规则"，这才是修复者要的信息。
        let mut grouped: BTreeMap<&str, usize> = BTreeMap::new();
        for leak in &report.leaks {
            // 去掉 HAR 文件名前缀（`<har 路径>: 残留描述`），只剩残留描述本身
            let desc = leak.split_once(": ").map_or(leak.as_str(), |(_, d)| d);
            *grouped.entry(desc).or_default() += 1;
        }
        for (desc, count) in grouped.iter().take(40) {
            out.push_str(&format!("    ! [{count} 次] {desc}\n"));
        }
        if grouped.len() > 40 {
            out.push_str(&format!("    … 另有 {} 种残留未展示\n", grouped.len() - 40));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_har(dir: &Path, name: &str, entries: Value) -> PathBuf {
        let path = dir.join(name);
        let har = serde_json::json!({ "log": { "entries": entries } });
        std::fs::write(&path, har.to_string()).unwrap();
        path
    }

    fn sample_entry(method: &str, url: &str, mime: &str, body: &str) -> Value {
        serde_json::json!({
            "request": {
                "method": method,
                "url": url,
                "headers": [
                    { "name": "auth", "value": "eyJhbGciOiJIUzI1NiJ9.REAL_SECRET" },
                    { "name": "user-agent", "value": "mcloud/12.6.0" }
                ],
                "postData": { "text": "{\"prizeId\":123,\"phone\":\"19900000001\"}" }
            },
            "response": {
                "status": 200,
                "headers": [{ "name": "content-type", "value": "application/json" }],
                "content": { "mimeType": mime, "text": body }
            }
        })
    }

    #[test]
    fn extracts_only_json_entries() {
        let dir = tempfile::tempdir().unwrap();
        write_har(
            dir.path(),
            "flow.har",
            serde_json::json!([
                sample_entry(
                    "POST",
                    "https://m.mcloud.139.com/a",
                    "application/json",
                    "{\"code\":0}"
                ),
                sample_entry("OPTIONS", "https://m.mcloud.139.com/a", "text/plain", "ok"),
                sample_entry(
                    "GET",
                    "https://m.mcloud.139.com/static/app.js",
                    "application/javascript",
                    "var x=1"
                ),
                sample_entry(
                    "GET",
                    "https://m.mcloud.139.com/pic.png",
                    "image/png",
                    "binary"
                ),
            ]),
        );

        let out = tempfile::tempdir().unwrap();
        let report = extract(dir.path(), out.path(), false).unwrap();

        assert_eq!(report.entries_kept, 1);
        assert_eq!(report.skipped_preflight, 1);
        assert_eq!(report.skipped_static, 2, ".js 与 .png 都算静态资源");
        assert_eq!(report.skipped_non_json, 0);
        assert!(report.leaks.is_empty(), "脱敏应通过: {:?}", report.leaks);
        assert_eq!(report.written.len(), 1);
    }

    #[test]
    fn written_fixture_contains_no_credentials() {
        let dir = tempfile::tempdir().unwrap();
        write_har(
            dir.path(),
            "sign.har",
            serde_json::json!([sample_entry(
                "POST",
                "https://m.mcloud.139.com/ycloud/signin?clientVersion=12.6.0&_t=1699999999",
                "application/json",
                "{\"code\":0,\"data\":{\"nickname\":\"账号2\",\"token\":\"SECRET_IN_BODY\"}}"
            )]),
        );

        let out = tempfile::tempdir().unwrap();
        let report = extract(dir.path(), out.path(), false).unwrap();
        assert!(report.leaks.is_empty());

        let written = std::fs::read_to_string(&report.written[0]).unwrap();
        for secret in ["REAL_SECRET", "SECRET_IN_BODY", "19900000001", "1699999999"] {
            assert!(!written.contains(secret), "{secret} 不得出现在 fixture 中");
        }
        // 业务数据与匹配键必须保留，否则 fixture 无法用于回放
        assert!(written.contains("账号2"));
        assert!(written.contains("/ycloud/signin"));
        assert!(!written.contains("_t="), "时间戳参数应被剔除");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_har(
            dir.path(),
            "a.har",
            serde_json::json!([sample_entry(
                "GET",
                "https://m.mcloud.139.com/x",
                "application/json",
                "{}"
            )]),
        );
        let out = tempfile::tempdir().unwrap();

        let report = extract(dir.path(), out.path(), true).unwrap();
        assert_eq!(report.entries_kept, 1);
        assert!(report.written.is_empty());
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }

    /// 非上游主机整条跳过，并且**要报出来**。
    ///
    /// 默认拒绝的前提是"被拒的东西看得见"：主机白名单是有限清单，
    /// 哪天真的需要某个主机，报告里得能查到它被挡过。
    #[test]
    fn foreign_hosts_are_skipped_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        write_har(
            dir.path(),
            "mixed.har",
            serde_json::json!([
                sample_entry(
                    "POST",
                    "https://m.mcloud.139.com/keep",
                    "application/json",
                    "{\"code\":0}"
                ),
                sample_entry(
                    "POST",
                    "https://dc.cmicapm.com:7080/service/drop",
                    "application/json",
                    "{\"code\":0}"
                ),
            ]),
        );

        let out = tempfile::tempdir().unwrap();
        let report = extract(dir.path(), out.path(), false).unwrap();

        assert_eq!(report.entries_kept, 1);
        assert_eq!(report.skipped_foreign_host, 1);
        assert!(
            report.foreign_hosts.contains("dc.cmicapm.com"),
            "被挡掉的主机应记下来: {:?}",
            report.foreign_hosts
        );
    }

    /// 同一接口最多留 [`MAX_SAMPLES_PER_ENDPOINT`] 条采样。
    #[test]
    fn duplicate_samples_of_one_endpoint_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let entry = |body: &str| {
            sample_entry(
                "POST",
                "https://m.mcloud.139.com/ycloud/dup",
                "application/json",
                body,
            )
        };
        write_har(
            dir.path(),
            "dup.har",
            serde_json::json!([
                entry("{\"n\":1}"),
                entry("{\"n\":2}"),
                entry("{\"n\":3}"),
                entry("{\"n\":4}"),
            ]),
        );

        let out = tempfile::tempdir().unwrap();
        let report = extract(dir.path(), out.path(), false).unwrap();

        assert_eq!(report.entries_kept, MAX_SAMPLES_PER_ENDPOINT);
        assert_eq!(report.skipped_duplicate, 4 - MAX_SAMPLES_PER_ENDPOINT);
    }

    /// 超大响应整条跳过（不是截断 —— 截断会得到一份非法 JSON）。
    #[test]
    fn oversize_responses_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let huge = format!("{{\"blob\":\"{}\"}}", "a".repeat(MAX_RESPONSE_BYTES));
        write_har(
            dir.path(),
            "huge.har",
            serde_json::json!([sample_entry(
                "POST",
                "https://m.mcloud.139.com/ycloud/huge",
                "application/json",
                &huge
            )]),
        );

        let out = tempfile::tempdir().unwrap();
        let report = extract(dir.path(), out.path(), false).unwrap();

        assert_eq!(report.entries_kept, 0);
        assert_eq!(report.skipped_oversize, 1);
        assert!(report.written.is_empty());
    }

    /// 陈旧夹具必须被删掉。
    ///
    /// 它是**更早、更宽松的脱敏规则**下的产物 —— 留着一个不再被当前规则覆盖的文件，
    /// 恰恰是最不该继续躺在版本库里的东西。
    #[test]
    fn stale_fixture_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        write_har(
            dir.path(),
            "gone.har",
            serde_json::json!([sample_entry(
                "POST",
                "https://dc.cmicapm.com:7080/service/drop",
                "application/json",
                "{\"code\":0}"
            )]),
        );

        let out = tempfile::tempdir().unwrap();
        let stale = out.path().join("gone.json");
        std::fs::write(&stale, "[]").unwrap();

        let report = extract(dir.path(), out.path(), false).unwrap();

        assert!(!stale.exists(), "陈旧夹具应被删除");
        assert_eq!(report.removed_stale, 1);
    }

    /// 夹具文件名必须落在仓库的命名约定里（kebab-case，小写，无空格/下划线）。
    #[test]
    fn har_names_are_slugified_to_kebab_case() {
        // 实测的 HAR 名风格：下划线、空格、大写、点混合
        assert_eq!(slugify("sign_in_3任务列表"), "sign-in-3任务列表");
        assert_eq!(slugify("玩 AI 抽支付宝红包"), "玩-ai-抽支付宝红包");
        assert_eq!(
            slugify("mobile.events.data.microsoft.com_2026_06_03_18_52_00"),
            "mobile-events-data-microsoft-com-2026-06-03-18-52-00"
        );
        // 连续分隔符合并、首尾不留 `-`
        assert_eq!(slugify("__a  b__"), "a-b");
        // 已经规范的保持不变
        assert_eq!(slugify("ref"), "ref");
        assert_eq!(slugify("sign-in-3"), "sign-in-3");
    }

    #[test]
    fn non_har_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = extract(dir.path(), dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("未找到任何 .har 文件"));
    }

    #[test]
    fn missing_path_is_an_error() {
        let err = extract(Path::new("/definitely/not/here"), Path::new("/tmp"), false).unwrap_err();
        assert!(err.to_string().contains("路径不存在"));
    }

    #[test]
    fn report_renders_counts() {
        let dir = tempfile::tempdir().unwrap();
        write_har(
            dir.path(),
            "b.har",
            serde_json::json!([sample_entry(
                "GET",
                "https://m.mcloud.139.com/x",
                "application/json",
                "{}"
            )]),
        );
        let out = tempfile::tempdir().unwrap();
        let report = extract(dir.path(), out.path(), false).unwrap();
        let text = render_report(&report, false);
        assert!(text.contains("保留条目 : 1"));
        assert!(text.contains("脱敏自检  : 通过"));
    }
}
