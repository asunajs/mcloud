//! 今日AI豆（云朵记录汇总）—— **只读**。
//!
//! 上游那一口是 [`mcloud_upstream::app_signin::cloud_record_v2_request`]
//! （`GET cloudRecordV2`，豆的收支流水）。本模块负责把它拉全，再按两种口径给出去：
//!
//! * **今日汇总**（[`read_today`]）—— 按"今天"过滤、按账号汇总，给路由
//!   `GET /api/beans/today`（总览页那张卡）。
//! * **流水明细**（[`read_records`]）—— 不限于今日，按窗口过滤后逐笔列出，
//!   给路由 `GET /api/beans/records`（点开某个账号看"这些数是怎么来的"）。
//!
//! ⚠ 两条的**数据源是同一个上游接口**，不是本仓的运行记录/明细账本：
//! 那是"我们做过什么"，流水是"上游账上发生了什么"，两者对不上是常态
//! （重试、手动操作、别的端），明细要回答的是后者。
//!
//! # 三条口径（已定稿，见 docs/task-inventory.md §5）
//!
//! 1. **按账号记账，绝不合计**：豆是**每个账号各一本账**，加出来的总数
//!    哪个账号都用不了（豆不能跨账号花）。所以本模块的产物是 [`TodayAccount`]
//!    的一列，**没有** `total` / `net` 这类跨账号字段。
//! 2. **“今天”按东八区算**：日界取 [`beijing_offset_secs`]（`mcloud_core`
//!    里那份“东八区”的**唯一**定义），不是运行机器的本地时区。
//! 3. **失败按账号隔离**：某个账号读不到（网络 / 上游错误码 / 解析不出）
//!    ⇒ 进 [`TodayFailed`]，其余账号照常出数。[`read_today`] **不返回 `Err`**：
//!    一个账号挂了不该把整张卡打成“服务挂了”。
//!
//! # 只读（这一条是硬要求）
//!
//! 整条链只有一枪：`GET cloudRecordV2`（`aiDou=1`、`type=0`，分页）。
//! 不取滑块、不下单、没有一个会改用户账上东西的请求 —— 这个端点可以被随便刷新。
//!
//! # 分页：**不照搬“只取首页”那个缺口**
//!
//! `cloudRecordV2` 是分页接口。参照实现只取首页，一天流水超过 `pageSize`
//! 就会**静默少算**，它自己也把这条列为“重写要修”。这里改成**翻到短页为止**：
//! 一页正好装满（`== PAGE_SIZE`）就要下一页；拿满 [`MAX_PAGES`] 还没到底
//! 就**如实报错**（该账号进 `failed`），绝不把一个可能少算的数列出来。
//!
//! ⚠ 这里**不假设**流水的排序：每一页每一条都按 `insertTime` 判“是不是今天”，
//! 不做“见到一条更早的就停”那种提前退出 —— 那需要“按时间倒序”这个前提，
//! 而本仓没有它的实拍依据。

use mcloud_core::{AccountId, Clock, Error, HttpClient, Result};
use mcloud_upstream::app_signin::{self, CloudRecord, SignInEnvelope};

/// 账本：`aiDou=1` = 改名后的 **AI豆**。
///
/// ⚠ `0` 是改名前的**云朵**账本，不是同一个东西（见
/// [`mcloud_upstream::app_signin::cloud_record_v2_request`] 的说明）。
pub const LEDGER_AI_DOU: i64 = 1;

/// 不过滤方向（`type=0`）—— 进与出都在同一条流水里。
pub const TYPE_ALL: i64 = 0;

/// 一页要多少条。
pub const PAGE_SIZE: i64 = 100;

/// 最多翻几页。
///
/// 这是**兜底**，不是“只读这么多页”：正常账号一两页就到底了。真撞上它说明
/// 这个账号的流水规模远超预期，那就如实报错而不是给一个可能少算的数。
pub const MAX_PAGES: usize = 20;

const DAY_MS: i64 = 86_400_000;

// ══════════════════════════ 日界（东八区）══════════════════════════════

