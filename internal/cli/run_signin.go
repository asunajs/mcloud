package cli

// `mcloud signin` 的**编排层**：选账号 → 解析设备形态 → 按互斥组走一条子功能 →
// 把结果交给渲染层 → 定退出码。
//
// 对位 Rust 侧 ../mcloud-rust/crates/mcloud-cli/src/signin_run.rs（多账号）与
// signin_one.rs（单账号）。渲染归 `signin_render.go`（另一条线），
// 协议与业务判定在 `internal/app` / `internal/upstream`。
//
// # 这一层不做什么（越界即返工）
//
//   - **不解析 HTTP 响应、不判 `code`** —— 那是 `internal/upstream` 的；
//   - **不判门槛**（`cloudType` 分流 / 每月一次机会 / 备份是否开启）——
//     都在 `internal/app`（`PlanReceive` / `PlanMultiple` / `PlanBackupGift`）；
//   - **不拼 `Authorization`** —— `app.CredentialFromAccount` / `CredentialFromPhone`；
//   - **不判"今天签没签"** —— `app.SignInReport.SignedToday()`。
//
// # 分支优先级（照抄 Rust 的 if 顺序，见规格 §1.6）
//
//	--records  >  --prizes  >  --raw  >  --status-only  >  签到主路
//
// 互斥表管不到这几对的组合（`--records --status-only` 是合法输入），
// 谁生效**只由顺序决定** —— 顺序写反了不会有任何报错，只是行为静默分叉。
//
// # 用 Go 写，不是用 Go 把 Rust 重写一遍（规格 §6.2）
//
//   - 单账号那一趟收成 [signinRun] 结构体 + 方法，**不**平铺成九个参数
//     （Rust 自己都挂了 `#[allow(clippy::too_many_arguments)]`）；
//   - 依赖直接用 `deps.Clock` / `deps.Random` / `deps.http()`，不为 trait 造接口；
//   - `--account` 是 `int`（下标），不是 `uint` —— `--account -1` 不能变成天文数字；
//   - "零值有意义"的字段一律走 `a.given(name)`（`--cloud-type 0` 是合法取值）。

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 分页常量
// ─────────────────────────────────────────────────────────────────────────────

// 这三个常量按**消费方**归属：分页循环是编排的事，所以它们在这里，
// 不在渲染文件里。
//
// ⚠ Rust 侧把它们定义在 `signin_output.rs`（`RECORD_PAGE_SIZE` `:49` /
// `RECORD_MAX_PAGES` `:54` / `PRIZE_PAGE_SIZE` `:102`）而**消费**在
// `signin_one.rs` —— 那是错位的切法（规格 §6.0 第 9 条），不照搬。
const (
	// recordPageSize 是 `--records` 每档取多少条。
	//
	// 50 是**只用来对账**的量级：H5 自己一页 10 条、滚动加载，
	// 而这里要的是"一眼看完整段近期流水"，翻页反而碍事。
	recordPageSize int64 = 50
	// recordMaxPages 是 `--records-all` 最多翻多少页 —— 纯粹是防
	// "上游 `total` 恒不回落到 0"那种死循环。20 页 = 1000 条，
	// 比实测最大的账本（460 条）宽裕一倍。
	recordMaxPages int64 = 20
	// prizePageSize 是 `--prizes` 取多少条。
	//
	// 与 H5 一致（它写死 `pageSize: 10`）：这条的用途是"看有没有待领的"，
	// 不是把历史全拉下来 —— 一页足够。
	prizePageSize int64 = 10
)

// signinMode 的两档**渲染层不认识**的取值。
//
// `signin_render.go` 的 [signinMode] 只认识 签到 / 状态 / 原始响应 三种，
// 因为它的 [signinView] 只有 `report` / `status` / `raw` 三个 payload 字段。
// `--records`（两个账本各一页）与 `--prizes`（一页奖品）的 payload 形状它表达不了，
// 所以那两档整批输出由本文件出（见 [signinRun.emit]）。
//
// 取值刻意与渲染层那三个拉开：哪天渲染层也加了同号常量，不会静默撞车。
const (
	signinModeRecords signinMode = 100
	signinModePrizes  signinMode = 101
)

// runSignin 是"不带子命令"与 `mcloud signin` 的**合流点**。
//
// 根命令与子命令各持一份 [SigninArgs]，但都走这个函数 ——
// 于是校验、退出码、后端缺口三条路径两边天然一致，
// 对位 Rust 的 `Cli::into_command()`（"两种写法在这里合流，往下只有一个入口"）。
func runSignin(deps *Deps, out *output, a *SigninArgs) error {
	// 先跑解析期之后、动手之前的用法校验：`--records-all` 单独给是**用法**错误，
	// 不该被"打不开库"这类**运行**错误盖掉（那会把退出码从 2 变成 1）。
	if err := a.validate(); err != nil {
		return err
	}

	store, err := openLocal(a.DB)
	if err != nil {
		return err
	}
	defer store.Close()

	ctx := context.Background()
	owner, err := preferredOwner(ctx, store)
	if err != nil {
		return err
	}
	scope := scopeForNewAccounts(owner)

	accounts, err := listSigninAccounts(ctx, store, scope)
	if err != nil {
		return err
	}

	// `--list` 在**选账号之前**短路：它要的就是"完整列表"（含停用），
	// 与本次要跑谁无关。⚠ 它**忽略 `--json`** —— 渲染层那张表是逐字抄 Rust 的。
	if a.List {
		renderSigninAccounts(out, accounts)
		return nil
	}

	targets, err := selectSigninTargets(out, accounts, a)
	if err != nil {
		return err
	}
	if len(targets) == 0 {
		reportNoSigninTargets(out, len(accounts))
		return nil
	}

	// 设备形态在**整次运行里只解析一次**：同一个安装就该是同一台设备。
	// 放进循环里会让每个账号拿到不同的 `device_short`（不传 `--device-short` 时
	// 现生成），于是"一次运行"对应了好几种设备形态 —— 与登录时的形态也对不上。
	appDeviceID, perr := app.LoadOrCreateAppDeviceID(a.AppDeviceFile, deps.Random)
	if perr != nil {
		return perr
	}
	// ⚠ `--device-short ""` 与"没给"必须分得开，所以判据是 [SigninArgs.given]
	// 而不是 `a.DeviceShort != ""` —— 前者才是 Rust 那个 `Option` 的对位。
	// 给了就只做 `ToUpper`（Rust 的 `to_ascii_uppercase`，**不** trim）。
	deviceShort := app.NewAppDeviceShort(deps.Random)
	if a.given("device-short") {
		deviceShort = strings.ToUpper(a.DeviceShort)
	}

	run := &signinRun{
		deps:        deps,
		out:         out,
		store:       store,
		scope:       scope,
		args:        a,
		up:          upstream.NewUpstream(deps.http()),
		appDeviceID: appDeviceID,
		deviceShort: deviceShort,
	}

	outcomes := make([]signinOutcome, 0, len(targets))
	for _, target := range targets {
		// **一个账号失败不影响其余**：逐个跑、失败记 stderr、继续下一个。
		// 旧项目对每个账号各自 try/catch，一个号挂了不该让其余账号今天签不上。
		outcomes = append(outcomes, run.one(ctx, target))
	}
	return run.emit(outcomes)
}

// listSigninAccounts 读账号清单（`sort_order ASC, id ASC`）。
func listSigninAccounts(ctx context.Context, store core.StateStore, scope core.Scope) ([]core.Account, error) {
	accounts, err := store.ListAccounts(ctx, scope)
	if err != nil {
		return nil, fmt.Errorf("读账号列表失败: %w", err)
	}
	return accounts, nil
}

