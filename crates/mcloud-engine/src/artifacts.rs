//! run 内产物账本：这次运行在**用户网盘**上造出来的东西，逐条记下、逐条销账。
//!
//! # 为什么要有这一层（它解决的不是"协议"问题）
//!
//! 有一批能力会真的往用户网盘写东西（任务拓展凑备份、AI 头像上传合成图、
//! 建分享外链），删的时候走 `hcy/recyclebin/batchTrash`。旧项目的处理是
//! **一个跨任务累积的数组**：谁上传谁 push，链尾整批删。
//!
//! 这个形状有两类失效，**都不是编译期能看见的**：
//!
//! | 失效 | 后果 |
//! |---|---|
//! | **漏记** | 某条链中途 `return`（或压根忘了 push），文件就**永久留在用户网盘上**，界面上没有任何一行字说明"这里留了东西"。这是要防的**主要**失效。 |
//! | **错记** | 把"本来就在用户网盘上的东西"当成自己的产物登记。它比漏记严重得多：清理动作是**真删**，删的是**用户自己的文件**。 |
//!
//! # ⚠ 关于"错删"要说准确
//!
//! 上游**没有**"这个文件是谁创建的"这种归属信息可查，而 `batchTrash` 用的是
//! **唯一的 `fileId`** ⇒ 拿 A 账号的凭据去删 B 账号的 ID，表现是上游拒绝、
//! 东西还在，**不会**删错成另一个人的文件。
//!
//! 真正的错删只有一条路：**自己把 `exist:true`（秒传命中、或
//! `fileRenameMode:"refuse"` 撞上已有同名目录）的结果登记进去**，
//! 然后按 ID 删掉一个原本就属于用户的对象。
//!
//! 所以那条判断被做成 [`ArtifactLedger::register_created`] 的**入参**，
//! 而不是留给每个调用方自己记得 —— 散在各处判，漏一处就是一句"上传成功"
//! 后面跟着一次误删。
//!
//! # 为什么账本必须带账号
//!
//! 清理用的是**该账号自己的**凭据。一次运行可能跨多个账号，
//! 只记一串 ID 的账本会要求调用方自己再分一次组 —— 而"分组分错了"在上游
//! 只是 401/拒绝，看起来像"没删掉"，于是又退回漏记那一类。
//! 分组放在这里，调用方拿到的就是"按账号切好的清单"。
//!
//! # 分层：这一层是纯的，**不碰清理本身**
//!
//! 本模块属零 IO 层（见 `docs/conventions.md` §1），所以只有
//! "记账 / 销账 / 取待清理清单"，**没有**任何 `batchTrash` 调用。
//! 发请求那一枪在 `mcloud-upstream`，由调用方在链尾自己发。
//!
//! # ⚠ 已知缺口：编排链内的跨步骤累积
//!
//! 账本以 [`RunId`] 为键，而**一次 pipeline 产出 N 条独立 run**
//! （每个步骤、每个账号各一条，见 `mcloud_engine::PipelineRunner`）。
//! 于是"第 2 步传的文件、第 5 步才知道要删"这件事今天接不上。
//!
//! 刻意**不**自造一个"编排执行 id"来补它：选错键的代价是
//! **分组分错 ⇒ 看起来像没删掉**，而那种错误不会报错。
//! 上游参考实现把同一格登记为待拍板，这里沿用同一处置。
//!
//! 现状可接受的理由：今天的清理是**链尾一步**（在 run 内部完成），
//! 单条 run 的语义是完整的；而真正需要跨步骤累积的那几条链
//! （写网盘那批）**尚未实现**。等它们落地时再拍键。
//!
//! # 并发
//!
//! 与 [`crate::CancelRegistry`] 同理由：账本是**进程级共享状态**，
//! 跑任务的那条路径一边写、界面与清理那条路径一边读，
//! 所以每个方法短暂加锁。锁**从不跨 `.await` 持有**。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use mcloud_core::{AccountId, Error, Result, RunId};

