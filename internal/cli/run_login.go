package cli

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/skip2/go-qrcode"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 登录两条叶子（`login qr` / `login sms`）的**命令行编排**。
//
// 对应 Rust 侧 legacy/crates/mcloud-cli/src/login_cmd.rs + support.rs。
//
// # 这一层只剩两件事
//
// 协议与状态机在 `capabilities::login`，驱动（等待、重试、落库）在 `app::login`
// 与 `app::app_login`。本文件剩下的只有：**把结果说给人听**与**读人输进来的东西**。
// 所以这里刻意**没有**任何业务判定 —— 出现 `if code == 200059554` 就是放错了层。
//
// # 凭据边界
//
// 登录结果里的 `userInfo` 含 `authToken`：它只用于落库，
// 打印时**只给字段名不给值**（[printLoginSuccess]）。

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

// terminalObserver 把二维码画出来，把进展印成一行。
type terminalObserver struct {
	out      *output
	deviceID string
	// qrOut 是 `--qr-out`：额外落一张图片。
	qrOut string
}

// OnQR 实现 [app.LoginObserver]：只调一次，在开始轮询之前。
func (o *terminalObserver) OnQR(payload string) {
	// 图片优先：终端字符画只在等宽字体下成立，实际很容易变形，
	// 而"码扫不出来"会被用户理解成"这软件坏了"。
	if o.qrOut != "" {
		if err := writeQRSVG(payload, o.qrOut); err != nil {
			o.out.human("（写二维码图片失败：%v）", err)
		} else {
			o.out.human("\n二维码已写入图片，**打开它扫描**（比终端字符画可靠）：\n  %s\n", o.qrOut)
		}
	}

	o.out.human("用移动云盘 App 扫描下面的二维码：\n")
	if art, err := renderQR(payload); err != nil {
		o.out.human("（终端画不出二维码：%v）", err)
	} else {
		o.out.human("%s", art)
	}
	// 链接也打出来：有的终端字体/配色会把二维码糊掉，手输链接是兜底
	o.out.human("%s", payload)
	o.out.human("设备指纹 %s（换掉它服务端就定位不到本次会话）\n", o.deviceID)
	if o.qrOut == "" {
		o.out.human("等待扫码…（Ctrl-C 取消）")
	}
}

// OnProgress 实现 [app.LoginObserver]：每次轮询后给一次。
func (o *terminalObserver) OnProgress(progress capabilities.QrProgress) {
	switch progress.Kind {
	case capabilities.QrNotScanned, capabilities.QrLoggedIn:
		// 未扫码是常态（实测 121/192），每次都印会刷屏；成功由 [printLoginSuccess] 收口。
	case capabilities.QrScanned:
		o.out.human("已扫码，请在手机上确认登录")
	case capabilities.QrFailed:
		// 限频会反复出现，所以给一句人话 + 说明"码还有效"，
		// 否则用户看到"失败"会以为要重开一张码 —— 而重开只会更深地撞进风控。
		if progress.Reason.Kind == capabilities.FailureTooFrequent {
			o.out.human("被限频（扫码登录过于频繁），等一会儿自动重试；二维码仍然有效，不用重开")
			return
		}
		o.out.human("失败：%s（%s）", progress.Reason.Error(), progress.Reason.Kind)
	}
}

