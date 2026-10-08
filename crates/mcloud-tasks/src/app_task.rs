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

use crate::app_task_matrix::{self};

/// 专用执行器那一半（旧 TS `tasks/app/runners.ts` 的 runner 表）。
///
/// ⚠ 单独一个文件是为了让"**判据**"（本文件上半，全是纯函数）与"**动作**"
/// （那个文件，一枪一枪的 IO 编排）分得开。见 `runners.rs` 文件头。
pub mod runners;

pub use runners::{AppRunner, RunnerState};

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

/// 收尾的**事后复查**：整张表再拉一遍，拿"动手前的状态/读数"和"动手后的"对上，
/// **由状态的变化**判这一趟成没成 ✓。
///
/// # 判据（用户口径 2026-10-07）
///
/// 「**成功与否不是看你做了啥，而是状态的变化**」✓ —— 也就是旧 TS
/// `tasks/app/results.ts:collectTaskResult` 那一套：
/// `doingList`（本次动过）＋ 复查后的 `state === "FINISH"` ⇒ 成功；动了而没变 ⇒ 失败 ✓。
///
/// ⚠⚠ **这一句改掉了本仓原来的算法**：以前 `acted` 是"**我们发了写请求并且它返回成功**"
/// 就 +1（打卡 200、文件 PUT 上去…）✗ —— 那是"看你做了啥"。真机读数证明它会把
/// "没记上"说成"做了"：run 155 里 `409 / 1005` 都 `clicked:true`，
/// 而复查时上游状态**还是 `WAIT`**（同一天 `431` 就变成了 `FINISH` ✓）⇒
/// 前者根本不该算"做了" ✗。
///
/// # 三种结论
///
/// | 结论 | 判据 | 处理 |
/// |---|---|---|
/// | **成** | 状态变成 `FINISH` ✓ 或名字里的 `a/b` 变大（434 次数 / 522 进度 ✓）| `ctx.success` + 计进 `acted` ✓ |
/// | **没记上** | 读到了状态/读数，但**没变** | `ctx.fail` + 进 `failures` ✓（**这才是"看你做了啥"该有的下场**）|
/// | **读不出** | 拉表失败 / 表里没有这一条 | 只写"未获上游确认"标注，**不硬判失败** ✓ |
///
/// 一次拉表服务**所有**"动过、要判定"的条目（同一份读数、同一个时刻 ✓，也省 GET ✓）。
async fn settle_progress(ctx: &TaskContext, result: &mut AppTaskResult) {
    // ⚠ "整条没动（上游已做满）"的那些档**不必再拉一次整表** ✗ —— 拉回来也没得比 ✓
    //（2026-10-07：分享 `7/7` 那种条目每一趟都会白拉 16 次 ✓）。
    //
    // ⚠⚠ **`skipped_today` 也必须排除**（2026-10-07 真机逮到，用户截图：
    // "不跑第二次"下面紧跟一行红 FAIL ✗）。这一类是"**本地记录说今天已经成功过 ⇒
    // 压根没动手**"，它的 `share_before` 必然是 `None`（跳过那一支不去读名字里的 a/b ✓）
    // ⇒ 放进来就会走 [`settle_share`] 的 `None` 分支、报「动手前读不出次数 ⇒ 无从比较」✗
    // —— 那句话的含义是"我们没读到"，而事实是"我们没打算读" ✗，两回事。
    // 附带：排除之后 `share_i`/`upload_i` 双双为 `None` ⇒ 那趟整表**压根不发** ✓。
    let share_i = result
        .steps
        .iter()
        .position(|s| s.task_id == Some(SHARE_TASK_ID) && !s.already_done && !s.skipped_today);
    // ⚠ 找的是"**报了进度的那些档**"（522 / 615 那一族…），不是写死 522 的 id ✓
    let upload_i = result.steps.iter().position(|s| s.progress_total.is_some());
    // ⚠⚠ 第三类：**动过、但只有状态可比**的那些（409 / 431 / 1005 / click-only 那一族…）
    // —— 以前它们**不进这一趟复查**（所以日志里只有 START、后面一片空白 ✗，
    // 用户 2026-10-07 指的就是这个）。现在把它们也纳进来：它们的"成"只能看状态变化 ✓。
    //
    // ⚠ 434 与"报进度"那两档**不在这里** —— 它们由 [`settle_share`] / [`settle_upload`]
    // 结算（各自的读数比状态更细 ✓），放进来会对同一条记两笔 ✗。
    let judge: Vec<usize> = result
        .steps
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            acted(s)
                && !s.already_done
                && !s.skipped_today
                && Some(*i) != share_i
                && Some(*i) != upload_i
        })
        .map(|(i, _)| i)
        .collect();
    if share_i.is_none() && upload_i.is_none() && judge.is_empty() {
        return; // 这一趟没有要复查的 ⇒ 那趟"事后整表"压根不发 ✓
    }
    let Some(items) = refetch_items(ctx).await else {
        for i in [share_i, upload_i].into_iter().flatten() {
            result.steps[i]
                .problems
                .push("跑完拉表读不出数 ⇒ 保守当作没记上".to_string());
        }
        return;
    };

    // ① 先把**动手后的状态**落到每一条上（判定的右半边 ✓）。
    //
    // ⚠ 覆盖**所有动过的条目**（含 434 与"报进度"那两档 ✓）—— 用户口径要的是
    // "记录开始时的状态、运行后的状态"（2026-10-07）⇒ 明细里每一条都该有这两格，
    // 不能只有"靠状态判"的那一批有 ✗。
    let mut watch = judge.clone();
    watch.extend([share_i, upload_i].into_iter().flatten());
    watch.sort_unstable();
    watch.dedup();
    for &i in &watch {
        let Some(id) = result.steps[i].task_id else {
            continue;
        };
        result.steps[i].state_after = items
            .iter()
            .find(|it| it.task_id == Some(id))
            .and_then(|it| it.state.clone());
    }

    if let Some(i) = share_i {
        settle_share(&items, result, i, ctx);
        // 用户口径 2026-10-07：「第一次跑本地记录，成功不跑二次」✓ —— **只在真的
        // 确认成功时写**（`share_counted` ✓）：没确认就写会把"试过了"当"成功过" ✗。
        if result.steps[i].share_counted {
            match beijing_day_digits(&*ctx.clock) {
                Some(day) => {
                    if let Err(e) = save_share_done_day(ctx, &day).await {
                        result.steps[i]
                            .problems
                            .push(format!("本地记录没写进去：{e}"));
                    }
                }
                None => result.steps[i]
                    .problems
                    .push("时钟读不出 ⇒ 这一笔没法记进本地记录".to_string()),
            }
        }
    }
    if let Some(i) = upload_i {
        settle_upload(&items, result, i, ctx);
    }

    // ② 逐条下结论，并**按状态变化重算 `acted`**。
    //
    // ⚠⚠ 为什么要重算而不是"在做的那一处 +1"：用户口径就是"不看做了啥、看状态变化" ✓
    // ⇒ 只有**被上游记上**的才算做了。原处那几发 `acted += 1` 照旧留着（它们是
    // "我们真动过"的记录 ✓，也是初值），这里按复查结果**逐条订正** ✓：
    // 原处记了而复查说"没变" ⇒ 撤回；原处没记而复查说"变了" ⇒ 补上。
    if !judge.is_empty() {
        for &i in &judge {
            let name = result.steps[i].name.clone();
            let verdict = verdict_of(&result.steps[i]);
            let counted = result.steps[i].acted_counted;
            match verdict {
                StepVerdict::Confirmed => {
                    if !counted {
                        result.steps[i].acted_counted = true;
                        result.acted += 1;
                    }
                    let how = change_line(&result.steps[i]);
                    ctx.success(format!("{name}：上游已确认（{how}）"));
                }
                StepVerdict::Unchanged => {
                    if counted {
                        result.steps[i].acted_counted = false;
                        result.acted = result.acted.saturating_sub(1);
                    }
                    // ⚠ 已经记过一次硬失败（网络/上游报错）的**不再补一笔** ——
                    // 同一条记两次，用户看到的失败数就比实际多 ✗
                    if result.steps[i].hard_failed {
                        continue;
                    }
                    let how = change_line(&result.steps[i]);
                    let msg = format!("{name}：本次做了，但上游状态/读数没变（{how}）⇒ 没记上");
                    ctx.fail(msg.clone());
                    result.steps[i].problems.push(msg.clone());
                    result.steps[i].hard_failed = true;
                    result.failures.push(msg);
                }
                // 读不出 ⇒ 交给 [`AppTaskResult::settle_unconfirmed`] 写"未获上游确认" ✓
                StepVerdict::Unknown => {}
            }
        }
    }
}

