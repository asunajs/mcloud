//! `app-task` 写的那一半：「把 App 任务矩阵里能做的做掉」。
//!
//! # ⚠ 这条能力**会写**
//!
//! 它每一发 `task/click` 都在告诉上游"这条我做完了"，而且**顺带往用户网盘传文件**
//! （玩法 106 / 522 / beiyong1 的名字门）。`task/click` 不可回滚；
//! 传上去的文件本仓**会自己收尾**（移进回收站 —— 与 `task-expansion` 同一取向，
//! 那里是对参考实现的刻意改进，理由见 `task_expansion.rs` 文件头）。
//!
//! # 为什么和 `app-task-matrix` **不是同一个 code**
//!
//! 本仓 `app-task-matrix` 是「只看」的只读那一半。写的那一半用**另一个 code**
//! （就是本模块）——拿一个 code 做两件事会让它在两处指不同的东西
//! ⇒ 只读那一半叫 `app-task-matrix`，写的那一半叫 `app-task`。
//!
//! # 判据做成**纯函数**
//!
//! 「这一条要不要传、传几个」全部落在 [`upload_shape_for`] 这个纯函数里，
//! 于是它能**在没有网络的情况下被钉住**。真发的枪在后面的执行层。
//!
//! 依据是 **`mcloud-sign`**（那份 TS 是**能跑**的，行为以它为准）：
//! `packages/core/src/tasks/app/runners.ts` 的 `106` / `522` 两个 runner，
//! 以及 `tasks.ts:68-77` 那个 `switch (task.groupid)` 的 `beiyong1` 分支。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_cloud_outlink as outlink;
use mcloud_upstream::app_cloud_upload as upload;
use mcloud_upstream::app_cloud_upload::{CloudChannel, CloudFileCreateInput, CloudOpType};
use mcloud_upstream::app_task_click;
use mcloud_upstream::app_task_list_v2::TaskV2Item;
use mcloud_upstream::cloud_trash;

use crate::app_task_matrix::{self, APP_TASK_MARKETS};

/// 「这一条要往网盘传文件」的参数集。
///
/// ⚠ **零值不成一档** ⇒ "要不要传"由 `Option` 表达，结构体内部不设 `None` 那样的变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadShape {
    /// **最多**传几个。
    pub rounds: u32,
    /// `> 0` ⇒ 每传一个之前先看条目自己的 `process`，达到就停。
    ///
    /// ⚠ 这一格读的 `process` **语义未验证**（实拍里三条恒 0，而同批条目全 `FINISH`）。
    /// 用它的理由不是"它靠谱"，是"**旧实现读它**"；而真正兜底的是 [`Self::rounds`] ——
    /// 就算 `process` 永远读到 0，这一趟也最多传 5 个，不会变成"连传 100 个"。
    pub stop_at_process: i64,
    /// **每传完一个**等多少毫秒（`0` = 不踱步）。
    ///
    /// ⚠ 它是 522 与 106 **唯一的形状差别**（旧实现 106 的 runner 体里没有这一句）。
    /// 把两者混成一个的后果是：要么 522 变成"连传 5 个零间隔"，要么 106 白等一拍
    /// —— 两种都**不会报错**，只在"哪天被判成脚本"那一类最难查的差异上显形。
    ///
    /// ⚠ 它读的是**每一族自己的数**（522 = `runners.ts:117-125` 的 `sleep(500)`、
    /// 615 = `ai-tasks.ts:203-210` 的 `sleep(300)` ⇒ **500 ≠ 300** ✗）。
    /// 原先这一格是 `bool`（"踱/不踱"）、时长借用任务的 `pace_ms` ⇒ 615 会白多等 200ms/张 ✗。
    /// 做成毫秒数之后，两族的差别**在形状里就写着** ✓。
    pub per_round_ms: i64,
    /// `true` ⇒ 把这一趟的上限收成 **`stop_at_process - process`** ✓。
    ///
    /// ⚠ 它与 522 **不是一回事**，这一格就是两族的差别：
    /// * **522**（`runners.ts:117-125`）：`for (i < 5) { if (task.process >= 100) break; ... }`
    ///   —— 那个 `task.process` 是**动手前的快照**、循环里**不会变** ⇒ 它只当"闸"用，
    ///   上限仍是 `rounds`（5）✓；
    /// * **615 那一族**（`ai-tasks.ts:192-203`）：`remaining = Math.max(0, 10 - (task.process || 0))`
    ///   ⇒ **先算剩余再循环**，上限就是"还差几张" ✓。
    ///
    /// 把 615 也照 522 那样跑，`process=6` 时会**白传 6 张**（实拍 614 正是 `process=6` ✗）。
    pub top_up_from_process: bool,
    /// 造出来的文件名后缀（旧实现的 `uploadRandomFile({ ext })` ✓）。
    ///
    /// ⚠ 「传 10 张」那一族传的是 **`.jpg`**（`ai-tasks.ts:205`）—— 而别处没有 `ext`，
    /// 走 [`random_plan`] 的缺省 `.bin` ✓。上游到底看不看这个后缀**未验证**，
    /// 但旧实现刻意分开写 ⇒ 照抄，不替它统一 ✗。
    pub file_ext: &'static str,
    /// 这一档按**哪个客户端**发 —— 落到 `x-yun-app-channel` 那一枚头（以及各档专有的几枚）✓。
    ///
    /// ⚠ 玩法 113 的**全部意义**就在这一格：它叫「**使用PC客户端**」——
    /// 按安卓档传等于没做 ✓（旧 TS `runners.ts:27-31` 里只有它一个人传了
    /// `channelSrc: "10200153"`，别处都是缺省的安卓档 ✓）。
    pub channel: CloudChannel,
}

/// 「就传一个」那一档（玩法 106 与 beiyong1 的名字门共用）。
///
/// 单独一个常量是为了让"106 与 beiyong1 走的是**同一档**形状"这件事可断言 ——
/// 旧 TS 两处都是裸的 `uploadRandomFile(ctx)`，没有任何参数差别。
pub const UPLOAD_ONE: UploadShape = UploadShape {
    rounds: 1,
    stop_at_process: 0,
    top_up_from_process: false,
    per_round_ms: 0,
    file_ext: ".bin",
    channel: CloudChannel::Android,
};

/// 522 那一趟最多传几个（旧实现的 `每月上传任务单日数量 ?? 5` 里那个默认值）。
///
/// ⚠ 本仓**没有**"每月上传任务单日数量"这一格配置（旧实现是可配的）⇒ 取旧实现那个
/// **默认值 5**。要开成参数面得连 CLI 与 WebUI 一起改，不在这一层单方面决定。
pub const ROUNDS_PER_RUN: u32 = 5;

/// 玩法 522 的 `process` 门槛（旧实现那个 `>= 100`）。
pub const STOP_AT_PROCESS: i64 = 100;

/// 玩法 113 的 id —— 「**使用PC客户端**」（旧 TS `runners.ts:27-31`）。
pub const PC_UPLOAD_TASK_ID: i64 = 113;

/// 「**传 10 张**」那一族（旧 TS `ai-tasks.ts:190-224` 的 `upload10PictureTask`）✓。
///
/// ```ts
/// const targetCount = 10;
/// const remaining = Math.max(0, targetCount - (task.process || 0));
/// if (remaining === 0) return true;
/// for (let i = 0; i < remaining; i++) {
///   const uploaded = await uploadRandomFile({ ext: ".jpg" });
///   if (uploaded) success++;
///   await sleep(300);
/// }
/// ```
///
/// ⇒ 形状 = **最多 10 张**、`process` 到 10 就停、每张 **300ms**、后缀 **`.jpg`** ✓
///（`rounds: 10` + `stop_at_process: 10` 合起来正是那句 `remaining = 10 - process` ✓；
/// 而 `process` 读不出（当 0）时也最多 10 张 ⇒ 不会失控 ✓）。
///
/// ⚠ **门不是 id**：这一族是**节日任务**，每期换 id（实拍 `id=614`「上传中秋团圆照」、
/// 真机 `id=615`「记录国庆节欢乐瞬间」，两者 `description` 都是「上传10张可领」、
/// `groupid` 都是 `beiyong1` ✓）⇒ 认它靠 [`beiyong1_button_gate`]，不是靠 id ✗。
pub const UPLOAD_TEN: UploadShape = UploadShape {
    rounds: 10,
    stop_at_process: 10,
    top_up_from_process: true,
    per_round_ms: 300,
    file_ext: ".jpg",
    channel: CloudChannel::Android,
};

/// `beiyong1` 分组的**按钮门**（旧 TS `tasks.ts:78-81` 逐字）：
///
/// ```ts
/// if ((task.button.app as Beiyong1App)?.link?.includes("uploadView")) {
///   await upload10PictureTask(task);
///   return;
/// }
/// ```
///
/// 实拍的 `link` 是 `mcloud://main/uploadView`（抓包仓
/// `har/2026-09-25-通知有礼.har` ✓）⇒ 这里照抄那句"**包含** `uploadView`"，
/// **不**写死整串（文案/路径将来变了才不会静默失效 ✓）。
///
/// ⚠ 名字门 [`beiyong1_name_gate`] **排在它前面**（旧 TS 就是先 `if (名字门) {...return}`）⇒
/// 两条都命中时走**一张**那一档 ✓。
pub fn beiyong1_button_gate(link: Option<&str>) -> bool {
    link.is_some_and(|l| l.contains("uploadView"))
}

