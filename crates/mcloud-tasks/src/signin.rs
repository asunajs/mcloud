//! 签到族（P1 八条）：共用**一条会话**的一批能力。
//!
//! # 为什么这八条一起做
//!
//! `sign` → `receive` → `multiple` → `backup-gift` → `revival` → `prizes`
//! → `cloud-snapshot` → `task-list` 全部打的是**同一个 H5 页面**
//! （`newsignin`），用的是**同一条 jwttoken 会话**。打通一条，其余是加请求。
//!
//! 参考实现里这就是一层装配（`run_dispatch_signin.go`）—— 本模块对应它。
//!
//! # 共同坑：**全链吞错**
//!
//! 旧实现每个失败分支只 `logger.info/error` 一句、**从不抛错**，于是对
//! runner 永远"成功"。后果是定时任务能在"天天报错"的情况下绿灯一个月。
//!
//! 所以本模块的纪律是：**失败一律变成 `Err` 或 `TaskOutcome::failed`**。
//! 唯一例外是那些"上游说本月已经领过了"的分支 —— 那不是失败，
//! 见 [`revival`] 与 [`multiple`] 的说明。
//!
//! # 分层
//!
//! 请求构造与响应解析在 [`mcloud_upstream::app_signin`]（纯函数、无 IO）；
//! 本模块只负责**编排**：串步骤、判分支、把结论说成一句人话。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{ParamSpec, Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_signin as proto;
use mcloud_upstream::app_signin::{SignInEnvelope, SignInStatus};

/// 发一个请求并把响应解成信封。
async fn call(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<SignInEnvelope> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    SignInEnvelope::parse(body)
}

/// 取签到族 H5 请求该带的 `deviceId`。
///
/// # 为什么**不能**用 `ctx.account.device_id`
///
/// 那一份是**登录时冒充 PC 客户端**用的（`Account::device_id`），
/// 与 H5 这条 smid 指纹链换回来的 id **不是同一个东西** —— 兑换那边的
/// 注释已经写死过一句（`routes/exchange.rs`）："⚠ 不是 `account.device_id`"。
///
/// 上游**校验的是内容**，不只是"非空"（抓包 `app-signin-samples.txt` §4.2
/// 的同一条请求三档实证，2026-09-19 账号 B，只换 `deviceId` 一项）：
///
/// | `deviceId` | `code` | 上游 msg |
/// |---|---|---|
/// | 不带 | `615` | 活动太火爆啦，**兑换失败**，试试其他奖品吧 |
/// | 长度对、只错一个字符 | `614` | 活动太火爆啦，**锁定失败**，试试其他奖品吧 |
/// | 真实值（`'B'` + 88 字符 base64 ⇒ 64 字节） | `0` | 正常 |
///
/// 2026-10-03 实测复现：账号里那份是 `B58A…-ENDIN`（32 hex + `-ENDIN` 后缀），
/// 于是 `receiveV3` 领AI豆回 **`614`** ⇒ 界面上一句"活动太火爆啦，锁定失败"，
/// 看着像限流，**其实是参数**。抓包 §4.2 第 3 条专门提醒过别照这句 msg 归类。
///
/// # 取值顺序
///
/// 1. **`body.deviceId`**（路由层注入的那一份 —— 兑换走的就是这个口子，
///    来源是 `SmidService::device_id`，"库里有就不打网络"）；
/// 2. **本机 smid 库里已换回来的那个**（`SmidState::device_id`，含 `B` 前缀）——
///    任务的 `ctx.store` 直接读得到，不联网、定时任务那条路径也覆盖得到；
/// 3. 都没有 ⇒ 退回 `account.device_id`，**并如实警告**（这条已知会被判
///    "内容无效"，不能让它静默地当成正常值用）。
pub(crate) async fn h5_device_id(ctx: &TaskContext, body: Option<&Value>) -> Option<String> {
    let body_id = body.and_then(|b| b.get("deviceId")).and_then(Value::as_str);
    let smid = match ctx.store.get_smid_state(&ctx.account.id).await {
        Ok(Some(st)) => st.device_id,
        Ok(None) => None,
        Err(e) => {
            ctx.warn(format!(
                "读 smid 状态失败（本次可能回落成登录时那份 deviceId，领取类接口会按「内容无效」回 614）：{e}"
            ));
            None
        }
    };
    let account = ctx.account.device_id.as_str();
    let picked = pick_device_id(body_id, smid.as_deref(), account);
    // 落到最后一档（登录时那份）时**必须说一声**：那一份已知会被判"内容无效"，
    // 而界面上只会看到"活动太火爆啦" —— 不说清就等于把参数问题伪装成限流。
    if picked.is_some() && pick_device_id(body_id, smid.as_deref(), "").is_none() {
        ctx.warn(
            "这个账号还没换过 smid deviceId（`data/smid.json` 里没有）—— 本次退回用登录时那份；\
             领取类接口（`receiveV3`）会按「内容无效」回 614，而文案是「活动太火爆啦」",
        );
    }
    picked
}