// reportNoSigninTargets 是"一个账号都没选中"时的说明。
//
// # 两种处境必须分开说（刻意修正 Rust 的一句误导文案）
//
// Rust 在 targets 为空时一律打「库里还没有账号。先登录一个…」——
// 而"有账号、只是全停用了"也走这条分支，于是它说了一句与事实不符的话：
// 用户明明登录过（`--list` 看得见），却被指去重新登录。
//
// 两种处境的下一步动作也不同：空库要**去登录**；全停用要**指定下标或看清单**。
// 所以这里分开打 —— 这是与 Rust 的**刻意差异**，不是漏抄。
func reportNoSigninTargets(out *output, total int) {
	if total == 0 {
		out.human("库里还没有账号。先登录一个：`mcloud login sms --client app`")
		return
	}
	out.human("库里有 %d 个账号，但没有启用中的（用 `--account N` 指定，或 `--list` 看全部）", total)
}

// ─────────────────────────────────────────────────────────────────────────────
// 选账号
// ─────────────────────────────────────────────────────────────────────────────

// signinTarget 是这一趟要跑的一个账号。
type signinTarget struct {
	// index 是展示用下标（从 1 起，与 `--list` 打出来的一致）。
	//
	// `--phone` 定位的账号**不在**启用列表里也能跑，那时下标没有意义，记 0 ——
	// 打一个 0 出来只会让人以为它是第一个账号。
	index int
	// accountID 是账号 id（掩码）。
	accountID core.AccountID
	// phone 非空 = `--phone` 给的完整号码：**强制走现拼那条路**。
	//
	// ⚠ 它是"强制现拼"的开关，不是"先查库、查不到再拼"（规格 §6.0 第 10/11 条）。
	// Go 侧不需要在 target 里放一个 `phoneOverride` 字段来表达"有没有给"——
	// 非空即强制，一条路。
	phone string
	// account 是给渲染层用的账号（`--phone` 时是按号码推出来的那个）。
	account core.Account
}

