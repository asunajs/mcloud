package cli

import (
	"fmt"
	"time"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"

	"github.com/catlair/mcloud/internal/capabilities"
)

// 默认路径。Rust 侧这些是 clap 的 `default_value` 字面量，抄过来的理由只有一个：
// 用户在 Rust 与 Go 之间切换时，**同一条命令必须指向同一个库文件**。
const (
	defaultDB            = "data/mcloud.db"
	defaultDeviceFile    = "data/device.txt"
	defaultAppDeviceFile = "data/device-app.txt"
	defaultLegacyBackup  = "data/legacy-source"
	defaultRepoRoot      = "."
	defaultFixtureHarDir = "har"
	defaultFixtureOutDir = "test/fixtures/http"
	defaultUserAdminName = "admin"
)

// SigninArgs 是签到（云朵中心）的参数。
//
// 对应 `legacy/crates/mcloud-cli/src/args.rs` 的 `SigninArgs`。
// 根命令与 `signin` 子命令共用这一份定义（见 [registerSigninFlags] 的注释）。
//
// # 三态字段怎么办
//
// Rust 用 `Option<T>` 表达"没给"，Go 的零值不行：`--account 0` 与"没给 `--account`"
// 都是 0。所以凡 Rust 是 Option 且**零值有意义**的字段，一律走
// [SigninArgs.given]（问 pflag 这个 flag 有没有被设置过），不去给零值编第二种语义。
type SigninArgs struct {
	Account       int
	List          bool
	Phone         string
	DB            string
	AppDeviceFile string
	DeviceShort   string
	DeviceID      string
	Receive       bool
	CloudID       string
	CloudType     int64
	Revive        bool
	Multiple      bool
	StatusOnly    bool
	Raw           bool
	Records       bool
	RecordsAll    bool
	Prizes        bool
	BackupGift    bool
	Force         bool
	JSON          bool

	flags *pflag.FlagSet
}

// given 报告某个 flag 是否被**显式**给出（给了默认值不算）。
func (a *SigninArgs) given(name string) bool {
	return a.flags != nil && a.flags.Changed(name)
}

// accountRequested 把"没给"与"给了 0"分开 —— 后者是用户的错，不是"跑全部"。
func (a *SigninArgs) accountRequested() (int, bool) {
	if !a.given("account") {
		return 0, false
	}
	return a.Account, true
}

// registerSigninFlags 注册签到那套参数。
//
// ⚠ 这个函数是**唯一**一份定义：根命令与 `signin` 各调一次，共享同一个结构体实例
// 之外的默认值/帮助文本/互斥关系。想加一个开关只改这里。
func registerSigninFlags(fs *pflag.FlagSet, a *SigninArgs) {
	a.flags = fs

	fs.IntVar(&a.Account, "account", 0,
		"账号**下标**（从 1 开始，就是 --list 打出来的那个编号）。不给 = 全部启用账号。"+
			"⚠ 下标是位置不是身份：中间插一个账号会让后面的编号平移，定时任务里别用它")
	fs.BoolVar(&a.List, "list", false,
		"打印账号清单（含下标）后退出 —— 用它查 --account 该填几")
	fs.StringVar(&a.Phone, "phone", "",
		"完整手机号（11 位），**不是**掩码 id。只给 v6 之前登录、库里没有整串凭据的老账号用；"+
			"跑完会把号码记进库里")
	fs.StringVar(&a.DB, "db", defaultDB, "SQLite 文件")
	fs.StringVar(&a.AppDeviceFile, "app-device-file", defaultAppDeviceFile,
		"仅 App：deviceId 的存放位置（32 位大写 HEX，无后缀）。必须与登录时同一个")
	fs.StringVar(&a.DeviceShort, "device-short", "",
		"仅 App：固定设备串第 7 段（12 位大写 HEX）。不传则每次现生成")
	fs.StringVar(&a.DeviceID, "device-id", "",
		"H5 那个 deviceId（89 字符 base64、B 前缀）。一般不用传：默认从库里的 "+
			"smid_device_id 取，没有就现上报一次换")
	// clap 的 `visible_alias = "h5-device-id"`。pflag 没有别名机制，
	// 这里用"第二个 flag 绑到同一个变量"达到同样效果：两者等价、后者覆盖前者。
	// 之所以要留着这个名字：旧文档与脚本里 `--h5-device-id` 出现过。
	fs.StringVar(&a.DeviceID, "h5-device-id", "", "（--device-id 的别名）")
	fs.BoolVar(&a.Receive, "receive", false,
		"签到之后领云朵：默认按 infoV3 的账目自动分流。⚠ 这是写请求")
	fs.StringVar(&a.CloudID, "cloud-id", "",
		"领取的 cloudId（infoV3 的 receiveList[].recordId），默认空串。只在显式模式下有用")
	fs.Int64Var(&a.CloudType, "cloud-type", 0,
		"显式指定 cloudType（**自检用，日常不要给**）：不给=按账目自动分流，"+
			"给了=只发这一笔。⚠ 别拿 2 去试，那是下月可领，发出去只拿到 +0")
	fs.BoolVar(&a.Revive, "revive", false,
		"用掉一张复活卡/回馈卡。⚠⚠ 写请求且**不可逆**：H5 原话「关闭弹窗视为放弃机会，无法补发」")
	fs.BoolVar(&a.Multiple, "multiple", false,
		"领签到翻倍（自动备份每月那一次机会）。门槛由代码自己判，不成立就一个请求都不发")
	fs.BoolVar(&a.StatusOnly, "status-only", false, "只查状态，不签到")
	fs.BoolVar(&a.Raw, "raw", false,
		"把 infoV3 的原始响应信封原样打出来。⚠ 输出带账号标识，别直接贴到公开的地方")
	fs.BoolVar(&a.Records, "records", false,
		"查豆的收支流水（cloudRecordV2），两个账本各打一页。⚠ 字段名没有实拍，原样打印")
	fs.BoolVar(&a.RecordsAll, "records-all", false,
		"--records 把两个账本翻完（默认只取最近 50 条）。只读，但输出几百行")
	fs.BoolVar(&a.Prizes, "prizes", false,
		"查待领取奖品（record 的「待领取」页签）。只读。⚠ 条目带账号标识")
	fs.BoolVar(&a.BackupGift, "backup-gift", false,
		"领备份好礼（backupgift/receive）。⚠ 写请求，每月一次，门槛不成立就一个请求都不发")
	fs.BoolVar(&a.Force, "force", false,
		"今天已签也发一次 startSignIn。**用途只有协议自检**，日常签到别开")
	fs.BoolVar(&a.JSON, "json", false, "以 JSON 输出（脚本 / 定时任务消费）")
}