/// 按优先级挑一个 `deviceId`（纯逻辑，便于单测 —— 见上面那张三档实证表）。
///
/// 顺序是：**`body` 注入的那份** → **smid 库里已换回来的那份** → 登录时那份（兜底，已知会被拒）。
/// 空串与纯空白一律当"没有"：上游对空串的判法等同于"不带"（回 `615`）。
fn pick_device_id(body: Option<&str>, smid: Option<&str>, account: &str) -> Option<String> {
    [body, smid, Some(account)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// 查签到状态（**权威判据**）。
pub async fn query_status(ctx: &TaskContext) -> Result<SignInStatus> {
    let device_id = h5_device_id(ctx, None).await;
    let env = call(
        ctx,
        proto::info_v3_request(device_id.as_deref()),
        "查询签到状态",
    )
    .await?;
    proto::parse_status(&env)
}

/// 把状态里"今天签没签"说成一句人话（供多个任务复用）。
fn describe_status(st: &SignInStatus) -> String {
    format!(
        "余额 {}｜本月已签 {} 天｜连续 {} 天",
        st.total, st.month_days, st.sign_count
    )
}

// ══════════════════════════ sign ══════════════════════════

/// 每日签到。
///
/// # ⚠ 判据是 `infoV3`，不是 `startSignIn` 的响应
///
/// 实测：`startSignIn` 响应里的 `cal` 与 `todaySignIn` **会骗人** ——
/// 它说没签，但紧接着 `infoV3` 已经是 `s:true` 了。判成败一律回查 `infoV3`。
///
/// 只信 `startSignIn` 的实现会把一次**成功**的签到报成失败 ——
/// 而用户看到失败又点一次，就是重复请求。
pub struct SignTask;

#[async_trait]
impl Task for SignTask {
    fn name(&self) -> &'static str {
        "sign"
    }
    fn title(&self) -> &'static str {
        "每日签到"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "在AI豆中心签到，并回查确认"
    }
    /// ⚠ **没有参数**（2026-10-02 用户口径：删掉 `dryRun`「试运行」）。
    ///
    /// 理由：签到是**无害且幂等**的动作（重复签到只会回"今日已签到"），
    /// 所以"预演"挡不住任何真实损失 —— 而它给的**安全感是假的**：
    /// 「试运行」= 只查状态、不签到 ⇒ 跑完你依然不知道"签到会不会成功"，
    /// 它验证的"网络通、凭据对、状态读得到"真跑时同样会验证。
    /// 想验证能不能签上，唯一的办法就是真签。
    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    // ⚠ 原本这条注释写着"形参留 `_body`：本任务**没有任何参数**"—— 现在不成立了：
    // 2026-10-03 起它要把 `body.deviceId`（路由层注入的那一份，若有）转交给
    // `h5_device_id`，所以这里真的用得上 body。
    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        ctx.progress(1, 3, "查询签到状态");
        let before = query_status(ctx).await?;
        ctx.info(format!("签到前：{}", describe_status(&before)));

        if before.signed_today() {
            return Ok(TaskOutcome::success(format!(
                "今日已签到，余额 {}",
                before.total
            )));
        }

        ctx.check_cancelled()?;

        ctx.progress(2, 3, "提交签到");
        let device_id = h5_device_id(ctx, Some(body)).await;
        let env = call(
            ctx,
            proto::start_sign_in_request(device_id.as_deref()),
            "提交签到",
        )
        .await?;
        // ⚠ 这一枪的**响应不作为判据** —— 回查才是。这里只把非 0 码当失败，
        // 因为"码非 0"是确定的坏消息；"码为 0"却不代表签上了。
        env.ensure_ok("提交签到")?;

        ctx.progress(3, 3, "回查签到结果");
        // 旧 TS `tasks/sign/task.ts:56`：提交之后、**再次获取签到状态以确认**（那句
        // `await sleep(1000)` ✓）之前要等一拍 —— 入账不是同步的，立刻回查会读到旧状态，
        // 表现就是"签上了却报没签上" ✗。
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let after = query_status(ctx).await?;

        if !after.signed_today() {
            return Ok(TaskOutcome::failed(format!(
                "提交签到后回查仍未签到（余额 {}）",
                after.total
            )));
        }

        let gained = after.total - before.total;
        let msg = if gained > 0 {
            format!("签到成功，获得 {gained} AI豆，余额 {}", after.total)
        } else {
            // 已签过或服务端延迟入账：状态是真的，但余额没动。
            // **不报失败** —— 状态是权威的；把数字如实说出来。
            format!(
                "签到成功（余额未变，当前 {}；可能是补签或延迟入账）",
                after.total
            )
        };
        ctx.info(&msg);
        Ok(TaskOutcome::success(msg))
    }
}

