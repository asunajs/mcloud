use super::*;
use crate::Envelope;
use mcloud_core::Error;
use serde_json::Value;

/// `infoV3` 的 `cal[]` 里的一天。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignInDay {
    /// 日（1..=31）。
    pub day: u8,
    /// 该日是否已签（`s`）。
    pub signed: bool,
    /// 是否属于本月（`currentMonth == 1`）。`false` 的是上/下月的补位格。
    pub current_month: bool,
    /// 该日可获得云朵数（`n`）。
    pub points: i64,
    /// 服务端是否把该日标为"今天"（`t`）。
    pub today: bool,
}
/// `infoV3` 的 `receiveList[]` 里的一条 —— "有一笔云朵挂在账上"。
///
/// 它同时是 [`receive_v3_request`] 的**输入来源**（`cloudId` / `cloudType` 都取自这里）。
///
/// ⚠ **名字里的 "receive" 不等于"现在能领"** —— 见 [`Self::cloud_type`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveItem {
    /// `cloudType` —— **决定这一笔现在能不能领**。
    ///
    /// 判据是当前版 H5 源码 `newsignin.336f18c3.js`（本仓库 `har/h5-signin/`）里的
    /// 那个点击分流，逐字如下：
    ///
    /// ```js
    /// 1 === t.cloudType ? A("2", t.acceptDate, e)          // receiveTaskExpansion：延期领取
    /// : 2 === t.cloudType ? s.$toast("下月可领取")           // ← 纯提示，**不发任何请求**
    /// : n("receivePoints", t, "", …)                        // ← 只有这一支走 receiveV3
    /// ```
    ///
    /// 同一份源码里还有两处独立旁证：模板上 `"is-next-month": 2 === g[0]?.cloudType`，
    /// 以及标签 `2 === g[0]?.cloudType ? "" : "可领取"` —— 即 `2` **不显示"可领取"**。
    ///
    /// ⇒ 所以 `2` 是**「下月可领取」**，不是"待领取"。给它发 `receiveV3` 是
    /// **官方客户端不会做的事**，实测回 `code=0` 但 `result.receive = 0`（什么都没领到，
    /// 见 `docs/capture/app-signin-samples.txt`）。
    ///
    /// ⚠ 这里曾经写着"新版 H5 则一律走 `receiveV3`"——**没有依据，已删**。
    /// 抓包里那条账号的 `receiveList` 只有一条 `{cloudType:2, cloudNum:234}`，
    /// 而整个 HAR 里 `receiveV3` 的请求数是 **0**：官方客户端正因为 `cloudType==2`
    /// 而**从未调用过它**。这才是"2 不可领"最直接的证据。
    ///
    /// # `0` 是**当前可领取**（2026-09-19 实测）
    ///
    /// 同一天稍晚，账号 159 的账目变成两条：
    ///
    /// ```json
    /// [{cloudType:2, cloudNum:240}, {cloudType:0, cloudNum:6}], "toReceive": 6
    /// ```
    ///
    /// 走 `receiveV3(cloudType=0)` 之后：`toReceive` 变 `0`、`cloudType:0` 那条
    /// **从账目里消失**，而 `cloudType:2` 的 240 **一动没动**。
    ///
    /// ⇒ 两个结论：① `0` 是**合法取值**（"当前可领取"），不是"上游没给"——
    /// 这正是 [`receive_v3_request`] 把缺字段写成空串、而不是写成 `0` 的原因；
    /// ② `toReceive` 是**当前可领取的合计**（`6`），不是废字段。
    pub cloud_type: i64,
    /// 这笔有多少云朵（`cloudNum`）。
    pub cloud_num: i64,
    /// `recordId` —— [`receive_v3_request`] 的 `cloud_id` 来源。
    ///
    /// ⚠ 实测抓到的条目里**没有这个字段**（只有 `cloudType` / `cloudNum`），
    /// 于是 H5 的 `e.recordId ?? ""` 实际给的就是空串。这里如实做成 `Option`，
    /// **不替上游编一个默认值**。
    pub record_id: Option<String>,
    /// `acceptDate` —— **只有 `cloudType == 1` 才用得上**。
    ///
    /// H5 里 `1 === t.cloudType ? A("2", t.acceptDate, e)`，那个 `A` 就是
    /// `receiveTaskExpansion({acceptDate})`（见 [`receive_task_expansion_request`]）。
    /// 少了它，`cloudType == 1` 那条路就**根本发不出去** —— 只能干瞪眼。
    ///
    /// 实测抓到的条目（`cloudType == 2`）里没有这个字段，所以是 `Option`。
    pub accept_date: Option<String>,
}