func runLoginQR(deps *Deps, out *output, a *QrArgs) error {
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

	platform := pcPlatform(a.Platform)
	deviceID, perr := app.LoadOrCreateDeviceID(a.DeviceFile, deps.Random)
	if perr != nil {
		return perr
	}

	up := upstream.NewUpstream(deps.http())
	observer := &terminalObserver{out: out, deviceID: deviceID, qrOut: a.QROut}

	out.human("正在以 %s 客户端身份登录（设备 %s）", platformName(platform), deviceID)

	// `--no-wait`：只出码就退出。它**不**轮询，所以一个请求都不发
	// （出码是本地拼字符串，这是抓包推翻的直觉假设里最重要的一个）。
	if a.NoWait {
		qr := capabilities.StartQrLogin(platform, deviceID, deps.Random, deps.Clock.Now())
		observer.OnQR(qr.QrPayload())
		return nil
	}

	outcome, perr := app.QRLogin(
		ctx, store, up,
		// `--ttl-seconds` 是**秒**（用户单位），能力层是 [time.Duration]
		// ⇒ 换算只在这一处，不渗进内部类型。
		app.NewLoginContext(scopeForNewAccounts(owner), platform, deviceID).
			WithTTL(time.Duration(a.TTLSeconds)*time.Second),
		upstream.NewLoginDataDecryptorProduction(),
		deps.Clock, deps.Random, observer, deps.waiter())
	if perr != nil {
		return perr
	}
	printLoginSuccess(out, outcome)
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 短信
// ─────────────────────────────────────────────────────────────────────────────

// smsCodePrompt 是读验证码那行的提示语。
const smsCodePrompt = "请输入收到的 6 位验证码: "

func runLoginSms(deps *Deps, out *output, a *SmsArgs) error {
	if !a.given("phone") || strings.TrimSpace(a.Phone) == "" {
		return fmt.Errorf("--phone 是必填的: %w", ErrUsage)
	}
	if a.Client == "app" {
		return runLoginSmsApp(deps, out, a)
	}
	return runLoginSmsPC(deps, out, a)
}

func runLoginSmsPC(deps *Deps, out *output, a *SmsArgs) error {
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

	platform := pcPlatform(a.Platform)
	deviceID, perr := app.LoadOrCreateDeviceID(a.DeviceFile, deps.Random)
	if perr != nil {
		return perr
	}
	up := upstream.NewUpstream(deps.http())

	out.human("正在以 %s 客户端身份登录（设备 %s）", platformName(platform), deviceID)

	// 有码就不再发码：那会白刷一次今日配额，而且 PC 端发码还要连带解一次滑块。
	// 发码与登录是两次独立调用，码已到手时只跑后一半。
	code, stopped, err := obtainSmsCode(deps, out, a, false,
		func() (app.SmsCodeSent, bool, *core.Error) {
			sent, sendErr := app.SmsSendCode(ctx, up, platform, deviceID, a.Phone, deps.Random)
			return sent, false, sendErr
		})
	if err != nil {
		return err
	}
	if stopped {
		return nil
	}

	outcome, perr := app.SmsLogin(
		ctx, store, up,
		app.NewLoginContext(scopeForNewAccounts(owner), platform, deviceID),
		app.SmsCredential{Phone: a.Phone, Code: code},
		upstream.NewLoginDataDecryptorProduction(),
		deps.Clock, deps.Random)
	if perr != nil {
		return perr
	}
	printLoginSuccess(out, outcome)
	return nil
}

func runLoginSmsApp(deps *Deps, out *output, a *SmsArgs) error {
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

	deviceID, perr := app.LoadOrCreateAppDeviceID(a.AppDeviceFile, deps.Random)
	if perr != nil {
		return perr
	}
	// `deviceShort` **每次运行现生成**：实测同一 `deviceId` 在不同时间点换过 short，
	// 所以它不是一个可以持久化的"每安装指纹"。传 `--device-short` 可以把它钉住，
	// 用于把"发码 → 登录"两半连成一次会话。
	short := strings.ToUpper(strings.TrimSpace(a.DeviceShort))
	if short == "" {
		short = app.NewAppDeviceShort(deps.Random)
	}
	profile := upstream.NewAppProfile(upstream.NewIPadAppDevice(deviceID, short))

	// 读回填与写落库**必须同一个范围**：范围不同会出现"读到某个账号的用户域、
	// 却把它发给另一个账号"，那比不带 `x-yun-uni` 更糟。
	scope := scopeForNewAccounts(owner)
	// 登录过一次之后 `userDomainId` 就在库里了，第二次起要带上（真机也一样）。
	// 这里从手机号推 id：与下面落库时用的是同一个 `core.AccountLabel`，
	// 所以"读回填"与"写落库"指的是同一行。
	profile, perr = app.ProfileWithStoredDomainID(
		ctx, store, scope, profile, core.NewAccountID(core.AccountLabel(nil, a.Phone)))
	if perr != nil {
		return perr
	}

	up := upstream.NewUpstream(deps.http())
	out.human("正在以 iOS App 身份登录（deviceId %s）", deviceID)

	code, stopped, err := obtainSmsCode(deps, out, a, a.SendOnly,
		func() (app.SmsCodeSent, bool, *core.Error) {
			outcome, sendErr := app.AppSendSmsCode(ctx, up, profile, a.Phone, deps.Clock)
			if sendErr != nil {
				return app.SmsCodeSent{}, false, sendErr
			}
			switch outcome.Kind {
			case app.AppCodeSent:
				return app.SmsCodeSent{
					RemainGetTimes: outcome.RemainGetTimes,
					SolvedSlider:   outcome.SolvedSlider,
				}, false, nil
			case app.AppCodeNeedCaptcha:
				// 只有**自动过滑块也失败**时才走到这里。把坐标一并打出来：
				// 万一是服务端要的答案口径变了，这一行就是唯一线索。
				captchaType := "未知"
				if outcome.CaptchaType != nil {
					captchaType = fmt.Sprintf("%d", *outcome.CaptchaType)
				}
				out.human("自动过滑块失败（已试 %d 次，captchaType=%s，最后发上去的坐标 %s）。",
					outcome.Attempts, captchaType, outcome.LastAnswer)
				out.human("改走兜底路径：请在浏览器打开下面这个地址，拖动拼图完成验证：")
				out.human("\n%s\n", outcome.URL)
				out.human("（**短信由该页面发出**，完成拖动后回到这里继续。不要再重新执行本命令）")
				// 落到这里表示"码没发出来"。⚠ 它不是**错误**：该打印的都打印了，
				// 用户接下来要做的是去浏览器里拖那一下。用 stopped 而不是 error，
				// 就是为了不再补一句"失败" —— 那会让人以为命令没跑成，于是重新执行，
				// 而重新执行正是这里最不希望发生的事（会再要一次滑块、把配额刷掉）。
				return app.SmsCodeSent{}, true, nil
			}
			return app.SmsCodeSent{}, false, core.NewInternal("发码没有产出结果")
		})
	if err != nil {
		return err
	}
	if stopped {
		// 把当次的设备短标识打出来：下一步用 `--device-short` 传回来，
		// 两半才会被上游看成同一台设备形态。
		out.human("\n本次设备短标识（继续时用 --device-short 传回）：%s", short)
		out.human("发码完成，已按 --send-only 退出。")
		return nil
	}

	outcome, perr := app.AppSmsLogin(
		ctx, store, up,
		app.AppLoginContext{
			Scope:      scope,
			Profile:    profile,
			Credential: app.SmsCredential{Phone: a.Phone, Code: code},
		},
		upstream.NewAppLoginCryptoProduction(),
		deps.Clock, deps.Random)
	if perr != nil {
		return perr
	}
	printLoginSuccess(out, outcome)
	return nil
}

// obtainSmsCode 拿到一个验证码：要么用户直接给了（`--code`），要么发一次再问用户要。
//
// # 为什么要收一个 `send` 回调而不是把两条端各自写一遍
//
// PC 与 App 的发码调用形状不同，但"有码就跳过发码、发完读一行、--send-only 就退出"
// 这几条是**同一套命令行约定**。分开写两次，迟早会在"只有一边处理了 --send-only"
// 这种地方分叉，而那种分叉只在真跑的时候才看得见。
//
// `sendOnly` 由调用方给而不是读 `a.SendOnly`：只有 **App 那条路**认这个开关
// （PC 那条发完就直接问码），所以这里不能替两条路统一决定。
//
// `send` 的第二个返回值是 **stopped**（"到此为止、不要再问用户要验证码"）：
// App 端自动过滑块也失败时，该说的都说了（H5 地址已打印），再问一次只会得到空输入。
//
// ⚠ stopped **不是**错误：用 bool 而不是 error，是为了让"用户接下来要去做别的事"
// 与"这次跑挂了"在类型上分开 —— 混在一起会让人想重试，而重试正是这里最坏的走向。
func obtainSmsCode(
	deps *Deps, out *output, a *SmsArgs, sendOnly bool,
	send func() (sent app.SmsCodeSent, stopped bool, perr *core.Error),
) (code string, stopped bool, err error) {
	if given := strings.TrimSpace(a.Code); given != "" {
		return given, false, nil
	}

	sent, stopped, perr := send()
	if perr != nil {
		return "", false, perr
	}
	if stopped {
		return "", true, nil
	}
	if sent.RemainGetTimes != nil {
		out.human("验证码已发送（今日剩余 %d 次）", *sent.RemainGetTimes)
	} else {
		out.human("验证码已发送")
	}
	if sent.SolvedSlider {
		out.human("（被风控要求滑块，已自动解出缺口位置并重发）")
	}

	if sendOnly {
		return "", true, nil
	}

	typed, readErr := deps.readSecret(smsCodePrompt)
	if readErr != nil {
		return "", false, readErr
	}
	return strings.TrimSpace(typed), false, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 打印
// ─────────────────────────────────────────────────────────────────────────────

// printLoginSuccess 打印登录成功的那几行。
//
// ⚠⚠ **只打字段名，不打值**：`userInfo` 里含 `authToken`（整串凭据）。
// 打印字段名是为了将来对得上"上游这次回了什么" —— 实测它比文档示例少一个 `userId`，
// 而字段名不是秘密、值才是。
func printLoginSuccess(out *output, outcome app.LoginOutcome) {
	out.human("\n登录成功，账号已保存：%s", outcome.Summary())
	out.human("（凭据只落在本地库，不会打印出来）")

	fields := loginInfoFields(outcome.UserInfo)
	if len(fields) > 0 {
		out.human("登录返回的字段：%s", strings.Join(fields, ", "))
	}
}

// loginInfoFields 按字典序列出 `userInfo` 的字段名。
//
// 排序是为了让输出**逐次稳定**：上游字段集合会变，不排序时"字段少了一个"
// 会表现成整行重排，人工比对看不出来。
func loginInfoFields(userInfo any) []string {
	obj, ok := userInfo.(map[string]any)
	if !ok {
		return nil
	}
	fields := make([]string, 0, len(obj))
	for key := range obj {
		fields = append(fields, key)
	}
	sort.Strings(fields)
	return fields
}

// ─────────────────────────────────────────────────────────────────────────────
// 二维码
// ─────────────────────────────────────────────────────────────────────────────

// qrQuietModules 是二维码四周留白的模块数（规范要求的 4）。
const qrQuietModules = 4

// renderQR 把载荷渲染成终端可打印的字符画。
//
// 用半高块字符（对位 Rust 侧 `Dense1x2`）：同样的信息量只占一半行数，
// 否则一屏放不下一个足够大的二维码（太小手机扫不出来）。
//
// **但它只在等宽字体 + 正确行距下成立**，所以同时提供 [writeQRSVG]：
// 字符画变形的概率远高于"用户以为的偶然"。
func renderQR(payload string) (string, error) {
	code, err := qrcode.New(payload, qrcode.Medium)
	if err != nil {
		return "", fmt.Errorf("二维码内容过长或无法编码: %w", err)
	}
	return code.ToSmallString(false), nil
}

// writeQRSVG 把二维码写成 SVG 图片。
//
// 用 SVG 而不是 PNG：与 Rust 侧**同一产物**（那边也是 SVG），
// 浏览器与 macOS 预览都能直接打开，扫描体验与位图没差别，
// 而且不需要把图像编码库拖进来（这里按矩阵直接写 rect/path）。
func writeQRSVG(payload, path string) error {
	code, err := qrcode.New(payload, qrcode.Medium)
	if err != nil {
		return fmt.Errorf("二维码内容过长或无法编码: %w", err)
	}
	bitmap := code.Bitmap()
	if len(bitmap) == 0 || len(bitmap[0]) == 0 {
		return fmt.Errorf("二维码矩阵是空的")
	}

	modules := len(bitmap) + qrQuietModules*2
	// 每模块 8px 左右：太小手机对焦困难，太大一屏放不下。
	// Rust 侧是 `min_dimensions(360, 360)`，这里同口径取"总尺寸至少 360"。
	scale := 8
	if modules*scale < 360 {
		scale = (360 + modules - 1) / modules
	}
	side := modules * scale

	var svg strings.Builder
	fmt.Fprintf(&svg,
		"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"%d\" height=\"%d\" "+
			"viewBox=\"0 0 %d %d\" shape-rendering=\"crispEdges\">\n",
		side, side, side, side)
	svg.WriteString("<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\n")
	svg.WriteString("<path fill=\"#000000\" d=\"")
	for y, row := range bitmap {
		for x, dark := range row {
			if !dark {
				continue
			}
			fmt.Fprintf(&svg, "M%d %dh%dv%dh-%dz",
				(x+qrQuietModules)*scale, (y+qrQuietModules)*scale, scale, scale, scale)
		}
	}
	svg.WriteString("\"/>\n</svg>\n")

	if dir := filepath.Dir(path); dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return fmt.Errorf("创建目录 %s 失败: %w", dir, err)
		}
	}
	if err := os.WriteFile(path, []byte(svg.String()), 0o644); err != nil {
		return fmt.Errorf("写二维码 %s 失败: %w", path, err)
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 参数 → 领域值
// ─────────────────────────────────────────────────────────────────────────────

// pcPlatform 把 `--platform` 的取值翻成端档案。
func pcPlatform(value string) upstream.PcPlatform {
	if strings.EqualFold(value, "macos") {
		return upstream.PlatformMacOS
	}
	return upstream.PlatformWindows
}

// platformName 是给终端看的那个人话名字。
func platformName(platform upstream.PcPlatform) string {
	switch platform {
	case upstream.PlatformMacOS:
		return "macOS"
	case upstream.PlatformWindows:
		return "Windows"
	}
	return "PC"
}

// scopeForNewAccounts 是"新登录进来的账号归谁"。
//
// 库里还没有用户时返回 [core.ScopeLocal]：账号保持无主，
// 等 `mcloud user init` 建首个管理员时被认领。这不是权限检查，只是挑一个默认主人。
func scopeForNewAccounts(owner *core.UserID) core.Scope {
	if owner == nil {
		return core.ScopeLocal()
	}
	return core.ScopeUser(owner)
}
