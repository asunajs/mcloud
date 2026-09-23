package cli

// `mcloud live-room` 的参数。
//
// 对位原项目 `packages/core/src/tasks/live-room-kouling.ts`（口令）
// 与 `tasks/red-flower/standalone.ts`（抽小红花）—— 那两条在原项目里是
// **两个任务**，这里合成一个命令。理由见 [runLiveRoom] 的说明。

import (
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

// LiveRoomArgs 是 `mcloud live-room` 的参数。
type LiveRoomArgs struct {
	DB      string
	Account int
	// AppDeviceFile 是 App 端 `deviceId` 的存放位置。
	//
	// ⚠ **必须与登录时同一个**：门的第①步 `querySpecTokenV2` 走的是 App 形态，
	// 设备漂移的表现是"凭据都在、却回一个空 SSO token"，
	// 而它看起来像"门没开"（见 signin-cli-spec 里同一条）。
	AppDeviceFile string
	// DeviceShort 是设备串第 7 段（给了就固定，不给则本次现生成）。
	//
	// 它**不落盘**（`app.NewAppDeviceShort` 的注释里写了为什么），
	// 所以这里默认空 —— 留这个开关只为复现问题：把上一次的形状钉住再跑一遍。
	DeviceShort string
	// Codes 是手工指定的口令（多个用换行 / 逗号分隔）。
	Codes string
	// ListenSeconds 是听弹幕的时长。
	ListenSeconds int
	// KoulingOnly 只听不投（**只读**：一个口令都不投给上游）。
	//
	// ⚠ 措辞是"不投"，不是"不消耗"：口令全场通用，不存在消耗
	// （见 `internal/upstream/live_flower.go` 文件头五条事实②）。
	KoulingOnly bool
	// SkipKouling 跳过收集，只投 `--codes` 给的口令。
	SkipKouling bool
	JSON        bool

	flags *pflag.FlagSet
}

// given 报告某个 flag 是否被**显式**给出（给了默认值不算）。
//
// ⚠ 与 [SigninArgs.given] 同一个理由：`--device-short ""` 与"没给"必须分得开，
// 否则一个空串会被当成"用户钉了一个空设备串"。
func (a *LiveRoomArgs) given(name string) bool {
	return a.flags != nil && a.flags.Changed(name)
}

// newLiveRoomCommand 建 `live-room`。
func newLiveRoomCommand(deps *Deps, out *output) *cobra.Command {
	a := &LiveRoomArgs{}

	cmd := &cobra.Command{
		Use:   "live-room",
		Short: "直播口令 → 抽小红花（仅在主播开播时有口令可用）",
		Long: "进直播间收口令，再逐个口令抽小红花。\n\n" +
			"⚠ 这条链**只在主播开播的那几小时里有效**：实拍里上游那句\n" +
			"「口令需当场直播获取才有效哦」说明从别处抄来的口令一律不通。\n" +
			"没开播时用 `--kouling-only` 之外的用法都只是白跑一趟。",
		RunE: func(*cobra.Command, []string) error {
			return runLiveRoom(deps, out, a)
		},
	}

	flags := cmd.Flags()
	a.flags = flags
	flags.StringVar(&a.DB, "db", defaultDB, "SQLite 文件")
	// ⚠ 帮助文本里**不许出现反引号**：pflag 会把第一对反引号之间的内容当成
	// **参数占位符**，于是 `--account` 在 help 里被印成
	// `--account mcloud signin --list`（而不是 `--account int`）。
	// 这个坑在 `--help` 的渲染里看得见，在测试里看不见 —— 见坑表 #115。
	flags.IntVar(&a.Account, "account", 0,
		"账号下标（从 1 起，与 mcloud signin --list 一致）；0 = 全部启用账号")
	flags.StringVar(&a.AppDeviceFile, "app-device-file", defaultAppDeviceFile,
		"仅 App：deviceId 的存放位置（32 位大写 HEX，无后缀）。必须与登录时同一个")
	flags.StringVar(&a.DeviceShort, "device-short", "",
		"仅 App：固定设备串第 7 段（12 位大写 HEX）。不传则每次现生成")
	flags.StringVar(&a.Codes, "codes", "",
		"手工指定口令（多个用换行或逗号分隔）；留空 = 自动收集。"+
			"⚠ 另一条口令在小红书、取不到，只能人工从这喂进来")
	flags.IntVar(&a.ListenSeconds, "listen-seconds", defaultLiveListenSeconds,
		"听弹幕的时长（秒）；只在自动收集时生效")
	flags.BoolVar(&a.KoulingOnly, "kouling-only", false,
		"只听不投：收口令并打印，**一个口令都不投给上游**")
	flags.BoolVar(&a.SkipKouling, "skip-kouling", false,
		"跳过收集，只投 --codes 给的口令")
	flags.BoolVar(&a.JSON, "json", false, "以 JSON 输出")

	return cmd
}

// defaultLiveListenSeconds 是听弹幕的默认时长。
//
// # ⚠ 别把它记成"原项目的超时"
//
// 原项目有**两个**超时，60 s 只是其中一个（`tasks/live-room-kouling.ts`）：
//
//	getLiveKouling(ctx, timeout = 0)   ← **主路径**：0 的含义是"收到第一个口令就返回"，
//	                                      而实际用到的上限是 **180 s**
//	getLiveKouling(ctx, 60000)         ← 只有"有口令被判无效、重试一次"那趟才是 60 s
//
// 也就是说主路径既可能比 60 s **长得多**（没人喊口令就等满 180 s），
// 又可能**短得多**（第 3 秒收到口令就收工）。这里默认 60 s 是
// **本项目自己的选择**：`mcloud live-room` 是手动跑的一条命令，
// "没人喊也硬等三分钟"不划算；想听满就用 `--listen-seconds` 补。
//
// ⚠ 另一处**刻意的差异**：这里**不做**"收到一个就早退" ——
// 听满整段才能把这一场里陆续喊的多个口令都收齐（原项目早退是为了让定时任务
// 尽快跑完，那与本命令的用法相反）。所以 `--listen-seconds 0` 在这里是
// **用法错误**（见 [validateLiveRoomArgs]），**不是**"早退"开关。
const defaultLiveListenSeconds = 60
