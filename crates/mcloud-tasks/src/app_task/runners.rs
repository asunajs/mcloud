//! `app-task` 的**专用执行器** —— 旧 TS `tasks/app/runners.ts` 那张 runner 表。
//!
//! # 为什么单独一个文件
//!
//! `app_task.rs` 那半边管的是"判形状 + 传文件 + 分享"，判据全是**纯函数**；
//! 这一半是**一枪一枪的 IO 编排**（登录邮箱 → 建笔记 → 发信 → 转 PDF …），
//! 两者混在一个文件里会让"哪些是判据、哪些是动作"看不出来。
//!
//! # 覆盖的 id（逐条对旧 TS `runners.ts`）
//!
//! | id | 旧 TS 函数 | 本模块 |
//! |---|---|---|
//! | 107 | `getNoteAuthToken → createNote → sleep 2s → moveToRecycleBin` | [`AppRunner::Note`] |
//! | 472 / 473 | `getMail139Session → autologin → taskClickV2` | [`AppRunner::MailActive`] |
//! | 1004 | `getMail139Session → getComposeId → composeMail → PE 上报 → taskClickV2` | [`AppRunner::SendMail`] |
//! | 1014 | 传一个 PDF → `disk:convertPdfFromYunFile` | [`AppRunner::PdfConvert`] |
//! | 1020 | `POST /mailsms/sms/backup` | [`AppRunner::MailBackup`] |
//! | 1028 | `queryUserToken → cloudTaskStatus → SSE 对话 → 复查` | [`AppRunner::AiWorkbench`] |
//! | 585 | `aiRecognize → SSE 对话` | [`AppRunner::AiCamera`] |
//! | 609 | `queryUserToken → cloudTaskStatus(609) → SSE 对话 → 复查` | [`AppRunner::AiAssistant`] |
//!
//! ⚠ 后两行那两个 id 是**旧 TS 的键**，今天两张真机任务表里都不存在
//! （同一个能力现在叫 **1057 / 618**）⇒ 两个 id 都收，理由见 [`AppRunner::for_task`]。
//!
//! # ⚠ 三条**今天已经跑不通**（2026-10-07 实测，见
//! `docs/review/ts-vs-rust-task-gap-261007.md`）
//!
//! 107 的笔记鉴权、472/473 的市场打卡、1020 的备份口 —— 上游分别回
//! "拿不到 `app_auth`" / `404` / `404`。本模块**照抄实现**（口径：TS 能跑就照它写），
//! 但把失败**如实写进明细**，绝不静默当成功 —— "跳过 ≠ 通过"是本仓的一条老口径。
//!
//! # ⚠ 与旧 TS 的一处**结构性差异**：`ctx.state` 是本模块自己管的
//!
//! 旧 TS 把邮箱 session 缓存在 `ctx.state.mail139`（一次运行内复用 ✓）。
//! 本仓的 `TaskContext` 是**不可变**的（没有可变 state 那一格）⇒
//! 缓存落在 [`RunnerState`] 里，由 `run_app_task` 持有并逐条传进来。
//! ⚠ **必须逐条传同一个 `RunnerState`**：每次现建一个会让 472/473/1004/1014/1020/1028
//! 各自重新登录一次邮箱（多发 6 次登录），而旧 TS 只登一次。

use mcloud_core::{Error, HttpRequest, Result};
use mcloud_crypto::{des_decrypt_b64, OsRandom, Random};
use mcloud_engine::TaskContext;
use mcloud_upstream::app_cloud_upload::CloudChannel;
use mcloud_upstream::{ai_cloud, mail139, note};

/// 一个专用执行器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRunner {
    /// 107 创建云笔记。
    Note,
    /// 472 / 473 体验 139 邮箱。
    MailActive,
    /// 1004 给好友发邮件。
    SendMail,
    /// 1014 PDF 转换。
    PdfConvert,
    /// 1020 邮箱资产备份。
    MailBackup,
    /// 1028 AI 工作台。
    AiWorkbench,
    /// 585 AI 相机。
    AiCamera,
    /// 1029「查看"我的账单"」（**2026-10-11 按真机抓包补 ✓**）。
    BillView,
    /// 609「和AI助手对话」—— 与 [`AppRunner::AiWorkbench`] **同一条 AI 链**，两处不同：
    ///
    /// * 状态查询用**本条任务自己的 id**（`609` ✓）—— 那一口是**按 taskId** 作答的
    ///   （抓包仓 `app-ai-workbench-samples.txt` §2 那张对照表：1028 → `501 任务不存在`、
    ///   585 / 106 / 1020 → 真实读数 ✓）⇒ 用别人的 id 问出来的读数对这条没有意义 ✗。
    /// * **不跳对话**：1028 那边照 TS 的 `needChat` 门（前置已满足就不发 ✗）；
    ///   609 在旧 TS 里**根本没有 runner**（全仓 0 命中 ✓）⇒ 那道门没有出处，
    ///   而这一条的**名字就是"对话"** ⇒ 每趟都发 ✓（状态读数只当诊断记进 notes ✓）。
    AiAssistant,
}

impl AppRunner {
    /// 这个 id 走哪个执行器（`None` = 不是这一族）。
    ///
    /// 依据：旧 TS `runners.ts:39-186` 那张表的键。
    ///
    /// # ⚠ 两处**id 别名**（2026-10-07 真机实测加上的）
    ///
    /// 旧 TS 那张表用的是 **585（AI 相机）** 与 **1028（AI 工作台）**，
    /// 而 2026-10-07 在**两个真实账号**上拉到的任务表里，这两条**都不存在**；
    /// 同一个能力今天叫 **618「体验AI相机」** 与 **1057「体验AI工作台」**：
    ///
    /// ```text
    /// 测试 0f6050105ab111f4 / 移动 b73bb07d5971e8e7 两张表：
    ///   618  | time | WAIT | 体验AI相机
    ///   1057 | time | WAIT | 体验AI工作台
    ///   （585 / 1028 两条表里都**没有**）
    /// ```
    ///
    /// ⇒ 按**能力**收，不按数字收：两个 id 都映射到同一个执行器 ✓。
    /// 这不是"照抄之外的自由发挥" —— 旧 TS 自己的主流程跑出来的读数就是
    /// `需前往云盘完成 体验AI相机 618` / `需前往邮箱完成 体验AI工作台 1057`
    ///（它的 runner 表没有这两个键 ⇒ 那两条它**从来打不动** ✗）。
    ///
    /// ⚠ 而 **107 / 472 / 473** 两个账号的任务表里**都没有** ⇒ 那三条 runner
    /// 今天在真机上**到不了**（它们的实现照旧保留：口径是"TS 能跑就照它写"，
    /// 而"上游今天不发这条任务"不是删代码的理由 ✓）。
    pub fn for_task(task_id: i64) -> Option<Self> {
        Some(match task_id {
            107 => Self::Note,
            472 | 473 => Self::MailActive,
            1004 => Self::SendMail,
            1014 => Self::PdfConvert,
            1020 => Self::MailBackup,
            // 1028 = 旧 TS 的键；1057 = 今天真机表里那个「体验AI工作台」。
            1028 | 1057 => Self::AiWorkbench,
            // 585 = 旧 TS 的键；618 = 今天真机表里那个「体验AI相机」。
            585 | 618 => Self::AiCamera,
            // 609「和AI助手对话」：两边都没有 runner ⇒ 这一条是本仓按读数补的 ✓
            //（打卡那一发照发，见 `app_task.rs` 的 click-only 串 —— 实测打卡**不翻账** ✗）。
            1029 => Self::BillView,
            609 => Self::AiAssistant,
            _ => return None,
        })
    }

