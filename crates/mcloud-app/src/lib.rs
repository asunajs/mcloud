//! mcloud-app —— 组装层。
//!
//! 把具体 IO 实现（reqwest / JSON 存储 / 系统时钟）注入 engine，
//! 供 `mcloud` 那个唯一二进制共用（2026-10-03 起 CLI 与服务合并成一个 bin）。

pub mod auth_refresh;
pub mod builder;
pub mod hub;
pub mod login;
// 「直播自动定时」：读共享口令池 → 算时刻 → upsert 那条 `auto-live-<owner>`。
// 触发点是**运行收尾**（`notify.rs` 的 `Finished` 分支 ✓），见模块头。
pub mod live_auto;
pub mod notify;
// 多渠道运行推送（照 mcloud-sign 的 push 模块移植）。
// ⚠ 本版**只做模块、不接线**：没有生产代码调用它，见 push.rs 模块头。
pub mod push;
// 「这个 run 是哪个账号」—— 让每一行日志都认得出主人。见模块头：
// 用户报「页面上看不到是那个账号的日志啊」的根因在**事件流**（账号只挂在 Started 上），
// 不在页面 ⇒ 由 sink 在这一层补齐。
pub mod run_labels;
pub mod sink;
pub mod smid;

pub use auth_refresh::{decide, AuthRefreshService, RefreshDecision, RefreshOutcome};
pub use builder::{App, AppBuilder};
pub use hub::{Delivery, EventHub, SubscribeError};
pub use live_auto::{read_auto_state, reschedule_live_auto, AutoLivePatch, AutoLiveState};
pub use login::{bare_transport, parse_platform, platform_label, AppSendSmsOutcome, LoginService};
pub use push::{
    format_logs_for_push, send_notify, send_request, BarkConfig, Channel, ChannelOutcome,
    CustomPost, DingTalkConfig, EmailConfig, PushApiData, PushConfig, PushContext,
    PushLogCollector, PushLogEntry, PushLogLevel, PushRequest, PushResponse, PushTransport,
    TgBotConfig, TokenConfig, TwoImConfig, WorkWeixinBotConfig, WorkWeixinConfig,
};
pub use run_labels::{trace_event, RunLabels};
pub use sink::BroadcastSink;
pub use smid::{
    device_id, ensure_fingerprint, report_device_id, rotate_fingerprint, SmidClient, SmidService,
    UploadContext,
};

use std::sync::Arc;

