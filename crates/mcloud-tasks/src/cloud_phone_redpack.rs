//! 红包派对（任务名 `cloud-phone-redpack`）—— 编排层。
//!
//! # 这一层今天做什么
//!
//! ① **换票**：`GET querySpecToken?targetSourceId=001216` 拿 SSO 票 →
//!    `POST /user/tokenValidate` 换这一族自己的 `token`；
//! ② **盘点**：`configTaskLoginList` 的任务表 + `userAccountInfo` 的余额。
//!
//! 四枪**全是只读**。`summary_line` 里如实写「本工具只读」—— 不做的事不假装做。
//!
//! # 写入侧为什么不在这一增量里
//!
//! 写入那 7 条（`userSign` / `userCompleteTask` / `userBrowse` /
//! `userTopicAnswer` / `apkInstall` / `reportInstallApp` / `getRcsToken`）
//! 的**判据**在协议层已经落定（`redpack_status_text` 对未知码回 `None`），
//! 但把它们接进编排层是**另一个**可验证单元：每条写都要先按 `userStatus`
//! 判定该不该发、同一个 `taskCode` 一次运行最多发一次、未知码不许当成「还没做」。
//! 那是一个独立的增量，不是「差一点写完」—— 所以这里先只读，并把 title 写成
//! 「红包派对盘点」让界面上也看得出来。写入侧接上时，title 与这句一起改。
//!
//! # 这面开关今天没人读
//!
//! `AccountSettings::redpack_enabled` 在全仓 **grep 只有定义与默认值**，
//! 没有任何地方读它（`play_ai_enabled` / `ai_avatar_enabled` 同样）。
//! 任务层因此**不加门**：照 `play_ai` 的做法来，免得凭印象发明一条没人走过的行为。
//! 开关的消费方该在调度/组装层，那是另一个决定。

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_crypto::{OsRandom, Random};
use mcloud_engine::{Task, TaskContext, TaskResult};
use mcloud_upstream::cloud_phone_redpack as proto;

// ══════════════════════════ 发一枪 ══════════════════════════

/// 发出去 → 判 HTTP → 解析。
///
/// 与 `play_ai` 里那个同名助手一样：协议层只构造请求，判 HTTP 与解析在编排层。
async fn call<T>(
    ctx: &TaskContext,
    req: HttpRequest,
    what: &str,
    parse: fn(&str) -> Result<T>,
) -> Result<T> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    parse(body)
}

// ══════════════════════════ 随机量 ══════════════════════════

/// 旧 TS 的 `generateRequestId()`：本地 `yyyyMMddHHmmss` + epoch 毫秒 + 8 位 base36。
///
/// ⚠ 旧 TS 里这个 `Date.now()` 与 `getHeaders` 里那个 `timestamp` 是**两次**调用，
/// 理论上能差 1 毫秒。这里两边用**同一个** `ts` —— 签名里含 `requestId`，
/// 让「签名用的时间」与「头里的时间」总是同一个值，比复刻那 1 毫秒的抖动更值得。
fn request_id(rng: &mut dyn Random, clock: &dyn mcloud_core::Clock, ts: i64) -> String {
    format!(
        "{}{ts}{}",
        mcloud_core::beijing_time_digits(clock),
        base36(rng, 8)
    )
}

/// 小写 base36 随机片段（旧 TS：`Math.random().toString(36).substring(2, 2+len)`）。
///
/// ⚠ 分布**不**与 JS 一致：JS 那个小数尾数的位数分布并不均匀，这里按 36 取模逐字符抽。
/// 上游看到的只是「一串定长小写字母数字」，形状一致即可 —— 这一格不参与任何判据，
/// 而逐字节复刻浮点尾数的成本远大于收益。
fn base36(rng: &mut dyn Random, len: usize) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = vec![0u8; len];
    rng.fill_bytes(&mut buf);
    buf.iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

// ══════════════════════════ 盘点 ══════════════════════════

/// 计数五档。
///
/// ⚠ 「状态读不出」**单独成档**，**不**折进「未完成」：折进去的表现是将来接
/// 写请求时，一条读不出状态的任务被当成「还没做」而**重复发一次写**。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedpackCounts {
    pub total: usize,
    pub waiting: usize,
    pub done: usize,
    pub sold_out: usize,
    pub tomorrow: usize,
    pub unknown: usize,
}

/// 盘点读到的东西。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RedpackBoard {
    /// 任务表的三组（新手 / 每日 / 每月）**合并**后的样子。
    pub tasks: Vec<proto::RedpackTask>,
    /// 签到格。只进明细，不参与任务计数 —— 它是「签到」不是「任务」。
    pub sign: Vec<proto::RedpackSignDay>,
    /// 余额。`None` 是「这一枪没读成」，与「读到了但上游没给数」不是一回事。
    pub account: Option<proto::RedpackAccount>,
}