/// 东八区的秒偏移。
///
/// 值取自 [`mcloud_core::beijing_offset`] —— 那是本仓“东八区”的**唯一**
/// 定义（`timezone: -480` 与 `Schedule.timezone` 的默认值 `Asia/Shanghai`
/// 都是同一个事实）。这里不另写一个 `8 * 3600`：两处各写一份而互不引用，
/// 正是“东八区有两个定义”的开始。
pub fn beijing_offset_secs() -> i64 {
    mcloud_core::beijing_offset().local_minus_utc() as i64
}

/// `now_ms` 所在东八区那一天的起点（epoch 毫秒）。
pub fn day_start_ms(now_ms: i64) -> i64 {
    let offset_ms = beijing_offset_secs() * 1_000;
    // `div_euclid` 而不是 `/`：负数时间戳下 `/` 会向 0 取整，
    // 把日界算到后一天上。本仓 `now_ms` 恒为正，但判定层不留这种错法。
    let local = now_ms + offset_ms;
    local.div_euclid(DAY_MS) * DAY_MS - offset_ms
}

/// `ts_ms` 是不是 `now_ms` 所在的**东八区**自然日。左闭右开。
pub fn is_today(ts_ms: i64, now_ms: i64) -> bool {
    let start = day_start_ms(now_ms);
    (start..start + DAY_MS).contains(&ts_ms)
}

/// 明细窗口的下界（epoch 毫秒）：**含今天在内的最近 `days` 个自然日**。
///
/// * `days <= 0` ⇒ `None` = **不限**（"全部"那一档）；
/// * `days == 1` ⇒ 今天 00:00（东八区）起；
/// * `days == 7` ⇒ 今天往前数 6 天的 00:00 起（**含今天共 7 天**）。
///
/// ⚠ 这里用的是**自然日**而不是"最近 7×24 小时"：界面上那一档写的是「近 7 天」，
/// 而用户对这句话的理解是"从上周同日 0 点到现在"。按 168 小时算的话，
/// 今天早上打开时会**看不见** 7 天前上午那几笔 —— 界面上看不出差别，
/// 只会让人以为上游漏记了。
///
/// ⚠ 下界是左闭（`>=`）：与 [`is_today`] 的 `start..start+DAY_MS` 同一条口径。
pub fn range_since_ms(days: i64, now_ms: i64) -> Option<i64> {
    if days <= 0 {
        return None;
    }
    // 先落到今天的日界，再往回退 (days-1) 天 —— 于是"近 1 天"就是今天。
    Some(day_start_ms(now_ms) - (days - 1) * DAY_MS)
}

// ══════════════════════════ insertTime 归一 ═════════════════════════════

/// 把流水的 `insertTime` 串归一成 epoch 毫秒。
///
/// # 认得的形状
///
/// `YYYY-MM-DDTHH:MM:SS[.SSS]` + **显式偏移**（`Z` / `±HH:MM`），例如
/// `2026-09-30T13:00:00.000+00:00`。这一形状有同族证据：同主机同族的
/// `record` 实拍里 `insertTime` / `expireTime` 都是它
/// （`test/golden/app-signin-vectors.json` 的 `prizeRecords`）。
///
/// # 认不出就回 `None`，由调用方**如实报错**
///
/// ⚠ 尤其**不猜时区**：没有偏移的串（如 `2026-09-30 21:00:00`）这里判不了，
/// 于是回 `None`。猜错的表现是“这一笔被算进（或漏出）今天”，而界面上看不出来
/// —— 比一句“这个格式我解不出”糟得多。
///
/// 小数秒只取前 3 位（毫秒）；日界判定用不着更细。
pub fn parse_insert_time_ms(raw: &str) -> Option<i64> {
    let s = raw.trim();
    let (date, rest) = s.split_once('T')?;

    // 先切出时区偏移。时刻部分里没有 `+` / `-`，所以从右往左找到的那个符号
    // 就是偏移的符号位（`-05:00` 这种负偏移也找得到）。
    let (time, offset_secs) =
        if let Some(t) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
            (t, 0)
        } else {
            let sign_at = rest.rfind(['+', '-'])?;
            let (t, off) = rest.split_at(sign_at);
            let sign = if off.starts_with('-') { -1 } else { 1 };
            let (hh, mm) = off[1..].split_once(':')?;
            let hh: i64 = hh.parse().ok()?;
            let mm: i64 = mm.parse().ok()?;
            if hh > 23 || mm > 59 {
                return None;
            }
            (t, sign * (hh * 3_600 + mm * 60))
        };

    let (hms, frac) = match time.split_once('.') {
        Some((a, b)) => (a, b),
        None => (time, ""),
    };
    let millis: i64 = if frac.is_empty() {
        0
    } else {
        if !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // 不足 3 位补 0（`.5` 是 500 毫秒），超过 3 位截断。
        frac.chars()
            .chain("000".chars())
            .take(3)
            .collect::<String>()
            .parse()
            .ok()?
    };

    let mut time_parts = hms.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some() {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month) || day < 1 || day > crate::exchange::days_in_month(year, month) {
        return None;
    }

    // 日期算术复用 `exchange` 那一份 Howard Hinnant 实现 —— 本模块不再写第二份
    // 闰年/世纪规则（那正是这类代码最常见的错处）。
    let days = crate::exchange::days_from_civil(year, month, day);
    let utc_secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_secs;
    Some(utc_secs * 1_000 + millis)
}