// ══════════════════════════ receive ══════════════════════════

/// 领取AI豆。
///
/// # 判据来自 `infoV3.receiveList` 的 `cloudType`
///
/// | 值 | 意义 | 动作 |
/// |---|---|---|
/// | `0` | 现在可领 | `receiveV3` |
/// | `1` | 已到期、现在能领 | `receiveTaskExpansion` |
/// | `2` | 下月可领取 | **什么都不发**（H5 只弹一句 toast）|
///
/// ⚠ 曾经有人写"一律走 `receiveV3`"——**没有依据**：抓包里那条账号的
/// `receiveList` 只有一条 `{cloudType:2}`，而整个 HAR 里 `receiveV3`
/// 的请求数是 **0**（官方客户端正因为 `cloudType==2` 而从未调用过它）。
pub struct ReceiveTask;

#[async_trait]
impl Task for ReceiveTask {
    fn name(&self) -> &'static str {
        "receive"
    }
    fn title(&self) -> &'static str {
        "领取AI豆"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "领取待收的AI豆奖励（含到期延期的那一档）"
    }

    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        ctx.progress(1, 2, "查询待领取账目");
        let st = query_status(ctx).await?;
        ctx.info(format!(
            "{}｜待领取 {}",
            describe_status(&st),
            st.to_receive.unwrap_or(0)
        ));

        if st.receive_list.is_empty() {
            return Ok(TaskOutcome::success(format!(
                "没有待领取的AI豆，余额 {}",
                st.total
            )));
        }

        // ⚠ 用 `h5_device_id`，**不是** `ctx.account.device_id`：后者是登录时冒充 PC 客户端
        // 那份，上游判"内容无效"回 614（2026-10-03 实测：领AI豆整笔领不到，
        // 报错文案却是"活动太火爆啦"）。详见那个函数的文档。
        let device_id = h5_device_id(ctx, Some(body)).await;
        let mut claimed_ids: Vec<String> = Vec::new();
        let mut total_gained = 0i64;
        let mut next_month = 0usize;
        // 逐笔上报：哪一笔失败要说得出是哪一笔（旧实现整体丢弃返回值，
        // 于是"领到了多少"在数据上是黑盒）。
        let mut step = 0u32;
        let total_steps = st.receive_list.len() as u32;
        // 失败的条目：**一条失败不拖垮其余**，但要如实收齐、最后报出来。
        //
        // # 为什么逐条兜错，而不是 `?` 直接上抛
        //
        // `receiveList` 里可能同时有「现在可领」与「已到期延期」两档，
        // 它们是**互不相干**的两笔账。第一笔因为某种原因失败（自己那条已被领过、
        // 上游偶发 5xx），没有理由让第二笔也跟着不发 ——
        // 参考实现（`mcloud` 的 `ReceiveAll`）对每一条就是
        // `perr != nil ⇒ 记一笔失败、continue`。
        //
        // ⚠ 早先这里是 `?` ⇒ **第一条失败整批中止**：用户看到"领取失败"，
        // 而后面那笔**本来能领到**。这是"少领了却说失败"的那一侧错误 ——
        // 该领的没领，且下轮要等一整天。
        let mut failed: Vec<String> = Vec::new();

        // 失败**当场**以 `warn` 打一行，而不是只等末行的小结 —— 两个理由：
        //
        // 1. ⚠ **级别**：`info` 在日志面板里是默认色 ✓，而"这一笔没领到"是**警告**
        //    ✗。用户 2026-10-03 实测原话：「日志没附上颜色确认状态」✓ —— 根因就是
        //    这些失败信息全都走了 `ctx.info` ✓。
        // 2. 多账号时日志是交错的一条流 ⇒ 只在末行汇总的话，那一行早被别人的
        //    进度冲走了 ✗。
        let mut note_failure = |msg: String| {
            // ⚠ 用 `fail` 而不是 `warn`：**级别**都是 warn ✓，但**分类**不同 ✓
            // ⇒ 界面上是 `FAIL` 标记 + 黄字，一眼能挑出"这一笔没成" ✓
            //（用户 2026-10-04 口径：「info warn error success 等等标记都没有」✓）。
            ctx.fail(&msg);
            failed.push(msg);
        };

        for item in &st.receive_list {
            ctx.check_cancelled()?;
            step += 1;

            match item.cloud_type {
                proto::cloud_type::NEXT_MONTH => {
                    // 下月可领取：H5 只弹一句提示，**不发任何请求**。
                    next_month += 1;
                    ctx.info(format!(
                        "{} AI豆标记为「下月可领取」，本轮不发请求",
                        item.cloud_num
                    ));
                    continue;
                }
                proto::cloud_type::EXPANSION => {
                    ctx.start("领取延期AI豆");
                    ctx.progress(step, total_steps, "领取延期AI豆");
                    let req = proto::receive_task_expansion_request(item.accept_date.as_deref());
                    // ⚠⚠ 这一笔失败**只跳过它自己** —— 见 `failed` 的说明。
                    match call(ctx, req, "领取延期AI豆").await {
                        Ok(env) => match env.ensure_ok("领取延期AI豆") {
                            Ok(()) => {
                                // `receiveTaskExpansion` 的 result 是**裸数字**（实拍
                                // `result: 100`），不是对象 —— 所以这里不解析形状。
                                claimed_ids.push("延期领取（实拍不返回份数）".to_string());
                                // 一件件报"成了" ✓ —— 逐笔的绿标记才看得出进展 ✓
                                ctx.success("延期领取 ✓（实拍不返回份数）");
                            }
                            Err(e) => note_failure(format!("延期领取：{e}")),
                        },
                        Err(e) => note_failure(format!("延期领取：{e}")),
                    }
                }
                _ => {
                    // `cloudType == 0`（以及任何未知值）：走 `receiveV3`。
                    //
                    // ⚠ `deviceId` 在这一条上是**必填**（H5 标了 `isDeviceId`），
                    // 不带会回 `615` —— 而签到那两条不带没事，别外推。
                    // 这一步开始了 ✓（分类 START ⇒ 蓝标记 ✓）
                    ctx.start(format!("领取AI豆（第 {step}/{total_steps} 笔）"));
                    ctx.progress(step, total_steps, "领取AI豆");
                    // `receive_v3_request` 的 `deviceId` 是**必填 `&str`**（漏发在编译期就
                    // 暴露）。真取不到时**不发**：上游对"没带"回 615、对"内容错"回 614 ——
                    // 两者都领不到，但如实说清比发一个已知会被拒的值强。
                    let Some(device_id) = device_id.as_deref() else {
                        note_failure(
                            "领取AI豆：这个账号还没有可用的 deviceId（smid 指纹没换过）"
                                .to_string(),
                        );
                        continue;
                    };
                    let cloud_id = item.record_id.clone().unwrap_or_default();
                    let req =
                        proto::receive_v3_request(device_id, &cloud_id, Some(item.cloud_type));
                    match call(ctx, req, "领取AI豆").await {
                        Ok(env) => match proto::parse_receive_v3(&env) {
                            Ok(got) => {
                                total_gained += got.receive;
                                claimed_ids.push(format!("+{} AI豆", got.receive));
                                // ⚠ 逐笔绿的 ✓：这是用户最想看的一行 ——
                                // "这一笔到底领到了几颗" ✓。
                                ctx.success(format!("领到 {} AI豆 ✓", got.receive));
                            }
                            Err(e) => note_failure(format!("领取AI豆（解析失败）：{e}")),
                        },
                        Err(e) => note_failure(format!("领取AI豆：{e}")),
                    }
                }
            }
        }

        let mut msg = if total_gained > 0 {
            format!(
                "共领取 {total_gained} AI豆，余额 {}",
                st.total + total_gained
            )
        } else {
            format!("本轮领取完成，余额 {}", st.total)
        };
        if !claimed_ids.is_empty() {
            msg.push_str(&format!("（{}）", claimed_ids.join("、")));
        }
        if next_month > 0 {
            msg.push_str(&format!("；另有 {next_month} 笔标记为下月可领取"));
        }
        if !failed.is_empty() {
            // ⚠ 失败**必须出现在小结里**：否则"领到 9 颗"会把
            // "另一笔没领到"这件事盖住，而那一笔下轮才会再试。
            msg.push_str(&format!(
                "；失败 {} 笔：{}",
                failed.len(),
                failed.join("；")
            ));
        }
        // ⚠ 级别跟着结局走：末行不是"一句普通进展"，扫日志时要能一眼看出黄/红 ✓
        if failed.is_empty() {
            ctx.info(&msg);
        } else {
            ctx.warn(&msg);
        }

        // 至少有一笔成功、也有失败 ⇒ **部分成功**；全失败 ⇒ 失败（不吞错）。
        //
        // ⚠ 这一档是 2026-10-03 补的：原来"有成的也有没成的"落到 `Success`
        // ⇒ 界面写「成功」而正文写「失败 1 笔」✗，用户实测原话就是
        // 「下面的 614 失败还直接说成功」✓。
        let claimed = claimed_ids.len();
        if failed.is_empty() {
            return Ok(TaskOutcome::success(msg));
        }
        if claimed == 0 && next_month == 0 {
            return Ok(TaskOutcome::failed(msg));
        }
        Ok(TaskOutcome::partial(msg))
    }
}

