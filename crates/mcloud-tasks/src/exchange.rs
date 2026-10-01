//! 兑换（抢兑）—— **判定层与编排**。
//!
//! 对应参考实现的 `internal/app/exchange_plan.go`（448 行，判定）与
//! `internal/app/exchange_run.go`（625 行，链）。逐条移植：档位、判定顺序、
//! 证据强度的标注都保留。
//!
//! # 为什么判定要独立成纯函数
//!
//! 下单是**不可逆消耗**：这一族没有任何幂等键，`oid` 由服务端生成
//! （见 [`ExchangeOrder::order_id`]），发出去就不收回来。代价因此
//! **完全不对称** —— 该不发时发了 = 花掉真AI豆；该发时不发 = 少一次机会，
//! 重跑还有。于是每一条"发"都必须指得出是**哪一档判据**放行的，
//! 而逐档的可观察后果只有纯函数才测得准（反向改坏哪一档，哪一条用例红）。
//!
//! # 这一层只读四样东西
//!
//! `limit`（本账号状态）、三个余量数（`count` / `dailyRemainderCount` /
//! `minRemainderCount`）、`pOrder`（价格，两拼法已在解析层收口）与余额快照。
//! **不读** `type` / `nextTimeStatus` / `prizeSectionStatus` / `riskUserEnable`
//! 那一族：前两个只有一档实拍（`1035` / `1`），后一组在参考项目里
//! **全仓无读取点** —— 拿望文生义的名字当判据，错的时候不报错，
//! 只会多发或少发一笔真扣豆的请求。
//!
//! # 分层
//!
//! ```text
//! 判定层（纯函数，不碰网络）    plan_exchange / locate_exchange_prize / plan_exchange_wait
//! 编排层（链）                  exchange —— 读清单 → 判 → 取 deviceId
//!                                          → 最多 SLIDER_ROUNDS 轮 {取题 → 解算 → 下单}
//! 任务壳                        ExchangeTask（**当前未**注册进 Registry，见文末）
//! ```
//!
//! # 一个必须说清的边界：`deviceId` 是**请求体里的值**
//!
//! 生产路径上的 HTTP 客户端是 `mcloud_app::smid::SmidClient`，它会透明地补
//! **请求头** `deviceid`。但 `exchangeV3` 要的是**下单体里那个 `deviceId` 键**
//! （实拍四条每一条都带了非空值）—— 头补得再多，体里那一位仍然是空的。
//! 所以链上留了一个显式的 [`DeviceIdFn`] 注入点，见 [`ExchangeEnv`] 那段。

use std::fmt;

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{Clock, Error, HttpClient, Result, TaskOutcome};
use mcloud_engine::{ParamSpec, Task, TaskContext, TaskResult};
use mcloud_upstream::app_signin::{self, SignInEnvelope, SignInStatus};
use mcloud_upstream::exchange::{
    self, ExchangeList, ExchangeResult as V3Result, SlidePuzzle, CODE_DUPLICATED,
    CODE_NOT_TARGET_USER, CODE_OK, CODE_SLIDER_FAILED, LIMIT_AVAILABLE, LIMIT_EXCHANGED,
    LIMIT_GROUP_EXCHANGED, LIMIT_TOMORROW, LIMIT_UNKNOWN3,
};
// `ExchangePrize` / `ExchangeOrder` 由下面那两条 `pub use` 转出（它们都出现在
// 本模块的**公开**签名里），所以这里**不再**从分组导入里引一次 —— 会撞名（E0252）。

// ══════════════════════════════ 常量 ══════════════════════════════

/// 清单里那一件的类型，**转出**给只看清单的调用方用。
///
/// ⚠ 转出是必要的而不是图省事：`ExchangePrize` 出现在本模块的**公开**
/// 签名里（[`JudgedPrize`] 的字段、[`ExchangeOutcome::prize`]），
/// 而一个公开结构体上的私有类型会让下游**用不了**那个字段
/// （拿不到类型名就没法声明变量、没法 `use`）。
///
/// 转出的只是**类型**，不是新的行为：上游换形状时这里跟着换，
/// 下游仍按同一份字段读。
pub use mcloud_upstream::exchange::ExchangePrize;

/// 成交那一笔的类型，**转出**理由同 [`ExchangePrize`]：
/// 它出现在 [`ExchangeOutcome::order`] 上。
///
/// ⚠ 一并转出还有一层现实原因：`mcloud-server` **不依赖** `mcloud-upstream`
/// （它只依赖 `mcloud-app` 与 `mcloud-tasks`）。路由要构造/断言订单，
/// 若不从这里转出，就得为了两个类型加一条 crate 依赖 —— 那会把
/// "server 不直接碰上游协议层"这条边界拆掉。
pub use mcloud_upstream::exchange::ExchangeOrder;

/// 取题 → 解算 → 下单，最多跑几轮 = 2。
///
/// 出处：旧实现 `code === 514` 时"重取滑块 + 再下单一次"（一次，不是循环）。
///
/// ⚠ 为什么第二轮**不**换成"再取题但不下单"：第二次 `514` 之后再取题就只是
/// 白花一次滑块额度（滑块接口计失败次数，见
/// [`mcloud_crypto::SLIDE_MAX_DEVIATION`] 那段）。
pub const SLIDER_ROUNDS: u32 = 2;

/// 整点后多久之内算"还在上一场里"= 20 秒。
///
/// 出处是参考实现那条组合判据（有效窗口 `hh:59:40 ~ (hh+1):00:15`，
/// `getMsToNextHour` 在 `nowMs < 20000` 时不再等待，见
/// `docs/business-feature-inventory.md` §2.5）。
///
/// ⚠ **上游是否真的整点开抢，我们没有证据**：我们自己那条成功兑换发生在
/// `12:16`（抓包 §5.4，CST），根本不在整点窗口里 ⇒ "整点开抢"**不是**上游的
/// 硬条件，至少对那一件不是。所以这一档只服务 `skip_wait == false`
/// （想赶下一场）那条路，默认参数（`skip_wait == true`）完全不碰它。
pub const WAIT_LEAD_INTO_HOUR_MS: i64 = 20_000;

/// 奖品自身时间窗的串形态 —— `"2006-01-02 15-04-05"`。
///
/// ⚠⚠ **时分秒之间也是 `-`**，不是 `:`。实拍两条都是这个形状：
/// `"2026-09-01 00-00-00"` / `"2026-09-30 23-59-59"`
/// （`test/golden/app-exchange-vectors.json`，见
/// `crates/mcloud-upstream/src/exchange/tests.rs` 里那两条断言）。
///
/// 按 `:` 解会让**所有**带窗口的商品判成"解不出"，而解不出的处置是**放行**
/// （见 [`ExchangeWindow::Unknown`]）—— 于是一条本该拦住的规则静默失效，
/// 且没有任何一处会报错。
pub const TIME_LAYOUT: &str = "%Y-%m-%d %H-%M-%S";

// ══════════════════════════════ 判定档 ══════════════════════════════

/// "这一件现在能不能下单"的档位。
///
/// 除 [`ExchangePlan::Go`] 之外每一档都说得出**为什么不是别的一档**：这些句子
/// 会直接变成界面上的一行，"跳过"那一类如果不说清是哪条判据挡的，
/// 用户就不知道该等明天还是换一件。
///
/// # 证据强度（逐档标注 —— **别当成一张同级表**）
///
/// | 档位 | 证据强度 |
/// |---|---|
/// | [`Go`](Self::Go) | 实拍：可兑那件的 `limit=0` |
/// | [`NotFound`](Self::NotFound) / [`Ambiguous`](Self::Ambiguous) | 我们自己的**匹配规则**，非上游字段 |
/// | [`OffShelf`](Self::OffShelf) | `onLine==1` 有实拍；⚠ **非 `1` 的取值零样本** |
/// | [`LimitMissing`](Self::LimitMissing) | **零样本**（实拍每件都有 `limit`） |
/// | [`LimitTomorrow`](Self::LimitTomorrow) | **零样本**，只有 H5 那句文案「明日再来」 |
/// | [`ExchangedThisPrize`](Self::ExchangedThisPrize) | ⭐ **有字段依据**：兑掉 `251230053` 后 `limit` 由 `0` 变 `2` |
/// | [`LimitThree`](Self::LimitThree) | ⚠ **两处文案冲突**（H5「已兑换」/ 旧实现「暂不可用」）⇒ 存疑 |
/// | [`ExchangedThisGroup`](Self::ExchangedThisGroup) | ⭐ **有字段依据**：`probability2.monthGroupLimit` 同组三件全是 `4` |
/// | [`LimitUnknown`](Self::LimitUnknown) | **零样本**（上游改了形态才会到这一档） |
/// | [`MinRemainderZero`](Self::MinRemainderZero) | H5 这一档显示「奖品被抢光啦」；字段本身有实拍 |
/// | [`SoldOut`](Self::SoldOut) / [`DailyStockEmpty`](Self::DailyStockEmpty) | 字段有实拍；⚠ 是**时刻量** |
/// | [`PriceMissing`](Self::PriceMissing) | **零样本**（实拍每件都有价格） |
/// | [`InsufficientCloud`](Self::InsufficientCloud) | 有 H5 那句 `x < t.pOrder ⇒ "云朵不够啦"` |
/// | [`NotStarted`](Self::NotStarted) / [`Ended`](Self::Ended) | ⚠ **只有一档实拍形状**，且"越界时上游回什么码"**无样本** |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExchangePlan {
    /// **可以下单**：下面每一条判据都有值、且都放行。
    ///
    /// ⚠ 它是本类型的**零值**（与参考实现的 `ExchangePlanGo` 同形），于是
    /// "忘了填 plan 的结构体"读起来就是"可以下单"。靠两条守住：判定值只能
    /// 出自 [`plan_exchange`] / [`locate_exchange_prize`]（链上没有别处会造一个
    /// `ExchangePlan`），而"零值当放行"这条后果由
    /// [`tests::an_unjudged_plan_never_sends_a_single_request`] 盯着。
    #[default]
    Go = 0,
    /// 清单里**没有**入参说的那件（既不是这里的 prizeId，也没有同名商品）。
    NotFound,
    /// **同名多件** —— 不能凭猜挑一件下单。
    Ambiguous,
    /// `onLine` 明确不是 `1`（下架）。⚠ 非 `1` 的取值**零样本**。
    OffShelf,
    /// 上游**没给** `limit` ⇒ 本账号状态判不了。
    ///
    /// ⚠ 单独一档且**不发**：`limit` 是 H5 那枚按钮的第一判据，缺了它就没有
    /// 任何依据说这件可兑；当成"多半可兑"会白发一笔真扣豆的请求。
    LimitMissing,
    /// `limit == 1`（H5 文案"明日再来"）。⚠ 本轮样本里**一件都没有**。
    LimitTomorrow,
    /// `limit == 2`：**这一件本身**本月已兑过。
    ///
    /// ⭐ 有字段依据：成功兑掉 `251230053` 之后它的 `limit` 由 `0` 变成 `2`。
    ExchangedThisPrize,
    /// `limit == 3`：**两处文案冲突**那一档（H5 说"已兑换，查看奖品"，
    /// 参考项目说"暂不可用"）⇒ 不挑一个信，一样不发。
    LimitThree,
    /// `limit == 4`：**同组别的件**本月已兑（组名额用掉了）。
    ExchangedThisGroup,
    /// `limit` 是这五档之外的值 —— 上游改了形态，别猜。
    LimitUnknown,
    /// `minRemainderCount == 0`：H5 在这一档显示"奖品被抢光啦"。
    MinRemainderZero,
    /// `count <= 0`：总投放余量为零。
    SoldOut,
    /// `dailyRemainderCount <= 0`：今日余量为零。
    DailyStockEmpty,
    /// 上游**没给**价格 ⇒ "余额够不够"这一条判不了。
    ///
    /// ⚠ 当成"不要豆"会白发一笔；当成"不够"会漏兑。缺判据只有停下来这一种处置。
    PriceMissing,
    /// 余额快照低于价格（H5 那句 `x < t.pOrder ⇒ "云朵不够啦"`）。
    InsufficientCloud,
    /// `startTime` 解得出、且还没到。
    NotStarted,
    /// `endTime` 解得出、且已经过了。
    Ended,
}

