//! 任务清单第二口：`taskListV2`（**App 任务矩阵**那张表，按 group 分次拉取）—— 协议层。
//!
//! `POST /ycloud/signin/task/taskListV2`
//!
//! 判定与聚合在 `mcloud_tasks::app_task_matrix`。
//!
//! # 为什么与 `taskListV3` 分两份实现，而不是"同一件事换个版本号"
//!
//! 两口的 **body 与响应形状都不同**：
//!
//! * `taskListV3`：body 多一枚 `client`、**没有** `group`，响应的 `result` 是**数组**。
//! * **这一口**：body 多一枚 `group`、**没有** `client`，响应的 `result` 是
//!   **以所请求的 group 名为键的对象**（值为数组）。
//!
//! 把两者折成一个解析器的表现是：哪天 `result` 从数组变成对象，兼容代码会"顺手"
//! 两种都吃，于是这条链在形状变化上**不会响**。所以各写一份，宁可重复几行取字段。
//!
//! # 证据（本仓夹具，两条同端点实拍）
//!
//! `3d.json` 里两条 `POST …/ycloud/signin/task/taskListV2`：
//!
//! ```text
//! body {"clientVersion":"12.6.0","group":"new","marketname":"sign_in_3"}
//!   → {"code":0,"msg":"success","result":{},"groupState":{}}      ← result 是空**对象**，不是空数组
//! body {"clientVersion":"12.6.0","group":"beiyong1","marketname":"sign_in_3"}
//!   → result.beiyong1 三条（id 596/597/598，state 全 FINISH、enable 全 1、
//!     process 全 0、currstep 4/6/8），groupState {"beiyong1":{"taskFinishState":2}}
//! ```
//!
//! ⚠ 这一口的请求头与拍拍那一族**正好相反**：它**带** `appversion` / `activityid` /
//! `deviceid`，还有 `origin` 与 `content-type`。
//!
//! # `activityid` 跟着本次的 `marketname` 走
//!
//! ⚠ 它**不是**恒等于签到页那个 `sign_in_3`：旧 TS 每次 POST 都带
//! `activityid: <本次的 marketname>`。所以这里在 `h5()`（它固定写 `sign_in_3`）
//! 之后**先剥再补**，而不是"再 append 一枚同名头" ——
//! [`HttpRequest::header`] 是**追加**（`push`），同名两枚头会让
//! "哪枚生效"变成传输层的事，而实拍只有一枚。

use serde_json::Value;

use mcloud_core::{HttpRequest, Result};

use crate::app_signin::{h5, SignInEnvelope};
use crate::endpoints::MCLOUD_HOST;

const TASK_LIST_V2_PATH: &str = "/ycloud/signin/task/taskListV2";

/// body 里的 `clientVersion` —— **三段** `12.6.0`。
///
/// ⚠ 别写成四段：四段那个是请求头 `appversion`（`12.6.0.0`），
/// 旧实现里两个拼法同时出现，混用会得到一个"看着对"的 body。
pub const TASK_CLIENT_VERSION: &str = "12.6.0";

/// 这一口的第二个市场名（邮箱那份任务表）。
///
/// 旧 TS 把它钉成 `"sign_in_3" | "newsign_139mail"` 两值；本层不枚举成"只准这两个"
/// （上游哪天加第三个时这里不该成为阻塞点），但常量必须成文，因为
/// `activityid` 要跟着它变。
pub const TASK_LIST_V2_EMAIL_MARKET: &str = "newsign_139mail";

/// 旧实现按序请求的**八个 group**（字面量与顺序）。
///
/// ⚠ 两处"类型与事实不一致"留在旧 TS 里，这里按**请求面**那一份为准：
/// `hiddenabc` 出现在旧 TS 的类型与结果分组文案里，却**不在**这个数组中（从不请求）；
/// `beiyong2` 在这个数组里，却**不在**旧 TS 的类型里。⇒ 以数组为准。
///
/// ⚠ 顺序**是**行为的一部分（旧实现串行请求，合并时先来的排在前面）。
pub const TASK_LIST_V2_GROUPS: [&str; 8] = [
    "new",
    "beiyong1",
    "beiyong2",
    "cloudEmail",
    "time",
    "day",
    "month",
    "hidden",
];