// signinExclusive 是 clap 那张 conflicts_with_all 表的对位。
//
// ⚠ 为什么是"逐对"而不是"一个大组"：`--receive` 与 `--multiple` 在 Rust 侧
// **可以一起给**（旧项目就是把它们分成两步），把它们塞进同一个互斥组会
// 直接砍掉一条受支持的用法。所以这里只列真正互斥的那些对。
var signinExclusive = [][]string{
	{"list", "account"},
	{"list", "phone"},

	{"receive", "status-only"},
	{"revive", "status-only"},
	{"multiple", "status-only"},
	{"backup-gift", "status-only"},

	{"raw", "receive"},
	{"raw", "revive"},
	{"raw", "multiple"},
	{"raw", "backup-gift"},
	{"raw", "prizes"},

	{"records", "receive"},
	{"records", "revive"},
	{"records", "multiple"},
	{"records", "backup-gift"},
	{"records", "raw"},
	{"records", "prizes"},

	{"prizes", "receive"},
	{"prizes", "revive"},
	{"prizes", "multiple"},
	{"prizes", "backup-gift"},
}

// signinRequires 是 clap 的 `requires`：给了前者就必须给后者。
//
// cobra 只有 `MarkFlagsMutuallyExclusive` / `MarkFlagsOneRequired` /
// `MarkFlagsRequiredTogether`，**没有**单向的 requires ——
// RequiredTogether 会把"只给 --records 不给 --records-all"也判错，
// 而那是 Rust 侧完全合法的一条命令。所以这条留给 [SigninArgs.validate] 手工判。
var signinRequires = [][2]string{
	{"records-all", "records"},
}

// applySigninGroups 把上面两张表装进命令。
func applySigninGroups(cmd *cobra.Command, a *SigninArgs) {
	for _, pair := range signinExclusive {
		cmd.MarkFlagsMutuallyExclusive(pair[0], pair[1])
	}
	// 表是数据，名字写错了不会编译失败 —— 这里让它当场响。
	for _, pair := range signinRequires {
		for _, name := range pair {
			if a.flags.Lookup(name) == nil {
				panic("cli: signinRequires 引用了不存在的 flag: " + name)
			}
		}
	}
}

// validate 跑"解析期之后、动手之前"的用法校验。
//
// 错误一律带 [ErrUsage]：这些是"调用写错了"，退出码要和真正的运行错误分开。
func (a *SigninArgs) validate() error {
	for _, rule := range signinRequires {
		if a.given(rule[0]) && !a.given(rule[1]) {
			return fmt.Errorf("--%s 需要同时给 --%s: %w", rule[0], rule[1], ErrUsage)
		}
	}
	return nil
}

// newCapabilitiesCommand 打印能力注册表。
//
// WebUI 的表单与 CLI 帮助**同源**（都读 capabilities 注册表），
// 所以这里就是契约：两边不会说出不一样的能力清单。
func newCapabilitiesCommand(deps *Deps, out *output) *cobra.Command {
	var asJSON bool
	cmd := &cobra.Command{
		Use:   "capabilities",
		Short: "打印能力注册表（WebUI 表单与 CLI 帮助同源，故此处即契约）",
		RunE: func(*cobra.Command, []string) error {
			return runCapabilities(out, asJSON)
		},
	}
	cmd.Flags().BoolVar(&asJSON, "json", false, "输出原始 JSON（前端/脚本消费用）")
	return cmd
}

// newVersionCommand 打印版本与运行时信息。
func newVersionCommand(deps *Deps, out *output) *cobra.Command {
	var asJSON bool
	cmd := &cobra.Command{
		Use:   "version",
		Short: "打印版本与运行时信息",
		RunE: func(*cobra.Command, []string) error {
			return runVersion(out, asJSON)
		},
	}
	cmd.Flags().BoolVar(&asJSON, "json", false,
		"以 JSON 输出（与 GET /api/version 的响应体一致）")
	return cmd
}

// qrTTLSeconds 的默认值取注册表里那份，**不在 CLI 再写一个 120**：
// 那会变成第二份真相，改了那边不改这里就是"码过期时间两边不一样"。
//
// ⚠ 秒是**用户单位**（`--ttl-seconds 600` 是给人看的），能力层存的是
// [time.Duration] —— 换算只发生在边界上这一次，不渗进内部类型。
func qrTTLSeconds() int64 { return int64(capabilities.QRTTL / time.Second) }
