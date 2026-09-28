//! 拍拍系列（旧 TS 能力 `play-ai-special`）—— **协议层**。
//!
//! 这一族上游有五个口子，本文件只实现**前两个（都是只读）**：
//!
//! ```text
//! ① GET /ycloud/newyear/user/info?marketName=National_PlayAISpecial
//! ② GET /ycloud/newyear/task/list?marketName=National_PlayAISpecial&source=app
//! ```
//!
//! 判定与编排在 `mcloud_tasks::play_ai`。
//!
//! # 另外三口**不是漏掉**，是"该不该发"今天答不出来
//!
//! * ③ `POST …/task/step/click`（**写**）：实拍序列是"查 → 上报一步 → 回查"，
//!   也就是说上游确实**认**这一枪（`state` 从 `WAIT` 变 `FINISH`）。但旧 TS 把
//!   `visitlog/journaling` 埋点当**门槛**用（上报失败就连 `step/click` 都不发），
//!   而那两条埋点**零实拍** ⇒ 接不接门槛是待拍板的事，不是这里能顺手定的。
//! * ④ `POST …/blindbox/lottery`（**写**）：判据本该是 ① 那个 `total_lottery_count`，
//!   而这一格**语义未验证**（实拍那条 `1` 出现在**抽奖之前**，所以它至少含
//!   "还没花掉"的意思，但一次观测说不出花掉会不会减回去）。拿一个读不懂的数当
//!   "还能抽几次"去抽 = 猜，而且抽奖是**不可回滚**的写请求。
//! * ⑤ `POST https://ai.yun.139.com/api/image/aiRecognize`（**写网盘**）：
//!   它 `uploadToCloud` 实拍恒 `true` ⇒ 真的往用户网盘写一张图，而"识别出来的
//!   `fileId` 谁来删"那一族今天接口层冻结 ⇒ 不发。
//!
//! # 证据：一份连续窗口的归档，但**缺了链路的前半段**
//!
//! 夹具 `夏日拍拍大作战.json` —— 13 条、全部 200、含真实响应体，
//! 归档时间 2026-06-29 12:10–12:12（**一次连续的操作窗口**）。
//!
//! ⚠ 整份夹具里**没有**换票那两步（`querySpecToken` → `tyrzLogin`）的任何样本
//! ⇒ 这一页的 `jwttoken` **使用公共来源**（仓库现成的建会话入口）。
//! 这是**临时态不是终判**：后续抓到本页特有取法就直接改用，并**改写这一句**，
//! 不留"注释说公共、代码走特有"。
//! ⚠ 但 **Referer 不借**：用 [`play_ai_page_referer`]（旧 TS 逐字，**B 档**），
//! 不用会话里那份签到页的 referer。"票借、页面身份不借"这个组合**零实拍** ——
//! 真打时若回非零码，第一个要试的就是把 referer 换成会话自带的那一份。
//!
//! # ⚠⚠ 这一族的请求头**比仓库其余 H5 接口少两个**（实拍裁出来的）
//!
//! 实拍四条 `newyear/*` 带的头里**没有** `appversion`、**没有** `activityid`、
//! 也**没有** `deviceid`。
//!
//! 这不是脱敏抹掉的：同一份归档里活动门户那四条**都带**这三个头，
//! 说明归档器保留这些头名 ⇒ 是这一族**真的没发**。
//!
//! ⇒ 所以这里**不复用** `app_signin::h5()`（它会多加 `activityid` 与 `appversion`
//! 两个头）。代价是这一族的头集合单独维护一处；反过来"顺手复用"的表现是
//! 发出一条**夹具里不存在的请求**，而上游对多余头的反应**没有样本**。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};

use crate::app_signin::SignInEnvelope;
use crate::endpoints::MCLOUD_HOST;

/// 这一期的市场名。实拍三处都是它：两条 GET 的 `marketName` 查询参数、
/// 两条 POST 的 body、以及活动落地页里那个 `path`。
///
/// ⚠ 它是**期次**的一部分：春日/夏日/秋日拍拍是同一实现的不同期次，换一期它就换。
/// 写死在这里是为了让"换期"这件事**必须**改代码 + 过一次测试，
/// 而不是在运行时静默带上过期市场名。
pub const PLAY_AI_MARKET_NAME: &str = "National_PlayAISpecial";