    /// 报错/日志里那句人话（`1004` 这类只给数字的，带上 id）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "创建云笔记",
            Self::MailActive => "体验139邮箱",
            Self::SendMail => "给好友发邮件",
            Self::PdfConvert => "PDF转换",
            Self::MailBackup => "邮箱备份",
            Self::AiWorkbench => "AI工作台",
            Self::AiCamera => "AI相机",
            Self::BillView => "查看“我的账单”",
            Self::AiAssistant => "和AI助手对话",
        }
    }
}

/// 一次运行的**可变**状态（旧 TS `ctx.state` 的那部分）。
#[derive(Debug)]
pub struct RunnerState {
    /// 邮箱 session 缓存（旧 TS `ctx.state.mail139` ✓）。
    pub mail: Option<mail139::MailSession>,
    rng: OsRandom,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self::new()
    }
}

impl RunnerState {
    pub fn new() -> Self {
        Self {
            mail: None,
            rng: OsRandom,
        }
    }

    /// `randomHex(n)`（旧 TS `rt.crypto.randomBytes(n).toString("hex")` ⇒ **2n** 个字符）。
    fn random_hex(&mut self, n: usize) -> String {
        let mut bytes = vec![0u8; n];
        self.rng.fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `Math.random().toFixed(15)` 那一格的替身（查询串里的 `rnd`）。
    ///
    /// ⚠ 形状照旧 TS（`0.xxxxxxxxxxxxxxx`）：它是**查询串里的一格随机数**，
    /// 上游不校验内容，但少一个 `0.` 前缀会变成另一种形状 —— 没样本说那样也行。
    fn random_fraction(&mut self) -> String {
        let mut bytes = [0u8; 8];
        self.rng.fill_bytes(&mut bytes);
        let n = u64::from_le_bytes(bytes) % 1_000_000_000_000_000;
        format!("0.{n:015}")
    }

    /// `cguid`：旧 TS `mail139.ts:253-257` 的 8 位 hex（**不带** `0` 前缀那一支）。
    fn random_cguid(&mut self) -> String {
        self.random_hex(4)
    }
}

/// 一条执行器的结论。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerReport {
    /// 这一条**真做成了**（旧 TS 的 `doingList` 语义 + 本仓"跳过 ≠ 通过"的口径）。
    pub acted: bool,
    /// 明细里要说的话（含失败原因）。
    pub notes: Vec<String>,
    /// 硬失败（进 `AppTaskResult::failures` ⇒ 影响运行级结论）。
    pub failures: Vec<String>,
}

impl RunnerReport {
    fn note(mut self, s: impl Into<String>) -> Self {
        self.notes.push(s.into());
        self
    }
    fn fail(mut self, s: impl Into<String>) -> Self {
        self.failures.push(s.into());
        self
    }
}

/// 跑一条执行器。`owned` 是"我们造的文件 id"清单（1014 会往里塞一个 PDF）。
pub async fn run_runner(
    ctx: &TaskContext,
    state: &mut RunnerState,
    runner: AppRunner,
    task_id: i64,
    owned: &mut Vec<String>,
) -> RunnerReport {
    match runner {
        AppRunner::Note => note_runner(ctx, state).await,
        AppRunner::MailActive => mail_active_runner(ctx, state, task_id).await,
        AppRunner::SendMail => send_mail_runner(ctx, state).await,
        AppRunner::PdfConvert => pdf_convert_runner(ctx, state, owned).await,
        AppRunner::MailBackup => mail_backup_runner(ctx, state).await,
        AppRunner::AiWorkbench => ai_workbench_runner(ctx, state).await,
        AppRunner::AiCamera => ai_camera_runner(ctx, state).await,
        AppRunner::BillView => bill_view_runner(ctx, state).await,
        AppRunner::AiAssistant => ai_assistant_runner(ctx, state, task_id).await,
    }
}

// ── 1029 查看"我的账单" ─────────────────────────────────────────────────

