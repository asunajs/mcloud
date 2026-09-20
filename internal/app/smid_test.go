package app

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const smidAccount = "199****0001"

// ─────────────────────────────────────────────────────────────────────────────
// 假存储
// ─────────────────────────────────────────────────────────────────────────────

// smidStore 是 [core.StateStore] 的**部分假实现**。
//
// # 为什么用内嵌接口而不是手写 27 个方法
//
// [core.StateStore] 有 27 个方法；没重写的一被调到就会 panic —— 那正是要的：
// "本层碰了它不该碰的存储入口"立刻响，而不是静默返回零值（§6.1 #35）。
//
// ⚠ 它**复刻**了两条真实契约，不然后面的断言会空转：
//   - `SetSmidFingerprint` 写特征的同时**清空** `device_id`（换长相 = 换设备）；
//   - `SetSmidDeviceID` 只改 `device_id`，不动特征。
//
// ⚠ 它不是真库。`io` 那侧落地后要补一条用真实 SQLite 的用例 ——
// 这两条契约的真值在库里，不在这个 stub 里。
type smidStore struct {
	core.StateStore
	state *core.SmidState
	// writes 记下"写过什么"，按先后。
	writes []string
	// failDeviceID 让写 deviceId 失败，用来验"落库失败就丢掉 id"。
	failDeviceID bool
}

func (s *smidStore) GetSmidState(
	_ context.Context, _ core.Scope, _ core.AccountID,
) (*core.SmidState, error) {
	return s.state, nil
}

func (s *smidStore) SetSmidFingerprint(
	_ context.Context, _ core.Scope, id core.AccountID, fingerprintJSON string,
) error {
	s.writes = append(s.writes, "fp")
	secret := core.NewSecretValue(fingerprintJSON)
	s.state = &core.SmidState{AccountID: id, FingerprintJSON: &secret, DeviceID: nil}
	return nil
}