// ══════════════════════════ multiple ══════════════════════════

/// 签到翻倍。
///
/// # 门槛**必须自己判**
///
/// 「自动备份」每月给**一次**翻倍机会：`curMonthBackup && !curMonthBackupSignAccept`
/// （两个值都在 `infoV3` 里）。**两个都成立才去领** ——
/// 每月只有一次机会，条件不成立还发是**白白消耗**。
pub struct MultipleTask;

#[async_trait]
impl Task for MultipleTask {
    fn name(&self) -> &'static str {
        "multiple"
    }
    fn title(&self) -> &'static str {
        "签到翻倍"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "用掉本月那一次签到翻倍机会（需先开自动备份）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 2, "检查翻倍门槛");
        let st = query_status(ctx).await?;

        if !st.should_claim_multiple() {
            // **跳过不是失败**：条件不成立就不该发那一枪。
            let why = match (st.cur_month_backup, st.cur_month_backup_sign_accept) {
                (Some(false), _) => "本月没有开启自动备份",
                (_, Some(true)) => "本月翻倍机会已经用掉",
                _ => "上游没给出翻倍门槛所需的字段",
            };
            return Ok(TaskOutcome::skipped(format!("不满足翻倍条件：{why}")));
        }

        ctx.check_cancelled()?;
        ctx.progress(2, 2, "领取翻倍");
        let env = call(ctx, proto::multiple_request(), "签到翻倍").await?;
        let r = proto::parse_multiple(&env)?;

        // `multiple` 是倍数、`cloudCount` 是合计 —— 两个都报，
        // 只报一个会丢掉"翻了几倍"这个信息。
        let msg = match (r.multiple, r.cloud_count) {
            (Some(m), Some(c)) => format!("翻倍成功：{m} 倍，共 {c} AI豆"),
            (None, Some(c)) => format!("翻倍成功：额外获得 {c} AI豆"),
            (Some(m), None) => format!("翻倍成功：{m} 倍（本次份数上游未给）"),
            (None, None) => "翻倍成功（份数上游未给）".to_string(),
        };
        ctx.info(&msg);
        Ok(TaskOutcome::success(msg))
    }
}

