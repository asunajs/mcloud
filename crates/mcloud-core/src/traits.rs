//! IO 边界：所有外部能力以 trait 形式暴露。
//!
//! 上层（engine / tasks）只依赖这些 trait；具体实现（reqwest、JSON 文件、
//! 系统时钟）由 `mcloud-app` 注入。这样：
//!
//! - 单元测试可用内存实现，无需网络与磁盘；
//! - 未来把 JSON 存储换成 SQLite 不需要改动业务代码。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::account::{Account, AccountId};
use crate::error::Result;
use crate::event::RunEvent;
use crate::log::{slice_page, AppendOutcome, LogLine, LogPage, LogPageQuery, LogStats, Seq};
use crate::pipeline::{Pipeline, PipelineId};
use crate::run::{RunId, RunRecord};
use crate::schedule::{Schedule, ScheduleId};
use crate::settings::AppSettings;
use crate::smid::SmidStore;
use crate::user::{Session, User, UserId};

// ── HTTP ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// 请求体的**原始字节**。
    ///
    /// # ⚠ 为什么是字节而不是 `String`
    ///
    /// 网盘上传那一枪（`PUT <uploadUrl>`）发的是**图片的原始字节** ——
    /// 用 `String` 装不下（那不是 UTF-8 文本，`from_utf8` 会失败或损坏内容）。
    /// 而"损坏内容"在那个位置**不会报错**：上游只会回一个校验失败，
    /// 排查方向会整个跑偏。所以类型上就让它装得下。
    ///
    /// 文本请求照旧用 [`HttpRequest::json_body`] / [`HttpRequest::text_body`]，
    /// 它们只是把文本转成字节；读回文本用 [`HttpRequest::body_str`]。
    #[serde(default)]
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: "POST".into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// `PUT` —— 上传那一枪用的方法。
    pub fn put(url: impl Into<String>) -> Self {
        Self {
            method: "PUT".into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn json_body(mut self, v: &serde_json::Value) -> Self {
        self.body = Some(v.to_string().into_bytes());
        self
    }

    pub fn text_body(mut self, s: impl Into<String>) -> Self {
        self.body = Some(s.into().into_bytes());
        self
    }

    /// 二进制体（上传图片用）。
    pub fn bytes_body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = Some(bytes.into());
        self
    }

    /// 把体读成文本（测试与少数要读回原文的调用点用）。
    ///
    /// ⚠ 体不是合法 UTF-8 时回 `None` —— 那说明这一枪带的是二进制，
    /// **不是**"没有体"（要判有没有体用 `body.is_some()`）。
    pub fn body_str(&self) -> Option<&str> {
        match self.body.as_ref() {
            Some(bytes) => std::str::from_utf8(bytes).ok(),
            None => None,
        }
    }

    /// 请求是否**带体**。
    ///
    /// ⚠ **空体也算带体**：`Some(vec![])` 与 `None` 语义不同 ——
    /// 直播那两条 `give` / `draw` 靠 `None` 表达"无体"，
    /// 而"显式给了个空体"是要发出一个 `content-length: 0` 的。
    /// ⇒ 判"有没有体"只看 `is_some()`，**不要**顺手加 `!is_empty()`：
    /// 那会把显式空体变成无体，表现是少一个头、少一段长度声明。
    pub fn has_body(&self) -> bool {
        self.body.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 解析为 JSON。失败时给出截断后的原文，便于定位。
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_str(&self.body).map_err(|e| {
            let head: String = self.body.chars().take(200).collect();
            crate::Error::Parse(format!("{e}；原文前 200 字符：{head}"))
        })
    }
}

/// HTTP 客户端抽象。
///
/// 实现方负责注入域名相关的凭证头（authorization / jwttoken / deviceid），
/// 上层无需关心。
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse>;

    /// 取**原始响应字节** —— 不走任何文本解码。
    ///
    /// # 为什么需要第二条通路
    ///
    /// [`HttpResponse::body`] 是 `String`，发送层填它时走的是
    /// `reqwest::Response::text()`（**有损** UTF-8）。图片（JPEG / PNG）不是
    /// UTF-8 文本：走那条路回来的是被替换字符破坏过的内容，而且**不会报错**；
    /// 再 base64 交给 upload，上游仍可能回 `code:0` ⇒ 网盘里多一张打不开的图。
    /// 这正是本仓最忌讳的「不发错、不报错，只是东西不对」。
    /// ⇒ 二进制响应体（AI 头像那条链下载合成图）必须走这一条。
    ///
    /// # 默认实现为什么是这个形状，以及**谁**可以不覆写
    ///
    /// 默认实现调 `self.request()` 再取字节 —— 这是**故意**的：按 host 注入凭据
    /// （`authorization` / `jwttoken` / `deviceid`）发生在 `request()` 里，
    /// 所以默认实现**不是**绕过注入的旁路，走到它时凭证照旧。
    ///
    /// 由此两条：
    ///
    /// 1. **不包别的 `HttpClient` 的实现（leaf）** 覆写它 —— 只有 leaf 手里
    ///    才有线上字节。本仓的 leaf 是 `ReqwestClient`（credential 注入也正好
    ///    在它里面，两件事是**同一个**类型）—— 只有它覆写：用 `.bytes()`。
    ///    测试替身想给非 UTF-8 字节时也必须覆写（`String` 装不下）。
    /// 2. ⚠⚠ **包着另一个 `HttpClient` 的装饰器必须转发**
    ///    （`self.transport.request_raw(req).await`），**不能只吃默认实现**。
    ///    原因不是洁癖：默认实现调的是 `self.request()`，对装饰器而言它会落到
    ///    **内层**的 `request()`，而最内层（leaf）回的是**有损文本**
    ///    ⇒ 字节在这一跳就毁了（`into_bytes()` 只是把被毁的内容再编码一次）。
    ///    ⚠ 本仓的链是 `SmidClient → H5Client → ReqwestClient`：这两个装饰器
    ///    今天都**还没有**转发，所以经过它们时这条通路会退化成「有损文本再转字节」。
    ///    忘了转发**不报错**，只会让下游的形状校验判失败（那正是要防的方向，
    ///    只是这次被校验挡住了）。
    async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
        let resp = self.request(req).await?;
        Ok(resp.body.into_bytes())
    }

    /// 便捷方法。
    async fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<HttpResponse> {
        self.request(HttpRequest::post(url).json_body(body)).await
    }
}