/// 构造 `POST /ycloud/signin/task/taskListV2` —— **只读**。
///
/// body 三键逐字：`marketname`、`clientVersion`、`group`。**没有** `client`。
pub fn task_list_v2_request(marketname: &str, group: &str) -> HttpRequest {
    let body = serde_json::json!({
        "marketname": marketname,
        "clientVersion": TASK_CLIENT_VERSION,
        "group": group,
    });
    let req = h5(
        false,
        HttpRequest::post(format!("{MCLOUD_HOST}{TASK_LIST_V2_PATH}")).json_body(&body),
        "true",
    );
    // 先剥掉 h5() 里那枚固定的 `sign_in_3`，再补上这一支的（理由见文件头）。
    let mut req = req;
    req.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case("activityid"));
    req.header("activityid", marketname)
}

/// `POST /ycloud/signin/task/taskListV3` —— **只读** ✓。
pub const TASK_LIST_V3_PATH: &str = "/ycloud/signin/task/taskListV3";

/// 构造 `POST /ycloud/signin/task/taskListV3` —— **只读** ✓。
///
/// ⚠⚠ **body 与 V2 同形，但 `group` 是空串** ✓ —— 实拍（2026-10-11 云盘 App 云朵中心）：
/// Reqable 没存下 body ✗（`request.body: null`），但 `content-length: 62` ✓ ⇒ 逐字节对出来的
/// 就是 `{"marketname":"sign_in_3","clientVersion":"12.6.0","group":""}` ✓
/// （`{`1 `"marketname"`12 `:`1 `"sign_in_3"`11 `,`1 `"clientVersion"`15 `:`1 `"12.6.0"`8
/// `,`1 `"group"`7 `:`1 `""`2 `}`1 = **62** ✓）。
/// ⇒ **空 `group` = 一次拿整个市场** ✓（这就是它 1 发能顶 V2 那 8 发的原因 ✓）。
///
/// ⚠ 本文件顶上那句旧说明（"V3 body 多一枚 `client`、没有 `group`" ✗）是**更早**的抓包读数 ✓，
/// 已被这一发**覆盖** ✗ —— 按"抓包 > 推断"取新的 ✓（旧形状发出去会 404 ✗，实测过 ✓）。
/// 空 body / 带 `client` 两种写法都试过：**都 404** ✗。
pub fn task_list_v3_request(market: &str) -> HttpRequest {
    let req = h5(
        false,
        HttpRequest::post(format!("{MCLOUD_HOST}{TASK_LIST_V3_PATH}")).json_body(
            &serde_json::json!({
                "marketname": market,
                "clientVersion": TASK_CLIENT_VERSION,
                "group": "",
            }),
        ),
        "true",
    );
    let mut req = req;
    req.headers
        .retain(|(k, _)| !k.eq_ignore_ascii_case("activityid"));
    req.header("activityid", market)
        .header("content-type", "application/json;charset=UTF-8")
}

/// `taskListV3` 里**V2 没有**的那几格 ✓。
///
/// ⚠⚠ 两个口**不是"同一件事换个版本号"** ✗ —— 实拍同一天同一条任务（551 ✓）：
///
/// ```text
/// V3  buttonProperty.app = {"text":"开启中","day":"1","total":31,"canReceive":0}
/// V2  button.app         = {"text":"去开启","textMiddle":"开启中","textFinish":"领AI豆"}
/// ```
///
/// ⇒ **进度（`day` / `total`）只在 V3 里** ✓ —— App 上那个「完成进度 **(1/31)**」
/// 就是这两格 ✓（`total` 是上游**直接给的** ✓，不是从名字里猜的 ✗）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct V3Overlay {
    pub text: Option<String>,
    pub text_middle: Option<String>,
    pub text_finish: Option<String>,
    pub action_middle: Option<String>,
    pub day: Option<String>,
    pub total: Option<i64>,
    pub can_receive: Option<i64>,
    pub description: Option<String>,
}