/// 「查看"我的账单"」（**2026-10-11 按真机抓包补 ✓**）。
///
/// # 为什么长这样（真机读数，抓包仓 `app-task-run-account-samples.txt` §12.6 ✓）
///
/// **小号**那条真机走完**真的翻账了**（面板「已完成 · 第 4 步」✓）⇒ 这一族是**能做成**的 ✓。
/// 那一趟的账单页（WebView 里）逐字发了这一串：
///
/// ```text
/// GET  html5.mail.10086.cn/html/mobilebill.html?sendId=1000006&sid=&cguid=
/// POST html5.mail.10086.cn/RmWeb/mail?func=mbox:searchMessages&…&encryptType=…   ← 账单邮件列表
/// POST html5.mail.10086.cn/setting/s?func=user:getCommConfig&…&behaviorData=…
/// POST html5.mail.10086.cn/RmWeb/mail?func=mbox:updateMessagesStatus&…          ← 标已读
/// GET  html5.mail.10086.cn/html/readmail.html?sid=&mid=
/// GET  html5.mail.10086.cn/RmWeb/view.do?func=view:readMessage&…&comefrom=166…  ← 打开那一封
/// GET  html5.mail.10086.cn/mw2/weather/weather?func=user:logBehaviorAction&action=111631&thingId=261
/// ```
///
/// 本执行器**逐格对齐实拍**（一趟发五发 ✓）：
///
/// ① `mobilebill.html?sendId=1000006&sid=&cguid=`（页面本体 ✓）；
/// ② `mbox:searchMessages`（账单邮件列表 ✓，五条 `sendId` 条件见
///    [`mail139::bill_mails_search_request`]）→ 取第一封的 `mid`；
/// ③ `mbox:updateMessagesStatus`（标已读 ✓）；
/// ④ `view.do?func=view:readMessage`（打开那一封 ✓）；
/// ⑤ 像素 `logBehaviorAction(action=111631, thingId=261)` ✓（**实拍那一对** ✓）。
///
/// ⚠ 第 ⑤ 格**必须用实拍编号**：本仓常量里的 `108365/66` 是从页面包
/// `CONFIG_INFO[1000006].loadCount` 推出来的 ✗，而那一趟真机**根本没发**它 ✗
/// ⇒ 别再把那一对当"账单页加载像素"用 ✗。
///
/// ⚠ 判据仍然只有上游读数（`state` / 任务读数 ✓）—— "两发都 200"**不算做成** ✗。
async fn bill_view_runner(ctx: &TaskContext, state: &mut RunnerState) -> RunnerReport {
    let mut report = RunnerReport::default();
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return report.note(format!("邮箱登录失败：{e}")),
    };
    let cguid = state.random_cguid();

    // ① 账单页本体（实拍：`mobilebill.html?sendId=1000006&sid=&cguid=` ✓）。
    //    ⚠ 这一格此前**从没发过**（早先那版 `bill_page_request` 连 `sendId` 都没有 ✗）。
    match send(
        ctx,
        mail139::bill_page_request(&session, &cguid),
        "账单页（mobilebill.html）",
    )
    .await
    {
        Ok(body) => report
            .notes
            .push(format!("账单页已拉（{} 字节）", body.len())),
        Err(e) => report.notes.push(format!("账单页没成：{e}")),
    }

    // ② 账单邮件列表（账单页自己的 getMailsList ✓）。
    let mid = match send(
        ctx,
        mail139::bill_mails_search_request(&session, &cguid),
        "账单邮件列表（mbox:searchMessages）",
    )
    .await
    {
        Ok(body) => match mail139::first_mail_id(&body) {
            Some(mid) => {
                report.notes.push(format!("账单邮件列表：拿到第一封的 mid"));
                mid
            }
            None => {
                return report.fail(format!(
                    "账单邮件列表里没有 mid（体的键名：{}）",
                    mail139::shape_hint(&body, 12)
                ))
            }
        },
        Err(e) => return report.fail(format!("账单邮件列表没成：{e}")),
    };

    // ③ 标已读（实拍里 `mbox:updateMessagesStatus` 就在"打开那一封"之前 ✓）。
    match send(
        ctx,
        mail139::mark_mail_read_request(&session, &cguid, &mid),
        "标已读（mbox:updateMessagesStatus）",
    )
    .await
    {
        Ok(body) => report
            .notes
            .push(format!("标已读已发（{} 字节）", body.len())),
        Err(e) => report.notes.push(format!("标已读没成：{e}")),
    }

    // ④ 打开那一封（账单页的 view:readMessage ✓）。
    match send(
        ctx,
        mail139::read_message_request(&session, &cguid, &mid),
        "读邮件（view:readMessage）",
    )
    .await
    {
        Ok(body) => report
            .notes
            .push(format!("读邮件已发（正文 {} 字节）", body.len())),
        Err(e) => report.notes.push(format!("读邮件没成：{e}")),
    }

    // ⑤ 上报到 MQ（实拍里"打开那一封"之后连发**三发** ✓：`actionType=1003` ×2 + `1017` ×1 ✓）。
    for (action_type, client_type, channel) in [
        ("1003", "2", ""),
        ("1003", "2", ""),
        ("1017", "1", "10242400"),
    ] {
        let rnd = state.random_fraction();
        match send(
            ctx,
            mail139::mail_report_to_mq_request(
                &session,
                &cguid,
                &rnd,
                &mail139::mail_report_body(action_type, client_type, &mid, channel),
            ),
            "上报到 MQ（disk:mailReportToMQ）",
        )
        .await
        {
            Ok(body) => report
                .notes
                .push(format!("MQ 上报已发（{action_type}）：{body}")),
            Err(e) => report
                .notes
                .push(format!("MQ 上报没成（{action_type}）：{e}")),
        }
    }

    // ⑥ 那一发像素（实拍里"打开那封信"那一刻发的就是它 ✓）。
    //
    // ⚠ **编号以实拍为准**：`action=111631` / `thingId=261` ✓ ——
    //   不是本仓常量里那对从页面包推出来的 `108365/66` ✗（那一发实拍里**根本没发** ✗）。
    match send(
        ctx,
        // ⚠ `behaviorData` 用**实拍那一对**（`110542_6` ✓）—— 它和像素自己的
        //   `action/thingId` 是**两回事**：前者是"这一发属于哪个行为"的标签 ✓。
        mail139::log_behavior_action_request(&session, 111631, 261, &cguid, "110542_6"),
        "行为像素（logBehaviorAction 111631/261）",
    )
    .await
    {
        Ok(_) => report.notes.push("行为像素已发（111631 / 261）".into()),
        Err(e) => report.notes.push(format!("行为像素没成：{e}")),
    }

    report
}

// ── 609 和AI助手对话 ────────────────────────────────────────────────────