/// `infoV3` 的解析结果 —— **判断"今天签没签"的唯一权威依据**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInStatus {
    /// **现行余额（豆）** —— 上游字段 `total`。
    ///
    /// # 为什么是 `total` 而不是 `beforeTotal`（这里记错过一次）
    ///
    /// H5 把两个数都显示出来，原话逐字：
    ///
    /// ```javascript
    /// total >= 600 && 1 == isCloudEnough
    ///   ? "恭喜你，现拥有" + total + "豆，足够兑换" + …
    ///   : total >= 600 && 0 == isCloudEnough
    ///     ? "您只需要再攒" + lackCloudNum + "豆就能兑换" + …
    /// ```
    ///
    /// 即 **`total` 是「现拥有」，`lackCloudNum` 是「还差多少」**。
    /// 实测自洽：兑换门槛 `canExchangeText[0].pOrder = 12000`，
    /// 而 `12000 - 10502(total) = 1498 = lackCloudNum` ✓。
    ///
    /// 两个数的**来源也分得清**（Vuex store `newsignin`）：
    /// `beforeTotal → setBeforeCloudTotal`、`total → setCloudTotal`；
    /// 前者只有**云朵明细（历史）页** `pointRecord` 读，后者是 **AI豆明细页**
    /// `AIBeanRecord` 读、而且**只有它进 `persist`**。
    ///
    /// ⚠ `beforeTotal` 我们**记错过**（当成余额记进 `cloud_stats`，趋势图因此是一条直线）。
    /// 它现在仍然解析出来（[`Self::cloud_total`]），但**不再当作余额用**。
    ///
    /// # 第三个独立证据（2026-09-19）：它就是账本的合计
    ///
    /// `GET cloudRecordV2?aiDou=1` 那一档的**全部条目求和，逐字等于 `total`**：
    /// 账号 1 = 7431、账号 2（159）= 10502，两个都命中。
    /// ⇒ 这个数不是"某个接口报的另一个值"，而是**那本流水从头到尾的累加**。
    ///
    /// ⚠ 云朵与 AI豆是**同一个东西的两个名字**（2026-07-31 按 1:1 改名），
    /// 所以别把这两个字段说成"两种货币" —— 它们只是**两个口径的数**。
    pub bean_total: i64,
    /// 上游字段 `beforeTotal` —— **改名那一刻的结转额**，不是余额。
    ///
    /// # 它到底是什么（2026-09-19 从流水里定案）
    ///
    /// 它就是改名交接的那一条：`GET cloudRecordV2?aiDou=1` 里
    /// `mark: "shouTiaoAiDou"` / `summary: "云朵转换AI豆"` 那笔的 `num`。
    ///
    /// | 账号 | `shouTiaoAiDou` | `beforeTotal` | AI豆账本合计 | `total` |
    /// | --- | --- | --- | --- | --- |
    /// | 1 | 5392 | **5392** ✓ | 7431 | **7431** ✓ |
    /// | 2（159）| 8027 | **8027** ✓ | 10502 | **10502** ✓ |
    ///
    /// ⇒ 两件事同时坐实：
    /// 1. `beforeTotal` = **改名时结转进来的那笔**，是一张**冻结的历史快照**，
    ///    所以它**永远不变**（先前"签到前后都不动"的实测，原因在这里）。
    /// 2. `bean_total`（= `total`）= **AI豆账本从头到尾的合计**，逐字相等 ——
    ///    这也说明那个账本自 2026-07-30 起是**完整**的。
    ///
    /// 两者差的那 2475 = **改名后新增的全部**（8 月 1556 + 9 月 904 + 7 月 15）。
    /// 先前"差 2475、差在哪没查清"的悬案到此收口。
    ///
    /// ⚠ 还有一处**没查清**、别当成已知：云朵账本（`aiDou=0`）的合计比
    /// `beforeTotal` **恰好多 100**，两个账号都是，且那 100 都是 2024 年那笔
    /// `mark:"1003"`「开启APP通知领云朵」；账本里**没有任何 -100 的扣减**。
    /// 推测是**云朵过期不入账**（那样云朵账本的合计就只能当"毛收入"、不能当余额），
    /// 但**没有证据**，故只记现象。
    ///
    /// # 为什么它是 `Option`（而 `bean_total` 不是）
    ///
    /// 判据是"缺了它还能不能回答本结构的问题"：
    /// `bean_total` 是**余额**，是我们要记、要报的那个数 —— 缺了就等于没答；
    /// 而这个是**我们不用的一个旁证数**，缺了只是少一条线索。
    /// 让一个不用的字段有能力把整条状态查询打挂，是没必要付的代价。
    pub cloud_total: Option<i64>,
    /// 本次签到可得数（`signInPoints`）。H5 渲染成 `+"{signInPoints}豆"`。
    pub sign_in_points: i64,
    /// 连续签到天数。
    pub sign_count: i64,
    /// 服务端报的本月已签天数（`monthDays`）。
    pub month_days: i64,
    /// 日历格。
    pub days: Vec<SignInDay>,
    /// 可领取的条目（`receiveList`）。
    ///
    /// 上游不给这个字段时是**空数组**，不是错误 —— 它只影响"要不要去领"，
    /// 不影响"今天签没签"这个主判据。所以这里不做 [`require_str`] 那种硬要求。
    pub receive_list: Vec<ReceiveItem>,
    /// **现在就能领**的合计（`toReceive`）。上游有时不给，所以是 `Option`。
    ///
    /// 语义是实测定的：账目 `[{2,240},{0,6}]` 时它是 `6`，领掉 `0` 那条之后变 `0`，
    /// 而 `2` 那条**不计入**。⇒ 它只统计「立即领取」那一类，不含月度线。
    pub to_receive: Option<i64>,
    /// 本月**是否已开启自动备份**（`curMonthBackup`）。
    ///
    /// 它是**两条翻倍线的总门槛**：
    /// - 月度任务奖励（`cloudType=1`）领不到会回 `1302`「开启自动备份按钮才可领取哦～」；
    /// - 签到翻倍（[`multiple_request`]）的前提条件之一。
    ///
    /// `None` = 上游没给这个字段（**不是 `false`**）。判据在
    /// [`crate::app_signin::multiple_request`] 的文档里 —— 别把 `None` 当 `false` 处理，
    /// 那会让"上游没报"变成"这个账号没开备份"。
    pub cur_month_backup: Option<bool>,
    /// 本月的**签到翻倍**是否**已经领过**（`curMonthBackupSignAccept`）。
    ///
    /// 每月只有一次机会，所以它是 [`multiple_request`] 的另一个前提条件。
    /// 旧项目 `mcloud-sign` 的判据逐字是
    /// `if (curMonthBackup && !curMonthBackupSignAccept)` —— 两个都要成立才去领。
    pub cur_month_backup_sign_accept: Option<bool>,
}

