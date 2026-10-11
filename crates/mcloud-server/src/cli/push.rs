//! `mcloud push` —— **推送规则**的命令行入口（2026-10-08 推翻重做后的形状）。
//!
//! 用户口径（这一版的设计就是这两句，别再往下加层 ✓）：
//!
//! > 「我设置一个推送规则，**这个规则配置推送的 token，包括推送哪几个平台**」
//! > 「**不是规则去命中行为，而是行为选择规则，一个行为可以选择多个规则**」
//!
//! # 三层收成两层
//!
//! | 层 | 谁配 | CLI 管不管 |
//! |---|---|---|
//! | ① **规则**（`PushRule` ✓，自带一份 `PushConfig` = 平台 + 凭据 ✓） | 每个系统用户一份（`data/push-rules.json` ✓） | **本模块** ✓ |
//! | ② 行为（定时 / 编排 / 面板 ✓） | 定时/编排存 `push_rules` ✓，面板存 `data/push-panel.json` ✓ | `push bind` / `push panel` ✓ |
//!
//! ⚠ **用户级"渠道配置"那一层整个没了** ✗ ⇒ `mcloud push config …` 子命令
//! 一起删掉 ✓（原来那三个动作 show / set / off 都没有对象了 ✓）。
//!
//! # ⚠ CLI 是**本机管理员工具**
//!
//! 规则按 owner 分表 ✓，而命令行上"我是谁"只能取**本机管理员**
//! （`owner::legacy_owner` = 创建最早的那个用户 ✓，与 `mcloud run`（不带参数）
//! 同一条口径 ✓）。
//! 因此 `mcloud push` 动的永远是**管理员那一份**规则 ✓ —— 普通用户的规则
//! 走 WebUI（那里有登录态 ✓）。
//!
//! # 用法
//!
//! ```text
//! mcloud push                                  # = push list
//! mcloud push list
//! mcloud push show <规则id>                     # 含它被哪些定时/编排/行为选中
//! mcloud push add <名字> --json <规则 JSON | ->  # 凭据就在这份 JSON 里
//! mcloud push rm <规则id>
//! mcloud push bind sch:<定时id>|pipe:<编排id> <规则id,…>
//! mcloud push bind sch:<定时id> -               # 清空
//! mcloud push panel <loginNew|loginFailed|perfSlow|perfFailStreak> [选项] <规则id,…>
//! mcloud push events                            # 事件名（run/panel 分组）与渠道键
//! ```

use mcloud_app::App;
use mcloud_core::push::{
    BehaviorKey, PushChannelKey, PushEvent, PushRule, PushRuleId, ThrottleMode,
};
use mcloud_core::{PipelineId, ScheduleId, UserId};

const USAGE: &str = "\
用法：
  mcloud push [list]                                列规则
  mcloud push show <规则id>                         看一条（含它被哪些定时/编排/行为选中）
  mcloud push add <名字> --json <规则 JSON | ->     建一条（`-` = 读 stdin；凭据就在这份 JSON 里）
  mcloud push rm <规则id>                           删一条
  mcloud push bind sch:<定时id>|pipe:<编排id> <规则id,…>   绑（可多选，逗号分隔）；`-` = 清空
  mcloud push panel <loginNew|loginFailed|perfSlow|perfFailStreak> [选项] <规则id,…>
  mcloud push events                                列事件名（run/panel 分组）与渠道键

⚠ CLI 上**没有** `--user` / `--owner` ✓ —— 上面每一条动的都是**本机管理员**
（= 创建最早的那个用户 ✓）那一份规则；想管别的用户请在 **WebUI** 上改 ✓。

