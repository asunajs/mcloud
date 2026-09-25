// run 内产物账本：这次运行在**用户网盘**上造出来的东西，逐条记下、逐条销账。
//
// # 为什么要有这一层（它解决的不是"协议"问题）
//
// 有一批能力会真的往用户网盘写东西：任务拓展凑备份（`hcy/file/create` → `PUT` →
// `file/complete`）、AI 头像上传合成图、`434` 建外链、以及 `after` 那一步的删除
// （`hcy/recyclebin/batchTrash`）。旧项目对这些东西的处理是
// **一个跨任务累积的数组** `ctx.state.files`：谁上传谁 `push`，`afterTask` 末段整批删。
//
// 这个形状有两类失效，而且都不是编译期能看见的：
//
//   - **漏记** —— 某条链中途 `return`（或压根忘了 push），文件就永久留在用户网盘上，
//     界面上没有任何一行字说明"这里留了东西"。这是这一层要防的**主要**失效。
//   - **错记** —— 把"本来就在用户网盘上的东西"当成自己的产物登记进去。
//     它比漏记严重得多：清理动作是**真删**，而且删的是用户自己的文件。
//
// ⚠ 关于"错删"要说准确（2026-09-19 用户订正过一轮，别照着旧项目的说法推）：
// 上游**没有**"这个文件是谁创建的"这种归属信息可查，而 `batchTrash` 用的是
// **唯一的 `fileId`** ⇒ 拿 A 账号的凭据去删 B 账号的 ID，表现是上游拒绝、东西还在，
// **不会**删错成另一个人的文件。真正的错删只有一条路：
// **自己把 `exist:true`（秒传 / `fileRenameMode:"refuse"` 撞上已有目录）的结果登记进去**，
// 然后按 ID 删掉一个原本就属于用户的对象。所以本文件把那条判断做成
// [ArtifactLedger.RegisterCreated] 的一个入参，而不是留给每个调用方自己记得。
//
// # 为什么账本必须带账号
//
// 清理用的是**该账号自己的**凭据（`Authorization` 里就是那台账号的 `authToken`）。
// 一次 run 可以跨多个账号（并行/串行都有），只记一串 ID 的账本会要求调用方
// 自己再分一次组 —— 而"分组分错了"在上游只是 401/拒绝，看起来像"没删掉"，
// 于是又退回漏记那一类。分组放在这里，调用方拿到的就是"按账号切好的清单"。
//
// # 分层：这一层是纯的，**不碰清理本身**
//
// `mcloudexec` 属纯层（禁止 IO，`scripts/check-go-deps.js` 断言）。
// 所以这里只有"记账 / 销账 / 取待清理清单"，**没有**任何 `batchTrash` 调用。
//
// ⚠ 2026-09-25 之后的实况（别照本文件旧版那句话推断）：`batchTrash` **已经有人发了** ——
// 发送点在 app 层的 `internal/app/task_matrix_run.go`（`cleanupCloudFiles`，链尾一次删掉
// 这一趟自己传的文件），但那条路**不经过本层**：它用的是这一次 `RunAccount` 内自己攒的
// 那份 id 列表。于是本层的账本**至今仍是零调用点**，缺口不是"清理没实现"，而是：
//
//   - [RunAccount] 那一路拿不到 `core.RunID`（账本以 runId 为键）⇒ 接不上；
//   - 跨链累积（`ai-avatar` 与 `play-ai` 也写文件、也拿不到 runId）正需要这一层，
//     而那两条链今天不删。
//
// 怎么补（两种形状，都要动组装根的签名）：要么把 `RunID` 一路带到 app 层的链入口，
// 要么把账本改成按"账号 + 一次运行句柄"键。这条选择登记为待拍板（见
// `docs/pending-decisions.md`），**不要**在本层猜一个键出来。
//
// # 并发
//
// 与 [RunRegistry] / [EventBus] 同理由：账本是**进程级共享状态**，跑任务的那条路径
// 一边写、界面与清理那条路径一边读，所以每个方法短暂加锁，指针接收者 + 构造函数。
// ⚠ 零值 [ArtifactLedger] **不可用**（内层 map 是 nil，写会 panic），必须走
// [NewArtifactLedger] —— 与包内另两个类型一致，`TestZeroValueTypesAreUnusable` 那类断言覆盖它。
package mcloudexec

import (
	"sort"
	"sync"

	"github.com/catlair/mcloud/internal/core"
)

// ArtifactKind 是产物类型。
//
// 用**开放字符串**而不是 [AdmissionKind] 那种整型枚举：这里没有"穷尽所有分支"的判据
// —— 新增一种产物（外链、笔记、相册快照）时**不该**要求所有读侧 switch 出新码，
// 那种地方漏一个分支的表现是"未知类型被当成已知类型处理"。
// 反之 [ArtifactKind.Valid] 只管一件确定的事：空串不是类型。
type ArtifactKind string

