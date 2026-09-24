// [ArtifactLedger] 的用例。
//
// 这一层的判据全部指向同一件事：**用户网盘上留下的东西不能没人管**。
// 所以每条用例都问一个反向问题 —— 把它改坏，用户会看见什么？
//   - 登记不幂等 ⇒ "记了 3 条、其实只有 2 个文件"，清理计数说谎；
//   - 不完整登记被放过 ⇒ 那个文件永远没人删（漏记）；
//   - `exist:true` 也登记 ⇒ 清理删掉用户原本就有的目录（错删，唯一的真实成因）；
//   - 不按账号切 ⇒ 用 A 的凭据去删 B 的 ID，上游拒绝 ⇒ 又变回漏记；
//   - 不排序 ⇒ 同一份账两次读出不同顺序，清单钉不住、日志永远在 diff。
//
// 账本本身纯内存，一个请求都不发（清理那条请求至今零实拍，见 artifacts.go 文件头）。
package mcloudexec

import (
	"errors"
	"fmt"
	"sync"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// artifactA/B 是两个账号（掩码形态，与 `docs/sanitize-guide.md` 一致：不带尾号）。
var (
	artifactA = core.NewAccountID("19****01")
	artifactB = core.NewAccountID("19****02")
)

// cloudFile 造一条网盘产物登记，本文件所有用例都从它出发改自己那一格。
func cloudFile(account core.AccountID, id string) Artifact {
	return Artifact{Kind: ArtifactCloudFile, Account: account, ID: id}
}

// oneArtifactRun 给出一个 runId（借用 exec 侧那台 splitmix64 随机源）。
func oneArtifactRun(t *testing.T) core.RunID {
	t.Helper()
	return core.NewRunID(newSeededRng())
}

func TestArtifactLedgerRegistrationIsIdempotent(t *testing.T) {
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)
	art := cloudFile(artifactA, "file-1")

	for range 3 {
		if err := ledger.Register(runID, art); err != nil {
			t.Fatalf("重复登记应幂等，实得 %v", err)
		}
	}
	if got := ledger.Count(runID); got != 1 {
		t.Fatalf("同一个 ID 记三次仍是一条产物，实得 %d 条 —— 内层退化成切片就会这样", got)
	}

	// 换一个 ID 才是第二条：判据是「三元组」而不是"有登记"。
	if err := ledger.Register(runID, cloudFile(artifactA, "file-2")); err != nil {
		t.Fatalf("登记第二条失败：%v", err)
	}
	if got := ledger.Count(runID); got != 2 {
		t.Fatalf("应有 2 条，实得 %d", got)
	}
}

func TestArtifactLedgerRejectsIncompleteRegistrations(t *testing.T) {
	cases := []struct {
		name  string
		runID core.RunID
		art   Artifact
	}{
		{"缺 runId", "", cloudFile(artifactA, "file-1")},
		{"缺类型", oneArtifactRun(t), Artifact{Account: artifactA, ID: "file-1"}},
		{"缺账号", oneArtifactRun(t), Artifact{Kind: ArtifactCloudFile, ID: "file-1"}},
		{"缺 ID", oneArtifactRun(t), Artifact{Kind: ArtifactCloudFile, Account: artifactA}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			ledger := NewArtifactLedger()
			err := ledger.Register(tc.runID, tc.art)
			if err == nil {
				t.Fatalf("%s 的登记必须响 —— 放过它就等于永久漏记", tc.name)
			}
			var coreErr *core.Error
			if !errors.As(err, &coreErr) {
				t.Fatalf("应是 *core.Error（参数不完整是调用方写错），实得 %T", err)
			}
			if coreErr.Kind() != core.KindBadRequest {
				t.Errorf("应归 BadRequest，实得 %v", coreErr.Kind())
			}
			// 响过之后**不能**留下半条账：否则"报错了"与"记上了"会同时成立。
			if got := ledger.Count(tc.runID); got != 0 {
				t.Errorf("被拒的登记不该入账，实得 %d 条", got)
			}
		})
	}
	// 反向盯一下 `Valid()`：三个字段各自少一个都要能被判出来。
	if (Artifact{}).Valid() {
		t.Error("零值产物不该算完整")
	}
	if !cloudFile(artifactA, "x").Valid() {
		t.Error("三格齐全的产物应判完整")
	}
}