add 的 JSON 形状（camelCase，字段与 PushRule 一致；`id` / `owner` 由本命令定 ✓，
名字也可省 —— 位置参数那个说话 ✓）：
  {\"config\":{\"serverChan\":{\"token\":\"SCT…\"}},\"digest\":\"perRun\"}

panel 的选项（只给**相关**的 ✓，给错行为传直接报错 ✗）：
  --rules a,b        这个行为选中的规则（也可写成位置参数 ✓）
  -                  清空规则
  --mode every|onChange      节流方式（every = 每次触发都推 ✓）
  --cooldown <秒|30m|2h>     同一行为两次推送的最小间隔
  --daily-cap <n>            每日上限（条；**0 = 不限** ✓；超了记日志、不静默 ✓）
  --slow-factor <倍数>       **只对 perfSlow 有意义** ✓（必须 > 0 ✓）
  --fail-streak <条数>       **只对 perfFailStreak 有意义** ✓";

/// 取"本机管理员"（最早那个用户 ✓）——与 `mcloud run`（不带参数）同一条口径 ✓：
/// CLI 是**本机管理员工具** ✓，而规则是**每人一份**的 ✓。
///
/// ⚠ `pub(crate)`：`cli/live_room.rs` 那条也用它 ✓ —— 「每登录用户一份」的东西
/// 在命令行上只能落到管理员那一份 ✓，两处各写一遍的话，"管理员是谁"迟早分叉 ✗。
pub(crate) async fn cli_owner(app: &App) -> anyhow::Result<UserId> {
    let users = app.handle.store.users().await?;
    mcloud_core::owner::legacy_owner(&users).ok_or_else(|| {
        anyhow::anyhow!("这个数据目录里还没有用户（先 mcloud user add <名字> <口令>）")
    })
}

/// ⚠ CLI **不提供**"选谁"的参数（`--user` / `--owner` / `-u`）✗ —— 见到就**报错** ✓。
///
/// 用户口径（2026-10-11）：「**让 cli 仅操作管理员账号**」✓ ⇒ 命令行上"我是谁"
/// **只有一个答案**（= 本机管理员 ✓，见 [`cli_owner`] ✓），
/// 没有任何"动别人那一份"的入口 ✓ —— 想管别的用户 ⇒ **WebUI**（那里有登录态 ✓）。
///
/// ⚠⚠ 这里必须扫**整条 `argv`**、不能只看第一个位置：只看首位的话
/// `mcloud auth auto-schedule on --user bob` 会把那一格**静默忽略** ✗ ——
/// 那比报错更糟 ✓（用户以为动的是 bob，实际动的是管理员，而输出里没有任何提示 ✗）。
///
/// ⚠ 判据只此一处 ✓：`cli/push.rs` / `cli/auth.rs` / `cli/live_room.rs` 都调它 ——
/// 三处各写一遍的话，迟早只认住一半 ✗（而那半格正是"偏离设计"的形态 ✗）。
pub(crate) fn reject_user_selection(args: &[String]) -> anyhow::Result<()> {
    let Some(flag) = args
        .iter()
        .find(|a| matches!(a.as_str(), "--user" | "--owner" | "-u"))
    else {
        return Ok(());
    };
    anyhow::bail!(
        "命令行不加 `{flag}` ✗ —— CLI **只作用于本机那一个用户**\
         （= 管理员 ✓，创建最早的那个 ✓，`mcloud user list` 里标着「管理员」的那个 ✓）。\n\
         想动**别的用户**那一份：让**他**登录 **WebUI** 改 ✓\
         （多用户是给不同设备的用户用的，那是 WebUI 的视角 ✓）。"
    )
}

pub async fn run(app: &App, args: &[String]) -> anyhow::Result<()> {
    // ⚠ CLI 不提供"选谁" ✓ —— 先扫一遍，免得 `mcloud push list --user bob`
    // 那类写法被**静默忽略** ✗（见 `reject_user_selection` ✓）。
    reject_user_selection(args)?;
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let rest = if args.is_empty() { args } else { &args[1..] };
    let outcome = match sub {
        "list" | "ls" => list(app).await,
        "show" => show(app, rest).await,
        "add" => add(app, rest).await,
        "rm" | "remove" | "del" => rm(app, rest).await,
        "events" => {
            events();
            Ok(())
        }
        "bind" => bind(app, rest).await,
        "panel" => panel(app, rest).await,
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => anyhow::bail!("未知的 push 子命令：{other}\n\n{USAGE}"),
    };
    // ⚠ **收尾 flush**（与 `cli/run.rs` / `cli/pipeline.rs` 同一条口径 ✓）：
    // 投递是 spawn 出去的 ✓，CLI 跑完就退 ⇒ 在飞的推送会被一起掐掉 ✗。
    // 本命令族今天只写配置（不触发投递 ✓），但这条**留在收尾**：一旦以后
    // 这里加"试发"，漏掉它就是"点了没反应、也不报错" ✗。
    app.handle.pushes.flush().await;
    outcome
}

async fn list(app: &App) -> anyhow::Result<()> {
    let owner = cli_owner(app).await?;
    let rules = app.handle.store.push_rules(&owner).await?;
    if rules.is_empty() {
        println!("还没有推送规则。");
        println!("\n{USAGE}");
        return Ok(());
    }
    // 一次把定时/编排读出来，好在列表里直接标"这条规则绑在哪" ✓
    // （分开读会 N+1 次读盘 ✓）。
    let schedules = app.handle.store.schedules().await?;
    let pipelines = app.handle.store.pipelines().await?;

    println!(
        "{} {:<18} {:<6} 渠道",
        pad("id", 22),
        pad("名字", 16),
        "状态"
    );
    for r in &rules {
        println!(
            "{} {} {:<6} {}",
            pad(r.id.as_str(), 22),
            pad(&truncate(&r.name, 16), 18),
            if r.enabled { "启用" } else { "停用" },
            channels_of(r).join("、")
        );
        let bound = bound_to(r, &schedules, &pipelines);
        if !bound.is_empty() {
            println!("{}└─ 绑在：{}", " ".repeat(22), bound.join("、"));
        }
    }
    Ok(())
}

async fn show(app: &App, args: &[String]) -> anyhow::Result<()> {
    let id = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("要给规则 id：mcloud push show <规则id>"))?;
    let owner = cli_owner(app).await?;
    let Some(r) = app
        .handle
        .store
        .push_rule(&owner, &PushRuleId::new(id))
        .await?
    else {
        anyhow::bail!("没有这条规则：{id}（别人的 id 与不存在一样 ✓）");
    };
    println!("id        {}", r.id);
    println!("名字      {}", r.name);
    println!("状态      {}", if r.enabled { "启用" } else { "停用" });
    println!("渠道      {}", channels_of(&r).join("、"));
    println!("汇总      {:?}", r.digest);
    // ⚠ 这里原先打一行「静默 from–to」—— 随静默时段整块删掉 ✗（2026-10-08 用户口径
    // 「不要静默时间」✓）。CLI 上原先那个 `--quiet` 选项在同一版里已经先没了 ✓。
    // ⚠ 原样打（**含 token / secret** ✓）—— 本机管理员显式要看的那一份 ✓
    // （凭据就在规则里 ✓，脱敏了这条命令就没法回答"我到底配了什么" ✗）。
    println!("配置      {}", serde_json::to_string(&r.config)?);

    let schedules = app.handle.store.schedules().await?;
    let pipelines = app.handle.store.pipelines().await?;
    let mut bound = bound_to(&r, &schedules, &pipelines);
    // ⚠ 面板行为那一份**要真查**（用户口径：「面板行为…单独一个可配置推送规则的入口」✓）：
    // 漏掉它的话，一条只被 `perfSlow` 选中的规则会显示成"没被任何行为选中" ✗。
    let panel = app.handle.store.push_panel(&owner).await?;
    for key in BehaviorKey::ALL {
        if panel.get(key).rules.contains(&r.id) {
            bound.push(format!("面板 {}", key.label()));
        }
    }
    println!(
        "被谁选中  {}",
        if bound.is_empty() {
            "（没被任何行为选中 ✓ —— 这条规则现在不会生效）".to_string()
        } else {
            bound.join("、")
        }
    );
    Ok(())
}