impl ExchangePlan {
    /// 全部**十七**档，按声明顺序。
    ///
    /// ⚠ 顺序不是随意的：选品接口的档位名与前端那张颜色表按它对齐。
    /// 测试 [`tests::every_exchange_plan_has_a_distinct_slug_and_a_reason`]
    /// 靠它逐个点名，所以**加一档必须同时改这里**。
    pub const ALL: [ExchangePlan; 17] = [
        ExchangePlan::Go,
        ExchangePlan::NotFound,
        ExchangePlan::Ambiguous,
        ExchangePlan::OffShelf,
        ExchangePlan::LimitMissing,
        ExchangePlan::LimitTomorrow,
        ExchangePlan::ExchangedThisPrize,
        ExchangePlan::LimitThree,
        ExchangePlan::ExchangedThisGroup,
        ExchangePlan::LimitUnknown,
        ExchangePlan::MinRemainderZero,
        ExchangePlan::SoldOut,
        ExchangePlan::DailyStockEmpty,
        ExchangePlan::PriceMissing,
        ExchangePlan::InsufficientCloud,
        ExchangePlan::NotStarted,
        ExchangePlan::Ended,
    ];

    /// 这一档说给用户的那半句（前面会接上"没兑换："）。
    ///
    /// ⚠ 只复述**判据本身**，不解释上游语义：`limit` 五档里有两档只有文案
    /// 没有字段依据、三个余量数是时刻量（会因为**别的用户**在同一分钟里兑掉
    /// 而变化）。具体数值（价格、余额、那个不认识的 `limit`）不在这里 ——
    /// 链把它们放进 [`ExchangeOutcome`]，由 [`ExchangeResult::summary_line`] 拼；
    /// 纯函数才留得住可测性。
    pub fn reason(self) -> &'static str {
        match self {
            ExchangePlan::Go => "可以下单",
            ExchangePlan::NotFound => "兑换清单里没有这一件（prizeId 与同名商品都没找到）",
            ExchangePlan::Ambiguous => "清单里同名的商品不止一件，不能凭猜下单",
            ExchangePlan::OffShelf => "这件商品的上架状态不是「在架」（onLine 非 1）",
            ExchangePlan::LimitMissing => {
                "上游没给这件的可兑状态（limit 缺失），判不了，本次没发请求"
            }
            ExchangePlan::LimitTomorrow => "上游状态是 limit=1（H5 那句文案是「明日再来」）",
            ExchangePlan::ExchangedThisPrize => "这一件本月已经兑过了（limit=2）",
            ExchangePlan::LimitThree => {
                "上游状态是 limit=3，它的含义两处说法互相冲突（H5「已兑换」/ 旧实现「暂不可用」），不猜，本次没发请求"
            }
            ExchangePlan::ExchangedThisGroup => "同组奖品本月已兑过（limit=4，组内互斥）",
            ExchangePlan::LimitUnknown => "上游给了一个我们不认识的 limit 取值，本次没发请求",
            ExchangePlan::MinRemainderZero => {
                "这件的余量阈值已经是 0（H5 在这一档显示「奖品被抢光啦」）"
            }
            ExchangePlan::SoldOut => "上游报的当前剩余为 0（count）",
            ExchangePlan::DailyStockEmpty => "上游报的今日余量为 0（dailyRemainderCount）",
            ExchangePlan::PriceMissing => {
                "上游没给这件商品的价格，余额够不够判不了，本次没发请求"
            }
            ExchangePlan::InsufficientCloud => "余额不够这件的价格，本次没发请求",
            ExchangePlan::NotStarted => "这件的开售时间还没到（startTime 晚于当前时刻）",
            ExchangePlan::Ended => "这件的兑换时间已过（endTime 早于当前时刻）",
        }
    }

    /// 这一档是不是"不该发请求"。
    ///
    /// 写成方法而不是让各调用点比 `plan != ExchangePlan::Go`：链上有一处要把
    /// "放行"当成继续往下走的条件、另两处（等待之后重判、界面渲染）只想知道
    /// "这算不算拦住了"，一个词表比六处字面量比较不容易漏。
    pub fn blocks(self) -> bool {
        self != ExchangePlan::Go
    }

    /// 这一档的**机器可读**名字，只给选品接口的响应体用。
    ///
    /// # 为什么不把 [`ExchangePlan`] 直接序列化成一个数
    ///
    /// 那等于把"档位的**顺序**"变成对外契约：加一档（或按便利把某档挪个位置）
    /// 会让前端那张颜色表**静默错配** —— 界面把"本月已兑"染成"可兑"的绿。
    /// 串就不一样了：新档位没登记 slug 时 `match` 那一支是**编译期**穷尽的，
    /// 而 [`tests::every_exchange_plan_has_a_distinct_slug_and_a_reason`]
    /// 要求十七档**逐一**有名字且互不相同。
    ///
    /// ⚠ 这些串**不是**上游回的字段，是我们自己的档位名。
    pub fn slug(self) -> &'static str {
        match self {
            ExchangePlan::Go => "go",
            ExchangePlan::NotFound => "not_found",
            ExchangePlan::Ambiguous => "ambiguous",
            ExchangePlan::OffShelf => "off_shelf",
            ExchangePlan::LimitMissing => "limit_missing",
            ExchangePlan::LimitTomorrow => "limit_tomorrow",
            ExchangePlan::ExchangedThisPrize => "exchanged_this_prize",
            ExchangePlan::LimitThree => "limit_three_conflict",
            ExchangePlan::ExchangedThisGroup => "exchanged_this_group",
            ExchangePlan::LimitUnknown => "limit_unknown",
            ExchangePlan::MinRemainderZero => "min_remainder_zero",
            ExchangePlan::SoldOut => "sold_out",
            ExchangePlan::DailyStockEmpty => "daily_stock_empty",
            ExchangePlan::PriceMissing => "price_missing",
            ExchangePlan::InsufficientCloud => "insufficient_cloud",
            ExchangePlan::NotStarted => "not_started",
            ExchangePlan::Ended => "ended",
        }
    }
}

impl fmt::Display for ExchangePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 定位：入参那一句 → 清单里的那一件
// ─────────────────────────────────────────────────────────────────────────────