/// 收尾那**两条特例打卡**（旧 TS `tasks/app/index.ts:44-56`）。
///
/// # 旧实现逐字
///
/// ```ts
/// for (const task of dedupTasksByName(await getAllAppTaskList(ctx))) {
///   if (task.id === 434) { …只打印进度…; continue; }
///   if (task.id === 481 && task.currstep === 2) await clickTask(ctx, 481, "randomCloudTask");
///   if (task.id === 478 && task.state !== "FINISH") await clickTask(ctx, 478, "randomCloudTask");
///   collectTaskResult(task, doingList, taskResults);
/// }
/// ```
///
/// ⇒ 它们**不在** `dispatchTask` 那张 runner 表里，而是**收尾时**照**重新拉回来的**
/// 表再打一发（`key = randomCloudTask`，见 [`click_key`]）。
///
/// # ⚠ 本仓**不无条件**再拉一次整表
///
/// 旧 TS 那一遍是"每趟都重新拉 16 组"。本仓 2026-10-07 刚把"已做满的档不必再拉表"
/// 那条优化做进去（见 [`settle_progress`]）⇒ 这里只在**这一趟的表里出现过
/// 478 / 481** 时才多拉一次。判据用的是**第一趟**的表：真出现"第一趟没有、
/// 收尾时才有"那种情况时我们会漏掉一发 —— 而那需要上游在一趟之内新加任务，
/// 没有样本；反过来"每趟都拉"是**确定**的多 16 次请求 ✓。
///
/// ⚠ 它们**算进 `planned` 与 `acted`**：这是一发真实的、不可回滚的写请求，
/// 不该被算成"没做"（否则摘要会显示"做了 0 条（判出 0 条）"而实际上打了卡 ✗）。
async fn settle_random_cloud_clicks(ctx: &TaskContext, result: &mut AppTaskResult) {
    let Some(items) = refetch_items(ctx).await else {
        result
            .failures
            .push("收尾拉表读不出数 ⇒ 478/481 那两条特例打卡没做".to_string());
        return;
    };
    for item in &items {
        let Some(id) = item.task_id else { continue };
        let hit = match id {
            // `task.currstep === 2`（旧 TS 逐字）。
            481 => item.curr_step == Some(2),
            // `task.state !== "FINISH"`（旧 TS 逐字）。
            478 => item.state.as_deref() != Some(app_task_matrix::STATE_DONE),
            _ => continue,
        };
        if !hit {
            continue;
        }
        let name = clean_task_name(&item.name.clone().unwrap_or_default());
        let mut step = TaskStep {
            task_id: Some(id),
            name: name.clone(),
            ..Default::default()
        };
        match click(ctx, item).await {
            Ok(notice) => {
                step.clicked = true;
                result.planned += 1;
                // ⚠ 这一发**不算进 `acted` 的"订正"**（它不在 `judge` 里：收尾补发的条目
                // 本身是"上游已做满/特例"那一类 ✓）⇒ 不留 `acted_counted` 标记 ✓
                result.acted += 1;
                ctx.success(format!("{name}：收尾那一发随机云朵任务打卡已打 ✓"));
                if let Some(n) = notice {
                    step.problems.push(format!("上游附言：{n}"));
                }
            }
            Err(e) => {
                let msg = format!("{name}：收尾那一发随机云朵任务打卡没成：{e}");
                step.problems.push(msg.clone());
                result.planned += 1;
                result.failures.push(msg);
            }
        }
        result.steps.push(step);
    }
}

/// 434 那一格的落地：比次数、决定要不要把这一条计进 `acted` ✓。
fn settle_share(items: &[TaskV2Item], result: &mut AppTaskResult, i: usize, ctx: &TaskContext) {
    // ⚠ 整条没动（上游已做满）⇒ **没什么可比的**，更不是失败 ✗
    //（2026-10-07 用户反馈：分享 `7/7` 之后每一趟都报失败 ✗）。
    //
    // ⚠⚠ **`skipped_today` 同一条道理**（2026-10-07 用户截图逮到）：那一档是
    // "本地记录说今天已经成功过 ⇒ 压根没动手"，`share_before` 必然是 `None`
    // ⇒ 放进来只会得到一句「动手前读不出次数 ⇒ 无从比较」，而**界面上那行是红的** ✗。
    // 调用方（[`settle_progress`]）已经把它排除了 ✓，这里再挡一道是**防回归**：
    // 换个人来调用时，不会因为少想一步又把"没动手"结算成"没记上" ✗。
    if result.steps[i].already_done || result.steps[i].skipped_today {
        return;
    }
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
            result.steps[i].acted_counted = true;
            result.acted += 1;
            // 旧 TS 在这里打的就是 success（`printShareProgress` ✓）—— 且**只在涨了时**打 ✓
            ctx.success(format!(
                "{name}：{was}/{total} → {}/{}（记上了）",
                after.0, after.1
            ));
        }
        Some((was, total)) => {
            // ⚠⚠ 2026-10-07：这一档也要进 `failures`（用户口径：「成功与否不是看你做了啥，
            // 而是状态的变化」✓）—— 以前只写一条 `problem` + 一行红日志，而摘要与运行级
            // 结论看的都是 `failures` ⇒ **日志说失败、摘要却不记** ✗（与 434 跳过那格同一类不一致）。
            let msg = format!(
                "分享后上游次数没动（{was}/{total} → {}/{}）⇒ 这一条不算记上",
                after.0, after.1
            );
            result.steps[i].problems.push(msg.clone());
            result.steps[i].hard_failed = true;
            result
                .failures
                .push(format!("{}：{msg}", result.steps[i].name));
            ctx.fail(format!(
                "{name}：{was}/{total} → {}/{}（上游次数没动 ⇒ 不算记上）",
                after.0, after.1
            ));
        }
        None => {
            // 同上：读不出 ⇒ 也算"没记上"（进 `failures` ✓），别只写明细 ✗
            let msg = format!(
                "动手前读不出次数（名字里没有 a/b）⇒ 无从比较，保守当作没记上（分享后 {}/{}）",
                after.0, after.1
            );
            result.steps[i].problems.push(msg.clone());
            result.steps[i].hard_failed = true;
            result
                .failures
                .push(format!("{}：{msg}", result.steps[i].name));
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
    // ⚠⚠ `uploaded == 0` 有**两种完全不同的来路**，混着说会把人带偏 ✗（2026-10-05 真机：
    // 86 发 create 全被 `exist:null` 打死，而这里当时说的是"可能还没记账" ✗✗）：
    //   ① 上游记账慢 / 计数已满 ⇒ 那句是对的 ✓，下一趟再看 ✓；
    //   ② **每一发都发失败了** ⇒ 必须报失败 ✓（判据是这一档自己的 `problems` 里有没有
    //      "上传失败" —— 那是硬失败的字面证据 ✓，比任何推测都硬 ✓）。
    let upload_failed = result.steps[i]
        .problems
        .iter()
        .any(|p| p.starts_with("上传失败"));
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
        // ⚠ 这一支要排在下面那条"可能还没记账"**前面**：上传全废时那句是**假安慰** ✗。
        (Some(b), Some(a)) if upload_failed => ctx.fail(format!(
            "{name}：{b}/{total} → {a}/{total}\
             （这一趟 {uploaded} 个都没传上去 ⇒ 见上面那条「上传失败」的原因）"
        )),
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
    // 单组失败不炸整件事（与开头那一趟同口径 ✓），只是少一页证据。
    // ⚠ 取消错误也走 `.ok()?` ⇒ 这一格回 `None`；真正的取消由调用方在链尾原样报出去 ✓。
    let (pages, _failures) = app_task_matrix::fetch_all(ctx).await.ok()?;
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
    /// **专用执行器**（107 / 472 / 473 / 1004 / 1014 / 1020 / 1028 / 585）。
    ///
    /// 旧 TS 的 `taskRunners` 表（`runners.ts:39-186`）—— 与 `Upload` / `Entry` 走
    /// **同一条门**（`handleTaskClick`：先打卡、门过了才跑 runner ✓），
    /// 差别只在"那一件事"由 [`runners::run_runner`] 去做。
    Runner(AppRunner),
    /// **click-only**：框架替它打卡，没有 runner（旧 TS `tasks.ts:97-100` 的
    /// `default:` 分支：`if (!SKIP_TASK_LIST.includes(task.id)) await clickTask(...)`）。
    ///
    /// ⚠⚠ 它和上面几档有一处**行为差异**，别抹平：**那一发不看 `currstep`** ✗ ——
    /// 旧 TS 在 `default:` 分支里是**无条件** `clickTask`，而 runner 那一支走
    /// `clickTaskWithStep`（`currstep != 0` 就不打）。本仓照抄：这一档
    /// **不走** [`should_click`] 那道门。
    ClickOnly,
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
    if let Some(id) = item.task_id {
        // 专用执行器（旧 TS 的 runner 表）排在 click-only 之前：
        // 两串 id 今天**没有交集**，但真出现交集时"有 runner 的走 runner"
        // 才是旧 TS 的顺序（`tasks.ts:97-100` 先问 `taskRunner[task.id]` ✓）。
        if let Some(r) = AppRunner::for_task(id) {
            return Some(Step::Runner(r));
        }
        if is_click_only(id) {
            return Some(Step::ClickOnly);
        }
    }
    upload_shape_for(item).map(Step::Upload)
}