func TestRegisterCreatedSkipsPreExistingObjects(t *testing.T) {
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)

	// `exist:true`（秒传命中 / `fileRenameMode:"refuse"` 撞上已有目录）⇒ 不是本 run 造的。
	added, err := ledger.RegisterCreated(runID, cloudFile(artifactA, "用户原有目录"), true)
	if err != nil {
		t.Fatalf("已存在不是错误，实得 %v", err)
	}
	if added {
		t.Error("已存在的对象不该记进本 run 账上 —— 记了就等着被清理删掉")
	}
	if got := ledger.Count(runID); got != 0 {
		t.Fatalf("账上应为空，实得 %d 条", got)
	}

	// `exist:false` 才登记，并且要能报出"这次真的记上了"。
	added, err = ledger.RegisterCreated(runID, cloudFile(artifactA, "新传的"), false)
	if err != nil || !added {
		t.Fatalf("新造的对象必须登记成功，实得 added=%v err=%v", added, err)
	}
	if got := ledger.Count(runID); got != 1 {
		t.Fatalf("应有 1 条，实得 %d", got)
	}

	// 入参校验走同一条路：`preExisting` 为假**不代表**这条登记就完整。
	if _, err := ledger.RegisterCreated(runID, Artifact{Account: artifactA, ID: "没类型"}, false); err == nil {
		t.Error("RegisterCreated 也要拦不完整的登记")
	}
}

func TestArtifactLedgerPendingIsDeterministicallyOrdered(t *testing.T) {
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)
	// 故意**逆序**登记，让"读出来的顺序"只能来自排序而不是插入顺序。
	for _, art := range []Artifact{
		cloudFile(artifactB, "b-2"),
		cloudFile(artifactA, "a-9"),
		{Kind: "cloud-note", Account: artifactA, ID: "note-1"},
		cloudFile(artifactA, "a-1"),
	} {
		if err := ledger.Register(runID, art); err != nil {
			t.Fatalf("登记 %s 失败：%v", art, err)
		}
	}

	want := []string{
		"cloud-file#19****01/a-1",
		"cloud-file#19****01/a-9",
		"cloud-note#19****01/note-1",
		"cloud-file#19****02/b-2",
	}
	// 反复读：map 的遍历顺序是**故意随机化**的，只读一次可能刚好撞对。
	for i := range 200 {
		got := ledger.Pending(runID)
		if len(got) != len(want) {
			t.Fatalf("应读出 %d 条，实得 %d", len(want), len(got))
		}
		for n, art := range got {
			if art.String() != want[n] {
				t.Fatalf("第 %d 次读取的第 %d 条是 %s，应为 %s（排序漏了就随遍历顺序漂）",
					i, n, art, want[n])
			}
		}
	}
}

func TestArtifactLedgerSplitsPendingByAccount(t *testing.T) {
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)
	for _, art := range []Artifact{
		cloudFile(artifactA, "a-1"),
		cloudFile(artifactB, "b-1"),
		cloudFile(artifactA, "a-2"),
	} {
		if err := ledger.Register(runID, art); err != nil {
			t.Fatalf("登记失败：%v", err)
		}
	}

	// 按账号切：清理时用谁的凭据，就只能拿谁名下的 ID。
	for _, tc := range []struct {
		account core.AccountID
		want    []string
	}{
		{artifactA, []string{"a-1", "a-2"}},
		{artifactB, []string{"b-1"}},
		{core.NewAccountID("19****03"), nil},
	} {
		got := ledger.PendingFor(runID, tc.account)
		if len(got) != len(tc.want) {
			t.Fatalf("账号 %s 名下应有 %d 条，实得 %d：%v", tc.account, len(tc.want), len(got), got)
		}
		for n, art := range got {
			if art.ID != tc.want[n] || art.Account != tc.account {
				t.Errorf("第 %d 条串了账号：%s", n, art)
			}
		}
	}

	// 总清单仍是全部（`PendingFor` 不得顺手改账）。
	if got := ledger.Count(runID); got != 3 {
		t.Fatalf("按账号读不该改动账本，实得 %d 条", got)
	}
}