// ══════════════════════════ revival ══════════════════════════

/// 复活奖励（用复活卡 / 回馈卡）。
///
/// # ⚠ 这一族**没有判活接口**
///
/// 参考实现记着"从未抓到"，所以"本月还剩几张卡"只能靠**发一次写请求去撞**。
/// 真账号撞到的答案是：
///
/// ```text
/// HTTP 200，{"code":500,"msg":"本月已领取复活卡奖励"}
/// ```
///
/// 它**不是故障**：本月这一档确实没得领。处置与"签到翻倍本月已领过"
/// 完全同档 —— 报"没领 + 依据"，**不报失败**。
///
/// # 为什么不按"上游月限 1 次"收紧
///
/// 旧实现本地按 **2 次**放行，参考实现明确说：那不是"上游给两次"，
/// 而是**防错过的保险**（判据是盲打 + 中文文案匹配，太紧就会整月空跑）。
/// **抓到判活接口之前不要按 1 收紧** —— 那是用一个没验证的假设
/// 换掉一条实测过的保险。
///
/// 所以本任务**每次运行都盲打一发**，靠服务端的"本月已领取"来做闸。
pub struct RevivalTask;

#[async_trait]
impl Task for RevivalTask {
    fn name(&self) -> &'static str {
        "revival"
    }
    fn title(&self) -> &'static str {
        "复活奖励"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "用复活卡 / 回馈卡换回失效的AI豆（每月一次）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "尝试领取复活奖励");
        let env = call(
            ctx,
            proto::receive_revival_reward_request(None),
            "领取复活奖励",
        )
        .await?;

        // ⭐ 先判"本月已领过" —— 那是**正常的没得领**，不是失败。
        //
        // 判错方向的代价不对称：判成失败会让历史页每天一条红、
        // 并触发 onlyError 推送，而它下个月之前都不会自己好。
        if proto::revival_already_claimed(&env) {
            return Ok(TaskOutcome::skipped(format!(
                "本月已领取过（上游：{}）",
                env.message
            )));
        }

        let r = proto::parse_revival_reward(&env)?;
        let card = if r.card_type == 1 {
            "复活卡"
        } else {
            "回馈卡"
        };
        // ⚠ 只报本次进账与卡型，**不把 totalClouds 当余额**：
        // 它到底是不是"AI豆余额"口径没有验证（H5 文案说的是
        // "把失效云朵转换成 AI 豆"）。接进余额记录只会混两个口径。
        let msg = format!(
            "用掉一张{card}，复活 {} AI豆（累计 {}）",
            r.reward_clouds, r.total_clouds
        );
        ctx.info(&msg);
        Ok(TaskOutcome::success(msg))
    }
}