/// 按参数 `prizeIdOrName` 在清单里定位一件。
///
/// 返回的 plan 只会是 [`ExchangePlan::Go`]（找到且唯一）/
/// [`ExchangePlan::NotFound`] / [`ExchangePlan::Ambiguous`] 三档，其余档位由
/// [`plan_exchange`] 负责 —— 分开放是因为它们**回答的是两个问题**：
/// "你说的是哪一件" 与 "那一件现在能不能兑"。
///
/// 找到时回 `(Some(prize), Go)`；没找到/同名多件回 `(None, 那一档)`。
/// 这样调用方**不可能**拿到一件空 prize 却看不见档位。
///
/// # 匹配规则：纯数字按 prizeId，其余按名称**精确相等**
///
/// 与参考实现同一取向（它是"入参是字符串时先查清单做名称→ID 解析"）。
/// 名称匹配取**精确相等**而不是"包含"：包含匹配在同名前缀的一堆商品里
/// （实拍就有"移动云盘100G云空间月卡 / 年卡 / 白银会员月卡 / 季卡…"这一类）
/// 会把"我要月卡"打到**别的**一件上 —— 而这是真扣豆。宁可说"没找到"。
///
/// ⚠ "旧实现用的是不是包含匹配"**未核**（本仓没有那份 TS 源码，只有摘要）。
/// 要核：拿到那份源码后比 `resolvePrizeId` 的匹配表达式；
/// 本仓这条规则的代价上界是"少兑一次"，安全侧。
pub fn locate_exchange_prize(
    list: &ExchangeList,
    query: &str,
) -> (Option<ExchangePrize>, ExchangePlan) {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return (None, ExchangePlan::NotFound);
    }

    // 纯数字 ⇒ 按 prizeId。这里**不**用"解析失败就走名称"的错误分支来决定走哪条：
    // "12 34" 这种带空格的串既不是 id 也不是名字，两边走哪条都是猜 ⇒
    // 落到名称匹配，名称里没有带空格的商品自然回"没找到"，
    // 那句话比"按 id 猜了一半"诚实。
    if let Ok(prize_id) = trimmed.parse::<i64>() {
        return match list.find(prize_id) {
            Some(prize) => (Some(prize.clone()), ExchangePlan::Go),
            None => (None, ExchangePlan::NotFound),
        };
    }

    let matches: Vec<&ExchangePrize> = list
        .all()
        .into_iter()
        // 没名字的那件不参与名称匹配（也不报错：清单照样能按 id 兑）。
        .filter(|item| {
            item.name
                .as_deref()
                .is_some_and(|name| name.trim() == trimmed)
        })
        .collect();

    match matches.as_slice() {
        [only] => (Some((*only).clone()), ExchangePlan::Go),
        [] => (None, ExchangePlan::NotFound),
        _ => (None, ExchangePlan::Ambiguous),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 判定（纯函数，不碰网络）
// ─────────────────────────────────────────────────────────────────────────────

/// 判"这一件、这个余额、这个时刻，能不能下单"。**纯函数**，不碰网络。
///
/// # `balance` 为什么是 `Option`
///
/// `None` = 手上**没有**余额读数，判不出 [`ExchangePlan::InsufficientCloud`]。
/// 界面上的选品器用的就是它：`infoV3` 是**账号级**请求（与 `exchangeList`
/// 不同一条链），多打一枪就多一分风控噪声，而"看一眼清单"这件事的收益
/// 不足以付它 —— 余额那一栏页面上本来就有别的路。
///
/// ⚠ 代价写清楚：`None` 这一路"可兑"比下单前的真判据**宽**，用户点了仍然
/// 可能被链挡下（那一次挡下是有钱包错误码的，不会静默）。
///
/// # 匹配顺序 = 从"这件不存在"到"我不够豆"
///
/// 先状态（`onLine` → `limit`）、再供给（三个余量）、再需求（价格与余额）、
/// 最后时间。两个方向的考虑：
///
/// - **具体原因优先**：`limit=2`（本月已兑）比"库存不足"更少出现，也更可执行；
/// - **便宜的挡在前面**：`limit` 与余量都来自**同一次**清单读取，不额外花请求；
///   余额那一条要用**另一次**读取的结果（链上先取后判），判不过就不必再去换
///   deviceId 与取滑块题面 —— 那两枪各有副作用（设备指纹上报、滑块失败计数）。
///
/// ⚠ 时间档**排在最后**是刻意的：`startTime` / `endTime` 的形态只有一档实拍，
/// 而它俩解不出来时**不挡**（见 [`ExchangeWindow::Unknown`]）—— 把一条可能因
/// 形态而误判的规则放在最前，会让所有真判据（limit / 余量 / 余额）
/// 一句都念不出来。
///
/// # `now_ms` 的时区
///
/// 收 unix 毫秒。时间窗串按**固定东八区**解（[`mcloud_core::beijing_offset`]，
/// 与 smid 特征体里写死的 `timezone: -480` 同源），所以调用方给 UTC 毫秒即可，
/// 不需要自己换时区。
pub fn plan_exchange(prize: &ExchangePrize, balance: Option<i64>, now_ms: i64) -> ExchangePlan {
    if prize.on_line.is_some_and(|on_line| on_line != 1) {
        return ExchangePlan::OffShelf;
    }
    // `limit` 五档：只有放行（== 0）才继续判下面的。
    //
    // ⚠ 这里**不**复用 [`ExchangePrize::can_exchange_now`]：它把 `limit == 0` 与
    // `min_remainder_count != 0` 合成一个 bool（H5 那枚按钮的口径），而本函数
    // 要的是"哪一条挡的" —— 合成之后 `limit=2` 与"余量为 0"说的是同一句
    // "不可兑"，界面上就成了"不能兑"，用户不知道该等明天还是该换一件。
    let limit = plan_exchange_limit(prize.limit);
    if limit.blocks() {
        return limit;
    }
    // 以下三条是**同一个 switch** 的三个分支（顺序即优先级）：
    // 余量阈值为 0 → 总余量 ≤ 0 → 今日余量 ≤ 0。
    if prize.min_remainder_count.is_some_and(|v| v == 0) {
        return ExchangePlan::MinRemainderZero;
    }
    if prize.count.is_some_and(|v| v <= 0) {
        return ExchangePlan::SoldOut;
    }
    if prize.daily_remainder_count.is_some_and(|v| v <= 0) {
        return ExchangePlan::DailyStockEmpty;
    }
    let Some(price) = prize.price else {
        return ExchangePlan::PriceMissing;
    };
    if balance.is_some_and(|balance| price > balance) {
        return ExchangePlan::InsufficientCloud;
    }
    plan_exchange_window(prize, now_ms)
}

/// 把 `limit` 五档翻成档位。放行时返回 [`ExchangePlan::Go`]。
///
/// ⚠ 这里**不**用"`None` 也放行"那种兜底：`limit` 缺失是单独一档
/// （[`ExchangePlan::LimitMissing`]）且**不发**。
///
/// ⚠ 也**不**复用 [`ExchangePrize::can_exchange_now`]，理由见 [`plan_exchange`]。
fn plan_exchange_limit(limit: Option<i64>) -> ExchangePlan {
    let Some(limit) = limit else {
        return ExchangePlan::LimitMissing;
    };
    if limit == LIMIT_AVAILABLE {
        return ExchangePlan::Go;
    }
    if limit == LIMIT_TOMORROW {
        return ExchangePlan::LimitTomorrow;
    }
    if limit == LIMIT_EXCHANGED {
        return ExchangePlan::ExchangedThisPrize;
    }
    if limit == LIMIT_UNKNOWN3 {
        return ExchangePlan::LimitThree;
    }
    if limit == LIMIT_GROUP_EXCHANGED {
        return ExchangePlan::ExchangedThisGroup;
    }
    ExchangePlan::LimitUnknown
}

/// 奖品自身时间窗的**三态**。**必须有"判不了"这一格**：
/// 两值布尔会把"上游没给 / 形态不认识"落成"在期"或"不在期"之一，两个都不对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeWindow {
    /// 明确在期内（或两边都解不出 ⇒ 放行，见 [`Self::Unknown`] 的分流）。
    In,
    /// 明确越界（还没到 或 已经过了）。
    Out,
    /// **这一边判不了**。
    ///
    /// ⚠ 与趣玩AI那一族**刻意不同**的处置：那里判不了就**不发**（写请求会花掉
    /// 当天唯一一次额度）；这里判不了**放行**。理由是这一族的硬判据是 `limit`
    /// 与余额，而 `startTime` / `endTime` 更像"这一件挂在页面上的时间" ——
    /// ⚠ 这句是**推断**，没有样本能说清越界时上游回什么码。放行一侧的代价是
    /// 可能白发一笔并被上游挡（那会落在链的失败/未知档，不会静默扣豆而不吭声），
    /// 挡住一侧的代价是**所有**没给窗口的商品都兑不了。
    Unknown,
}

/// 只判**解得出来**的那一边；两边都解不出来就是判不了。
///
/// 解不出来只当"这一边不存在"，**不报错** —— 那正是这里要的三态输入。
pub fn exchange_window_of(prize: &ExchangePrize, now_ms: i64) -> ExchangeWindow {
    let mut known = 0u32;
    let mut out = false;
    if let Some(start) = prize.start_time.as_deref().and_then(parse_exchange_time) {
        known += 1;
        out = out || now_ms < start;
    }
    if let Some(end) = prize.end_time.as_deref().and_then(parse_exchange_time) {
        known += 1;
        out = out || now_ms > end;
    }
    if known == 0 {
        return ExchangeWindow::Unknown;
    }
    if out {
        return ExchangeWindow::Out;
    }
    ExchangeWindow::In
}

/// 把时间窗三态落到档位：判不了与在期都放行，只有**明确越界**才挡。
///
/// 越界那一侧还要再分一次（"还没到"与"已经过了"处置完全不同：等一场 vs 换一件）。
fn plan_exchange_window(prize: &ExchangePrize, now_ms: i64) -> ExchangePlan {
    if exchange_window_of(prize, now_ms) != ExchangeWindow::Out {
        return ExchangePlan::Go;
    }
    if let Some(start) = prize.start_time.as_deref().and_then(parse_exchange_time) {
        if now_ms < start {
            return ExchangePlan::NotStarted;
        }
    }
    ExchangePlan::Ended
}