/// 应用句柄。
pub struct AppHandle {
    /// 数据目录（`.json` 表 + 日志都在这儿）。
    ///
    /// 放在句柄上而不是 `App` 上：WebUI 的「存储信息」要报绝对路径，
    /// 而服务器只拿得到句柄（`ServerState::new(Arc::new(app.handle))`）。
    pub data_dir: std::path::PathBuf,
    /// 主动关停信号。
    ///
    /// WebUI 的「重启服务」用它：`notify_one` ⇒ `main.rs` 的
    /// `with_graceful_shutdown` 分支醒过来 ⇒ 进程优雅退出 ⇒ 由 launchd/systemd 重拉。
    /// **不自己 `kill`**：托管场景下信号会走一遍完整收尾（停调度器、日志落盘），
    /// 而 `process::exit` 会把这些全跳过。
    pub shutdown: Arc<tokio::sync::Notify>,
    pub runner: Arc<mcloud_engine::Runner>,
    pub registry: Arc<mcloud_engine::Registry>,
    pub store: Arc<dyn mcloud_core::Store>,
    pub clock: Arc<dyn mcloud_core::Clock>,
    /// 事件中枢：日志收集、序号分配、SSE 重放都经它。
    pub hub: Arc<EventHub>,
    /// 定时调度器。
    pub scheduler: Arc<mcloud_engine::Scheduler>,
    /// 编排执行器。
    pub pipelines: Arc<mcloud_engine::PipelineRunner>,
    /// 运行中任务的取消登记表。
    pub cancel: Arc<mcloud_engine::CancelRegistry>,
    /// **推送投递器**。
    ///
    /// 两条路都在用它：
    /// * **运行类**（定时 / 编排跑完 ✓）：sink 那条线投递 ✓（`notify::decorate_with` ✓）；
    /// * **面板类**（登录新设备 / 登录失败 ✓）：`mcloud-server` 的 `routes/auth.rs`
    ///   从**句柄**上取它调 `spawn_panel` ✓（登录请求路径拿不到 sink ✓）。
    ///
    /// ⚠ CLI 收尾还要等它清空（`Notifier::flush` ✓）—— 短命进程不能把在飞的推送
    /// 一起掐掉 ✗（见 `Notifier::flush` 的说明 ✓）。
    pub pushes: Arc<crate::notify::Notifier>,
    /// smid 设备指纹编排（查看当前设备、主动更换）。
    ///
    /// 请求路径上的 `deviceid` 由 `SmidClient` 自动补，不需要任务关心；
    /// 这一份是给 WebUI / CLI 用的（"换一台设备"这类显式动作）。
    pub smid: Arc<SmidService>,
    /// 账号登录编排（扫码 / 短信 / 账密）。
    ///
    /// 与 `smid` 同类：请求路径不需要它，是给 CLI / WebUI 的**交互式**入口
    /// （登录要等人扫码、输验证码，做不成无人值守的 `Task`）。
    pub login: Arc<crate::login::LoginService>,
    /// 凭据续期编排。
    ///
    /// 与 `login` 并列的理由：它也需要按账号构造请求，而 "刷哪个账号"
    /// 是调用方给的。四个触发面（CLI 单账号 / CLI 批量 / WebUI 按钮 /
    /// 服务内调度）共用它，所以判据与窄写不会各自漂移。
    pub auth_refresh: Arc<crate::auth_refresh::AuthRefreshService>,
    /// **按账号**的业务 HTTP 客户端工厂（生产 = `builder.rs` 里那套装饰器链 ✓）。
    ///
    /// # 为什么挂在句柄上
    ///
    /// 「直播自动定时」的 `PUT` 路由要**立刻重算一次**，而重算要拿
    /// **每个账号自己的票**去上游现取「下一场」（用户口径 2026-10-08：
    /// 「下一场应该从上游获取啊」✓）—— 路由那层只拿得到句柄 ✓。
    ///
    /// ⚠ 它**不是**给任务用的：任务的客户端由 `Runner` 在每次运行时按账号造 ✓
    /// （`TaskContext.http` ✓）。这里这份只是"服务进程里那些**不在任务里**的
    /// 上游调用"（目前只有直播自动定时的重算 ✓）的入口。
    pub http_for: mcloud_engine::HttpClientFactory,
}