// ── 存储 ────────────────────────────────────────────────────────────────

/// 持久化抽象。
///
/// 拆成几个聚焦的子 trait（而不是一个 20 方法的大接口）：
///
/// - 任务只需要 `LogStore`，不该被迫依赖账号读写；
/// - 换后端时（JSON ↔ SQLite）可以逐个替换；
/// - 测试里可以只造假的那一个。
///
/// `Store` 是它们的组合，凡是"能注入一个后端"的地方都收 `Store`。
#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn accounts(&self) -> Result<Vec<Account>>;
    async fn account(&self, id: &AccountId) -> Result<Option<Account>>;

    /// 列出**归 owner 所有**的账号（列表页唯一该用的那条）。
    ///
    /// # 为什么默认实现是「空表」而不是「调用 accounts()」
    ///
    /// 归属过滤是安全边界，默认必须**失败关闭**：真后端忘了实现它，
    /// 表现是「看不到任何账号」（会立刻被发现），而不是「看到别人的账号」
    /// （静默、不可发现）。本仓唯一的落盘后端 `JsonStore` 覆写它；
    /// 测试替身不需要归属，默认空表不会影响它们。
    async fn accounts_for(&self, _owner: &UserId) -> Result<Vec<Account>> {
        Ok(Vec::new())
    }

    /// 取**归 owner 所有**的那个账号。不是自己的、或不存在，一律 `None`
    /// —— 调用方据此回 **404**，不泄露「它存在但不属于你」这个事实
    /// （`docs/architecture.md` 的「归属错误一律 404」）。
    async fn account_for(&self, _owner: &UserId, _id: &AccountId) -> Result<Option<Account>> {
        Ok(None)
    }
    /// 保存账号。实现方必须**原子写**并保留备份。
    async fn save_account(&self, account: &Account) -> Result<()>;
    async fn remove_account(&self, id: &AccountId) -> Result<()>;

    /// 读取账号运行期状态（deviceId、jwtToken 缓存等）。
    async fn account_state(&self, id: &AccountId) -> Result<serde_json::Value>;
    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()>;
}

/// 运行记录。
#[async_trait]
pub trait RunStore: Send + Sync {
    /// 落盘一条运行记录。**同 `run_id` 已存在时是更新**（运行开始时先写
    /// `running`，结束时回填结果）。
    async fn upsert_run(&self, record: &RunRecord) -> Result<()>;
    async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>>;

    /// 列出**归 owner 所有**的运行记录（最近在前）。
    ///
    /// 运行记录自己没有 owner 列 —— 它的归属由 `RunRecord.account` 指向的那个
    /// 账号决定（账号已不存在时按未归属处理）。默认失败关闭（空表）。
    async fn runs_for(&self, _owner: &UserId, _limit: usize) -> Result<Vec<RunRecord>> {
        Ok(Vec::new())
    }

    /// 取**归 owner 所有**的那条运行记录；不是自己的、或不存在，都是 `None`。
    ///
    /// 日志 / SSE / 中止这三条入口都先过它 —— 拿不到运行记录就回 404，
    /// 于是「看不到别人的日志」与「看不到别人的运行记录」是同一条判据。
    async fn run_for(&self, _owner: &UserId, _id: RunId) -> Result<Option<RunRecord>> {
        Ok(None)
    }
    async fn run(&self, id: RunId) -> Result<Option<RunRecord>>;
    /// 列出仍在运行中的记录 —— 进程重启后据此把孤儿标记为已中止。
    async fn running_runs(&self) -> Result<Vec<RunRecord>>;
}

/// 运行日志。
#[async_trait]
pub trait LogStore: Send + Sync {
    /// 追加一批日志。超出上限时丢弃最旧的并返回 `Trimmed`。
    async fn append_logs(&self, run_id: RunId, lines: &[LogLine]) -> Result<AppendOutcome>;
    /// 取某次运行的日志。`after_seq` 用于断线重连续传。
    async fn logs(&self, run_id: RunId, after_seq: Option<Seq>) -> Result<Vec<LogLine>>;
    /// 删除某次运行的日志（运行记录被清理时一并删）。
    async fn clear_logs(&self, run_id: RunId) -> Result<()>;

    /// **分页**取某次运行的日志（用户口径 2026-10-09：
    /// 「大日志做虚拟加载或者分页，避免一次加载太多」✓）。
    ///
    /// # 为什么给默认实现
    ///
    /// 本仓有二十来处"假 store"（各任务的测试夹具 ✓）只实现了上面三个方法。
    /// 让它们全部跟着改 = 为一个纯函数式的切片改动二十个文件 ✗。
    /// 默认实现**就是**那条判据本身（[`slice_page`] ✓）⇒ "没覆盖"不等于
    /// "语义不同" ✓ —— 只有 `JsonStore` 需要覆盖它（它要**按需读盘**，
    /// 而不是先把整个文件读进内存 ✓）。
    async fn logs_page(&self, run_id: RunId, q: LogPageQuery) -> Result<LogPage> {
        let all = self.logs(run_id, q.after).await?;
        Ok(slice_page(&all, &q))
    }