impl RedpackBoard {
    /// 逐项判读。
    ///
    /// 判据就是 `userStatus`：0 未完成 / 1 已完成 / 3 奖品已兑完 / 4 明天再来；
    /// 其余码进 `unknown`。**缺失**这一位按 0（未完成）—— 旧 TS 读的是
    /// `Number(userStatus ?? 0)`，照它来。
    pub fn counts(&self) -> RedpackCounts {
        let mut c = RedpackCounts {
            total: self.tasks.len(),
            ..RedpackCounts::default()
        };
        for t in &self.tasks {
            match t.user_status.unwrap_or(proto::REDPACK_STATUS_TODO) {
                proto::REDPACK_STATUS_DONE => c.done += 1,
                proto::REDPACK_STATUS_SOLD_OUT => c.sold_out += 1,
                proto::REDPACK_STATUS_TOMORROW => c.tomorrow += 1,
                proto::REDPACK_STATUS_TODO => c.waiting += 1,
                _ => c.unknown += 1,
            }
        }
        c
    }
}

/// 这一趟的结论。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RedpackVerdict {
    /// 读到了（与「读到 0 件」不是一回事，见 `summary_line`）。
    #[default]
    Read,
    /// 有一条**致命读**没成。
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RedpackResult {
    pub verdict: RedpackVerdict,
    pub board: RedpackBoard,
    pub error: String,
}

impl RedpackResult {
    /// 面向用户的一句话。
    ///
    /// ⚠ 空表**不**念成「没事做」：上游「这一期没配」与「任务都做完了」是同一种回法，
    /// 把空数组念成结论就是替上游下结论。
    pub fn summary_line(&self) -> String {
        if self.verdict == RedpackVerdict::Failed {
            return match self.error.is_empty() {
                true => "红包派对没读成".to_string(),
                false => format!("红包派对没读成：{}", self.error),
            };
        }
        let c = self.board.counts();
        format!(
            "红包派对任务 {} 件（未完成 {}、已完成 {}、奖品已兑完 {}、明天再来 {}、状态读不出 {}）；{}；本工具只读：不签到、不做任务、不写上游",
            c.total,
            c.waiting,
            c.done,
            c.sold_out,
            c.tomorrow,
            c.unknown,
            balance_line(&self.board.account)
        )
    }

    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            RedpackVerdict::Read => TaskOutcome::success(summary),
            RedpackVerdict::Failed => TaskOutcome::failed(summary),
        }
    }
}

/// 余额那一行。
fn balance_line(account: &Option<proto::RedpackAccount>) -> String {
    match account {
        None => "余额没读成（这一格不是致命读，任务表照报）".to_string(),
        Some(a) => match (a.can_amount, a.total_amount) {
            (Some(can), Some(total)) => format!("余额 {}（累计 {}）", yuan(can), yuan(total)),
            // 半格数据不补 0：0 元与「没给」是两回事。
            _ => "余额那一格上游没给（不按 0 念）".to_string(),
        },
    }
}

/// 分 → 元。
///
/// **整数算术**：`1234` 分经 `f64` 除 100 再乘回去会出现 `12.339999` 这种尾数，
/// 而这一格是要念给用户看的钱数。
fn yuan(cents: i64) -> String {
    let (sign, v) = if cents < 0 {
        ("-", cents.unsigned_abs())
    } else {
        ("", cents as u64)
    };
    format!("{sign}{}.{:02} 元", v / 100, v % 100)
}

/// 明细。
///
/// ⚠ `token` / SSO 票**不进** `details`：那是凭据，会落进运行台账与界面。
/// 这里只放「读到了什么」。
fn report_details(ctx: &TaskContext, result: &RedpackResult) {
    let c = result.board.counts();
    let tasks: Vec<Value> = result
        .board
        .tasks
        .iter()
        .map(|t| {
            json!({
                "taskCode": t.task_code.clone(),
                "taskName": t.task_name.clone(),
                "userStatus": t.user_status,
                "statusText": t.user_status.and_then(proto::redpack_status_text),
                "taskAmount": t.task_amount,
                "prizeAmount": t.prize_amount,
            })
        })
        .collect();
    let sign: Vec<Value> = result
        .board
        .sign
        .iter()
        .map(|d| {
            json!({
                "isToday": d.is_today,
                "status": d.status,
                "statusText": d.status.and_then(proto::redpack_status_text),
                "signAmount": d.sign_amount,
            })
        })
        .collect();
    let account = result
        .board
        .account
        .as_ref()
        .map(|a| json!({"canAmount": a.can_amount, "totalAmount": a.total_amount}));
    ctx.details(json!({
        "kind": "cloud-phone-redpack",
        "verdict": match result.verdict {
            RedpackVerdict::Read => "read",
            RedpackVerdict::Failed => "failed",
        },
        "error": result.error,
        "counts": {
            "total": c.total,
            "waiting": c.waiting,
            "done": c.done,
            "soldOut": c.sold_out,
            "tomorrow": c.tomorrow,
            "unknown": c.unknown,
        },
        "account": account,
        "signDays": sign,
        "tasks": tasks,
    }));
}