impl AppHandle {
    /// **种下"默认的那两条"**：默认编排 + 默认定时（各自幂等 ✓）。
    ///
    /// # 用户口径（2026-10-07）
    ///
    /// 「实现一个默认的编排任务，让本项目能和 ts 版一样一键运行，**cli 一条命令，
    /// webui 一个定时**」✓ —— 紧接着两次更正把形状定死了：
    ///
    /// 1. 「**默认编排也要和普通的自定义编排一个逻辑**」⇒ 它落库之后就是**一条普通编排**
    ///    （普通定时同理）：没有"内置"标记、没有专用执行路径，运行/定时/编辑/删除
    ///    与手建的一条**逐格相同** ✓；
    /// 2. 「**啊？不自动新建怎么算默认**」⇒ "默认"的意思就是**开箱就有** ✓，
    ///    不是"界面上放个按钮等人点" ✗。
    ///
    /// # 幂等：按 id 判（两张表各自的 id 都是 `daily` ✓）
    ///
    /// 已有就**不动它** ✓ —— 用户改过默认编排（去掉几步、改名）之后，下次启动不会被
    /// 覆盖回原样 ✓。⚠ 反过来说：**删掉它，下次启动会再种一条**（"默认"就是这个语义 ✓）。
    ///
    /// # 归属
    ///
    /// 写 `None`：与老数据同形，加载时回填给管理员（`owner::legacy_owner` ✓）。
    /// 服务那条路在**调度器读盘之前**调它（`main.rs` ✓），所以那条默认定时第一次启动
    /// 就会被调度器看见 ✓。
    /// 给**一个用户**建好他自己的三份"开箱就有"：默认编排 + 默认定时 + 「直播自动定时」
    ///（幂等：已有就不动 ✓）。
    ///
    /// ⚠ 第三条是**后加**的（2026-10-08 ✓），所以它对**老用户**也是"第一次走到这里
    /// 才补上"（启动与登录都调本函数 ✓）—— 用户口径：「别的用户压根没有这条条目」✗
    /// ⇒ 要补的正是"**每个用户开箱就有**" ✓。
    ///
    /// # 用户口径（2026-10-07，三次更正后的定稿）
    ///
    /// 「实现一个默认的编排任务…cli 一条命令，webui 一个定时」→
    /// 「默认编排也要和普通的自定义编排一个逻辑」→「不自动新建怎么算默认」→
    /// 「默认编排应该是給所有人」→「**我不要这种所有人可动的，别人给你改了不就跑不了了？
    /// 不能登录时給好建一个？**」✓
    ///
    /// ⇒ 定稿是**每人一份自己的**：id `daily-<用户id>`（`default_pipeline_id` ✓）、
    /// 归属就是这个用户 ✓ ⇒ 只有他自己看得见、改得动 ✓，
    /// 而"默认"仍然是**开箱就有**（登录/启动时给好 ✓）。
    ///
    /// ⚠ **不做共享那一条**（我做过、被否掉 ✗）：共享 = 谁都能改能删 ⇒
    /// 别人改坏了，你的每日就跑不了了 ✗ —— 这正是用户否掉它的理由 ✓。
    pub async fn seed_defaults_for(&self, owner: &mcloud_core::UserId) -> mcloud_core::Result<()> {
        let id = mcloud_core::pipeline::default_pipeline_id(Some(owner));
        let pid = mcloud_core::PipelineId::new(id.clone());
        if self.store.pipeline(&pid).await?.is_none() {
            self.store
                .save_pipeline(&mcloud_core::pipeline::default_pipeline(Some(
                    owner.clone(),
                )))
                .await?;
        }
        let sid = mcloud_core::ScheduleId::new(id);
        if self.store.schedule(&sid).await?.is_none() {
            self.store
                .save_schedule(&mcloud_core::pipeline::default_schedule(Some(
                    owner.clone(),
                )))
                .await?;
        }
        // ⭐ 第三条：**「直播自动定时」也开箱就有**（用户口径 2026-10-08
        // 「小红花定时默认开启」✓）。
        //
        // ⚠ 与上面两条**同一个幂等口径**：已经有就一个字节都不动 ✓ ——
        // 尤其**不覆盖用户的 `managed_wanted: Some(false)`**（他明确关过 ✓），
        // 也**不碰**恰好占着这个 id 的用户手建定时 ✓（判据在
        // `live_auto::seed_auto_schedule` 那一处 ✓）。
        //
        // ⚠ 种完**只在"这次真的新种出来"时**顺手在**后台**重算一次
        //（`spawn` 出去、**不 await** ✓，失败只 warn ✓）：为什么不 await、
        // 为什么已经有就不再打上游、以及"这次重算全是读、**不会领小红花**"，
        // 三条理由都写在 `live_auto::seed_and_reschedule_auto` /
        // `spawn_seed_reschedule` 的注释里 ✓。
        crate::live_auto::seed_and_reschedule_auto(
            &self.store,
            &self.clock,
            Some(&self.http_for),
            owner,
        )
        .await?;
        Ok(())
    }