/// 玩法 113 那一档：形状与 [`UPLOAD_ONE`] **完全一样**，差别**只在渠道头** ✓。
///
/// # 旧 TS `runners.ts:27-31` 逐字
///
/// ```ts
/// 113: async () => {
///   await refreshToken();
///   await sleep(1000);
///   await uploadRandomFile({ channelSrc: "10200153" });
/// },
/// ```
///
/// ⇒ 按 PC 档传（`channelSrc: "10200153"` = [`mcloud_upstream::app_cloud_upload::CLOUD_CHANNEL_PC`] ✓）
/// 就是这条任务的本体：它奖励的是"**用 PC 客户端**" ✓。落到头上是
/// `x-yun-app-channel: 10200153` + `x-yun-market-source` / `x-yun-module-type` /
/// `x-yun-svc-type` 那三枚，而**没有** `x-huawei-channelSrc`、**没有** `x-yun-sub-op-type`
/// —— 那两枚是安卓档才有的 ✓（见 `cloud_file_op_headers`）。
///
/// # ⚠ 前面那句 `refreshToken()` 本仓**不做**
///
/// 它打的是 `aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do`（XML 体、`clienttype=656`），
/// 而本仓 `mcloud-upstream::auth_refresh` 的模块文档已经判定过：**那条零抓包 ⇒ 不实现**，
/// 只走实拍到的 `/user/auth/refreshToken` ✓。并且：那条刷新与"113 算不算做成"之间
/// **没有任何可证的因果**，而"拿回来的新 token 又丢掉"（旧 TS 的调用方确实不看返回值）
/// 还有**把账号的 token 搞坏**的风险 ✗ ⇒ 宁可不做这一句，等有实拍再说 ✓。
pub const UPLOAD_ONE_PC: UploadShape = UploadShape {
    rounds: 1,
    stop_at_process: 0,
    top_up_from_process: false,
    per_round_ms: 0,
    file_ext: ".bin",
    channel: CloudChannel::Pc,
};