func TestMarkRemovedNeedsTheExactTriple(t *testing.T) {
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)
	registered := cloudFile(artifactA, "file-1")
	if err := ledger.Register(runID, registered); err != nil {
		t.Fatalf("登记失败：%v", err)
	}

	// 同 ID、换账号 ⇒ 销不掉。这一条是"别拿 B 的凭据去销 A 的账"的落点。
	if ledger.MarkRemoved(runID, cloudFile(artifactB, "file-1")) {
		t.Error("账号对不上不该算销账")
	}
	// 同 ID 同账号、换类型 ⇒ 也销不掉（将来按类型分流清理时会漏一条）。
	if ledger.MarkRemoved(runID, Artifact{Kind: "cloud-note", Account: artifactA, ID: "file-1"}) {
		t.Error("类型对不上不该算销账")
	}
	if got := ledger.Count(runID); got != 1 {
		t.Fatalf("两次错销都不该动账，实得 %d 条", got)
	}

	if !ledger.MarkRemoved(runID, registered) {
		t.Fatal("三元组齐全应销账成功")
	}
	if got := ledger.Count(runID); got != 0 {
		t.Fatalf("销账后应空，实得 %d", got)
	}
	// 再销一次返回 false 而不是 panic / error：清理重试是合法路径。
	if ledger.MarkRemoved(runID, registered) {
		t.Error("重复销账该报 false（调用方要把它当成不一致报出来，而不是当成删好了）")
	}
	// 未跟踪的 run 也走同一条答案。
	if ledger.MarkRemoved(oneArtifactRun(t), registered) {
		t.Error("没跟踪过的 run 里没有这条账")
	}
}

func TestArtifactLedgerIsScopedPerRun(t *testing.T) {
	ledger := NewArtifactLedger()
	first, second := twoRunIDs(t)
	art := cloudFile(artifactA, "file-1")
	if err := ledger.Register(first, art); err != nil {
		t.Fatalf("登记失败：%v", err)
	}
	if got := ledger.Count(second); got != 0 {
		t.Fatalf("另一个 run 不该看到别人的产物，实得 %d", got)
	}
	if len(ledger.Pending(second)) != 0 {
		t.Error("另一个 run 的清单应为空")
	}
	if ledger.MarkRemoved(second, art) {
		t.Error("不能拿别人的 runId 销账")
	}
	if got := ledger.Count(first); got != 1 {
		t.Fatalf("别人的 run 账目不该被动过，实得 %d", got)
	}
}

func TestForgetHandsBackLeftovers(t *testing.T) {
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)
	for _, id := range []string{"b", "a"} {
		if err := ledger.Register(runID, cloudFile(artifactA, id)); err != nil {
			t.Fatalf("登记失败：%v", err)
		}
	}

	// 销一条，剩下那条必须由 Forget 交回来 —— 账本没了不等于文件没了。
	if !ledger.MarkRemoved(runID, cloudFile(artifactA, "a")) {
		t.Fatal("先销掉 a")
	}
	leftovers := ledger.Forget(runID)
	if len(leftovers) != 1 || leftovers[0].ID != "b" {
		t.Fatalf("Forget 应把未清理的 1 条交回，实得 %v", leftovers)
	}
	if got := ledger.Count(runID); got != 0 {
		t.Fatalf("Forget 之后不再跟踪，实得 %d", got)
	}
	if ids := ledger.Runs(); len(ids) != 0 {
		t.Fatalf("账本应空，实得 %v", ids)
	}
	// 已清空时交回 nil：调用方据此判断"真的一点没留"。
	if got := ledger.Forget(runID); got != nil {
		t.Errorf("未跟踪的 run 该交回 nil，实得 %v", got)
	}

	// 全清干净再 Forget ⇒ nil，这是清理路径的正常终点。
	other := oneArtifactRun(t)
	if err := ledger.Register(other, cloudFile(artifactA, "c")); err != nil {
		t.Fatalf("登记失败：%v", err)
	}
	ledger.MarkRemoved(other, cloudFile(artifactA, "c"))
	if got := ledger.Forget(other); got != nil {
		t.Errorf("清空后再 Forget 不该报剩余，实得 %v", got)
	}
}