/// 解一档 `"2006-01-02 15-04-05"`（**固定东八区**），回 unix 毫秒。
///
/// 形态见 [`TIME_LAYOUT`]。解不出来回 `None` —— 调用方按 [`ExchangeWindow`]
/// 的三态处置，**不报错**。
///
/// # 为什么自己算，而不是 `chrono::NaiveDateTime::parse_from_str`
///
/// 两条理由，第二条是硬的：
///
/// 1. `chrono` 的默认格式串是 `%Y-%m-%d %H:%M:%S`，与本形态**不同**
///    （按 `:` 解会让**所有**带窗口的商品判成"解不出"，而解不出的处置是
///    **放行** ⇒ 时间窗规则整体静默失效）。自己拆 + 逐字段**校验**
///    （`13` 月、`32` 日回 `None`，不被"规范化"成别的日期）只有十来行。
/// 2. ⚠ `mcloud-tasks` 的 `Cargo.toml` **没有** `chrono` 依赖，
///    而本模块的写入范围不含 `Cargo.toml` ⇒ 这里引不到它。
///
/// 所以走纯算术：**东八区 = unix 秒 + 8 小时**（[`BEIJING_OFFSET_SECS`]，
/// 与 [`mcloud_core::beijing_offset`] 同一个事实、同为固定偏移、无夏令时）。
/// 这样"按东八区而不是按运行机器的本地时区"这件事是**逐字写出来**的，
/// 不依赖任何库的时区解析行为。
fn parse_exchange_time(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let (date, time) = raw.split_once(' ')?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    // ⚠ 时分秒之间也是 `-`，与日期一致（实拍如此，见 [`TIME_LAYOUT`]）。
    let mut time_parts = time.split('-');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some() {
        return None;
    }

    // 逐字段校验：`13` 月、`32` 日、`24:00:00` 一律回 `None`，
    // 而**不是**被"规范化"成别的时刻（那会让一条越界的时间窗看起来合法）。
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if day > days_in_month(year, month) {
        return None;
    }
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let local_secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
    Some((local_secs - BEIJING_OFFSET_SECS) * 1_000)
}

/// 东八区的秒偏移。
///
/// 与 [`mcloud_core::beijing_offset`] 同一个事实（`UTC+8`，无夏令时）。
/// 那边返回 `chrono::FixedOffset`，而本 crate 引不到 `chrono`（见
/// [`parse_exchange_time`]），所以这里以秒的形式写一遍，
/// 并由 [`tests::time_parsing_agrees_with_core_beijing_offset`] 与它**对拍** ——
/// 两处各写一份而**不**对拍，正是"东八区有两个定义"的开始。
const BEIJING_OFFSET_SECS: i64 = 8 * 3_600;

/// `(year, month, day)` → 1970-01-01 起的天数。
///
/// Howard Hinnant 的 `days_from_civil`，逐字实现。选它而不是手写闰年循环，
/// 是因为闰年与世纪规则在这一段里已经收口成一行算术，
/// 而"我自己写的闰年判断"正是这类代码最常见的错处。
///
/// ⚠ 提升到 `pub(crate)` 是给 `crate::beans` 的 `insertTime` 归一回用：
/// 那边要的正是同一套日期算术，抄一份过去就是"闰年规则有两个实现"。
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (month + 9) % 12; // 3 月 → 0
    let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// 这个月有多少天（含闰年规则）。
///
/// ⚠ 与 [`days_from_civil`] 同为 `pub(crate)` 的理由：`crate::beans` 的
/// `insertTime` 归一要用同一套校验。
pub(crate) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 等待窗口（`skip_wait == false` 才用得上）
// ─────────────────────────────────────────────────────────────────────────────

/// "这一趟要不要等、等到什么时候"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExchangeWait {
    /// 要不要等。
    pub needed: bool,
    /// 等待目标时刻（unix 毫秒；`needed == false` 时无意义）。
    pub until_ms: i64,
}

/// 按参考实现的取向算等待目标：整点后 20 秒内不等（还在上一场尾巴里），
/// 否则等到**下一个整点**。
///
/// # 不减"提前量"（旧实现那个 `offset = 10` 毫秒）
///
/// 参数面**没有** `offset` 这一栏（只有 `prizeIdOrName` + `skipWait`），
/// 而 10 毫秒小于我们一次请求的往返 —— 拿它当判据只会让"什么时候发出去"
/// 这件事不可复现。
///
/// # 时区不影响结果
///
/// 目标取的是"下一个整点"这个**时刻**，而这里按 UTC 毫秒截断；
/// 东八区是**整小时**偏移 ⇒ 按 UTC 截断与按东八区截断给出同一个时刻
/// （半小时偏移的时区才会错开，本仓不涉及）。
pub fn plan_exchange_wait(now_ms: i64) -> ExchangeWait {
    // `rem_euclid` 而不是 `%`：负数时间戳下 `%` 会给出负的余数，
    // 于是"整点"会被算到未来的一小时上。本仓 `now_ms` 恒为正，
    // 但这里不该让一个对负输入才错的写法留在判定层里。
    let hour = now_ms - now_ms.rem_euclid(3_600_000);
    if now_ms - hour < WAIT_LEAD_INTO_HOUR_MS {
        return ExchangeWait {
            needed: false,
            until_ms: 0,
        };
    }
    ExchangeWait {
        needed: true,
        until_ms: hour + 3_600_000,
    }
}

// ══════════════════════════════ 结论档位 ══════════════════════════════

/// 这一趟的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExchangeStatus {
    /// "没走到能报结论的那一步"（取票失败、清单读失败…）。
    ///
    /// ⭐ 它**刻意占住零值**：本链唯一的写请求花的是真豆，而"漏填 status 的
    /// 结果结构体"如果读成 [`ExchangeStatus::Ordered`]，界面上就是一条绿色的
    /// "已兑到"。这一档在 [`ExchangeResult::run_report`] 里落**失败**，
    /// 不会静默 —— 见 [`tests::status_zero_value_is_not_attempted`]。
    #[default]
    NotAttempted,
    /// **下单成功且余额差对得上**（判据链两条都满足）。
    Ordered,
    /// **判据不成立 ⇒ 一条写请求都没发**。
    ///
    /// 它是成功还是跳过，看的是"用户要的那件到底兑没兑到"
    /// （见 [`ExchangeResult::run_report`]）。
    Skipped,
    /// 上游回 `2301 重复兑奖`：这一件此前已经兑过，本次没有重复发奖 ⇒
    /// 目标其实已经达成，不算失败。
    AlreadyExchanged,
    /// **滑块这一关没过**：两个口径分歧太大而没算出来（一枪都没发），
    /// 或 [`SLIDER_ROUNDS`] 轮都被上游判错。
    SliderUnresolved,
    /// 上游回 `407`（实拍那句 `msg` 是「非移动用户不可领奖」）。
    ///
    /// ⚠ 它是**账号级**闸门：两件不同奖品回的是同一个码。
    NotTargetUser,
    /// **结果判不了**：下单那枪遇到传输层错误（可能已成交也可能没有），
    /// 或上游回 `code 0` 而余额差与价格**对不上**。⇒ 必须人去核对，
    /// 程序不能替它编一个结论。
    Unknown,
    /// 上游明确回了失败（`415`/`420`/`421`/`610`/`614`/`615`…），
    /// 以及 `501`（那是我们自己的代码漏带了 offset —— 领域层本不该走到那里）。
    Failed,
}

// ══════════════════════════════ 链的环境 ══════════════════════════════

/// 兑换链要用的最小环境。
///
/// 分成一个结构体而不是一串参数，是为了让**生产**与**测试**走同一条链：
/// 测试给一个假 `http` 与固定时钟，生产给 `H5Client` 与系统时钟。
///
/// # ⚠ `deviceId` 为什么是一个显式的注入点
///
/// 生产路径上的 `http` 是 `mcloud_app::smid::SmidClient`，它会透明地补
/// **请求头** `deviceid`。但 `exchangeV3` 要的是**下单体里那个 `deviceId` 键**
/// （实拍四条每一条都带了非空值，抓包 §5.2）—— 头补得再多，体里那一位
/// 仍然是空的。所以链上把"取一个 deviceId"留成显式的 [`DeviceIdFn`]：
/// 由组装层注入"上报指纹换 id"那一套（它要指纹存储 + 随机源，
/// 都在 `mcloud-app`，`mcloud-tasks` 依赖不到）。
///
/// ⚠ "少了它会回什么码"**没有样本** ⇒ 不猜：`None` 或空串一律**一枪不发**。
pub struct ExchangeEnv<'a> {
    /// HTTP 传输。生产上是带会话与 smid 的那条装饰链。
    pub http: &'a dyn HttpClient,
    /// 时钟：整点等待与商品时间窗都靠它。
    pub clock: &'a dyn Clock,
    /// 换 H5 `deviceId` 的注入点；`None` ⇒ 这一趟**不发**下单。
    ///
    /// 做成闭包而不是把实现传进来：这一枪要**上报设备指纹**，
    /// 只有全部判据都放行之后才值得打（见 [`exchange`] 里取用的时机）。
    pub device_id: Option<&'a DeviceIdFn<'a>>,
}

impl fmt::Debug for ExchangeEnv<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 这里刻意**只**打印"有没有注入点"：环境里迟早会带上账号，
        // 而 `Account` 的 Debug 会把凭据打出来。
        f.debug_struct("ExchangeEnv")
            .field("device_id", &self.device_id.is_some())
            .finish_non_exhaustive()
    }
}

/// 换 H5 `deviceId` 的注入点。
///
/// 生产上是"上报设备指纹 → 从响应里取 `detail.deviceId` 并拼 `B` 前缀"
/// 那一套（算法在 `mcloud_upstream::smid`，编排在 `mcloud_app::smid`）；
/// 测试上是一个固定串或一个必然失败的分支。
pub type DeviceIdFn<'a> = dyn Fn() -> Result<String> + Send + Sync + 'a;

/// "等到某个时刻"的注入点，收 unix 毫秒。
///
/// `None` 时 `skip_wait == false` 这一支会**如实挡下**（见 [`exchange`]）——
/// 绝不"静默不等待"：那会让这个开关看起来生效、实际没等，
/// 而"什么时候发出去"恰恰是这条链最有副作用的一件事。
pub type WaitFn<'a> = dyn Fn(i64) -> Result<()> + Send + Sync + 'a;