// ══════════════════════════ 任务 ══════════════════════════

/// 红包派对（今天：只读盘点）。
pub struct CloudPhoneRedpackTask {
    rng: Mutex<Box<dyn Random>>,
}

impl CloudPhoneRedpackTask {
    /// 生产用真随机源。
    pub fn production() -> Self {
        Self {
            rng: Mutex::new(Box::new(OsRandom::new())),
        }
    }

    /// 注入随机源 —— 测试用它把 `requestId` 与 `x-DeviceInfo` 里那截随机串钉死。
    ///
    /// `Task::run` 拿的是 `&self`，而 `Random::uuid_v4` 要 `&mut self`，
    /// 所以这里用 `Mutex` 提供内部可变性；锁**只在取随机量时**持有，不跨 `await`。
    pub fn with_rng(rng: Box<dyn Random>) -> Self {
        Self {
            rng: Mutex::new(rng),
        }
    }

    /// 组装一次调用。
    fn new_call(&self, token: &str, ts: i64, clock: &dyn mcloud_core::Clock) -> proto::RedpackCall {
        let (id, nonce) = {
            let mut rng = self.rng.lock().expect("随机源互斥量中毒");
            (request_id(&mut **rng, clock, ts), base36(&mut **rng, 16))
        };
        proto::RedpackCall::new(token, ts, id, nonce)
    }

    /// 换票 + 盘点。四枪全只读。
    async fn inventory(&self, ctx: &TaskContext) -> Result<RedpackResult> {
        let device_id = ctx.account.device_id.as_str().to_string();

        // ① 换 SSO 票。这一枪头里**还没有**任何凭证。
        ctx.progress(1, 4, "换 SSO 票");
        ctx.check_cancelled()?;
        let ts = ctx.clock.now_ms();
        let sso = call(
            ctx,
            proto::spec_token_request(ts, proto::REDPACK_SSO_SOURCE_ID),
            "红包派对换 SSO 票",
            proto::parse_spec_token,
        )
        .await?;

        // ② 用 SSO 票换这一族自己的 token。
        //    ⚠ 这一枪的 `token` 头是**空串**，签名也按空串算（协议层有说明）。
        ctx.progress(2, 4, "换活动 token");
        ctx.check_cancelled()?;
        let ts = ctx.clock.now_ms();
        let empty = self.new_call("", ts, &*ctx.clock);
        let token = call(
            ctx,
            proto::token_validate_request(&empty, &sso, &device_id),
            "红包派对换活动 token",
            proto::parse_token_validate,
        )
        .await?;

        // ③ 任务表 —— **致命读**：缺了它就会把「这个号没事做」当成结论。
        ctx.progress(3, 4, "读任务表");
        ctx.check_cancelled()?;
        let ts = ctx.clock.now_ms();
        let c = self.new_call(&token, ts, &*ctx.clock);
        let list = call(
            ctx,
            proto::task_login_list_request(&c),
            "红包派对任务表",
            proto::parse_task_login_list,
        )
        .await?;

        // ④ 余额 —— **非**致命读：读不到就少报一行，不让整条链塌掉。
        ctx.progress(4, 4, "读余额");
        ctx.check_cancelled()?;
        let ts = ctx.clock.now_ms();
        let c = self.new_call(&token, ts, &*ctx.clock);
        let account = call(
            ctx,
            proto::account_info_request(&c),
            "红包派对余额",
            proto::parse_account_info,
        )
        .await
        .ok();

        let mut tasks = Vec::new();
        tasks.extend(list.novice);
        tasks.extend(list.daily);
        tasks.extend(list.monthly);

        Ok(RedpackResult {
            verdict: RedpackVerdict::Read,
            board: RedpackBoard {
                tasks,
                sign: list.sign,
                account,
            },
            error: String::new(),
        })
    }
}

#[async_trait]
impl Task for CloudPhoneRedpackTask {
    fn name(&self) -> &'static str {
        "cloud-phone-redpack"
    }

    fn title(&self) -> &'static str {
        "红包派对盘点"
    }

    fn description(&self) -> &'static str {
        "换票后只读盘点红包派对的任务表与余额（本增量不发任何写请求）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.info("红包派对：开始盘点（只读）");
        let result = match self.inventory(ctx).await {
            Ok(r) => r,
            // 取消不是「读失败」：原样上抛，别折成一份看不出被叫停的结论。
            Err(e) if e.is_cancelled() => return Err(e),
            Err(e) => RedpackResult {
                verdict: RedpackVerdict::Failed,
                board: RedpackBoard::default(),
                error: e.to_string(),
            },
        };
        report_details(ctx, &result);
        Ok(result.task_outcome())
    }
}

#[cfg(test)]
mod tests;