/// 从 `taskListV3` 响应里取 `id → [`V3Overlay`]` ✓。
///
/// ⚠ 这一口的 `result` 是**数组**（V2 是按 group 分组的**对象** ✓）—— 两者别混解 ✗。
/// 只认 [`V3Overlay`] 那几格，**不抄整条** ✓（V2 那份才是主条目 ✓）。
pub fn parse_v3_overlay(body: &str) -> Result<std::collections::HashMap<i64, V3Overlay>> {
    let env = SignInEnvelope::parse(body)?;
    env.ensure_ok("读取 App 任务矩阵（V3）")?;
    let mut out = std::collections::HashMap::new();
    let Some(arr) = env.result.as_ref().and_then(Value::as_array) else {
        return Ok(out);
    };
    for raw in arr {
        let Some(obj) = raw.as_object() else { continue };
        let Some(id) = obj.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let get = |k: &str| obj.get(k);
        // 形态优先级与条目那份**同一口径** ✓（`email` → `app` → `out` ✓，`buttonProperty` 先于 `button` ✓）。
        let shape = ["buttonProperty", "button"]
            .iter()
            .filter_map(|k| get(k))
            .flat_map(|b| {
                ["email", "app", "out"]
                    .iter()
                    .filter_map(move |s| b.get(*s))
                    .collect::<Vec<_>>()
            })
            .next();
        let str_at = |v: Option<&Value>, k: &str| {
            v.and_then(|o| o.get(k))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        out.insert(
            id,
            V3Overlay {
                text: str_at(shape, "text"),
                text_middle: str_at(shape, "textMiddle"),
                text_finish: str_at(shape, "textFinish"),
                action_middle: str_at(shape, "actionMiddle"),
                day: str_at(shape, "day"),
                total: shape.and_then(|o| o.get("total")).and_then(Value::as_i64),
                can_receive: shape
                    .and_then(|o| o.get("canReceive"))
                    .and_then(Value::as_i64),
                description: str_at(Some(raw), "description"),
            },
        );
    }
    Ok(out)
}

/// `taskListV2` 响应里的一条任务。
///
/// # 取哪些字段
///
/// 实拍一条有 **25 个键**（另有 `button` / `buttonProperty` 七份端形态、
/// `steps` / `stepTypeSet`、`starttime` / `endtime`、素材与 `sort` 等）。
/// 本层要回答的是"这张表里有哪些任务、各在哪一步、还能不能做"，
/// 所以只取身份 + 状态 + 那三个门。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskV2Item {
    /// `id`。
    pub task_id: Option<i64>,
    /// `name`（列表主文案）。
    pub name: Option<String>,
    /// `description`（实拍是奖励文案，如「永葆童心 领38云朵」）。
    pub description: Option<String>,
    /// `groupid`。
    ///
    /// ⚠⚠ **全小写** —— 而拍拍那一族是**驼峰** `groupId`。这是两族的**真实差异**
    /// （实拍逐字比对过），写成另一个会有一族**静默读不到**。
    pub group_id: Option<String>,
    /// `state`（实拍 `FINISH`）。不归一：读不出原样就交 `None`。
    pub state: Option<String>,
    /// `limitType`（实拍 `EVER` / `DAY`）。
    pub limit_type: Option<String>,
    /// `enable` —— 旧实现的门是 `enable != 1` ⇒ 跳过。
    pub enable: Option<i64>,
    /// `currstep` —— 旧实现把它带进 `task/click` 的 `step` 参数。
    ///
    /// ⚠ 实拍三条是 `4/6/8` 而 `state` 已经全是 `FINISH`。
    pub curr_step: Option<i64>,
    /// `process`。
    ///
    /// ⚠ **夹具那三条恒为 0**，而同三条的 `state` 已是 `FINISH`、`currstep` 是 4/6/8
    /// ⇒ "进度到底读哪个字段"**未验证**。
    /// ⚠ 而且"恒为 0"只是**那一份夹具**的形状，不是这一格的性质：
    /// 2026-09-25 两个真账号现场读到过 `0 / 9 / 59 / 64 / 69 / 73 / 74 / 100`。
    /// ⇒ 只存不判。
    pub process: Option<i64>,
    /// `marketname`（实拍 `sign_in_3`）。
    ///
    /// ⚠ 邮箱那一支（`newsign_139mail`）**回数据、且同形**，但它回来的条目里
    /// `marketname` **混着**给 `newsign_139mail` 与 `sign_in_3` ⇒ 两边不是干净的两批表。
    pub market_name: Option<String>,
    /// `button.app.link` —— 「传 10 张」那一族**唯一的门**就在这一格 ✓。
    ///
    /// # 实拍（抓包仓 `har/2026-09-25-通知有礼.har` 的 `taskListV3` 响应）
    ///
    /// ```json
    /// "button": {"app": {"text": "去完成", "action": "jump", "link": "mcloud://main/uploadView"}}
    /// ```
    ///
    /// 同页命中这一格的条目：`id=614`（`group=beiyong1`、`name=上传中秋团圆照`、
    /// `description=上传10张可领`、`process=6`、`state=WAIT`）、以及 `id=106` / `id=522`
    /// —— 后两条**在别处已被 id 认领** ⇒ 这一格真正派上用场的是 beiyong1 那一族 ✓。
    ///
    /// 旧 TS `tasks.ts:78-81` 的 `beiyong1` 分支正是：
    /// `if ((task.button.app as Beiyong1App)?.link?.includes("uploadView")) { await upload10PictureTask(task); }`
    /// ⇒ 命中就"传 (10 − process) 张"✓（`ai-tasks.ts:190-224`）。
    ///
    /// ⚠ 只取这一格、**不**抄整份 `button`：这条链只判"是不是那个按钮" ✓
    ///（那 25 个键里其余的是七份端形态、素材、排序之类，本层不判 ✓）。
    /// `buttonProperty.out.text` —— **上游自己给这一条、这一状态写的那句话** ✓。
    ///
    /// ⚠⚠ **别拿它当状态词**（2026-10-11 用户口径：「与原接口文案一致」）：
    /// 它是**逐条**的 —— 真机实拍同一趟里 `state=WAIT` 的两条，
    /// 1029「查看"我的账单"」是 **「去完成」** ✓，而「连续31天开启邮箱APP通知」是 **「开启中」** ✓；
    /// `state=FINISH` 的「下月签到领惊喜福利」是 **「次月可领」** ✓。
    /// ⇒ 界面若一刀切写「待做」，就把"上游已经给了具体动作"这件事盖掉了 ✗。
    ///
    /// 取 `buttonProperty.out`（**外部端**那一份 ✓ —— WebUI 就是外部端 ✓），
    /// 缺了再回落 `button.out` ✓；两层都缺 / 不是字符串 ⇒ `None`（界面回落状态词 ✓）。
    pub button_out_text: Option<String>,
    /// `total` —— 上游**直接给的**分母 ✓（实拍 V3 的 551 是 `31` ✓）。
    ///
    /// ⚠ 只有 **V3** 给这一格 ✗（V2 没有 ✓）⇒ 由 [`crate::app_task_list_v2::parse_v3_overlay`]
    /// 并进来 ✓。有了它就**不用**从名字里猜「连续N天」✗。
    pub button_out_total: Option<i64>,
    /// `content_display2` —— 条目上那句**奖励**（实拍 550 是 `"+200豆"` ✓）——
    /// App 那一行「完成进度 (5/31) **+100豆**」里的「+100豆」就是它 ✓。
    pub reward_text: Option<String>,
    /// `textMiddle` —— **进行中**那一档（实拍 551/1079 是「开启中」✓、550 是「领AI豆」✓）。
    pub button_text_middle: Option<String>,
    /// `textFinish` —— **已完成**那一档（实拍 550 是「已领取」✓、551/1079 是「领AI豆」✓）。
    pub button_text_finish: Option<String>,
    /// `actionMiddle` —— **"在不在进行中"就看这一格** ✓（实拍 551/1079 是 `"jump"` ✓、550 是 `""` ✓）。
    ///
    /// ⚠ 它本身是"点了要跳哪" ✓，但**空/非空**恰好区分了阶段 ✓：
    /// 550（没有中间动作）⇒ 界面显示 `text`「去开启」✓；
    /// 551/1079（有中间动作）⇒ 显示 `textMiddle`「开启中」✓ —— 与 App 一致 ✓。
    pub button_action_middle: Option<String>,
    /// `button.out.day` —— **上游给的"进行到第几天"** ✓（**字符串**，实拍 `"5"` ✗ 不是数字 ✗）。
    ///
    /// 出处：真机实拍任务 **1021「连续31天开启邮箱APP通知」**（抓包仓
    /// `app-task-run-account-samples.txt` §12.6 ✓）：
    ///
    /// ```json
    /// "button": { "out":   { "text": "开启中", "day": "5", "canReceive": 0 },
    ///             "email": { "itemBg": "#fff6e5", "btnBg": "#ffa939",
    ///                        "text": "开启中", "day": "5", "canReceive": 0 } }
    /// ```
    ///
    /// ⇒ **进度不在 `process` 里** ✗（那条 `process` 是 0 ✓），就在这一格 ✓ ——
    /// 界面此前只读 `process`，于是"App 里有进度、这里没有" ✗（2026-10-11 用户口径 ✓）。
    /// ⚠ 同一条任务**没有** `buttonProperty`（只有 `button` ✓），而 1029 反过来有 ✓
    /// ⇒ 两处都取，`buttonProperty.out` 优先 ✓（与 [`Self::button_out_text`] 同一口径 ✓）。
    pub button_out_day: Option<String>,
    /// `button.out.canReceive` —— 上游说"现在能不能领" ✓（实拍 `0` ✓；`1` 才算能领 ✓）。
    ///
    /// ⚠ 它和"任务完成没完成"**不是一回事** ✗（实拍 1021 是 `WAIT` + `canReceive=0` ✓）。
    pub button_can_receive: Option<i64>,
    pub button_app_link: Option<String>,
    /// `button.email.link` —— **邮箱端**那一档的入口（实拍是短链 `https://y.10086.cn/x/<码>`
    /// 或带 `token=#ssoToken#` 占位的落地页）。
    ///
    /// ⚠ 2026-10-10 真机读数（临时取证，见 `docs/app-task-status.md` §5-11）：**十来个条目都有这一格**
    /// （548/549/522/1075/1005/1017/1004/1014/1020 …）⇒ 它**不是**"某一条任务的特征"，
    /// 谁拿它做什么**必须逐条看** ✗（本仓今天只有 **1017** 会去取它 ✓）。
    ///
    /// `#ssoToken#` 是**上游给的占位**（1075 与 1017 两条实拍都带它）⇒ 真发之前要换成真票 ✓。
    pub button_email_link: Option<String>,
}