// ══════════════════════════════ 链 ══════════════════════════════

/// 链上每一个环节的读数。
#[derive(Debug, Clone)]
pub struct ExchangeOutcome {
    /// 结局档位。零值是 [`ExchangeStatus::NotAttempted`]（刻意，见那一档的说明）。
    pub status: ExchangeStatus,
    /// 最后**生效**的那一次判定（等待之后重判过就是重判那份）。
    pub plan: ExchangePlan,
    /// 判据来自的那一件（清单读数，含价格与名称）。
    pub prize: ExchangePrize,
    /// 下单**之前**那次余额读数。
    pub balance: i64,
    /// 下单**之后**复查的那次读数；`None` = 没读到（不判成交）。
    pub after_balance: Option<i64>,

    /// `Some` ⇒ 这一趟先等到这个时刻（`skip_wait == false` 那条路）。
    pub waited_until_ms: Option<i64>,
    /// "取题 → 解算"跑了几轮。
    pub rounds: u32,
    /// **实际发出的下单枪数**。
    ///
    /// ⭐ 与 `rounds` 分开数，是因为 `attempts == 0` 是本模块最重要的一条不变式：
    /// 凡判据没过 / 算不出 offset，一枪都不能发
    /// （[`tests::a_rejected_plan_sends_nothing`] 与
    /// [`tests::an_unresolvable_slider_sends_nothing`] 钉这条）。
    pub attempts: u32,
    /// 最后一枪带出去的 `puzzleOffset`；`None` ⇒ 没发过单。
    pub offset: Option<i64>,
    /// 换不到 `deviceId` 的原因（有值 ⇒ 一枪没发）。
    pub device_id_failed: String,

    /// 上游回的业务码。
    pub code: i64,
    /// 上游那句原文（只进小结与日志，不进 API 响应）。
    pub message: String,
    /// 成交那一份（只在 [`ExchangeStatus::Ordered`] 时非 `None`）。
    pub order: Option<ExchangeOrder>,
    /// 本地失败原因（解算报错、等待被取消…），与上游码是两回事。
    pub error: String,
}

impl Default for ExchangeOutcome {
    fn default() -> Self {
        Self {
            status: ExchangeStatus::NotAttempted,
            plan: ExchangePlan::Go,
            prize: ExchangePrize::default(),
            balance: 0,
            after_balance: None,
            waited_until_ms: None,
            rounds: 0,
            attempts: 0,
            offset: None,
            device_id_failed: String::new(),
            code: 0,
            message: String::new(),
            order: None,
            error: String::new(),
        }
    }
}

impl ExchangeOutcome {
    /// "这一趟看见余额少了多少"；核不到（缺任一次读数或没有价格）时回 `None`。
    pub fn spent(&self) -> Option<i64> {
        let after = self.after_balance?;
        self.prize.price?;
        Some(self.balance - after)
    }
}

/// "读清单 + 读余额 + 判"这一段的产物。
///
/// 独立成一个结构体只为了一件事：`skip_wait == false` 等完整点之后要
/// **原样再跑一遍**（清单与余额都是时刻量，等了一小时的那份判据不能继续用）。
/// 写成两段会漂移 —— 漂移的表现是"等完之后的那一轮少查了余额"，而它不报错。
struct Judge {
    prize: ExchangePrize,
    balance: i64,
    plan: ExchangePlan,
}

/// 读清单 → 定位 → （定位成功才）读余额 → 判。
///
/// 余额只在**定位成功**之后才读：查无此件时那一枪只是白打
/// （`infoV3` 也是账号级请求，少打一次就少一分风控噪声）。
async fn judge_exchange(env: &ExchangeEnv<'_>, query: &str, now_ms: i64) -> Result<Judge> {
    let list = read_exchange_list(env).await?;
    let (prize, plan) = locate_exchange_prize(&list, query);
    if plan.blocks() {
        // 定位失败时**不读余额**，所以 `balance` 是 0（没有读数）。
        // ⚠ 这不是"余额 0"的意思：`status` 会是 `Skipped`、`plan` 是
        // `NotFound`/`Ambiguous`，小结念的是 `plan.reason()` 而不是余额。
        return Ok(Judge {
            prize: prize.unwrap_or_default(),
            balance: 0,
            plan,
        });
    }
    // 定位放行 ⇒ `locate_exchange_prize` 的契约保证 prize 是 `Some`。
    let prize =
        prize.ok_or_else(|| Error::Other("内部不变式破了：定位放行却没有拿到商品".into()))?;
    let status = read_status(env).await?;
    let plan = plan_exchange(&prize, Some(status.total), now_ms);
    Ok(Judge {
        prize,
        balance: status.total,
        plan,
    })
}

/// 读兑换清单（**只读**）。
///
/// 公开是为了让只读的 HTTP 路由（选品器那一类）复用**同一次**读取 ——
/// 那边没有 `mcloud-upstream` 依赖，而把这段抄一份过去，两份的差别
/// 会是"任务看得见的清单"与"页面上看得见的清单"不是同一份。
pub async fn read_exchange_list(env: &ExchangeEnv<'_>) -> Result<ExchangeList> {
    let resp = env.http.request(exchange::exchange_list_request()).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "读取兑换清单")?;
    let parsed = SignInEnvelope::parse(body)?;
    exchange::parse_exchange_list(&parsed)
}

/// **只读**：读一次组字典（`prizeGroup.json`：组名 + 该组的限兑规则）。
///
/// `market_name` 取清单里的 `marketName`（同一份清单里各组同值，实拍 `sign_in_3_ex`）。
///
/// # 为什么返回"可选"，而不是直接报错
///
/// 组名是**界面文案**，不是判据 —— 拿不到只是分区标题难看，不该影响清单本身。
/// 三种结果刻意分成三档，别混：
///
/// | 结果 | 含义 |
/// |---|---|
/// | `Ok(None)` | 从 `marketName` **推不出**目录名 ⇒ **连请求都没发**（"上游没有"与"我们没问"要分得清）|
/// | `Ok(Some(_))` | 拿到了 |
/// | `Err(_)` | 问了但没成（⚠ 实拍有"HTTP 200 + 0 字节"这一档，见 [`exchange::parse_prize_groups`]）|
pub async fn read_prize_groups(
    env: &ExchangeEnv<'_>,
    market_name: &str,
) -> Result<Option<Vec<exchange::ExchangeGroup>>> {
    let Some(req) = exchange::prize_group_request(market_name) else {
        return Ok(None);
    };
    let resp = env.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "读取奖品组字典")?;
    Ok(Some(exchange::parse_prize_groups(body)?))
}

/// 清单里的一件 + 它当下的判定档位。
///
/// 只读选品器（HTTP 那一条）要的正是这一对，所以类型在这里定：
/// 档位与商品必须是**同一次读取**的产物，分成两个 `Vec` 返回的话，
/// 两边的顺序一旦不同，界面上"这件可兑"就会配到**另一件**的价格上。
#[derive(Debug, Clone)]
pub struct JudgedPrize {
    pub prize: ExchangePrize,
    pub plan: ExchangePlan,
}

/// **只读**：读一次清单，并逐件判"现在能不能兑"。**不碰余额、不碰滑块、不下单。**
///
/// # `balance = None` 这一档是刻意的（代价写在下面）
///
/// 与参考实现给选品器的那一档一致：**没有余额读数** ⇒ 判不出
/// [`ExchangePlan::InsufficientCloud`]。所以这里标成"可兑"的商品，
/// 用户点了仍可能被链挡下 —— 那一次挡下**有错误码**（`Skipped` + `plan`），
/// 不会静默不发。
///
/// 为什么不顺手查一次余额：`infoV3` 是**账号级**请求，多打一枪就多一分
/// 风控噪声，而"看一眼清单"的收益不足以付它。
///
/// # 时间窗用**调用方给的** `now_ms`
///
/// 由 `env.clock` 取也一样，但显式传参让"这一份档位是哪个时刻判的"
/// 可以被回给前端 —— `NotStarted` / `Ended` 是相对时刻的结论，
/// 页面显示"还没开售"却不说是什么时候判的，用户没法判断该不该刷新。
pub async fn read_judged_prizes(env: &ExchangeEnv<'_>, now_ms: i64) -> Result<Vec<JudgedPrize>> {
    let list = read_exchange_list(env).await?;
    // 顺序由解析层保证（按 groupId **数值**升序）—— 这里不再排一遍。
    Ok(list
        .all()
        .into_iter()
        .map(|prize| JudgedPrize {
            plan: plan_exchange(prize, None, now_ms),
            prize: prize.clone(),
        })
        .collect())
}

/// 读账号状态（余额的**唯一**来源）。
async fn read_status(env: &ExchangeEnv<'_>) -> Result<SignInStatus> {
    let resp = env.http.request(app_signin::info_v3_request(None)).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "查询签到状态")?;
    let parsed = SignInEnvelope::parse(body)?;
    app_signin::parse_status(&parsed)
}

/// 取一次滑块题面（**只读**，但滑块接口计失败次数）。
async fn fetch_puzzle(env: &ExchangeEnv<'_>) -> Result<SlidePuzzle> {
    let resp = env.http.request(exchange::get_slide_request()).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "取滑块题面")?;
    let parsed = SignInEnvelope::parse(body)?;
    exchange::parse_slide_puzzle(&parsed)
}

/// 发一枪 `exchangeV3`（**真扣豆**）。
async fn submit_exchange(
    env: &ExchangeEnv<'_>,
    device_id: &str,
    prize_id: i64,
    offset: i64,
) -> Result<V3Result> {
    let resp = env
        .http
        .request(exchange::exchange_v3_request(
            device_id,
            prize_id,
            Some(offset),
        ))
        .await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "兑换下单")?;
    let parsed = SignInEnvelope::parse(body)?;
    exchange::parse_exchange_result(&parsed)
}