/// 「和AI助手对话」（**2026-10-11 加**，两边都没有 runner ⇒ 本仓按读数补 ✓）。
///
/// # 为什么要补（真机读数）
///
/// 真机原始条目：`mcloud://main/aiAssistantGlobal?params=<base64>` 深链
/// （解出来 `{"params":{"adId":"66197","adVt":"47535","sourceId":"101151000"}}` ✓）、
/// `currstep = 0`、`stepTypeSet = ["cloudtasklogstep","sendcloud","processbindtask","click","interfacecallback"]`
/// —— 比同族的 547（`["sendcloud","click"]`）多三格 ✓。
/// **先只打卡**（旧 TS 的 `default:` 分支口径）：真机 run 318 `WAIT → WAIT` ✗
/// （同趟的 547 打卡就翻了 ✓）⇒ 这一条**打卡不够** ✓。
///
/// ⇒ 按"这一条的名字就是对话"接上 AI 链（1028 那套件全部复用 ✓）：
/// ① 派生 AI 域鉴权 → ② 状态查询（**taskId = 本条任务自己的 id** ✓，读数只当诊断）
/// → ③ 对话（SSE）→ ④ `sleep 5s` 复查。
///
/// ⚠ **不跳对话**（与 1028 不同的那一格）：1028 照 TS 的 `needChat` 门"前置已满足就不发"，
/// 而 609 在旧 TS 里没有 runner ⇒ 那道门**没有出处**；这一条的动因就是"发一发对话" ✓
/// ⇒ 每趟都发，状态读数写进 notes 供下次判断 ✓。
async fn ai_assistant_runner(
    ctx: &TaskContext,
    state: &mut RunnerState,
    task_id: i64,
) -> RunnerReport {
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return RunnerReport::default().note(format!("邮箱登录失败：{e}")),
    };
    let mut report = RunnerReport::default();

    // ① 派生 AI 域鉴权（与 1028 同一条取法 ✓）。
    let auth = match derive_ai_auth(ctx, state, &session).await {
        Ok(a) => a,
        Err(e) => return report.note(format!("AI 助手鉴权派生失败：{e}")),
    };
    if auth.user_domain_id.is_empty() {
        return report.note("AI 助手对话需要 userDomainId，未取到");
    }

    // ② 状态查询：**用本条任务自己的 id**（那一口是按 taskId 作答的 ✓）——只当诊断 ✓。
    if let Ok(rsa) = ai_cloud::encrypt_phone_for_task_status(&auth.phone, &mut state.rng) {
        let req = ai_cloud::cloud_task_status_request(
            &rsa,
            task_id,
            ai_cloud::AI_WORKBENCH_SOURCE_CHANNEL,
        );
        match send(ctx, req, "cloudTaskStatus").await {
            Ok(body) => match ai_cloud::parse_cloud_task_status(&body) {
                Ok(cs) => report.notes.push(format!(
                    "状态读数（taskId={task_id}）：code={:?} taskStatus={:?} cloudStatus={:?}",
                    cs.code, cs.task_status, cs.cloud_status
                )),
                Err(e) => report.notes.push(format!("状态读数解不出：{e}")),
            },
            Err(e) => report.notes.push(format!("cloudTaskStatus 查询异常：{e}")),
        }
    }

    // ③ 对话（SSE）—— 这一条的正事 ✓。
    let input_time = beijing_stamp(ctx, "%Y-%m-%d %H:%M:%S");
    let dialogue = ai_cloud::build_dialogue_input("你好", &input_time);
    let req = ai_cloud::workbench_chat_request(
        &auth.authorization,
        &auth.user_domain_id,
        &dialogue,
        ai_cloud::AI_WORKBENCH_SOURCE_CHANNEL,
    );
    match send(ctx, req, "AI 助手对话（chat/v2/add）").await {
        Ok(body) => {
            let sse = ai_cloud::parse_sse_stream(&body);
            if sse.event_count == 0 || sse.full_text.is_empty() {
                report
                    .notes
                    .push("AI 对话未取到任何输出（可能内容触发审核被断流）".into());
            } else {
                report.acted = true;
                let head: String = sse.full_text.chars().take(40).collect();
                report.notes.push(format!(
                    "AI 对话成功（events={}，回复：{head}…）",
                    sse.event_count
                ));
            }
        }
        Err(e) => report.notes.push(format!("AI 对话异常：{e}")),
    }

    // ④ 复查（同 1028 的口径：`sleep 5s` 再问一次 ✓）。
    tokio::time::sleep(std::time::Duration::from_millis(5_000)).await;
    if let Ok(rsa) = ai_cloud::encrypt_phone_for_task_status(&auth.phone, &mut state.rng) {
        let req = ai_cloud::cloud_task_status_request(
            &rsa,
            task_id,
            ai_cloud::AI_WORKBENCH_SOURCE_CHANNEL,
        );
        if let Ok(body) = send(ctx, req, "cloudTaskStatus 复查").await {
            if let Ok(cs) = ai_cloud::parse_cloud_task_status(&body) {
                report.notes.push(format!(
                    "复查：taskStatus={:?} cloudStatus={:?}",
                    cs.task_status, cs.cloud_status
                ));
            }
        }
    }
    report
}

// ── 公共件 ──────────────────────────────────────────────────────────────

/// 取邮箱 session（一次运行内只登一次，旧 TS `ctx.state.mail139` ✓）。
async fn mail_session(ctx: &TaskContext, state: &mut RunnerState) -> Result<mail139::MailSession> {
    if let Some(s) = &state.mail {
        return Ok(s.clone());
    }
    // ① 换 SSO token（旧 TS `auth-service.ts:12` 的 `querySpecToken(ctx.http, "001003")`）。
    //
    // ⚠ `spec_token_request` 只读 `RecipeParams::to_source_id` 那一格 —— 其余几格
    // 在这一步**不参与**（它们属于页面会话配方，不是这一发）。所以这里给一份
    // 占位值，别把它当成"这个会话的配方"。
    let params = mcloud_upstream::session::RecipeParams {
        to_source_id: mail139::MAIL_SSO_SOURCE_ID,
        market_name: "",
        source_id: "",
        h5_app_id: None,
        page_path: "",
        portal_page: "",
        target_source_id: None,
    };
    let req = mcloud_upstream::session::spec_token_request(params, &ctx.account.auth.raw);
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "换邮箱 SSO token")?;
    let sso = mcloud_upstream::session::parse_sso_token(body)?;

    // ② 用 SSO token 换 sid / rmkey。
    let req = mail139::mail_login_request(&sso);
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "邮箱登录（inlogin）")?;
    let session = mail139::parse_mail_session(body)?;
    state.mail = Some(session.clone());
    Ok(session)
}