/// 一条规则被哪些**定时 / 编排**选中（`push_rules` 里含它 ✓）。
fn bound_to(
    rule: &PushRule,
    schedules: &[mcloud_core::Schedule],
    pipelines: &[mcloud_core::Pipeline],
) -> Vec<String> {
    let mut out = Vec::new();
    for s in schedules.iter().filter(|s| s.push_rules.contains(&rule.id)) {
        out.push(format!("sch:{}（{}）", s.id, s.name));
    }
    for p in pipelines.iter().filter(|p| p.push_rules.contains(&rule.id)) {
        out.push(format!("pipe:{}（{}）", p.id, p.name));
    }
    out
}

async fn add(app: &App, args: &[String]) -> anyhow::Result<()> {
    let Some(name) = args.first().filter(|a| !a.starts_with("--")) else {
        anyhow::bail!("要给名字：mcloud push add <名字> --json <规则 JSON | ->\n\n{USAGE}");
    };
    let mut raw: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                raw = args.get(i + 1).cloned();
                i += 2;
            }
            other => anyhow::bail!("不认识的选项：{other}\n\n{USAGE}"),
        }
    }
    let raw = raw.ok_or_else(|| anyhow::anyhow!("要给规则 JSON：--json <json|->\n\n{USAGE}"))?;
    let text = read_json_arg(&raw)?;

    let owner = cli_owner(app).await?;
    // ⚠ 解析成**完整的 `PushRule`**（不是另造一个入参结构 ✓）：`config` 就是
    // 凭据本身 ✓（用户口径：「这个规则配置推送的 token，包括推送哪几个平台」✓），
    // 另造一层只会多一份要与 `PushRule` 对齐的字段表 ✗。
    //
    // ⚠ 但**先过一遍 `Value`**：`PushRule` 的 `id` / `name` 是必填 ✗，而命令行上
    // 用户给的 JSON 不该带它们 ✓（id 由本命令生成 ✓、name 是位置参数 ✓）。
    // 直接 `from_str::<PushRule>` 会让每一份正常的 JSON 都撞 "missing field `id`" ✗
    // —— 那是**本命令的**形状要求，不该甩给用户 ✓。
    let mut v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("JSON 解析失败：{e}\n（字段名是 camelCase ✓）"))?;
    let Some(obj) = v.as_object_mut() else {
        anyhow::bail!("规则 JSON 要是一个对象（收到的是别的 JSON 类型）");
    };
    // `id` / `owner` 由本命令定 ✓ —— JSON 里带了也**以命令行为准** ✗（否则
    // 一条 `--json` 就能把规则塞给别人 ✓，那是 `owner` 那一格存在的意义 ✓）。
    let rule_id = next_rule_id(app);
    obj.insert("id".into(), serde_json::Value::String(rule_id));
    obj.remove("owner");
    if !name.is_empty() {
        // 位置参数是**显式**给的 ⇒ 它说话 ✓（JSON 里的 `name` 当缺省 ✓）。
        obj.insert("name".into(), serde_json::Value::String(name.clone()));
    }
    let mut rule: PushRule = serde_json::from_value(v)
        .map_err(|e| anyhow::anyhow!("规则 JSON 不对：{e}\n（字段名是 camelCase ✓）"))?;
    rule.owner = Some(owner.clone());
    if rule.name.trim().is_empty() {
        anyhow::bail!("名字不能为空");
    }
    // ⚠ 校验在**存之前**：`validate()` 的错误信息原样透出 ✓
    // （里面有「pushConfig.pushplus.token 不能为空」这类**路径** ✓，不带值 ✓）。
    rule.validate()?;
    app.handle.store.save_push_rule(&owner, &rule).await?;
    println!("建好了：{}（{}）", rule.id, rule.name);
    println!("渠道      {}", channels_of(&rule).join("、"));
    println!(
        "\n⚠ 它现在**还没生效** ✗ —— 得被某个行为选中才算数：\n    \
         mcloud push bind sch:<定时id> {0}\n    \
         mcloud push panel perfSlow {0}",
        rule.id
    );
    Ok(())
}