    /// 日志存储的用量读数（给「存储信息」卡与容量上限用 ✓）。
    ///
    /// 默认全 0：假的 store 不落盘，报 0 是**真话** ✓（比编一个数强 ✓）。
    async fn log_stats(&self) -> Result<LogStats> {
        Ok(LogStats::default())
    }
}

/// 定时任务。
#[async_trait]
pub trait ScheduleStore: Send + Sync {
    async fn schedules(&self) -> Result<Vec<Schedule>>;
    async fn schedule(&self, id: &ScheduleId) -> Result<Option<Schedule>>;

    /// 列出**归 owner 所有**的定时。默认失败关闭（空表），理由同
    /// [`AccountStore::accounts_for`]。
    async fn schedules_for(&self, _owner: &UserId) -> Result<Vec<Schedule>> {
        Ok(Vec::new())
    }

    /// 取**归 owner 所有**的那条定时；不是自己的、或不存在，都是 `None`。
    async fn schedule_for(&self, _owner: &UserId, _id: &ScheduleId) -> Result<Option<Schedule>> {
        Ok(None)
    }
    async fn save_schedule(&self, schedule: &Schedule) -> Result<()>;
    async fn remove_schedule(&self, id: &ScheduleId) -> Result<()>;

    /// **窄写**：只推进水位线。
    ///
    /// 为什么不能复用 `save_schedule`：调度器在触发那一刻只持有
    /// "几秒前读到的行"，整行覆盖会把用户在这期间通过 WebUI 改过的
    /// 名字/参数一起抹掉。而水位线必须在打上游**之前**落盘，
    /// 所以这个写既紧急又必须最小。
    ///
    /// ⚠ 它会**累加 `run_count`**（"跑过一次"的记账）。要推进水位但**不**计数的
    /// 场合（错过的触发点被整体弃掉）用 [`Store::advance_schedule_watermark`]。
    async fn mark_schedule_fired(&self, id: &ScheduleId, fire_ms: i64) -> Result<()>;

    /// **窄写**：推进水位线，但**不**累加运行次数。
    ///
    /// 为什么要和 [`Store::mark_schedule_fired`] 分开（2026-10-03 review §1.7）：
    /// 那个方法里 `run_count += 1` 记的是"真的跑过一次"，而
    /// `Decision::Missed`（错过的触发点整体弃掉）也调它 ⇒ 一次上游都没打的**弃置**
    /// 会被记成一次运行，界面上「累计触发 N 次」（`schedules-page.tsx:384`）虚高。
    ///
    /// 默认实现退回 `mark_schedule_fired`：生产只有 `JsonStore` 一处要覆盖，
    /// 而各任务测试里的 `Store` 桩有十来份 —— 给一个**默认方法**，
    /// 才不用为这一行去改那十来份桩。
    async fn advance_schedule_watermark(&self, id: &ScheduleId, fire_ms: i64) -> Result<()> {
        self.mark_schedule_fired(id, fire_ms).await
    }

    /// **窄写**：只记录本次执行结果。
    ///
    /// `run_ms` 为 `None` ⇒ **不动**这一列（错过的触发点被弃掉时不产生运行，
    /// 抹掉真实的上次运行时间会显示成"从未执行"）。
    /// `error` 为 `None` ⇒ **写入空**（清掉旧错误本身就是结论，
    /// 否则"成功"旁边挂着一条旧错误，正是查不出原因的形态）。
    async fn record_schedule_run(
        &self,
        id: &ScheduleId,
        run_ms: Option<i64>,
        status: &str,
        error: Option<&str>,
    ) -> Result<()>;

    /// **窄写**：只记「上次结果」的那句**人话说明**（[`Schedule::last_note`] ✓）。
    ///
    /// `note` 为 `None` ⇒ **写入空**（与 `record_schedule_run` 的 `error` 同一条口径：
    /// 结论变了却留着上一趟的说明，就是"成功旁边挂着旧错误"那类形态 ✓）。
    ///
    /// # 为什么不并进 `record_schedule_run`
    ///
    /// 并进去要给那一格加一个参数 ⇒ 本仓**二十来处**假 store（各任务的测试夹具 ✓）
    /// 全得跟着改签名 —— 为一个纯展示字段改二十个文件 ✗。
    /// 给一个**默认实现**（什么都不做 ✓），才不用动那些桩
    ///（与 [`Store::advance_schedule_watermark`] 那条理由逐字相同 ✓）。
    ///
    /// ⚠ 代价说清楚：它是**第二次**窄写，两写之间进程被杀会留下
    /// "状态是新的、说明是上一趟的"那一瞬。这里接受它 ——
    /// 这一格**纯展示**、两写相隔微秒，且只有「凭据自动刷新」那条定时会写它
    ///（它每次触发都写 ⇒ 下一趟就纠正回来 ✓）。
    ///
    /// 默认实现什么都不做 ⇒ 假 store 不记这一格，而**真 store（`JsonStore`）会记** ✓。
    async fn record_schedule_note(&self, _id: &ScheduleId, _note: Option<&str>) -> Result<()> {
        Ok(())
    }
}

/// 任务编排。
#[async_trait]
pub trait PipelineStore: Send + Sync {
    async fn pipelines(&self) -> Result<Vec<Pipeline>>;
    async fn pipeline(&self, id: &PipelineId) -> Result<Option<Pipeline>>;

    /// 列出**归 owner 所有**的编排。默认失败关闭（空表），理由同
    /// [`AccountStore::accounts_for`]。
    async fn pipelines_for(&self, _owner: &UserId) -> Result<Vec<Pipeline>> {
        Ok(Vec::new())
    }

