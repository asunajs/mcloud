//! 趣玩 AI 抽奖（`fun-ai`）：读判据 ⇒ 判 ⇒ **最多抽一次**。
//!
//! 请求构造与解析在 [`mcloud_upstream::app_funai`]（纯函数、无 IO）；
//! 本模块只负责**编排**：读判据、判门槛、说结论。
//!
//! # 链只有两枪
//!
//! ```text
//! GET  /ycloud/funai/api/page/getmodule   读 drawTimes / drawsUsed / 活动起止
//! POST /ycloud/funai/api/page/lottery     body 只有 {"moduleId":"…"}，值取自上一枪
//! ```
//!
//! # 为什么门槛判在这里，而不是在能力函数里写个 `if`
//!
//! 抽奖是**写请求**且每天只有一次额度：判错的两种代价**不对称** ——
//! 该不发时发了会**用掉**这一次（重跑也回不来），该发时不发只是少一次。
//! 判定因此做成纯函数 [`plan_funai`] 单独测。
//!
//! # `jwttoken` 走**公共来源**（临时态）
//!
//! 实拍归档里**没有**换出该页 jwttoken 的那一步，所以该页的特有取法无从得知
//! ⇒ 按退路走签到页那条配方。抓到特有取法时**主动更正**这一处调用点并改掉这段说明。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_funai as proto;
use mcloud_upstream::app_funai::FunaiModule;
use mcloud_upstream::app_signin::SignInEnvelope;

/// 发一个请求并解成信封。
async fn call(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<SignInEnvelope> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    SignInEnvelope::parse(body)
}

/// 只读判据、不抽（给"看一眼"那种用法留着）。
pub async fn query_module(ctx: &TaskContext) -> Result<FunaiModule> {
    let env = call(ctx, proto::funai_module_request(None), "读取趣玩AI活动信息").await?;
    proto::parse_funai_module(&env)
}

// ══════════════════════════ 门槛判定（纯函数） ══════════════════════════

/// **本工具**每天最多抽几次 = 1。
///
/// # 这个 `1` 是从哪来的（三种数最容易混成一个）
///
/// * **上游规则文本说 5**：那句「每个用户每日最多可通过**完成任务**获得 5 次抽奖机会」
///   说的是**做任务能挣到几次**，不是"每天最多能抽几次"。⚠ 别拿它当本常量的出处。
/// * **实拍的两个计数是 `1/0`**：那是**当期这一个模块**给出来的数，
///   不是"上限"这个规则本身 —— 一份样本定不了它跟规则的关系。
/// * **本常量 `1`**：与旧 TypeScript 实现一致的取向（那边就是"只抽一次，
///   不看剩余次数循环"）⇒ 这是**刻意**比上游可能允许的更少，
///   理由是不拿写请求去试探额度。
///
/// ⚠ "上游会不会因为一天只抽一次而少给什么"没有样本，**未测试**。
pub const FUNAI_DAILY_DRAW_LIMIT: i64 = 1;

/// 活动期**三态**。
///
/// 必须有"判不了"这一格：两值布尔会把"上游没给时间"落成"不在期"或"在期"
/// 中的一个，而这两种处置都不对（见 [`FunaiPlan::WindowUnknown`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunaiWindow {
    /// 落在 `start_time`~`end_time` 之内（含端点）。
    In,
    /// 明确在区间之外。
    Out,
    /// 一个边界都判不出来。
    Unknown,
}