// ══════════════════════════ backup-gift ══════════════════════════

/// 备份好礼。
///
/// # ⚠ 它是一个**独立活动页**，不是签到页里的接口
///
/// 端点在 `caiyun.feixin.10086.cn/market/backupgift/*` —— **另一个主机**。
/// 这一族里只有它是这样（其余七条都在 `m.mcloud.139.com/ycloud/`）。
///
/// # ⚠ 而且这是一个 **GET 却是写** 的接口
///
/// 参考实现在能力表里专门标注了"记账（**GET 却是写**）"——
/// 意味着 `receive` 那一枪**不能**被当成只读接口反复重试。
/// 所以这里的实现只发一枪、不重试。
///
/// # 门槛在 `info` 里判，不是猜
///
/// `state` 三态：`-1` 未开备份 / `1` 本月已领过 / `0` 可领。
/// 旧项目先判 `curMonth`（本月有没有这份礼），它比 `state` 更靠前。
pub struct BackupGiftTask;

#[async_trait]
impl Task for BackupGiftTask {
    fn name(&self) -> &'static str {
        "backup-gift"
    }
    fn title(&self) -> &'static str {
        "备份好礼"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "领取备份好礼（先查状态再领；注意 receive 是 GET 但会记账）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 2, "查询备份好礼状态");
        let env = call(ctx, proto::backup_gift_info_request(), "查询备份好礼状态").await?;
        let gift = proto::parse_backup_gift(&env)?;

        // ① 本月没有这份礼 ⇒ 连 state 都不用看（旧项目的判据顺序）
        if gift.no_gift_this_month() {
            return Ok(TaskOutcome::skipped("本月没有备份好礼可领"));
        }

        // ② 未开备份 ⇒ 跳过，并给出**人工出口**（这是"需人工"那一类，
        //    不是失败：下个月开了就有了）
        if gift.state == proto::backup_gift_state::OFF {
            return Ok(TaskOutcome::skipped(
                "本月未开启自动备份，领不到备份好礼（需在 App 里开启自动备份）",
            ));
        }

        // ③ 本月已领过 ⇒ 跳过（不是失败）
        if gift.state == proto::backup_gift_state::CLAIMED {
            return Ok(TaskOutcome::skipped("本月备份好礼已经领过了"));
        }

        // ④ 其余值：上游加了新状态而我们不认识 —— **不猜**，如实报出来。
        //    猜"可领"会白打一枪写请求；猜"不可领"会永久漏领。
        if !gift.claimable() {
            return Err(mcloud_core::Error::Other(format!(
                "备份好礼状态未知（state={}），无法判断该不该领；请核对上游是否新增了状态",
                gift.state
            )));
        }

        ctx.check_cancelled()?;
        ctx.progress(2, 2, "领取备份好礼");
        let env = call(ctx, proto::backup_gift_receive_request(), "领取备份好礼").await?;
        let got = proto::parse_backup_gift_receive(&env)?;

        let msg = match got.received {
            Some(n) => format!("备份好礼领取成功，获得 {n} AI豆"),
            // 上游没给份数**不判失败**：领取这个动作本身已经成功了
            None => "备份好礼领取成功（份数上游未给）".to_string(),
        };
        ctx.info(&msg);
        Ok(TaskOutcome::success(msg))
    }
}