    /// 取**归 owner 所有**的那条编排；不是自己的、或不存在，都是 `None`。
    async fn pipeline_for(&self, _owner: &UserId, _id: &PipelineId) -> Result<Option<Pipeline>> {
        Ok(None)
    }
    async fn save_pipeline(&self, pipeline: &Pipeline) -> Result<()>;
    async fn remove_pipeline(&self, id: &PipelineId) -> Result<()>;

    /// **窄写**：只记上次执行时间。
    ///
    /// 一次编排要跑几分钟；用整行 `save_pipeline` 会把这段时间里
    /// 用户改过的名字与步骤一起抹掉。
    async fn mark_pipeline_ran(&self, id: &PipelineId, run_ms: i64) -> Result<()>;
}

/// 系统用户与会话（WebUI 登录闸）。
///
/// 与 `AccountStore` 分开是刻意的：那是"要操作的对象"，这是"谁能操作"。
/// 合成一个 trait 会让"只想给任务写个内存假实现"的人被迫实现用户管理，
/// 而假实现正是保证函数级可测的手段。
#[async_trait]
pub trait UserStore: Send + Sync {
    /// 用户数。**首次部署判定用它**：0 个用户 ⇒ 需要走 `/api/setup`。
    async fn user_count(&self) -> Result<usize>;
    async fn users(&self) -> Result<Vec<User>>;
    async fn user_by_name(&self, name: &str) -> Result<Option<User>>;
    async fn user(&self, id: &UserId) -> Result<Option<User>>;
    async fn save_user(&self, user: &User) -> Result<()>;

    /// **首次初始化**：只在"一个用户都没有"时建这个管理员。
    /// 返回 `Ok(true)` = 建成了；`Ok(false)` = 已经有用户了（调用方据此拒绝）。
    ///
    /// # ⚠⚠ 为什么不能"先 `user_count` 再 `save_user`"（2026-10-06 安全审计 `SEC-09`）
    ///
    /// 那两步之间隔着一次完整的 argon2（几十毫秒）且**不持任何锁** ⇒ 两个并发的
    /// `POST /api/setup` 都能看到 0 个用户，于是各建一个管理员，谁也不知道对方存在：
    /// 后一个覆盖前一个的会话表，而"管理员"到底是谁从此不确定 ✗。
    /// 实现方必须在**同一把写锁内**完成"判空 + 插入"。
    ///
    /// # 为什么默认实现是**报错**
    ///
    /// 与 `save_settings` / `save_task_params` 同一口径：替身存储不支持就得**显式失败**，
    /// 而不是退回那个有竞态的两步写法 —— 那样调用方会以为这条已经修好了 ✗
    /// （而它只是把窗口缩小到"两次调用之间"，没有消除）。
    async fn create_first_user(&self, user: &User) -> Result<bool> {
        let _ = user;
        Err(crate::Error::Config(
            "这个存储不支持首次初始化（create_first_user 未实现）".into(),
        ))
    }

    /// 建会话（存的是 `sha256(token)`，不是原文）。
    async fn save_session(&self, session: &Session) -> Result<()>;
    /// 按令牌哈希查会话。
    async fn session(&self, token_hash: &str) -> Result<Option<Session>>;
    async fn remove_session(&self, token_hash: &str) -> Result<()>;
    /// 清掉某用户的所有会话（改口令时用 —— 旧会话必须失效）。
    ///
    /// 返回被清掉的条数：调用方可以据此提示"已登出 N 个设备"。
    async fn remove_user_sessions(&self, user_id: &UserId) -> Result<usize>;
    /// 清掉已过期的会话。顺手回收，避免表无限增长。
    async fn purge_expired_sessions(&self, now_ms: i64) -> Result<usize>;

    /// 读**共享口令池**（实例级：跨 mcloud 用户、跨重启）。
    ///
    /// 默认实现回一个空池（不是错误）：测试替身不关心它，而空池的语义就是
    /// "什么都没抓过" —— 一切都会照常走一遍抓取。
    ///
    /// # 为什么也挂在 `UserStore`
    ///
    /// 与 `settings()` / `task_params()` 同一个理由：新开 trait 会让 16 处
    /// 测试替身各补一行空实现。语义上它属于**实例**而不是某个用户 ——
    /// 而它偏偏必须跨用户共享，所以更不能挂在某个用户下面。
    async fn kouling_pool(&self) -> Result<Box<crate::kouling::KoulingPool>> {
        Ok(Box::new(crate::kouling::KoulingPool::default()))
    }

    /// 写共享口令池。
    ///
    /// 默认实现**报错**（与 `save_settings` 同一口径）：写不进去却回成功，
    /// 会让"够 2 条就停抓"这类判据永远不成立 —— 那是查都没法查的一种坏。
    async fn save_kouling_pool(&self, pool: &crate::kouling::KoulingPool) -> Result<()> {
        let _ = pool;
        Err(crate::Error::Config(
            "这个存储不支持下改口令池（save_kouling_pool 未实现）".into(),
        ))
    }

    /// 读某个任务的**默认参数**（任务页「配置」里存的那份）。
    ///
    /// 没配过就是**空对象**（不是错误）：前端据此回落到参数自己的默认值。
    ///
    /// # 为什么也挂在 `UserStore`
    ///
    /// 与 `settings()` 同一个理由（见下面的长注释）：新开 trait 会让 16 处测试替身
    /// 各补一行空实现。语义上它不属于"用户"，但**归属感不值得为它改 16 个文件**。
    async fn task_params(&self, task: &str) -> Result<serde_json::Value> {
        let _ = task;
        Ok(serde_json::Value::Object(serde_json::Map::new()))
    }