/// [`plan_funai`] 的结论档位。
///
/// 每一档都必须说得出**为什么不是别的一档**：这六个值最后都会变成界面上一句话，
/// 而"跳过"这句话没有用的（用户会问"那我明天来还是后天来"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunaiPlan {
    /// **可以抽**：活动期内、且次数判据显示还有额度。
    Draw,
    /// **今天已经抽过** —— `draws_used` 已经达到 [`FUNAI_DAILY_DRAW_LIMIT`]。
    ///
    /// 它排在"没次数"之前：两个条件同时成立时（实拍那一档抽完后就是 `1/1`），
    /// "今天已经抽过"是更有用的那句 —— 它同时回答了"明天再来有没有"。
    AlreadyDrawn,
    /// **没有次数**：`draw_times - draws_used <= 0`，但今天这一次额度还没用掉。
    ///
    /// ⚠ 这一档的**式子**在语义上多扣了一层：`draw_times` 收口后**本身就是余额**
    /// （恒等式 `todayGet = drawTimes + drawsUsed`），再减一次 `draws_used` 是重复扣。
    /// 它**当前等价**于 `draw_times <= 0` —— 能走到这条判据说明上面那条没命中，
    /// 即 `draws_used < 1`，而它是整数 ⇒ 只能 `= 0` ⇒ 减数恒为 `0`。
    ///
    /// ⚠ **不顺手改成 `draw_times <= 0`**：那一句实际含义是"抬高每日上限"，
    /// 而"一天多抽几次上游会不会少给"零样本，且恒等式只从**同日**读数收口
    /// （跨天是否清零未测试）。要改先拿样本。
    NoChances,
    /// 上游**没给判据字段**（`draw_times` 或 `draws_used` 缺 / 是 `null`）⇒ 判不了。
    ///
    /// ⚠ 单独成一档，且**不发请求**：当成"有次数"会白发一次写请求把今天的额度用掉，
    /// 当成"没次数"会漏抽。实拍里 `timed/getmodule` 那一份三个计数**全是 `null`**
    /// —— 这一档**不是**假想分支。
    NoCriteria,
    /// **活动不在期**：两个边界与当前时刻比过了，在区间之外。
    OutOfWindow,
    /// **活动期判不了**：两个边界都没给，或形态不认识。
    ///
    /// ⚠ 这一档**不发请求**。判不了就是判不了 —— 拿"多半还在期"去发写请求，
    /// 代价是可能花掉今天唯一那一次。
    WindowUnknown,
}

impl FunaiPlan {
    /// 这一档说给用户的那半句。
    ///
    /// ⚠ 只复述判据本身，**不解释上游语义**：计数含义今天有了同日多次读数的口径
    /// （`today = drawTimes + drawsUsed`，即 `draw_times` 是**剩余额度**），
    /// 但仍不念成"总共能抽 N 次" —— 那是**跨天**关系，跨天重置未测试；
    /// 且这一族判据挂在**共享账号**上，念具体数字会把"别人今天用掉过几次"也算进这句话。
    pub fn reason(self) -> &'static str {
        match self {
            FunaiPlan::Draw => "可以抽",
            // ⚠ 这句话里的数字必须跟着 [`FUNAI_DAILY_DRAW_LIMIT`] 走 ——
            // 它是个 `&'static str` 没法插值，所以**由测试**把两者钉在一起
            // （见 `tests::the_already_drawn_reason_tracks_the_limit`）。
            // 改常量却忘了改这句话，表现是界面上写着"最多抽 1 次"而代码已允许 2 次。
            FunaiPlan::AlreadyDrawn => "今天已经抽过一次（每天最多抽 1 次）",
            FunaiPlan::NoChances => "上游报的次数判据显示没有可用次数",
            FunaiPlan::NoCriteria => "上游没给次数判据字段，判不了，本次没发请求",
            FunaiPlan::OutOfWindow => "不在活动起止时间内",
            FunaiPlan::WindowUnknown => "活动起止时间判不了，本次没发请求",
        }
    }

    /// 这一档的稳定标识（落进 JSON，供界面/脚本按它分流）。
    pub fn slug(self) -> &'static str {
        match self {
            FunaiPlan::Draw => "draw",
            FunaiPlan::AlreadyDrawn => "already-drawn",
            FunaiPlan::NoChances => "no-chances",
            FunaiPlan::NoCriteria => "no-criteria",
            FunaiPlan::OutOfWindow => "out-of-window",
            FunaiPlan::WindowUnknown => "window-unknown",
        }
    }
}

/// 按当前时刻判活动期。
///
/// # 只用上游给的两个边界，不自造 TTL
///
/// **两个边界都没给** ⇒ 判不了；只给了一个就只判那一边
/// （上游确实可能只挂一个 —— 未验证，所以按最保守的可用信息判）。
///
/// ⚠ 区间本身坏掉（`start` 晚于 `end`）时**不需要**单独一格：
/// 那两条越界检查已经把这种区间的全部时刻都判成"不在期" ——
/// 设 `start > end`，则任一时刻要么 `< start`（第一条命中）、要么 `> end`（第二条命中），
/// 想同时躲开两条得满足 `start <= now <= end`，而它无解。
/// 落"不在期"而不是"判不了"也更诚实：一个倒挂的窗口里确实没有任何时刻在期。
pub fn funai_window_of(module: &FunaiModule, now_ms: i64) -> FunaiWindow {
    if module.start_time.is_none() && module.end_time.is_none() {
        return FunaiWindow::Unknown;
    }
    if let Some(start) = module.start_time {
        if now_ms < start {
            return FunaiWindow::Out;
        }
    }
    if let Some(end) = module.end_time {
        if now_ms > end {
            return FunaiWindow::Out;
        }
    }
    FunaiWindow::In
}