/// 发一个请求并把非 2xx 变成错误（体照旧交回）。
async fn send(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<String> {
    let resp = ctx.http.request(req).await?;
    mcloud_upstream::session::ensure_http_ok(&resp, what).map(str::to_string)
}

/// 市场打卡（472/473/1004 共用的"推进市场步"）。
///
/// ⚠ 这一口**今天整体 404**（两个 `marketname` 都试过）⇒ 调用方把它当**明细**记，
/// 不当硬失败：旧 TS 也是 `tryRequest` 包着、失败只不打印成功那句 ✓。
async fn market_click(ctx: &TaskContext, task_id: i64, marketname: &str) -> Result<()> {
    let body = send(
        ctx,
        mail139::market_task_click_v2_request(task_id, marketname),
        "邮箱市场打卡（taskClickV2）",
    )
    .await?;
    let _ = body;
    Ok(())
}

// ── 107 创建云笔记 ──────────────────────────────────────────────────────

async fn note_runner(ctx: &TaskContext, state: &mut RunnerState) -> RunnerReport {
    let token = ctx.account.auth.token.clone();
    let phone = ctx.account.auth.phone.clone();
    if token.is_empty() {
        return RunnerReport::default().note("未配置 authToken，跳过云笔记任务");
    }

    let body = match send(ctx, note::note_auth_request(&token, &phone), "云笔记鉴权").await {
        Ok(b) => b,
        Err(e) => return RunnerReport::default().note(format!("云笔记鉴权请求失败：{e}")),
    };
    let headers = match note::parse_note_headers(&body) {
        Ok(Some(h)) => h,
        Ok(None) => {
            return RunnerReport::default().note("获取笔记鉴权失败，跳过云笔记任务");
        }
        Err(e) => return RunnerReport::default().note(format!("笔记鉴权响应读不出：{e}")),
    };
    if note::note_app_auth(&headers).is_none() {
        // ⚠ 2026-10-07 实测就是这一支（上游不再给 `headers.app_auth`）——
        // 后三枪**一枪都不发**（旧 TS 那句 `return` 逐字）。
        return RunnerReport::default()
            .note("获取笔记鉴权失败（响应里没有 headers.app_auth），跳过云笔记任务");
    }

    let note_id = state.random_hex(32);
    let title = state.random_hex(3);
    let now = ctx.clock.now_ms();
    let mut report = RunnerReport::default();

    match send(
        ctx,
        note::create_note_request(&note_id, &title, &phone, &headers, now),
        "建云笔记（createNote）",
    )
    .await
    {
        // 旧 TS 用 `tryRequest` 包着、结果不看 ⇒ 失败只记明细，不改结论。
        Ok(_) => report.notes.push("云笔记已创建".into()),
        Err(e) => report
            .notes
            .push(format!("建云笔记失败（旧实现也忽略）：{e}")),
    }
    // 旧 TS：`await sleep(2000)`（建完停 2 秒再删）。
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
    match send(
        ctx,
        note::delete_note_request(&note_id, &headers),
        "删云笔记（moveToRecycleBin）",
    )
    .await
    {
        Ok(_) => report.notes.push("云笔记已移进回收站".into()),
        Err(e) => report
            .notes
            .push(format!("删云笔记失败（旧实现也忽略）：{e}")),
    }
    report.acted = true;
    report
}

// ── 472 / 473 体验 139 邮箱 ─────────────────────────────────────────────

async fn mail_active_runner(
    ctx: &TaskContext,
    state: &mut RunnerState,
    task_id: i64,
) -> RunnerReport {
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return RunnerReport::default().note(format!("邮箱登录失败：{e}")),
    };
    let mut report = RunnerReport::default();
    // ① 打一次 autologin（旧 TS `triggerMailActive`：`tryRequest` 包着、不看结果）。
    if let Err(e) = send(
        ctx,
        mail139::mail_autologin_request(&session),
        "邮箱活跃（autologin）",
    )
    .await
    {
        report
            .notes
            .push(format!("邮箱 autologin 没打成（旧实现也忽略）：{e}"));
    }
    // ② 推进市场步（旧 TS 只在 472/473 那一支做）。
    match market_click(ctx, task_id, "sign_in_3").await {
        Ok(()) => {
            report.acted = true;
            report
                .notes
                .push(format!("邮箱市场 click 已推进 task {task_id}"));
        }
        Err(e) => report.notes.push(format!("邮箱市场 click 没成：{e}")),
    }
    report
}

// ── 1004 给好友发邮件 ───────────────────────────────────────────────────

async fn send_mail_runner(ctx: &TaskContext, state: &mut RunnerState) -> RunnerReport {
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return RunnerReport::default().note(format!("邮箱登录失败：{e}")),
    };
    let mut report = RunnerReport::default();

    // ① composeId。
    let compose_id = match send(
        ctx,
        mail139::get_compose_id_request(&session),
        "取 composeId",
    )
    .await
    {
        Ok(body) => match mail139::parse_compose_id(&body) {
            Ok(Some(id)) => id,
            Ok(None) => {
                return report.note("getComposeId 取不到 composeId，跳过");
            }
            Err(e) => return report.note(format!("getComposeId 响应读不出：{e}")),
        },
        Err(e) => return report.note(format!("getComposeId 请求失败：{e}")),
    };

    // ② 发信。收件人/主题/正文取旧 TS 的默认值（本仓今天没有 mail139 配置面）。
    let account = format!("{}@139.com", ctx.account.auth.phone);
    let subject = format!("签到任务测试 {}", ctx.clock.now_ms());
    let content = "本邮件由 mcloud 自动发送，可忽略。";
    let mail = mail139::ComposeMail {
        compose_id,
        account: account.clone(),
        to: account.clone(),
        subject,
        content: content.into(),
        is_html: 0,
        action: "asyn".into(),
    };
    match send(
        ctx,
        mail139::compose_mail_request(&session, &mail),
        "发邮件（compose）",
    )
    .await
    {
        Ok(body) => match mail139::parse_mail_code(&body) {
            Ok(code) if code.is_ok() => {
                report.acted = true;
                report.notes.push(format!("邮件发送成功 to={account}"));
                // ③ PE 活跃事件（eventType "3" = 发邮件）。旧 TS 也是 best-effort。
                if let Err(e) = report_pe_send_mail(ctx, state, &session).await {
                    report
                        .notes
                        .push(format!("PE 活跃上报没打成（旧实现也忽略）：{e}"));
                }
            }
            Ok(code) => {
                let summary = code.summary.unwrap_or_default();
                report
                    .notes
                    .push(format!("邮件发送失败 code={} summary={summary}", code.code));
            }
            Err(e) => report.notes.push(format!("发信响应读不出：{e}")),
        },
        Err(e) => report.notes.push(format!("发信请求失败：{e}")),
    }

    // ④ 推进市场步（旧 TS 无条件做）。
    match market_click(ctx, 1004, "newsign_139mail").await {
        Ok(()) => report.notes.push("邮箱市场 click 已推进 task 1004".into()),
        Err(e) => report.notes.push(format!("邮箱市场 click 没成：{e}")),
    }
    report
}