    /// 写某个任务的默认参数。
    ///
    /// 默认实现**报错**（与 `save_settings` 同一口径）：替身存储不支持，
    /// 就得让它显式失败，而不是默默吞掉——写不进去而界面说"已保存"是最坏的一种。
    async fn save_task_params(&self, task: &str, params: &serde_json::Value) -> Result<()> {
        let _ = (task, params);
        Err(crate::Error::Config(
            "这个存储不支持下改任务参数（save_task_params 未实现）".into(),
        ))
    }

    /// 读应用级设置（口令下限、会话时长）。
    ///
    /// # 为什么挂在 `UserStore` 而不是新开一个 trait
    ///
    /// 这两项就是**登录闸的策略**，与用户、会话是同一块东西。而新开一个 trait
    /// 会让每一个测试替身都得再补一行空实现 —— 本仓有 16 处 `impl UserStore for X`。
    ///
    /// # 为什么给默认实现
    ///
    /// 替身不关心设置，逼它们逐个实现只是噪音。真实存储（`JsonStore`）会覆盖它，
    /// 去读 `settings.json`。**默认值必须等于那两个老常量**（见 `settings.rs` 模块头）。
    async fn settings(&self) -> Result<AppSettings> {
        Ok(AppSettings::default())
    }

    /// 存应用级设置。
    ///
    /// 默认实现**报错**而不是静默成功：一个不持久化的存储若假装存上了，
    /// 用户会以为改生效了 —— 那比明确失败糟得多。真实存储会覆盖它。
    async fn save_settings(&self, _settings: &AppSettings) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持下改设置（save_settings 未实现）".into(),
        ))
    }

    /// 读**某个用户自己设的**预设刷新钟点（`HH:MM` ✓，2026-10-11 新增）。
    ///
    /// # 它只回"他自己设过的那一份" —— 没设过就是 `None`
    ///
    /// ⚠ 这里**刻意不回落**到 [`AppSettings::auth_refresh_at`]（老那个实例级值 ✓）：
    /// 回落那一步是**口径**（"他自己的优先，没设过用实例级那一格" ✓），
    /// 只能有一处实现 —— 在 [`crate::auth_refresh::effective_at`] ✓。
    /// 在这里也回落一次的话，"谁的值算数"就有了两份写法，迟早分叉 ✗
    /// （与 `push_panel()` 只回"他自己那份"、默认值由类型自己带是同一条取向 ✓）。
    ///
    /// 没设过 ⇒ `None`（**不是错误**）—— 与 `push_rules()` 回空列表同一口径 ✓。
    ///
    /// # 为什么按 `owner` 分表
    ///
    /// 用户口径（2026-10-11）：「**要每用户一个钟点**」✓ —— 那是**某个人自己**的选择，
    /// 与 `push_rules()` / `exchange_subscriptions()` 同一档 ✓：
    /// 换个人登录不该看见、更不该改得动别人的钟点 ✗。
    /// 落盘 `data/auth-refresh.json`（按 owner 分表 ✓）。
    async fn auth_refresh_at(&self, _owner: &UserId) -> Result<Option<String>> {
        Ok(None)
    }

    /// 写**某个用户自己那份**预设刷新钟点（**整格替换** ✓）。
    ///
    /// 默认实现**报错**（与 `save_settings` / `save_push_rule` 同一口径）：
    /// 写不进去却回成功，界面会说"已保存"而下次刷新就回到旧钟点 ✗。
    ///
    /// ⚠ 实现方要**自己守一道钟点的形状**（`auth_refresh::parse_clock` ✓）——
    /// 绕过 HTTP 的调用方（CLI / 脚本）同样不该写进非法值 ✓
    ///（与 `save_settings` 里那道 `validate()` 同一条理由 ✓）。
    async fn save_auth_refresh_at(&self, _owner: &UserId, _at: &str) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持下改预设刷新时间（save_auth_refresh_at 未实现）".into(),
        ))
    }

    /// 读兑换页的**订阅**（用户钉在顶部的那几件奖品）。
    ///
    /// 没订阅过就是**空列表**（不是错误）—— 与 `task_params()` 同一口径 ✓。
    ///
    /// # 为什么也挂在 `UserStore`
    ///
    /// 与 `settings()` / `task_params()` / `kouling_pool()` 同一个理由：
    /// 新开一个 trait 会让本仓 16 处测试替身各补一行空实现 ✗。
    ///
    /// # 为什么按 `owner` 分表（与 `kouling_pool` 相反）
    ///
    /// 口令池是**全场共享的资源**（谁抓到的口令大家都能用 ✓），而订阅是
    /// **"我想看哪几件"**—— 那是某个人自己的偏好 ✓。本仓用户资源的惯例就是
    /// 按 owner 分表（`pipelines_for(owner)` 同款）⇒ 换个人登录不该看见别人的订阅 ✗。
    async fn exchange_subscriptions(
        &self,
        _owner: &UserId,
    ) -> Result<Vec<crate::ExchangeSubscription>> {
        Ok(Vec::new())
    }

    /// 写兑换页的订阅（**整份替换** —— 切换由 `exchange::toggled` 先算好）。
    ///
    /// 默认实现**报错**（与 `save_settings` 同一口径）：写不进去却回成功，
    /// 界面会说"已订阅"而下次刷新就没了 ✗ —— 那是查都没法查的一种坏 ✓。
    async fn save_exchange_subscriptions(
        &self,
        _owner: &UserId,
        _items: &[crate::ExchangeSubscription],
    ) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持下改兑换订阅（save_exchange_subscriptions 未实现）".into(),
        ))
    }

    /// 读某个用户的**推送规则**（用户口径 2026-10-07 的第二层，见 `crate::push`）。
    ///
    /// 没建过就是**空列表**（不是错误）—— 与 `exchange_subscriptions()` 同一口径 ✓。
    ///
    /// # 为什么也挂在 `UserStore`
    ///
    /// 与 `settings()` / `task_params()` / `exchange_subscriptions()` 同一个理由：
    /// 新开一个 trait 会让本仓 16 处测试替身各补一行空实现 ✗ —— 那 16 行里
    /// 没有一行会真的用到推送规则 ✓。
    ///
    /// # 为什么按 `owner` 分表
    ///
    /// 与 `exchange_subscriptions()` 同款：规则是**某个人自己配的**（"我这条定时
    /// 只想推失败"✓），换个人登录不该看见、更不该改得动别人的规则 ✗。
    async fn push_rules(&self, _owner: &UserId) -> Result<Vec<crate::push::PushRule>> {
        Ok(Vec::new())
    }

    /// 读某个用户的**一条**推送规则。没有 ⇒ `None`（不是错误）。
    ///
    /// 单条读而不是每次把整份列表捞回来再筛：路由的 upsert / delete 都要先看
    /// 「自己的那一行在不在」✓ —— 那一趟只该读一行 ✓。
    async fn push_rule(
        &self,
        _owner: &UserId,
        _id: &crate::push::PushRuleId,
    ) -> Result<Option<crate::push::PushRule>> {
        Ok(None)
    }

    /// 存一条推送规则（**按 id 覆盖或新增**，同一 owner 下）。
    ///
    /// 默认实现**报错**（与 `save_settings` / `save_exchange_subscriptions` 同一口径）：
    /// 写不进去却回成功，界面会说"已保存"而下一次刷新就没了 ✗。
    async fn save_push_rule(&self, _owner: &UserId, _rule: &crate::push::PushRule) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持下改推送规则（save_push_rule 未实现）".into(),
        ))
    }

    /// 删一条推送规则。删不存在的**不算错**（幂等 ✓）—— 调用方多半已经先查过。
    async fn remove_push_rule(&self, _owner: &UserId, _id: &crate::push::PushRuleId) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持删推送规则（remove_push_rule 未实现）".into(),
        ))
    }

    /// 读**全部用户的**推送规则（摊平）。
    ///
    /// # 为什么它不能省（与上面几条的区别）
    ///
    /// 上面四条都带 `owner` ✓，而**事件匹配那一层没有"当前用户"这个概念** ✗：
    /// 一次定时触发、一次编排跑完，手里只有 `Schedule` / `Pipeline` 与那个**账号** ——
    /// 账号的归属是 `Account::owner`，而不是"正在看 WebUI 的人" ✓。
    /// 要按 `push_rule` 那一格把规则捞出来，就只能不带 owner 查 ✓
    /// （调用方自己按规则里的 `owner` 判归属 ✓）。
    ///
    /// ⚠ 摊平出来的每一条**自带 `owner`**（[`crate::push::PushRule::owner`] ✓）——
    /// 归属判定用那一格，不要靠"是谁查的"来推 ✗。
    async fn all_push_rules(&self) -> Result<Vec<crate::push::PushRule>> {
        Ok(Vec::new())
    }

    // ⚠ 这里原先有 `push_config()` / `save_push_config()`（**用户级**渠道配置，
    // 落盘 `data/push-config.json` ✓）—— 2026-10-08 推翻重做时**两条都删了** ✗。
    // 用户口径是「我设置一个推送规则，**这个规则配置推送的 token，包括推送哪几个
    // 平台**」✓ ⇒ "发给谁"全部来自**规则自己的 `config`** ✓，
    // "用户级渠道配置"那一层（以及它带来的"两份 token 可能对不上"✓）整个不存在了 ✓。
    // 连旧文件一起删：`data/push-config.json` 由 `JsonStore` 启动时清掉 ✓。

    /// **面板行为**的绑定（没有定时/编排可绑的那几类：登录新设备 / 登录失败 /
    /// 耗时异常 / 连续失败 ✓）。
    ///
    /// # 为什么它在 `UserStore` 上（与 `push_rules()` 同一格）
    ///
    /// 用户口径：「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的
    /// 入口」✓ —— 那是**某个用户自己**的选择 ✓（换个人登录不该看见、更不该改得动
    /// 别人的 ✓），与 `push_rules()` / `exchange_subscriptions()` 同一口径 ✓。
    /// 落盘 `data/push-panel.json`（按 owner 分表 ✓）。
    ///
    /// # 为什么没配过也要回一份默认
    ///
    /// 与 `push_rules()` 回空列表同一个道理 ✓：面板绑定的**默认是"不绑任何规则"**
    /// 但有**保守的节流默认值**（1800s 冷却 / 每日 10 条 ✓，见
    /// [`crate::push::BehaviorKey::default_throttle`] ✓）⇒ 回 `PanelBindings::default()`
    /// 而不是 `Option` ✓ —— 调用方拿到的一定是"能直接用的那份" ✓。
    async fn push_panel(&self, _owner: &UserId) -> Result<crate::push::PanelBindings> {
        Ok(crate::push::PanelBindings::default())
    }

    /// 写面板行为的绑定（**整份替换** —— 与 `save_push_rule` 同一口径 ✓）。
    ///
    /// 默认实现**报错**（与 `save_settings` / `save_push_rule` 同一口径）：
    /// 写不进去却回成功，界面会说"已保存"而下次刷新就没了 ✗。
    async fn save_push_panel(
        &self,
        _owner: &UserId,
        _bindings: &crate::push::PanelBindings,
    ) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持下改面板推送绑定（save_push_panel 未实现）".into(),
        ))
    }

    /// **面板行为的节流状态**（每行为：上次发送时刻 / 今日计数 / 上次状态 ✓）。
    ///
    /// # 为什么非得落盘（用户口径：「登录，性能，这些可能**频繁触发**，注意」✓）
    ///
    /// 只放内存 ⇒ **进程一重启，冷却与计数全归零** ⇒ 重启后同一批事件再来一轮 ✗
    /// —— 而重启本来就可能紧接着一串触发 ✓。落盘 `data/push-throttle.json` ✓。
    ///
    /// 没记过 ⇒ 全默认（`last_sent_ms: None` ⇒ 第一次一定放行 ✓）。
    async fn push_throttle(&self, _owner: &UserId) -> Result<crate::push::ThrottleState> {
        Ok(crate::push::ThrottleState::default())
    }

    /// 写节流状态（**整份替换** ✓）。
    ///
    /// 默认实现**报错**：静默失败等于"节流没生效"，而那正是要防的风暴 ✗
    /// —— 宁可让调用方看见一次错误，也不要它以为拦住了 ✓。
    async fn save_push_throttle(
        &self,
        _owner: &UserId,
        _state: &crate::push::ThrottleState,
    ) -> Result<()> {
        Err(crate::Error::Storage(
            "这个存储不支持下改推送节流状态（save_push_throttle 未实现）".into(),
        ))
    }

    /// 记下一次**见过的登录设备**；返回它**是不是第一次见** ✓
    /// （`true` = 新设备/新地点 ⇒ `loginNew` 该报 ✓）。
    ///
    /// # 为什么"判"与"记"合成一次（不是两条方法）
    ///
    /// 拆成 `login_devices()` + `note_login_device()` 的话，调用方要自己
    /// "先查后写"，而两次读之间可以插进另一次登录 —— 两台新设备会**各自以为
    /// 自己只是第二台** ✓，于是一台都报不出来 ✗（与 `create_first_user` 那条
    /// 判空/插入必须在同一把锁里、同一个教训 ✓）。
    ///
    /// # 默认实现为什么回 `false`（= **不是**新的）
    ///
    /// 本仓十几处测试替身不该为这一条各补一行 ✓（与 `push_rules()` 同一个理由 ✓）。
    /// 而"没实现"必须落在**不发**那一侧：回 `true` = 每次登录都报"新设备" ✗
    /// —— 那既是噪音，也是**骗人** ✓。
    ///
    /// 落盘 `data/login-devices.json`（按 owner 分表 ✓）。
    async fn remember_login_device(&self, _owner: &UserId, _key: &str) -> Result<bool> {
        Ok(false)
    }
}