// selectSigninTargets 决定这次跑哪些账号。
//
// 三条路，优先级从高到低：
//  1. `--phone`：老账号的逃生口，直接用号码推 id（与登录落库**同源**）；
//  2. `--account N`：按下标取**第 N 个账号**（含停用的 —— 显式指定就照做）；
//  3. 都不给：全部**启用**的账号。
//
// ⚠ 第 2 条把停用的账号也算进去，是为了让"下标 = 列表里的位置"这条规则不带例外：
// 一旦下标随启用状态变动，同一个数字在不同时刻会指到不同账号上。
func selectSigninTargets(out *output, accounts []core.Account, a *SigninArgs) ([]signinTarget, error) {
	// `--phone` 那条路**不查库**：id 由 [core.AccountLabel] 推出来，
	// 与登录落库用的是同一个函数，所以"登录时存进去的"和"签到时要读的"
	// 必然是同一行。
	if a.given("phone") {
		phone := strings.TrimSpace(a.Phone)
		if phone == "" {
			// `--phone ""` 在 Rust 侧会一路走到"库里没有账号 账号"这种看不懂的错。
			// 那是调用写错了，早点说清楚。
			return nil, fmt.Errorf("--phone 给的是空串: %w", ErrUsage)
		}
		id := core.NewAccountID(core.AccountLabel(nil, phone))
		return []signinTarget{{
			index:     0,
			accountID: id,
			phone:     phone,
			account:   core.Account{ID: id, Enabled: true},
		}}, nil
	}

	if wanted, requested := a.accountRequested(); requested {
		// 下标从 1 开始：`--account 0` 一定是搞错了，早点说清楚。
		//
		// ⚠ 负数是 Go 侧才有的输入（Rust 的 `usize` 在 clap 那层就拒了），
		// 而 `accounts[-1]` 会 panic。所以这里判 `<= 0`，
		// 与"从 1 开始"合并成同一句话 —— 这也是**运行**错误（退 1），不是用法错误。
		if wanted <= 0 {
			return nil, fmt.Errorf("`--account` 的下标从 1 开始（`--account 1` 是第一个账号）")
		}
		if wanted > len(accounts) {
			return nil, fmt.Errorf(
				"下标 %d 超出范围：库里一共 %d 个账号（用 `mcloud signin --list` 看清单）",
				wanted, len(accounts))
		}
		account := accounts[wanted-1]
		if !account.Enabled {
			// 显式指定就照做，但要提醒一句 —— 否则"我明明把它停用了、怎么还在跑"
			// 会变成一次没头没尾的排查。
			out.note("⚠ 账号 %d (%s) 是停用状态，本次仍然按你指定的跑。", wanted, account.ID)
		}
		return []signinTarget{{
			index:     wanted,
			accountID: account.ID,
			account:   account,
		}}, nil
	}

	targets := make([]signinTarget, 0, len(accounts))
	for position, account := range accounts {
		if !account.Enabled {
			continue
		}
		targets = append(targets, signinTarget{
			index:     position + 1,
			accountID: account.ID,
			account:   account,
		})
	}
	return targets, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 一趟运行
// ─────────────────────────────────────────────────────────────────────────────

// signinRun 是一次 `mcloud signin` 的全部上下文。
//
// 做成结构体而不是把九个参数平铺进函数签名：Rust 那边摆了七个参数并显式挂了
// `#[allow(clippy::too_many_arguments)]`，而 `store` / `scope` / `args` 三样
// 在每一层都要往下传 —— 平铺的代价是"摆错顺序编译器不吭声"（规格 §6.0 第 2 条）。
type signinRun struct {
	deps  *Deps
	out   *output
	store core.StateStore
	scope core.Scope
	args  *SigninArgs
	up    *upstream.Upstream
	// appDeviceID / deviceShort 是**这台安装**的设备形态，整次运行不变
	// （`--app-device-file` 那个 `deviceId` 与设备串第 7 段）。
	appDeviceID string
	deviceShort string
}

// signinOutcome 是一个账号这一趟的全部产出。
//
// `view` 是给渲染层的（签到 / 状态 / 原始响应那三档）；
// 其余几项是渲染层的 [signinView] 表达不了的 payload，由本文件自己出。
type signinOutcome struct {
	view signinView
	// extras 是四个写开关（`--receive` / `--revive` / `--multiple` / `--backup-gift`）
	// 的结果。⚠ 它们**不是**账号的成败：失败也进这里，`ok` 只表示"这次调用没出错"。
	extras []signinExtra
	// records 是 `--records` 的两个账本（改名前的云朵在前）。
	records []signinLedgerPage
	// prizes 是 `--prizes` 的那一页。
	prizes *upstream.PrizeRecords
	// cloudNote 是余额落库那一行（"" = 没成功，已经警告过了）。
	cloudNote string
}

// failed 记下"这一趟做了但没做成"。
func (o signinOutcome) failed(perr *core.Error) signinOutcome {
	o.view.failure = perr
	return o
}

// signinExtra 是一个写开关的结果：人看的一段文本 + `--json` 里的那个对象。
//
// 两处都要用，打包在一起是为了让"结果在哪个分支被消费"变得显式 ——
// 与 Rust 的 `WriteOutcome` 同形。
type signinExtra struct {
	// key 是 `--json` 里那个键（`receive` / `revive` / `multiple` / `backupGift`）。
	key string
	// human 是给人看的那几行（含"正在做什么"的前缀）。
	human string
	// payload 是塞进 `--json` 的那个对象。
	payload map[string]any
}

// signinLedgerPage 是一个账本收集完之后的整页。
type signinLedgerPage struct {
	key     string // `--json` 里的键：`cloud` / `bean`
	label   string
	book    int64
	records []any
	total   *int64
}

// one 跑**一个账号**的一趟。
//
// 顺序（每一步的理由都在下面）：
// 设备档案 → 凭据 → `--phone` 回填 → H5 `deviceId` → 进度行 → 分支。
func (r *signinRun) one(ctx context.Context, target signinTarget) signinOutcome {
	outcome := signinOutcome{view: signinView{
		account: target.account,
		index:   target.index,
		mode:    r.branchMode(),
	}}

	// 设备形态必须与登录时一致（同一个文件 → 同一个 `deviceId`）。
	// 漂移的表现是"凭据都在、却回 90001 未登录"，看起来像凭据坏了。
	profile := upstream.NewAppProfile(upstream.NewIPadAppDevice(r.appDeviceID, r.deviceShort))
	// 回填库里存的 `userDomainId`（= `x-yun-uni` 头）。读的范围必须与写的一致，
	// 否则会出现"读到某个账号的用户域、却把它发给另一个账号"。
	profile, perr := app.ProfileWithStoredDomainID(ctx, r.store, r.scope, profile, target.accountID)
	if perr != nil {
		return outcome.failed(perr)
	}

	credential, perr := r.credential(ctx, target)
	if perr != nil {
		return outcome.failed(perr)
	}

	// `--phone` 只该输一次：把号码记下来，下次 `--account <下标>` 就行。
	//
	// ⚠ **只记手机号**，不记上面那个现拼出来的整串 —— `authorization` 列的口径是
	// "服务端给的原文"，写进去等于给一个本地合成物伪造来源（见
	// [app.CredentialFromPhone] 的说明）。失败不致命（这次用的凭据已经拼好了），
	// 但要说一声，否则用户下次还得再输一遍而不知道为什么。
	if target.phone != "" {
		phone := target.phone
		if err := r.store.FillAccountCredentials(ctx, r.scope, target.accountID, nil, &phone); err != nil {
			r.out.note("⚠ 手机号没能记进库里（下次还得再输一遍）：%v", err)
		}
	}

	h5DeviceID := r.h5DeviceID(ctx, target.accountID)

	// 进度行在**发请求之前**打 —— 那才是这行的用处（等待期间看得见进度）。
	//
	// ⚠ 一律走 **stderr**（`out.note`）：这是与 Rust 的**刻意差异** ——
	// Rust 只在 `--json` 时走 stderr，非 json 走 stdout。本项目的约定是
	// "进度/提示永远 stderr"（见 output.go 顶部），否则
	// `mcloud signin > out.json` 会被进度行污染。
	r.out.note("%s", r.branchLine(outcome.view))

	switch {
	case r.args.Records:
		r.runRecords(ctx, &outcome, profile, credential)
	case r.args.Prizes:
		r.runPrizes(ctx, &outcome, profile, credential)
	case r.args.Raw:
		r.runRaw(ctx, &outcome, profile, credential)
	case r.args.StatusOnly:
		r.runStatusOnly(ctx, &outcome, profile, credential)
	default:
		r.runSignIn(ctx, &outcome, target.accountID, profile, credential, h5DeviceID)
	}
	return outcome
}

// credential 取签到凭据。
//
// `--phone` 那条路是"**强制现拼**"，不是"先查库、查不到再拼" ——
// 别把两条路合成一条（规格 §6.0 第 10 条）。
func (r *signinRun) credential(ctx context.Context, target signinTarget) (app.SignInCredential, *core.Error) {
	if target.phone != "" {
		return app.CredentialFromPhone(ctx, r.store, r.scope, target.phone)
	}
	return app.CredentialFromAccount(ctx, r.store, r.scope, target.accountID)
}

// h5DeviceID 取 H5 那个 `deviceId`（`--receive` 要用）。
//
// 两条规则：
//   - `--status-only` / `--raw` / `--records` **跳过解析** —— 查个状态不该顺手
//     打一次第三方风控（`--prizes` 不在跳过之列，与 Rust 一致）；
//   - 解析失败**不致命**：降级成"不带"，只把原因打到 stderr。签到本身不带它也能签上；
//     真正需要它的 `--receive` 会从上游拿到 `615` —— 那时这条提示就是
//     "为什么是 615"的唯一线索。
func (r *signinRun) h5DeviceID(ctx context.Context, accountID core.AccountID) *string {
	if r.args.StatusOnly || r.args.Raw || r.args.Records {
		return nil
	}
	if explicit, given := explicitH5DeviceID(r.args); given {
		return &explicit
	}
	id, perr := app.DeviceID(ctx, r.up, r.store, r.scope, accountID, r.deps.Clock, r.deps.Random)
	if perr != nil {
		r.out.note("⚠ 取 H5 deviceId 失败，这次不带它：%s", perr.Error())
		return nil
	}
	return &id
}

// explicitH5DeviceID 报告 `--device-id`（或它的别名 `--h5-device-id`）
// 是不是被**显式**给过。
//
// ⚠ 两个名字绑同一个变量，所以 `a.given("device-id")` 单独一条不够 ——
// 只给别名时它读不到。空串也算"给了"（Rust 那边 `Some("")` 同样直接用）。
func explicitH5DeviceID(a *SigninArgs) (string, bool) {
	if a.given("device-id") || a.given("h5-device-id") {
		return a.DeviceID, true
	}
	return "", false
}

// explicitCloudType 把 `--cloud-type` 的"没给"与"给了 0"分开。
//
// ⚠⚠ **不能**用 `a.CloudType != 0` 判：`0` 是合法取值（当前可领取）。
// 判错了的表现是"我明明说领 cloudType=0 那一笔，它却去自动分流了"。
func explicitCloudType(a *SigninArgs) (*int64, bool) {
	if !a.given("cloud-type") {
		return nil, false
	}
	value := a.CloudType
	return &value, true
}

// branchMode 是这一趟走哪条分支 —— 决定进度行的措辞。
func (r *signinRun) branchMode() signinMode {
	switch {
	case r.args.Records:
		return signinModeRecords
	case r.args.Prizes:
		return signinModePrizes
	case r.args.Raw:
		return signinModeRaw
	case r.args.StatusOnly:
		return signinModeStatus
	}
	return signinModeSignIn
}

// branchLine 是"正在做什么"那行。
//
// 签到 / 状态 / 原始响应三档直接用渲染层的 [signinProgressLine]；
// records / prizes 两档渲染层不认识（它的 mode 里没有这两个值），
// 措辞在这里补 —— 格式与那一行**逐字一致**。
func (r *signinRun) branchLine(view signinView) string {
	switch view.mode {
	case signinModeRecords:
		return fmt.Sprintf("%s：以 iOS App 身份取豆的收支流水", signinWho(view))
	case signinModePrizes:
		return fmt.Sprintf("%s：以 iOS App 身份查待领取奖品", signinWho(view))
	}
	return signinProgressLine(view)
}

// ─────────────────────────────────────────────────────────────────────────────
// 五条分支
// ─────────────────────────────────────────────────────────────────────────────

// runRecords 是 `--records`：两个账本各取一页（`--records-all` 时翻到底）。
//
// 账本顺序固定：改名前的云朵在前、改名后的 AI 豆在后 ——
// 这样人看的输出与 `--json` 的键序一致。
func (r *signinRun) runRecords(
	ctx context.Context, outcome *signinOutcome,
	profile *upstream.AppProfile, credential app.SignInCredential,
) {
	ledgers := []struct {
		key   string
		label string
		book  int64
	}{
		{"cloud", "云朵账本（改名前）", upstream.RecordBookCloud},
		{"bean", "AI豆账本（改名后）", upstream.RecordBookBean},
	}
	pages := make([]signinLedgerPage, 0, len(ledgers))
	for _, ledger := range ledgers {
		page, perr := r.ledger(ctx, profile, credential, ledger.book)
		if perr != nil {
			outcome.view.failure = perr
			return
		}
		page.key = ledger.key
		page.label = ledger.label
		pages = append(pages, page)
	}
	outcome.records = pages
}

// ledger 把一个账本收集完。
//
// 逐字规则（规格 §2.3，三个退出条件**缺一不可**）：
//
//	collected = []; total = nil
//	for page_number in 1..=recordMaxPages:
//	    page = cloud_records(book, filter=0, page_number, page_size=50)
//	    got = len(page.records)
//	    total = 第一个非 nil 的 total
//	    collected += page.records
//	    if !records_all || got == 0: break     # 默认只取一页 / 这一页是空的
//	    if total != nil && len(collected) >= total: break
//
// ⚠ 第二个条件（`got == 0`）是 `total` 缺失时**唯一**的终止依据：
// 少了它，上游不给 `total` 时 `--records-all` 会一路翻到 20 页上限。
//
// ⚠ `total` 取**第一个非 nil** 的那个：Rust 写的是 `page.total.or(total)`
// （后到的非 nil 会覆盖先到的），而规格 §2.3 记的是"保留先到的"。
// 两者只在两页给的 `total` 不一致时才有差别（那是上游数据有问题），
// 这里按规格的"保留先到的"实现，并把差别留在这里。
func (r *signinRun) ledger(
	ctx context.Context, profile *upstream.AppProfile,
	credential app.SignInCredential, book int64,
) (signinLedgerPage, *core.Error) {
	collected := make([]any, 0, recordPageSize)
	var total *int64
	for pageNumber := int64(1); pageNumber <= recordMaxPages; pageNumber++ {
		page, perr := app.CloudRecords(ctx, r.up, profile, &credential, app.RecordsQuery{
			AiDou:      book,
			Filter:     upstream.RecordFilterAll,
			PageNumber: pageNumber,
			PageSize:   recordPageSize,
		}, r.deps.Clock, r.deps.Random)
		if perr != nil {
			return signinLedgerPage{}, perr
		}
		got := len(page.Records)
		if total == nil {
			total = page.Total
		}
		collected = append(collected, page.Records...)
		if !r.args.RecordsAll || got == 0 {
			break
		}
		if total != nil && int64(len(collected)) >= *total {
			break
		}
	}
	return signinLedgerPage{book: book, records: collected, total: total}, nil
}

// runPrizes 是 `--prizes`：**只取一页**，只读、不落库。
//
// 不落库是有意的：奖品列表不是余额，记进 `cloud_stats` 只会往趋势图里塞噪声。
func (r *signinRun) runPrizes(
	ctx context.Context, outcome *signinOutcome,
	profile *upstream.AppProfile, credential app.SignInCredential,
) {
	page, perr := app.PendingPrizes(
		ctx, r.up, profile, &credential, prizePageSize, r.deps.Clock, r.deps.Random)
	if perr != nil {
		outcome.view.failure = perr
		return
	}
	outcome.prizes = &page
}

// runRaw 是 `--raw`：`infoV3` 的原始信封，**不解析、不落库**。
//
// 不落库是有意的 —— 原始响应没被解析成余额，记一条"余额未知"的历史
// 只会往趋势图里塞噪声。`--raw` 是拿来看的，不是拿来攒数据的。
func (r *signinRun) runRaw(
	ctx context.Context, outcome *signinOutcome,
	profile *upstream.AppProfile, credential app.SignInCredential,
) {
	value, perr := app.SignInStatusRaw(ctx, r.up, profile, &credential, r.deps.Clock, r.deps.Random)
	if perr != nil {
		outcome.view.failure = perr
		return
	}
	// 渲染层要的是 [json.RawMessage]（`[]byte` 会被编成 base64 字符串）。
	// 用 core.MarshalJSON 而不是 json.Marshal：前者与 serde_json 同形
	// （不转义 HTML、不补尾换行），后者会把 `<` 写成 `\u003c`。
	encoded, err := core.MarshalJSON(value)
	if err != nil {
		outcome.view.failure = core.NewInternal("序列化 infoV3 原始响应失败: " + err.Error())
		return
	}
	outcome.view.raw = json.RawMessage(encoded)
}

// runStatusOnly 是 `--status-only`：只查状态，不发写请求。
//
// ⚠ **会落库余额**，而且记的是 `bean_total`（上游 `total`）**不是** `beforeTotal` ——
// 后者不随领取变化，记它等于让趋势图永远是一条直线（见 [app.RecordCloud]）。
func (r *signinRun) runStatusOnly(
	ctx context.Context, outcome *signinOutcome,
	profile *upstream.AppProfile, credential app.SignInCredential,
) {
	status, perr := app.SignInStatus(ctx, r.up, profile, &credential, r.deps.Clock, r.deps.Random)
	if perr != nil {
		outcome.view.failure = perr
		return
	}
	outcome.view.status = &status
	// 落库与"这次怎么打印"无关：`--json` 模式同样该攒下余额历史，
	// 否则脚本调用的那些天在趋势图上是空的。
	outcome.cloudNote = r.recordCloud(ctx, outcome.view.account.ID, status.BeanTotal)
}

// runSignIn 是签到主路，外加四个写开关。
func (r *signinRun) runSignIn(
	ctx context.Context, outcome *signinOutcome, accountID core.AccountID,
	profile *upstream.AppProfile, credential app.SignInCredential, h5DeviceID *string,
) {
	report, perr := app.CloudSignIn(ctx, r.up, profile, &credential, app.SignInOptions{
		DeviceID: h5DeviceID,
		Force:    r.args.Force,
	}, r.deps.Clock, r.deps.Random)
	if perr != nil {
		outcome.view.failure = perr
		return
	}
	outcome.view.report = &report
	// 记 `after` 而不是 `before`：两者在余额上应当相等（签到只是打卡，
	// 豆要另外领 —— 实测签到前后 `total` 不变），取 `after` 是"更晚读到的那次"。
	outcome.cloudNote = r.recordCloud(ctx, accountID, report.After.BeanTotal)

	// ⚠ 四个写开关**必须跑在"怎么打印"之前**：放在后面的话，
	// `--json --receive` 会**静默什么都不做** —— 而静默空操作比报错难查得多
	// （定时任务会以为领过了，日志里却一个字都没有）。
	//
	// ⚠⚠ 它们**失败也不让整个账号失败**：用途恰恰是"看上游到底回什么码"，
	// 抛出去就只剩一句错误信息（规格 §6.2 第 8 条）。
	if r.args.Receive {
		outcome.extras = append(outcome.extras, r.receive(ctx, profile, credential, h5DeviceID))
	}
	if r.args.Revive {
		outcome.extras = append(outcome.extras, r.revive(ctx, profile, credential))
	}
	if r.args.Multiple {
		outcome.extras = append(outcome.extras, r.multiple(ctx, profile, credential))
	}
	if r.args.BackupGift {
		outcome.extras = append(outcome.extras, r.backupGift(ctx, profile, credential))
	}
}

// recordCloud 把这次读到的余额落库，返回要打印的那一行（"" = 没成功，已经警告过了）。
//
// **失败不影响签到结论**：余额是顺带采集的副产品，它写不进去不该把一次成功的签到
// 变成失败（规划 §3.4："多源取可信值，失败降级，不阻塞主流程"）。
// 但也**不能静默** —— 悄悄不采集的后果是趋势图上少一段，而那时没人会想到
// 是几个月前某次写库失败。所以退化成一句 stderr 警告。
func (r *signinRun) recordCloud(ctx context.Context, accountID core.AccountID, count int64) string {
	stats, perr := app.RecordCloud(ctx, r.store, r.scope, accountID, count, r.deps.Clock)
	if perr != nil {
		r.out.note("⚠ 余额没能落库（不影响本次签到）：%s", perr.Error())
		return ""
	}
	return fmt.Sprintf("  余额已记录：%d 豆（日增 %+d，周增 %+d）",
		stats.CloudCount, stats.CloudDiff, stats.CloudDiffWeek)
}

// ─────────────────────────────────────────────────────────────────────────────
// 四个写开关
// ─────────────────────────────────────────────────────────────────────────────

// receive 是 `--receive`。两条子路，由 `--cloud-type` 是否**显式给出**决定：
//
//   - 给了 → **显式模式**（只发这一笔，自检用）：`receiveV3`；
//   - 没给 → **自动分流**（日常）：先取 `infoV3`，再按账目自己判断该领哪些。
//
// ⚠ `ok` = "这次调用没出错"，**不等于"领到了"**。自动模式下每条账目各自的
// 成败在 `items` 里；`ok` 仍是 `true`。别把 `ok` 实现成"领到了"。
func (r *signinRun) receive(
	ctx context.Context, profile *upstream.AppProfile,
	credential app.SignInCredential, h5DeviceID *string,
) signinExtra {
	device := ""
	if h5DeviceID != nil {
		device = *h5DeviceID
	}
	// 设备来源要说清楚：`--receive` 不带 deviceId 时上游回 `615`，
	// 而"为什么是 615"只有这一行说得出来。
	deviceNote := "为空"
	if device != "" {
		if _, given := explicitH5DeviceID(r.args); given {
			deviceNote = "用 --device-id 给的"
		} else {
			deviceNote = fmt.Sprintf("用库里/新上报的（%s…）", firstRunes(device, 8))
		}
	}

	if cloudType, given := explicitCloudType(r.args); given {
		// ── 显式模式（自检用）：我说领哪一笔就领哪一笔 ──────────────────
		head := fmt.Sprintf("指定领取：再发一次 receiveV3（cloudId=%q，cloudType=%s，deviceId %s）",
			r.args.CloudID, optionInt64(cloudType), deviceNote)
		result, perr := app.ReceiveCloud(ctx, r.up, profile, &credential, app.ReceiveTarget{
			CloudID:   r.args.CloudID,
			CloudType: cloudType,
			DeviceID:  device,
		}, r.deps.Clock, r.deps.Random)
		if perr != nil {
			return signinExtra{
				key:     "receive",
				human:   head + "\n" + fmt.Sprintf("  领取失败：%s", perr.Error()),
				payload: map[string]any{"ok": false, "mode": "explicit", "error": perr.Error()},
			}
		}
		return signinExtra{
			key:     "receive",
			human:   head + "\n" + fmt.Sprintf("  上游回的 result.receive = %d 豆", result.Receive),
			payload: map[string]any{"ok": true, "mode": "explicit", "received": result.Receive},
		}
	}

	// ── 默认：按上游的账目**自己判断**该领哪些 ────────────────────────
	//
	// 这是日常该走的路。把"该领哪个"交给用户，等于让用户去猜 `cloudType` 的含义，
	// 而猜错的表现是"领到 0 朵"——看起来像上游没发。
	head := fmt.Sprintf("按 infoV3 的账目自动分流领取（deviceId %s）", deviceNote)
	outcomes, perr := app.ReceiveAll(ctx, r.up, profile, &credential, device, r.deps.Clock, r.deps.Random)
	if perr != nil {
		return signinExtra{
			key:     "receive",
			human:   head + "\n" + fmt.Sprintf("  领取失败：%s", perr.Error()),
			payload: map[string]any{"ok": false, "mode": "auto", "error": perr.Error()},
		}
	}
	items := make([]any, 0, len(outcomes))
	lines := make([]string, 0, len(outcomes))
	for _, item := range outcomes {
		items = append(items, receiveOutcomePayload(item))
		lines = append(lines, receiveOutcomeLine(item))
	}
	tail := "  账目里没有可处理的条目（`receiveList` 为空）"
	if len(lines) > 0 {
		tail = strings.Join(lines, "\n")
	}
	return signinExtra{
		key:     "receive",
		human:   head + "\n" + tail,
		payload: map[string]any{"ok": true, "mode": "auto", "items": items},
	}
}

// revive 是 `--revive`：用掉一张复活卡 / 回馈卡。
//
// ⚠⚠ **不可逆** —— H5 原话「关闭弹窗视为放弃机会，无法补发」。
// 所以它单独一个开关，绝不跟着签到顺手跑。
//
// ⚠ `deviceid` 头**只取显式 `--device-id`，故意不回落库**：
// H5 的 `receiveRevivalReward` 定义里没有 `isDeviceId`，默认就该一个头都不带。
// `nil` = 不带，与"给了个空串"是两件事。
func (r *signinRun) revive(
	ctx context.Context, profile *upstream.AppProfile, credential app.SignInCredential,
) signinExtra {
	var device *string
	if explicit, given := explicitH5DeviceID(r.args); given {
		device = &explicit
	}
	head := fmt.Sprintf("再发一次 receiveRevivalReward（用复活卡/回馈卡，deviceid 头%s）",
		optionGiven(device != nil, "已给", "不带"))

	reward, perr := app.ReceiveRevivalReward(
		ctx, r.up, profile, &credential, device, r.deps.Clock, r.deps.Random)
	if perr != nil {
		return signinExtra{
			key:     "revive",
			human:   head + "\n" + fmt.Sprintf("  失败：%s", perr.Error()),
			payload: map[string]any{"ok": false, "error": perr.Error()},
		}
	}
	card := "回馈卡"
	if reward.CardType == 1 {
		card = "复活卡"
	}
	return signinExtra{
		key: "revive",
		human: head + "\n" + fmt.Sprintf("  用掉一张%s：本次 +%d 豆，累计 %d（littleFirstTime=%d）",
			card, reward.RewardClouds, reward.TotalClouds, reward.LittleFirstTime),
		payload: map[string]any{
			"ok":              true,
			"cardType":        reward.CardType,
			"rewardClouds":    reward.RewardClouds,
			"totalClouds":     reward.TotalClouds,
			"littleFirstTime": reward.LittleFirstTime,
		},
	}
}

// multiple 是 `--multiple`：领签到翻倍。**写请求，每月只有一次机会。**
//
// 门槛（`curMonthBackup && !curMonthBackupSignAccept`）由 [app.ClaimMultiple] 自己判 ——
// 不成立就**一个请求都不发**。
//
// ⚠ `ok` = "这次调用没出错"，**不等于"领到了"**：三种"按逻辑就没领"
// （已领过 / 没开备份 / 判不了）也是 `ok: true`。领没领到看 `claimed`。
func (r *signinRun) multiple(
	ctx context.Context, profile *upstream.AppProfile, credential app.SignInCredential,
) signinExtra {
	const head = "领签到翻倍（`multiple`，每月一次）"
	outcome, perr := app.ClaimMultiple(ctx, r.up, profile, &credential, r.deps.Clock, r.deps.Random)
	if perr != nil {
		return signinExtra{
			key:     "multiple",
			human:   head + "\n" + fmt.Sprintf("  失败：%s", perr.Error()),
			payload: map[string]any{"ok": false, "claimed": false, "error": perr.Error()},
		}
	}
	return signinExtra{
		key:   "multiple",
		human: head + "\n" + multipleOutcomeLine(outcome),
		payload: map[string]any{
			"ok":      true,
			"claimed": outcome.Kind == app.MultipleOutcomeClaimed,
			"outcome": multipleOutcomePayload(outcome),
		},
	}
}

// backupGift 是 `--backup-gift`：领备份好礼。**写请求，每月一次。**
//
// 门槛（`curMonth` 前置、再看 `state`）由 [app.ClaimBackupGift] 自己判。
// `ok` / `claimed` 的约定与 `--multiple` 完全一致。
func (r *signinRun) backupGift(
	ctx context.Context, profile *upstream.AppProfile, credential app.SignInCredential,
) signinExtra {
	const head = "领备份好礼（`backupgift`，每月一次）"
	outcome, perr := app.ClaimBackupGift(ctx, r.up, profile, &credential, r.deps.Clock, r.deps.Random)
	if perr != nil {
		return signinExtra{
			key:     "backupGift",
			human:   head + "\n" + fmt.Sprintf("  失败：%s", perr.Error()),
			payload: map[string]any{"ok": false, "claimed": false, "error": perr.Error()},
		}
	}
	return signinExtra{
		key:   "backupGift",
		human: head + "\n" + backupGiftOutcomeLine(outcome),
		payload: map[string]any{
			"ok":      true,
			"claimed": outcome.Kind == app.BackupGiftOutcomeClaimed,
			"outcome": backupGiftOutcomePayload(outcome),
		},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 写开关的文案与形状（逐字对位 Rust 的 signin_output.rs）
// ─────────────────────────────────────────────────────────────────────────────

// receiveOutcomeLine 是一条领取实况 → 人看的一行。
//
// 四种"没领到"要分得开，否则全都长得像"上游没发"：
//   - 下月才可领：**按逻辑就不该领**；
//   - 月度那条没带结算日：**发了裸请求**（上游没给，与真实客户端一致）；
//   - 领到 0 与"上游没报数"：`None` ≠ 0；
//   - 失败：**发了但失败**（例如月度那条回 `1302` 未开备份）。
func receiveOutcomeLine(outcome app.ReceiveOutcome) string {
	switch outcome.Kind {
	case app.ReceiveOutcomeNextMonth:
		return fmt.Sprintf(
			"  跳过 %d 豆：下月才可领（`cloudType=2`，官方 H5 也只弹提示、不发请求）",
			outcome.CloudNum)
	case app.ReceiveOutcomeMonthlyExpansion:
		// 没带结算日要说出来：结果可能对不上，但**请求确实发了**。
		date := "**没带结算日**（上游没给，照 H5 发的是裸请求）"
		if outcome.AcceptDate != "" {
			date = "结算日 " + outcome.AcceptDate
		}
		if outcome.CloudCount != nil {
			return fmt.Sprintf("  月度奖励领取成功（%s）：+%d 豆", date, *outcome.CloudCount)
		}
		return fmt.Sprintf("  月度奖励已提交（%s）：上游没报领到多少", date)
	case app.ReceiveOutcomeImmediate:
		return fmt.Sprintf("  立即领取（cloudType=%d）：+%d 豆", outcome.CloudType, outcome.Received)
	default:
		return fmt.Sprintf("  领取失败（cloudType=%d）：%s", outcome.CloudType, outcome.Error)
	}
}

// receiveOutcomePayload 同上，给 `--json` 用的结构化形状。
func receiveOutcomePayload(outcome app.ReceiveOutcome) map[string]any {
	switch outcome.Kind {
	case app.ReceiveOutcomeNextMonth:
		return map[string]any{"action": "nextMonth", "cloudNum": outcome.CloudNum}
	case app.ReceiveOutcomeMonthlyExpansion:
		return map[string]any{
			"action":     "monthlyExpansion",
			"acceptDate": optionalString(outcome.AcceptDate),
			"cloudCount": outcome.CloudCount,
		}
	case app.ReceiveOutcomeImmediate:
		return map[string]any{
			"action":    "immediate",
			"cloudType": outcome.CloudType,
			"received":  outcome.Received,
		}
	default:
		return map[string]any{
			"action":    "failed",
			"cloudType": outcome.CloudType,
			"error":     outcome.Error,
		}
	}
}

// multipleOutcomeLine 是一条签到翻倍结果 → 人看的一行。
//
// 四种"没领到"要分得开，否则全都长得像"上游没发"：
// 本月已领过 / 本月没开备份 / 上游没给门槛字段（判不了，所以没发）/ 发了但失败。
func multipleOutcomeLine(outcome app.MultipleOutcome) string {
	switch outcome.Kind {
	case app.MultipleOutcomeClaimed:
		// `multiple` 是**倍数**、`cloudCount` 是**合计** —— 两者都可能缺，分开拼。
		switch {
		case outcome.Multiple != nil && outcome.CloudCount != nil:
			return fmt.Sprintf("  签到翻倍领取成功：%d 倍，共计 +%d 豆",
				*outcome.Multiple, *outcome.CloudCount)
		case outcome.CloudCount != nil:
			return fmt.Sprintf("  签到翻倍领取成功：+%d 豆（上游没报倍数）", *outcome.CloudCount)
		case outcome.Multiple != nil:
			return fmt.Sprintf("  签到翻倍已领取：%d 倍（上游没报合计）", *outcome.Multiple)
		default:
			return "  签到翻倍已领取：上游没报倍数也没报合计"
		}
	case app.MultipleOutcomeAlreadyAccepted:
		return "  跳过：本月已经领过翻倍机会（每月只有一次）"
	case app.MultipleOutcomeNoBackup:
		return "  跳过：本月未开启自动备份，翻倍机会没产生。\n" +
			"  ⚠ 旧项目会上传一个文件把备份打开再重试，我们还没有上传能力 ——\n" +
			"  去 App 里随便备份一个文件，下个月起就有了。"
	case app.MultipleOutcomeUnknown:
		return "  跳过：`infoV3` 没给 `curMonthBackup` / `curMonthBackupSignAccept`，" +
			"判不了，故一个请求都没发"
	default:
		return fmt.Sprintf("  领取失败：%s", outcome.Error)
	}
}

// multipleOutcomePayload 同上，给 `--json` 用的结构化形状。
func multipleOutcomePayload(outcome app.MultipleOutcome) map[string]any {
	switch outcome.Kind {
	case app.MultipleOutcomeClaimed:
		return map[string]any{
			"action":     "claimed",
			"multiple":   outcome.Multiple,
			"cloudCount": outcome.CloudCount,
		}
	case app.MultipleOutcomeAlreadyAccepted:
		return map[string]any{"action": "alreadyAccepted"}
	case app.MultipleOutcomeNoBackup:
		return map[string]any{"action": "noBackup"}
	case app.MultipleOutcomeUnknown:
		return map[string]any{"action": "unknownGate"}
	default:
		return map[string]any{"action": "failed", "error": outcome.Error}
	}
}

// backupGiftOutcomeLine 是领备份好礼的一条结果 → 人看的一行。
//
// 四档"没领"必须分得开 —— 它们在日志里都长得像"没动静"，但下一步该做什么完全不同。
func backupGiftOutcomeLine(outcome app.BackupGiftOutcome) string {
	switch outcome.Kind {
	case app.BackupGiftOutcomeClaimed:
		if outcome.Received != nil {
			return fmt.Sprintf("  备份好礼领取成功：+%d 豆", *outcome.Received)
		}
		return "  备份好礼已领取：上游没报领到多少"
	case app.BackupGiftOutcomeAlreadyReceived:
		return "  跳过：本月已经领过备份好礼（每月只有一次）"
	case app.BackupGiftOutcomeNoBackup:
		return "  跳过：未开启自动备份（`state=-1`）——\n" +
			"  去 App 里开启自动备份，之后每月才有这份礼。"
	case app.BackupGiftOutcomeNothingThisMonth:
		return "  跳过：本月没有备份好礼可领（`curMonth` 为假）—— 这**不是**未开备份"
	default:
		return fmt.Sprintf("  领取失败：%s", outcome.Error)
	}
}

// backupGiftOutcomePayload 同上，给 `--json` 用的结构化形状。
func backupGiftOutcomePayload(outcome app.BackupGiftOutcome) map[string]any {
	switch outcome.Kind {
	case app.BackupGiftOutcomeClaimed:
		return map[string]any{"action": "claimed", "received": outcome.Received}
	case app.BackupGiftOutcomeAlreadyReceived:
		return map[string]any{"action": "alreadyReceived"}
	case app.BackupGiftOutcomeNoBackup:
		return map[string]any{"action": "noBackup"}
	case app.BackupGiftOutcomeNothingThisMonth:
		return map[string]any{"action": "nothingThisMonth"}
	default:
		return map[string]any{"action": "failed", "error": outcome.Error}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 输出
// ─────────────────────────────────────────────────────────────────────────────

// emit 把整批产出交出去，并定退出码。
//
// # 三条出口
//
//  1. `--records` / `--prizes`：这两档的 payload 形状（两个账本 / 一页奖品）
//     渲染层的 [signinView] 表达不了，整批由本文件出；
//  2. `--json` 且带写开关结果：Rust 把 `receive` / `revive` / `multiple` /
//     `backupGift` 四个键**并进每个账号那个对象**，而 [renderSigninJSON] 只认
//     [signinStatusJSON] —— 所以这条也由本文件拼（复用渲染层的 payload 构造器）；
//  3. 其余：交给渲染层（签到 / 状态 / 原始响应三档 + 失败 + 汇总）。
//
// ⚠ 退出码与汇总**同源**：都用 [summarizeSignin] 的 `.failed`，
// 否则会出现"屏幕上写着失败 2 个、退出码却是 0"。
func (r *signinRun) emit(outcomes []signinOutcome) error {
	views := make([]signinView, 0, len(outcomes))
	for _, outcome := range outcomes {
		views = append(views, outcome.view)
	}

	switch {
	case r.args.Records || r.args.Prizes:
		if err := r.emitBranch(outcomes); err != nil {
			return err
		}
	case r.args.JSON && anyExtras(outcomes):
		if err := r.emitJSONWithExtras(outcomes); err != nil {
			return err
		}
	default:
		if err := renderSigninBatch(r.out, views, r.args.JSON); err != nil {
			return err
		}
		// 余额落库那行排在结果块之后（与 Rust 同序）。⚠ 只在人看的模式打：
		// `--json` 的文档里没有它（Rust 同样不带）。
		if !r.args.JSON {
			r.emitCloudNotes(outcomes)
			// 写开关的输出排在**签到结论与余额之后**：它们是在上面已经发生过的动作，
			// 但"这次签到怎么样"才是这一屏的主角（`signin_one.rs:549-556`）。
			r.emitExtrasHuman(outcomes)
		}
	}

	if summary := summarizeSignin(views); summary.failed > 0 {
		// 退出码要非零：定时任务靠它判断"这次到底有没有全部成功"。
		// 只打日志的话，一个静默失败的账号可以连着挂一个月没人知道。
		return fmt.Errorf("%d 个账号失败（本次共 %d 个）", summary.failed, len(outcomes))
	}
	return nil
}

// emitCloudNotes 打余额落库那几行。
func (r *signinRun) emitCloudNotes(outcomes []signinOutcome) {
	for _, outcome := range outcomes {
		if outcome.cloudNote != "" {
			r.out.human("%s", outcome.cloudNote)
		}
	}
}

// emitExtrasHuman 打四个写开关的人看那几段。
//
// ⚠ 写开关**只在这条路（非 `--json` 的签到主路）上有输出**：
// `--records` / `--prizes` 与它们互斥（`args.go` 的互斥表），
// 而 `--json` 那条走 [signinRun.emitJSONWithExtras]（它们进的是每个账号那个对象）。
//
// ⚠ 多账号时这里是**按账号分组**打的（先把所有账号的结果块打完，再打各账号的写开关），
// 而 Rust 是**按账号交错**的（`signin_one.rs` 一个账号一趟全打完）。分组是渲染契约
// 决定的：[renderSigninBatch] 拿的是整批 `views`，它打完就已经越过了所有账号。
// 单账号（最常见的用法）两者完全一致。
func (r *signinRun) emitExtrasHuman(outcomes []signinOutcome) {
	for _, outcome := range outcomes {
		for _, extra := range outcome.extras {
			if extra.human != "" {
				r.out.human("%s", extra.human)
			}
		}
	}
}

// emitBranch 是 `--records` / `--prizes` 那一批的输出。
//
// 这两档渲染层出不了（它没有对应的 payload 字段与 mode），所以分隔行、
// 失败行、结果块、`--json` 文档都在这里 —— 形状逐字对位 Rust 的
// `print_cloud_records` / `print_pending_prizes` 与那两个 `json!`。
//
// ⚠ 进度行**不在这里打**：[signinRun.one] 已经在发请求之前用 `out.note` 打过一次。
func (r *signinRun) emitBranch(outcomes []signinOutcome) error {
	multi := len(outcomes) > 1
	entries := make([]any, 0, len(outcomes))
	for _, outcome := range outcomes {
		if multi {
			separator := fmt.Sprintf("------ %s ------", signinWho(outcome.view))
			if r.args.JSON {
				r.out.note("%s", separator)
			} else {
				r.out.human("%s", separator)
			}
		}
		if outcome.view.failure != nil {
			// 失败**永远**走 stderr：它不是这一趟的结果，而是"这一趟没做成"。
			r.out.note("✗ %s 失败：%s", signinWho(outcome.view), outcome.view.failure.Error())
			if r.args.JSON {
				entries = append(entries, failureEntry(outcome.view))
			}
			continue
		}
		switch {
		case outcome.records != nil:
			if r.args.JSON {
				entries = append(entries, recordsPayload(outcome.records))
			} else {
				printLedgers(r.out, outcome.records)
			}
		case outcome.prizes != nil:
			if r.args.JSON {
				entries = append(entries, prizesPayload(*outcome.prizes))
			} else {
				printPrizes(r.out, *outcome.prizes)
			}
		}
	}
	if r.args.JSON {
		return renderSigninDocument(r.out, entries)
	}
	return nil
}

// emitJSONWithExtras 是"`--json` 且带写开关结果"那条路。
//
// Rust 把四个写开关的结果**并进每个账号那个对象**（`signin_one.rs:514-526`）。
// [renderSigninJSON] 只认 [signinStatusJSON]，没有放这些键的地方 ——
// 所以这条自己拼，但**复用渲染层的 payload 构造器**：键名与顺序只有一份真相。
func (r *signinRun) emitJSONWithExtras(outcomes []signinOutcome) error {
	entries := make([]any, 0, len(outcomes))
	for _, outcome := range outcomes {
		if outcome.view.failure != nil {
			entries = append(entries, failureEntry(outcome.view))
			continue
		}
		payload, err := signinPayloadWithExtras(outcome.view, outcome.extras)
		if err != nil {
			return err
		}
		entries = append(entries, payload)
	}
	return renderSigninDocument(r.out, entries)
}

// failureEntry 是 `--json` 里失败那一档的形状。
func failureEntry(view signinView) signinFailureJSON {
	return signinFailureJSON{
		Account: signinAccountID(view.account),
		Error:   view.failure.Error(),
		Index:   view.index,
	}
}

// signinPayloadWithExtras 把状态对象与写开关的结果合成一个对象。
//
// 走一遍 JSON 而不是直接改结构体：`signinStatusPayload` 的键名与顺序是渲染层
// 定下的契约（按 JSON 键名排），这里**只加键、不动它** —— 复用同一个构造器，
// 键名与顺序才不会有两份真相。合出来的键序仍是字典序（Go 的 map 与 Rust 的
// BTreeMap 在这一点上一致）。
func signinPayloadWithExtras(view signinView, extras []signinExtra) (any, error) {
	encoded, err := core.MarshalJSON(signinStatusPayload(view))
	if err != nil {
		return nil, fmt.Errorf("序列化签到结果失败: %w", err)
	}
	decoded, ok := core.ParseJSON(string(encoded))
	if !ok {
		return nil, fmt.Errorf("内部错误：签到结果不是合法 JSON")
	}
	object, ok := decoded.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("内部错误：签到结果不是对象")
	}
	for _, extra := range extras {
		object[extra.key] = extra.payload
	}
	return object, nil
}

// renderSigninDocument 打一份"条目集合"的 JSON 文档：**一个条目给对象、
// 否则给数组**。
//
// ⚠ 这条规则与 [renderSigninJSON] 是**同一条**（`len(entries) == 1` → 对象）。
// 之所以在这里再写一次：上面那两条出口的 payload 形状渲染层表达不了，
// 它们的文档只能在编排层拼。规则本身仍以渲染层那份为准。
func renderSigninDocument(out *output, entries []any) error {
	if len(entries) == 1 {
		return out.json(entries[0])
	}
	return out.json(entries)
}

// anyExtras 报告这一批里有没有写开关的结果。
func anyExtras(outcomes []signinOutcome) bool {
	for _, outcome := range outcomes {
		if len(outcome.extras) > 0 {
			return true
		}
	}
	return false
}

// ─────────────────────────────────────────────────────────────────────────────
// records / prizes 的人看输出与 JSON 形状
// ─────────────────────────────────────────────────────────────────────────────

// printLedgers 打两个账本。
func printLedgers(out *output, pages []signinLedgerPage) {
	for _, page := range pages {
		printLedger(out, page)
	}
}

// printLedger 打一个账本的一页（或翻到底之后的全部）。
//
// ⚠ 条目**原样打**（一条一行 compact JSON）：流水里的字段名**没有实拍**
// （H5 只把它丢给模板渲染，从没读过具体字段名）。假装知道哪个是金额、哪个是时间，
// 只会把猜测写进输出里 —— 而那正是"看起来很像真的"的错误。
func printLedger(out *output, page signinLedgerPage) {
	// `nil` ≠ 0：上游没给，不是"一条都没有"。
	total := "上游没给"
	if page.total != nil {
		total = fmt.Sprintf("%d", *page.total)
	}
	out.human("  %s（aiDou=%d）：共 %s 条，本页 %d 条",
		page.label, page.book, total, len(page.records))
	if len(page.records) == 0 {
		// 空**不是错**：改名前的账本本来就只到 2026-07-30。
		out.human("    （本页没有记录）")
		return
	}
	for _, record := range page.records {
		out.human("    %s", compactJSON(record))
	}
	// ⚠ Rust 还会把「备份翻倍奖励」（`type == "backup_1"`）单独再列一遍。
	// 那一步**不在这里**：① 它是一次按字段名的筛选，属于渲染；
	// ② `records[]` 的字段名**没有实拍**，那个判据本身还是待确认的
	// （规格 §7 #5）。这里只把原样条目交出去。
}

// printPrizes 打待领取奖品那一页。
//
// # 只打认得出的字段
//
// `prizeName` / `insertTime` / `expireTime` 都有**实拍**，所以打它们。
// **过期时间才是要盯的那个** —— 奖品躺着会过期，而 H5 的列表只显示中奖时间。
//
// ⚠ Rust 还会把「原始条目」整份再列一遍（`item.raw`）—— Go 侧**刻意不打**：
// 原样条目里带 `userDomainId`（账号标识），而 [upstream.PrizeRecordItem]
// 就是为了不把它带出来才只留 typed 字段的（见 `app_signin_records.go:198-200`）。
// 安全优先于逐字对齐。
func printPrizes(out *output, page upstream.PrizeRecords) {
	if len(page.Records) == 0 {
		// 空**不是错**：待领取本来就常常是空的。
		out.human("  没有待领取的奖品。")
		return
	}
	out.human("  待领取奖品 %d 个：", len(page.Records))
	for _, item := range page.Records {
		name := "（上游没给奖品名）"
		if item.PrizeName != nil {
			name = *item.PrizeName
		}
		insert := "中奖时间未给"
		if item.InsertTime != nil {
			insert = "中奖 " + *item.InsertTime
		}
		expire := "过期时间未给"
		if item.ExpireTime != nil {
			expire = "过期 " + *item.ExpireTime
		}
		out.human("    %s（%s，%s）", name, insert, expire)
	}
}

// recordsPayload 是 `--records` 一个账号那个对象的形状。
//
// 两个账本各一个键（`cloud` 在前、`bean` 在后，与请求顺序一致）。
func recordsPayload(pages []signinLedgerPage) map[string]any {
	object := make(map[string]any, len(pages))
	for _, page := range pages {
		records := page.records
		if records == nil {
			// nil 切片会被编成 `null`，而上游没给时 Rust 那边给的是 `[]` ——
			// "一条都没有"与"这个字段不存在"是两件事。
			records = []any{}
		}
		object[page.key] = map[string]any{
			"aiDou":   page.book,
			"total":   page.total,
			"records": records,
		}
	}
	return object
}

// signinPrizesJSON 是 `--prizes --json` 那个对象的形状。
//
// ⚠ **typed 结构，不带 `Raw`**：Rust 把上游原样条目带出来（`item.raw`），
// 而那份条目里有 `userDomainId`（账号标识）。Go 侧的 [upstream.PrizeRecordItem]
// 就是**有意**只留 typed 字段的 —— 安全优先于逐字对齐（规格 §7 #3）。
//
// 键名用上游的字段名（camelCase），因为它们才是"这条数据在协议里的名字"。
type signinPrizesJSON struct {
	MarketName string            `json:"marketName"`
	PageSize   int64             `json:"pageSize"`
	Records    []signinPrizeJSON `json:"records"`
	Status     int64             `json:"status"`
}

// signinPrizeJSON 是奖品列表里的一条。
type signinPrizeJSON struct {
	ExpireTime *string `json:"expireTime"`
	Flag       *int64  `json:"flag"`
	InsertTime *string `json:"insertTime"`
	MarketName *string `json:"marketName"`
	Oid        *string `json:"oid"`
	PrizeID    *int64  `json:"prizeId"`
	PrizeName  *string `json:"prizeName"`
	PrizeType  *int64  `json:"type"`
}

// prizesPayload 把一页奖品翻成那个对象。
func prizesPayload(page upstream.PrizeRecords) signinPrizesJSON {
	records := make([]signinPrizeJSON, 0, len(page.Records))
	for _, item := range page.Records {
		records = append(records, signinPrizeJSON{
			PrizeName:  item.PrizeName,
			InsertTime: item.InsertTime,
			ExpireTime: item.ExpireTime,
			Flag:       item.Flag,
			PrizeID:    item.PrizeID,
			Oid:        item.Oid,
			MarketName: item.MarketName,
			PrizeType:  item.PrizeType,
		})
	}
	return signinPrizesJSON{
		MarketName: upstream.PrizeMarketName,
		Status:     upstream.PrizeStatusPending,
		PageSize:   prizePageSize,
		Records:    records,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// compactJSON 把一条原样条目编成一行。
//
// 用 [core.MarshalJSON] 而不是 `json.Marshal`：前者与 serde_json 同形
// （不转义 HTML），后者会把 `<` 写成 `\u003c`。编不出来时退回 `%v` ——
// 一条编不出来的条目不该让整次查询挂掉。
func compactJSON(value any) string {
	encoded, err := core.MarshalJSON(value)
	if err != nil {
		return fmt.Sprintf("%v", value)
	}
	return string(encoded)
}

// firstRunes 取前 n 个**字符**（不是字节）。
//
// ⚠ 按 rune 取：`deviceId` 是 base64，ASCII 时两者一样，
// 但一旦上游哪天塞个非 ASCII 进来，按字节切会切出半个字符。
func firstRunes(value string, n int) string {
	runes := []rune(value)
	if len(runes) <= n {
		return value
	}
	return string(runes[:n])
}

// optionalString 把"空串"落成 `nil`（Rust 的 `Option<String>` → `null`）。
func optionalString(value string) *string {
	if value == "" {
		return nil
	}
	return &value
}

// optionInt64 按 Rust 的 `{:?}` 形状打印一个 `Option<i64>`。
//
// 逐字对齐的是**输出文本**：`Some(0)` / `None` 都在冻结的 Rust 输出里出现过，
// 换成 `0` / `-` 会让两边的自检输出 diff 不起来。
func optionInt64(value *int64) string {
	if value == nil {
		return "None"
	}
	return fmt.Sprintf("Some(%d)", *value)
}

// optionGiven 是上面那个的 bool 版。
func optionGiven(given bool, whenTrue, whenFalse string) string {
	if given {
		return whenTrue
	}
	return whenFalse
}