/// **click-only** 那一串 —— 旧 TS 里"框架自动打卡"的 id。
///
/// 依据：`mcloud-sign/TASKS.md`「**click-only 任务（框架自动处理）：** 115、431、
/// 435、1009、1010、1013、1016、447」，与 `tasks.ts:97-100` 的 `default:` 分支
///（有 runner 的走 runner，否则只要不在 [`SKIP_TASK_LIST`] 里就打卡 ✓）。
///
/// ⚠ 与 [`SKIP_TASK_LIST`] 是**互补**关系、**不是**子集关系：那一串是"刻意不打"，
/// 这一串是"刻意要打"。两串**不得有交集**（有交集时 [`should_act`] 里那道黑名单
/// 门会让这一档永远到不了这里 —— 那是安全的方向 ✓），由测试钉住。
pub const CLICK_ONLY_TASK_IDS: [i64; 8] = [115, 431, 435, 1009, 1010, 1013, 1016, 447];

/// 这一条是不是 click-only 那一档。
pub fn is_click_only(task_id: i64) -> bool {
    CLICK_ONLY_TASK_IDS.contains(&task_id)
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

/// 这一条**上游是不是已经做满了**（做也白做 ⇒ 整条不动 ✓）。
///
/// # 为什么要单开这道门（2026-10-07 用户反馈）
///
/// 原话：「**分享任务 7/7 了还在做，做完 7/7 说失败**」✗ —— 两件都错：
///
/// 1. **还在做**：434 的成败判据是"次数**有没有涨**"（[`TaskStep::share_counted`] ✓），
///    而 `7/7` 已经顶到上限 ⇒ 次数**永远涨不动** ⇒ 每做一次就白传一个文件、
///    白分享一次（还在链尾多删一次 ✗）；
/// 2. **说失败**：涨不动 ⇒ `settle_share` 判"没记上" ⇒ 进 `failures` ⇒ 运行报失败 ✗。
///
/// ⚠ 上游那条目的 `state` **不可靠**：真机读数里 434 涨到 `7/7` 之后 `state` 仍可能是
/// `WAIT` ✗（`should_act` 那道门拦不住它 ✓）⇒ 只能从**它自己给的进度**判 ✓。
///
/// # 判据（两族各一条，都用上游自己的读数）
///
/// * **434**：名字里那个 `a/b`（[`share_progress_from_name`] ✓）`a >= b` 且 `b > 0`
///   ⇒ 满了 ✓。`0/0` **不算满**（分母为 0 判不了 ✓）。
/// * **522 / 615 那一族**：`process` 到了这一档形状的 `stop_at_process`（100 / 10 ✓）
///   ⇒ 满了 ✓（`stop_at_process == 0` 的档没有"满"这个概念 ⇒ 恒 `false` ✓）。
///
/// ⚠ 读不出进度**不算满** ✓（与"读不出不是 0"同一条口径 ✗）—— 判不出就照旧做，
/// 让 `settle_*` 去如实报"没动"，别把"读不出"悄悄读成"不用做" ✗。
pub fn already_done(item: &TaskV2Item) -> bool {
    if item.task_id == Some(SHARE_TASK_ID) {
        return item.name.as_deref().is_some_and(share_is_full);
    }
    if let Some(shape) = upload_shape_for(item) {
        if shape.stop_at_process > 0 {
            return item.process.unwrap_or(0) >= shape.stop_at_process;
        }
    }
    false
}

/// 434 的名字里那对 `a/b` 是不是**已经满了**（`a >= b > 0`）。
pub fn share_is_full(name: &str) -> bool {
    matches!(share_progress_from_name(name), Some((done, total)) if total > 0 && done >= total)
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

/// `account_state` 里那一枚键：**434 今天已经成功过**（值 = 东八区日期 `yyyyMMdd` ✓）。
///
/// # 为什么要这一枚键（用户口径 2026-10-07）
///
/// 原话：「**第一次跑本地记录，成功不跑二次**」✓。起因是 434 的"成功"只能靠
/// **分享次数涨没涨**判（[`TaskStep::share_counted`] ✓），而上游对同一天第二次分享
/// **不会再加数** ⇒ 那一天第二次跑（手动点一次、或定时 + 手动）必然读到"次数没动" ✗
/// ⇒ 只跑 434 的那一趟就报 failed ✗（真机 run 151：`4/7 → 4/7` ✓）。
/// ⇒ 把"今天成功过"记在本地 ✓，第二次直接跳过 ✓（不是失败 ✗）。
///
/// ⚠ **只在"成功"时写**（`share_counted == true` ✓，或满了之后的领奖那一发打成了 ✓）：
/// 没确认成功就不写 ⇒ 下一趟照旧重试 ✓（不能拿"试过了"当"成功过" ✗）。
pub const SHARE_DONE_STATE_KEY: &str = "share_434_done_day";

/// 东八区墙钟的 `yyyyMMdd`；时钟读不出 ⇒ `None`。
pub(crate) fn beijing_day_digits(clock: &dyn mcloud_core::Clock) -> Option<String> {
    let digits = mcloud_core::beijing_time_digits(clock);
    (digits.len() >= 8).then(|| digits[..8].to_string())
}

/// 读"434 上次成功是哪一天"（东八区 `yyyyMMdd`）。`None` = 没记录 / 读不出 ✓。
pub async fn read_share_done_day(ctx: &TaskContext) -> Option<String> {
    let state = ctx.store.account_state(&ctx.account.id).await.ok()?;
    state
        .get(SHARE_DONE_STATE_KEY)?
        .as_str()
        .map(str::to_string)
}

/// 记下"434 今天成功了"。读-改-写，理由同 [`save_month_backup`] ✓（这张表里还躺着
/// `device_id` / `pc_platform` / `cur_month_backup` 这些**别的任务**的键 ✗）。
pub async fn save_share_done_day(ctx: &TaskContext, day: &str) -> Result<()> {
    let mut state = ctx.store.account_state(&ctx.account.id).await?;
    if !state.is_object() {
        state = serde_json::json!({});
    }
    if let Some(obj) = state.as_object_mut() {
        obj.insert(
            SHARE_DONE_STATE_KEY.to_string(),
            serde_json::Value::String(day.to_string()),
        );
    }
    ctx.store.save_account_state(&ctx.account.id, &state).await
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
    /// 这一条**本地记录说今天已经成功过 ⇒ 不跑第二次**（[`SHARE_DONE_STATE_KEY`] ✓）。
    pub skipped_today: bool,
    /// 这一条**上游已经做满 ⇒ 整条没动**（[`already_done`] ✓）。
    ///
    /// ⚠ 它同时是 `settle_*` 的**免罪符**：没动是**因为没得做** ✓，不是失败 ✗
    ///（2026-10-07 用户反馈：分享 `7/7` 之后每一趟都报失败 ✗）。
    pub already_done: bool,
    /// 434：**上游真把这一笔记上了** —— 唯一的判据是 `share_after > share_before` ✓。
    ///
    /// # 为什么不能看别的
    ///
    /// 官方**没有**"这条任务完成了"的接口 ⇒ 只能看次数（旧 TS
    /// `services/app/tasks.ts:113-125` 的 `getShareCount` + `printShareProgress`、
    /// 调用点在 `app.ts:30`（动手前记一次）与 `app.ts:56-60`（全跑完再拉整表比大小）✓）。
    /// 所以它**既不等于**"打卡门过了"（`clicked`），**也不等于**"外链建成了"
    /// （[`Self::shared`]）。
    ///
    /// # ⚠ 这一格是**本仓自己加的判据**，不是 TS 的做法（2026-10-07 对着 TS 原文核过）
    ///
    /// TS 里 434 **根本不进收尾判定**：`tasks/app/index.ts:42-50` 那一支是
    /// `if (task.id === 434) { …只打印进度…; continue; }` ✓ —— 它连
    /// `collectTaskResult` 都**不调用** ✗；而那份 `results.ts:33-45` 判的是
    /// `state === "FINISH"` + `doingList`（"本次跑过"），**与次数无关** ✗。
    /// ⇒ 本仓按"次数涨没涨"算 `acted` 是**用户 2026-10-04 的口径** ✓
    ///（依据：次数才是上游唯一的硬读数 ✓）—— 但**别**把它说成"参考实现也这么判" ✗
    ///（本行旧注释就是这么写的，2026-10-07 查实是**错的** ✗）。
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
    /// 上游在这一条**动手前**自报的状态（`"WAIT"` / `"DOING"` / `"FINISH"` …）。`None` = 没读到。
    ///
    /// # 它是"这一趟成没成"的**左半边**（用户口径 2026-10-07）
    ///
    /// 「**成功与否不是看你做了啥，而是状态的变化**」✓ —— 旧 TS 就是这么判的：
    /// `tasks/app/results.ts:collectTaskResult` 拿 `doingList`（本次动过）配上
    /// **复查后**的 `state === "FINISH"` ⇒ 成功；动了而没变 `FINISH` ⇒ **失败** ✓。
    ///
    /// ⚠ 左半边几乎恒为"非 FINISH" —— 因为 [`should_act`] 本来就不会把
    /// `FINISH` 的条目放进 `planned` ✓（真出现 `FINISH` 说明判据被改坏了）。
    pub state_before: Option<String>,
    /// 收尾复查（整表再拉一遍）读到的状态 —— 判定的**右半边** ✓。`None` = 读不出。
    pub state_after: Option<String>,
    /// 这一条**已经记过一次硬失败**（网络/解析/上游报错）。
    ///
    /// ⚠ 收尾复查**不再**给它补一条"状态没变"的失败 —— 同一条任务记两笔，
    /// 用户看到的失败数就比实际多 ✗（2026-10-07）。
    pub hard_failed: bool,
    /// 这一条**已经计进过 `acted`**（做那一件事的那一处 `acted += 1` ✓）。
    ///
    /// ⚠ 收尾复查要按**状态变化**逐条订正 `acted`（用户口径 2026-10-07）⇒
    /// 没有这枚标记就分不清"该撤回"还是"本来就该补上" ✗。
    pub acted_counted: bool,
    /// 逐条的原因。
    pub problems: Vec<String>,
}

/// 这一条**动过没有**（技术动作层面，**不等于**上游记上了 ✓）。
///
/// ⚠ 它是收尾复查的**入口条件**：没动过的条目没有"变化"可言 ✓。
fn acted(s: &TaskStep) -> bool {
    // ⚠⚠ `acted_counted` **必须算进来**（2026-10-07 真机逮到，用户截图）：
    // 有几档 runner 干活的方式**既不打门上的卡、也不走"上传 N 个"** —— 典型是 1014
    //（PDF 转换）：上游那一步已经把 `currstep` 走到 1 了 ⇒ `should_click` 为假 ⇒
    // `clicked=false`；而它生成/上传那份 PDF 走的是 runner 内部，`uploaded` 也没加 ⇒
    // 三个条件**全为假** ⇒ 这条**整个跳过收尾复查**、日志里一行结论都没有 ✗，
    // 而 AI豆 那边明明 +10 记上了 ✓（用户看到的就是这个"日志和结果不一样"✗）。
    // `acted_counted` 是"我们确实动了它"的**总账**（每一处 `acted += 1` 都留痕 ✓）
    // ⇒ 用它当入口条件才不漏 ✓。
    s.uploaded > 0 || s.clicked || s.shared || s.acted_counted
}

/// 收尾复查给一条条目下的结论（[`settle_progress`] ✓）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepVerdict {
    /// 上游的状态/读数**真的变了** ⇒ 这一趟做成了 ✓。
    Confirmed,
    /// 读到了状态/读数，但**没变** ⇒ 没记上 ✓。
    Unchanged,
    /// 读不出（拉表失败 / 表里没这条）⇒ 说不清，别硬判 ✗。
    Unknown,
}

/// **由状态的变化**判这一趟成没成（用户口径 2026-10-07：
/// 「成功与否不是看你做了啥，而是状态的变化」✓）。
///
/// 判据次序（能用的最硬读数优先）：
///
/// 1. `state` 变成了 `FINISH` ⇒ 成 ✓（旧 TS `results.ts` 判的就是这一条 ✓）；
/// 2. 名字里的 `a/b` 变大了（434 次数 / 522 进度）⇒ 成 ✓ —— 它比 `state` 更细：
///    522 要传满 100 个才 `FINISH`，而"这一趟 +5 个"本身就是变化 ✓；
/// 3. 三个读数**一个都没读到** ⇒ [`StepVerdict::Unknown`]（不是"没变" ✗）；
/// 4. 其余 ⇒ [`StepVerdict::Unchanged`]（读到了、没变 ⇒ 没记上 ✓）。
fn verdict_of(s: &TaskStep) -> StepVerdict {
    if s.state_after.as_deref() == Some("FINISH") {
        return StepVerdict::Confirmed;
    }
    let moved = |before: Option<&str>, after: Option<&str>| -> bool {
        match (before.and_then(parse_ratio), after.and_then(parse_ratio)) {
            (Some(b), Some(a)) => a > b,
            _ => false,
        }
    };
    if moved(s.share_before.as_deref(), s.share_after.as_deref())
        || moved(s.progress_before.as_deref(), s.progress_after.as_deref())
    {
        return StepVerdict::Confirmed;
    }
    let read_anything =
        s.state_after.is_some() || s.share_after.is_some() || s.progress_after.is_some();
    if !read_anything {
        return StepVerdict::Unknown;
    }
    StepVerdict::Unchanged
}

/// 把"动手前 → 动手后"写成一行，给日志与明细用 ✓（读不出的那一侧写 `读不出` ✓）。
fn change_line(s: &TaskStep) -> String {
    if s.share_before.is_some() || s.share_after.is_some() {
        return format!(
            "次数 {} → {}",
            s.share_before.as_deref().unwrap_or("读不出"),
            s.share_after.as_deref().unwrap_or("读不出")
        );
    }
    if s.progress_before.is_some() || s.progress_after.is_some() {
        return format!(
            "进度 {} → {}",
            s.progress_before.as_deref().unwrap_or("读不出"),
            s.progress_after.as_deref().unwrap_or("读不出")
        );
    }
    format!(
        "状态 {} → {}",
        s.state_before.as_deref().unwrap_or("读不出"),
        s.state_after.as_deref().unwrap_or("读不出")
    )
}

/// 这一趟的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppTaskResult {
    /// **这一趟真的推进了的条目数** —— 逐档按各自**能拿到的最硬判据**算：
    ///
    /// * `Upload` / `Entry`：传过文件**或**打过卡（那两档除了"发过写请求"没别的可看 ✓）；
    /// * `434`：**上游次数涨了**（[`TaskStep::share_counted`] ✓）—— 打卡过、外链建成都**不算**。
    ///
    /// ⚠ 旧 TS 的 `doingList`（`services/app/tasks.ts:29-36`）记的是"打卡门过了"，而它的
    /// 收尾判定 `collectTaskResult`（`results.ts:33-45`）看的是 `state === "FINISH"` ✓ ——
    /// **434 两条都不走**：`tasks/app/index.ts:42-50` 对它 `continue` ✓（只打印进度 ✗）。
    /// ⇒ 本仓 434 取"次数涨没涨"这条更硬的读数，是**用户 2026-10-04 的口径** ✓，
    /// **不是**"TS 也这么判" ✗（2026-10-07 对 TS 原文核过：旧注释那句是**错的** ✗）。
    pub acted: usize,
    /// 判出来"要传"的条目数。
    pub planned: usize,
    pub steps: Vec<TaskStep>,
    /// 我们造的文件 id（**`exist` 那一档不进来** —— 那是用户自己的文件）。
    pub file_ids: Vec<String>,
    pub cleaned_up: usize,
    pub cleanup_error: String,
    pub failures: Vec<String>,
    /// 动了、但**没得到上游确认**的条目 id（2026-10-04 复审 `tk-1` 新增的这一格）。
    ///
    /// ⚠ **不参与成败判定**（口径 B，用户 2026-10-04 拍板：「只加诚实标注」）——
    /// [`Self::task_outcome`] 看都不看它。它唯一的用途是把"我们其实**没核对**"
    /// 这件事从 summary 里说出来，别让"N 条"被读成"N 条都验过了"。
    ///
    /// 填它的地方只有 [`Self::settle_unconfirmed`] **一处**（别散进各个 `settle_*`：
    /// 判定规则只有一份，才敢说它是同一把尺子）。
    pub unconfirmed: Vec<i64>,
    /// **上游已经做满、整条没动**的条数（[`already_done`] ✓）。
    ///
    /// ⚠ 它是"全都没动"与"全都没得做"的分界：`planned > 0 && acted == 0` 时，
    /// 只有它等于 `planned` 才说明**是没得做**（⇒ `skipped` ✓），否则仍是失败 ✗
    ///（2026-10-07 用户反馈：分享 `7/7` 之后每一趟都报失败 ✗）。
    pub full_skipped: usize,
    /// **本地记录说"今天已经成功过"、因此没跑第二次**的条数 ✓（用户口径 2026-10-07 ✓）。
    pub skipped_today: usize,
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
        // ⚠ 满了之后有两种去向，**别混着说** ✗：领了奖（`clicked` ✓）／门没让打、
        // 真的没动 ✓ —— 2026-10-07 用户口径：「7/7 就不应该再运行而是领取奖励」✓。
        let claimed = self
            .steps
            .iter()
            .filter(|st| st.already_done && st.clicked)
            .count();
        let idle = self
            .steps
            .iter()
            .filter(|st| st.already_done && !st.clicked)
            .count();
        if claimed > 0 {
            s.push_str(&format!("；{claimed} 条上游已做满（只领奖）"));
        }
        if idle > 0 {
            s.push_str(&format!("；{idle} 条上游已做满（没动）"));
        }
        if self.skipped_today > 0 {
            s.push_str(&format!(
                "；{} 条今天已经成功过（本地记录，不跑二次）",
                self.skipped_today
            ));
        }
        if !self.unconfirmed.is_empty() {
            s.push_str(&format!(
                "；另有 {} 条未获上游确认（明细见运行详情）",
                self.unconfirmed.len()
            ));
        }
        s
    }

    /// 结算"动了、但**没得到上游确认**"的条目（2026-10-04 复审 `tk-1`，口径 B）。
    ///
    /// # 它要解决的问题
    ///
    /// [`Self::acted`] 是**两套口径**合起来的：434 要"上游次数真涨" ✓，而上传/埋点只要
    /// "写请求发成功了" ✓ —— 那两档除了这个**没别的可看**（这是诚实的下限，不是偷懒）。
    /// 问题不在口径，在**表述**：summary 只说"做了 N 条"，读起来就是"N 条都确认成了" ✗。
    /// 同文件自己记着真机读数：106 `clicked:true` 而上游 `state` 恒 `WAIT`
    ///（`app_task.rs` 里那处真机记录）⇒ 那次"成功"其实是"**没核对**"。
    ///
    /// # 规则（有读数就比读数，没读数就明说"没核对"）
    ///
    /// * **434**：`share_counted`（次数涨了）⇒ 算确认 ✓；否则写"上游次数没涨（a → b）"；
    /// * **522 / 615**：跑前/跑后的进度比大小 ⇒ 涨了算确认 ✓，没动写"上游进度没动（a → b）"；
    /// * **其余**（106/113 这类压根**没有可比读数**的）：写"本档没有可核对的读数" ——
    ///   这是**如实**，不是判它失败 ✗。
    ///
    /// ⚠ 只写**真动过**的条目（`uploaded > 0 || clicked || shared`）：没动过的归
    /// [`Self::failures`] 管，别在这里重复念一遍 ✗。
    ///
    /// ⚠⚠ 这一格**不参与成败**：口径 A（"只认上游动了才算成功"）2026-10-04 被用户否过一次
    ///（"最后都是看次数的…"），所以这里只加标注、不动 `task_outcome` ✓。
    pub fn settle_unconfirmed(&mut self) {
        self.unconfirmed.clear();
        for step in &mut self.steps {
            if !acted(step) {
                continue;
            }
            // 两档读数都是"a/b"形状（434 是次数、522 是进度）⇒ 同一个比较器
            let moved = |before: Option<&str>, after: Option<&str>| -> bool {
                match (before.and_then(parse_ratio), after.and_then(parse_ratio)) {
                    (Some(b), Some(a)) => a > b,
                    // 任一侧读不出 ⇒ **不当成"没动"**……但也不能算确认 ⇒ 落到"未确认" ✓
                    _ => false,
                }
            };
            let confirmed = step.share_counted
                // ⚠⚠ **状态变成 `FINISH` 也算确认**（用户口径 2026-10-07：「成功与否不是
                // 看你做了啥，而是状态的变化」✓）—— 这一格是 2026-10-07 新加的：
                // 以前只认"次数/进度涨了"，于是 `409 / 431 / 1005` 这种**只有状态可比**
                // 的条目永远落在"未获上游确认"里 ✗（而 `431` 复查时明明已经 `FINISH` ✓）。
                || step.state_after.as_deref() == Some("FINISH")
                || moved(
                    step.progress_before.as_deref(),
                    step.progress_after.as_deref(),
                );
            // ⚠ 收尾复查已经判过"没记上"（进 `failures` ✓）的不再重复念一遍 ✗
            if confirmed || step.hard_failed {
                continue;
            }

            let why = if step.share_before.is_some() || step.share_after.is_some() {
                format!(
                    "上游次数没涨（{} → {}）",
                    step.share_before.as_deref().unwrap_or("读不出"),
                    step.share_after.as_deref().unwrap_or("读不出")
                )
            } else if step.progress_total.is_some() {
                format!(
                    "上游进度没动（{} → {}）",
                    step.progress_before.as_deref().unwrap_or("读不出"),
                    step.progress_after.as_deref().unwrap_or("读不出")
                )
            } else {
                "本档没有可核对的读数（打卡/上传成功 ≠ 任务记上）".to_string()
            };
            step.problems.push(format!("未获上游确认：{why}"));
            if let Some(id) = step.task_id {
                self.unconfirmed.push(id);
            }
        }
    }

    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        // ⚠ 一条都没做成、又确实判出该做 ⇒ 报失败，别报成"没事发生"。
        if self.planned > 0 && self.acted == 0 {
            // ⚠⚠ **但"全都没动"有两种来路**（2026-10-07 用户反馈：「分享任务 7/7 了还在做，
            // 做完 7/7 说失败」✗）：① 判出来了却做不成 ⇒ 失败 ✓；② **上游说全都做满了**
            // ⇒ 那是"没得做"，报失败就是把"今天没事干"说成"今天干砸了" ✗。
            // 判据：没失败、且**每一条** planned 都是"已做满"（`full_skipped == planned` ✓）。
            // ⚠ 两种"没得做"都算：上游已做满（`full_skipped` ✓）／本地记录说今天
            // 已经成功过（`skipped_today` ✓，用户口径 2026-10-07 ✓）。
            let nothing_to_do = self.full_skipped + self.skipped_today;
            if self.failures.is_empty() && self.planned > 0 && nothing_to_do == self.planned {
                return TaskOutcome::skipped(format!(
                    "{summary} —— 判出的 {nothing_to_do} 条这一趟没有要做的 ✓"
                ));
            }
            return TaskOutcome::failed(summary);
        }
        if self.planned == 0 {
            return TaskOutcome::skipped(summary);
        }
        // ⚠⚠ **有硬失败就不许报"成功"**（2026-10-05 真机：86 发 create 全被上游
        // `exist:null` 打死，而结论因为别的条目"打卡成功"报了 `success` ✗✗）。
        // 这与 2026-10-03 用户报的「下面的 614 失败还直接说成功」是**同一类**事故 ✗ ——
        // 那次的解法就是 [`TaskOutcome::Partial`]，这里照同一条口径接上 ✓：
        // 有成功也有失败 ⇒ 部分成功 ✓（不是成功、也不是全败 ✓）。
        if !self.failures.is_empty() {
            return TaskOutcome::partial(summary);
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
    // ⚠ 专用执行器之间**共用一份状态**（旧 TS `ctx.state.mail139` 那个缓存）：
    // 一次运行只登一次邮箱，472/473/1004/1014/1020/1028 各自复用同一份 sid/rmkey ✓。
    let mut runner_state = RunnerState::new();
    // ⚠ 取消**不能**用 `?` 直接窜出去（2026-10-04 复审 tk-2）：这条链会往用户网盘里
    // 传文件，而清理挂在链尾 —— `?` 一窜就绕过了它，这一趟传上去的文件**永远留着**。
    // 所以取消记在这里，先走完收尾（清理 + 事后读数），最后再把取消原样报出去。
    let mut cancelled: Option<mcloud_core::Error> = None;

    // ① 读整张表（十六组，逐次容错）。
    let (pages, fetch_failures) = app_task_matrix::fetch_all(ctx).await?;
    // 一次失败不算失败 —— 登记它、继续拉下一组（与只读那一半同口径）。
    result.failures.extend(fetch_failures);
    let (items, _raw) = app_task_matrix::merge_pages(&pages);
    // 收尾那两条特例打卡要不要做（旧 TS 每趟都重新拉表判；本仓只在表里真出现过
    // 478/481 时才多拉一次 —— 理由见 [`settle_random_cloud_clicks`]）。
    let had_random_cloud_special = items.iter().any(|i| matches!(i.task_id, Some(478 | 481)));

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
        // ⚠ 同理：取消也要先收尾（这一趟可能已经传了文件）⇒ 记下来、跳出，
        // 不是 `?` 出去。见函数开头 `cancelled` 的说明。
        if let Err(e) = ctx.check_cancelled() {
            cancelled = Some(e);
            break;
        }
        // ⚠ 上游的 name 里会带 HTML（见 `clean_task_name` 的说明）⇒ 进记录前清掉 ✓
        let name = clean_task_name(&item.name.clone().unwrap_or_default());
        let mut step = TaskStep {
            task_id: item.task_id,
            name: name.clone(),
            // 动手前那一份状态 ⇒ 收尾复查拿它跟"动手后"比（用户口径 2026-10-07：
            // 「成功与否不是看你做了啥，而是状态的变化」✓）
            state_before: item.state.clone(),
            ..Default::default()
        };
        ctx.progress(
            result.steps.len() as u32 + 1,
            planned.len() as u32,
            format!("做任务 {name}"),
        );

        // ③-0a **本地记录：今天已经成功过 ⇒ 不跑第二次** —— 用户口径（2026-10-07）：
        // 「**第一次跑本地记录，成功不跑二次**」✓。理由见 [`SHARE_DONE_STATE_KEY`]：
        // 上游对同一天第二次分享不加数 ⇒ 不拦的话那趟必然读到"次数没动" ⇒ 报失败 ✗。
        if item.task_id == Some(SHARE_TASK_ID) {
            if let Some(day) = beijing_day_digits(&*ctx.clock) {
                if read_share_done_day(ctx).await.as_deref() == Some(day.as_str()) {
                    step.skipped_today = true;
                    result.skipped_today += 1;
                    ctx.info(format!(
                        "{name}：本地记录显示今天（{day}）已经成功过 ⇒ 不跑第二次"
                    ));
                    result.steps.push(step);
                    continue;
                }
            }
        }

        // ③-0 **上游已经做满 ⇒ 整条不动**（2026-10-07 用户反馈：「分享任务 7/7 了还在做，
        // 做完 7/7 说失败」✗）。连 `task/click` 都不发：那一发也是不可回滚的写 ✓，
        // 而对一条做满的条目它没有任何意义 ✗。判据见 [`already_done`]。
        if already_done(item) {
            step.already_done = true;
            result.full_skipped += 1;
            // 顺手把读到的进度记进明细（434 那格就是上游自己给的 `a/b` ✓）
            let progress = if item.task_id == Some(SHARE_TASK_ID) {
                step.share_before = item
                    .name
                    .as_deref()
                    .and_then(share_progress_from_name)
                    .map(|(a, b)| format!("{a}/{b}"));
                step.share_before
                    .clone()
                    .unwrap_or_else(|| "读不出进度".into())
            } else {
                match (
                    item.process,
                    upload_shape_for(item).map(|sh| sh.stop_at_process),
                ) {
                    (Some(p), Some(t)) if t > 0 => format!("{p}/{t}"),
                    _ => "读不出进度".into(),
                }
            };
            // ⚠⚠ **但"领奖"那一发照打** —— 用户口径（2026-10-07）：
            // 「**7/7 就不应该再运行而是领取奖励**」✓。这里"运行"指的是**分享**
            //（建外链 + 必要时补传一个文件 ✗），而**领取**就是 `task/click` 那一发 ✓
            //（旧 TS 的 `clickTask`，434 的例外步号 `22` 正是为它留的 ✓ —— 见
            // [`should_click`]；那个魔数 `{434: 22}` 现在有了解释：**分享满了 ⇒ 去领奖** ✓）。
            // ⚠ 门照旧走 `should_click`（**不绕过** ✗）：旧实现就是拿它决定打不打的 ✓
            // —— `currstep == 22` 才打 ✓，其余步号一律不打（照实说"没动" ✓）。
            if should_click(item) {
                match click(ctx, item).await {
                    Ok(notice) => {
                        step.clicked = true;
                        // 领到了就是**真推进** ✓（与 409 / 1005 那种"点一下就是全部"同口径 ✓）
                        step.acted_counted = true;
                        result.acted += 1;
                        ctx.success(format!(
                            "{name}：上游已做满（{progress}）⇒ 领奖那一发已打，不再重复做 ✓"
                        ));
                        // 用户口径 2026-10-07：领奖成功也算"今天成功过" ⇒ 记一笔 ✓
                        match beijing_day_digits(&*ctx.clock) {
                            Some(day) => {
                                if let Err(e) = save_share_done_day(ctx, &day).await {
                                    step.problems.push(format!("本地记录没写进去：{e}"));
                                }
                            }
                            None => step
                                .problems
                                .push("时钟读不出 ⇒ 这一笔没法记进本地记录".to_string()),
                        }
                        if let Some(n) = notice {
                            step.problems.push(format!("上游附言：{n}"));
                        }
                    }
                    Err(e) => {
                        // ⚠ **领奖没成是硬失败** ✗ —— 别让它被"已做满"这顶帽子盖住 ✗
                        //（用户口径：跳过 ≠ 通过 ✓）。
                        let msg = format!("{name}：领奖那一发没成：{e}");
                        step.problems.push(msg.clone());
                        result.failures.push(msg);
                    }
                }
            } else {
                ctx.info(format!("{name}：上游已经做满 ⇒ 这一趟不动它（{progress}）"));
            }
            result.steps.push(step);
            continue;
        }

        // ③-0c **click-only 那一档：打卡就是它的全部动作**，而且**不看 `currstep`**。
        //
        // 旧 TS `tasks.ts:97-100` 的 `default:` 分支：
        // `if (!SKIP_TASK_LIST.includes(task.id)) await clickTask(ctx, task.id);`
        // —— 它**不经过** `clickTaskWithStep` ⇒ 第 0 步之外也照打 ✓
        //（这是与 `Upload` / `Entry` / `Runner` 那三档**唯一**的行为差异，
        //  写在 [`Step::ClickOnly`] 的文档里）。
        //
        // ⚠ 黑名单由 `should_act` 那一层挡（`is_skipped_task` ✓），这一档到不了
        // 那些 id ⇒ 这里不必再判一次（判了也不会错，只是重复）。
        if matches!(step_kind, Step::ClickOnly) {
            match click(ctx, item).await {
                Ok(notice) => {
                    step.clicked = true;
                    step.acted_counted = true;
                    result.acted += 1;
                    if let Some(n) = notice {
                        step.problems.push(format!("上游附言：{n}"));
                    }
                }
                Err(e) => {
                    let msg = format!("{name}：打卡没成：{e}");
                    step.problems.push(msg.clone());
                    step.hard_failed = true;
                    result.failures.push(msg);
                }
            }
            result.steps.push(step);
            continue;
        }

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
        if let Step::Runner(runner) = step_kind {
            let report = runners::run_runner(
                ctx,
                &mut runner_state,
                *runner,
                item.task_id.unwrap_or(0),
                &mut result.file_ids,
            )
            .await;
            for n in &report.notes {
                ctx.info(format!("{name}：{n}"));
            }
            if report.acted {
                step.acted_counted = true;
                result.acted += 1;
            }
            // ⚠ 硬失败必须进 `failures`（那是摘要与运行级结论唯一的来源 ✓）。
            for f in &report.failures {
                step.problems.push(f.clone());
                // ⚠ 记过硬的之后，收尾复查**不再**给它补一条"状态没变" ✗（同一条记两笔）
                step.hard_failed = true;
                // ⚠⚠ **必须打日志**（2026-10-07 真机逮到，用户截图）：以前只 push 明细
                // ⇒ 摘要写着"4 条有失败"、日志里却只有 3 行 FAIL ✗（对不上账）。
                ctx.fail(format!("{name}：{f}"));
                result.failures.push(format!("{name}：{f}"));
            }
            for n in &report.notes {
                step.problems.push(n.clone());
            }
            result.steps.push(step);
            continue;
        }
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
                step.acted_counted = true;
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
                match upload_one_file(ctx, CloudChannel::Android, ".bin", &mut result.file_ids)
                    .await
                {
                    // ⚠ id **已经在 `upload_one_file` 里落账**（那边一拿到就 push）⇒
                    // 这里不再 push，否则同一个 id 会被删两次。
                    Ok(Some(_)) => {
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
            // ⚠ 取消：**不能** `?` 出去 —— 那会绕过链尾的清理，把这一趟已经传上去的
            // 文件永远留在用户网盘里（2026-10-04 复审 tk-2）。记下来、跳出上传循环，
            // 让链尾照常收尾（清理读的就是 `result.file_ids`，id 早已提前落账 ✓）。
            if let Err(e) = ctx.check_cancelled() {
                cancelled = Some(e);
                break;
            }
            // ⚠ 渠道按**这一档的形状**走：113 是 PC 档、别的是安卓档 ✓
            //（create 与 complete 两发用同一个 `shape.channel` —— 旧 TS 那两发也同源 ✓）。
            match upload_one_file(ctx, eff.channel, eff.file_ext, &mut result.file_ids).await {
                // ⚠ id **已经在 `upload_one_file` 里落账** ⇒ 这里不再 push。
                Ok(Some(_)) => {
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

        // ⚠⚠ **硬失败必须进 `failures`**（2026-10-05 真机踩过 ✗）：`step.problems` 只进明细，
        // 而摘要（"N 条有失败"）与运行级结论看的都是 `result.failures` ⇒ 上传**全废**那次，
        // 摘要一个字都没提，结论还因为别的条目"打卡成功"报了 success ✗✗。
        // ⚠ 每条**只记一次**：`problems` 里是一发一条，按条数记会把"1 条任务失败"说成"10 条" ✗。
        if let Some(first) = step.problems.iter().find(|p| p.starts_with("上传失败")) {
            step.hard_failed = true;
            result.failures.push(format!("{}：{first}", step.name));
        }
        if step.uploaded > 0 || step.clicked {
            step.acted_counted = true;
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

    // ⑤-2 收尾那两条特例打卡（478 / 481，旧 TS `index.ts:51-55`）。
    if had_random_cloud_special {
        settle_random_cloud_clicks(ctx, &mut result).await;
    }

    // ⑤′ 结算"动了、但**没得到上游确认**"的条目（2026-10-04 复审 tk-1，口径 B）：
    //     只往明细与 summary 里加诚实标注，**不改成败**（用户 2026-10-04 拍板）。
    result.settle_unconfirmed();

    // ⑥ 取消是"提前退出"，但**不是**跳过收尾的理由 —— 文件已经传上去了，
    //    清理与事后读数都已经跑完，现在把取消原样报出去（2026-10-04 复审 tk-2）。
    if let Some(e) = cancelled {
        return Err(e);
    }
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
/// `owned` 是**清理名单**（调用点传 `result.file_ids`）：一拿到 `fileId` 就落账，
/// 所以后面 PUT / complete 挂掉、甚至整趟被取消，那个文件仍会被链尾回收 ✓。
///
/// `channel` 是**必填**的：旧 TS 的 `uploadRandomFile` 缺省按安卓档发
///（`getFileOpHeader` 的 `channelSrc` 缺省值 ✓），只有玩法 113 显式改成 PC 档 ✓ ——
/// 把它做成参数而不是写死，正是为了"哪一档走哪个客户端"这件事**在调用点看得见** ✓。
async fn upload_one_file(
    ctx: &TaskContext,
    channel: CloudChannel,
    ext: &str,
    owned: &mut Vec<String>,
) -> Result<Option<String>> {
    let mut rng = mcloud_crypto::OsRandom;
    let plan = crate::task_expansion::random_plan_with_ext(&mut rng, 0, ext);
    upload_bytes(ctx, channel, &plan.name, &plan.content, owned).await
}

/// 传一个**内容与名字都给定**的文件（`create → PUT → complete` 三枪）。
///
/// # 为什么从 `upload_one_file` 里抽出来
///
/// 玩法 **1014（PDF 转换）** 要传的是一份**确定的 PDF**（文件名 `asign-pdf-<ts>.pdf`、
/// 内容是那 20 行最小 PDF ✓，旧 TS `runners.ts:296-343` 逐字），而
/// [`upload_one_file`] 传的是随机内容。抽出来之后两条路**共用同一套落账与自清**
/// —— 各写一份的后果是"PDF 那个文件链尾删不掉"，而那种失败**不报错**，
/// 只是永久留在用户网盘里 ✗。
///
/// ⚠ `owned` 落账在 **PUT / complete 之前**（与 [`upload_one_file`] 同一条理由：
/// 后面任何一步失败都会提前返回，不提前落账那个文件就永远留在用户网盘里 ✓）。
pub(crate) async fn upload_bytes(
    ctx: &TaskContext,
    channel: CloudChannel,
    name: &str,
    content: &[u8],
    owned: &mut Vec<String>,
) -> Result<Option<String>> {
    let hash = mcloud_crypto::sha256_hex_bytes(content);
    let (created, raw_body) = {
        let req = upload::file_create_request(CloudFileCreateInput {
            parent_file_id: "/".into(),
            // ⚠ 这一条是**手动上传**支（任务要求传文件），不是备份支：
            // 抓包篇 §3 那三处差里，手动支就是"按 id 定位 + force_rename"那一侧 ✓
            // （备份支才用 parentPath + refuse_duplicate_content，见 `task_expansion.rs`）。
            parent_path: None,
            name: name.to_string(),
            size: content.len() as i64,
            content_hash: hash.clone(),
            rename_mode: upload::CloudRenameMode::ForceRename,
            parallel_upload: false,
            local_created_at: None,
            channel,
            op: CloudOpType::Upload,
        })?;
        let resp = ctx.http.request(req).await?;
        (upload::parse_file_create(&resp.body)?, resp.body)
    };
    // ⚠ `exist` 是**三态**（2026-10-04 复审 up-1）：`Some(true)` = 撞同名（不是我们造的）、
    // `Some(false)` = 我们造的、`None` = 这一格形状变了。**读不出来按"不是我们造的"处理**：
    // 兜成 `false` 会把用户自己的文件记进清理名单 ⇒ 链尾删掉它 ✗。
    match created.owned_by_us() {
        Some(true) => {}
        // 撞同名 ⇒ 那个文件**不是我们造的**，交回 `None`（不落账、不清理）。
        Some(false) => return Ok(None),
        None => {
            // ⚠⚠ 这条错误**必须自带读数**：`exist` 读不出来时，唯一能回答"上游到底给了什么"
            // 的就是回执本身。2026-10-05 真机踩过：86 发 create 全部走到这里，而日志层
            // **只记请求、不记响应** ⇒ 排查时手上一个字节都没有，只能靠猜 ✗。
            // 正文先过 `redact`（authorization / token 之类 ✓）再按**字符**截断 ✓。
            let preview: String = mcloud_core::event::redact(&raw_body)
                .chars()
                .take(400)
                .collect();
            return Err(mcloud_core::Error::Parse(format!(
                "file/create 的 exist 读不出来（形状变了）⇒ 判不了那个文件是不是我们造的，按不清理处理；\
                 已解出的几格={created:?}；回执前 400 字={preview}"
            )));
        }
    }
    let Some(file_id) = created.file_id.clone() else {
        return Err(mcloud_core::Error::Parse("create 没给 fileId".into()));
    };
    // ⚠⚠ **提前落账**（在 PUT / complete 之前）：从这一格起网盘里那个文件就是我们造的，
    // 而下面两枪任何一步失败都会让本函数提前返回 —— 返回值里带不走 id，
    // 不在这里落账的话那个文件就**永远留在用户网盘里**（2026-10-04 复审 tk-3；
    // `task_expansion.rs` 早就是这么写的）。
    owned.push(file_id.clone());
    if !created.rapid_upload {
        let Some(url) = created.upload_url.clone() else {
            return Err(mcloud_core::Error::Parse("create 没给 uploadUrl".into()));
        };
        let resp = ctx
            .http
            .request(upload::upload_put_request(&url, content))
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
        "把 App 任务矩阵里能做的做掉：按玩法传文件、逐条打卡（传上去的文件跑完自己删）\
         —— 含 107 云笔记 / 139 邮箱族（1004 发信、1014 PDF、1020 备份、1057 AI 工作台）/ \
         618 AI 相机 / click-only 那 8 条（旧 TS 的 runners.ts 表）"
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
            // ⚠ 动了但**没核对到上游**的条目（口径 B 的诚实标注；不参与成败）✓
            "unconfirmed": result.unconfirmed,
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
                "alreadyDone": s.already_done,
                "skippedToday": s.skipped_today,
                // ⚠ 判"这一趟成没成"的两半边（用户口径 2026-10-07：「成功与否不是看你
                // 做了啥，而是状态的变化」✓）—— 留在这里，界面上能自己核对 ✓
                "stateBefore": s.state_before,
                "stateAfter": s.state_after,
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