/// 全部持久化能力的组合。
///
/// 用 trait 继承而不是把方法摊平：这样 `Arc<dyn Store>` 仍然是
/// **一个**对象，注入时不用带五个 `Arc`。
///
/// # 为什么 2026-10-03 把 [`SmidStore`] 也并了进来
///
/// 任务是**拿得到 `ctx.store` 的**（`TaskContext`），所以"读这个账号
/// 已换回来的 smid `deviceId`"本来不该再拉一条注入线。没并进来之前，
/// 任务层只能去用 `Account::device_id` —— 而那一份是**登录时冒充 PC
/// 客户端**用的，与 H5 那条链不是同一个东西：上游**校验内容**，
/// 内容错就回 `614`（`receiveV3` 领AI豆因此整笔领不到，
/// 报错文案却是"活动太火爆啦"）。见 `mcloud_tasks::signin::h5_device_id`。
pub trait Store:
    AccountStore
    + RunStore
    + LogStore
    + ScheduleStore
    + PipelineStore
    + UserStore
    + SmidStore
    + Send
    + Sync
{
}

/// 任何同时实现了全部子 trait 的类型自动成为 `Store`。
impl<T> Store for T where
    T: AccountStore
        + RunStore
        + LogStore
        + ScheduleStore
        + PipelineStore
        + UserStore
        + SmidStore
        + Send
        + Sync
{
}