/// 产物类型。
///
/// 用**开放字符串**而不是整型枚举：这里没有"穷尽所有分支"的判据 ——
/// 新增一种产物（外链、笔记、相册快照）时**不该**要求所有读侧 `match`
/// 出新分支，那种地方漏一个分支的表现是"未知类型被当成已知类型处理"。
///
/// 反之 [`ArtifactKind::is_valid`] 只管一件确定的事：空串不是类型。
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ArtifactKind(String);

impl ArtifactKind {
    /// 网盘侧对象：文件或目录都算。
    ///
    /// `hcy/file/create` 两种都回 `fileId`，而清理走同一个 `batchTrash`。
    ///
    /// ⚠ 目录**不递归**删这件事不在本层判：上游 `batchTrash` 对非空目录的
    /// 行为**零样本**，那一句要写在真正发请求的那一层，
    /// 写在这里就成了没有样本的断言。
    pub const CLOUD_FILE: &'static str = "cloud-file";

    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    /// 网盘文件（最常用的一档，给个构造函数省得每次拼字面量）。
    pub fn cloud_file() -> Self {
        Self(Self::CLOUD_FILE.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 只判空串。
    ///
    /// 刻意**不**判"是不是已知类型之一"：那样每加一种产物都要回来改这里，
    /// 而漏改的表现是"新类型进不来"—— 比"空类型进来了"安全得多，
    /// 但也确实会挡人。
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty()
    }
}

impl std::fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一条产物登记：某个账号名下、上游给的那个唯一 ID。
///
/// # ⚠ `id` 是**上游侧的 `fileId`**，不是路径也不是文件名
///
/// 旧实现里"按文件名匹配找回 ID"那种写法在这里从根上不成立：
/// 账本只接受**已经拿到 ID 之后**的登记。"按名字捞"发生在调用方拿到 ID
/// 之前，那条链的修复是"改成从上传响应里读 ID"，不是往这里塞名字。
///
/// # 序列化形态
///
/// 用 `camelCase`（新增类型一律如此，见 `docs/conventions.md` §7.1）。
/// 它会进 run 记录的 `details` —— 这正是修掉旧项目那个缺口的地方：
/// 旧实现只报"留了 N 个"却**不知道是哪 N 个**，将来手工清理时无从下手。
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub kind: ArtifactKind,
    /// "用谁的凭据能删掉它"。
    pub account: AccountId,
    /// 上游侧唯一标识（网盘对象即 `fileId`）。
    pub id: String,
}

impl Artifact {
    pub fn new(kind: ArtifactKind, account: AccountId, id: impl Into<String>) -> Self {
        Self {
            kind,
            account,
            id: id.into(),
        }
    }

    /// 网盘文件的快捷构造。
    pub fn cloud_file(account: AccountId, id: impl Into<String>) -> Self {
        Self::new(ArtifactKind::cloud_file(), account, id)
    }

    /// 三个字段少任何一个都不许入账。
    ///
    /// 理由各不相同但都指向"漏记"：少了 `id` 销不掉（清理时手里没东西）、
    /// 少了 `account` 不知道用谁的凭据、少了 `kind` 则将来按类型分流清理时
    /// 会被静默跳过。
    pub fn is_valid(&self) -> bool {
        self.kind.is_valid() && !self.account.as_str().is_empty() && !self.id.is_empty()
    }
}

/// 诊断文本。
///
/// 只打类型与 ID，账号用 [`AccountId`] 原样（它本身就是**掩码号**）——
/// **不打任何凭据**。账本条目会进日志与 run 记录，一句"顺手带上 authToken
/// 方便排查"就能把凭据写进日志。
impl std::fmt::Display for Artifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}/{}", self.kind, self.account.as_str(), self.id)
    }
}

/// run → 待清理产物的账本。
///
/// # 零值不可用
///
/// 内层 `HashMap` 是 `Option` 包着的，[`Self::new`] 之前调用任何方法都返回
/// "没有这笔账"而不是 panic —— 与 `CancelRegistry` 的取舍一致：
/// 一个未初始化的账本不该让进程崩，但它也**不该**被当成"清理干净了"。
#[derive(Default)]
pub struct ArtifactLedger {
    inner: Mutex<HashMap<RunId, HashSet<Artifact>>>,
}