/// 按一次 `page/getmodule` 的解析结果判 [`FunaiPlan`]。**纯函数**，不碰网络。
///
/// # 为什么只用这三个计数，不碰 `isDraw` / `drawTime` / `drawResult`
///
/// 那三个字段在实拍里**全是 `null`**，而真在同一页抽过之后回读**仍然是 `null`**
/// ⇒ 它们的非 `null` 形态**一个样本都没有**。拿"望文生义的名字"当判据，
/// 等于把猜测变成断言（且它错的时候不报错，只是抽或不抽）。
///
/// # 匹配顺序有意义
///
/// 先「明确不在期」（最硬、且与次数无关）→ 再「判不了次数」→ 再「今天已抽过」→
/// 再「没有次数」→ 最后才用「期次判不了」兜住那一档不发。
/// 换句话说：**能给出具体原因的排前面**，"判不了"这种没有信息量的说法排最后。
///
/// ⚠ 这个顺序还**顺带兜住了一处语义重复扣**（`draw_times` 已确认是余额）：
/// 上限 `1` 使减法那条只在 `draws_used == 0` 时被走到。
/// ⇒ 动这个顺序（尤其把"已抽过"挪到减法之后）会立刻把它变成真实行为差异，
/// 而那个差异的后果（一天抽多次上游认不认）没有样本。
pub fn plan_funai(module: &FunaiModule, now_ms: i64) -> FunaiPlan {
    let window = funai_window_of(module, now_ms);
    if window == FunaiWindow::Out {
        return FunaiPlan::OutOfWindow;
    }
    let (Some(draw_times), Some(draws_used)) = (module.draw_times, module.draws_used) else {
        return FunaiPlan::NoCriteria;
    };
    if draws_used >= FUNAI_DAILY_DRAW_LIMIT {
        return FunaiPlan::AlreadyDrawn;
    }
    if draw_times - draws_used <= 0 {
        return FunaiPlan::NoChances;
    }
    if window == FunaiWindow::Unknown {
        return FunaiPlan::WindowUnknown;
    }
    FunaiPlan::Draw
}

// ══════════════════════════ 跑一个账号 ══════════════════════════

/// 一次趣玩AI抽奖的结局档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FunaiVerdict {
    /// **抽到了**：请求发了，上游回 `code == 0`。
    ///
    /// ⚠ 它**不等于"中奖"** —— 第二档实拍就是 `code:0` + `isWin:0` + `prize*` 三键 `null`
    /// ⇒ "没中奖"就是**这种形状**，它**算这一档**、不算失败。
    /// 这一档说的是"这一次抽完了"；中的是什么看 [`FunaiResult::prize_name`]。
    Done,
    /// **本来就没得抽**（今天已抽过 / 没次数 / 判不了 / 不在期）——
    /// **一条写请求都没发**。它算**成功**：这一趟的目标是"该抽的时候抽掉"，
    /// 来一趟确认"现在不该抽"就是达成了。
    Nothing,
    /// **请求发了但失败** —— 这一趟没办成。
    ///
    /// ⚠ 不返回错误：折成结论才能把"上游回什么"带出去。
    /// "没有次数"那一类失败会落这一档（上游那句话**尚无样本**，所以不猜码值）。
    ///
    /// ⚠ 这个变体**刻意是零值**：`FunaiResult` 有提前 `return` 的路径，
    /// 零值若是 `Done`，一句"抽到了"就会在**一个请求都没发**的那趟上被念出来。
    #[default]
    Failed,
}

/// 从 `page/getmodule` 读到的**判据原值**，只用于播报。
///
/// # 为什么把数一路带到小结
///
/// 真账号跑一轮判成"没抽"时，界面上如果只留一句"没有可用次数"，
/// 这一趟就**答不出**任何判据问题：`draw_times` 到底是几、`draws_used` 是不是满了、
/// 活动期到哪天，全都知道但全都没说。而"没中奖长什么样"与"次数判据的语义"
/// 这两条都要靠**下一次真跑的读数**才能收口 ⇒ 读数必须进小结。
///
/// ⚠ **仍然只照抄、不解释**：本链跑在**别人的账号**上，念"今天还能抽 N 次"这种话
/// 要求"当天没在别处抽过"这一条我们**判不了** ⇒ 只把原值摆出来，结论交给人看。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunaiReadings {
    pub draw_times: Option<i64>,
    pub draws_used: Option<i64>,
    pub today_get_lottery_count: Option<i64>,
    /// 活动期两界（epoch 毫秒）。`None` = 没给**或**形态不认识。
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
}