// ── 时钟 ────────────────────────────────────────────────────────────────

/// 时钟抽象。测试用固定时钟，签名向量才能稳定。
pub trait Clock: Send + Sync {
    /// 当前时间，毫秒级 unix 时间戳。
    fn now_ms(&self) -> i64;

    fn now_secs(&self) -> i64 {
        self.now_ms() / 1000
    }
}

/// 东八区固定偏移 —— `timezone: -480` 就是 UTC+8 的分钟数取负。
///
/// # 为什么写死在代码里，不用系统本地时区
///
/// smid 特征体里 `timezone: -480` 是**写死的**东八区，而 `smid` 字段里的
/// `yyyyMMddHHmmss` 必须与它同源。如果时间按运行机器的本地时区算，
/// 一台设在 UTC 的服务器就会自报"我在东八区，但我的时间戳是 UTC"——
/// 这是风控侧一眼可见的不一致。
///
/// 所以"东八区"这个事实在本项目里**只有这一处定义**，需要它的地方都引用这里。
pub fn beijing_offset() -> chrono::FixedOffset {
    // UTC+8 是合法偏移，构造不会失败；真失败说明常量被改错了，那属于代码缺陷。
    chrono::FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| {
        debug_assert!(false, "东八区偏移必须合法");
        // 退化到 UTC：时间偏 8 小时总好过崩溃，而 debug 断言会先把问题暴露出来。
        chrono::FixedOffset::east_opt(0).expect("UTC 偏移必然合法")
    })
}

/// 东八区墙钟的 14 位 `yyyyMMddHHmmss`。
///
/// 见 [`beijing_offset`] 说明为什么固定用东八区。
/// 时间**只能**来自注入的 [`Clock`]，不许直接 `SystemTime::now()`——
/// 否则测试无法钉住上报内容（见 docs/conventions.md §7.2）。
pub fn beijing_time_digits(clock: &dyn Clock) -> String {
    use chrono::TimeZone as _;
    let offset = beijing_offset();
    match offset.timestamp_millis_opt(clock.now_ms()) {
        chrono::LocalResult::Single(t) => t.format("%Y%m%d%H%M%S").to_string(),
        // 东八区**没有夏令时**，所以这两个分支对本偏移不可达。
        // 真出现说明偏移常量被改成了有 DST 的地方。
        _ => String::new(),
    }
}

