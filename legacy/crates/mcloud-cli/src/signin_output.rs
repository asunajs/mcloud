// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use mcloud_app::{cloud, signin};
use mcloud_core::{AccountId, Scope};
use mcloud_io::{SqliteStore, SystemClock};
use mcloud_upstream::app_signin::SignInStatus;

/// `--receive` / `--revive` 这类**写开关**的结果。
///
/// 两处都要用：人看的输出是一段文本，`--json` 要的是结构化字段。
/// 打包在一起是为了让"结果在哪个分支被消费"变得显式 —— 见 `signin_one` 里的说明。
pub(crate) struct WriteOutcome {
    /// 给人看的那几行（含"正在做什么"的前缀）。
    pub(crate) human: String,
    /// 塞进 `--json` 输出的那个对象。
    pub(crate) json: serde_json::Value,
}

/// 把这次读到的余额落库，返回要打印的那一行（`None` = 没成功，已经警告过了）。
///
/// **失败不影响签到结论**：余额是顺带采集的副产品，它写不进去不该把一次成功的签到
/// 变成失败（规划 §3.4："多源取可信值，失败降级，不阻塞主流程"）。
/// 但也**不能静默** —— 悄悄不采集的后果是趋势图上少一段，而那时没人会想到
/// 是几个月前某次写库失败。所以退化成一句 `stderr` 警告。
///
/// 用 `eprintln` 而不是 `println`：这行不是签到结果的一部分，
/// 有人把 stdout 重定向成日志时不该混进去。
pub(crate) async fn record_cloud_note(
    store: &SqliteStore,
    scope: Scope<'_>,
    account_id: &AccountId,
    count: i64,
) -> Option<String> {
    match cloud::record_cloud(store, scope, account_id, count, &SystemClock).await {
        Ok(stats) => Some(format!(
            "  余额已记录：{} 豆（日增 {:+}，周增 {:+}）",
            stats.cloud_count, stats.cloud_diff, stats.cloud_diff_week
        )),
        Err(err) => {
            eprintln!("⚠ 余额没能落库（不影响本次签到）：{err}");
            None
        }
    }
}

/// `--records` 每档取多少条。
///
/// 50 是**只用来对账**的量级：H5 自己一页 10 条、滚动加载，
/// 而这里是要"一眼看完整段近期流水"，翻页反而碍事。
pub(crate) const RECORD_PAGE_SIZE: i64 = 50;

/// `--records-all` 最多翻多少页 —— 纯粹是防"上游 `total` 恒不回落到 0"那种死循环。
///
/// 按 `RECORD_PAGE_SIZE` 算，20 页 = 1000 条，比实测最大的账本（460 条）宽裕一倍。
pub(crate) const RECORD_MAX_PAGES: i64 = 20;

/// 一档账本的一页流水 → 人看的几行。
///
/// ⚠ 条目**原样打**（一条一行 compact JSON）：流水里的字段名**没有实拍**
/// （H5 只把它丢给模板渲染，从没读过具体字段名）。假装知道哪个是金额、
/// 哪个是时间，只会把猜测写进输出里 —— 而那正是"看起来很像真的"的错误。
pub(crate) fn print_cloud_records(label: &str, book: i64, page: &signin::CloudRecords) {
    let total = match page.total {
        Some(total) => total.to_string(),
        // `None` ≠ 0：上游没给，不是"一条都没有"。
        None => "上游没给".to_string(),
    };
    println!(
        "  {label}（aiDou={book}）：共 {total} 条，本页 {} 条",
        page.records.len()
    );
    if page.records.is_empty() {
        // 空**不是错**：改名前的账本本来就只到 2026-07-30。
        println!("    （本页没有记录）");
        return;
    }
    for record in &page.records {
        println!("    {record}");
    }
    // 单独把「备份翻倍奖励」挑出来再列一遍。
    //
    // 理由：这一条是**唯一能解释 `receiveList[cloudType=2]` 那个数从哪来的**凭证 ——
    // 它是**按月结算**的（次月 1 日入账），而整页流水是几百条按时间倒序的杂项，
    // 那几笔混在里面根本找不着。见 `docs/capture/app-signin-samples.txt` §3.7。
    let backup: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.get("type").and_then(serde_json::Value::as_str) == Some("backup_1"))
        .collect();
    if !backup.is_empty() {
        println!("    ── 其中「备份翻倍奖励」（type=backup_1，按月结算）──");
        for record in backup {
            println!("    {record}");
        }
    }
}