func (s *smidStore) SetSmidDeviceID(
	_ context.Context, _ core.Scope, id core.AccountID, deviceID string,
) error {
	if s.failDeviceID {
		return core.NewInternal("写库失败（夹具）")
	}
	s.writes = append(s.writes, "id")
	if s.state == nil {
		s.state = &core.SmidState{AccountID: id}
	}
	secret := core.NewSecretValue(deviceID)
	s.state.DeviceID = &secret
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 假上游
// ─────────────────────────────────────────────────────────────────────────────

// smidHTTP 按队列返回 `{"code":1100,"detail":{"deviceId":…}}`，并数自己被调了几次。
//
// 队列**空了就报错**而不是再给一个 id：那代表"这次不该打上游"，
// 让 `DeviceID` 那条"库里有就不联网"的断言真能变红。
type smidHTTP struct {
	ids   []string
	calls int
}

func (c *smidHTTP) Send(_ context.Context, _ core.HTTPRequest) (core.HTTPResponse, error) {
	c.calls++
	if len(c.ids) == 0 {
		return core.HTTPResponse{}, core.NewInternal("脚本已用完：这次不该再打上游")
	}
	raw := c.ids[0]
	c.ids = c.ids[1:]
	buf, err := core.MarshalJSON(map[string]any{
		"code": 1100, "detail": map[string]any{"deviceId": raw},
	})
	if err != nil {
		return core.HTTPResponse{}, err
	}
	return core.NewResponse(200, buf), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// smidNow 是 UTC 06:30 → 东八区 14:30（同一天）。
func smidNow() time.Time {
	return time.Date(2026, 9, 19, 6, 30, 0, 0, time.UTC)
}

func smidClock() core.Clock { return core.NewFixedClock(smidNow()) }

func smidScope() core.Scope {
	user := core.NewUserID("u-smid")
	return core.ScopeUser(&user)
}

func smidAccountID() core.AccountID { return core.NewAccountID(smidAccount) }

// ─────────────────────────────────────────────────────────────────────────────
// 时间
// ─────────────────────────────────────────────────────────────────────────────

// TestTimeDigitsIsBeijingWallClockWith14Digits：东八区墙钟，14 位纯数字。
func TestTimeDigitsIsBeijingWallClockWith14Digits(t *testing.T) {
	digits := BeijingTimeDigits(smidClock())
	if digits != "20260919143000" {
		t.Errorf("UTC 06:30 要看成同日 14:30，实得 %q", digits)
	}
	if len(digits) != 14 {
		t.Errorf("应恒为 14 位，实得 %d", len(digits))
	}
	for _, ch := range digits {
		if ch < '0' || ch > '9' {
			t.Fatalf("给上游的就是纯数字串，实得 %q", digits)
		}
	}
}

// TestTimeDigitsRollsTheDateAcrossUTCMidnight：跨 UTC 午夜那天必须按**东八区**算。
//
// 按 UTC 拼串会差一天，而特征体里 `timezone` 仍写着 -480 ——
// 等于自报"我在东八区，可我的日期是别处来的"。
func TestTimeDigitsRollsTheDateAcrossUTCMidnight(t *testing.T) {
	late := core.NewFixedClock(time.Date(2026, 9, 18, 23, 30, 0, 0, time.UTC))
	if got := BeijingTimeDigits(late); got != "20260919073000" {
		t.Errorf("东八区已是次日 07:30，实得 %q", got)
	}
}

// TestUploadContextCarriesThePreviousDeviceIDIntoBox：
// "取对了旧 id"这一半在本层断言（上报体是加密的，从请求里看不出来）。
func TestUploadContextCarriesThePreviousDeviceIDIntoBox(t *testing.T) {
	context := NewUploadContext(smidClock(), "B1990000000000001")
	if context.TimeDigits != "20260919143000" {
		t.Errorf("时间位是 %q", context.TimeDigits)
	}
	if context.NowMillis != smidNow().UnixMilli() {
		t.Errorf("nowMillis 应取时钟的毫秒，实得 %d", context.NowMillis)
	}
	if got := context.AsUpload().OldDeviceID; got != "B1990000000000001" {
		t.Errorf("旧 id 应原样带进 box，实得 %q", got)
	}
}

// TestBoxValueIsAnEmptyStringBeforeTheFirstReport：
// 首次上报的 `box` 是**空串**，不是 null 也不是省略字段。
func TestBoxValueIsAnEmptyStringBeforeTheFirstReport(t *testing.T) {
	binding := FingerprintBinding{
		Fingerprint: upstream.CaiyunAndroidWebFingerprint(),
		State:       core.NewSmidState(smidAccount),
		NewlyBound:  true,
	}
	if _, ok := binding.DeviceID(); ok {
		t.Error("还没上报过就不该有 id")
	}
	if got := binding.BoxValue(); got != "" {
		t.Errorf("首次上报的 box 应是空串，实得 %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 绑定与复用
// ─────────────────────────────────────────────────────────────────────────────

// TestFirstUseBindsTheFingerprintAndReusesItAfterwards：
// 绑定那一列一旦写下，换随机源再读也还是同一份。
func TestFirstUseBindsTheFingerprintAndReusesItAfterwards(t *testing.T) {
	store := &smidStore{}
	scope := smidScope()
	account := smidAccountID()

	first, perr := EnsureFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 7})
	if perr != nil {
		t.Fatalf("首次绑定失败: %v", perr)
	}
	if !first.NewlyBound {
		t.Error("库里原先没有，应算新绑")
	}
	if _, ok := first.DeviceID(); ok {
		t.Error("刚绑完还没上报，不该有 id")
	}
	stored, _ := first.Fingerprint.ToJSONString()
	if store.state == nil || store.state.FingerprintJSON.Value() != stored {
		t.Error("库里那份必须与生成那份逐字节相同，否则下次读出来是另一台设备")
	}

	// 换一个随机源再读：还是同一份特征 —— 绑定不会重新掷
	second, perr := EnsureFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 200})
	if perr != nil {
		t.Fatalf("复用失败: %v", perr)
	}
	if second.NewlyBound {
		t.Error("第二次读不该写成新绑")
	}
	if second.Fingerprint != first.Fingerprint {
		t.Error("复用路径上随机源换了，特征也不该变")
	}
}

// TestTheReusePathWritesNothing：复用那条路**一次写都不发**。
func TestTheReusePathWritesNothing(t *testing.T) {
	store := &smidStore{}
	scope := smidScope()
	account := smidAccountID()
	if _, perr := EnsureFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 1}); perr != nil {
		t.Fatalf("绑定失败: %v", perr)
	}
	before := len(store.writes)

	if _, perr := EnsureFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 99}); perr != nil {
		t.Fatalf("复用失败: %v", perr)
	}
	if len(store.writes) != before {
		t.Errorf("复用路径不该写任何东西，实得多出 %v", store.writes[before:])
	}
}