// ══════════════════════════ prizes ══════════════════════════

/// 待领取奖品（**只读**）。
///
/// # ⚠ 只读第 1 页是有意的，且要说出来
///
/// 旧实现**只读第 1 页 10 条**，多于 10 条静默截断 —— 那是"看起来没奖品、
/// 其实有"的坑。这里老实报出"本页 N 条"，并说明只查了一页。
///
/// 分页判据与流水那条**不同**：H5 逐字写的是 `records.length < pageSize`
/// ⇒ 到底了，它**不读 `total`**。
pub struct PrizesTask;

#[async_trait]
impl Task for PrizesTask {
    fn name(&self) -> &'static str {
        "prizes"
    }
    fn title(&self) -> &'static str {
        "待领取奖品"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "列出待领取的奖品记录，供你去 App 领取（只读，只查第一页）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "查询待领取奖品");
        // 「待领取」页签是 `type=1`。市场名用签到页那一个。
        let req = proto::prize_record_request(proto::MARKET_NAME, 1, 1, 10);
        let env = call(ctx, req, "查询奖品记录").await?;
        let records = proto::parse_prize_records(&env)?;

        let pending: Vec<_> = records.iter().filter(|r| r.pending()).collect();
        if pending.is_empty() {
            return Ok(TaskOutcome::success(format!(
                "本页没有待领取奖品（共查 {} 条，只查了第一页）",
                records.len()
            )));
        }

        for r in &pending {
            // 逐条 → 用户要拿着名字与过期时间去 App 里找
            ctx.info(format!(
                "待领取：{}（过期 {}）",
                r.prize_name,
                r.expire_time.as_deref().unwrap_or("未知")
            ));
        }
        Ok(TaskOutcome::success(format!(
            "本页有 {} 个待领取奖品（共查 {} 条，只查了第一页）",
            pending.len(),
            records.len()
        )))
    }
}

// ══════════════════════════ cloud-snapshot ══════════════════════════