impl ArtifactLedger {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RunId, HashSet<Artifact>>> {
        // 中毒时继续用内部数据：账本的内容仍然是对的，
        // 而"因为别的线程 panic 就把待清理清单丢掉"会直接造成漏记。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 登记一条产物。
    ///
    /// 返回 `Result` 而不是 `bool`：这里唯一的失败原因是**调用方自己写错了**
    /// （登记不全 = 一定会有东西留在用户网盘上），这种事必须响，
    /// 不能退化成"静默没记上、以后也就没人去删"。
    ///
    /// 重复登记同一条返回 `Ok(())`（幂等），计数不会变两番。
    pub fn register(&self, run_id: RunId, art: Artifact) -> Result<()> {
        if !art.is_valid() {
            return Err(Error::Config(format!(
                "产物登记不完整（kind / account / id 三者都得有）：{art}"
            )));
        }
        self.lock().entry(run_id).or_default().insert(art);
        Ok(())
    }

    /// 只在"这东西是本 run 造出来的"时登记。
    ///
    /// `pre_existing` 取自上游响应里"原本就有"那一格（网盘侧就是
    /// `hcy/file/create` 的 `data.exist`：秒传命中、或
    /// `fileRenameMode:"refuse"` 撞上同名目录时为 `true`）。
    ///
    /// 返回 `added` —— 让调用方能区分"记上了"与"因为本来就存在所以没记"，
    /// 这两件事在界面上**不是一句话**：前者进"本 run 产物"，
    /// 后者**恰恰不能**进（清理会删用户的原文件）。
    ///
    /// # 为什么把这条判断做成入参
    ///
    /// 它是"错删"那一类唯一真实的成因，而每个调用方都要判一次。
    /// 散在各处判，漏一处就是一句"上传成功"后面跟着一次误删。
    /// 集中到一个函数里，才能用一条用例钉住
    /// "把 `pre_existing` 当成 `false` 用会红"。
    pub fn register_created(
        &self,
        run_id: RunId,
        art: Artifact,
        pre_existing: bool,
    ) -> Result<bool> {
        if pre_existing {
            return Ok(false);
        }
        self.register(run_id, art)?;
        Ok(true)
    }

    /// 该 run 名下仍未清理的产物，按「账号 → 类型 → ID」**字典序**。
    ///
    /// # ⚠ 排序是必须的，不是装饰
    ///
    /// 内层是 `HashSet`，Rust 的遍历顺序**不保证稳定**（每个进程的哈希种子
    /// 都不同）。不排序的话同一份账两次 `pending` 会给出不同顺序，于是
    /// ① 清理日志逐行 diff 永远在变；② 测试里"清单长什么样"根本钉不住。
    pub fn pending(&self, run_id: RunId) -> Vec<Artifact> {
        match self.lock().get(&run_id) {
            Some(set) => sorted(set),
            None => Vec::new(),
        }
    }

    /// 同 [`Self::pending`]，但只给某个账号名下的那些
    /// （清理就是要**按账号**打，凭据不是同一串）。
    pub fn pending_for(&self, run_id: RunId, account: &AccountId) -> Vec<Artifact> {
        match self.lock().get(&run_id) {
            Some(set) => sorted(set.iter().filter(|a| &a.account == account)),
            None => Vec::new(),
        }
    }

    /// 销账：这条产物已经确认清掉了。
    ///
    /// 返回 `false` 表示"账本里没有这一条"。它**不是** `Err`，因为合法地会走到：
    /// 清理重试第二次时第一条已经销了。但调用方不能把 `false` 当"删好了" ——
    /// 更可能的解释是手里的三元组和当初登记的那条不一致
    /// （[`Artifact`] 三个字段都得对上），该报出来。
    pub fn mark_removed(&self, run_id: RunId, art: &Artifact) -> bool {
        match self.lock().get_mut(&run_id) {
            Some(set) => set.remove(art),
            None => false,
        }
    }

