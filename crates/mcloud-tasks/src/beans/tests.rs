//! 今日AI豆（beans）测试。
//!
//! 全程离线：假 HTTP 按脚本回放**实拍形状**的流水响应，时钟用
//! [`mcloud_core::testkit::FixedClock`]（注入的时刻，东八区）。
//!
//! 断言的是四件在路由层看不到、却决定那张卡对不对的事：
//! 1. **按账号各算各的**（绝不合计）；
//! 2. **账号级失败被隔离**（一个挂了，别的照常出数）；
//! 3. `insertTime` 归一与"今天"的东八区日界（左闭右开）；
//! 4. 翻页读到短页为止，且**只发只读请求**。

use std::sync::Mutex;

use async_trait::async_trait;
use mcloud_core::testkit::FixedClock;
use mcloud_core::{AccountId, HttpClient, HttpRequest, HttpResponse, Result};

use super::*;

// ── 固定时刻 ────────────────────────────────────────────────────────────

/// 2026-09-30 21:00 CST（东八区）。
const NOW_MS: i64 = 1_790_773_200_000;
/// 同一天的 00:00 CST（= 2026-09-29T16:00Z）。
const DAY_START_MS: i64 = 1_790_697_600_000;
/// 次日 00:00 CST（= 2026-09-30T16:00Z）—— 右开边界。
const NEXT_DAY_START_MS: i64 = 1_790_784_000_000;

/// 今天的一刻（2026-09-30 09:00 CST = 01:00Z），翻页那两条用它。
const TODAY_AT_01Z: &str = "2026-09-30T01:00:00.000+00:00";

// ── 假 HTTP ─────────────────────────────────────────────────────────────

/// 按顺序回放响应、记录收到的请求。
///
/// 与 `exchange` / `signin` 那两个 `Scripted` 同形：用尽后重复最后一个，
/// 于是"某一支多发了一枪"会表现为最后那个响应被复用，而请求条数断言立刻红。
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<String>>,
    /// 让第 n 个请求（1 起）在传输层失败（`Err`）。
    fail_at: Mutex<Option<usize>>,
}

impl Scripted {
    fn new(replies: Vec<String>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into_iter().rev().collect()),
            fail_at: Mutex::new(None),
        }
    }

    fn failing_at(self, n: usize) -> Self {
        *self.fail_at.lock().unwrap() = Some(n);
        self
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn urls(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.url).collect()
    }
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        let n = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(req);
            seen.len()
        };
        if *self.fail_at.lock().unwrap() == Some(n) {
            return Err(Error::Network(format!(
                "第 {n} 个请求超时（模拟传输层失败）"
            )));
        }
        let body = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or_else(|| "{}".into())
            }
        };
        Ok(HttpResponse {
            status: 200,
            body,
            headers: Vec::new(),
        })
    }
}

// ── 夹具 ────────────────────────────────────────────────────────────────