    /// 给**库里每个用户**都建好他自己那份（服务启动时调一次 ✓）。
    ///
    /// ⚠ 登录那条路也会补一次（`routes/auth.rs` ✓）：`mcloud user add` 加进来的新用户
    /// **不必等下一次重启** ✓。
    ///
    /// ⚠ 每人也各带一条「直播自动定时」（`auto-live-<他的id>` ✓，见
    /// [`AppHandle::seed_defaults_for`] ✓）—— 它是 `enabled=false` 起步的，
    /// 所以调度器这一趟读盘**不会**因为它多跑任何东西 ✓（`decide()` 对停用直接返回 ✓）。
    pub async fn seed_defaults(&self) -> mcloud_core::Result<()> {
        for user in self.store.users().await? {
            self.seed_defaults_for(&user.id).await?;
        }
        Ok(())
    }

    /// 用既有历史**续上 run id**。起服务之前必须调一次。
    ///
    /// # 为什么这件事非做不可（2026-10-01 实际踩到）
    ///
    /// `Runner` 的编号从 **1** 开始（`Runner::new`），而运行记录是**按编号 upsert** 的
    /// （`RunStore::upsert_run`：同编号就覆盖）。两件事撞在一起 = **静默丢数据**：
    /// 每次重启之后新跑的 1、2、3… 会把**上次启动**留下的同编号记录一条条盖掉，
    /// 于是一重启历史就"只剩最近这两条"。
    ///
    /// 用户看到的是"为啥历史只有两条" —— 而原因是编号被复用了，
    /// 与历史页、与存储限额都无关。
    ///
    /// # 为什么不在 `AppBuilder::build()` 里顺手做掉
    ///
    /// `build()` 是**同步**的，而读历史是 `async`（`RunStore::runs`）。
    /// 在 sync 里 `block_on` 一个库调用，在 tokio 运行时里会直接 panic ——
    /// 那是比这个 bug 更难查的一类故障。所以它做成必须显式调用的方法，
    /// 调用点唯一且显眼：`mcloud-server/src/main.rs`。
    pub async fn seed_run_ids(&self) -> mcloud_core::Result<()> {
        let runs = self.store.runs(usize::MAX).await?;
        if let Some(max) = runs.iter().map(|r| r.run_id.0).max() {
            self.runner.seed_run_id(max);
            tracing::info!(max_run_id = max, kept = runs.len(), "已从历史续上 run id");
        }
        Ok(())
    }