/// beiyong1 分组的名字门（旧实现逐字）：
/// `name.includes("上传") && (name.includes("图") || name.includes("照"))`。
///
/// ⚠ 旧实现同一支里**还有一句**看 `button.app.link` 含 `uploadView` ⇒ 走"补满 10 张"
/// 那条（`ai-tasks.ts` 的 `upload10PictureTask`）。**本模块不实现那一支**：
/// 我们的 `TaskV2Item` 里没有 `button` 那一格（解析器没读它），而"10 张"那条玩法
/// 的产物链（`aiRecognize` / `chat/v2/add` SSE）本仓**零实拍**。
/// ⇒ 留空并注明，不编一个"看起来合理"的入口。
/// 上游 `name` 里**真的会带 HTML** ✗。
///
/// 实拍（2026-10-04，用户报「app-task 存在 bug，无法正常使用」）：
///
/// ```text
/// 分享文件领好礼<span id='share_title'>0/7</span>
/// ```
///
/// ⚠ 这个 `name` **只用于展示与记录**（发给上游的那几枪用的是 `task_id` ✓，
/// 见 `TaskStep` 的字段说明），所以就地在**进入记录之前**清干净 ✓。不清的话
/// 同一串脏东西会脏在三个地方：进度日志 ✓、`details.reported.steps[].name` ✓、
/// 以及界面上 ✓ —— 而 `format!` 出来的 `<span>` 在界面上是**看不见**的 ✗，
/// 用户只会觉得"任务名怪怪的"却说不出哪里怪 ✓。
///
/// 只做两件事：去掉 `<…>` 段 ✓、把连续空白压成一个空格 ✓。
///
/// **刻意不做** HTML 实体解码（`&amp;` → `&` 之类）：上游给的是**标签** ✓，
/// 实体没实测到 ✓；而无脑解码会把本来正确的文本改坏、且不可逆 ✗ ——
/// 这属于"没证据的加工"✓。
fn clean_task_name(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            // ⚠ 只有**真的能找到配对的 `>`** 才当标签去掉 ✓。
            // 找不到的那种（`a < b 上传` ✓，上游偶尔会这么给）就**原样留着** ——
            // 宁可显示得丑一点，也**不能静默吞掉后面的正文** ✗
            // （这是本仓最忌讳的"不发错、不报错，只是东西不对"）。
            if let Some(close) = chars[i + 1..].iter().position(|c| *c == '>') {
                i += close + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    // 去标签可能留下多余空白（`名称 <span>…</span>` ⇒ `名称 ` ✓）
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn beiyong1_name_gate(name: &str) -> bool {
    name.contains("上传") && (name.contains("图") || name.contains("照"))
}

/// 这一条要不要传、按哪一档传。判不了（不传）⇒ `None`。
///
/// 三个来源逐条对着旧实现：
///
/// | 命中 | 档 | 旧实现出处 |
/// |---|---|---|
/// | `task_id == 106` | [`upload_one`] | `runners.ts:41-44` 的 `106` runner |
/// | `task_id == 522` | `{5, 100, pace}` | `runners.ts:117-125` 的循环 |
/// | `group_id == "beiyong1"` 且过名字门 | [`upload_one`] | `tasks.ts:68-77` |
///
/// ⚠ 三条是**并列**的，不是"先按 id 再看分组"：旧实现里 106 / 522 走 runner 表、
/// beiyong1 走 `switch (task.groupid)`，两条路互不遮罩 —— 一个 beiyong1 分组里
/// 的条目也可能正好 `task_id == 106`，那时两边给的**是同一档**（都是 `upload_one`），
/// 所以先判哪一条都不改结果。这一点由测试钉住，免得将来有人"顺手"给 beiyong1 换一档。
pub fn upload_shape_for(item: &TaskV2Item) -> Option<UploadShape> {
    match item.task_id {
        // 113「使用PC客户端」：形状与 106 同档，**差别只在渠道头**（PC 档 ✓）。
        Some(PC_UPLOAD_TASK_ID) => return Some(UPLOAD_ONE_PC),
        Some(106) => return Some(UPLOAD_ONE),
        Some(522) => {
            return Some(UploadShape {
                rounds: ROUNDS_PER_RUN,
                stop_at_process: STOP_AT_PROCESS,
                // ⚠ 522 是"闸"不是"剩余量"：那个 `process` 在循环里不会变 ✓
                top_up_from_process: false,
                per_round_ms: 500,
                file_ext: ".bin",
                channel: CloudChannel::Android,
            });
        }
        _ => {}
    }
    if item.group_id.as_deref() == Some("beiyong1") {
        // ① 名字门：`name` 里有"上传"且有"图"/"照" ⇒ **一张**（`tasks.ts:68-77` ✓）。
        if let Some(name) = item.name.as_deref() {
            if beiyong1_name_gate(name) {
                return Some(UPLOAD_ONE);
            }
        }
        // ② 按钮门：名字门没中才看它（旧 TS 的顺序 ✓）⇒ **传 10 张**那一族
        //（`tasks.ts:78-81` ✓）。⚠ 这一格是刚接上的：615「记录国庆节欢乐瞬间」的
        // 名字里既没有"上传"也没有"图/照" ⇒ 主跟它没关系，全靠 `button.app.link` ✓。
        if beiyong1_button_gate(item.button_app_link.as_deref()) {
            return Some(UPLOAD_TEN);
        }
    }
    None
}

/// 玩法 434 的 id（分享文件）。
pub const SHARE_TASK_ID: i64 = 434;

/// 从条目名字里解出分享进度 `(已分享几次, 共几次)`。
///
/// # 活体形状（run 93 / run 99 实测）
///
/// ```text
/// 分享文件领好礼<span id='share_title'>0/7</span>
/// ```
///
/// # 为什么这样解，而不是照抄旧 TS 的 `replace`
///
/// 旧 TS 的 `getShareCount`（`services/app/tasks.ts:113-115`）是按**固定前后缀**做
/// `replace` 的：`"分享文件得云朵<span id='share_title'>"` 与 `"/7</span>"` ⇒ 只要那个
/// "已做"的数 ✓。那份能跑，但它把**活动文案**（"得云朵"、"/7"）写死在代码里 ✗，
/// 文案一换就解出 `NaN` ⇒ `NaN > x` 恒为 `false` ⇒ **静默地永远不报成功** ✗。
///
/// ⇒ 本仓按**结构**解，容忍文案变化：
/// ① 先取 `share_title` 那一段的文本，在里面找 `a/b`；
/// ② 取不到就在整串里找**第一个** `a/b`（上游偶尔不带那个 span ✓）；
/// ③ 都取不到 ⇒ `None` —— **"读不出"不是 0** ✓（当成 0 会把"没涨"判反 ✗）。
pub fn share_progress_from_name(name: &str) -> Option<(i64, i64)> {
    share_title_text(name)
        .and_then(parse_ratio)
        .or_else(|| parse_ratio(name))
}

/// `share_title` 那个标签**里面**的文本（不含标签本身）。
fn share_title_text(name: &str) -> Option<&str> {
    let at = name.find("share_title")?;
    let after = &name[at + "share_title".len()..];
    let text = &after[after.find('>')? + 1..];
    let end = text.find('<').unwrap_or(text.len());
    Some(text[..end].trim())
}

/// 取**第一个** `数字/数字`（返回 `(前, 后)`）。
///
/// 刻意只认 ASCII 数字与 `/`：这是上游自己给的形状（`0/7`）✓，不去猜
/// "共 7 次"中文写法之类的东西（没实拍 ✗）。
fn parse_ratio(s: &str) -> Option<(i64, i64)> {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if !cs[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < cs.len() && cs[i].is_ascii_digit() {
            i += 1;
        }
        if i < cs.len() && cs[i] == '/' {
            let mut j = i + 1;
            while j < cs.len() && cs[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 {
                let a: String = cs[start..i].iter().collect();
                let b: String = cs[i + 1..j].iter().collect();
                if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                    return Some((a, b));
                }
            }
        }
    }
    None
}

/// 收尾的**事后读数**：整张表再拉一遍，把"跑之前记下的数"和"跑之后的数"对上 ✓。
///
/// 一次拉表服务**两档**（都靠"数字"说话，都是官方没给"完成没完成"接口的那种）：
///
/// * **434** —— 分享次数 `shareBefore → shareAfter`，`after > before` 才算记上 ✓；
/// * **522** —— 上传**进度** `42/100 → 47/100` ✓ —— 用户口径 2026-10-04：
///   「毕竟需要 100 个，也不急着一次做完，**进度还是很重要的**」✓。
///   这一档一趟只传 [`ROUNDS_PER_RUN`] 个，光看 `uploaded:5` 根本看不出走到哪儿了 ✗。
///
/// 一次拉表而不是每档各拉一趟：同一份读数、同一个时刻 ✓（也省 16 发 GET ✓）。
async fn settle_progress(ctx: &TaskContext, result: &mut AppTaskResult) {
    let share_i = result
        .steps
        .iter()
        .position(|s| s.task_id == Some(SHARE_TASK_ID));
    // ⚠ 找的是"**报了进度的那些档**"（522 / 615 那一族…），不是写死 522 的 id ✓
    let upload_i = result.steps.iter().position(|s| s.progress_total.is_some());
    if share_i.is_none() && upload_i.is_none() {
        return; // 这两档都没做 ⇒ 那一趟"事后整表"压根不发 ✓
    }
    let Some(items) = refetch_items(ctx).await else {
        for i in [share_i, upload_i].into_iter().flatten() {
            result.steps[i]
                .problems
                .push("跑完拉表读不出数 ⇒ 保守当作没记上".to_string());
        }
        return;
    };

    if let Some(i) = share_i {
        settle_share(&items, result, i, ctx);
    }
    if let Some(i) = upload_i {
        settle_upload(&items, result, i, ctx);
    }
}

/// 434 那一格的落地：比次数、决定要不要把这一条计进 `acted` ✓。
fn settle_share(items: &[TaskV2Item], result: &mut AppTaskResult, i: usize, ctx: &TaskContext) {
    let Some(after) = items
        .iter()
        .find(|it| it.task_id == Some(SHARE_TASK_ID))
        .and_then(|it| it.name.as_deref())
        .and_then(share_progress_from_name)
    else {
        result.steps[i]
            .problems
            .push("分享后读不出次数（表里没这条 / 名字里没有 a/b）⇒ 保守当作没记上".to_string());
        return;
    };
    result.steps[i].share_after = Some(format!("{}/{}", after.0, after.1));
    let before = result.steps[i]
        .share_before
        .as_deref()
        .and_then(parse_ratio);
    let name = result.steps[i].name.clone();
    match before {
        Some((was, total)) if after.0 > was => {
            result.steps[i].share_counted = true;
            result.acted += 1;
            // 旧 TS 在这里打的就是 success（`printShareProgress` ✓）—— 且**只在涨了时**打 ✓
            ctx.success(format!(
                "{name}：{was}/{total} → {}/{}（记上了）",
                after.0, after.1
            ));
        }
        Some((was, total)) => {
            result.steps[i].problems.push(format!(
                "分享后上游次数没动（{was}/{total} → {}/{}）⇒ 这一条不算记上",
                after.0, after.1
            ));
            ctx.fail(format!(
                "{name}：{was}/{total} → {}/{}（上游次数没动 ⇒ 不算记上）",
                after.0, after.1
            ));
        }
        None => {
            result.steps[i].problems.push(format!(
                "动手前读不出次数（名字里没有 a/b）⇒ 无从比较，保守当作没记上（分享后 {}/{}）",
                after.0, after.1
            ));
            ctx.fail(format!(
                "{name}：动手前读不出次数 ⇒ 无从比较（分享后 {}/{}）",
                after.0, after.1
            ));
        }
    }
}

/// 522 那一格的落地：把**进度**写进明细、并打一行日志 ✓。
///
/// ⚠ 这一档**不**因为进度没动就扣 `acted` —— 它的"成"是**传了文件**（那一支自己计 ✓），
/// 进度只是给用户看的"走到哪儿了" ✓（用户口径：不急着一次做完 ✓）。
/// 但**读不出**与**没动**要分开说：前者是"我们没读到"，后者是"上游还没记账" ✗。
fn settle_upload(items: &[TaskV2Item], result: &mut AppTaskResult, i: usize, ctx: &TaskContext) {
    let Some(total) = result.steps[i].progress_total else {
        return;
    };
    let Some(id) = result.steps[i].task_id else {
        return;
    };
    let Some(item) = items.iter().find(|it| it.task_id == Some(id)) else {
        result.steps[i]
            .problems
            .push("跑完拉表里没有这一条 ⇒ 读不出进度".to_string());
        return;
    };
    let after = item.process;
    result.steps[i].progress_after = after.map(|p| format!("{p}/{total}"));
    let name = result.steps[i].name.clone();
    let uploaded = result.steps[i].uploaded;
    let before = result.steps[i]
        .progress_before
        .as_deref()
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse::<i64>().ok());
    match (before, after) {
        (Some(b), Some(a)) if a > b => {
            let moved = a - b;
            // ⚠ 差额要**说出来**，别让两个数字自己打架 ✗：实测"我们传了 5 个、上游只 +4"
            // 是**记账慢一步**（真的），下一趟的"动手前"读数正好补上那一个 ——
            // run 105 `48 → 52`（传 5）⇒ run 106 的动手前读数是 **53** ✓ 再 `53 → 58`（传 5 ✓）。
            // ⇒ 最多差**最后一个**，不是 106 那种 76 分钟的延迟 ✓。
            let gap = if moved == i64::from(uploaded) {
                String::new()
            } else {
                format!(
                    "；我们实际传了 {uploaded} 个，差的 {} 个上游还没记账 ✓",
                    i64::from(uploaded) - moved
                )
            };
            let tail = if a >= total { "，满了 ✓" } else { "" };
            ctx.success(format!(
                "{name}：{b}/{total} → {a}/{total}（这一趟 +{moved}{gap}）{tail}"
            ));
        }
        (Some(b), Some(a)) => ctx.info(format!(
            "{name}：{b}/{total} → {a}/{total}\
             （这一趟传了 {uploaded} 个，上游计数还没动 ⇒ 可能还没记账）"
        )),
        (_, Some(a)) => ctx.info(format!(
            "{name}：{a}/{total}（动手前没读到，没法比；这一趟传了 {uploaded} 个）"
        )),
        (_, None) => result.steps[i]
            .problems
            .push("这一条没有 process 这一格 ⇒ 读不出进度".to_string()),
    }
}

/// 把整张表再拉一遍、去重后交回条目；**16 次全失败** ⇒ `None`（"读不出" ✓）。
///
/// # 为什么是**整张表**
///
/// 旧 TS `app.ts:56` 就是 `await getAllAppTaskList()`（全表）之后再比大小 ✓ ——
/// 本仓照抄这条口径，而不是去猜它这次落在了哪一组（条目会换组、组名也会换 ✗）。
/// 代价是 16 发只读 GET（与开跑前那次同量 ✓，实测整表读一次约 1.4 s ✓）。
async fn refetch_items(ctx: &TaskContext) -> Option<Vec<TaskV2Item>> {
    let mut pages: Vec<mcloud_upstream::app_task_list_v2::TaskListV2> = Vec::new();
    for market in app_task_matrix::APP_TASK_MARKETS {
        for group in mcloud_upstream::app_task_list_v2::TASK_LIST_V2_GROUPS {
            // 单组失败不炸整件事（与只读矩阵同口径 ✓），只是少一页证据。
            if let Ok(p) = app_task_matrix::fetch_one(ctx, market, group).await {
                pages.push(p);
            }
        }
    }
    if pages.is_empty() {
        // 16 发**全**没成 ⇒ "读不出"（这与"拉到了、只是表里没这条"是两回事 ✗）
        return None;
    }
    let (items, _) = app_task_matrix::merge_pages(&pages);
    Some(items)
}

// ══════════════════════ 三道门（第一道：刻意跳过的 id） ══════════════════════

/// 旧实现刻意**跳过**的任务 id 名单（**逐字照抄，不增删**）。
///
/// # 依据
///
/// `mcloud-sign` 的 `packages/core/src/tasks/app/tasks.ts:111`：
///
/// ```ts
/// export const SKIP_TASK_LIST = [117, 437, 991, 406, 385, 480, 1022, 1018, 603, 1021, 120];
/// ```
///
/// 用法在**同文件** `tasks.ts:101-103`（`default:` 分支）：
///
/// ```ts
/// if (!SKIP_TASK_LIST.includes(task.id)) {
///   await clickTask(ctx, task.id);
/// }
/// ```
///
/// # 为什么这些 id 要跳过
///
/// TS 给这一串只写了「需要跳过的任务」一句注释（`tasks.ts:108-110`），
/// **没有逐条列理由** ⇒ 本仓也不替它编。可以确定的是：这些 id 的
/// `task/click` **有副作用或没有意义**（那一发**不可回滚**），所以 TS
/// 刻意不对它们打卡 —— 我们照抄，不去"帮它补上"。
///
/// ⚠ 任务书写的是「12 个 id」，而 TS 原文只有 **11 个**（逐字数过，见测试）。
/// 这里以**文件原文**为准：照抄，不拿任务书里的数目去增删。
pub const SKIP_TASK_LIST: [i64; 11] = [117, 437, 991, 406, 385, 480, 1022, 1018, 603, 1021, 120];

/// 这一条是否在那串刻意跳过的 id 里。
///
/// ⚠ `task_id` 缺席（`None`）**不算**命中：旧 TS 那句读的是 `task.id`，
/// 而 `undefined.includes(...)` 在它那边是 `false` —— 与这里同向。
pub fn is_skipped_task(task_id: Option<i64>) -> bool {
    matches!(task_id, Some(id) if SKIP_TASK_LIST.contains(&id))
}

// ══════════════════════ 第二道门：本账号配置的跳过名单 ══════════════════════

/// 这一条是否在**本账号**配置的跳过名单里。
///
/// # 依据（旧 TS `index.ts:33` 逐字）
///
/// ```ts
/// if (ctx.config.tasks?.skipTasks?.includes(task.id)) continue;
/// ```
///
/// 名单是**每账号**的（`ctx.config` 属于当前账号）—— 本仓落在
/// `AccountSettings::skip_tasks`（`Vec<i64>`，与 TS 的 `number[]` 同形）。
pub fn is_configured_skip(item: &TaskV2Item, skip_tasks: &[i64]) -> bool {
    matches!(item.task_id, Some(id) if skip_tasks.contains(&id))
}

/// 这一条要做的**那一件事**。
///
/// ⚠ 做成枚举而不是两个 `Option`：这两件事**互斥**（434 不传文件、106/522 不分享），
/// 而两个独立 `Option` 允许"两个都填"那种表达不出来的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 往网盘传文件（106 / 522 / beiyong1 名字门）。
    Upload(UploadShape),
    /// 把这趟传上去的**第一个**文件分享出去、再立刻删掉外链（434）。
    Share,
    /// 「从固定入口进一次」的埋点 + 补一发打卡（409 / 1005）。
    ///
    /// ⚠ 与另外两档不同的地方：它**额外再打一次卡** —— TS 在这两个 runner 内部
    /// 各自 `clickTask` 了一发（`runners.ts:107` / `:133`），而 `handleTaskClick`
    /// 那发是按步数打的 ⇒ **两发都要** ✓。key 由 [`click_key`] 给
    ///（409 → `task2` ✓，1005 → 默认 `task` ✓）。
    Entry,
}