/// ② 查询串里的 `source`，实拍恒 `app`。
pub const PLAY_AI_SOURCE: &str = "app";

/// `showloading` 的取值。
///
/// 取仓库里其余 H5 读接口在调用点上一直钉的那一个，理由是"别为这一族单独发明一个"。
/// ⚠ "上游认不认别的值""它是否参与判据"**未测试**。
pub const PLAY_AI_SHOW_LOADING: &str = "true";

/// 本页落地 URL 的 `sourceid`（旧 TS 的 Referer，**B 档**）。
///
/// ⚠ 它与埋点那三档 `sourceid` 中的 `2019` 同值 —— 同值不等于同一格：
/// 一个是页面入口标识、一个是埋点事件分组。
pub const PLAY_AI_PAGE_SOURCE_ID: &str = "2019";

/// 任务表 `state` 的两个实拍字面值。
///
/// ⚠ 刻意**不**引用签到族那份枚举：两族字面相同，但各自的**取值集**各自只由
/// 各自的样本支撑。借用过来会让"这一族哪天出现第三个值"看起来像上游改了枚举，
/// 而实际是我们引错了表。
pub const PLAY_AI_TASK_STATE_WAITING: &str = "WAIT";
pub const PLAY_AI_TASK_STATE_DONE: &str = "FINISH";

/// 这一页的 Referer（活动落地页）。
///
/// ⚠ **B 档**：实拍那六条的 `referer` 头**已脱敏**，所以"本页真实发出的 referer
/// 长这样"**没有实拍支撑** —— 这一串来自旧 TS 的 URL 模板，逐字。
pub fn play_ai_page_referer() -> String {
    format!(
        "{MCLOUD_HOST}/portal/yunpanpage/index.html?path={PLAY_AI_MARKET_NAME}&sourceid={PLAY_AI_PAGE_SOURCE_ID}&enableShare=1"
    )
}

/// 这一族的头集合 —— `Accept` / `Referer` / `showloading` **三片**。
///
/// `jwttoken` 由会话层（`H5Client`）注入，不在这里。
/// ⚠ **不发** `activityid` / `appversion` / `isdeviceid`：理由见文件头。
fn play_ai_headers(req: HttpRequest, referer: &str) -> HttpRequest {
    req.header("Accept", "*/*")
        .header("Referer", referer)
        .header("showloading", PLAY_AI_SHOW_LOADING)
}

// ══════════════════════════ ① 活动侧状态 ══════════════════════════

/// ① `GET …/ycloud/newyear/user/info?marketName=…` —— 只读。
///
/// ⚠ 这一条**只带 `marketName`**、**不带 `source`** —— 与 [`play_ai_task_list_request`]
/// 差一个键。实拍两次调用这一条的 URL 逐字相同，所以"少带/多带一个键"是**看得见**的
/// 偏离，测试按字节钉住。
///
/// 查询串用**字面拼接**而不是交给库排序：实拍串很短，逐字钉住更省心；
/// 而顺序交给库，哪天新增一个键顺序会**悄悄变**。
pub fn play_ai_status_request() -> HttpRequest {
    let url = format!("{MCLOUD_HOST}/ycloud/newyear/user/info?marketName={PLAY_AI_MARKET_NAME}");
    play_ai_headers(HttpRequest::get(url), &play_ai_page_referer())
}

/// ① 的解析结果。
///
/// # 为什么只有这一格
///
/// 实拍 `result` 有 `showData{prizeBg, prizeImages[6], titleImage}` 与
/// `totalLotteryCount` 两项。`showData` 全是**奖池图片素材**（渲染用），
/// 不参与任何判据 ⇒ 不声明（声明出来只会多一处会漂移的字段表）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayAiUserStatus {
    /// `result.totalLotteryCount`。
    ///
    /// 实拍两条：`0`（做完任务之前）→ 做完任务并复查时 `1`。
    ///
    /// ⚠ **语义未验证**："累计已获得几次"还是"当前还剩几次"两说 ——
    /// 这一窗口里 `1` 那条**出现在抽奖之前**，所以它至少含"还没花掉"的意思，
    /// 但一次观测说不出会不会被花掉后减回去。
    /// ⇒ **只存不判**：本仓今天也不据此发任何写请求。
    ///
    /// `Option`：`0` 是**合法取值**（实拍就有），必须与"上游没给"分得开。
    pub total_lottery_count: Option<i64>,
}