async fn rm(app: &App, args: &[String]) -> anyhow::Result<()> {
    let id = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("要给规则 id：mcloud push rm <规则id>"))?;
    let owner = cli_owner(app).await?;
    let rule_id = PushRuleId::new(id);
    let Some(rule) = app.handle.store.push_rule(&owner, &rule_id).await? else {
        anyhow::bail!("没有这条规则：{id}");
    };
    // ⚠ 先看有没有行为选着它：删掉之后那些行为**就真的不推了** ✓（新模型里
    // "空 = 不推" ✓）—— 那是合法行为（不是坏掉 ✓），但用户多半以为"我删的是
    // 一条没用的规则" ✗ ⇒ 得说一声 ✓。
    let schedules = app.handle.store.schedules().await?;
    let pipelines = app.handle.store.pipelines().await?;
    let mut bound = bound_to(&rule, &schedules, &pipelines);
    let panel = app.handle.store.push_panel(&owner).await?;
    for key in BehaviorKey::ALL {
        if panel.get(key).rules.contains(&rule_id) {
            bound.push(format!("面板 {}", key.label()));
        }
    }
    app.handle.store.remove_push_rule(&owner, &rule_id).await?;
    println!("删了：{id}");
    if !bound.is_empty() {
        println!("⚠ 它原来被这些行为选中：{}", bound.join("、"));
        println!("  这些现在**不再推**了（新模型里空 = 不推 ✓）—— 想继续收，得重新选一条规则。");
    }
    Ok(())
}

