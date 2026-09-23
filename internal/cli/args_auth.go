package cli

import (
	"github.com/spf13/cobra"
)

// AuthRefreshArgs 是 `auth refresh` 的参数。
type AuthRefreshArgs struct {
	Account       string
	All           bool
	DB            string
	LeadDays      float64
	Force         bool
	Platform      string
	AppDeviceFile string
}

// newAuthCommand 建 `auth refresh`。
//
// # 为什么端别不是一个参数
//
// `login` 那里必须选 `--client pc|app`，因为登录时**还没有**任何凭据可依据。
// 刷新时库里那一串 `Authorization` 自己就带着端别（第一段），
// 再由它决定要配哪一套头 —— 反过来让用户再选一次，就会出
// "库里是 App 的串、参数选 PC"这种矛盾组合（那正是本仓库拦过五次的同类错）。
//
// # 为什么 `--platform` 只作用于 PC 那一支
//
// 库里没记这个 PC 账号当时是 Windows 还是 macOS（`accounts.platform` 存的是
// **端别** `pc`/`mobile`，不是操作系统）。所以它是个**必须有人拍板**的信息：
// 给了就用，没给就用 Windows（与 `login` 那边的默认一致），
// 并且把它打进输出里 —— 刷完要看得见这次是以哪个端刷的。
func newAuthCommand(deps *Deps, out *output) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "auth",
		Short: "维护已存账号的登录凭据（auth）",
	}

	args := &AuthRefreshArgs{}
	refresh := &cobra.Command{
		Use:   "refresh",
		Short: "到期前向上游换新 token，并把库里那套凭据换掉",
		Long: "向 `user/auth/refreshToken` 换新 token，成功就**覆盖**库里那一套" +
			"（`authorization` / `jwt_token` 与 `accounts.auth_expire`，其余列不动）。\n\n" +
			"端别由库里那一串 auth 自己决定：`pc` 走 PC 客户端头，`mobile` 走 App 头。\n" +
			"App 那一支是实拍闭环过的；PC 那一支按 H5 的形状拼、**尚未实测**" +
			"（详见 docs/capture/app-rights-token-samples.txt §4）。\n\n" +
			"⚠ 这会轮换真凭据：一次只发一个请求，**不自动重试** —— " +
			"上游可能已经换掉了 token 只是响应没回来（实测最慢一次 28.4 秒）。",
		RunE: func(*cobra.Command, []string) error {
			return runAuthRefresh(deps, out, args)
		},
	}
	flags := refresh.Flags()
	flags.StringVar(&args.Account, "account", "",
		"账号 id（形如 19****01）。与 --all 二选一")
	flags.BoolVar(&args.All, "all", false,
		"扫一遍所有**启用中的**账号，只刷到点的那些（没到点的一个请求都不发）")
	flags.StringVar(&args.DB, "db", defaultDB, "SQLite 文件")
	flags.Float64Var(&args.LeadDays, "lead-days", 7,
		"还剩几天就该刷。太短会在真过期后才动手，太长会白打上游")
	flags.BoolVar(&args.Force, "force", false,
		"到不到点都刷：越过 --lead-days 那道闸（WebUI 那一枚按钮就是这个语义）。"+
			"与 --all 同时给等于把每个启用中的账号都换一遍 token，想清楚再用")
	flags.Var(newEnumFlag(&args.Platform, platformValues...), "platform",
		"仅 PC 那一支：clientType / cpid / 版本号。库里没记当时是哪个系统，"+
			"所以这一条要你拍板")
	flags.StringVar(&args.AppDeviceFile, "app-device-file", defaultAppDeviceFile,
		"仅 App 那一支：deviceId 的存放位置（要与登录时那份一致，"+
			"换了它等于换一台设备）")

	// `--account` 与 `--all` 互斥：同时给的话"到底是哪一个"没有正确答案，
	// 而静默偏向一边会让一次批量刷新看起来只刷了一个账号。
	refresh.MarkFlagsMutuallyExclusive("account", "all")

	cmd.AddCommand(refresh)
	return cmd
}