/// 这一条该做哪件事。判不出来 ⇒ `None`（**什么都不做**）。
///
/// 两处出处都是旧 TS：
///
/// * 上传那三档见 [`upload_shape_for`]（`runners.ts` 的 106 / 522、`tasks.ts` 的 beiyong1）
/// * 434 见 `runners.ts:68-88`
pub fn step_for(item: &TaskV2Item) -> Option<Step> {
    // 409 / 1005：两条"从固定入口访问云朵中心"（`runners.ts:98-108` / `:126-133`）。
    if matches!(item.task_id, Some(409 | 1005)) {
        return Some(Step::Entry);
    }
    if item.task_id == Some(SHARE_TASK_ID) {
        return Some(Step::Share);
    }
    upload_shape_for(item).map(Step::Upload)
}

/// 这一条**要不要动**。
///
/// # 旧 TS `index.ts:34` 逐字
///
/// ```ts
/// if (task.state === "FINISH" || task.enable !== 1) continue;
/// ```
///
/// 两道门，**都在这一个 `continue` 里**：
///
/// * `state == "FINISH"` ⇒ 已经做完了，别再去写一遍。
/// * `enable != 1` ⇒ 上游把这一条关了。⚠ `enable` **缺席也算关**
///   （`None != Some(1)`）—— 这一格既不是 1，也就过不了那道门。
///   与只读那一半 `disabled_count` 的口径一致。
///
/// # 为什么这一条对"会写的链"特别要紧
///
/// 之前这里只判**形状**（`upload_shape_for`），不看状态 ⇒ 会对着**已完成的**
/// 条目也发 `task/click`（不可回滚）并往网盘传文件。真机读数是
/// **未完成 15 / 已完成 13** ⇒ 那 13 条里形状匹配的都会被误伤。
/// ⚠ 另有一道**同一层**的门（旧 TS 与 `state` 那句挨着）：
/// 命中 [`SKIP_TASK_LIST`] 的 id **一律不动** —— 它们点了有副作用/没意义
/// （见该常量的依据）。放在这里而不是只放在 [`should_click`] 里：黑名单
/// 条目**连形状都不该判进 `planned`**，否则摘要那句"判出 N 条"会把它算进去。
pub fn should_act(item: &TaskV2Item) -> bool {
    if is_skipped_task(item.task_id) {
        return false;
    }
    if item.state.as_deref() == Some(crate::app_task_matrix::STATE_DONE) {
        return false;
    }
    item.enable == Some(1)
}

/// 这一条**要不要打那一发 `task/click`**。
///
/// # 门是 `currstep`（旧 TS `tasks.ts:19-28` 逐字）
///
/// ```ts
/// const specialMap: Record<number, number> = { 434: 22 };
/// if (specialMap[id] && currstep === specialMap[id]) return clickTask(ctx, id);
/// if (currstep === 0) return clickTask(ctx, id);
/// return true;                       // ← 不打，但门**通过**
/// ```
///
/// ⇒ 两种"不打"要分开读：
///
/// * `currstep == 0`（不传这一格时 JS 的默认值就是 0）⇒ **打**。
/// * `currstep != 0`（这一条已经走起来了）⇒ **不打**，而门**照样通过**
///   —— `return true` 意味着 runner 还是会跑。
///
/// ⚠⚠ 434 那条 `specialMap` 是**叠加**在上面两句**之上**的，**不是替代** ✗：
/// 它在第 22 步要打，而"第 0 步 ⇒ 打"那一句对它**照样生效** ✓
///（参考实现的下标顺序就是先问 `specialMap`、再问 `currstep === 0`）。
/// 这一版最初把 434 写成了**替代**规则（`if 434 { return step == 22 }`）✗
/// ⇒ 上游不给 `currstep` 时（2026-10-04 小号 run 96 正是如此）它**一发都没打**
/// ✗，而界面那条自述还写着"按旧实现不打卡" —— 旧实现在第 0 步**恰恰会打** ✗。
///
/// ⚠ 这一版最初还是**无条件打卡**，于是对"已经走起来的"条目多发了一发写请求
/// （`task/click` 不可回滚）。真机第一趟那条
/// `522 "当月上传文件满100个" uploaded 0 clicked true` 就是它 ——
/// 上游本没打算让这一趟再记一笔，是我们自己补的。
/// ⚠ 打卡前**再过一次**黑名单：这一道与 [`should_act`] 里那道不是冗余 ——
/// 434 在第 22 步也要打卡，而"哪些 id 不能打"是**独立于步号**的一条，
/// 写在两处才挡得住"将来有人从别的入口直接调 `click`"。
pub fn should_click(item: &TaskV2Item) -> bool {
    if is_skipped_task(item.task_id) {
        return false;
    }
    let step = item.curr_step.unwrap_or(0);
    // 434：第 22 步是**额外**要打的一发（叠加）✓；其余一律落回下面的
    // "第 0 步 ⇒ 打"，所以 `currstep` 缺席（None）时它也**照打** ✓。
    if item.task_id == Some(SPECIAL_CLICK_TASK) && step == SPECIAL_CLICK_STEP {
        return true;
    }
    step == 0
}

/// `specialMap` 里那唯一一条：玩法 434 的外链分享。
pub const SPECIAL_CLICK_TASK: i64 = 434;
/// 玩法 434 打那一发打卡的步号。
pub const SPECIAL_CLICK_STEP: i64 = 22;

/// 这一趟还要不要再传一个（每轮开始前问一次）。
///
/// 两条停止条件：
/// * 已经传满 `rounds`
/// * `stop_at_process > 0` 且条目当前 `process >= stop_at_process`
///
/// ⚠ `process` 缺失（上游没给这一格）**当 0**，与旧实现读 `task.process` 拿到
/// `undefined` 的行为差别在于：JS 里 `undefined >= 100` 是 `false`（继续传），
/// 我们当 0 也是继续传 ⇒ **同向**。这一格由测试钉住。
pub fn should_upload_another(shape: UploadShape, done: u32, process: Option<i64>) -> bool {
    if done >= shape.rounds {
        return false;
    }
    if shape.stop_at_process > 0 && process.unwrap_or(0) >= shape.stop_at_process {
        return false;
    }
    true
}

// ══════════════════════ 第三道门：本账号当月有没有备份 ══════════════════════

/// 月份门里的市场名（旧 TS `tasks.ts:88` 的 `task.marketname === "sign_in_3"`）。
pub const MONTH_GATE_MARKET: &str = "sign_in_3";
/// 月份门只对 `month` 组生效（旧 TS `tasks.ts:86` 的 `case "month"`）。
pub const MONTH_GATE_GROUP: &str = "month";
/// 当月第几天**之前**跳过（旧 TS `tasks.ts:90` 的 `< 20`）。
pub const MONTH_GATE_DAY_LIMIT: i64 = 20;