// ══════════════════════════ 聚合（纯函数）══════════════════════════════

/// 一个账号**今天**的读数。
///
/// 刻意**不含账号主键**：聚合是纯函数，账号是谁由调用方配；也就不存在
/// “把两个账号的数加在一起”这种写法 —— 形状上就摆不下。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BeanDay {
    /// 今日净增（进 − 出）。**可负** —— 兑换扣豆也在这条流水里。
    pub net: i64,
    /// 今日收入合计（正数）。
    pub gained: i64,
    /// 今日支出合计（**取正数**）。
    pub spent: i64,
    /// 今日读到几笔流水。
    pub count: i64,
}

/// 把一段流水按“今天”过滤并汇总。**纯函数**，不碰网络。
///
/// # 解不出 `insertTime` 的流水 ⇒ 报错（而不是丢掉）
///
/// 丢掉它是**静默少算**：界面上“今天 +30”与“今天 +30（还有一笔不知道算不算）”
/// 长得一模一样。所以这里如实回 `Err`，由 [`read_today`] 把那个账号挪进
/// `failed` —— “没读到”与“今天没进出”是两件事，不能混。
pub fn aggregate_today(records: &[CloudRecord], now_ms: i64) -> Result<BeanDay> {
    let mut day = BeanDay::default();
    for record in records {
        let Some(raw) = record.insert_time.as_deref() else {
            return Err(Error::Parse(format!(
                "流水里有一笔没有 insertTime（summary={}），读不出它是不是今天",
                record.summary
            )));
        };
        let Some(ts) = parse_insert_time_ms(raw) else {
            return Err(Error::Parse(format!(
                "流水的 insertTime 解不出（{raw}）—— 不猜它属于哪一天"
            )));
        };
        if !is_today(ts, now_ms) {
            continue;
        }
        day.count += 1;
        // `num` 的正负号就是进出。`0` 归进收入一侧（+0），两种写法等价。
        if record.num >= 0 {
            day.gained += record.num;
        } else {
            day.spent += -record.num;
        }
    }
    day.net = day.gained - day.spent;
    Ok(day)
}

// ══════════════════════════ 拉取（只读）══════════════════════════════

/// 只读：拉一个账号的**全部** AI豆流水（翻到短页为止），再按今天汇总。
///
/// 失败（网络 / HTTP 非 2xx / 业务码非 0 / 形状不对 / 翻页翻不到底）
/// 一律原样上抛 —— 路由据此把该账号放进 `failed`，**不影响别的账号**。
pub async fn read_day(http: &dyn HttpClient, now_ms: i64) -> Result<BeanDay> {
    let mut all: Vec<CloudRecord> = Vec::new();
    for page in 1..=MAX_PAGES {
        let resp = http
            .request(app_signin::cloud_record_v2_request(
                LEDGER_AI_DOU,
                TYPE_ALL,
                page as i64,
                PAGE_SIZE,
            ))
            .await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "查询AI豆流水")?;
        let parsed = SignInEnvelope::parse(body)?;
        let batch = app_signin::parse_cloud_records(&parsed)?;
        let full = batch.len() as i64 == PAGE_SIZE;
        all.extend(batch);
        if !full {
            return aggregate_today(&all, now_ms);
        }
    }
    Err(Error::Other(format!(
        "AI豆流水翻满 {MAX_PAGES} 页仍然是满页，读不到底 —— 这一次没拿到全部记录，不报一个可能少算的数"
    )))
}