impl FunaiReadings {
    /// 把读数念成一句短语。缺的键念「缺」而**不念 0** ——
    /// "上游没给这一格"与"上游给了 0"在判据上是两件事。
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = [
            ("drawTimes", self.draw_times),
            ("drawsUsed", self.draws_used),
            ("todayGetLotteryCount", self.today_get_lottery_count),
        ]
        .iter()
        .map(|(k, v)| match v {
            Some(n) => format!("{k}={n}"),
            None => format!("{k}=缺"),
        })
        .collect();
        // 两界只在**真读到**时才念：那两格决定"不在期"这一档，
        // 而"没给"与"给了但解不开"在读数上分不开 ⇒ 都念成缺反而是噪声。
        for (key, value) in [("startTime", self.start_time), ("endTime", self.end_time)] {
            if let Some(ms) = value {
                parts.push(format!("{key}={ms}"));
            }
        }
        parts.join(" ")
    }
}

/// 一个账号这一趟的产出。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunaiResult {
    /// 档位。
    pub verdict: FunaiVerdict,
    /// 判定档位（[`FunaiVerdict::Nothing`] 时它就是原因）。
    pub plan: Option<FunaiPlan>,
    /// 这一趟用的模块 id（来自 `page/getmodule`，**不是**写死的）。
    pub module_id: String,
    /// 上游报的奖品名。`None` = 上游没给这一格。
    ///
    /// ⚠ 实拍里**没中奖那一档 `prize_name` 就是 `None`** ⇒ 读到 `None`
    /// **优先**按"没中奖"理解。但**反过来不成立**（"中了却没报名"是零样本），
    /// 判"中没中"要看上游的 `is_win`。
    pub prize_name: Option<String>,
    /// 判据原值（抽奖那一枪发没发都带着它）。
    pub readings: FunaiReadings,
    /// [`FunaiVerdict::Nothing`] 的人话依据。
    pub reason: String,
    /// [`FunaiVerdict::Failed`] 的失败原因。
    pub error: String,
}

impl FunaiResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// ⚠ 里面只有档位、奖品名与上游给的计数，没有任何凭据形态的东西。
    /// 念计数而**不翻成人话**：理由见 [`FunaiReadings`]。
    pub fn summary_line(&self) -> String {
        let line = match self.verdict {
            FunaiVerdict::Done => match self.prize_name.as_deref() {
                None | Some("") => "趣玩AI已抽一次（上游没报奖品名）".to_string(),
                Some(name) => format!("趣玩AI抽到「{name}」"),
            },
            FunaiVerdict::Failed => {
                if self.error.is_empty() {
                    "趣玩AI抽奖失败".to_string()
                } else {
                    format!("趣玩AI抽奖失败：{}", self.error)
                }
            }
            FunaiVerdict::Nothing => {
                let reason = if self.reason.is_empty() {
                    self.plan
                        .map(FunaiPlan::reason)
                        .unwrap_or("没抽")
                        .to_string()
                } else {
                    self.reason.clone()
                };
                format!("趣玩AI没抽：{reason}")
            }
        };

        // ⚠ **抽成了那一档不念读数**：这几格是抽奖**之前**读的，
        // 抽成之后 `draws_used` 已经变了，念出来会被读成"抽完还剩这些"。
        // 与"不许把领之前的槽当复查结果念"是同一类错。
        if self.verdict == FunaiVerdict::Done {
            return line;
        }
        let readings = self.readings.line();
        if readings.is_empty() {
            line
        } else {
            format!("{line}（读数：{readings}）")
        }
    }

    /// 把档位映射到 [`TaskOutcome`]。
    ///
    /// 只有 [`FunaiVerdict::Failed`] 落失败；抽到了与三种"没发请求"都算成功
    /// （"没发请求"落 `Skipped` —— 本仓"条件不成立、不是失败"的表达）。
    ///
    /// 这里**没有** `ManualRequired` 那一档：趣玩AI的"做任务挣次数"要人去 App 里
    /// 用各个 AI 工具，但本能力只承诺"每天该抽的那一次抽掉"，
    /// **不承诺把任务做完** ⇒ 没有一件"必须人去做、程序代劳不了"的事要挂出来。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            FunaiVerdict::Done => TaskOutcome::success(summary),
            FunaiVerdict::Failed => TaskOutcome::failed(summary),
            FunaiVerdict::Nothing => TaskOutcome::skipped(summary),
        }
    }
}