/// 把"判据没过"折成结论（一枪都没发）。
///
/// ⚠ 只覆盖 `status` / `plan` / `prize` / `balance` 四样，其余读数
/// （`rounds`、`waited_until_ms`、`error`…）原样留着 —— 等过整点再被挡住
/// 的那一趟，"等到了什么时候"是用户要看的。
fn skipped(judge: &Judge, outcome: ExchangeOutcome) -> ExchangeOutcome {
    ExchangeOutcome {
        status: ExchangeStatus::Skipped,
        plan: judge.plan,
        prize: judge.prize.clone(),
        balance: judge.balance,
        ..outcome
    }
}

/// 一次兑换的输入。
pub struct ExchangeOptions<'a> {
    /// 参数 `prizeIdOrName`：纯数字按 prizeId 找，其余按商品名**精确**找。
    pub prize: &'a str,
    /// `false` ⇒ 若当前时刻离整点还远就先等到整点（[`plan_exchange_wait`]）。
    ///
    /// ⚠ "整点"是参考实现抢兑那一场的时序取向，**不是**上游的硬条件 ——
    /// 我们自己的成功兑换发生在 `12:16`。默认 `true`。
    pub skip_wait: bool,
    /// 换 `deviceId` 的注入点；`None` ⇒ **一枪不发**（见 [`ExchangeEnv`]）。
    pub device_id: Option<&'a DeviceIdFn<'a>>,
    /// "等到某个时刻"的注入点；`None` ⇒ `skip_wait == false` 时**如实挡下**。
    pub wait: Option<&'a WaitFn<'a>>,
}

/// 跑一次兑换：**最多**下一单（真扣豆）。
///
/// # 错误与结论的分界
///
/// 只有"**没走到能报结论的那一步**"才回 `Err`：取票之后的只读查询失败
/// （清单、余额、第一次取题）、入参本身不成立。凡已经判过 / 已经发过单的，
/// 一律折成 [`ExchangeOutcome`] 的档位 —— 否则"这件本月已兑过"会在历史里
/// 变成一条红色的 errored，而它是一句正常答复。
///
/// # 一枪都不发的三条闸
///
/// 判据没过、换不到 `deviceId`、offset 算不出来。前两条各自有原因字段
/// （`plan` 与 `device_id_failed`），第三条记在 [`ExchangeOutcome::error`] 里。
///
/// # 重试口径（写操作零自动重试）
///
/// 只有 `514`（滑块算错）可以再来一轮，且**上限 [`SLIDER_ROUNDS`] 轮**：
/// 那三条实拍期间余额始终没动，代价为零。其余一律不重发 ——
/// 特别是传输层错误：**发出去超时的这一笔可能已经成交**，
/// 重发的表现是同一件扣两次豆。
///
/// # 枪序（与实拍对得上）
///
/// ```text
/// 1  GET  exchangeList   清单：limit 与三个余量、价格（只读）
/// 2  GET  infoV3         余额（判"够不够"，也是成交后的对照基准）
/// 3  ← 换 deviceId（注入点；这一步会**上报设备指纹**）
/// 4  POST getSlide       取滑块题面（每次下单前现取）
/// 5  POST exchangeV3     下单
/// 6  GET  infoV3         复查余额（判据链第 2 条）
/// ```
///
/// 实拍四条 `exchangeV3` 每一条前面都紧跟一条 `getSlide`，所以 4、5 是
/// **成对**的：`514` 重来 = 再取一题 + 再下一单，而不是拿旧 offset 重发。
pub async fn exchange(
    env: &ExchangeEnv<'_>,
    now_ms: i64,
    opts: &ExchangeOptions<'_>,
) -> Result<ExchangeOutcome> {
    let query = opts.prize.trim();
    if query.is_empty() {
        return Err(Error::Config("兑换没给奖品：prizeIdOrName 是必填".into()));
    }

    let mut judge = judge_exchange(env, query, now_ms).await?;
    let mut outcome = ExchangeOutcome {
        plan: judge.plan,
        prize: judge.prize.clone(),
        balance: judge.balance,
        ..ExchangeOutcome::default()
    };
    if judge.plan.blocks() {
        return Ok(skipped(&judge, outcome));
    }

    if !opts.skip_wait {
        let wait = plan_exchange_wait(now_ms);
        if wait.needed {
            outcome.waited_until_ms = Some(wait.until_ms);
            // ⚠ 没接上等待实现 ⇒ **如实挡下**，绝不"静默不等待"往下走：
            // 那会让 `skip_wait == false` 这个开关看起来生效、实际没等。
            let Some(sleep) = opts.wait else {
                return Ok(set_error(
                    skipped(&judge, outcome),
                    "skipWait=false 但这一趟没接上等待实现（不静默跳过等待，本次没发请求）".into(),
                ));
            };
            if let Err(e) = sleep(wait.until_ms) {
                return Ok(set_error(skipped(&judge, outcome), e.to_string()));
            }
            // 等到点之后**重读重判**：清单与余额都是时刻量，等完这一小时里
            // 别人可能已经把池子兑空、而上游只有一句 `415` 等着。
            //
            // ⚠ 重判用**重判时**的时钟，不是入参那个 `now_ms`：等待可能被
            // 缩短（取消）或跨过目标时刻，拿旧时刻判时间窗等于没重判。
            let reopened = env.clock.now_ms();
            judge = judge_exchange(env, query, reopened).await?;
            outcome.plan = judge.plan;
            outcome.prize = judge.prize.clone();
            outcome.balance = judge.balance;
            if judge.plan.blocks() {
                return Ok(skipped(&judge, outcome));
            }
        }
    }

    // 下单体里那个 `deviceId` 键必须有值：实拍四条每一条都带了非空值。
    // ⚠ "少了它会回什么码"**没有样本** ⇒ 不猜，直接不发。
    let Some(device_id_fn) = opts.device_id else {
        outcome.device_id_failed = "这一趟没接上 deviceId 来源（下单硬需要它，一枪都没发）".into();
        return Ok(skipped(&judge, outcome));
    };
    let device_id = match device_id_fn() {
        Ok(id) => id,
        Err(e) => {
            outcome.device_id_failed = e.to_string();
            return Ok(skipped(&judge, outcome));
        }
    };
    if device_id.trim().is_empty() {
        outcome.device_id_failed = "换到的 deviceId 是空串（不发，避免打到未定义的那一档）".into();
        return Ok(skipped(&judge, outcome));
    }

    let mut round = 1u32;
    while round <= SLIDER_ROUNDS {
        let puzzle = match fetch_puzzle(env).await {
            Ok(puzzle) => puzzle,
            Err(e) => {
                if round == 1 {
                    // 一枪未发，且连题都没有 ⇒ 这一趟确实什么都没办成，交回错误。
                    outcome.status = ExchangeStatus::NotAttempted;
                    outcome.error = e.to_string();
                    return Err(e);
                }
                // 第二轮取题失败**不**推翻第一轮的 `514`：那一枪确实被上游判错了，
                // 而重试的机会已经用完。落回"滑块没过"（循环尾那一档）。
                break;
            }
        };
        outcome.rounds = round;

        let solution = match mcloud_crypto::solve_slide(&puzzle.picture_png, &puzzle.puzzle_png) {
            Ok(solution) => solution,
            Err(e) => {
                // 算不出来就**不发**：拿 `0` 或上一次的答案去凑，是拿真豆去赌
                // （旧实现取题失败时用 `0` 兜底照常下单，那条路径我们不照搬 ——
                // `0` 会回什么**未验证**）。
                outcome.status = ExchangeStatus::SliderUnresolved;
                outcome.error = if round > 1 {
                    format!("重取题后算不出 offset：{e}")
                } else {
                    e.to_string()
                };
                return Ok(outcome);
            }
        };
        let offset = solution.offset;
        outcome.offset = Some(offset);

        let submitted = submit_exchange(env, &device_id, judge.prize.prize_id, offset).await;
        outcome.attempts += 1;
        let result = match submitted {
            Ok(result) => result,
            Err(e) => {
                // ⚠ 这一枪**可能已经成交**：请求发出去了，只是我们没读到响应。
                // 不重发（写操作零自动重试），并报成"结果未知"。
                outcome.status = ExchangeStatus::Unknown;
                outcome.error = e.to_string();
                return Ok(outcome);
            }
        };
        outcome.code = result.code;
        outcome.message = result.message.clone();

        if result.code == CODE_OK {
            outcome.order = result.order.clone();
            return Ok(settle_ordered(env, &judge, outcome).await);
        }
        if result.code == CODE_SLIDER_FAILED {
            // 代价为零（三条 `514` 期间余额始终没动）⇒ 允许再来一轮。
            if round == SLIDER_ROUNDS {
                outcome.status = ExchangeStatus::SliderUnresolved;
                return Ok(outcome);
            }
            round += 1;
            continue;
        }
        if result.code == CODE_DUPLICATED {
            outcome.status = ExchangeStatus::AlreadyExchanged;
            return Ok(outcome);
        }
        if result.code == CODE_NOT_TARGET_USER {
            outcome.status = ExchangeStatus::NotTargetUser;
            return Ok(outcome);
        }
        // 其余一切（`415`/`420`/`421`/`501`/`610`/`614`/`615`…）都是明确失败。
        outcome.status = ExchangeStatus::Failed;
        return Ok(outcome);
    }

    // 走到这里只有一条路：第二轮取题失败（`break`），而第一轮那一枪确实被
    // 上游判成 `514` 了。⇒ 与"两轮都判错"同一档。
    outcome.status = ExchangeStatus::SliderUnresolved;
    Ok(outcome)
}