/// `--prizes` 取多少条。
///
/// 与 H5 一致（它写死 `pageSize: 10`）。这条的用途是"看有没有待领的"，
/// 不是把历史全拉下来 —— 一页足够，而且**正好与 H5 的分页判据对齐**
/// （`records.length < pageSize` ⇒ 到底了）。
pub(crate) const PRIZE_PAGE_SIZE: i64 = 10;

/// 待领取奖品的一页 → 人看的几行。
///
/// # 只打认得出的字段
///
/// `prizeName` / `insertTime` / `expireTime` 都有**实拍**（2026-09-19 账号 159），
/// 所以打它们。**过期时间才是要盯的那个** —— 奖品躺着会过期，而 H5 的列表
/// 只显示中奖时间，看不出还剩几天。
pub(crate) fn print_pending_prizes(page: &signin::PrizeRecords) {
    if page.records.is_empty() {
        // 空**不是错**：待领取本来就常常是空的（账号 1 实测就是空的）。
        println!("  没有待领取的奖品。");
        return;
    }
    println!("  待领取奖品 {} 个：", page.records.len());
    for item in &page.records {
        let name = item.prize_name.as_deref().unwrap_or("（上游没给奖品名）");
        let insert = item
            .insert_time
            .as_deref()
            .map(|time| format!("中奖 {time}"))
            .unwrap_or_else(|| "中奖时间未给".to_string());
        let expire = item
            .expire_time
            .as_deref()
            .map(|time| format!("过期 {time}"))
            .unwrap_or_else(|| "过期时间未给".to_string());
        println!("    {name}（{insert}，{expire}）");
    }
    // 与 `--records` 同一策：认不出的字段**原样带出来**，不替上游编名字。
    println!("    ── 原始条目（我们只认得出上面那几个字段）──");
    for item in &page.records {
        println!("    {}", item.raw);
    }
}