// ══════════════════════════ 明细（不限于今日）═══════════════════════════
//
// 「今日AI豆」那张卡只给一个净增数字。用户点开某个账号要看的是**这些数字由哪些
// 流水构成**，而且往往不止今天（"上周那笔 200 扣的是什么？"）。
//
// ⚠ 口径与数据源跟上面那条**完全同源**：还是上游 `cloudRecordV2` 的流水
// （`aiDou=1`、`type=0`）。**不是**本仓的运行记录 / 明细账本 —— 那是"我们做过什么"，
// 流水是"上游账上发生了什么"，两者对不上是常态（重试、手动操作、别的端）。
// 界面要回答的是后者。

/// 明细一次最多回多少笔。
///
/// 界面是一个弹窗、一列流水，再多也翻不完。撞上它时**不静默截断**：
/// [`BeanRecords::has_more`] 会置真，界面据此写一句"还有更多没列出来"。
pub const MAX_DETAIL_RECORDS: usize = 500;

/// 明细里的一笔流水（**上游原话**，只做了时间归一）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeanRecord {
    /// `insertTime` 归一成的 epoch 毫秒。**解不出就是 `None`** —— 不猜时区
    /// （理由见 [`parse_insert_time_ms`]）。
    pub at_ms: Option<i64>,
    /// 原样的 `insertTime`。`at_ms` 为 `None` 时这是**唯一**能显示的东西。
    pub raw_time: Option<String>,
    /// 正负号就是进出。
    pub num: i64,
    /// 上游给的摘要（"每日签到"这种）。
    pub summary: String,
    /// 上游给的备注/批次号。
    pub mark: String,
}

/// 一个账号的流水明细（**不限于今日**）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeanRecords {
    /// 按时间**倒序**（新的在前）。解不出时间的那几笔排在最后。
    pub records: Vec<BeanRecord>,
    /// 时间解不出的笔数。⚠ 这些笔**已经在 `records` 里**，这里只是如实数出来 ——
    /// "不知道它属于哪天"不等于"它不是一笔"。
    pub undated: i64,
    /// 还有没读到的（撞上页数上限或 [`MAX_DETAIL_RECORDS`]）⇒ 这一份可能不全。
    pub has_more: bool,
}

/// 只读：拉一个账号的**全部** AI豆流水，按窗口过滤、倒序、截到 `limit`。
///
/// # 与 [`read_day`] 共用同一套翻页口径
///
/// 一样是"翻到短页为止"，一样**不假设流水的排序**（那需要实拍依据，本仓没有），
/// 一样在撞满 [`MAX_PAGES`] 时**不静默给出一个可能不全的数列** —— 区别只在
/// 这里把"可能不全"做成一个**如实回传的标志**（[`BeanRecords::has_more`]）而不是
/// 一个错误：看明细的人点的是"全部"，看到 500 条 + 一句"还有更多"比一个红框有用。
///
/// # 窗口与解不出时间的笔
///
/// `since_ms` 为 `None` = 不限。**时间解不出的笔照样列出来**（`at_ms = None`）：
/// 按窗口把它们丢掉就是静默少算，而它们恰恰是最需要人看一眼的那种
/// （上游换过时间格式时，全都会落到这里）。
pub async fn read_records(
    http: &dyn HttpClient,
    since_ms: Option<i64>,
    limit: usize,
) -> Result<BeanRecords> {
    let mut all: Vec<CloudRecord> = Vec::new();
    let mut has_more = false;
    for page in 1..=MAX_PAGES {
        let resp = http
            .request(app_signin::cloud_record_v2_request(
                LEDGER_AI_DOU,
                TYPE_ALL,
                page as i64,
                PAGE_SIZE,
            ))
            .await?;
        let body = mcloud_upstream::session::ensure_http_ok(&resp, "查询AI豆流水")?;
        let parsed = SignInEnvelope::parse(body)?;
        let batch = app_signin::parse_cloud_records(&parsed)?;
        let full = batch.len() as i64 == PAGE_SIZE;
        all.extend(batch);
        if !full {
            break;
        }
        // 正好在第 MAX_PAGES 页还是满页 ⇒ 后面还有，如实标出来。
        if page == MAX_PAGES {
            has_more = true;
        }
    }

    let mut records: Vec<BeanRecord> = all
        .into_iter()
        .map(|r| BeanRecord {
            at_ms: r.insert_time.as_deref().and_then(parse_insert_time_ms),
            raw_time: r.insert_time,
            num: r.num,
            summary: r.summary,
            mark: r.mark,
        })
        .filter(|r| match (r.at_ms, since_ms) {
            // 窗口之外（时间认得出来、且早于下界）才丢。
            (Some(at), Some(since)) => at >= since,
            // 不限窗口，或这笔时间解不出 ⇒ 留着。
            _ => true,
        })
        .collect();

    // 倒序：新的在前。`Option` 的序是 `None < Some` ⇒ 解不出时间的自然落到最后。
    records.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    if records.len() > limit {
        records.truncate(limit);
        has_more = true;
    }
    let undated = records.iter().filter(|r| r.at_ms.is_none()).count() as i64;

    Ok(BeanRecords {
        records,
        undated,
        has_more,
    })
}