/// 解析 ①。
///
/// 没有 `result` **报错**：实拍两条都有（与"成败已由 code 说过"那种接口不同）。
pub fn parse_play_ai_status(body: &str) -> Result<PlayAiUserStatus> {
    let env = play_ai_envelope(body)?;
    let result = env.ok_result("拍拍活动 user/info")?;
    if !result.is_object() {
        return Err(Error::Parse("拍拍活动 user/info 的 result 不是对象".into()));
    }
    Ok(PlayAiUserStatus {
        total_lottery_count: opt_int_at(result, "totalLotteryCount"),
    })
}

// ══════════════════════════ ② 任务表 ══════════════════════════

/// ② `GET …/ycloud/newyear/task/list?marketName=…&source=app` —— 只读。
///
/// 实拍这一条出现**两次**，两次的 method / URL / 查询串**逐字相同**
/// （差别只在响应：中间夹了一次 ③ 上报，于是 `id:1` 那条从 `WAIT` 变 `FINISH`）。
/// ⇒ "同一端点两次"不是两种参数，别据此造两个变体。
pub fn play_ai_task_list_request() -> HttpRequest {
    let url = format!(
        "{MCLOUD_HOST}/ycloud/newyear/task/list?marketName={PLAY_AI_MARKET_NAME}&source={PLAY_AI_SOURCE}"
    );
    play_ai_headers(HttpRequest::get(url), &play_ai_page_referer())
}

/// 任务表里的一条。
///
/// # 为什么只声明这些
///
/// 实拍一条有 21 个键：另有 `button`（七份端形态，里面是链接与素材）、
/// `showData`（CSS 串）、`icon` / `img`（素材）、`rule`（一段 HTML 规则文案）、
/// `sort` / `source` / `currDay` / `updateTime`。那些是**渲染与投放**用的：
/// 本层要回答的是"有哪些任务、做没做、做一步要回传哪个 id"。
///
/// ⚠ `button.*.link` 里有 `mcloud://` 跳转与带 token 的第三方链接：
/// 那是**跳转与埋点凭据**，既不声明也不进夹具（脱敏位**不可当样本**）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayAiUserTask {
    /// `id` —— ③ 的 body 要回传它。
    pub task_id: Option<i64>,
    /// `name`（列表主文案，实拍「体验拍照问AI」「拍夏日关键词」「去小红书点赞」）。
    pub name: Option<String>,
    /// `flag` —— 实拍的短标签（「拍照问AI」「拍夏日关键词」「小红书」）。
    ///
    /// 声明它而不是只用 `name`：③ 上报时"哪一步"在实拍里靠 `key` + `taskId` 表达，
    /// 而 `flag` 是**唯一**能把任务与步对上眼的字面量
    /// （`button.*.ext` 三条任务全都一样）。
    pub flag: Option<String>,
    /// `description`（实拍三条都是「抽奖次数+1」，即**奖励**那一格）。
    pub description: Option<String>,
    /// `groupId`。
    ///
    /// ⚠⚠ **驼峰** `groupId`：签到任务表那族是全小写 `groupid`。这是两族的**真实差异**，
    /// 写成同一个键会有一族**静默读不到**（读到 `None`，而 `None` 是合法的"上游没给"）。
    pub group_id: Option<String>,
    /// `state`（实拍 `WAIT` / `FINISH`）。**不归一**：读不出原样就交 `None`。
    pub state: Option<String>,
    /// `limitType`（实拍 `DAY` / `EVER`）。
    pub limit_type: Option<String>,
    /// `recordid`。
    ///
    /// 实拍两种形态都出现过：`null`（没做过）与**字符串** `"20674897"`（做过）。
    /// ⇒ 按字符串收（数字形态零样本，不猜）。`None` = "上游给了 `null` 或没给"，
    /// 与"给了空串"是两回事。
    pub record_id: Option<String>,
    /// `currstep`（**全小写**，实拍 ② 两次之间 `id:1` 那条从 0 变 3）。
    pub curr_step: Option<i64>,
    /// `process`（实拍三条**全是 0**，包括已完成的两条）。
    ///
    /// ⚠ 所以"进度"这个口径在 `process` 上**说不通** —— 该读 `process` 还是
    /// `curr_step` **未验证**。两个都存，不加解释。
    pub process: Option<i64>,
}