func TestRunsListsTrackedRunsInOrder(t *testing.T) {
	ledger := NewArtifactLedger()
	first, second := twoRunIDs(t)
	for _, id := range []core.RunID{second, first} {
		if err := ledger.Register(id, cloudFile(artifactA, fmt.Sprintf("file-%s", id))); err != nil {
			t.Fatalf("登记失败：%v", err)
		}
	}
	ids := ledger.Runs()
	if len(ids) != 2 || ids[0] > ids[1] {
		t.Fatalf("应给出 2 个且按字典序，实得 %v", ids)
	}
}

func TestArtifactLedgerIsSafeUnderConcurrency(t *testing.T) {
	// 门禁 #4 带 `-race`：这条用例的真正价值是那个竞态报告，而不是最后的计数。
	ledger := NewArtifactLedger()
	runID := oneArtifactRun(t)
	const accounts = 8

	var wg sync.WaitGroup
	for i := range accounts {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			art := cloudFile(core.NewAccountID(fmt.Sprintf("19****%02d", n)), "same-id")
			if err := ledger.Register(runID, art); err != nil {
				t.Errorf("并发登记失败：%v", err)
			}
			_ = ledger.Pending(runID)
			_ = ledger.PendingFor(runID, art.Account)
		}(i)
	}
	wg.Wait()

	// 同一个 ID 挂在不同账号名下是**不同**的产物（凭据不同），一条都不该被合并掉。
	if got := ledger.Count(runID); got != accounts {
		t.Fatalf("应有 %d 条（按账号各一条），实得 %d", accounts, got)
	}
}

func TestZeroValueArtifactLedgerIsUnusable(t *testing.T) {
	// 与包内另两个类型一致：零值的内层 map 是 nil，**写**会 panic、读不会。
	// 这条用例钉住"必须走构造函数"这句注释不是客套 —— 反向：哪天给零值加了懒初始化，
	// 这里会读到 0 而不是 panic，就该把包注释一并改掉。
	ledger := &ArtifactLedger{}
	defer func() {
		if recover() == nil {
			t.Error("零值账本的写入应当 panic（构造函数是唯一入口）")
		}
	}()
	_ = ledger.Register(oneArtifactRun(t), cloudFile(artifactA, "file-1"))
}

func TestArtifactStringCarriesNoCredentials(t *testing.T) {
	// 账本条目会进日志与 run 事件。这里断言它只有三段、且不含任何凭据形状的东西。
	text := cloudFile(artifactA, "file-1").String()
	if text != "cloud-file#19****01/file-1" {
		t.Errorf("诊断文本形状变了：%s", text)
	}
	for _, forbidden := range []string{"Basic ", "authToken", "jwttoken", "|1|RCS|"} {
		if containsString(text, forbidden) {
			t.Errorf("诊断文本里出现了 %q —— 它会进日志", forbidden)
		}
	}
}

// containsString 是个小工具：本文件只需要"含不含"，不想为此拉 strings。
func containsString(haystack, needle string) bool {
	for i := 0; i+len(needle) <= len(haystack); i++ {
		if haystack[i:i+len(needle)] == needle {
			return true
		}
	}
	return false
}