const (
	// ArtifactCloudFile 是网盘侧对象：文件或目录都算（`hcy/file/create` 两种都回 `fileId`，
	// 而清理走同一个 `batchTrash`）。
	//
	// ⚠ 目录**不递归**删这件事不在本层判：上游 `batchTrash` 对非空目录的行为**零样本**，
	// 那一句要写在真正发请求的那一层，写在这里就成了没有样本的断言。
	ArtifactCloudFile ArtifactKind = "cloud-file"
)

// Valid 报告类型是否可用（只判空串）。
//
// 刻意**不**判"是不是已知类型之一"：那样每加一种产物都要回来改这里，
// 而漏改的表现是"新类型进不来"——比"空类型进来了"安全得多，但也确实会挡人。
func (k ArtifactKind) Valid() bool { return k != "" }

// Artifact 是一条产物登记：某个账号名下、上游给的那个唯一 ID。
//
// ⚠ `ID` 是**上游侧的 fileId**，不是路径也不是文件名。旧实现里 `ai-avatar` 那种
// "按文件名匹配找回 ID"的写法（`docs/business-feature-inventory.md` §2.3）在这里
// 从根上不成立：账本只接受已经拿到 ID 之后的登记，"按名字捞"发生在调用方拿到 ID 之前，
// 那条链的修复是"改成从上传响应里读 ID"，不是往这里塞名字。
//
// ⚠ 这个结构体**不进 JSON**（纯内存账本；对外表达由 app 层另写带 tag 的响应体），
// 所以它登记在 `scripts/check-go-jsontags.cjs` 的白名单里。
type Artifact struct {
	// Kind 是产物类型，见 [ArtifactKind]。
	Kind ArtifactKind
	// Account 是"用谁的凭据能删掉它"。
	Account core.AccountID
	// ID 是上游侧唯一标识（网盘对象即 `fileId`）。
	ID string
}

// Valid 报告这条登记是否完整。
//
// 三个字段少任何一个都不许入账，理由各不相同但都指向"漏记"：
// 少了 `ID` 销不掉（清理时手里没东西）、少了 `Account` 不知道用谁的凭据、
// 少了 `Kind` 则将来按类型分流清理时会被静默跳过。
func (a Artifact) Valid() bool {
	return a.Kind.Valid() && a.Account != "" && a.ID != ""
}

// String 返回诊断文本。
//
// 这里**只**打类型与 ID，账号用 [core.AccountID] 原样（它本身就是掩码号，见
// `docs/sanitize-guide.md`）——不打任何凭据。账本条目会进日志与 run 事件，
// 一句"顺手带上 authToken 方便排查"就能把凭据写进日志。
func (a Artifact) String() string {
	return string(a.Kind) + "#" + a.Account.String() + "/" + a.ID
}

// ledgerRun 是一个 run 名下的登记集合。
//
// 用 `map[Artifact]struct{}` 而不是切片：登记天然**幂等**（重试同一次上传会拿到同一个
// ID，或至少同一条要记两次），切片会让"记了几条"与"有几个东西要清"这两个数分开，
// 而后者才是清理该看的数。
type ledgerRun struct {
	pending map[Artifact]struct{}
}

// ArtifactLedger 是 run → 待清理产物的账本。
type ArtifactLedger struct {
	mu   sync.Mutex
	runs map[core.RunID]*ledgerRun
}

// NewArtifactLedger 建一个空账本。
//
// ⚠ 零值不可用，见包文件头。
func NewArtifactLedger() *ArtifactLedger {
	return &ArtifactLedger{runs: map[core.RunID]*ledgerRun{}}
}

// Register 登记一条产物。
//
// 返回 `error` 而不是 `bool`：这里唯一的失败原因是**调用方自己写错了**
// （登记不全 = 一定会有东西留在用户网盘上），这种事必须响，不能退化成
// "静默没记上、以后也就没人去删"。
//
// 重复登记同一条返回 `nil`（幂等），计数不会变两番。
func (l *ArtifactLedger) Register(runID core.RunID, art Artifact) error {
	if runID == "" {
		return core.NewBadRequest("产物登记缺 runId：这条产物将来没有清理的归属")
	}
	if !art.Valid() {
		return core.NewBadRequest("产物登记不完整（Kind / Account / ID 三者都得有）：" + art.String())
	}

	l.mu.Lock()
	defer l.mu.Unlock()

	run, ok := l.runs[runID]
	if !ok {
		run = &ledgerRun{pending: map[Artifact]struct{}{}}
		l.runs[runID] = run
	}
	run.pending[art] = struct{}{}
	return nil
}