/// PE 活跃上报（`eventType = "3"` = 发邮件）。
async fn report_pe_send_mail(
    ctx: &TaskContext,
    state: &mut RunnerState,
    session: &mail139::MailSession,
) -> Result<()> {
    // 时间用**东八区墙钟**（旧 TS 用本地 `Date` 的字段；本机时区就是东八区，
    // 而本仓的"东八区"定义只有 `mcloud_core::beijing_offset` 一处）。
    let digits = mcloud_core::beijing_time_digits(&*ctx.clock);
    if digits.len() != 14 {
        return Err(Error::Other(format!(
            "时钟读不出 14 位墙钟（实得 {digits:?}）"
        )));
    }
    let clock = mail139::PeClock::from_local(
        digits[0..4].parse().unwrap_or(0),
        digits[4..6].parse().unwrap_or(0),
        digits[6..8].parse().unwrap_or(0),
        digits[8..10].parse().unwrap_or(0),
        digits[10..12].parse().unwrap_or(0),
        digits[12..14].parse().unwrap_or(0),
    );
    let ev = mail139::PeActiveEvent {
        event_time: clock.event_time.clone(),
        event_type: "3".into(),
    };
    let cguid = state.random_cguid();
    let req = mail139::report_active_action_request(session, &[ev], &cguid, &clock)?;
    let body = send(ctx, req, "PE 活跃上报").await?;
    let _ = mail139::parse_mail_code(&body);
    Ok(())
}

// ── 1014 PDF 转换 ───────────────────────────────────────────────────────

async fn pdf_convert_runner(
    ctx: &TaskContext,
    state: &mut RunnerState,
    owned: &mut Vec<String>,
) -> RunnerReport {
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return RunnerReport::default().note(format!("邮箱登录失败：{e}")),
    };
    let mut report = RunnerReport::default();

    // ① 生成最小 PDF（旧 TS `runners.ts:299-315` 逐字）。
    let ts = ctx.clock.now_ms();
    let file_name = format!("asign-pdf-{ts}.pdf");
    let unique_text = format!("asign auto pdf {ts}");
    let pdf = format!(
        "%PDF-1.4\n\
         1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
         2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
         3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n\
         4 0 obj\n<< /Length {} >>\nstream\n\
         BT\n/F1 18 Tf\n72 720 Td\n({unique_text}) Tj\nET\n\
         endstream\nendobj\n\
         5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n\
         xref\n0 6\n0000000000 65535 f\n0000000009 00000 n\n0000000058 00000 n\n\
         0000000115 00000 n\n0000000223 00000 n\n0000000378 00000 n\n\
         trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n444\n%%EOF\n",
        unique_text.len() + 50
    );

    // ② 传上去（**走本仓统一的 create/put/complete 那条路** ⇒ 会自动落账、链尾自清 ✓）。
    let file_id = match crate::app_task::upload_bytes(
        ctx,
        CloudChannel::Android,
        &file_name,
        pdf.as_bytes(),
        owned,
    )
    .await
    {
        Ok(Some(id)) => id,
        Ok(None) => return report.note("PDF 上传撞同名（不是我们造的）⇒ 跳过转换"),
        Err(e) => return report.note(format!("PDF 上传失败：{e}")),
    };

    // ③ 调转换口。
    let params = mail139::ConvertPdf {
        file_name,
        file_real_size: pdf.len(),
        file_id,
        convert_type: 1,
    };
    let rnd = state.random_fraction();
    let cguid = state.random_cguid();
    match send(
        ctx,
        mail139::convert_pdf_request(&session, &params, &rnd, &cguid),
        "PDF 转换（convertPdfFromYunFile）",
    )
    .await
    {
        Ok(body) => match mail139::parse_mail_code(&body) {
            Ok(code) if code.is_ok() => {
                report.acted = true;
                report.notes.push("PDF转换完成".into());
            }
            Ok(code) => report.notes.push(format!(
                "PDF转换失败 code={} summary={}",
                code.code,
                code.summary.unwrap_or_default()
            )),
            Err(e) => report.notes.push(format!("PDF 转换响应读不出：{e}")),
        },
        Err(e) => report.notes.push(format!("PDF 转换请求失败：{e}")),
    }
    report
}

// ── 1020 邮件转存（App 实拍三发；旧路 `mailsms/sms/backup` 实测 404）──────

/// 「完成一次邮件转存」（描述：**重要邮件存储云盘**）。
///
/// 三发，全部来自 **139邮箱 App 自己的字面量**（旧 TS 这一条只有一条今天 404 的口 ✗）：
/// ① `disk:getDirectorys`（落点目录）→ ② `mbox:SearchByFidOrCondition`（一枚邮件号 `mid`）
/// → ③ `disk:backupMail`（转存）。形状与出处见 [`mcloud_upstream::mail139`] 那一节 ✓。
///
/// ⚠ 判据：**只有 ③ 被上游认了才算做成** ✓ —— ①② 是取参数（取不到就如实记、并把
/// **体的键名形状**打进明细，好让下一次改动有依据 ✓）。
async fn mail_backup_runner(ctx: &TaskContext, state: &mut RunnerState) -> RunnerReport {
    let mut report = RunnerReport::default();
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return report.note(format!("邮箱登录失败：{e}")),
    };
    let cguid = state.random_cguid();

    // ① 落点目录。取不到就**留空**（上游可能自己落到根/默认目录）—— 不猜一个 id 塞进去 ✗。
    let mut directory_id = String::new();
    match send(
        ctx,
        mail139::cloud_directories_request(&session, &cguid, ""),
        "云盘目录（disk:getDirectorys）",
    )
    .await
    {
        Ok(body) => match mail139::first_field(&body, "directoryId") {
            Some(id) => directory_id = id,
            None => report.notes.push(format!(
                "目录列表里没有 directoryId ⇒ 落点留空（体的键名：{}）",
                mail139::shape_hint(&body, 12)
            )),
        },
        Err(e) => report
            .notes
            .push(format!("云盘目录取不到（落点留空）：{e}")),
    }

    // ② 一枚邮件号。
    let mid = match send(
        ctx,
        mail139::mail_search_request(&session, &cguid),
        "邮件列表（mbox:SearchByFidOrCondition）",
    )
    .await
    {
        Ok(body) => match mail139::first_field(&body, "mid") {
            Some(mid) => mid,
            None => {
                return report.fail(format!(
                    "邮件列表里取不到 mid（体的键名：{}）",
                    mail139::shape_hint(&body, 12)
                ))
            }
        },
        Err(e) => return report.fail(format!("邮件列表请求失败：{e}")),
    };

    // ③ 转存。
    match send(
        ctx,
        mail139::backup_mail_request(&session, &cguid, &mid, &directory_id),
        "邮件转存（disk:backupMail）",
    )
    .await
    {
        Ok(body) => match mail139::parse_mail_code(&body) {
            Ok(code) if code.is_ok() => {
                report.acted = true;
                report.notes.push("邮件转存已提交（上游认了）".into());
            }
            // ⚠ 打回就是**没做成**（它是这一条唯一的动作 ✓）⇒ 进 `failures`，
            // 让摘要里带上上游的 `code`/`summary`（旧那条 404 就是这么报的 ✓）。
            Ok(code) => report.failures.push(format!(
                "邮件转存被上游打回：code={} summary={}",
                code.code,
                code.summary.unwrap_or_default()
            )),
            Err(e) => report.notes.push(format!("邮件转存响应读不出：{e}")),
        },
        Err(e) => report.notes.push(format!("邮件转存请求失败：{e}")),
    }
    report
}