/// 打印签到状态。
///
/// **不把整份 `cal` 倒出来**：35 个格子对排障没有帮助，而"今天签没签"
/// 与"本月已签几天"才是要看的两件事。
///
/// # 单位叫「豆」，不叫「朵」
///
/// 云朵与「云盘专属 AI豆」是**同一个东西的两个名字**（2026-07-31 按 1:1 改名），
/// 所以这里不是"换算"，是**同一个量的两种叫法**。界面（H5）上写的是「豆」，
/// 输出跟着界面走 —— 用户对着 App 核数时才不会以为差了什么东西。
pub(crate) fn print_signin_status(status: &SignInStatus) {
    let today = match status.signed_today() {
        Some(true) => "已签",
        Some(false) => "未签",
        // 服务端没把任何一格标成 `t` 时才会走到这里。这时**不能**猜，
        // 因为"按日期裸查"会命中上/下月的补位格（`cal` 里真的混着它们）。
        None => "未知（服务端没标出今天是哪一格）",
    };
    println!("  今天：{today}");
    println!("  豆余额：{}", status.bean_total);
    // 两个口径不一致时**把另一个也说出来** —— 不说的话，看到 `--raw` 的人
    // 会以为我们漏读了一个字段。
    //
    // 说清它是什么：`beforeTotal` 是**2026-07-31 改名时结转进来的那笔**
    // （流水里 `mark: "shouTiaoAiDou"`「云朵转换AI豆」），是一张**冻结的快照**，
    // 所以它永远不变、也永远不等于余额。见 `SignInStatus::cloud_total`。
    //
    // 上游没给这个字段（`None`）就**不提**：那是"没说"，不是"是 0"。
    if let Some(cloud_total) = status.cloud_total
        && cloud_total != status.bean_total
    {
        println!(
            "  （另有一个数 {cloud_total}：改名时结转进来的那笔，不是余额 —— 见 `--raw` 的 beforeTotal）"
        );
    }
    println!("  连续签到：{} 天", status.sign_count);
    println!(
        "  本月已签：{} 天（服务端报 {}）",
        status.counted_month_days(),
        status.month_days
    );
    println!("  本次可得：{} 豆", status.sign_in_points);
    // 有才打这一行 —— 天天打一句"没有"是纯噪音。
    //
    // ⚠ **"在 `receiveList` 里" ≠ "现在能领"**：能不能领由 `cloudType` 决定，
    // 判据见 [`cloud_type_label`]。这里曾经一律写成"可领取：cloudType=2 × 154 朵"，
    // 而 `2` 恰恰是**下月可领取** —— 一句话把"领不了"说成了"能领 154 朵"。
    if !status.receive_list.is_empty() {
        let items: Vec<String> = status
            .receive_list
            .iter()
            .map(|item| {
                format!(
                    "{} {} 豆（cloudType={}）",
                    cloud_type_label(item.cloud_type),
                    item.cloud_num,
                    item.cloud_type
                )
            })
            .collect();
        println!("  可领取账目：{}", items.join("，"));
    }
    // 「自动备份」的门槛 —— `--multiple`（签到翻倍）与月度奖励都靠它。
    //
    // 三态要分得开：开了 / 没开 / **上游没说**。混成一个 bool 的话，
    // "判不了"会被读成"没开"，然后有人去追一个不存在的备份问题。
    let backup = match (status.cur_month_backup, status.cur_month_backup_sign_accept) {
        (Some(true), Some(false)) => "本月已开启（翻倍机会还没领，可 `--multiple`）".to_string(),
        (Some(true), Some(true)) => "本月已开启（翻倍机会本月已领过）".to_string(),
        (Some(true), None) => "本月已开启（翻倍机会领没领，上游没说）".to_string(),
        (Some(false), _) => "本月未开启 —— 翻倍奖励与月度奖励都拿不到".to_string(),
        (None, _) => "上游没给 `curMonthBackup`，判不了".to_string(),
    };
    println!("  自动备份：{backup}");
}

/// `receiveList` 里那条 `cloudType` 的人话 —— **"现在能不能领、怎么领"由它决定**。
///
/// 判据是当前版 H5 源码 `newsignin.336f18c3.js` 的点击分流（逐字见
/// `mcloud_upstream::app_signin::ReceiveItem::cloud_type`）：
///
/// | 值 | 含义 | 动作 | 生效 |
/// | --- | --- | --- | --- |
/// | `0` | **当前可领取**（`toReceive` 就是它的合计）| `receiveV3` | **直接生效** |
/// | `1` | **月度任务奖励**（上月攒下的），**需开启自动备份** | `receiveTaskExpansion(acceptDate)` | 按结算日 |
/// | `2` | **下月**才可领 | 什么都不发 | — |
/// | 其它 | 当前可领取 | `receiveV3` | 直接生效 |
///
/// `0` 与"其它"走同一条路（H5 的 `else` 分支就是 `receiveV3`）——
/// **`0` 是合法取值**，不是"上游没给"。写成 `?? ""` 的是**缺字段**那种情形。
///
/// ⚠ `1` 与 `2` 是**同一个特性的两个时点**，不是两种奖励：
/// `2` 是"本月攒的、下月才能领"的预告；等它到期就变成 `1`（那时才带 `acceptDate`）。
/// 旧项目 `mcloud-sign` 走的**老路径**把这两个数分别叫
/// `curMonthTaskRecordCount`（`>0` 就领）与 `nextMonthTaskRecordCount`（只打印）——
/// 与这里 `1 → 领` / `2 → 不发` 一一对应。
///
/// # 实测（2026-09-19，账号 159）
///
/// 账目 `[{cloudType:2, cloudNum:240}, {cloudType:0, cloudNum:6}]`、`toReceive: 6`：
/// 自动分流跳过了 240（下月），领掉了 6（当前）——
/// 领完 `toReceive` 变 `0`、`cloudType:0` 那条**从账目里消失**，而 `cloudType:2` 的 240 一动没动。
///
/// ⇒ 由此也定死了 `toReceive` 的语义：**"现在就能领"的合计**（`0` 那类之和），
/// 不含 `1` / `2` 那条线。`--raw` 打出来的原始响应里 `total` 是**现行余额**（豆），
/// 与 `toReceive` 不是一回事。
pub(crate) fn cloud_type_label(cloud_type: i64) -> &'static str {
    match cloud_type {
        0 => "可领取（立即生效）",
        1 => "月度可领取（需开启备份）",
        2 => "下月可领取",
        _ => "可领取（立即生效）",
    }
}