/// **一次**（一个 group 的）`taskListV2` 响应的解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskListV2 {
    /// **请求时**要的那个 group 名 —— 解析器不猜它，由调用方传进来。
    ///
    /// 为什么需要：实拍 `result` 这个对象里只有本次那一个键，但"只有一个键"是
    /// **一次观测、不是契约**（旧 TS 是把全部值摊平，不看键名）。
    /// 所以这里同样**全摊平**，而把请求的那名留着做日志与对账。
    pub group: String,
    /// 把 `result` 的**所有**值摊平后的条目（键名不参与筛选）。
    pub items: Vec<TaskV2Item>,
    /// 顶层 `groupState.<group>.taskFinishState`，按 group 存。
    ///
    /// ⚠ **只存不判**：夹具里一次是 `{}`、一次是 `{"beiyong1":{"taskFinishState":2}}`
    /// （而同一份响应里三条**全是** `FINISH`）⇒ 它不是"完成/未完成"那种二值口径。
    /// 2026-09-25 真账号读数看起来像"组内完成度"（`0`/`1`/`2` 三档），
    /// 但那是**相关性观察、不是因果** ⇒ 判据面照旧不读它。
    pub group_finish_states: Vec<(String, Option<i64>)>,
}

/// 解析一次 `taskListV2` 响应。
///
/// `group` 只用于回填 [`TaskListV2::group`]（见上）。
///
/// ⚠ 与 V3 相反：这一口的 `result` 必须是**对象**。实拍里有两种合法形态 ——
/// `result: {}`（这一组今天没有任务）与 `result: {"<group>": [...]}`。
/// "`result` 键整个缺席"同样按空组处理：把"上游没给这一组"判成解析失败的代价是
/// **整条聚合链**被一次空响应打断。
pub fn parse_task_list_v2(body: &str, group: &str) -> Result<TaskListV2> {
    let env = SignInEnvelope::parse(body)?;
    env.ensure_ok("读取 App 任务矩阵")?;

    let mut out = TaskListV2 {
        group: group.to_string(),
        ..Default::default()
    };

    // `result` 缺席 ⇒ 空组；存在 ⇒ 必须是对象（数组在这里就是"这不是这一口"）。
    let groups = match env.result.as_ref() {
        None => None,
        Some(v @ Value::Object(_)) => Some(v),
        Some(other) => {
            return Err(mcloud_core::Error::Parse(format!(
                "taskListV2 的 result 不是按 group 分组的对象（实得 {other}；这一口与 V3 不同形，别混解）"
            )));
        }
    };
    if let Some(groups) = groups {
        let obj = groups.as_object().expect("上面已判过是对象");
        // 键名按**字典序**摊平：`serde_json` 的对象默认是 BTreeMap 以外的顺序，
        // 这里显式排序让结果可复现（键名本身不参与筛选，只影响顺序）。
        let mut names: Vec<&String> = obj.keys().collect();
        names.sort();
        for name in names {
            let raw = &obj[name];
            let Value::Array(items) = raw else {
                return Err(mcloud_core::Error::Parse(format!(
                    "taskListV2 的 result.{name} 不是数组，实得 {raw}"
                )));
            };
            for raw_item in items {
                // ⚠ 这一口与拍拍那族**不同**：单条不是对象时**不报错**，
                // 而是落成一条全 `None` 的条目（照参考实现）。它会进"状态读不出"那一档，
                // 而这一条链今天**只读**，所以不至于改变任何行为。
                let item = raw_item.as_object();
                let get = |key: &str| item.and_then(|m| m.get(key));
                // `buttonProperty.out` 优先、回落 `button.out`（实拍两种形态都出现过 ✓：
                // 1029 有 `buttonProperty` ✓、1021 只有 `button` ✓）。每条现取 ✓，别提到循环外 ✗。
                // ⚠ **`email` 优先于 `out`**（2026-10-11 用户口径：以 **App 里看到的**为准 ✓）：
                //   App 渲染的是 `email` 那一份（实拍 `"email":{"itemBg":"#fff6e5","btnBg":"#ffa939"…}`
                //   —— `#ffa939` 就是那个**橙色按钮** ✓，用户截图里正是它 ✓）。
                //   实拍 14 条任务里两处 `text`/`day` **逐条相同** ✓，所以这是"按谁为准"的排序，
                //   不是"补一个不同的值" ✓；真出现分歧时以 App 那一份为准 ✓。
                //   顺序：`email` **跨形态**先取 ✓（`buttonProperty.email` → `button.email`），
                //   再退到 `out`（`buttonProperty.out` → `button.out`）✓。
                //   顺序：`email`（139邮箱 H5 ✓）→ `app`（**原生 App** ✓，用户就是拿 App 比 ✓）
                //   → `out`（外部端 ✓）；同一形态内 `buttonProperty` 先于 `button` ✓。
                //   实拍三种形态都出现过：1021/1079 有 `email` ✓、550/551 只有 `app` + `out` ✓。
                let out_obj = get("buttonProperty")
                    .and_then(|b| b.get("email"))
                    .or_else(|| get("button").and_then(|b| b.get("email")))
                    .or_else(|| get("buttonProperty").and_then(|b| b.get("app")))
                    .or_else(|| get("button").and_then(|b| b.get("app")))
                    .or_else(|| get("buttonProperty").and_then(|b| b.get("out")))
                    .or_else(|| get("button").and_then(|b| b.get("out")));
                out.items.push(TaskV2Item {
                    task_id: int_at(get("id")),
                    name: str_at(get("name")),
                    description: str_at(get("description")),
                    // ⚠ 全小写 `groupid`（不是 `groupId`）
                    group_id: str_at(get("groupid")),
                    state: str_at(get("state")),
                    limit_type: str_at(get("limitType")),
                    enable: int_at(get("enable")),
                    curr_step: int_at(get("currstep")),
                    process: int_at(get("process")),
                    market_name: str_at(get("marketname")),
                    reward_text: str_at(get("content_display2")),

                    // `button.app.link`（玩法 615 那一族唯一的门 ✓）。缺键 / 少一层 /
                    // 不是字符串 ⇒ `None`（"读不出" ⇒ 那一族就**不动**，不会误点 ✓）。
                    // `button.out` 那一族（`buttonProperty.out` 优先 ✓，两处形态都实拍过 ✓）。
                    button_out_day: out_obj
                        .and_then(|o| o.get("day"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    button_can_receive: out_obj
                        .and_then(|o| o.get("canReceive"))
                        .and_then(Value::as_i64),
                    // 与上面的 `out_obj` **同一份来源** ✓（`email` 优先 —— App 渲染的就是它 ✓）。
                    // ⚠ V2 **不给** `total` ✗（那是 V3 那一格 ✓）⇒ 这里恒 `None`，
                    // 由 `app_task_matrix` 用 V3 的 overlay 并进来 ✓。
                    button_out_total: None,
                    button_out_text: out_obj
                        .and_then(|o| o.get("text"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    button_text_middle: out_obj
                        .and_then(|o| o.get("textMiddle"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    button_text_finish: out_obj
                        .and_then(|o| o.get("textFinish"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    button_action_middle: out_obj
                        .and_then(|o| o.get("actionMiddle"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    button_app_link: get("button")
                        .and_then(|b| b.get("app"))
                        .and_then(|a| a.get("link"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    // `button.email.link`（1017 的"云朵中心"入口页 ✓）。
                    button_email_link: get("button")
                        .and_then(|b| b.get("email"))
                        .and_then(|e| e.get("link"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
    }

    // 顶层 `groupState.<group>.taskFinishState`。
    // 从**原始 JSON** 里取 —— `SignInEnvelope` 只留了 `result`。
    let root: Value = serde_json::from_str(body)
        .map_err(|e| mcloud_core::Error::Parse(format!("taskListV2 响应不是 JSON：{e}")))?;
    if let Some(Value::Object(state)) = root.get("groupState") {
        let mut names: Vec<&String> = state.keys().collect();
        names.sort();
        for name in names {
            let value = state[name].get("taskFinishState").and_then(Value::as_i64);
            out.group_finish_states.push((name.clone(), value));
        }
    }
    Ok(out)
}

/// 取一个**可选**整数：缺键、`null`、非数字都回 `None`。
fn int_at(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// 取一个**可选**字符串。
fn str_at(v: Option<&Value>) -> Option<String> {
    v?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests;