fn events() {
    println!("运行类事件（定时/编排的 `pushEvents` 里填左边这些 ✓）：");
    for e in PushEvent::ALL {
        if event_kind(e) != "run" {
            continue;
        }
        let mark = if PushEvent::default_selected().contains(&e) {
            "（默认选中）"
        } else {
            ""
        };
        println!("  {:<16} {}{}", event_key(e), e.label(), mark);
    }
    println!("\n面板类事件（**不进** `pushEvents` ✗；用 `mcloud push panel <行为> …` ✓）：");
    for e in PushEvent::ALL {
        if event_kind(e) != "panel" {
            continue;
        }
        println!("  {:<16} {}", event_key(e), e.label());
    }
    println!("\n面板行为名（`mcloud push panel` 的第一个参数 ✓）：");
    for k in BehaviorKey::ALL {
        println!("  {:<16} {}", behavior_key(k), k.label());
    }
    println!("\n渠道键（写进规则的 `config` 里，例如 {{\"serverChan\":{{\"token\":\"…\"}}}} ✓）：");
    for c in PushChannelKey::ALL {
        println!("  {:<16} {}", channel_key(c), c.label());
    }
    println!("\n⚠ 「运行成功」默认**不选**：每天准点推一条「一切正常」，人会把通知关掉 ✗。");
}

async fn bind(app: &App, args: &[String]) -> anyhow::Result<()> {
    let (target, rule_arg) = match (args.first(), args.get(1)) {
        (Some(t), Some(r)) => (t, r),
        _ => {
            anyhow::bail!("用法：mcloud push bind <sch:定时id|pipe:编排id> <规则id,…|->\n\n{USAGE}")
        }
    };
    let owner = cli_owner(app).await?;

    // `-` = 清空（新模型里空 = 不推 ✓）。写成 `--none` / `none` 也认 ✓。
    let cleared = rule_arg == "-" || rule_arg == "--none" || rule_arg == "none";
    let rules = if cleared {
        Vec::new()
    } else {
        let ids = parse_rule_ids(rule_arg)?;
        if ids.is_empty() {
            anyhow::bail!("至少要给一条规则 id（清空请写 `-`）");
        }
        for id in &ids {
            if app.handle.store.push_rule(&owner, id).await?.is_none() {
                anyhow::bail!("没有这条规则：{id}（先 mcloud push list 看看 ✓）");
            }
        }
        ids
    };
    let shown = if rules.is_empty() {
        "不推".to_string()
    } else {
        rules
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("、")
    };

    if let Some(sid) = target.strip_prefix("sch:") {
        let id = ScheduleId::new(sid);
        let Some(mut sch) = app.handle.store.schedule(&id).await? else {
            anyhow::bail!("没有这条定时：{sid}");
        };
        sch.push_rules = rules;
        app.handle.store.save_schedule(&sch).await?;
        println!("定时 {sid}（{}）现在选中：{shown}", sch.name);
        return Ok(());
    }
    if let Some(pid) = target.strip_prefix("pipe:") {
        let id = PipelineId::new(pid);
        let Some(mut p) = app.handle.store.pipeline(&id).await? else {
            anyhow::bail!("没有这条编排：{pid}");
        };
        p.push_rules = rules;
        app.handle.store.save_pipeline(&p).await?;
        println!("编排 {pid}（{}）现在选中：{shown}", p.name);
        return Ok(());
    }
    anyhow::bail!("目标要写成 sch:<定时id> 或 pipe:<编排id>（收到的是 {target}）");
}