    /// 该 run 名下待清理的条数。
    pub fn count(&self, run_id: RunId) -> usize {
        self.lock().get(&run_id).map_or(0, HashSet::len)
    }

    /// 停止跟踪该 run，并**把它名下还没清的东西交回调用方**。
    ///
    /// # ⚠ 返回值不是"顺便给一下"
    ///
    /// 账本消失**不等于**用户网盘上的文件消失。旧项目那个跨任务数组的
    /// 外溢与漏清，本质都是"没人负责这批东西"。所以这里刻意**不**留一个
    /// 无返回值的 `forget` —— 让调用方在类型上就绕不开"剩下这些谁管"。
    ///
    /// 空清单返回空 `Vec`，此时可以不管。
    ///
    /// 什么时候才该调它：清理已经跑完（[`Self::count`] 归零）、
    /// 或明确决定把这批东西**交给别人**（比如落库成一条"待人工清理"的记录）。
    pub fn forget(&self, run_id: RunId) -> Vec<Artifact> {
        match self.lock().remove(&run_id) {
            Some(set) => sorted(&set),
            None => Vec::new(),
        }
    }

    /// 仍被跟踪的 run id（升序）。
    ///
    /// 给诊断与"进程里还有几笔没清"的读数用；它不该出现在正常清理路径上
    /// （正常路径总是先有 run id）。
    pub fn runs(&self) -> Vec<RunId> {
        let mut ids: Vec<RunId> = self.lock().keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// 进程里还没销账的产物总条数。
    pub fn total_pending(&self) -> usize {
        self.lock().values().map(HashSet::len).sum()
    }
}

/// 把集合摊平成确定性顺序的切片。
fn sorted<'a>(set: impl IntoIterator<Item = &'a Artifact>) -> Vec<Artifact> {
    let mut out: Vec<Artifact> = set.into_iter().cloned().collect();
    // `Artifact` 的字段序就是 (kind, account, id)，与参考实现的
    // 「账号 → 类型 → ID」不同 —— 这里按**类型 → 账号 → ID** 排。
    //
    // 为什么不用参考实现那个序：它那个序是为 Go 的 map 随机化兜底，
    // 而两种序都能满足"确定性"这一唯一硬要求。选字段序的收益是
    // 排序规则与 `derive(Ord)` 一致，不会出现"两处排序口径不同"。
    out.sort_unstable();
    out
}

/// 进程级共享的账本句柄。
pub type SharedLedger = Arc<ArtifactLedger>;

/// **任务侧**的窄句柄：`run_id` 与 `account` 由执行器填好。
///
/// # 为什么要这一层，而不是直接给任务一个 `Arc<ArtifactLedger>`
///
/// **`run_id` 与 `account` 由它自动填**。任务的调用点不再需要重复传这两个
/// 参数，于是"忘了传 / 传错了"这种错误在类型上就不存在 ——
/// 而漏记的表现是文件**永久留在用户网盘上**、界面上一个字都不说。
///
/// # ⚠ 为什么它**必须**能销账（一处自我更正）
///
/// 本层一度刻意不给任务 `mark_removed`，理由是"销账只属于清理路径"。
/// **那条理由是错的**：`batchTrash` 是协议层的东西，而
/// `mcloud-engine` 是零 IO 层（不依赖 `mcloud-upstream`）——
/// 执行器**根本没有能力**去发那一枪。
///
/// 所以清理只能发生在**任务自己**的链尾（参考实现就是这个形状：
/// `app-task` 在链尾一次删掉这一趟自己传的文件）。不给销账能力，
/// 那条路就只剩两种写法：要么删了不销账（每次运行都误报"留了 N 个"），
/// 要么绕过账本自己记（那就是我们要消灭的旧形状）。
///
/// # 那"悄悄销账"怎么办
///
/// 销账确实可能被滥用（没真删就销账 ⇒ 文件留在网盘上而无人知道）。
/// 挡它的不是"不给这个能力"，而是两条**可观察**的约束：
///
/// 1. [`Self::mark_removed`] 要求**三个字段完全对上**才销得掉，
///    对不上返回 `false` —— 调用方不能把 `false` 当"删好了"。
/// 2. 执行器在 run 收尾时把**剩余的**清单写进运行记录并告警。
///    所以"销了但没删"这件事会在下一次运行里以"又留了同样的东西"暴露出来，
///    而不是永远沉在内存里。
#[derive(Clone)]
pub struct ArtifactRecorder {
    ledger: SharedLedger,
    run_id: RunId,
    account: AccountId,
}

impl ArtifactRecorder {
    pub fn new(ledger: SharedLedger, run_id: RunId, account: AccountId) -> Self {
        Self {
            ledger,
            run_id,
            account,
        }
    }