    /// 事件中枢也是 `EventSink`，任务通过它发日志。
    pub fn sink(&self) -> Arc<dyn mcloud_core::EventSink> {
        Arc::clone(&self.hub) as Arc<dyn mcloud_core::EventSink>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{PasswordHash, User, UserId};

    fn user(id: &str, created_ms: i64) -> User {
        User {
            id: UserId::new(id),
            name: id.into(),
            password_hash: PasswordHash("hashed".into()),
            created_at_ms: created_ms,
        }
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d =
            std::env::temp_dir().join(format!("mcloud-app-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// ⭐⭐ 哨兵：默认编排是**每人一份自己的**，不是一条共享的。
    ///
    /// # 这条用例钉的是用户三次更正后的定稿（2026-10-07）
    ///
    /// 「默认编排应该是給所有人」→「**我不要这种所有人可动的，别人给你改了不就跑不了了？
    /// 不能登录时給好建一个？**」✓
    ///
    /// ⇒ 三条断言，各自对应一个被否掉的写法：
    /// 1. **每人一条、id 不同** —— 否掉"共享一条" ✗；
    /// 2. **各归各的归属** ⇒ 只有他自己看得见、改得动 —— 否掉"谁都能改" ✗
    ///    （`pipelines_for(&other)` 看不到你的 ✓，路由按归属查 ⇒ 也就改不了 ✓）；
    /// 3. **两人都有** —— 否掉"只给管理员" ✗。
    #[tokio::test]
    async fn seeded_defaults_are_per_user_and_private() {
        let dir = tmpdir("seed");
        let app = AppBuilder::new().data_dir(&dir).build().unwrap();
        let admin = UserId::new("admin-1");
        let other = UserId::new("user-2");
        app.handle
            .store
            .save_user(&user("admin-1", 1))
            .await
            .unwrap();
        app.handle
            .store
            .save_user(&user("user-2", 2))
            .await
            .unwrap();

        app.handle.seed_defaults().await.unwrap();

        let ps_admin = app.handle.store.pipelines_for(&admin).await.unwrap();
        let ps_other = app.handle.store.pipelines_for(&other).await.unwrap();
        assert_eq!(ps_admin.len(), 1, "管理员有他自己那一份：{ps_admin:?}");
        assert_eq!(ps_other.len(), 1, "普通用户也有他自己那一份：{ps_other:?}");
        assert!(
            ps_admin[0].id.as_str().starts_with("daily-"),
            "id 要带归属：{}",
            ps_admin[0].id
        );
        assert_ne!(
            ps_admin[0].id, ps_other[0].id,
            "**两人两条**，不是共享一条（共享那条谁都能改 ⇒ 别人改坏了你就跑不了了 ✗）"
        );
        assert_eq!(ps_admin[0].owner.as_ref(), Some(&admin));
        assert_eq!(ps_other[0].owner.as_ref(), Some(&other));
        // ⭐ 关键：**别人的看不见** ⇒ 路由按归属查 ⇒ 也就**改不了** ✓
        assert!(
            ps_admin.iter().all(|p| p.owner.as_ref() == Some(&admin)),
            "管理员那份不该混进别人的"
        );

        // 定时：每人**两条** —— 默认编排那条（`daily-<id>`）+ 「直播自动定时」
        //（`auto-live-<id>` ✓，2026-10-08 起"开箱就有" ✓）。两条都各归各的 ✓。
        let ss_admin = app.handle.store.schedules_for(&admin).await.unwrap();
        let ss_other = app.handle.store.schedules_for(&other).await.unwrap();
        assert_eq!(ss_admin.len(), 2, "默认编排 + 直播自动定时：{ss_admin:?}");
        assert_eq!(ss_other.len(), 2, "{ss_other:?}");
        assert!(
            ss_admin.iter().all(|s| s.owner.as_ref() == Some(&admin)),
            "别人的定时不该混进来"
        );
        let daily_admin = ss_admin
            .iter()
            .find(|s| s.id.as_str().starts_with("daily-"))
            .expect("默认编排那条定时必须在");
        let daily_other = ss_other
            .iter()
            .find(|s| s.id.as_str().starts_with("daily-"))
            .expect("普通用户也有一条");
        assert_ne!(daily_admin.id, daily_other.id, "定时同理：各是各的");
        assert_eq!(
            daily_admin.body.get("pipelineId").and_then(|v| v.as_str()),
            Some(ps_admin[0].id.as_str()),
            "定时要指向**自己那条**编排，别指向别人的"
        );
        // ⭐ 那条「直播自动定时」也在，而且四格值就是定稿那四格 ✓
        //（`enabled=false` 是**重算前**的形态：这个 App 的用户一个云盘账号都没有、
        //  口令池也是空的 ⇒ 后台那次重算算不出时刻，它照旧停用 ✓）。
        let auto = ss_admin
            .iter()
            .find(|s| s.is_managed_by(mcloud_core::live_auto::MANAGED_BY_LIVE_AUTO))
            .expect("「直播自动定时」必须开箱就有");
        assert_eq!(
            auto.id,
            mcloud_core::live_auto::live_auto_schedule_id(&admin)
        );
        assert!(!auto.enabled, "先关着：还没算出下一场");
        assert_eq!(
            auto.managed_wanted,
            Some(true),
            "「默认开启」= 用户意愿那一格是「要」"
        );
        assert_eq!(auto.task, mcloud_core::live_auto::LIVE_ROOM_TASK);

        // 幂等：再种一次不报错、也不多出条目（每人仍是各两条 ✓）
        app.handle.seed_defaults().await.unwrap();
        assert_eq!(app.handle.store.pipelines().await.unwrap().len(), 2);
        assert_eq!(app.handle.store.schedules().await.unwrap().len(), 4);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