/// `mcloud push panel <行为> [选项] [规则id,…]`。
///
/// ⚠ 只给**相关**选项（用户口径 ✓）：`--slow-factor` 只对 `perfSlow` 有意义 ✓、
/// `--fail-streak` 只对 `perfFailStreak` 有意义 ✓ —— 给别的行为传就**报错** ✗，
/// 绝不静默忽略 ✗（静默忽略的表现是"我明明设了 5 倍、它按 3 倍跑" ✗，不报错 ✓）。
async fn panel(app: &App, args: &[String]) -> anyhow::Result<()> {
    let Some(raw_key) = args.first() else {
        anyhow::bail!(
            "要给行为名：mcloud push panel <loginNew|loginFailed|perfSlow|perfFailStreak> [选项] [规则id,…]\n\n{USAGE}"
        );
    };
    let key = parse_behavior_key(raw_key)?;
    let owner = cli_owner(app).await?;

    let mut rules: Option<Vec<PushRuleId>> = None;
    let mut mode: Option<ThrottleMode> = None;
    let mut cooldown: Option<u64> = None;
    let mut daily_cap: Option<u32> = None;
    let mut slow_factor: Option<f64> = None;
    let mut fail_streak: Option<u32> = None;

    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--rules" => {
                let v = take_value(args, arg, i)?;
                rules = Some(if v == "-" {
                    Vec::new()
                } else {
                    parse_rule_ids(v)?
                });
                i += 2;
            }
            "--mode" => {
                mode = Some(match take_value(args, arg, i)?.as_str() {
                    "every" => ThrottleMode::Every,
                    "onChange" | "on-change" | "onchange" => ThrottleMode::OnChange,
                    other => anyhow::bail!("--mode 只认 every / onChange：{other}"),
                });
                i += 2;
            }
            "--cooldown" => {
                cooldown = Some(parse_duration(take_value(args, arg, i)?)?);
                i += 2;
            }
            "--daily-cap" => {
                daily_cap = Some(
                    take_value(args, arg, i)?
                        .parse()
                        .map_err(|_| anyhow::anyhow!("--daily-cap 要是一个整数（例如 10）"))?,
                );
                i += 2;
            }
            "--slow-factor" => {
                // ⚠ **相关才给**：耗时异常的判据只对 perfSlow 有意义 ✓。
                if key != BehaviorKey::PerfSlow {
                    anyhow::bail!(
                        "--slow-factor 只对 perfSlow 有意义（当前行为：{}）",
                        key.label()
                    );
                }
                slow_factor =
                    Some(take_value(args, arg, i)?.parse().map_err(|_| {
                        anyhow::anyhow!("--slow-factor 要是一个数（例如 3 或 2.5）")
                    })?);
                i += 2;
            }
            "--fail-streak" => {
                if key != BehaviorKey::PerfFailStreak {
                    anyhow::bail!(
                        "--fail-streak 只对 perfFailStreak 有意义（当前行为：{}）",
                        key.label()
                    );
                }
                fail_streak = Some(
                    take_value(args, arg, i)?
                        .parse()
                        .map_err(|_| anyhow::anyhow!("--fail-streak 要是一个整数（例如 3）"))?,
                );
                i += 2;
            }
            "-" => {
                rules = Some(Vec::new());
                i += 1;
            }
            other if other.starts_with("--") => {
                anyhow::bail!("不认识的选项：{other}\n\n{USAGE}")
            }
            other => {
                // 位置参数：规则 id 清单（逗号分隔 ✓）—— 与 `--rules` 同义 ✓。
                rules = Some(parse_rule_ids(other)?);
                i += 1;
            }
        }
    }

    let mut bindings = app.handle.store.push_panel(&owner).await?;
    let changed = rules.is_some()
        || mode.is_some()
        || cooldown.is_some()
        || daily_cap.is_some()
        || slow_factor.is_some()
        || fail_streak.is_some();
    if !changed {
        // 什么都不给 = **看一眼**（不是"清空" ✗ —— 一条不带参数的写命令
        // 把绑定抹掉，是那种"手滑一下、数据没了、还不报错"的形态 ✗）。
        print_binding(key, bindings.get(key));
        return Ok(());
    }

    if let Some(ids) = &rules {
        for id in ids {
            if app.handle.store.push_rule(&owner, id).await?.is_none() {
                anyhow::bail!("没有这条规则：{id}（先 mcloud push list 看看 ✓）");
            }
        }
    }

    let slot = bindings.get_mut(key);
    if let Some(ids) = rules {
        slot.rules = ids;
    }
    if let Some(m) = mode {
        slot.throttle.mode = m;
    }
    if let Some(c) = cooldown {
        slot.throttle.cooldown_secs = c;
    }
    if let Some(n) = daily_cap {
        slot.throttle.daily_cap = n;
    }
    if let Some(v) = slow_factor {
        slot.slow_factor = Some(v);
    }
    if let Some(v) = fail_streak {
        slot.fail_streak = Some(v);
    }
    // ⚠ 阈值校验在**存之前**（`PanelBindings::validate` ✓，与 `PUT /api/push/panel`
    // **共用同一份** ✓）：`--slow-factor 0` 会让「耗时异常」的判据**永远成立**
    //（`耗时 > 中位数 × 0` 恒真 ✓）⇒ 用户设一次 0，从此每次运行都推一条"耗时异常" ✗。
    // ⚠ 命令行上真的能写出来：`"nan"` / `"inf"` 都能 `parse::<f64>()` 成功 ✓
    // ⇒ 这条不是"防手滑"，是**真会发生**的输入 ✓。
    bindings.validate()?;
    app.handle.store.save_push_panel(&owner, &bindings).await?;
    println!("✓ 面板行为「{}」已保存：", key.label());
    print_binding(key, bindings.get(key));
    Ok(())
}