/// `code == 0` 之后**必须回查余额**才算成交。
///
/// # 为什么不能只看 `code`
///
/// 抓包把判据钉成了三条（`code 0` + 两次 `infoV3.total` 差 == 商品价格 +
/// 账目/待领取），而旧实现只看 `code === 0 && result.prizeName` ——
/// 那种写法在"上游说成功却没扣豆"这种情形下会**报出一条绿的假成功**。
///
/// 两处例外要一起说清：
///
/// - **余额复查那一枪失败** ⇒ 仍报成交（`code 0` + `oid` 就是证据），
///   但把"没核到"留在 [`ExchangeOutcome::error`] 里（小结会念出来）；
/// - **上游没给价格**（正常到不了这里，[`ExchangePlan::PriceMissing`] 已经挡过）
///   ⇒ 没有对照基准，按"没核到"处理：报成交并留一句原因。
///   刻意**不**回退成"拿 `uorder` 当价格"—— 那个字段是"上游说扣了多少"，
///   不是"该扣多少"，用它当基准等于拿上游的话去验证上游的话。
async fn settle_ordered(
    env: &ExchangeEnv<'_>,
    judge: &Judge,
    mut outcome: ExchangeOutcome,
) -> ExchangeOutcome {
    let after = match read_status(env).await {
        Ok(status) => status,
        Err(e) => {
            outcome.status = ExchangeStatus::Ordered;
            outcome.error = format!("余额复查失败：{e}");
            return outcome;
        }
    };
    let balance_after = after.total;
    outcome.after_balance = Some(balance_after);

    let Some(price) = judge.prize.price else {
        outcome.status = ExchangeStatus::Ordered;
        outcome.error = "这一件没有价格读数，余额差核不了（只满足了「上游回成功」这一条）".into();
        return outcome;
    };
    if balance_after == outcome.balance - price {
        outcome.status = ExchangeStatus::Ordered;
        return outcome;
    }
    // 差值对不上：**不能**默认成功。要么上游异步扣豆还没落到这个读数上，
    // 要么这一笔根本没成交（而它回了 0）。两种处置都得人去核对。
    outcome.status = ExchangeStatus::Unknown;
    outcome
}

/// 只在"没走到发单"那几条早退路上补一句原因，不改档位。
fn set_error(outcome: ExchangeOutcome, message: String) -> ExchangeOutcome {
    ExchangeOutcome {
        error: message,
        ..outcome
    }
}

/// 优先用上游那句原文，没有时退回我们自己的说法。
///
/// ⚠ 不把空 `msg` 渲染成「」—— 界面上那一对空引号会被读成"上游说了句空话"。
fn or_upstream(message: &str, fallback: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// 把本地失败原因拼成"（…）"，空原因就不留括号。
fn tail(reason: &str) -> String {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("（{trimmed}）")
    }
}

// ══════════════════════════════ 一个账号那一趟 ══════════════════════════════

/// 一个账号这一趟的实况。
#[derive(Debug, Clone)]
pub struct ExchangeResult {
    /// 账号 id（**脱敏形态**，见 `AccountId` 的渲染）。
    pub account: String,
    /// 结算结论。
    pub outcome: ExchangeOutcome,
}

impl ExchangeResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// ⚠ 里面只有档位、商品名、价格与余额这些**账号内**的数，没有任何凭据形态
    /// 的东西。上游那句 `msg` 会被带出去（它是上游写给人看的文案），
    /// 但它不含凭据。
    pub fn summary_line(&self) -> String {
        let outcome = &self.outcome;
        let name = outcome
            .prize
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("未知商品")
            .to_string();
        let price = match outcome.prize.price {
            Some(price) => format!("，价 {price} 豆"),
            None => String::new(),
        };

        match outcome.status {
            ExchangeStatus::Ordered => {
                let line = format!("兑到「{name}」{price}");
                if let Some(spent) = outcome.spent() {
                    let after = outcome.after_balance.unwrap_or_default();
                    return format!("{line}；余额 {} → {after}（差 {spent}）", outcome.balance);
                }
                if outcome.error.is_empty() {
                    format!("{line}；⚠ 余额差没核到")
                } else {
                    format!("{line}；⚠ 余额差没核到：{}", outcome.error)
                }
            }
            ExchangeStatus::AlreadyExchanged => {
                format!("「{name}」此前已兑过（上游回「重复兑奖」），本次没有重复发奖")
            }
            ExchangeStatus::Skipped => {
                if outcome.device_id_failed.is_empty() {
                    format!("没兑换：{}", outcome.plan.reason())
                } else {
                    format!("没兑换：{}", outcome.device_id_failed)
                }
            }
            ExchangeStatus::SliderUnresolved => {
                if outcome.attempts == 0 {
                    format!("没兑换：滑块解算没过（一枪都没发）{}", tail(&outcome.error))
                } else {
                    format!(
                        "没兑到：滑块验证 {} 次都被上游判错{}",
                        outcome.attempts,
                        tail(&outcome.error)
                    )
                }
            }
            ExchangeStatus::NotTargetUser => format!(
                "没兑到：上游回「{}」—— 这一族是账号级闸门，这个账号兑不了兑换页的奖品",
                or_upstream(&outcome.message, "非移动用户不可领奖")
            ),
            ExchangeStatus::Unknown => {
                let mut line = "兑换结果判不了".to_string();
                if outcome.code != 0 {
                    line.push_str(&format!("（上游回 {}）", outcome.code));
                }
                if let (Some(spent), Some(price)) = (outcome.spent(), outcome.prize.price) {
                    line.push_str(&format!("：余额差 {spent} 与价格 {price} 对不上"));
                } else if !outcome.error.is_empty() {
                    line.push_str(&format!("：{}", outcome.error));
                }
                line.push_str("，需要去 App 核对余额与待领取奖品");
                line
            }
            ExchangeStatus::Failed => format!(
                "兑换失败：上游回 {}「{}」",
                outcome.code,
                or_upstream(&outcome.message, "没给原因")
            ),
            ExchangeStatus::NotAttempted => format!("兑换没判成{}", tail(&outcome.error)),
        }
    }

    /// 把档位映射到 [`TaskOutcome`]。
    ///
    /// 分流只有一条判据：**用户要的那一件到底兑到没有**。
    ///
    /// - 兑到了（含上游判为"重复兑奖" = 本来就有了）⇒ **成功**；
    /// - 判据没过、且没过的是**本账号已经兑过**（`limit` 2 / 4）⇒ 也算**成功** ——
    ///   那一句"本月已兑"说的就是目标已达成；
    /// - 其余"判据没过"（查无此件 / 余额不够 / 库存没了 / 时间不对 / 判不了）
    ///   ⇒ **[`TaskOutcome::Skipped`]**：目标没达成，但也**没有**一件
    ///   "必须人去做、程序代劳不了"的事。
    ///   ⚠ 参考实现把它记为"部分成功"（`OutcomePartialSuccess`），而本仓
    ///   [`TaskOutcome`] 没有那一档 —— `Skipped` 正是本仓"条件不成立、
    ///   不是失败"的表达（见 `TaskOutcome` 自己的文档）。
    /// - 滑块没过 / 结果判不了 ⇒ **失败**，并且小结里点了名要人去 App 里
    ///   点那一下（手动滑一次、或核对这一笔到底成交没有）；
    /// - 上游明确回失败、以及一枪都没发就取不到题 ⇒ **失败**。
    ///
    /// # 关于参考实现里的 `ManualRequired`（**已知的结构差**）
    ///
    /// 参考实现的 `RunReport` 有一条 `OutcomeManualRequired` +
    /// `core.NewRunReportManualRequired`，用来在界面上挂一个**具体动作**
    /// （滑块没过 ⇒ "到 App 的签到页手动兑一次这件"；结果判不了 ⇒
    /// "到 App 核对余额与待领取奖品"）。本仓的 [`TaskOutcome`] 还没有这一档。
    ///
    /// 处置：把那两句"要人去做"的说明**逐字放进小结**
    /// （见 [`Self::summary_line`] 的 `SliderUnresolved` 与 `Unknown` 两支），
    /// 档位落到 [`TaskOutcome::Failed`]。⚠ 这是**已知的结构差**，不是等价物：
    /// `Failed` 会触发告警，而 `ManualRequired` 在参考实现里是"要人去做"、
    /// 未必报红。等 `TaskOutcome` 补上那一档，这两支应当改挂它，
    /// 小结里那两句就是现成的 `instruction`。
    pub fn run_report(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.outcome.status {
            ExchangeStatus::Ordered | ExchangeStatus::AlreadyExchanged => {
                TaskOutcome::success(summary)
            }
            ExchangeStatus::Skipped => {
                if matches!(
                    self.outcome.plan,
                    ExchangePlan::ExchangedThisPrize | ExchangePlan::ExchangedThisGroup
                ) {
                    TaskOutcome::success(summary)
                } else {
                    TaskOutcome::skipped(summary)
                }
            }
            _ => TaskOutcome::failed(summary),
        }
    }
}

/// 跑一个账号这一趟的输入。
pub struct ExchangeRun<'a> {
    /// 参数 `prizeIdOrName`（必填）。
    pub prize: &'a str,
    /// 与参数面同名：`false` ⇒ 先等到整点再判一次。
    pub skip_wait: bool,
    /// 换 `deviceId` 的注入点。
    pub device_id: Option<&'a DeviceIdFn<'a>>,
    /// 等待实现。
    pub wait: Option<&'a WaitFn<'a>>,
}

impl ExchangeRun<'_> {
    /// 跑**一个账号**的这一笔。
    ///
    /// ⚠ 下单失败**不**回错误（折成档位），否则"这件本月已兑过"会在历史里
    /// 多一条红色；只有"没走到能报结论的那一步"（取票 / 清单 / 余额 /
    /// 第一次取题）才交回错误。
    pub async fn run_account(&self, env: &ExchangeEnv<'_>, now_ms: i64) -> Result<ExchangeResult> {
        let opts = ExchangeOptions {
            prize: self.prize,
            skip_wait: self.skip_wait,
            device_id: self.device_id,
            wait: self.wait,
        };
        let outcome = exchange(env, now_ms, &opts).await?;
        Ok(ExchangeResult {
            account: String::new(),
            outcome,
        })
    }
}

