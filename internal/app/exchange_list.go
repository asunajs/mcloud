package app

// 兑换清单的**只读**入口 —— 给界面上的选品器
// （`GET /api/accounts/{id}/exchange-prizes`）用。
//
// # 为什么单独一个文件、而不是让 server 直接打上游
//
// 这一枪看起来只是 `upstream.AppExchangeList(ctx, jwt, referer)` 的一层薄皮，
// 但**薄皮不是重点，[do] 才是**：票属于"账号 × 页面"（AGENTS §1.12），
// 而撞 `90001` 时"刷一次票、重放一次"这件事只写在 [do] 里一处。
// 让 server 层自己拿 jwt 打上游，就会造出**第二条**不走
// 那条重放路径的调用点 —— 表现是"界面上偶发一次失败，重跑就好了"，
// 而这类现象在测试里根本不复现。所以上游那一口只能从这里出去。
//
// # 为什么这里**不**读余额
//
// 判"能不能下单"的完整口径是 [PlanExchange]（要余额），而 [PlanExchangeWithoutBalance]
// 故意少那一档：余额是**另一次**账号级读取（`infoV3`），"看一眼清单"不值得多打一枪
// （风控噪声的口径与 `exchange_run.go` 里"查无此件就不读余额"同一条）。
// ⇒ 界面上的"可兑"因此比下单前的真判据**宽**：点了仍可能被链挡下，
// 而那一挡有码、有小结，不会静默扣豆。
//
// # `jwttoken` 使用公共来源（临时态，不是终判）
//
// 这一口要的票来自 [SignInSessionSource]，也就是签到页那份配方（AGENTS §1.12 要求留痕的那一句）。
// ⭐ 2026-09-24 用库里两个真账号真打时，这张票在 `exchangeList` 上**被认**（`code 0`、各 55 件）
// ⇒ 兜底今天有实测支撑；但"被认"不等于"这就是兑换页自己的取法"，
// 特有取法那一格仍登记在 `docs/pending-decisions.md` R5，抓到要主动更正这一处。
//
// # 真打状态（2026-09-24 起）
//
// 只读的这一半**已经真打成功**（读数在 `docs/status.md`「兑换页的选品只读接口」那一节：
// 两个真账号各 55 件 / 9 组、组键不连续、四档 `plan` 露面）⇒ 上面那句"分组键不连续只能
// 在解析层挡住"不再只是推断。⚠ **下单**那一半（`exchangeV3`）仍未由本仓发过 ——
// 那三条实拍是 2026-09-22 从真机 App 抓来的，不是本仓打的（要拍板：P2）。

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ListExchangePrizes 读一次"这个账号此刻看见的兑换清单"。**只读**，没有副作用。
//
// 返回值是**解析后的**清单（[upstream.ExchangeList]），不是原始 JSON：
// 分组键不连续（`0,1,2,5,7,8,9,11,15`）这件事只能在解析层挡住 ——
// 把 map 原样交给前端去遍历，"按 0..n 遍历漏掉一半"那一类失效会换个地方重演。
//
// 清单是**时刻量**（`count` / `dailyRemainderCount` 会因为别的用户在同一分钟里兑掉而变），
// 所以这一口不缓存、也不与下单那次共用读数（下单前链上还会现读一次）。
func ListExchangePrizes(
	ctx context.Context, src *SignInSessionSource,
) (upstream.ExchangeList, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.ExchangeList, *core.Error) {
		return src.up.AppExchangeList(ctx, s.jwt, s.referer)
	})
}