fn print_binding(key: BehaviorKey, b: &mcloud_core::push::BehaviorBinding) {
    println!("  行为      {}（{}）", behavior_key(key), key.label());
    println!(
        "  选中的规则  {}",
        if b.rules.is_empty() {
            "（空 —— 不推 ✓）".to_string()
        } else {
            b.rules
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
                .join("、")
        }
    );
    println!(
        "  节流      {} / 冷却 {} / 每日上限 {}",
        throttle_mode_label(b.throttle.mode),
        b.throttle.cooldown_secs,
        b.throttle.daily_cap
    );
    if key == BehaviorKey::PerfSlow {
        println!(
            "  耗时判据  {}",
            match b.slow_factor {
                Some(v) => format!(">{v}× 历史中位数"),
                None => "（用内置默认）".to_string(),
            }
        );
    }
    if key == BehaviorKey::PerfFailStreak {
        println!(
            "  连败判据  {}",
            match b.fail_streak {
                Some(v) => format!("≥{v} 次"),
                None => "（用内置默认）".to_string(),
            }
        );
    }
}

// ── 小工具 ────────────────────────────────────────────────────────────────

/// 规则 id：`push-<毫秒>` ✓（与本仓别处的 id 风格一致：**时间序 + 短** ✓，
/// 人一眼能看出先后 ✓）。
fn next_rule_id(app: &App) -> String {
    format!("push-{}", app.handle.clock.now_ms())
}

/// 取值型选项的下一个参数（缺了就直接报错，**别猜** ✓）。
fn take_value<'a>(args: &'a [String], flag: &str, i: usize) -> anyhow::Result<&'a String> {
    args.get(i + 1)
        .ok_or_else(|| anyhow::anyhow!("{flag} 后面要给一个值"))
}

/// `--json <json|->`：`-` = 从 stdin 读 ✓。
fn read_json_arg(raw: &str) -> anyhow::Result<String> {
    if raw != "-" {
        return Ok(raw.to_string());
    }
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

/// 逗号分隔的规则 id 清单（**可多选** ✓；空串 ⇒ 空清单 ✓）。
fn parse_rule_ids(v: &str) -> anyhow::Result<Vec<PushRuleId>> {
    Ok(v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PushRuleId::new)
        .collect())
}

/// `<秒|30m|2h>` → 秒 ✓（不带后缀就是秒 ✓）。
fn parse_duration(v: &str) -> anyhow::Result<u64> {
    let s = v.trim();
    let bad = || anyhow::anyhow!("时长要写成 1800 / 30m / 2h 这样的形式：{v}");
    if let Some(n) = s.strip_suffix('m') {
        return Ok(n.trim().parse::<u64>().map_err(|_| bad())? * 60);
    }
    if let Some(n) = s.strip_suffix('h') {
        return Ok(n.trim().parse::<u64>().map_err(|_| bad())? * 3600);
    }
    if let Some(n) = s.strip_suffix('s') {
        return n.trim().parse::<u64>().map_err(|_| bad());
    }
    s.parse::<u64>().map_err(|_| bad())
}