/// 按**给定固定偏移**把注入时钟格式化成 `fmt`（chrono 格式串）。
///
/// # 为什么需要它（2026-10-07）
///
/// 上游有两个口子要的**不是** `yyyyMMddHHmmss`，而 `chrono` 只有本 crate 引得到
/// （`mcloud-tasks` / `mcloud-upstream` 的依赖里都没有它）⇒ 任务层要拼
/// `"2026-10-07 10:45:30"` 这种串就只能自己算历法。那正是"各写一份时间换算"
/// 的老毛病（`docs/conventions.md` §8.7）：写错的表现是**一个偏 8 小时的时间戳**，
/// 而上游只会回一个含义模糊的业务码。
///
/// 于是把格式化也收在这里 —— 与 [`beijing_time_digits`] 同一个出处、同一条口径
/// （时间只能来自注入的 [`Clock`]，不许 `SystemTime::now()`）。
///
/// * [`beijing_format`] = 东八区（旧 TS 用本地 `Date` 的字段；本机时区就是东八区）
/// * [`utc_format`] = UTC（旧 TS `new Date().toISOString()` 那一类）
pub fn format_with_offset(clock: &dyn Clock, offset_secs: i32, fmt: &str) -> String {
    use chrono::TimeZone as _;
    let offset = chrono::FixedOffset::east_opt(offset_secs).unwrap_or_else(beijing_offset);
    match offset.timestamp_millis_opt(clock.now_ms()) {
        chrono::LocalResult::Single(t) => t.format(fmt).to_string(),
        _ => String::new(),
    }
}

/// 东八区墙钟按 `fmt` 格式化（见 [`format_with_offset`]）。
pub fn beijing_format(clock: &dyn Clock, fmt: &str) -> String {
    format_with_offset(clock, 8 * 3600, fmt)
}

/// UTC 墙钟按 `fmt` 格式化（见 [`format_with_offset`]）。
pub fn utc_format(clock: &dyn Clock, fmt: &str) -> String {
    format_with_offset(clock, 0, fmt)
}

/// 把一个**不带时区**的本地时刻串按**东八区**解成 epoch 毫秒。///
/// 解不出来回 `None`（`None` ≠ "没有边界"，调用方自己按三态处置）。
///
/// # 为什么在这里，而不是各任务层各写一份
///
/// 上游那些活动页给的是 `"2025-05-28T00:00:00"` 这种**本地墙钟**串，
/// 判"活动在不在期"必须按东八区解释 —— 按 UTC 解会把边界**整体推后 8 小时**
/// （快到 `endTime` 时多发一次写请求，快到 `startTime` 时少发一次）。
///
/// [`beijing_offset`] 已经是"东八区"在本项目的**唯一**定义，而 `chrono`
/// 只有本 crate 引得到（`mcloud-tasks` / `mcloud-upstream` 的依赖里都没有它）
/// ⇒ 日期算术与偏移一起放这里，别处就不会再出现第二份"自己拆年月日"的实现。
///
/// ⚠ `mcloud-tasks::exchange` 里那份**手写**的解析器（`parse_exchange_time`）
/// 早于本函数，形态也不同（它那边时分秒之间是 `-`）。它**仍然有效**，
/// 且已用一个"与 [`beijing_offset`] 对拍"的用例钉住偏移来源一致 ——
/// 不要因为本函数的出现就顺手去改它（改了是白担一次回归风险）。
///
/// # `fmt` 用 chrono 的格式串
///
/// 例：`"%Y-%m-%dT%H:%M:%S"`。上游形态变了就把 `fmt` 改对，
/// **不要**在这里"兼容多种形态" —— 那会让"上游换了格式"这件事静默消失。
pub fn parse_beijing_time(raw: &str, fmt: &str) -> Option<i64> {
    use chrono::TimeZone as _;
    let naive = chrono::NaiveDateTime::parse_from_str(raw.trim(), fmt).ok()?;
    // 固定偏移下不存在歧义，`single()` 就够了（`ambiguous` 只会出现在有 DST 的时区）。
    beijing_offset()
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp_millis())
}

// ── 事件 ────────────────────────────────────────────────────────────────

/// 事件接收端。
///
/// 实现方负责扇出：写日志、推 SSE、按级别过滤后推送通知。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: RunEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只实现 `request` 的假客户端：证明 [`HttpClient::request_raw`] 的默认实现
    /// 就是「走 `request()` 再取字节」。
    struct TextOnly;

    #[async_trait]
    impl HttpClient for TextOnly {
        async fn request(&self, _req: HttpRequest) -> Result<HttpResponse> {
            Ok(HttpResponse {
                status: 200,
                body: "文本体".into(),
                headers: Vec::new(),
            })
        }
    }

    /// 默认实现 = `request()` 交回的体的字节（纯文本请求走它没问题）。
    ///
    /// ⚠ 二进制**不能**指望默认实现：这条路上的 `HttpResponse.body` 已经过
    /// 有损 UTF-8，`into_bytes()` 只是把被毁的内容再编码一次。真发请求的实现
    /// （`ReqwestClient`）必须覆写它去拿 `.bytes()`。
    #[tokio::test]
    async fn the_default_request_raw_is_the_request_body_bytes() {
        let got = TextOnly
            .request_raw(HttpRequest::get("https://example.com/x"))
            .await
            .unwrap();
        assert_eq!(got, "文本体".as_bytes());
    }
}