/// `account_state` 里存"本账号本月备份读数"的键。
///
/// ⚠ **账号维度由 store 保证**：`account_state` 本身就是按账号 id 存的一张表
/// （[`mcloud_core::AccountStore::account_state`]），同一进程里 A、B 各读各的那一份
/// ⇒ 不可能串号。**月份**写进值的 `month` 字段：不写月份，上个月判出的
/// `false` 会被套到本月头上，把整组 `month` 任务白白跳过。
pub const MONTH_BACKUP_STATE_KEY: &str = "cur_month_backup";

/// 东八区墙钟的 `(yyyyMM, 日)`；时钟读不出 ⇒ `None`。
///
/// ⚠ 时间只能来自注入的 [`mcloud_core::Clock`]（`beijing_time_digits`），
/// 不许 `SystemTime::now()` —— 否则测试钉不住月份门。
/// 旧 TS 那句 `new Date().getDate()` 读的是**本机本地**日期；本仓按全仓惯例
/// 固定东八区（`mcloud_core::beijing_offset` 是"东八区"在本项目的唯一出处）。
pub(crate) fn beijing_month_day(clock: &dyn mcloud_core::Clock) -> Option<(String, i64)> {
    let digits = mcloud_core::beijing_time_digits(clock);
    if digits.len() < 8 {
        return None;
    }
    let month = digits.get(0..6)?.to_string();
    let day = digits.get(6..8)?.parse::<i64>().ok()?;
    Some((month, day))
}

/// 这一条**该不该因为"本月没开备份"而跳过**（旧 TS `tasks.ts:86-95` 逐字）：
///
/// ```ts
/// case "month": {
///   if (
///     task.marketname === "sign_in_3" &&
///     ctx.state.curMonthBackup === false &&
///     new Date().getDate() < 20
///   ) {
///     ctx.logger.info(`跳过任务（未开启备份）: ${task.name}`);
///     return;
///   }
/// }
/// ```
///
/// 三态读 `month_backup`：
/// * `Some(false)` ⇒ 确认本月没开备份 ⇒ **跳过**（日期 < 20 时）；
/// * `Some(true)` ⇒ 已开 ⇒ 不跳；
/// * `None` ⇒ 本月没记录 / 读失败 ⇒ **不跳**（把"不知道"当成"没开"就是
///   拿一次跳过补一个空洞，与 `task_expansion` 对 `cur_month_backup` 的三态口径同向）。
pub fn month_gate_skips(item: &TaskV2Item, month_backup: Option<bool>, day: Option<i64>) -> bool {
    let Some(day) = day else {
        // 时钟读不出 ⇒ 判不了"今天是不是 20 号前" ⇒ 不跳（保守）。
        return false;
    };
    item.group_id.as_deref() == Some(MONTH_GATE_GROUP)
        && item.market_name.as_deref() == Some(MONTH_GATE_MARKET)
        && month_backup == Some(false)
        && day < MONTH_GATE_DAY_LIMIT
}

/// 读本账号**当月**的备份读数。`None` = 没有本月的记录。
///
/// ⚠ 读失败（store 挂了 / 状态不是对象 / 月份对不上 / 缺 `backup` 这一格）
/// 一律回 `None`（"不知道"），**不回 `Some(false)`** —— 见 [`month_gate_skips`]。
pub async fn read_month_backup(ctx: &TaskContext, month: &str) -> Option<bool> {
    let state = ctx.store.account_state(&ctx.account.id).await.ok()?;
    let slot = state.get(MONTH_BACKUP_STATE_KEY)?;
    if slot.get("month")?.as_str()? != month {
        return None;
    }
    slot.get("backup")?.as_bool()
}

/// 把本账号**当月**的备份读数写进 `account_state`。
///
/// # 为什么是读-改-写
///
/// `save_account_state` 是**整块替换**（`crates/mcloud-store/src/json_store.rs:73`
/// 明写），而这张表里还躺着 `device_id` / `pc_platform`
/// （`crates/mcloud-tasks/src/login.rs:549-553`）这些**别的任务**的键。
/// 直接写一份只有我们这枚键的 Value 就会把它们抹掉。
/// ⇒ 先整份读回、只改自己那一枚键、再整份写回。
///
/// ⚠ 读不到（`Null`）或读出来不是对象 ⇒ 从空对象起一份新的：那是"这个账号
/// 还没有任何状态"，不是"要覆盖别人的"。
pub async fn save_month_backup(ctx: &TaskContext, month: &str, backup: bool) -> Result<()> {
    let mut state = ctx.store.account_state(&ctx.account.id).await?;
    if !state.is_object() {
        state = serde_json::json!({});
    }
    if let Some(obj) = state.as_object_mut() {
        obj.insert(
            MONTH_BACKUP_STATE_KEY.to_string(),
            serde_json::json!({ "month": month, "backup": backup }),
        );
    }
    ctx.store.save_account_state(&ctx.account.id, &state).await
}

// ══════════════════════════ 执行层 ══════════════════════════

/// 每一轮上传之间等多久（旧 TS 522 循环体末尾那句 `sleep(500)`）。
pub const PACE_MS: i64 = 500;

/// 一条待做任务的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskStep {
    pub task_id: Option<i64>,
    pub name: String,
    /// 真传上去的个数。
    pub uploaded: u32,
    /// 打卡那一枪的结局。
    pub clicked: bool,
    /// 434 那一档：外链**建了又删**了。`false` = 没做（含"没有可分享的文件"）。
    ///
    /// ⚠ 它只说明**技术动作**成了，**不等于**任务记上了 —— 判据看 [`Self::share_counted`] ✓。
    pub shared: bool,
    /// 434：这一趟**动手前**从条目名字里读到的次数（活体形如 `"0/7"`）。
    /// `None` = 名字里没有那个 `a/b`（读不出，**不当 0** ✗）。
    pub share_before: Option<String>,
    /// 434：**跑完之后整张表再拉一遍**读到的次数（同 `"0/7"` 的写法）。`None` = 读不出。
    pub share_after: Option<String>,
    /// 434：**上游真把这一笔记上了** —— 唯一的判据是 `share_after > share_before` ✓。
    ///
    /// # 为什么不能看别的
    ///
    /// 官方**没有**"这条任务完成了"的接口 ⇒ 只能看次数（旧 TS
    /// `services/app/tasks.ts:113-125` 的 `getShareCount` + `printShareProgress`、
    /// 调用点在 `app.ts:30`（动手前记一次）与 `app.ts:56-60`（全跑完再拉整表比大小）✓）。
    /// 所以它**既不等于**"打卡门过了"（`clicked`），**也不等于**"外链建成了"
    /// （[`Self::shared`]）—— 外链建成而次数不动时，参考实现的 `collectTaskResult`
    ///（`results.ts:18-45`）照样把这条记进 `failed` ✓。
    pub share_counted: bool,
    /// 这一档要报的**分母**（`stop_at_process`：522 = 100 ✓、615 那一族 = 10 ✓）。
    ///
    /// ⚠ 它就是"这一档要不要报进度"的开关（`Some` ⇒ 报 ✓）。**分母必须来自这一档自己的
    /// 形状**，不能写死 522 那个 100 ✗ —— 2026-10-04 真机 run 110 就是这么错的：
    /// 615 那一格里冒出了 `progressBefore: "0/100"` ✗（实际是"10 张里 0 张" ✓）。
    pub progress_total: Option<i64>,
    /// 这一档**动手前**条目自报的进度（`"42/100"` / `"0/10"`）。`None` = 没读到 process。
    pub progress_before: Option<String>,
    /// 522：跑完之后**整张表再拉一遍**读到的进度（`"47/100"`）✓。
    ///
    /// ⚠ 这一格**不参与成败判定**（522 的"成"是"这一趟真传了文件"，那一支自己计 `acted` ✓）——
    /// 它只是给用户看"走到哪儿了" ✓（用户口径 2026-10-04：「毕竟需要 100 个，
    /// 也不急着一次做完，**进度还是很重要的**」✓）。
    pub progress_after: Option<String>,
    /// 逐条的原因。
    pub problems: Vec<String>,
}

/// 这一趟的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppTaskResult {
    /// **这一趟真的推进了的条目数** —— 逐档按各自**能拿到的最硬判据**算：
    ///
    /// * `Upload` / `Entry`：传过文件**或**打过卡（那两档除了"发过写请求"没别的可看 ✓）；
    /// * `434`：**上游次数涨了**（[`TaskStep::share_counted`] ✓）—— 打卡过、外链建成都**不算**。
    ///
    /// ⚠ 旧 TS 的 `doingList`（`services/app/tasks.ts:29-36`）记的是"打卡门过了"，
    /// 但那一份在收尾时会被 `collectTaskResult`（`results.ts:18-45`）**按上游 state 复判**：
    /// 处理过、state 仍非 `FINISH` ⇒ 记 `failed` ✓ ⇒ 本仓 434 这一档取的是后者那条更硬的
    /// 判据（"次数涨没涨"），**不是**"门过了" ✗（2026-10-04 用户纠正 ✓）。
    pub acted: usize,
    /// 判出来"要传"的条目数。
    pub planned: usize,
    pub steps: Vec<TaskStep>,
    /// 我们造的文件 id（**`exist` 那一档不进来** —— 那是用户自己的文件）。
    pub file_ids: Vec<String>,
    pub cleaned_up: usize,
    pub cleanup_error: String,
    pub failures: Vec<String>,
    /// 这一趟判定用的东八区月份 / 日；`None` = 时钟读不出（那时月份门一律"不知道"）。
    pub month: Option<String>,
    pub day: Option<i64>,
    /// 本账号当月备份读数（`None` = 没记录 / 读失败）。**按账号**取，见
    /// [`read_month_backup`]。
    pub month_backup: Option<bool>,
}