// TestRotationReplacesTheFingerprintAndVoidsTheDeviceID：
// 换特征 = 换一台设备，旧 id 当场作废。
//
// ⚠ "写特征同时清空 id"是 [core.StateStore.SetSmidFingerprint] 的契约
// （一条 SQL），本层**没有、也没法补**第二步 —— 所以这条断言验的是
// "换了之后读回来的那一列确实是空的"。
func TestRotationReplacesTheFingerprintAndVoidsTheDeviceID(t *testing.T) {
	store := &smidStore{}
	scope := smidScope()
	account := smidAccountID()
	old, perr := EnsureFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 1})
	if perr != nil {
		t.Fatalf("绑定失败: %v", perr)
	}
	if err := store.SetSmidDeviceID(context.Background(), scope, account, "B1990000000000001"); err != nil {
		t.Fatalf("写 id 失败: %v", err)
	}

	rotated, perr := RotateFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 50})
	if perr != nil {
		t.Fatalf("换特征失败: %v", perr)
	}
	if !rotated.NewlyBound {
		t.Error("换特征必须写成新绑")
	}
	if _, ok := rotated.DeviceID(); ok {
		t.Error("新长相还没有 id —— 旧 id 应已作废")
	}
	if rotated.Fingerprint == old.Fingerprint {
		t.Error("换特征必须真的换一份")
	}
	if store.state.DeviceID != nil {
		t.Error("换特征后库里的 device_id 应已清空")
	}
}

