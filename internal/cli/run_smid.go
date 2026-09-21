package cli

import (
	"context"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// `smid show` / `reset` / `refresh` 三条叶子的**命令行编排**。
//
// 对应 Rust 侧 legacy/crates/mcloud-cli/src/smid_cmd.rs（187 行）。
//
// # 这一层只剩两件事
//
// 生命周期在 `app::smid`（什么时候生成、复用、作废），算法在 `upstream::smid`。
// 本文件剩下的只有：**把结果说给人听**与**挑出账号**。
//
// # 输出一律是**摘要**
//
// 设备特征是这台设备的**长相**，deviceId 是风控服务给它的标识 ——
// 两者都不该整份进终端：终端上那一眼通常是被截图、被贴进 issue 的那一份。
// 所以这里只有 [fingerprintLine] 与 [maskDeviceID] 两个出口，
// 完整值留在库里。

// withLocalStore 打开库、跑一次、关掉。
//
// 三条叶子各写一遍 open/close 是三条重复路径，而"忘记 defer Close"在
// 只跑一次命令的 CLI 里**没有任何症状** —— 它只在"同一个进程连开多个库"
// 时才现形，而那恰恰是测试才会做的事。收成一个入口就没有这个口子。
func withLocalStore(dbPath string, fn func(store core.StateStore) error) error {
	store, err := openLocal(dbPath)
	if err != nil {
		return err
	}
	defer store.Close()
	return fn(store)
}

// ─────────────────────────────────────────────────────────────────────────────
// show
// ─────────────────────────────────────────────────────────────────────────────

func runSmidShow(out *output, dbPath string) error {
	return withLocalStore(dbPath, func(store core.StateStore) error {
		ctx := context.Background()
		accounts, err := store.ListAccounts(ctx, core.ScopeLocal())
		if err != nil {
			return fmt.Errorf("读账号列表失败: %w", err)
		}
		if len(accounts) == 0 {
			out.human("还没有云盘账号。先跑：mcloud login qr")
			return nil
		}

		bound := 0
		for _, account := range accounts {
			out.human("%s", accountTitle(account))

			state, err := store.GetSmidState(ctx, core.ScopeLocal(), account.ID)
			if err != nil {
				return fmt.Errorf("读设备特征失败: %w", err)
			}
			switch {
			case state != nil && state.FingerprintJSON != nil:
				bound++
				out.human("  设备特征  %s", fingerprintLine(state.FingerprintJSON.Value()))
			default:
				// 说清"下一次会自动绑"：否则用户会以为要手动做点什么，
				// 而手动做（现在就 reset）反而白白作废一个还能用的特征。
				out.human("  设备特征  未绑定（下一次 `mcloud signin` 会自动绑一个并上报，" +
					"也可以现在就做：`mcloud smid refresh --account …`）")
			}

			if state != nil && state.DeviceID != nil {
				out.human("  deviceId  %s", maskDeviceID(state.DeviceID.Value()))
			} else {
				out.human("  deviceId  还没有（上报后由风控服务给）")
			}
		}
		out.human("\n共 %d 个账号，已绑定设备特征 %d 个", len(accounts), bound)
		return nil
	})
}

// accountTitle 是账号那一行：有昵称就带上。
func accountTitle(account core.Account) string {
	if account.Nickname != nil && *account.Nickname != "" {
		return fmt.Sprintf("%s（%s）", account.ID, *account.Nickname)
	}
	return account.ID.String()
}

// ─────────────────────────────────────────────────────────────────────────────
// reset / refresh
// ─────────────────────────────────────────────────────────────────────────────

func runSmidReset(deps *Deps, out *output, dbPath, account string, report bool) error {
	return withLocalStore(dbPath, func(store core.StateStore) error {
		ctx := context.Background()
		id, err := requireAccount(ctx, store, account)
		if err != nil {
			return err
		}

		binding, perr := app.RotateFingerprint(ctx, store, core.ScopeLocal(), id, deps.Random)
		if perr != nil {
			return perr
		}
		text, perr := binding.Fingerprint.ToJSONString()
		if perr != nil {
			return perr
		}
		out.human("已换上新的设备特征：%s", fingerprintLine(text))
		out.human("旧的 deviceId 同时作废（新长相配旧 id 会被认成同一台设备在撒谎）")

		if !report {
			out.human("没有上报：下一次真正用到 deviceId 时会自动换一个新的")
			return nil
		}
		deviceID, perr := app.ReportDeviceID(
			ctx, upstream.NewUpstream(deps.http()), store, core.ScopeLocal(),
			id, deps.Clock, deps.Random)
		if perr != nil {
			return perr
		}
		out.human("已上报，新的 deviceId：%s", maskDeviceID(deviceID))
		return nil
	})
}

func runSmidRefresh(deps *Deps, out *output, dbPath, account string) error {
	return withLocalStore(dbPath, func(store core.StateStore) error {
		ctx := context.Background()
		id, err := requireAccount(ctx, store, account)
		if err != nil {
			return err
		}
		deviceID, perr := app.ReportDeviceID(
			ctx, upstream.NewUpstream(deps.http()), store, core.ScopeLocal(),
			id, deps.Clock, deps.Random)
		if perr != nil {
			return perr
		}
		out.human("已上报，新的 deviceId：%s", maskDeviceID(deviceID))
		return nil
	})
}

// requireAccount 把 `--account` 翻成一个**确实存在**的账号 id。
//
// 打错一个字就会凭空给一个不存在的账号绑上特征 —— 那一行永远没人读得到，
// 只会留在库里让人困惑。所以查不到就报错，并把现有账号列出来：
// 账号 id 是脱敏手机号（`19****01`），用户是照着 `smid show` 手打的，
// "打错了"比"真的没有"常见得多。
func requireAccount(ctx context.Context, store core.StateStore, account string) (core.AccountID, error) {
	id := core.NewAccountID(account)
	found, err := store.GetAccount(ctx, core.ScopeLocal(), id)
	if err != nil {
		return "", fmt.Errorf("读账号失败: %w", err)
	}
	if found != nil {
		return id, nil
	}
	accounts, err := store.ListAccounts(ctx, core.ScopeLocal())
	if err != nil {
		return "", fmt.Errorf("读账号列表失败: %w", err)
	}
	known := make([]string, 0, len(accounts))
	for _, account := range accounts {
		known = append(known, account.ID.String())
	}
	if len(known) == 0 {
		return "", fmt.Errorf("没有账号 %s（库里还没有账号）", account)
	}
	return "", fmt.Errorf("没有账号 %s（库里现有：%s）", account, strings.Join(known, ", "))
}

// ─────────────────────────────────────────────────────────────────────────────
// 摘要
// ─────────────────────────────────────────────────────────────────────────────

// fingerprintLine 是设备特征的**摘要**行：能认出"是哪台设备"，
// 但不把识别位整份抄进终端。
//
// `canvas` 那一段就是这份特征的识别位（真机上由渲染结果决定、稳定不变），
// 所以它只进 `fp#` 那个摘要里。UA 不打：本项目只用一个公开常量 UA，
// 打出来不带任何区分信息。
func fingerprintLine(raw string) string {
	digest := core.MD5Hex([]byte(raw))
	if len(digest) > 8 {
		digest = digest[:8]
	}
	fingerprint, perr := upstream.SmFingerprintFromJSON(raw)
	if perr != nil {
		// 读不出来时**不**回吐库里那串：那就是这台设备的长相，
		// 终端/日志里不该有第二份。只给摘要 + 一句怎么修。
		return fmt.Sprintf("fp#%s  （库里那串读不出来：`mcloud smid reset --account …` 重新绑定）", digest)
	}
	return fmt.Sprintf("fp#%s  %s · %d×%d · dpr=%d · 触屏 %d 点 · rtt %d · cpu %d",
		digest,
		fingerprint.Platform,
		fingerprint.ScreenWidth, fingerprint.ScreenHeight,
		fingerprint.Dpr, fingerprint.MaxTouchPoints,
		fingerprint.ConnectionRtt, fingerprint.CPUCount)
}

// maskDeviceID 是 `deviceId` 的展示形态：只留首尾。
//
// 它是第三方风控服务给这台设备的标识 —— 完整值留在库里即可，
// 终端上那一眼通常是被截图/被贴进 issue 的，所以按卡片号的规矩来。
//
// ⚠ 按 **rune** 而不是字节取尾：这串是 base64，ASCII 时两者一样，
// 但一旦上游哪天塞个非 ASCII 进来，按字节切会切出一个半个字符。
func maskDeviceID(id string) string {
	chars := []rune(id)
	if len(chars) == 0 {
		return "（0 字符）"
	}
	keep := len(chars)
	if keep > 4 {
		keep = 4
	}
	return fmt.Sprintf("%c…%s（%d 字符）",
		chars[0], string(chars[len(chars)-keep:]), len(chars))
}