/// 一条领取实况 → 人看的一行。
///
/// 三种"没领到"要分得开，否则全都长得像"上游没发"：
/// - [`signin::ReceiveOutcome::NextMonth`]：**按逻辑就不该领**（下月才作数）；
/// - [`signin::ReceiveOutcome::MonthlyExpansion`] 里 `accept_date: None`：
///   **发了裸请求**（上游没给结算日，与真实客户端一致）；
/// - [`signin::ReceiveOutcome::Failed`]：**发了但失败**（例如月度那条回 `1302` 未开备份）。
pub(crate) fn receive_outcome_line(outcome: &signin::ReceiveOutcome) -> String {
    use signin::ReceiveOutcome as R;
    match outcome {
        R::NextMonth { cloud_num } => format!(
            "  跳过 {} 豆：下月才可领（`cloudType=2`，官方 H5 也只弹提示、不发请求）",
            cloud_num
        ),
        R::MonthlyExpansion {
            accept_date,
            cloud_count,
        } => {
            // 没带结算日要说出来：结果可能对不上，但**请求确实发了**。
            // 这里曾经写成"跳过不发"，那是与真实客户端分叉的说法。
            let date = match accept_date {
                Some(date) => format!("结算日 {date}"),
                None => "**没带结算日**（上游没给，照 H5 发的是裸请求）".to_string(),
            };
            match cloud_count {
                Some(count) => format!("  月度奖励领取成功（{date}）：+{count} 豆"),
                // `None` ≠ 0：上游没报数，不是"领到 0 朵"。
                None => format!("  月度奖励已提交（{date}）：上游没报领到多少"),
            }
        }
        R::Immediate {
            cloud_type,
            received,
        } => format!("  立即领取（cloudType={cloud_type}）：+{received} 豆"),
        R::Failed { cloud_type, error } => {
            format!("  领取失败（cloudType={cloud_type}）：{error}")
        }
    }
}

/// 同上，给 `--json` 用的结构化形状。
pub(crate) fn receive_outcome_json(outcome: &signin::ReceiveOutcome) -> serde_json::Value {
    use signin::ReceiveOutcome as R;
    match outcome {
        R::NextMonth { cloud_num } => {
            serde_json::json!({ "action": "nextMonth", "cloudNum": cloud_num })
        }
        R::MonthlyExpansion {
            accept_date,
            cloud_count,
        } => serde_json::json!({
            "action": "monthlyExpansion",
            "acceptDate": accept_date,
            "cloudCount": cloud_count,
        }),
        R::Immediate {
            cloud_type,
            received,
        } => serde_json::json!({
            "action": "immediate",
            "cloudType": cloud_type,
            "received": received,
        }),
        R::Failed { cloud_type, error } => serde_json::json!({
            "action": "failed",
            "cloudType": cloud_type,
            "error": error,
        }),
    }
}