// ── 1028 AI 工作台 ──────────────────────────────────────────────────────

async fn ai_workbench_runner(ctx: &TaskContext, state: &mut RunnerState) -> RunnerReport {
    let session = match mail_session(ctx, state).await {
        Ok(s) => s,
        Err(e) => return RunnerReport::default().note(format!("邮箱登录失败：{e}")),
    };
    let mut report = RunnerReport::default();

    // ① 派生 AI 域鉴权（queryUserToken → DES 解手机号 → Basic）。
    let auth = match derive_ai_auth(ctx, state, &session).await {
        Ok(a) => a,
        Err(e) => return report.note(format!("AI 工作台鉴权派生失败：{e}")),
    };

    // ② 前置状态：已满足就跳过对话（旧 TS `needChat`）。
    let mut need_chat = true;
    match ai_cloud::encrypt_phone_for_task_status(&auth.phone, &mut state.rng) {
        Ok(rsa) => {
            let req = ai_cloud::cloud_task_status_request(
                &rsa,
                1028,
                ai_cloud::AI_WORKBENCH_SOURCE_CHANNEL,
            );
            match send(ctx, req, "cloudTaskStatus").await {
                Ok(body) => {
                    if let Ok(cs) = ai_cloud::parse_cloud_task_status(&body) {
                        if cs.code == 0 && cs.task_status == Some(1) && cs.cloud_status == Some(0) {
                            report
                                .notes
                                .push("AI 工作台前置条件已满足，跳过对话".into());
                            need_chat = false;
                        }
                    }
                }
                Err(e) => report.notes.push(format!("cloudTaskStatus 查询异常：{e}")),
            }
        }
        Err(e) => report.notes.push(format!("RSA 加密手机号失败：{e}")),
    }

    // ③ 发起对话（SSE）。
    if need_chat {
        if auth.user_domain_id.is_empty() {
            return report.note("AI 工作台对话需要 userDomainId，未取到");
        }
        let input_time = beijing_stamp(ctx, "%Y-%m-%d %H:%M:%S");
        let dialogue = ai_cloud::build_dialogue_input("你好", &input_time);
        let req = ai_cloud::workbench_chat_request(
            &auth.authorization,
            &auth.user_domain_id,
            &dialogue,
            ai_cloud::AI_WORKBENCH_SOURCE_CHANNEL,
        );
        match send(ctx, req, "AI 工作台对话（chat/v2/add）").await {
            Ok(body) => {
                let sse = ai_cloud::parse_sse_stream(&body);
                if sse.event_count == 0 || sse.full_text.is_empty() {
                    report
                        .notes
                        .push("AI 对话未取到任何输出（可能内容触发审核被断流）".into());
                } else {
                    report.acted = true;
                    let head: String = sse.full_text.chars().take(40).collect();
                    report.notes.push(format!(
                        "AI 对话成功（events={}，回复：{head}…）",
                        sse.event_count
                    ));
                }
            }
            Err(e) => report.notes.push(format!("AI 对话异常：{e}")),
        }
    } else {
        // 前置已满足也算"这一条已经不用做了"—— 旧 TS 这时不设 `acted` 的语义，
        // 但后一步复查照样发 ⇒ 这里把 `acted` 置真（它确实是"无需再做"）。
        report.acted = true;
    }

    // ④ 确认推进（旧 TS `sleep(5s)` → 复查）。
    tokio::time::sleep(std::time::Duration::from_millis(5_000)).await;
    if let Ok(rsa) = ai_cloud::encrypt_phone_for_task_status(&auth.phone, &mut state.rng) {
        let req =
            ai_cloud::cloud_task_status_request(&rsa, 1028, ai_cloud::AI_WORKBENCH_SOURCE_CHANNEL);
        if let Ok(body) = send(ctx, req, "cloudTaskStatus 复查").await {
            if let Ok(cs) = ai_cloud::parse_cloud_task_status(&body) {
                report.notes.push(format!(
                    "复查：taskStatus={:?} cloudStatus={:?}",
                    cs.task_status, cs.cloud_status
                ));
            }
        }
    }
    report
}

/// 从邮箱侧取 `userDomainId`（`disk:queryUserToken` ✓）。
///
/// 用途只有一个：**PC 端登录的账号**没落 `account.user_domain_id`，而 585 那一枪
/// 需要 `userId` / `x-yun-uni`（见 [`ai_camera_runner`] 的说明）。
/// 与 1028 走的是**同一条**取法（同一个账号、同一个用户域 ✓）。
async fn derive_user_domain_from_mail(
    ctx: &TaskContext,
    state: &mut RunnerState,
) -> Result<String> {
    let session = mail_session(ctx, state).await?;
    let rnd = state.random_fraction();
    let cguid = state.random_cguid();
    let body = send(
        ctx,
        mail139::query_user_token_request(&session, &rnd, &cguid),
        "queryUserToken",
    )
    .await?;
    let t = mail139::parse_query_user_token(&body)?;
    match t.user_domain_id {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(Error::Parse("queryUserToken 没给 userDomainId".into())),
    }
}

/// 1028 派生出来的 AI 域凭据。
struct AiAuth {
    phone: String,
    authorization: String,
    user_domain_id: String,
}

/// 旧 TS `runners.ts:427-448` 的 `deriveAiDomainAuth` 逐字。
async fn derive_ai_auth(
    ctx: &TaskContext,
    state: &mut RunnerState,
    session: &mail139::MailSession,
) -> Result<AiAuth> {
    let rnd = state.random_fraction();
    let cguid = state.random_cguid();
    let body = send(
        ctx,
        mail139::query_user_token_request(session, &rnd, &cguid),
        "queryUserToken",
    )
    .await?;
    let t = mail139::parse_query_user_token(&body)?;
    // DES-ECB，key = "139_mail"（旧 TS `rt.crypto.desDecrypt(inner.account, "139_mail")`）。
    let phone = des_decrypt_b64(&t.account, "139_mail")
        .map_err(|e| Error::Other(format!("DES 解手机号失败：{e}")))?;
    let authorization = format!(
        "Basic {}",
        mcloud_crypto::b64_encode(format!("mobile:{phone}:{}", t.token).as_bytes())
    );
    Ok(AiAuth {
        phone,
        authorization,
        user_domain_id: t.user_domain_id.unwrap_or_default(),
    })
}