/// 行为名的**线格式**（serde 的 camelCase ✓）—— 命令行里填的就是它 ✓。
///
/// ⚠ 不手写字符串表 ✗：`serde_json::to_value` 拿到的就是落盘/接口用的那一份 ✓，
/// 改了枚举名两边自动一致 ✓（手写一张表迟早与 serde 分叉 ✗）。
fn behavior_key(k: BehaviorKey) -> String {
    match serde_json::to_value(k) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{k:?}"),
    }
}

fn event_key(e: PushEvent) -> String {
    match serde_json::to_value(e) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{e:?}"),
    }
}

fn channel_key(c: PushChannelKey) -> String {
    match serde_json::to_value(c) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{c:?}"),
    }
}

/// 事件属于哪一组（与路由那份**同一条判据** ✓：`match` 穷尽，不靠字符串前缀 ✓）。
fn event_kind(e: PushEvent) -> &'static str {
    match e {
        PushEvent::RunFailed
        | PushEvent::RunPartial
        | PushEvent::RunOk
        | PushEvent::RunCancelled => "run",
        PushEvent::LoginNew
        | PushEvent::LoginFailed
        | PushEvent::PerfSlow
        | PushEvent::PerfFailStreak => "panel",
    }
}

fn throttle_mode_label(m: ThrottleMode) -> &'static str {
    match m {
        ThrottleMode::Every => "every",
        ThrottleMode::OnChange => "onChange",
    }
}

/// 解析行为名：大小写不敏感 ✓，也认 `perf_fail_streak` 这种下划线写法 ✓。
fn parse_behavior_key(v: &str) -> anyhow::Result<BehaviorKey> {
    let want = v.trim().to_ascii_lowercase().replace(['_', '-'], "");
    BehaviorKey::ALL
        .into_iter()
        .find(|k| {
            behavior_key(*k).to_ascii_lowercase() == want
                || format!("{k:?}").to_ascii_lowercase() == want
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "不认识的面板行为：{v}（可用：loginNew / loginFailed / perfSlow / perfFailStreak ✓）"
            )
        })
}

/// 这份规则配置里**开了哪几个渠道**（键名照 wire ✓）。
///
/// 用 serde 转成对象再挑非 null 的键 ✓ —— 比手写十个 `if let Some` 短，而且
/// **加了新渠道不用改这里** ✓（`PushConfig` 的字段名就是 wire 名 ✓）。
fn channels_of(r: &PushRule) -> Vec<String> {
    let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&r.config) else {
        return Vec::new();
    };
    let keys: Vec<String> = map
        .into_iter()
        .filter(|(k, v)| !v.is_null() && !matches!(k.as_str(), "title" | "onlyError" | "minLevel"))
        .map(|(k, _)| k)
        .collect();
    if keys.is_empty() {
        vec!["（一个渠道都没有 ✗）".to_string()]
    } else {
        keys
    }
}

/// 按**字符**截断（`&s[..n]` 会在中文上 panic ✗）。
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect::<String>() + "…"
}

/// 按**显示宽度**补齐（`{:<22}` 只数 `char` ⇒ 中文名会挤成一团 ✗）。
///
/// 判据用 `char` 的码点：CJK / 全角那几段算 2 列 ✓（本命令的输出只有
/// 中文与 ASCII 两种，不值得为它拉一个宽度表 ✓）。
fn pad(s: &str, width: usize) -> String {
    let w: usize = s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum();
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F      // 谚文字母
        | 0x2E80..=0xA4CF    // CJK 部首 … 注音
        | 0xAC00..=0xD7A3    // 谚文音节
        | 0xF900..=0xFAFF    // CJK 兼容
        | 0xFE30..=0xFE6F    // CJK 兼容形式
        | 0xFF00..=0xFF60    // 全角
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF  // emoji（⚠ 宽度算 2 是近似）
    )
}

// ⚠ 这里原先有 `mcloud push config [show|set|off]`（**用户级渠道配置** ✓）——
// 2026-10-08 推翻重做时**整个删掉** ✗：用户口径是「规则配置推送的 token，
// 包括推送哪几个平台」✓ ⇒ 凭据在**规则自己的 `config`** 里 ✓，
// 再留一层"用户级渠道"就一定会出现"两份 token 对不上"✓。
// 替代入口：`mcloud push add <名字> --json <规则 JSON>` ✓ / WebUI 的设置页 ✓。