impl AppTaskResult {
    pub fn summary_line(&self) -> String {
        const HEAD: &str = "App 任务";
        if self.planned == 0 {
            return format!("{HEAD}没做：这张表里没有需要上传的条目");
        }
        let tail = match (self.file_ids.is_empty(), self.cleaned_up) {
            (true, _) => String::new(),
            (false, n) if n == self.file_ids.len() => {
                format!("（网盘新增 {} 个文件，已移进回收站）", self.file_ids.len())
            }
            (false, n) => format!("（网盘新增 {} 个文件，只收回 {n} 个）", self.file_ids.len()),
        };
        let mut s = format!(
            "{HEAD}做了 {} 条（判出 {} 条）{tail}",
            self.acted, self.planned
        );
        if !self.cleanup_error.is_empty() {
            s.push_str(&format!("；清理失败：{}", self.cleanup_error));
        }
        if !self.failures.is_empty() {
            s.push_str(&format!("；{} 条有失败", self.failures.len()));
        }
        s
    }

    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        // ⚠ 一条都没做成、又确实判出该做 ⇒ 报失败，别报成"没事发生"。
        if self.planned > 0 && self.acted == 0 {
            return TaskOutcome::failed(summary);
        }
        if self.planned == 0 {
            return TaskOutcome::skipped(summary);
        }
        TaskOutcome::success(summary)
    }
}

/// 跑一次 `app-task`：**写**。
///
/// # ⚠ 顺序是**先打卡、再上传**（不是我原先以为的反过来）
///
/// 旧 TS `tasks.ts:33-44` 的 `handleTaskClick` 逐字：
///
/// ```ts
/// if (await clickTaskWithStep(ctx, task.id, task.currstep)) {   // ← 先打卡
///   await taskRunner[task.id]?.(ctx, task);                     // ← 成功之后才跑玩法
/// }
/// ```
///
/// ⇒ **打卡是一道门**：先问上游"这一条记上了没有"，成了才去真的做那件事。
/// 参考实现（Go）写的是同一句："beiyong1 的'上传图/照'那一条旧实现是
/// **先打卡、再上传**"。
///
/// ⚠ 这一版最初把顺序写反了（先传再打卡），理由是我自己推的
/// "打卡是'我做完了'的声明，声明在动作之前就是撒谎" —— **两边参考都不支持那句**。
/// 从第一原理推出来的行为，只要与参考相反，就是错的。
pub async fn run_app_task(
    ctx: &TaskContext,
    pace_ms: i64,
    only_task_ids: &[i64],
) -> Result<AppTaskResult> {
    let mut result = AppTaskResult::default();

    // ① 读整张表（十六组，逐次容错）。
    let mut pages: Vec<mcloud_upstream::app_task_list_v2::TaskListV2> = Vec::new();
    for market in APP_TASK_MARKETS {
        for group in mcloud_upstream::app_task_list_v2::TASK_LIST_V2_GROUPS {
            ctx.check_cancelled()?;
            match app_task_matrix::fetch_one(ctx, market, group).await {
                Ok(p) => pages.push(p),
                // 一次失败不算失败 —— 登记它、继续拉下一组（与只读那一半同口径）。
                Err(e) => result.failures.push(format!("读取 {market}/{group}：{e}")),
            }
        }
    }
    let (items, _raw) = app_task_matrix::merge_pages(&pages);

    // ② 取**本账号当月**的备份读数（月份门的输入），再逐条判。判不出来的一律**不动**。
    //
    // ⚠ **先过三道门、再判形状**（旧 TS 就是先 `continue` 再分派）。
    // 顺序反了不改结果，但会把"已完成/该跳过"的条目也算进 `planned`，
    // 于是摘要里那句"判出 N 条"是假的。三道门的分工：
    //
    // * `should_act` —— `state == "FINISH" || enable != 1`（`index.ts:34`）
    //   **加上** [`SKIP_TASK_LIST`] 黑名单（`tasks.ts:101-103`）；
    // * `is_configured_skip` —— 本账号的 `skipTasks`（`index.ts:33`）；
    // * `month_gate_skips` —— `sign_in_3` 的 `month` 组（`tasks.ts:86-95`）。
    let (month, day) = match beijing_month_day(&*ctx.clock) {
        Some((m, d)) => (Some(m), Some(d)),
        None => (None, None),
    };
    result.month = month.clone();
    result.day = day;
    result.month_backup = match &month {
        Some(m) => read_month_backup(ctx, m).await,
        None => None,
    };
    let mut planned: Vec<(TaskV2Item, Step)> = items
        .iter()
        .filter(|i| should_act(i))
        .filter(|i| !is_configured_skip(i, &ctx.account.settings.skip_tasks))
        .filter(|i| !month_gate_skips(i, result.month_backup, day))
        // 🧪 调试闸（`onlyTaskIds`）：留空 ⇒ 不筛。有它才敢"只试一条"
        //（2026-10-04 用户口径：这么多任务，得有个方便调试单条的接口）。
        .filter(|i| {
            only_task_ids.is_empty()
                || i.task_id
                    .is_some_and(|id| only_task_ids.contains(&id))
        })
        .filter_map(|i| step_for(i).map(|s| (i.clone(), s)))
        .collect();
    // ⚠⚠ **434 必须排在最后**：它消费的是**这一趟自己传上去的文件**
    //（`share_first_uploaded(ctx, &result.file_ids)` ✓），而计划是按 id 升序的
    //（`merge_pages` 里那句 `all.sort_by_key(task_id)` ✓）⇒ 434(434) **永远**排在
    // 522(522) 前面 ✗。2026-10-04 真机实测（run 103，测试号，`onlyTaskIds=522,434`）：
    // 522 明明传了 5 个文件，434 那一格却是「**没有可分享的文件，跳过分享任务**」✗。
    // 更糟的是**两个账号的 106 都是 `FINISH`** ⇒ 只要按 id 排，434 就**永远**分享不到东西
    // ✗✗（这一条是"单独跑一次 434"验出来的，单测夹具里 106 是 WAIT、正好掩盖了它 ✓）。
    // ⇒ 把它挪到末尾：它**只消费、不生产**，排最后不影响任何别人 ✓（`sort_by_key` 稳定 ⇒
    // 其余条目照旧 id 升序 ✓）。
    planned.sort_by_key(|(i, _)| i.task_id == Some(SHARE_TASK_ID));
    result.planned = planned.len();

    // ③ 逐条做。
    for (item, step_kind) in &planned {
        ctx.check_cancelled()?;
        // ⚠ 上游的 name 里会带 HTML（见 `clean_task_name` 的说明）⇒ 进记录前清掉 ✓
        let name = clean_task_name(&item.name.clone().unwrap_or_default());
        let mut step = TaskStep {
            task_id: item.task_id,
            name: name.clone(),
            ..Default::default()
        };
        ctx.progress(
            result.steps.len() as u32 + 1,
            planned.len() as u32,
            format!("做任务 {name}"),
        );

        // ③-a **先打卡**：它是一道门，成了才去做那件事。
        //
        // ⚠ 但"打不打"本身还有一道门（[`should_click`]）：已经走起来的条目
        // **不该再打**，而门**照样通过**（旧实现那句 `return true`）。
        if should_click(item) {
            match click(ctx, item).await {
                Ok(notice) => {
                    step.clicked = true;
                    if let Some(n) = notice {
                        step.problems.push(format!("上游附言：{n}"));
                    }
                }
                Err(e) => {
                    // 门没过 ⇒ **不传**（旧 `if (await clickTaskWithStep(...))` 那个形状）。
                    step.problems.push(format!("打卡失败，这一条不传：{e}"));
                    result.steps.push(step);
                    continue;
                }
            }
        } else {
            step.problems.push(format!(
                "已走到第 {} 步 ⇒ 按旧实现不打卡（门仍通过，照常往下做）",
                item.curr_step.unwrap_or(0)
            ));
        }

        // ③-b 门过了 ⇒ 做那一件事。
        // ⚠ 434 与上传**互斥**（见 `Step`）：它消费的是别人生产的文件。
        if matches!(step_kind, Step::Entry) {
            match entry_do_task(ctx).await {
                Ok(()) => {}
                // 埋点失败**不挡路**（TS 那句 `tryRequest` 也不看结果 ✓）
                Err(e) => step
                    .problems
                    .push(format!("入口埋点没打成（不影响后面的打卡）：{e}")),
            }
            // 旧 TS：埋点 → `sleep(500)` → `clickTask`。
            if pace_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pace_ms as u64)).await;
            }
            match click(ctx, item).await {
                Ok(notice) => {
                    step.clicked = true;
                    if let Some(n) = notice {
                        step.problems.push(format!("上游附言：{n}"));
                    }
                }
                Err(e) => step.problems.push(format!("入口那一发打卡失败：{e}")),
            }
            if step.clicked {
                result.acted += 1;
            }
            result.steps.push(step);
            continue;
        }
        let Step::Upload(shape) = step_kind else {
            // 434：打卡那一发在上面打过了；这里先**记下动手前的次数**再分享。
            // ⚠ 必须从**原始** `item.name` 解（`step.name` 那份已经被 `clean_task_name`
            // 把 `<span>` 剥掉了 ✗ ⇒ 那里已经读不出数字）。
            step.share_before = item
                .name
                .as_deref()
                .and_then(share_progress_from_name)
                .map(|(a, b)| format!("{a}/{b}"));
            // ⚠⚠ **没有可分享的文件 ⇒ 自己传一个**（用户口径 2026-10-04）。
            // 434 消费的是**这一趟**传上去的文件，而计划按 id 升序 ⇒ 只要同趟的
            // 106 是 `FINISH`、或者这一趟只跑 434（`onlyTaskIds=434`），它手里就是空的，
            // 旧实现那句「没有可分享的文件，跳过分享任务」会让它**永远做不成** ✗
            //（真机实证 run 103：522 传了 5 个，434 那一格照样是"空手" ✗）。
            // ⇒ 两道保险：计划里 434 排**末尾**（让别人先生产 ✓），轮到自己还空着
            //就**自己造一个** ✓。
            if result.file_ids.is_empty() {
                // ⚠ 这一发补传**按安卓档**（缺省档 ✓）：旧 TS 的 `shareTime` 自己不上传，
                // 这一发是本仓补的 ⇒ 没有哪一档渠道要求，用缺省最不容易出错 ✓。
                match upload_one_file(ctx, CloudChannel::Android, ".bin").await {
                    // 交回来的 id 进 `file_ids` ⇒ 收尾那一发会把它一起移进回收站 ✓
                    Ok(Some(id)) => {
                        result.file_ids.push(id);
                        step.uploaded += 1;
                    }
                    Ok(None) => step
                        .problems
                        .push("撞同名（那不是我们造的）⇒ 这次没有可分享的文件".to_string()),
                    Err(e) => step.problems.push(format!("为分享补传一个文件失败：{e}")),
                }
            }
            match share_first_uploaded(ctx, &result.file_ids).await {
                Ok(()) => step.shared = true,
                // ⚠ "没有可分享的文件"也走这一支 —— 它不是失败，只是这一档没做成 ✓。
                Err(e) => step.problems.push(e.to_string()),
            }
            // ⚠⚠ **不在这里记 `acted`**：这一档唯一的判据是**上游次数涨没涨**
            //（`share_counted` ✓），而那只可能在**跑完之后再拉一次表**才知道
            //（参考实现的位置也一样：`app.ts:56-60` 是跑完再拉全表比大小 ✓）。
            // 打卡过（`clicked`）/ 外链建成（`shared`）**都不算推进** ✗。
            result.steps.push(step);
            continue;
        };
        let mut done: u32 = 0;
        // 522：动手前先把**进度**记下来（跑完再拉一次表 → 比出"走到哪儿了" ✓）。
        // 只在**循环档**（`stop_at_process > 0`，即 522）记：106 那种一趟一个的没有"进度"可言 ✗。
        if shape.stop_at_process > 0 {
            // ⚠ 分母取**这一档自己**的 `stop_at_process`（522=100、615 那一族=10 ✓）：
            // 写死 522 那个常量会让 615 显示成 `0/100` ✗（真机 run 110 就这样错了 ✓）。
            step.progress_total = Some(shape.stop_at_process);
            step.progress_before = item
                .process
                .map(|p| format!("{p}/{}", shape.stop_at_process));
        }
        // ⚠ 615 那一族：上限 = `stop_at_process - process`（旧 TS 先算 `remaining` ✓）。
        // 522 不用这一支（它的 `process` 只是闸、上限另由 `rounds` 管 ✓）⇒ 见 `top_up_from_process`。
        let eff = if shape.top_up_from_process && shape.stop_at_process > 0 {
            UploadShape {
                rounds: (shape.stop_at_process - item.process.unwrap_or(0)).max(0) as u32,
                // 上限已经按剩余量收好 ⇒ 静态那道闸不必再看 ✓
                stop_at_process: 0,
                ..*shape
            }
        } else {
            *shape
        };
        while should_upload_another(eff, done, item.process) {
            ctx.check_cancelled()?;
            // ⚠ 渠道按**这一档的形状**走：113 是 PC 档、别的是安卓档 ✓
            //（create 与 complete 两发用同一个 `shape.channel` —— 旧 TS 那两发也同源 ✓）。
            match upload_one_file(ctx, eff.channel, eff.file_ext).await {
                Ok(Some(id)) => {
                    result.file_ids.push(id);
                    step.uploaded += 1;
                }
                // `exist` 那一档：**不是我们造的** ⇒ 不记账（记了就是删用户的文件）。
                Ok(None) => step
                    .problems
                    .push("上游说同名文件已存在 ⇒ 不算我们造的，也不去删".into()),
                Err(e) => step.problems.push(format!("上传失败：{e}")),
            }
            done += 1;
            // ⚠ 等的是**这一档自己的数**（522=500ms ✓、615=300ms ✓、106/113=0 不等 ✓）；
            // `pace_ms > 0` 只是让测试能把等待整体关掉（测试传 0 ✓），不改线上行为 ✓。
            if eff.per_round_ms > 0 && pace_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(eff.per_round_ms as u64)).await;
            }
        }

        if step.uploaded > 0 || step.clicked {
            result.acted += 1;
            // 旧 TS `tasks.ts:41-46`（`handleTaskClick`）做完一条之后那句
            // `await sleep(500)` —— **只对真动过的条目**等：没做的事（`clicked=false`
            // 且一个文件都没传）在 TS 那边根本不进 `handleTaskClick`，也就不等 ✗。
            // ⚠ 这与 522 循环体内那一拍是**两处**，TS 两处都有
            //（`runners.ts:117-125` 的循环 + 这里）⇒ 别把哪一处当重复删掉。
            if pace_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pace_ms as u64)).await;
            }
        }
        result.steps.push(step);
    }

    // ④ 清理：**只删我们自己造的**。
    cleanup(ctx, &mut result).await;

    // ⑤ 事后读数：**整张表再拉一遍、比数字** —— 434 判"记上没记上"、522 报"进度" ✓
    //    （参考实现的位置一样：`app.ts:56-60`，全部任务跑完 → `getAllAppTaskList()`
    //     → `printShareProgress` 比 `state.shareCount` ✓）。
    settle_progress(ctx, &mut result).await;

    Ok(result)
}