// TestAnUnparseableStoredFingerprintIsAnErrorNotAReroll：
// 库里那串读不出来要**报错**，不能悄悄重掷一份 ——
// 那等于把"这个账号换了一台设备"变成一次没有记录的事件。
func TestAnUnparseableStoredFingerprintIsAnErrorNotAReroll(t *testing.T) {
	broken := core.NewSecretValue(`{"ua":`)
	store := &smidStore{state: &core.SmidState{
		AccountID:       smidAccountID(),
		FingerprintJSON: &broken,
	}}
	before := len(store.writes)

	got, perr := EnsureFingerprint(context.Background(), store, smidScope(), smidAccountID(), &seqRandomApp{b: 3})
	if perr == nil {
		t.Fatalf("应报错；静默重掷会让账号悄悄换脸，实得 %+v", got)
	}
	if len(store.writes) != before {
		t.Error("报错时也不该写库")
	}
	// ⚠ 错误文案里**不回吐**库里那串：它就是这台设备的长相
	if strings.Contains(perr.Message(), "ua") {
		t.Errorf("报错文案不该回吐特征原文，实得 %q", perr.Message())
	}
	if !strings.Contains(perr.Message(), "reset") {
		t.Errorf("应给出可执行的下一步，实得 %q", perr.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 取 id 与上报
// ─────────────────────────────────────────────────────────────────────────────

// TestDeviceIDDoesNotTouchTheNetworkWhenItIsAlreadyKnown：
// 绑定省流量就省在这里 —— 库里有就直接返回，**一个请求都不发**。
func TestDeviceIDDoesNotTouchTheNetworkWhenItIsAlreadyKnown(t *testing.T) {
	store := &smidStore{}
	http := &smidHTTP{ids: []string{"1990000000000000001"}}
	up := upstream.NewUpstream(http)
	scope := smidScope()
	account := smidAccountID()

	if _, perr := DeviceID(context.Background(), up, store, scope, account, smidClock(), &seqRandomApp{b: 1}); perr != nil {
		t.Fatalf("第一次取 id 失败: %v", perr)
	}
	if http.calls != 1 {
		t.Fatalf("首次应打一次上游，实得 %d", http.calls)
	}

	again, perr := DeviceID(context.Background(), up, store, scope, account, smidClock(), &seqRandomApp{b: 9})
	if perr != nil {
		t.Fatalf("第二次取 id 失败: %v", perr)
	}
	if http.calls != 1 {
		t.Errorf("库里已有 id 就不该再打上游（脚本空了会报错），实得 %d 次", http.calls)
	}
	if again != "B1990000000000000001" {
		t.Errorf("应拿到拼好前缀的那个，实得 %q", again)
	}
}

// TestReportDeviceIDAlwaysUploadsEvenWhenAKnownIDIsStored：
// 换 id 那条路不看库里有没有 —— 每次都真的上报。
func TestReportDeviceIDAlwaysUploadsEvenWhenAKnownIDIsStored(t *testing.T) {
	store := &smidStore{}
	http := &smidHTTP{ids: []string{"1990000000000000001", "1990000000000000002"}}
	up := upstream.NewUpstream(http)
	scope := smidScope()
	account := smidAccountID()

	first, perr := ReportDeviceID(context.Background(), up, store, scope, account, smidClock(), &seqRandomApp{b: 1})
	if perr != nil {
		t.Fatalf("首次上报失败: %v", perr)
	}
	second, perr := ReportDeviceID(context.Background(), up, store, scope, account, smidClock(), &seqRandomApp{b: 1})
	if perr != nil {
		t.Fatalf("再次上报失败: %v", perr)
	}
	if http.calls != 2 {
		t.Errorf("ReportDeviceID 每次都应真的上报，实得 %d 次", http.calls)
	}
	if first == second {
		t.Errorf("两次应换到不同的 id，实得都是 %q", first)
	}
	if second != "B1990000000000000002" {
		t.Errorf("第二次应是队列里第二个，实得 %q", second)
	}
}

// TestTheOldIDGoesIntoTheNextUpload：`box` 带的是**上一次**那个 id，
// 让风控服务看出"还是那台机器在换 id"。
func TestTheOldIDGoesIntoTheNextUpload(t *testing.T) {
	store := &smidStore{}
	http := &smidHTTP{ids: []string{"1990000000000000001", "1990000000000000002"}}
	up := upstream.NewUpstream(http)
	scope := smidScope()
	account := smidAccountID()

	if _, perr := ReportDeviceID(context.Background(), up, store, scope, account, smidClock(), &seqRandomApp{b: 1}); perr != nil {
		t.Fatalf("首次上报失败: %v", perr)
	}
	binding, perr := EnsureFingerprint(context.Background(), store, scope, account, &seqRandomApp{b: 1})
	if perr != nil {
		t.Fatalf("读绑定失败: %v", perr)
	}
	if got := binding.BoxValue(); got != "B1990000000000000001" {
		t.Errorf("下一次上报的 box 应是上一次那个 id，实得 %q", got)
	}
	if got := NewUploadContext(smidClock(), binding.BoxValue()).AsUpload().OldDeviceID; got != "B1990000000000000001" {
		t.Errorf("旧 id 应穿到上游那一层，实得 %q", got)
	}
}

// TestAStoredIDIsReturnedOnlyAfterItIsStored：
// 落库失败就**丢掉**这个刚换来的 id，不返回 ——
// 反过来会留下"用过了但库里没有"的状态，那更难查。
func TestAStoredIDIsReturnedOnlyAfterItIsStored(t *testing.T) {
	store := &smidStore{failDeviceID: true}
	http := &smidHTTP{ids: []string{"1990000000000000001"}}
	up := upstream.NewUpstream(http)

	got, perr := DeviceID(context.Background(), up, store, smidScope(), smidAccountID(),
		smidClock(), &seqRandomApp{b: 1})
	if perr == nil {
		t.Fatalf("写库失败时应报错，实得 %q", got)
	}
	if got != "" {
		t.Errorf("没落库的 id 不该交出去，实得 %q", got)
	}
	if http.calls != 1 {
		t.Errorf("上游确实打过了（id 是浪费掉一个，不是没取），实得 %d 次", http.calls)
	}
	if store.state != nil && store.state.DeviceID != nil {
		t.Error("写失败的 id 不该留在库里")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具：随机源
// ─────────────────────────────────────────────────────────────────────────────

// seqRandomApp 是确定性随机源（递增字节）。
type seqRandomApp struct{ b byte }

func (r *seqRandomApp) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.b
		r.b++
	}
}
