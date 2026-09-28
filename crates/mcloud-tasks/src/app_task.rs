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

use mcloud_upstream::app_task_list_v2::TaskV2Item;

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
    /// **每传完一个**等一拍（旧 522 循环体末尾那句 `sleep(500)`，**包括最后一轮**）。
    ///
    /// ⚠ 它是 522 与 106 **唯一的形状差别**（旧实现 106 的 runner 体里没有这一句）。
    /// 把两者混成一个的后果是：要么 522 变成"连传 5 个零间隔"，要么 106 白等一拍
    /// —— 两种都**不会报错**，只在"哪天被判成脚本"那一类最难查的差异上显形。
    pub pace_each_round: bool,
}

/// 「就传一个」那一档（玩法 106 与 beiyong1 的名字门共用）。
///
/// 单独一个常量是为了让"106 与 beiyong1 走的是**同一档**形状"这件事可断言 ——
/// 旧 TS 两处都是裸的 `uploadRandomFile(ctx)`，没有任何参数差别。
pub const UPLOAD_ONE: UploadShape = UploadShape {
    rounds: 1,
    stop_at_process: 0,
    pace_each_round: false,
};

/// 522 那一趟最多传几个（旧实现的 `每月上传任务单日数量 ?? 5` 里那个默认值）。
///
/// ⚠ 本仓**没有**"每月上传任务单日数量"这一格配置（旧实现是可配的）⇒ 取旧实现那个
/// **默认值 5**。要开成参数面得连 CLI 与 WebUI 一起改，不在这一层单方面决定。
pub const ROUNDS_PER_RUN: u32 = 5;

/// 玩法 522 的 `process` 门槛（旧实现那个 `>= 100`）。
pub const STOP_AT_PROCESS: i64 = 100;

/// beiyong1 分组的名字门（旧实现逐字）：
/// `name.includes("上传") && (name.includes("图") || name.includes("照"))`。
///
/// ⚠ 旧实现同一支里**还有一句**看 `button.app.link` 含 `uploadView` ⇒ 走"补满 10 张"
/// 那条（`ai-tasks.ts` 的 `upload10PictureTask`）。**本模块不实现那一支**：
/// 我们的 `TaskV2Item` 里没有 `button` 那一格（解析器没读它），而"10 张"那条玩法
/// 的产物链（`aiRecognize` / `chat/v2/add` SSE）本仓**零实拍**。
/// ⇒ 留空并注明，不编一个"看起来合理"的入口。
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
        Some(106) => return Some(UPLOAD_ONE),
        Some(522) => {
            return Some(UploadShape {
                rounds: ROUNDS_PER_RUN,
                stop_at_process: STOP_AT_PROCESS,
                pace_each_round: true,
            })
        }
        _ => {}
    }
    if item.group_id.as_deref() == Some("beiyong1") {
        if let Some(name) = item.name.as_deref() {
            if beiyong1_name_gate(name) {
                return Some(UPLOAD_ONE);
            }
        }
    }
    None
}

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

#[cfg(test)]
mod tests;