/// 解析 ②，给出任务数组。
///
/// `result` 必须是**数组**：上游改成对象时在这里立刻响，不"顺手兼容两种形状"
/// （兼容会把形状变化藏起来）。
///
/// 单条不是对象时**报错**（而不是跳过）：任务表是本能力的判据来源，
/// 一条读不出来的记录比"少一条"更危险（调用方会以为那一步没做）。
pub fn parse_play_ai_task_list(body: &str) -> Result<Vec<PlayAiUserTask>> {
    let env = play_ai_envelope(body)?;
    let result = env.ok_result("拍拍活动 task/list")?;
    let Value::Array(items) = result else {
        return Err(Error::Parse("拍拍活动 task/list 的 result 不是数组".into()));
    };
    let mut out = Vec::with_capacity(items.len());
    for raw in items {
        if !raw.is_object() {
            return Err(Error::Parse(
                "拍拍活动 task/list 的 result 里有一条不是对象".into(),
            ));
        }
        out.push(PlayAiUserTask {
            task_id: opt_int_at(raw, "id"),
            name: opt_str_at(raw, "name"),
            flag: opt_str_at(raw, "flag"),
            description: opt_str_at(raw, "description"),
            group_id: opt_str_at(raw, "groupId"),
            state: opt_str_at(raw, "state"),
            limit_type: opt_str_at(raw, "limitType"),
            record_id: opt_str_at(raw, "recordid"),
            curr_step: opt_int_at(raw, "currstep"),
            process: opt_int_at(raw, "process"),
        });
    }
    Ok(out)
}

// ══════════════════════════ 信封 ══════════════════════════

/// 这一族的信封 —— `code` 必须是**数字字面量**。
///
/// # ⚠ 为什么不直接用 `SignInEnvelope::parse`
///
/// 那个解析器把**数字与数字字符串两种 `code` 都收**（它服务的是签到族，
/// 那边历史上两种都出现过）。而这一族实拍是**数字** `0`。
///
/// **AI 那族的 `code` 是字符串 `"0000"`** —— 拿一份 AI 族的响应喂给这里，
/// 宽松解析会让 `"0000"` 落成 `0`、也就是**判成成功**。这道检查就是为了
/// 在那一刻立刻响，而不是拿错族的响应当成这一族的读数。
///
/// 检查过类型之后**再**交给 [`SignInEnvelope::parse`]：`90001` 那条
/// "这是票的问题、该重建会话"的语义在那边，不重复实现一份。
fn play_ai_envelope(body: &str) -> Result<SignInEnvelope> {
    let raw: Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("拍拍活动响应不是 JSON：{e}")))?;
    match raw.get("code") {
        Some(Value::Number(_)) => {}
        Some(other) => {
            return Err(Error::Parse(format!(
                "拍拍活动的 code 不是数字字面量，实得 {other}（两族信封不同构，别混解）"
            )));
        }
        None => {
            return Err(Error::Parse(
                "拍拍活动响应没有 code 字段（这一族必带）".into(),
            ));
        }
    }
    SignInEnvelope::parse(body)
}

// ══════════════════════════ 小工具 ══════════════════════════

/// 取一个**可选**整数：缺键与"给了非数字"都回 `None`（不伪造成 0）。
fn opt_int_at(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// 取一个**可选**字符串：缺键、`null`、非字符串都回 `None`。
fn opt_str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests;