/// 跑一次趣玩AI抽奖。
///
/// # 门槛不成立 ⇒ **一个写请求都不发**（读那一枪仍会发，它是判据的唯一来源）
///
/// # 错误只有一种：读判据那一枪失败
///
/// 那时整趟没有结论可报，交回错误。抽奖那一枪的失败**不**走这条路，
/// 折进 [`FunaiVerdict::Failed`] —— 那一枪已经发出去了。
pub struct FunaiTask;

#[async_trait]
impl Task for FunaiTask {
    fn name(&self) -> &'static str {
        "fun-ai"
    }
    fn title(&self) -> &'static str {
        "趣玩AI抽奖"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::App
    }
    fn description(&self) -> &'static str {
        "全网趣玩AI赢好礼：读判据后每天最多抽一次（转盘那一口，不是刮刮乐）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "读取趣玩AI活动信息");
        let module = query_module(ctx).await?;

        let readings = FunaiReadings {
            draw_times: module.draw_times,
            draws_used: module.draws_used,
            today_get_lottery_count: module.today_get_lottery_count,
            start_time: module.start_time,
            end_time: module.end_time,
        };
        let plan = plan_funai(&module, ctx.clock.now_ms());

        if plan != FunaiPlan::Draw {
            let result = FunaiResult {
                verdict: FunaiVerdict::Nothing,
                plan: Some(plan),
                module_id: module.module_id,
                prize_name: None,
                readings,
                reason: plan.reason().to_string(),
                error: String::new(),
            };
            report_details(ctx, &result);
            return Ok(result.task_outcome());
        }

        ctx.check_cancelled()?;
        ctx.progress(1, 1, "趣玩AI抽一次");
        // ⚠ 模块 id 一律取**这一次** `getmodule` 给的那个：它与判据同批，
        //    写死了就会在换期时打到过期模块上。
        let req = proto::funai_lottery_request(&module.module_id, None);
        let lottery = match call(ctx, req, "趣玩AI抽奖").await {
            Ok(env) => proto::parse_funai_lottery(&env),
            Err(e) => Err(e),
        };
        let result = match lottery {
            Err(e) => FunaiResult {
                verdict: FunaiVerdict::Failed,
                plan: Some(plan),
                module_id: module.module_id,
                prize_name: None,
                readings,
                reason: String::new(),
                error: e.to_string(),
            },
            Ok(l) => FunaiResult {
                // `code == 0` 就是"抽完了"（含没中奖那一档）。
                verdict: FunaiVerdict::Done,
                plan: Some(plan),
                module_id: module.module_id,
                prize_name: l.prize_name,
                readings,
                reason: String::new(),
                error: String::new(),
            },
        };
        report_details(ctx, &result);
        Ok(result.task_outcome())
    }
}

/// 把这一趟的结论交给明细通道（进 `record.details.reported`）。
///
/// 这一族有**五格**判据读数、且"这三个计数跨天清不清零"未测试 ⇒
/// 真跑时唯一能回答的就是这份读数。只把小结点进 `runs.message`，
/// 那些数就跟着历史一起被压成一句话了。
fn report_details(ctx: &TaskContext, result: &FunaiResult) {
    let slug = match result.verdict {
        FunaiVerdict::Done => "done",
        FunaiVerdict::Nothing => "nothing",
        FunaiVerdict::Failed => "failed",
    };
    ctx.details(serde_json::json!({
        "kind": "fun-ai",
        "verdict": slug,
        "plan": result.plan.map(FunaiPlan::slug),
        "reason": result.reason,
        "error": result.error,
        "moduleId": result.module_id,
        "prizeName": result.prize_name,
        "readings": {
            "drawTimes": result.readings.draw_times,
            "drawsUsed": result.readings.draws_used,
            "todayGetLotteryCount": result.readings.today_get_lottery_count,
            "startTime": result.readings.start_time,
            "endTime": result.readings.end_time,
        },
    }));
}

#[cfg(test)]
mod tests;