// ── 585 AI 相机 ─────────────────────────────────────────────────────────

/// 585 用的样图：100×100 白色 PNG 的裸 base64（旧 TS `ai-tasks.ts:11-14` 逐字）。
pub const AI_CAMERA_SAMPLE_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAGQAAABkCAIAAAD/gAIDAAAACklEQVR42u3BAQEAAACCIP+vbkhAAQAAAO8GECAAASFhMcYAAAAASUVORK5CYII=";

async fn ai_camera_runner(ctx: &TaskContext, state: &mut RunnerState) -> RunnerReport {
    let mut report = RunnerReport::default();
    // ⚠ 旧 TS 从 `jwtToken` 解 `userDomainId`（`parseJwtUserDomainId`）。本仓任务层
    // 拿不到那张票，但账号上落着一份**同值**的 `user_domain_id`
    //（登录响应里的 `userInfo.userDomainId`，与请求头 `x-yun-uni` 同值）⇒ 先用它。
    //
    // ⚠⚠ **PC 端登录的账号没有这一格**（`user_domain_id` 是 App 登录才落的；
    // 真机实测：`移动` 那个账号就没有 ⇒ 这一条以前**永远**报"缺少用户信息" ✗）。
    // 兜底走 **1028 已经在用的那条派生链**（`disk:queryUserToken` → `userDomainId` ✓）
    // —— 同一个账号、同一个用户域，只是取的路径不同 ✓。
    let user_id = match ctx.account.user_domain_id.clone() {
        Some(v) => v,
        None => match derive_user_domain_from_mail(ctx, state).await {
            Ok(v) => v,
            Err(e) => {
                return report.note(format!(
                    "AI相机任务失败: 缺少用户信息（账号没记 userDomainId，邮箱侧也没取到：{e}）"
                ))
            }
        },
    };

    // ① 识图。
    let mut rng = OsRandom;
    let tid = random_hex_with(&mut rng, 8);
    // ⚠ `deviceid` 要的是**本机 smid 换回来那一份**（旧 TS `ctx.http` 按域名注入的
    // 就是 `getDeviceId()` 那个 ✓），不是登录时冒充 PC 客户端那一份 ——
    // 两者**不是同一个东西**（见 `signin::h5_device_id` 的说明）。
    let device_id = crate::signin::h5_device_id(ctx, None)
        .await
        .unwrap_or_else(|| ctx.account.device_id.as_str().to_string());
    let req = ai_cloud::ai_recognize_request(
        // ⚠⚠ 识图这一枪的 `Authorization` 是**完整 auth**（`"Basic …"` 那一串），
        // 不是裸 token —— 2026-10-07 真机对拍过：裸 token ⇒
        // `{"code":"01000002","message":"鉴权失败"}`，而完整 auth ⇒ `success:true`。
        //
        // 为什么代码读起来像"与旧 TS 不一致"：旧 TS 那一句写的是
        // `Authorization: ctx.config.token`，但它的 `ctx.http` 会按域名
        //（`AUTH_DOMAINS` 命中 `yun.139.com`）**再注入一次** authorization ——
        // 抓包抓到的实发头就是 `Basic …`（完整串）✓。本仓不走那层注入
        //（这几台主机不吃云盘那套按域名注入 ✓）⇒ 由这里显式给对 ✓。
        &ctx.account.auth.raw,
        &user_id,
        AI_CAMERA_SAMPLE_BASE64,
        &tid,
        &device_id,
    );
    let rec = match send(ctx, req, "AI 相机识图（aiRecognize）").await {
        Ok(body) => match ai_cloud::parse_ai_recognize(&body) {
            Ok(r) => r,
            Err(e) => return report.note(format!("AI 相机识图响应读不出：{e}")),
        },
        Err(e) => return report.note(format!("AI 相机识图请求失败：{e}")),
    };
    if !rec.success {
        return report.note(format!(
            "AI相机识图失败: {}",
            rec.message.unwrap_or_else(|| "未知错误".into())
        ));
    }
    let Some(file_id) = rec.file_id.clone() else {
        return report.note("AI相机识图失败: 缺少文件ID");
    };

    // ② 文件名 = `${parseInt(taskId) + 1}.jpeg`（旧 TS 那一行逐字）。
    let task_id_num: i64 = rec
        .task_id
        .clone()
        .unwrap_or_default()
        .parse()
        .unwrap_or(ctx.clock.now_ms());
    let file_name = format!("{}.jpeg", task_id_num + 1);
    // 旧 TS 用的是 `new Date().toISOString()` ⇒ **UTC** 带毫秒的 ISO 串。
    let input_time = mcloud_core::utc_format(&*ctx.clock, "%Y-%m-%dT%H:%M:%S%.3fZ");
    let payload = ai_cloud::ai_camera_chat_payload(&user_id, &file_id, &file_name, &input_time);
    let tid = random_hex_with(&mut rng, 8);
    let req = ai_cloud::ai_camera_chat_request(&ctx.account.auth.raw, &payload, &tid);
    match send(ctx, req, "AI 相机对话（chat/v2/add）").await {
        Ok(body) => {
            let last = ai_cloud::parse_last_sse_data(&body);
            if ai_cloud::is_ai_chat_success(&last) {
                report.acted = true;
                report.notes.push("AI相机任务完成".into());
            } else {
                report.notes.push("AI相机对话失败".into());
            }
        }
        Err(e) => report.notes.push(format!("AI相机对话失败: {e}")),
    }
    report
}

/// 东八区墙钟按 `chrono` 格式串输出（`%Y-%m-%d %H:%M:%S` 之类）。
///
/// ⚠ 格式化收在 `mcloud-core`（`chrono` 只有它引得到 ✓，见
/// [`mcloud_core::format_with_offset`] 的说明）—— 任务层不自己算历法。
fn beijing_stamp(ctx: &TaskContext, fmt: &str) -> String {
    mcloud_core::beijing_format(&*ctx.clock, fmt)
}

/// `randomHex(n)`（2n 个字符）。
fn random_hex_with(rng: &mut impl Random, n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