impl SignInStatus {
    /// 本月某天是否已签。传 `0` 或 `>31` 返回 `None`。
    ///
    /// 只看 `current_month == true` 的格子 —— `cal` 里还混着上月最后一天
    /// 与下月开头几天，按 `d` 裸查会命中错的月份（实测抓包里就有 `d:31` 的上月格）。
    pub fn signed_on(&self, day: u8) -> Option<bool> {
        if day == 0 || day > 31 {
            return None;
        }
        self.days
            .iter()
            .find(|d| d.current_month && d.day == day)
            .map(|d| d.signed)
    }

    /// 服务端标为"今天"的那格是否已签。
    ///
    /// 用它**不需要本地时钟** —— 这也是它比"自己算今天几号"更稳的地方
    /// （服务端与时区才是判"今天"的那一方）。`cal` 里没有 `t:true` 的格子时返回 `None`。
    pub fn signed_today(&self) -> Option<bool> {
        self.days.iter().find(|d| d.today).map(|d| d.signed)
    }

    /// 按 `cal` 数出来的本月已签天数。
    ///
    /// 与 [`SignInStatus::month_days`] 是两条独立路径（一个来自日历格、一个来自服务端字段），
    /// 相等才说明解析没走样。测试里用它对拍。
    pub fn counted_month_days(&self) -> usize {
        self.days
            .iter()
            .filter(|d| d.current_month && d.signed)
            .count()
    }
}

