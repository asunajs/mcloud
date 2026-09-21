package cli

import (
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

// QrArgs 是 `login qr` 的参数（对位 `args.rs` 的 `QrArgs`）。
type QrArgs struct {
	DB         string
	Platform   string
	DeviceFile string
	TTLSeconds int64
	QROut      string
	NoWait     bool
}

// SmsArgs 是 `login sms` 的参数。
//
// ⚠ 这里有一条不能抹平的分界：`--code` **可以**从命令行给，口令**永远不能**。
// 短信验证码单次、分钟级、用过即废，非交互提交它是自动化与远程协助的前提；
// 口令长期有效、会留在 shell 历史与 `ps` 里，所以任何 `--password` 都不许出现。
type SmsArgs struct {
	Phone         string
	DB            string
	Client        string
	Platform      string
	DeviceFile    string
	AppDeviceFile string
	SendOnly      bool
	Code          string
	DeviceShort   string

	flags *pflag.FlagSet
}

func (a *SmsArgs) given(name string) bool { return a.flags != nil && a.flags.Changed(name) }

// newLoginCommand 建 `login`（扫码 / 短信）。
func newLoginCommand(deps *Deps, out *output) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "login",
		Short: "登录移动云盘账号（扫码 / 短信）",
	}

	qr := &QrArgs{}
	qrCmd := &cobra.Command{
		Use:   "qr",
		Short: "扫码登录：终端里画出二维码，用手机扫",
		RunE: func(*cobra.Command, []string) error {
			return runLoginQR(deps, out, qr)
		},
	}
	qrFlags := qrCmd.Flags()
	qrFlags.StringVar(&qr.DB, "db", defaultDB, "SQLite 文件")
	qrFlags.Var(newEnumFlag(&qr.Platform, platformValues...), "platform",
		"冒充哪个 PC 平台。两端的产品串不同，混搭是上游一眼可辨的矛盾组合")
	qrFlags.StringVar(&qr.DeviceFile, "device-file", defaultDeviceFile,
		"设备指纹文件。**换掉它等于换一台设备**：服务端按 dID + sID 定位会话")
	qrFlags.Int64Var(&qr.TTLSeconds, "ttl-seconds", qrTTLSeconds(),
		"二维码有效期（秒）。把码发给别人扫、或先贴出码再拿手机时调大")
	qrFlags.StringVar(&qr.QROut, "qr-out", "",
		"额外把二维码写成图片（SVG）。终端等宽不保证，字符画经常变形扫不出来")
	qrFlags.BoolVar(&qr.NoWait, "no-wait", false, "不等用户扫，只出码就退出（供调试）")

	sms := &SmsArgs{}
	smsCmd := &cobra.Command{
		Use:   "sms",
		Short: "短信验证码登录",
		RunE: func(*cobra.Command, []string) error {
			return runLoginSms(deps, out, sms)
		},
	}
	smsFlags := smsCmd.Flags()
	smsFlags.StringVar(&sms.Phone, "phone", "", "手机号（必填，11 位）")
	smsFlags.StringVar(&sms.DB, "db", defaultDB, "SQLite 文件")
	smsFlags.Var(newEnumFlag(&sms.Client, clientValues...), "client",
		"冒充 PC 还是手机 App。**必须显式选**：请求体是否加密、滑块归谁发、"+
			"Authorization 前缀都不一样")
	smsFlags.Var(newEnumFlag(&sms.Platform, platformValues...), "platform",
		"仅 PC：clientType / cpid / 版本号")
	smsFlags.StringVar(&sms.DeviceFile, "device-file", defaultDeviceFile,
		"PC 设备指纹文件。**换掉它等于换一台设备**")
	smsFlags.StringVar(&sms.AppDeviceFile, "app-device-file", defaultAppDeviceFile,
		"仅 App：deviceId 的存放位置（32 位大写 HEX，无后缀。形状与 PC 那份不同）")
	smsFlags.BoolVar(&sms.SendOnly, "send-only", false,
		"只发码、不进入输验证码这一步就退出。与 --code 互斥：码已到手时没有「只发码」这回事")
	smsFlags.StringVar(&sms.Code, "code", "",
		"直接给出验证码，**同时跳过发码**（再发一次会白刷今日配额、还要连带过一次滑块）")
	smsFlags.StringVar(&sms.DeviceShort, "device-short", "",
		"仅 App：固定设备串第 7 段（12 位大写 HEX）。不传每次现生成；"+
			"--send-only 会把当次生成的值打出来，原样传回来即可")
	sms.flags = smsFlags
	// clap 的 `conflicts_with = "code"`。
	smsCmd.MarkFlagsMutuallyExclusive("send-only", "code")

	cmd.AddCommand(qrCmd, smsCmd)
	return cmd
}

// newSmidCommand 建 `smid show|reset|refresh`。
//
// 输出**只给摘要**：deviceId 是风控服务给这台设备的标识，canvas 那段是特征的
// 识别位，都不该整份打到终端（脱敏口径见 core.SmidState）。
func newSmidCommand(deps *Deps, out *output) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "smid",
		Short: "查看 / 更换账号绑定的设备特征（smid），以及它换来的 deviceId",
	}

	var showDB string
	show := &cobra.Command{
		Use:   "show",
		Short: "列出每个账号绑定的设备特征与最近换到的 deviceId",
		RunE: func(*cobra.Command, []string) error {
			return notMigrated("smid show")
		},
	}
	show.Flags().StringVar(&showDB, "db", defaultDB, "SQLite 文件")

	var (
		resetAccount string
		resetDB      string
		resetReport  bool
	)
	reset := &cobra.Command{
		Use:   "reset",
		Short: "更换某个账号的设备特征：新特征落库，旧的 deviceId 当场作废",
		RunE: func(*cobra.Command, []string) error {
			return notMigrated("smid reset")
		},
	}
	resetFlags := reset.Flags()
	resetFlags.StringVar(&resetAccount, "account", "",
		"账号 id（`mcloud smid show` 里那一列，形如 19****01）")
	resetFlags.StringVar(&resetDB, "db", defaultDB, "SQLite 文件")
	resetFlags.BoolVar(&resetReport, "report", false,
		"换完立刻上报一次，把新的 deviceId 也拿到手。"+
			"不传就等到下一次真正用到它时再报（少一次无谓的第三方请求）")

	var (
		refreshAccount string
		refreshDB      string
	)
	refresh := &cobra.Command{
		Use:   "refresh",
		Short: "不换特征，只再上报一次换一个新的 deviceId",
		RunE: func(*cobra.Command, []string) error {
			return notMigrated("smid refresh")
		},
	}
	refreshFlags := refresh.Flags()
	refreshFlags.StringVar(&refreshAccount, "account", "", "账号 id")
	refreshFlags.StringVar(&refreshDB, "db", defaultDB, "SQLite 文件")

	cmd.AddCommand(show, reset, refresh)
	return cmd
}