// ══════════════════════════ 多账号：失败隔离 ═══════════════════════════

/// 一个账号 + 它自己的 HTTP 传输。
///
/// 会话是按账号建的（`H5Client` 按主机注入 `jwttoken`），所以传输必须**一个
/// 账号一条**。本模块不认识账号存储，由组装层（server 路由）按归属过滤后
/// 逐账号从 `Runner::http_for` 取来。
pub struct BeanAccount<'a> {
    pub account: &'a AccountId,
    pub http: &'a dyn HttpClient,
}

/// 组装层要用的环境。
///
/// “今天”的日界由 [`BeansEnv::clock`] 决定 —— 同一趟里**所有账号共用同一个
/// `now_ms`**（在 [`read_today`] 开头取一次），否则跨零点那一瞬间两个账号会
/// 按不同的日界算。
pub struct BeansEnv<'a> {
    pub clock: &'a dyn Clock,
}

/// 一个账号今天的那一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodayAccount {
    pub account: AccountId,
    pub net: i64,
    pub gained: i64,
    pub spent: i64,
    pub count: i64,
}

/// 没读到流水的账号，以及**后端原话**的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodayFailed {
    pub account: AccountId,
    pub error: String,
}

/// 今日AI豆的一份读数。
#[derive(Debug, Clone)]
pub struct Today {
    /// 服务端算“今天”用的时刻（epoch 毫秒）。
    pub now_ms: i64,
    pub accounts: Vec<TodayAccount>,
    pub failed: Vec<TodayFailed>,
}

/// 读**一组**账号今天的AI豆。**不返回 `Err`**：账号级失败进 `failed`。
///
/// # 为什么这里必须是“逐账号隔离”
///
/// 一个账号读不到（超时、票过期、上游回非 0 码）**不是**“服务挂了”，
/// 只是那一格没数。所以这里把每个账号的 `Result` 收在闭包里，成功的进
/// `accounts`、失败的进 `failed` —— 整张卡照常渲染，读不到的那一行单独列出原因。
///
/// 各账号**并发**读：它们各自一条会话，互不影响；串行的话最慢那个账号的
/// 超时（HTTP 层 30 秒）会把整张卡拖住。
pub async fn read_today(env: &BeansEnv<'_>, accounts: &[BeanAccount<'_>]) -> Today {
    let now_ms = env.clock.now_ms();
    let reads = accounts
        .iter()
        .map(|src| async move { (src.account, read_day(src.http, now_ms).await) });
    let results = futures::future::join_all(reads).await;

    let mut ok = Vec::new();
    let mut failed = Vec::new();
    for (account, result) in results {
        match result {
            Ok(day) => ok.push(TodayAccount {
                account: account.clone(),
                net: day.net,
                gained: day.gained,
                spent: day.spent,
                count: day.count,
            }),
            Err(e) => failed.push(TodayFailed {
                account: account.clone(),
                error: e.to_string(),
            }),
        }
    }
    Today {
        now_ms,
        accounts: ok,
        failed,
    }
}

#[cfg(test)]
mod tests;