/// 一条签到翻倍结果 → 人看的一行。
///
/// 四种"没领到"要分得开，否则全都长得像"上游没发"：
/// - [`signin::MultipleOutcome::AlreadyAccepted`]：**本月已经领过了**（每月一次，正常）；
/// - [`signin::MultipleOutcome::NoBackup`]：**本月没开自动备份**，机会根本没产生；
/// - [`signin::MultipleOutcome::Unknown`]：**上游没给门槛字段**，判不了，所以没发；
/// - [`signin::MultipleOutcome::Failed`]：**发了但失败**（例如回 `1302`）。
pub(crate) fn multiple_outcome_line(outcome: &signin::MultipleOutcome) -> String {
    use signin::MultipleOutcome as M;
    match outcome {
        // `multiple` 是**倍数**、`cloud_count` 是**合计** —— 旧项目那句话就是
        // `"成功获得", multiple, "倍云朵，共计", cloudCount`。两者都可能缺，
        // 所以分开拼：缺倍数就只说合计，缺合计就只说倍数。
        M::Claimed {
            cloud_count,
            multiple,
        } => match (multiple, cloud_count) {
            (Some(multiple), Some(count)) => {
                format!("  签到翻倍领取成功：{multiple} 倍，共计 +{count} 豆")
            }
            (None, Some(count)) => format!("  签到翻倍领取成功：+{count} 豆（上游没报倍数）"),
            (Some(multiple), None) => {
                format!("  签到翻倍已领取：{multiple} 倍（上游没报合计）")
            }
            // `None` ≠ 0：上游什么都没报，不是"领到 0 豆"。
            (None, None) => "  签到翻倍已领取：上游没报倍数也没报合计".to_string(),
        },
        M::AlreadyAccepted => "  跳过：本月已经领过翻倍机会（每月只有一次）".to_string(),
        M::NoBackup => "  跳过：本月未开启自动备份，翻倍机会没产生。\n  \
                        ⚠ 旧项目会上传一个文件把备份打开再重试，我们还没有上传能力 ——\n  \
                        去 App 里随便备份一个文件，下个月起就有了。"
            .to_string(),
        M::Unknown => "  跳过：`infoV3` 没给 `curMonthBackup` / `curMonthBackupSignAccept`，\
                       判不了，故一个请求都没发"
            .to_string(),
        M::Failed { error } => format!("  领取失败：{error}"),
    }
}

/// 同上，给 `--json` 用的结构化形状。
pub(crate) fn multiple_outcome_json(outcome: &signin::MultipleOutcome) -> serde_json::Value {
    use signin::MultipleOutcome as M;
    match outcome {
        M::Claimed {
            cloud_count,
            multiple,
        } => serde_json::json!({
            "action": "claimed",
            "multiple": multiple,
            "cloudCount": cloud_count,
        }),
        M::AlreadyAccepted => serde_json::json!({ "action": "alreadyAccepted" }),
        M::NoBackup => serde_json::json!({ "action": "noBackup" }),
        M::Unknown => serde_json::json!({ "action": "unknownGate" }),
        M::Failed { error } => serde_json::json!({ "action": "failed", "error": error }),
    }
}

/// 领备份好礼的一条结果 → 人看的一行。
///
/// 四档"没领"必须分得开 —— 它们在日志里都长得像"没动静"，但下一步该做什么完全不同：
/// - [`signin::BackupGiftOutcome::AlreadyReceived`]：**本月已领过**，等下次；
/// - [`signin::BackupGiftOutcome::NoBackup`]：**未开备份**，去 App 开；
/// - [`signin::BackupGiftOutcome::NothingThisMonth`]：**本月没有这个活动**，什么都不用做；
/// - [`signin::BackupGiftOutcome::Failed`]：**发了但失败**，要看错误码。
pub(crate) fn backup_gift_outcome_line(outcome: &signin::BackupGiftOutcome) -> String {
    use signin::BackupGiftOutcome as B;
    match outcome {
        B::Claimed { received } => match received {
            Some(received) => format!("  备份好礼领取成功：+{received} 豆"),
            // `None` ≠ 0：上游没说领到多少。
            None => "  备份好礼已领取：上游没报领到多少".to_string(),
        },
        B::AlreadyReceived => "  跳过：本月已经领过备份好礼（每月只有一次）".to_string(),
        B::NoBackup => "  跳过：未开启自动备份（`state=-1`）——\n  \
                        去 App 里开启自动备份，之后每月才有这份礼。"
            .to_string(),
        B::NothingThisMonth => {
            "  跳过：本月没有备份好礼可领（`curMonth` 为假）—— 这**不是**未开备份".to_string()
        }
        B::Failed { error } => format!("  领取失败：{error}"),
    }
}