/// 一笔流水（带 `inserttime`）。
fn rec(num: i64, insert_time: &str) -> String {
    format!(r#"{{"num":{num},"summary":"测试","mark":"m","inserttime":"{insert_time}"}}"#)
}

/// 一笔**没有** `inserttime` 的流水。
fn rec_without_time(num: i64) -> String {
    format!(r#"{{"num":{num},"summary":"测试","mark":"m"}}"#)
}

/// 一页成功的流水响应。
fn page(records: &[String]) -> String {
    format!(
        r#"{{"code":0,"msg":"success","result":{{"records":[{}]}}}}"#,
        records.join(",")
    )
}

/// 一个账号 + 它的传输。
fn source<'a>(account: &'a AccountId, http: &'a dyn HttpClient) -> BeanAccount<'a> {
    BeanAccount { account, http }
}

fn clock_at(now_ms: i64) -> FixedClock {
    FixedClock(now_ms)
}

// ── 1. 按账号各算各的 ───────────────────────────────────────────────────

/// ⭐ 两个账号各一条读数 ⇒ **各自独立**，谁都不带出对方的数。
///
/// 形状上就**没有**合计字段（[`TodayAccount`] 里一个是账号、四个是本账号
/// 的数）—— 加出来的"总AI豆"哪个账号都用不了。这条用例把两个账号的读数
/// 都钉死，反向改坏"混账"立刻红。
#[tokio::test]
async fn two_accounts_are_aggregated_separately_and_never_summed() {
    let a_id = AccountId::new("acc-a");
    let b_id = AccountId::new("acc-b");
    // A：今天进 120、出 90 ⇒ 净 30；B：今天出 200 ⇒ 净 -200。
    let a_http = Scripted::new(vec![page(&[
        rec(120, "2026-09-30T00:00:00.000+00:00"),
        rec(-90, "2026-09-30T04:00:00.000+00:00"),
    ])]);
    let b_http = Scripted::new(vec![page(&[rec(-200, "2026-09-30T02:00:00.000+00:00")])]);

    let clock = clock_at(NOW_MS);
    let env = BeansEnv { clock: &clock };
    let sources = vec![source(&a_id, &a_http), source(&b_id, &b_http)];
    let today = read_today(&env, &sources).await;

    assert_eq!(today.now_ms, NOW_MS);
    assert!(
        today.failed.is_empty(),
        "不该有账号失败：{:?}",
        today.failed
    );
    assert_eq!(today.accounts.len(), 2);
    // 顺序与入参一致（join_all 保序）。
    assert_eq!(today.accounts[0].account, a_id);
    assert_eq!(today.accounts[1].account, b_id);

    let a = &today.accounts[0];
    assert_eq!((a.net, a.gained, a.spent, a.count), (30, 120, 90, 2));
    let b = &today.accounts[1];
    assert_eq!((b.net, b.gained, b.spent, b.count), (-200, 0, 200, 1));
    // 两个账号的净增之和是 -170 —— 那是一个**用不了**的数，这里确认
    // 谁都没有把它当成自己的读数。
    assert_ne!(a.net, -170);
    assert_ne!(b.net, -170);
}

// ── 2. 账号级失败隔离 ───────────────────────────────────────────────────

/// ⭐ A 的传输挂了 ⇒ A 进 `failed`（带原话），B 照常出数 —— 整趟**不**失败。
#[tokio::test]
async fn a_failed_account_lands_in_failed_and_the_other_still_reports() {
    let a_id = AccountId::new("acc-a");
    let b_id = AccountId::new("acc-b");
    let a_http = Scripted::new(vec![page(&[rec(7, TODAY_AT_01Z)])]).failing_at(1);
    let b_http = Scripted::new(vec![page(&[rec(5, TODAY_AT_01Z)])]);

    let clock = clock_at(NOW_MS);
    let env = BeansEnv { clock: &clock };
    let sources = vec![source(&a_id, &a_http), source(&b_id, &b_http)];
    let today = read_today(&env, &sources).await;

    assert_eq!(today.accounts.len(), 1, "只有 B 读到了");
    assert_eq!(today.accounts[0].account, b_id);
    assert_eq!(today.accounts[0].net, 5);

    assert_eq!(today.failed.len(), 1);
    assert_eq!(today.failed[0].account, a_id);
    assert!(
        today.failed[0].error.contains("模拟传输层失败"),
        "失败原因要带后端原话：{}",
        today.failed[0].error
    );
}

/// `insertTime` 解不出的账号也走同一条失败路径：**不静默当 0 混进去**。
#[tokio::test]
async fn an_unreadable_insert_time_fails_only_that_account() {
    let a_id = AccountId::new("acc-a");
    let b_id = AccountId::new("acc-b");
    // A 的第一笔没有时间 ⇒ 读不出它是不是今天。
    let a_http = Scripted::new(vec![page(&[rec(7, TODAY_AT_01Z), rec_without_time(3)])]);
    let b_http = Scripted::new(vec![page(&[rec(5, TODAY_AT_01Z)])]);

    let clock = clock_at(NOW_MS);
    let env = BeansEnv { clock: &clock };
    let sources = vec![source(&a_id, &a_http), source(&b_id, &b_http)];
    let today = read_today(&env, &sources).await;

    assert_eq!(today.accounts.len(), 1);
    assert_eq!(today.accounts[0].account, b_id);
    assert_eq!(today.failed.len(), 1);
    assert_eq!(today.failed[0].account, a_id);
    assert!(
        today.failed[0].error.contains("insertTime"),
        "原因要点名是那笔流水的时间解不出：{}",
        today.failed[0].error
    );
}

// ── 3. 归一与日界 ───────────────────────────────────────────────────────

/// ⭐ "今天"= **东八区**自然日，左闭右开：本地 00:00:00.000 算今天，
/// 次日 00:00:00.000 算明天。前一天最后一毫秒**不**算。
#[test]
fn day_boundary_is_beijing_midnight_and_exclusive_at_the_end() {
    assert_eq!(beijing_offset_secs(), 8 * 3_600, "东八区偏移变了？");
    assert_eq!(day_start_ms(NOW_MS), DAY_START_MS);

    let records = vec![
        // 前一天最后一毫秒（15:59:59.999Z = 23:59:59.999 CST）⇒ 排除。
        fake_record(1_000, "2026-09-29T15:59:59.999+00:00"),
        // 本地 00:00:00.000 ⇒ 计入（左闭）。
        fake_record(1, "2026-09-29T16:00:00.000+00:00"),
        // 本地 23:59:59.999 ⇒ 计入。
        fake_record(10, "2026-09-30T15:59:59.999+00:00"),
        // 次日 00:00:00.000 ⇒ 排除（右开）。
        fake_record(100, "2026-09-30T16:00:00.000+00:00"),
    ];
    let day = aggregate_today(&records, NOW_MS).unwrap();
    assert_eq!((day.net, day.gained, day.spent, day.count), (11, 11, 0, 2));

    assert!(is_today(DAY_START_MS, NOW_MS));
    assert!(is_today(NEXT_DAY_START_MS - 1, NOW_MS));
    assert!(!is_today(DAY_START_MS - 1, NOW_MS));
    assert!(!is_today(NEXT_DAY_START_MS, NOW_MS));
}

/// `insertTime` 归一：实拍形状（`…+00:00`）与 `Z` / 显式偏移都收，
/// 小数秒按毫秒补零/截断；**没有偏移的串判不了**（不猜时区）。
#[test]
fn insert_time_parser_normalizes_the_observed_shape() {
    assert_eq!(
        parse_insert_time_ms("2026-09-30T13:00:00.000+00:00"),
        Some(NOW_MS)
    );
    assert_eq!(parse_insert_time_ms("2026-09-30T13:00:00Z"), Some(NOW_MS));
    assert_eq!(
        parse_insert_time_ms("2026-09-30T21:00:00+08:00"),
        Some(NOW_MS),
        "带显式 +08:00 的串不该被再当成东八区加一次"
    );
    // 小数秒：不足 3 位补 0、超过 3 位截断。
    assert_eq!(
        parse_insert_time_ms("2026-09-30T13:00:00.5+00:00"),
        Some(NOW_MS + 500)
    );
    assert_eq!(
        parse_insert_time_ms("2026-09-30T13:00:00.05+00:00"),
        Some(NOW_MS + 50)
    );
    assert_eq!(
        parse_insert_time_ms("2026-09-30T13:00:00.1234+00:00"),
        Some(NOW_MS + 123)
    );

    for bad in [
        // 没有偏移 ⇒ 判不了。猜时区错一整天的表现界面上看不出来。
        "2026-09-30T21:00:00",
        // 分隔符不是 `T`。
        "2026-09-30 21:00:00Z",
        // 字段越界。
        "2026-13-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-02-29T00:00:00Z",
        "2026-09-30T24:00:00Z",
        "2026-09-30T13:00:00+99:00",
        "",
        "not-a-time",
    ] {
        assert_eq!(parse_insert_time_ms(bad), None, "不该解得出：{bad}");
    }
    // 闰年是闰年（反向：`2024` 的 2 月 29 日解得出）。
    assert!(parse_insert_time_ms("2024-02-29T00:00:00Z").is_some());
}

/// ⭐ 一笔读不出时间的流水 ⇒ `aggregate_today` **报错**，而不是丢掉。
///
/// 丢掉它是静默少算：界面上"今天 +30"与"今天 +30（还有一笔不知道算不算）"
/// 长得一模一样。所以必须是一句能看见的错。
#[test]
fn aggregate_refuses_a_record_it_cannot_place_in_time() {
    let missing = aggregate_today(
        &[fake_record(1, TODAY_AT_01Z), fake_record_opt(2, None)],
        NOW_MS,
    )
    .unwrap_err();
    assert!(
        missing.to_string().contains("insertTime"),
        "要说清是时间读不出：{missing}"
    );

    let unparsable = aggregate_today(&[fake_record(1, "2026/09/30 09:00")], NOW_MS).unwrap_err();
    assert!(
        unparsable.to_string().contains("2026/09/30 09:00"),
        "要把原串带出来：{unparsable}"
    );
}

// ── 4. 翻页与只读 ───────────────────────────────────────────────────────

/// ⭐ 第一页装满 ⇒ **再要下一页**（不照搬"只取首页"那个静默少算的缺口）。
#[tokio::test]
async fn read_day_walks_pages_until_a_short_page() {
    let full: Vec<String> = (0..PAGE_SIZE).map(|_| rec(1, TODAY_AT_01Z)).collect();
    let http = Scripted::new(vec![page(&full), page(&[rec(5, TODAY_AT_01Z)])]);

    let day = read_day(&http, NOW_MS).await.unwrap();
    assert_eq!(day.count, PAGE_SIZE + 1, "两页都要算进来");
    assert_eq!(day.gained, PAGE_SIZE + 5);

    let urls = http.urls();
    assert_eq!(urls.len(), 2, "第二页必须真的去要了：{urls:?}");
    assert!(urls[1].contains("pageNumber=2"), "{}", urls[1]);
    assert!(
        urls[1].contains(&format!("pageSize={PAGE_SIZE}")),
        "{}",
        urls[1]
    );
}

/// ⭐ 只读哨兵：整条链只发 `GET cloudRecordV2`，没有体、没有一条写请求。
#[tokio::test]
async fn read_day_writes_nothing_upstream() {
    let http = Scripted::new(vec![page(&[rec(1, TODAY_AT_01Z)])]);
    let day = read_day(&http, NOW_MS).await.unwrap();
    assert_eq!(day.net, 1);

    let requests = http.requests();
    assert_eq!(requests.len(), 1);
    for req in &requests {
        assert_eq!(req.method, "GET", "只读端点不许发别的动词");
        assert!(req.body.is_none(), "查询流水不带体");
        assert!(
            req.url.starts_with(app_signin::CLOUD_RECORD_V2),
            "只能打流水那一口：{}",
            req.url
        );
    }
    assert_eq!(
        requests[0].url,
        format!(
            "{}?aiDou={LEDGER_AI_DOU}&type={TYPE_ALL}&pageNumber=1&pageSize={PAGE_SIZE}",
            app_signin::CLOUD_RECORD_V2
        ),
        "账本必须是 aiDou=1、不过滤方向"
    );
    // 反面：链上绝不能出现会改账的那些口。
    for url in http.urls() {
        for forbidden in [
            "exchangeV3",
            "getSlide",
            "receiveV3",
            "receiveRevivalReward",
        ] {
            assert!(
                !url.contains(forbidden),
                "只读端点出现了写口 {forbidden}：{url}"
            );
        }
    }
}

/// 翻满 `MAX_PAGES` 还是满页 ⇒ **报错**（该账号进 `failed`），
/// 而不是给一个可能少算的数。
#[tokio::test]
async fn read_day_refuses_to_report_a_truncated_ledger() {
    let full: Vec<String> = (0..PAGE_SIZE).map(|_| rec(1, TODAY_AT_01Z)).collect();
    let one_full_page = page(&full);
    let http = Scripted::new(vec![one_full_page; MAX_PAGES]);

    let err = read_day(&http, NOW_MS).await.unwrap_err();
    assert!(
        err.to_string().contains("读不到底"),
        "要说清是没读完：{err}"
    );
    assert_eq!(http.urls().len(), MAX_PAGES, "每一页都真的去要了");
}

// ── 5. 明细（不限于今日）───────────────────────────────────────────────

/// ⭐ 窗口的下界：`days` 是**自然日**，不是 `days × 24h`。
///
/// `1` ⇒ 今天 00:00（东八区）；`7` ⇒ 含今天共 7 个自然日；`0` / 负数 ⇒ 不限。
#[test]
fn range_since_is_beijing_midnight_of_the_first_day_in_the_window() {
    assert_eq!(range_since_ms(0, NOW_MS), None, "0 = 全部（不限窗口）");
    assert_eq!(range_since_ms(-3, NOW_MS), None);
    assert_eq!(
        range_since_ms(1, NOW_MS),
        Some(DAY_START_MS),
        "近 1 天 = 今天"
    );
    assert_eq!(
        range_since_ms(7, NOW_MS),
        Some(DAY_START_MS - 6 * 86_400_000),
        "近 7 天要含今天，所以只往回退 6 天"
    );
}

/// ⭐ 明细**不限于今日**：窗口之外丢掉，窗口之内全留，按时间倒序。
///
/// 时间解不出的那一笔**照样列出来**（排在最后、并计入 `undated`）：
/// 按窗口把它丢掉就是静默少算，而它恰恰是最该被人看一眼的那种。
#[tokio::test]
async fn read_records_filters_the_window_and_sorts_newest_first() {
    let http = Scripted::new(vec![page(&[
        // 故意乱序给：明细的排序不能指望上游。
        rec(1, "2026-09-01T00:00:00.000+00:00"),
        rec(3, TODAY_AT_01Z),
        rec_without_time(9),
        rec(2, "2026-09-29T00:00:00.000+00:00"),
    ])]);

    let got = read_records(&http, range_since_ms(1, NOW_MS), MAX_DETAIL_RECORDS)
        .await
        .unwrap();

    assert!(!got.has_more, "一页就到底了");
    assert_eq!(got.records.len(), 2, "只留今天那笔 + 时间解不出的那笔");
    // 新的在前；解不出时间的排最后。
    assert_eq!(got.records[0].num, 3);
    assert_eq!(got.records[0].at_ms, Some(NOW_MS - 12 * 3_600_000));
    assert_eq!(got.records[1].num, 9);
    assert_eq!(got.records[1].at_ms, None);
    assert_eq!(
        got.records[1].raw_time, None,
        "原串没有就如实是 None（不是空串）"
    );
    assert_eq!(got.undated, 1);
}

/// 不限窗口（`since = None`）时四笔都在，且严格倒序 —— `None` 排在最后。
#[tokio::test]
async fn read_records_without_a_window_keeps_everything_newest_first() {
    let http = Scripted::new(vec![page(&[
        rec(1, "2026-09-01T00:00:00.000+00:00"),
        rec_without_time(9),
        rec(3, TODAY_AT_01Z),
        rec(2, "2026-09-29T00:00:00.000+00:00"),
    ])]);

    let got = read_records(&http, None, MAX_DETAIL_RECORDS).await.unwrap();
    let nums: Vec<i64> = got.records.iter().map(|r| r.num).collect();
    assert_eq!(nums, vec![3, 2, 1, 9], "按时间倒序，解不出时间的垫底");
    assert_eq!(got.undated, 1);
}

/// ⭐ 撞上条数上限 ⇒ **不静默截断**：`has_more` 置真、`undated` 只数**回给前端的**那些。
#[tokio::test]
async fn read_records_reports_truncation_instead_of_silently_cutting() {
    let http = Scripted::new(vec![page(&[
        rec(1, "2026-09-30T03:00:00.000+00:00"),
        rec_without_time(9),
        rec(2, "2026-09-30T04:00:00.000+00:00"),
    ])]);

    let got = read_records(&http, None, 2).await.unwrap();
    assert!(got.has_more, "截断了就必须说");
    assert_eq!(got.records.len(), 2);
    // 倒序后前两笔是有时间的（3:00 与 4:00），那笔没时间的被截在外面 ⇒ undated = 0。
    assert_eq!(got.undated, 0, "数的是**留在结果里**的，不是被丢掉的");
}

/// ⭐ 明细也翻页：第一页装满就再要一页；`has_more` 只表示"还有没读到的"。
#[tokio::test]
async fn read_records_walks_pages_until_a_short_page() {
    let full: Vec<String> = (0..PAGE_SIZE).map(|_| rec(1, TODAY_AT_01Z)).collect();
    let http = Scripted::new(vec![page(&full), page(&[rec(5, TODAY_AT_01Z)])]);

    let got = read_records(&http, None, MAX_DETAIL_RECORDS).await.unwrap();
    assert_eq!(got.records.len() as i64, PAGE_SIZE + 1);
    assert!(!got.has_more);
    let urls = http.urls();
    assert_eq!(urls.len(), 2, "第二页必须真的去要了：{urls:?}");
    assert!(urls[1].contains("pageNumber=2"), "{}", urls[1]);
}

/// ⭐ 翻满 `MAX_PAGES` 仍是满页 ⇒ 明细**不报错**，而是 `has_more = true`。
///
/// 与 [`read_day`] 刻意不同：那边是一个"今日净增"的**数**，少算就错；
/// 这边是给人看的**列表**，给他已经读到的 500 条 + 一句"还有更多"比一个红框有用。
#[tokio::test]
async fn read_records_marks_has_more_when_it_hits_the_page_cap() {
    let full: Vec<String> = (0..PAGE_SIZE).map(|_| rec(1, TODAY_AT_01Z)).collect();
    let http = Scripted::new(vec![page(&full); MAX_PAGES]);

    let got = read_records(&http, None, MAX_DETAIL_RECORDS).await.unwrap();
    assert!(got.has_more, "读不到底必须如实说");
    assert_eq!(got.records.len(), MAX_DETAIL_RECORDS);
    assert_eq!(http.urls().len(), MAX_PAGES, "每一页都真的去要了");
}

/// ⭐ 只读哨兵（明细那一条）：只发 `GET cloudRecordV2`，没有一条写请求。
#[tokio::test]
async fn read_records_writes_nothing_upstream() {
    let http = Scripted::new(vec![page(&[rec(1, TODAY_AT_01Z)])]);
    let _ = read_records(&http, None, MAX_DETAIL_RECORDS).await.unwrap();

    let requests = http.requests();
    assert_eq!(requests.len(), 1);
    for req in &requests {
        assert_eq!(req.method, "GET", "只读端点不许发别的动词");
        assert!(req.body.is_none(), "查询流水不带体");
        assert!(
            req.url.starts_with(app_signin::CLOUD_RECORD_V2),
            "只能打流水那一口：{}",
            req.url
        );
    }
    assert_eq!(
        requests[0].url,
        format!(
            "{}?aiDou={LEDGER_AI_DOU}&type={TYPE_ALL}&pageNumber=1&pageSize={PAGE_SIZE}",
            app_signin::CLOUD_RECORD_V2
        ),
        "账本必须是 aiDou=1、不过滤方向"
    );
}

// ── 纯函数夹具（不走 HTTP） ─────────────────────────────────────────────

/// 直接造一笔 `CloudRecord`。
fn fake_record(num: i64, insert_time: &str) -> CloudRecord {
    fake_record_opt(num, Some(insert_time))
}

fn fake_record_opt(num: i64, insert_time: Option<&str>) -> CloudRecord {
    CloudRecord {
        num,
        summary: "测试".into(),
        mark: "m".into(),
        insert_time: insert_time.map(String::from),
    }
}