/// 玩法 434 的本体：把这趟传上去的**第一个**文件分享出去、再**立刻**删掉外链。
///
/// # 逐字照旧 TS `runners.ts:68-88`
///
/// ```ts
/// const files = ctx.state.files;
/// if (!files || files.length === 0) { log("没有可分享的文件，跳过分享任务"); return; }
/// const fileId = files[0];
/// const [err, data] = await tryRequest(getOutLink, ctx.http, phone, [fileId], "");
/// if (!err && data?.code === "0") {
///   const linkId = data?.data?.getOutLinkRes?.getOutLinkResSet?.[0]?.linkID;
///   if (linkId) await tryRequest(delOutLink, ctx.http, phone, [linkId]);
/// } else { log("分享文件失败"); }
/// ```
///
/// 四处照抄、每一处都有"改了不报错"的后果：
///
/// 1. **只取第一个文件** —— `files[0]`。全分享出去会留下**多条**外链，
///    而旧实现只删回第一条（见 ③）。
/// 2. **`dedicatedName` 是空串** —— 调用处第五个参数就是 `""`。
/// 3. **只删第一个 linkID** —— `?.[0]`。⚠ 上游给了多条而我们只删一条，
///    剩下的就留在用户的分享记录里；这是旧实现的行为，**照抄并标出来**，
///    不"顺手改好" —— 改好是另一种行为，而我们没有样本说哪种对。
/// 4. **没有可分享的文件 ⇒ 报一句"跳过"就走**，不建空外链
///    （与协议层那道"空表不发"的守门同一取向）。
///
/// ⚠ **它依赖同一趟前面的上传真的传上来过东西** —— 这是旧实现自带的脆弱点。
/// 待做表按 id 升序，若当天能上传的那几条 id 都比 434 大，它就永远没有可分享的文件。
/// 我们这边的处置是**如实报"跳过"**，而不是为它造一个文件（那是为补样造行为）。
async fn share_first_uploaded(ctx: &TaskContext, uploaded: &[String]) -> Result<()> {
    let Some(file_id) = uploaded.first() else {
        // 照旧 TS 那句：跳过去，不建空外链
        return Err(mcloud_core::Error::Other(
            "没有可分享的文件，跳过分享任务".to_string(),
        ));
    };
    let account = ctx.account.auth.phone.clone();
    let req = outlink::get_out_link_request(&account, std::slice::from_ref(file_id), "")?;
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "建外链（getOutLink）")?;
    let link = outlink::parse_get_out_link(&outlink::OutLinkEnvelope::parse(body)?)?;
    // ⚠ 建了却拿不到 linkID ⇒ 必须报出来（那是一条删不掉的外链）
    link.ensure_found()?;
    let first = &link.link_ids[0];
    let req = outlink::del_out_link_request(&account, std::slice::from_ref(first))?;
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "删外链（delOutLink）")?;
    outlink::parse_del_out_link(&outlink::OutLinkEnvelope::parse(body)?)?;
    Ok(())
}