/// 同上，给 `--json` 用的结构化形状。
pub(crate) fn backup_gift_outcome_json(outcome: &signin::BackupGiftOutcome) -> serde_json::Value {
    use signin::BackupGiftOutcome as B;
    match outcome {
        B::Claimed { received } => {
            serde_json::json!({ "action": "claimed", "received": received })
        }
        B::AlreadyReceived => serde_json::json!({ "action": "alreadyReceived" }),
        B::NoBackup => serde_json::json!({ "action": "noBackup" }),
        B::NothingThisMonth => serde_json::json!({ "action": "nothingThisMonth" }),
        B::Failed { error } => serde_json::json!({ "action": "failed", "error": error }),
    }
}

/// 状态 → JSON。
///
/// `signedDays` 与 `monthDays` 是**两条独立路径**算出来的同一个数：
/// 前者自己数 `cal`，后者是服务端报的。两者不一致就是上游数据有问题，
/// 摆在同一个 JSON 里比塞进一句自然语言里更容易被脚本发现。
pub(crate) fn signin_status_json(
    status: &SignInStatus,
    report: Option<&signin::SignInReport>,
) -> serde_json::Value {
    let signed_days: Vec<u8> = status
        .days
        .iter()
        .filter(|day| day.current_month && day.signed)
        .map(|day| day.day)
        .collect();

    let mut out = serde_json::json!({
        "signedToday": status.signed_today(),
        // ⚠ `beanTotal` 才是**余额**（上游 `total`）。`cloudTotal` 保留是因为它一直在这儿，
        // 但它装的是上游 `beforeTotal` —— 云朵明细（历史）页口径的另一个数，
        // **不随领取变化**。别拿它画趋势，见 `mcloud_app::cloud::record_cloud`。
        "beanTotal": status.bean_total,
        "cloudTotal": status.cloud_total,
        "signCount": status.sign_count,
        "signInPoints": status.sign_in_points,
        "monthDays": status.month_days,
        "countedMonthDays": status.counted_month_days(),
        "signedDays": signed_days,
        // `receiveList` 是 `--receive` 自动分流的取值来源；
        // `recordId` 实拍里没有，所以这里如实给 `null` 而不是编一个空串。
        //
        // `acceptDate` 只有 `cloudType=1`（月度那条线）才用得上 —— 但**要打出来**：
        // 少了它就只能干瞪眼（请求发不出去），而"为什么发不出去"必须看得见。
        "receiveList": status.receive_list.iter().map(|item| serde_json::json!({
            "cloudType": item.cloud_type,
            "cloudNum": item.cloud_num,
            "recordId": item.record_id,
            "acceptDate": item.accept_date,
        })).collect::<Vec<_>>(),
        "toReceive": status.to_receive,
        // 「自动备份」的两条门槛 —— 它们是 `--multiple` 与月度奖励（`cloudType=1`）
        // 能不能领的前提。**不给的话，"为什么没领到"就只能靠猜。**
        //
        // `null` = 上游没给这个字段，**不是 `false`** —— 见
        // `SignInStatus::cur_month_backup`。两者对 `--multiple` 的处置完全不同。
        "curMonthBackup": status.cur_month_backup,
        "curMonthBackupSignAccept": status.cur_month_backup_sign_accept,
    });
    if let Some(report) = report {
        out["signedNow"] = serde_json::json!(report.signed_now());
        // 与 `signedNow` 成对：只有分开给，"定时任务到底有没有在干活"才判得出来。
        out["alreadySigned"] = serde_json::json!(report.was_already_signed());
        // 签到前的余额。**叫 `beanBefore` 而不是 `cloudBefore`** —— 改名是有意的：
        // 这个名字以前装的是上游 `beforeTotal`（那个不随领取变动的数），
        // 叫"before"会让人以为它是"签到前的余额"。改名比留一个静默错的键好。
        out["beanBefore"] = serde_json::json!(report.before.bean_total);
    }
    out
}