// RegisterCreated 只在"这东西是本 run 造出来的"时登记。
//
// `preExisting` 取自上游响应里"原本就有"那一格（网盘侧就是 `hcy/file/create` 的
// `data.exist`：秒传命中、或 `fileRenameMode:"refuse"` 撞上同名目录时为 `true`）。
//
// 返回 `added` —— 让调用方能区分"记上了"与"因为本来就存在所以没记"，
// 这两件事在界面上不是一句话：前者进"本 run 产物"，后者**恰恰不能**进（清理会删用户的原文件）。
//
// # 为什么把这条判断做成入参
//
// 它是"错删"那一类唯一真实的成因，而每个调用方都要判一次。散在各处判，
// 漏一处就是一句"上传成功"后面跟着一次误删。集中到一个函数里，
// 才能用一条用例钉住"把 `preExisting` 当成 `false` 用会红"。
func (l *ArtifactLedger) RegisterCreated(runID core.RunID, art Artifact, preExisting bool) (bool, error) {
	if preExisting {
		return false, nil
	}
	if err := l.Register(runID, art); err != nil {
		return false, err
	}
	return true, nil
}

// Pending 返回该 run 名下仍未清理的产物，按「账号 → 类型 → ID」**字典序**。
//
// ⚠ 排序是必须的，不是装饰：内层是 map，Go 的遍历顺序**故意随机化**。
// 不排序的话同一份账两次 `Pending` 会给出不同顺序，于是
// ① 清理日志逐行 diff 永远在变；② 测试里"清单长什么样"根本钉不住。
// 同一条坑见 [ParamsDigest] 的注释。
func (l *ArtifactLedger) Pending(runID core.RunID) []Artifact {
	l.mu.Lock()
	defer l.mu.Unlock()

	run, ok := l.runs[runID]
	if !ok {
		return nil
	}
	return sortedArtifacts(run.pending)
}

// PendingFor 同 [ArtifactLedger.Pending]，但只给某个账号名下的那些
// （清理就是要按账号打，凭据不是同一串）。
func (l *ArtifactLedger) PendingFor(runID core.RunID, account core.AccountID) []Artifact {
	l.mu.Lock()
	defer l.mu.Unlock()

	run, ok := l.runs[runID]
	if !ok {
		return nil
	}
	filtered := map[Artifact]struct{}{}
	for art := range run.pending {
		if art.Account == account {
			filtered[art] = struct{}{}
		}
	}
	return sortedArtifacts(filtered)
}

// MarkRemoved 销账：这条产物已经确认清掉了。
//
// 返回 `false` 表示"账本里没有这一条"。它**不是** `error`，因为合法地会走到：
// 清理重试第二次时第一条已经销了。但调用方不能把 `false` 当"删好了" ——
// 更可能的解释是手里的三元组和当初登记的那条不一致（[Artifact] 三个字段都得对上），
// 该报出来。
func (l *ArtifactLedger) MarkRemoved(runID core.RunID, art Artifact) bool {
	l.mu.Lock()
	defer l.mu.Unlock()

	run, ok := l.runs[runID]
	if !ok {
		return false
	}
	if _, existed := run.pending[art]; !existed {
		return false
	}
	delete(run.pending, art)
	return true
}

// Count 返回该 run 名下待清理的条数。
func (l *ArtifactLedger) Count(runID core.RunID) int {
	l.mu.Lock()
	defer l.mu.Unlock()

	run, ok := l.runs[runID]
	if !ok {
		return 0
	}
	return len(run.pending)
}

// Forget 停止跟踪该 run，并**把它名下还没清的东西交回调用方**。
//
// ⚠ 返回值不是"顺便给一下"：账本消失不等于用户网盘上的文件消失。
// 旧项目 `ctx.state.files` 的外溢与漏清，本质都是"没人负责这批东西"。
// 所以这里刻意不留一个无返回值的 `Forget` —— 让调用方在类型上就绕不开"剩下这些谁管"。
// 空清单返回 `nil`，此时可以不管。
//
// 什么时候才该调它：清理已经跑完（`Count` 归零）、或明确决定把这批东西**交给别人**
// （比如落库成一条"待人工清理"的记录）。
func (l *ArtifactLedger) Forget(runID core.RunID) []Artifact {
	l.mu.Lock()
	defer l.mu.Unlock()

	run, ok := l.runs[runID]
	if !ok {
		return nil
	}
	delete(l.runs, runID)
	if len(run.pending) == 0 {
		return nil
	}
	return sortedArtifacts(run.pending)
}

// Runs 返回仍被跟踪的 runId（字典序）。
//
// 给诊断与"进程里还有几笔没清"的读数用；它不该出现在正常清理路径上
// （正常路径总是先有 runId）。
func (l *ArtifactLedger) Runs() []core.RunID {
	l.mu.Lock()
	defer l.mu.Unlock()

	ids := make([]core.RunID, 0, len(l.runs))
	for id := range l.runs {
		ids = append(ids, id)
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	return ids
}

// sortedArtifacts 把集合摊平成确定性顺序的切片。
func sortedArtifacts(set map[Artifact]struct{}) []Artifact {
	out := make([]Artifact, 0, len(set))
	for art := range set {
		out = append(out, art)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Account != out[j].Account {
			return out[i].Account < out[j].Account
		}
		if out[i].Kind != out[j].Kind {
			return out[i].Kind < out[j].Kind
		}
		return out[i].ID < out[j].ID
	})
	return out
}