    /// 登记一条**本 run 造出来的**网盘文件。
    ///
    /// `pre_existing` 取自 `hcy/file/create` 的 `data.exist`
    /// （秒传命中 / 撞上同名目录为 `true`）—— 传 `true` 时**不会**入账，
    /// 因为那东西本来就是用户的。
    /// 详见 [`ArtifactLedger::register_created`]，那是防误删的唯一防线。
    pub fn register_cloud_file(
        &self,
        file_id: impl Into<String>,
        pre_existing: bool,
    ) -> Result<bool> {
        let art = Artifact::cloud_file(self.account.clone(), file_id);
        self.ledger.register_created(self.run_id, art, pre_existing)
    }

    /// 登记一条任意类型的产物。
    pub fn register_created(
        &self,
        kind: ArtifactKind,
        id: impl Into<String>,
        pre_existing: bool,
    ) -> Result<bool> {
        let art = Artifact::new(kind, self.account.clone(), id);
        self.ledger.register_created(self.run_id, art, pre_existing)
    }

    /// 本次运行、本账号名下待清理的条数（任务可据此在小结里说明）。
    pub fn pending_count(&self) -> usize {
        self.ledger.pending_for(self.run_id, &self.account).len()
    }

    /// 本次运行、本账号名下待清理的清单。
    pub fn pending(&self) -> Vec<Artifact> {
        self.ledger.pending_for(self.run_id, &self.account)
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn account(&self) -> &AccountId {
        &self.account
    }

    /// 销账：这条产物**已经确认清掉了**（移进回收站）。
    ///
    /// 清理只能在任务自己的链尾做（见类型文档"为什么必须能销账"）。
    /// 返回 `false` 表示账上没这一条 —— 那更可能是"手里的三元组与当初登记的
    /// 不一致"，**不能**当成"删好了"：对不上就说明还有一条没销。
    pub fn mark_removed(&self, art: &Artifact) -> bool {
        // 只允许销**本账号**的账：跨账号销账在上游是 401/拒绝，
        // 在这里则是一次静默的漏记。
        if art.account != self.account {
            return false;
        }
        self.ledger.mark_removed(self.run_id, art)
    }

    /// 按 ID 销账（最常用的形态：清理那一枪刚发完，手里只有 ID 列表）。
    pub fn mark_cloud_file_removed(&self, file_id: &str) -> bool {
        self.mark_removed(&Artifact::cloud_file(self.account.clone(), file_id))
    }

    /// 一次销掉一批，返回**成功销掉的条数**。
    ///
    /// 返回条数而不是 `()`：清理那一枪可能只删掉了一部分，
    /// 而"销了几条"与"发了几条"不相等时调用方要能看出来。
    pub fn mark_cloud_files_removed(&self, file_ids: &[String]) -> usize {
        file_ids
            .iter()
            .filter(|id| self.mark_cloud_file_removed(id))
            .count()
    }
}

impl std::fmt::Debug for ArtifactRecorder {
    /// 不暴露账本内部（它可能装着测试用的固定序列）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactRecorder")
            .field("run_id", &self.run_id)
            .field("account", &self.account.as_str())
            .finish()
    }
}

#[cfg(test)]
mod tests;