// ══════════════════════════════ 任务壳 ══════════════════════════════

/// `exchange`：兑换页抢兑（**真扣AI豆**）。
///
/// # 参数
///
/// | 参数 | 必填 | 说明 |
/// |---|---|---|
/// | `prizeIdOrName` | ✅ | 纯数字按 prizeId，其余按商品名**精确**找 |
/// | `skipWait` | | `false` ⇒ 先等到下一个整点再判一次（默认 `true`） |
///
/// # ⚠ `deviceId` 这条缺口必须说清（本任务目前**不能**单独跑通）
///
/// 生产路径上 `deviceId` 要由"上报设备指纹换 id"那一套产生
/// （算法在 `mcloud_upstream::smid`，编排在 `mcloud_app::smid`，
/// 它需要指纹存储 + 随机源）。而 `mcloud-tasks` 依赖不到 `mcloud-app`
/// （那是反向依赖）—— 所以这个任务壳**没有**注入点，链会走到
/// [`ExchangeStatus::Skipped`] 并给出 `device_id_failed`：
///
/// > 这一趟没接上 deviceId 来源（下单硬需要它，一枪都没发）
///
/// 这是**刻意的**：一条"随手生一个 id"的兜底会让上游不认那个 id、
/// 白扣一次机会；而"静默不发"至少是诚实的。
///
/// 接线方式（留给组装层）：把 `DeviceIdFn` 从 `mcloud_app` 那层注入进来
/// （`mcloud_app` 已经依赖 `mcloud-tasks`，方向是对的）。
///
/// # 当前**不**注册进 `register_all`（⇒ `/api/capabilities` 里也没有它）
///
/// 未注册：缺 `device_id` 来源 + 缺 `visitlog` 预热 + 等待档与 TS 不一致，
/// 补齐后才应放出来。三条缺口分别是：
///
/// 1. **缺 `device_id` 来源**（上面那段）：注不进它就永远走 `Skipped`，
///    用户点了什么都不会发生。接线要等 `mcloud_app` 那层把
///    "上报设备指纹换 id"做成 `DeviceIdFn` 注进来。
/// 2. **缺 `reportVisitLog` 预热两枪**：参考实现
///    （`exchange/task.ts:84-100`、`utils.ts:7`）在下单前先上报访问日志；
///    本仓没接。那两枪的判据在手头样本里不足 —— 不猜（`AGENTS.md` 的
///    依据优先级：真写不了的分支留空并注明原因）。
/// 3. **等待档与 TS 不一致**：参考实现默认 `skipWait=false` 且等到
///    `hh:59:40`；本仓默认 `skipWait=true` 只等到整点
///    （见 `plan_exchange_wait` 与 `ExchangeTask::run` 里那句
///    `unwrap_or(true)`）。
///
/// 在这三条补齐之前，本任务**可以被直接调用**（判定层与编排层能跑、能测，
/// 缺 `deviceId` 时如实回 `device_id_failed`），但**不**应对外宣称可用：
/// 一个"点了永远什么都不会发生"的入口比没有入口更坏。
/// `lib.rs` 的 `register_all` 里登记了这件事，
/// 哨兵是 `tests::register_all_does_not_expose_exchange`。
pub struct ExchangeTask;

#[async_trait]
impl Task for ExchangeTask {
    fn name(&self) -> &'static str {
        "exchange"
    }

    fn title(&self) -> &'static str {
        "兑换奖品"
    }

    fn description(&self) -> &'static str {
        "在兑换页兑换指定奖品（真扣AI豆；下单前逐档判据放行，成交后回查余额）"
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::text("prizeIdOrName", "奖品", "奖品 id 或名字；下单会真扣AI豆"),
            ParamSpec::boolean("skipWait", "跳过等待", true, "不等冷却/档位窗口，直接下单"),
        ]
    }

    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        let prize = match body.get("prizeIdOrName").and_then(Value::as_str) {
            Some(prize) if !prize.trim().is_empty() => prize.to_string(),
            _ => {
                return Err(Error::Config(
                    "exchange 需要 prizeIdOrName：纯数字按 prizeId，其余按商品名精确找".into(),
                ))
            }
        };
        let skip_wait = body
            .get("skipWait")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let env = ExchangeEnv {
            http: ctx.http.as_ref(),
            clock: ctx.clock.as_ref(),
            // 见 [`ExchangeTask`] 那段缺口说明：这里刻意**不**编一个 deviceId。
            device_id: None,
        };
        ctx.check_cancelled()?;
        // ⚠ 这里**不用 `?`**：`Ok` 与 `Err` 两种都要自报结构化明细。
        //
        // 原先这里是 `...run_account(...).await?`，那样"没走到能报结论的那一步"
        // （取票失败、清单读不出来、余额查询挂了）会**直接上抛**，于是
        // HTTP 侧与运行历史里都只剩下一句话 —— 而"是哪一枪断的"这件事
        // 在 `ExchangeOutcome.error` 里本来就有。花真豆的那条链，
        // 事后必须答得出"走到哪一步、发没发单、花没花"。
        let ran = ExchangeRun {
            prize: &prize,
            skip_wait,
            device_id: None,
            wait: None,
        }
        .run_account(&env, ctx.clock.now_ms())
        .await;

        match ran {
            Ok(result) => {
                ctx.info(result.summary_line());
                ctx.details(detail_payload(&result.outcome));
                Ok(result.run_report())
            }
            Err(e) => {
                // 一枪未发就断在前面：如实报出来，**不**编一份 outcome ——
                // 编一份空的正好是这一族最危险的误报：`ExchangePlan` 的零值是
                // `Go`（"可以下单"）、`ExchangeStatus` 的零值是 `NotAttempted`，
                // 一份漏填 outcome 读起来就是"这一件可以下单"。
                // 见 `ExchangePlan::Go` 那段与 `status_zero_value_is_not_attempted`。
                ctx.warn(format!("兑换没跑起来：{e}"));
                ctx.details(serde_json::json!({
                    "kind": "exchange",
                    "reached": false,
                    "error": e.to_string(),
                }));
                Err(e)
            }
        }
    }
}

/// 把一次兑换的读数折成**给程序读**的形状（进 `RunRecord.details["reported"]`）。
///
/// # 为什么在这里序列化，而不是让路由自己拼
///
/// HTTP 路由、运行历史、将来的推送都要看同一批字段。三处各拼一份的话，
/// 加一个字段就得改三处，而**漏改一处不报错** —— 表现是"某个页面少显示一栏"。
///
/// # 键名与 `mcloud-server` 路由的响应体**对齐**
///
/// 路由那条 `ExchangeRunResponse` 是从这一份里读回来的，所以两边是同一套
/// `snake_case` 键名。改这里必须同时改那边。
pub fn detail_payload(outcome: &ExchangeOutcome) -> serde_json::Value {
    serde_json::json!({
        "kind": "exchange",
        "reached": true,
        "status": status_slug(outcome.status),
        "plan": outcome.plan.slug(),
        "plan_reason": outcome.plan.reason(),
        "code": outcome.code,
        "message": outcome.message,
        "rounds": outcome.rounds,
        "attempts": outcome.attempts,
        "offset": outcome.offset,
        "balance": outcome.balance,
        "after_balance": outcome.after_balance,
        // `skipWait=false` 那条路上，"等到了什么时候"是用户要看的。
        "waited_until_ms": outcome.waited_until_ms,
        "device_id_failed": outcome.device_id_failed,
        "error": outcome.error,
        "prize": {
            "prize_id": outcome.prize.prize_id,
            "name": outcome.prize.name,
            "price": outcome.prize.price,
        },
        "order": outcome.order.as_ref().map(|o| serde_json::json!({
            "order_id": o.order_id,
            "prize_id": o.prize_id,
            "prize_name": o.prize_name,
            "cost": o.cost,
            "insert_time": o.insert_time,
            "expire_time": o.expire_time,
        })),
    })
}

/// 结局档位的机器可读名。
///
/// ⚠ 逐档点名而不是 `{:?}` 或序号：前者把 Rust 变体名变成对外契约、
/// 后者把**声明顺序**变成对外契约（加一档就让界面那张颜色表静默错配）。
/// `match` 是穷尽的 —— 上游加档时这里**编译不过**，那正是我们要的响法。
///
/// ⚠ 它只此一份：`mcloud-server` 的兑换路由**引用**这里，不另抄一份。
/// 两处各写一份的表现是"历史里说 ordered、页面上说 failed"。
pub fn status_slug(status: ExchangeStatus) -> &'static str {
    match status {
        ExchangeStatus::NotAttempted => "not_attempted",
        ExchangeStatus::Ordered => "ordered",
        ExchangeStatus::Skipped => "skipped",
        ExchangeStatus::AlreadyExchanged => "already_exchanged",
        ExchangeStatus::SliderUnresolved => "slider_unresolved",
        ExchangeStatus::NotTargetUser => "not_target_user",
        ExchangeStatus::Unknown => "unknown",
        ExchangeStatus::Failed => "failed",
    }
}

/// 在**立即**返回 `Ok(())` 之前不做任何等待的等待实现。
///
/// 只给测试与"我就是要现在试试"那一支用。生产路径上要等到整点，
/// 得注入一个真的 sleep（见 [`wait_until`]）——
/// ⚠ 链**不会**在 `wait` 为 `None` 时静默跳过等待（见 [`exchange`]）。
pub fn no_wait() -> impl Fn(i64) -> Result<()> {
    |_until_ms| Ok(())
}

#[cfg(test)]
mod tests;