/// 传一个随机文件，交回**我们造的**那个 id（`exist` 档交 `None`）。
///
/// `channel` 是**必填**的：旧 TS 的 `uploadRandomFile` 缺省按安卓档发
///（`getFileOpHeader` 的 `channelSrc` 缺省值 ✓），只有玩法 113 显式改成 PC 档 ✓ ——
/// 把它做成参数而不是写死，正是为了"哪一档走哪个客户端"这件事**在调用点看得见** ✓。
async fn upload_one_file(
    ctx: &TaskContext,
    channel: CloudChannel,
    ext: &str,
) -> Result<Option<String>> {
    let mut rng = mcloud_crypto::OsRandom;
    let plan = crate::task_expansion::random_plan_with_ext(&mut rng, 0, ext);
    let hash = mcloud_crypto::sha256_hex_bytes(&plan.content);
    let created = {
        let req = upload::file_create_request(CloudFileCreateInput {
            parent_file_id: "/".into(),
            name: plan.name.clone(),
            size: plan.content.len() as i64,
            content_hash: hash.clone(),
            channel,
            op: CloudOpType::Upload,
        })?;
        let resp = ctx.http.request(req).await?;
        upload::parse_file_create(&resp.body)?
    };
    // ⚠ 撞同名 ⇒ 那个文件**不是我们造的**，交回 `None`。
    if created.exist {
        return Ok(None);
    }
    let Some(file_id) = created.file_id.clone() else {
        return Err(mcloud_core::Error::Parse("create 没给 fileId".into()));
    };
    if !created.rapid_upload {
        let Some(url) = created.upload_url.clone() else {
            return Err(mcloud_core::Error::Parse("create 没给 uploadUrl".into()));
        };
        let resp = ctx
            .http
            .request(upload::upload_put_request(&url, &plan.content))
            .await?;
        upload::ensure_put_ok(&resp)?;
        let req = upload::file_complete_request(
            &file_id,
            created.upload_id.as_deref().unwrap_or(""),
            &hash,
            channel,
            CloudOpType::Upload,
        )?;
        let resp = ctx.http.request(req).await?;
        upload::parse_file_complete(&resp.body)?;
    }
    Ok(Some(file_id))
}

/// 🧪 调试闸：`body.onlyTaskIds`（`"106,522"` 或 `[106,522]`）⇒ 只动这几条。
///
/// 留空 ⇒ 不筛（正常跑）。这条存在只为一件事：**单独试一条任务**时不连带动别的
///（用户口径 2026-10-04：「做个方便调试某个具体任务的接口，比较有这么多任务」）。
fn parse_only_task_ids(body: &Value) -> Vec<i64> {
    let raw = body.get("onlyTaskIds");
    let mut out: Vec<i64> = Vec::new();
    match raw {
        Some(Value::Array(items)) => {
            for v in items {
                if let Some(n) = v.as_i64() {
                    out.push(n);
                }
            }
        }
        Some(Value::String(s)) => {
            for part in s.split([',', '，', ' ', '\n']) {
                if let Ok(n) = part.trim().parse::<i64>() {
                    out.push(n);
                }
            }
        }
        _ => {}
    }
    out
}

/// 「从固定入口进一次」那一发埋点（409 / 1005）。
///
/// 旧 TS `runners.ts:98-108` / `:126-133`。`deviceId` 用 smid 换回来的那一个 ✓。
async fn entry_do_task(ctx: &TaskContext) -> Result<()> {
    let device_id = crate::signin::h5_device_id(ctx, None).await;
    let req = mcloud_upstream::app_task_entry::entry_do_task_request(device_id.as_deref())?;
    let resp = ctx.http.request(req).await?;
    mcloud_upstream::session::ensure_http_ok(&resp, "入口埋点（page/doTask）")?;
    Ok(())
}

/// 打卡用的 `key`。默认 `task`；个别任务按参考实现换成专用值。
///
/// ⚠⚠ **`key` 不是"任务分组"** —— 这里从前发的是 `item.group_id`
/// （`day` / `month` / `time` / `hidden`）✗，参考实现里根本没有这个形状：
///
/// | 出处 | 值 |
/// |---|---|
/// | `mcloud-sign/…/tasks/app/helpers.ts:10` | 默认 `taskStr ?? "task"` |
/// | `runners.ts:107` | `409` → `task2`（**按任务**给，不是按分组）|
/// | `index.ts:51,54` | `478` / `481` → `randomCloudTask` |
///
/// 本仓发分组名这件事**没有任何实拍或黄金向量支持**（`test/golden/` 里没有
/// `task/click` 的向量），而真机读数正好相反：2026-10-04 用户实测 106 —— 那趟记录
/// 是 `{uploaded:1, clicked:true, problems:[]}`，可上游的 `state` 始终是 `WAIT` ✗。
fn click_key(task_id: Option<i64>) -> &'static str {
    match task_id {
        // `runners.ts:107`：409「从固定入口进入云朵中心」—— `Step::Entry` 里
        // **补的那一发**打卡用的就是它 ✓（`handleTaskClick` 那发另算）。
        Some(409) => "task2",
        // `index.ts:51,54`：478 / 481 走「随机云朵任务」；这两条本仓**还没接**
        //（`step_for` 里没有它们）⇒ 先留在这里，将来接上时不漏 ✓。
        Some(478) | Some(481) => "randomCloudTask",
        _ => app_task_click::TASK_CLICK_DEFAULT_KEY,
    }
}

/// 打一发「这条我做完了」。
async fn click(ctx: &TaskContext, item: &TaskV2Item) -> Result<Option<String>> {
    let Some(id) = item.task_id else {
        return Err(mcloud_core::Error::Config("这一条没有 id，不打卡".into()));
    };
    let key = click_key(item.task_id);
    let req: HttpRequest = app_task_click::task_click_request(key, id)?;
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "打卡（task/click）")?;
    let env = mcloud_upstream::app_signin::SignInEnvelope::parse(body)?;
    Ok(app_task_click::parse_task_click(&env)?.notice)
}

/// 把我们自己传上去的那些移进回收站。
async fn cleanup(ctx: &TaskContext, result: &mut AppTaskResult) {
    if result.file_ids.is_empty() {
        return;
    }
    let req = match cloud_trash::batch_trash_request(&result.file_ids) {
        Ok(r) => r,
        Err(e) => {
            result.cleanup_error = e.to_string();
            return;
        }
    };
    match ctx.http.request(req).await {
        Ok(resp) => match cloud_trash::parse_batch_trash(&resp.body) {
            Ok(()) => result.cleaned_up = result.file_ids.len(),
            Err(e) => result.cleanup_error = e.to_string(),
        },
        Err(e) => result.cleanup_error = e.to_string(),
    }
}

/// 跑一次 `app-task`（**会写**：往用户网盘传文件并打卡）。
pub struct AppTaskTask {
    /// 每轮之间的等待。做成字段是为了让测试设 0。
    pub pace_ms: i64,
}

impl Default for AppTaskTask {
    fn default() -> Self {
        Self { pace_ms: PACE_MS }
    }
}

impl AppTaskTask {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Task for AppTaskTask {
    fn name(&self) -> &'static str {
        "app-task"
    }
    fn title(&self) -> &'static str {
        "App 任务（做任务）"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::App
    }
    fn description(&self) -> &'static str {
        "把 App 任务矩阵里能做的做掉：按玩法传文件、逐条打卡（传上去的文件跑完自己删）"
    }

    fn params(&self) -> Vec<mcloud_engine::ParamSpec> {
        vec![mcloud_engine::ParamSpec::text(
            "onlyTaskIds",
            "只做这些 id（调试用）",
            "留空 = 照常跑全表；填了就**只动这些 id**（逗号分隔，例如 106）。\
             给「想单独试一条任务」用的 —— 列表有 25 条、每条副作用不同，\
             不筛就会连带动别的。正常跑别填。",
        )]
    }

    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        ctx.check_cancelled()?;
        let only_task_ids = parse_only_task_ids(body);
        let result = run_app_task(ctx, self.pace_ms, &only_task_ids).await?;
        ctx.details(serde_json::json!({
            "kind": "app-task",
            "planned": result.planned,
            "acted": result.acted,
            "uploadedFiles": result.file_ids.len(),
            "cleanedUp": result.cleaned_up,
            "cleanupError": result.cleanup_error,
            "failures": result.failures,
            "month": result.month,
            "day": result.day,
            "monthBackup": result.month_backup,
            "steps": result.steps.iter().map(|s| serde_json::json!({
                "taskId": s.task_id,
                "name": s.name,
                "uploaded": s.uploaded,
                "clicked": s.clicked,
                "shared": s.shared,
                // 434 的"成没成"是**比出来的**：动手前/后的次数 + 涨没涨 ✓
                "shareBefore": s.share_before,
                "shareAfter": s.share_after,
                "shareCounted": s.share_counted,
                // 522 的**进度**：`42/100 → 47/100` ✓（不参与成败，只报走到哪儿 ✓）
                "progressTotal": s.progress_total,
                "progressBefore": s.progress_before,
                "progressAfter": s.progress_after,
                "problems": s.problems,
            })).collect::<Vec<_>>(),
        }));
        ctx.info(result.summary_line());
        Ok(result.task_outcome())
    }
}

#[cfg(test)]
mod tests;