/// 解析 `infoV3` 的响应。
pub fn parse_sign_in_status(envelope: &Envelope) -> Result<SignInStatus, Error> {
    let payload = ensure_ok(envelope)?;
    let payload = payload.ok_or_else(|| Error::Parse("infoV3 响应没有 result".to_string()))?;

    let cal = payload
        .get("cal")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("infoV3 响应缺少 result.cal 数组".to_string()))?;

    let mut days = Vec::with_capacity(cal.len());
    for cell in cal {
        let day = require_i64(cell, "d")?;
        let day = u8::try_from(day)
            .map_err(|_| Error::Parse(format!("infoV3 的 cal[].d 越界: {day}")))?;
        days.push(SignInDay {
            day,
            signed: cell.get("s").and_then(Value::as_bool).unwrap_or(false),
            current_month: cell.get("currentMonth").and_then(Value::as_i64) == Some(1),
            points: cell.get("n").and_then(Value::as_i64).unwrap_or(0),
            today: cell.get("t").and_then(Value::as_bool).unwrap_or(false),
        });
    }

    // `receiveList` 与主判据无关：上游不给时给空数组，而不是报错 ——
    // 那会让"今天签没签"这件确定的事被一个可选的附属字段拖垮。
    let receive_list = payload
        .get("receiveList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(ReceiveItem {
                        cloud_type: item.get("cloudType").and_then(Value::as_i64)?,
                        cloud_num: item.get("cloudNum").and_then(Value::as_i64)?,
                        record_id: item
                            .get("recordId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        accept_date: item
                            .get("acceptDate")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(SignInStatus {
        bean_total: require_i64(payload, "total")?,
        cloud_total: payload.get("beforeTotal").and_then(Value::as_i64),
        sign_in_points: require_i64(payload, "signInPoints")?,
        sign_count: require_i64(payload, "signCount")?,
        month_days: require_i64(payload, "monthDays")?,
        days,
        receive_list,
        to_receive: payload.get("toReceive").and_then(Value::as_i64),
        // 两个门槛字段都是 `Option<bool>`：**只认真正的布尔**。
        // 实测上游给的就是 `true` / `false`；万一哪天变成 `1` / `0`，这里会得到 `None`，
        // 于是 `plan_multiple` 判 `Unknown` 而**不去发那个写请求** —— 这是个安全的失败方向。
        cur_month_backup: payload.get("curMonthBackup").and_then(Value::as_bool),
        cur_month_backup_sign_accept: payload
            .get("curMonthBackupSignAccept")
            .and_then(Value::as_bool),
    })
}