/// AI豆余额快照（**只读**）。
///
/// # 这是旧项目缺的那一格
///
/// 旧项目 `sign` 里那句 `ctx.state.totalCloud = total` 被**注释掉了**，
/// 于是余额只进日志、不进状态 ⇒ 无法回答"这账号现在还有多少AI豆、
/// 今天涨了多少"。
///
/// 本任务把余额**结构化**地报出来（进 run 记录的 `details`），
/// 于是它是一个可被记录的读数，而不是一行日志。
///
/// # ⚠ `total` 与 `beforeTotal` 不是一回事
///
/// 实拍里两个数**不相等**（`total=10496`、`beforeTotal=8027`）。
/// `beforeTotal` 是"**云朵**改名成 AI 豆那一刻的结转额"，是**冻结的历史快照**，
/// 永远不变 —— 曾把它当余额记进趋势图，结果是一条直线。
/// 所以这里只取 `total`。
pub struct CloudSnapshotTask;

#[async_trait]
impl Task for CloudSnapshotTask {
    fn name(&self) -> &'static str {
        "cloud-snapshot"
    }
    fn title(&self) -> &'static str {
        "AI豆快照"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "记录当前AI豆余额与本月签到进度（只读）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "读取AI豆余额");
        let st = query_status(ctx).await?;

        ctx.info(format!(
            "AI豆余额 {}｜本月已签 {} 天｜连续 {} 天｜今日{}",
            st.total,
            st.month_days,
            st.sign_count,
            if st.signed_today() {
                "已签"
            } else {
                "未签"
            }
        ));
        Ok(TaskOutcome::success(format!(
            "AI豆余额 {}（本月 {} 天，连续 {} 天）",
            st.total, st.month_days, st.sign_count
        )))
    }
}

// ══════════════════════════ task-list ══════════════════════════

/// 任务清单（**只读**）。
///
/// 把AI豆中心当下"有什么可做"列出来：待领取、翻倍机会、到期的延期领取。
///
/// # 与 `prizes` 的分工
///
/// `prizes` 读的是**奖品记录**（奖品最终落点，要人去 App 领）；
/// 本任务读的是**AI豆账目**（AI豆中心自己那本账，能自动领的）。
/// 两者不是同一批东西，混起来会让人以为"清单里有就是奖品"。
pub struct TaskListTask;

#[async_trait]
impl Task for TaskListTask {
    fn name(&self) -> &'static str {
        "task-list"
    }
    fn title(&self) -> &'static str {
        "任务清单"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Signin
    }
    fn description(&self) -> &'static str {
        "列出AI豆中心当下可做的事：待领取、翻倍机会、延期领取（只读）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "读取任务清单");
        let st = query_status(ctx).await?;

        let mut items: Vec<String> = Vec::new();
        for item in &st.receive_list {
            let label = match item.cloud_type {
                proto::cloud_type::NOW => "现在可领",
                proto::cloud_type::EXPANSION => "已到期可领（延期领取）",
                proto::cloud_type::NEXT_MONTH => "下月可领",
                // ⚠ 别只写"未知类型" ✗：用户原话「未知又是啥，不明不白」✓。
                // 把**上游给的原始码**带出来 ⇒ 看图就能查、也知道该往哪加分支 ✓
                // （已知的三个是 0=现在可领 / 1=已到期 / 2=下月，见 `proto::cloud_type`）。
                _ => {
                    return Ok(TaskOutcome::partial(format!(
                        "任务清单里出现未识别的类型码 {}（已知 0/1/2），本次只报了能识别的部分",
                        item.cloud_type
                    )))
                }
            };
            items.push(format!("{label} {} AI豆", item.cloud_num));
        }
        if st.should_claim_multiple() {
            items.push("签到翻倍机会可用（本月尚未使用）".to_string());
        }

        if items.is_empty() {
            return Ok(TaskOutcome::success(format!(
                "当下没有可做的事（余额 {}）",
                st.total
            )));
        }
        for it in &items {
            ctx.info(format!("可做：{it}"));
        }
        Ok(TaskOutcome::success(format!(
            "共 {} 件可做：{}",
            items.len(),
            items.join("；")
        )))
    }
}

#[cfg(test)]
mod tests;
