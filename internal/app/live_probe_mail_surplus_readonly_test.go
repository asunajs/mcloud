package app

// 活体探针（**只读**）：⑤ `mail/resource/disk:surplusNum` —— 上游自己说"PDF 转换还剩几次"。
//
// # 为什么单独要这一发
//
// 1014 那条链今天有两处判断要靠它才能不靠猜：
//
//   - **今天还发不发得出 ⑥**。第廿三趟之后剩下的那一格读数是"小结那句改对了没有"，
//     而它只在 ⑥ **被认**（`code=="S_OK"`）之后才会走到产物那一段。同一天上午 ⑥ 还回
//     `S_OK`，下午同一枚号就回"今日次数已达上限"那一档 ⇒ 重跑之前先问一句"还剩几次"，
//     比发出去撞到拒绝才知道便宜（也少耗一次权益）。
//   - **产物那一枚值不值一次权益**。`docs/capture/app-cloud-residue-samples.txt` §7 记的是
//     "权益是有数的"，而"有数"到今天为止只有**失败文本**这一种读法（`AppMailSurplusNum`
//     在本仓生产层**没人调**，只有上游层与探针 ⇒ 成文 R37 那一格还空着）。
//
// # 这一发不做什么
//
//   - **不发 ③④⑥⑧**：①② 换到会话之后直接打 ⑤。④ 是不可回滚的打卡，⑥ 会真的扣一次权益
//     并在网盘里另存一份产物 ⇒ 本探针一个都不碰。
//   - 只念 `benefitSurplusCount` / `benefitStatus` 两格：它们是**计数与状态**，不是凭据
//     （凭据边界见 `live_probe_test.go` 文件头），所以可以原样打印；`sid` / `rmkey` 仍只念长度。
//   - **没有断言**：它是仪器。上游哪天改口（比如这一口回空），红一格既不是缺陷也不是判据。

import (
	"context"
	"testing"
)

func TestLiveProbeMailConvertSurplusReadOnly(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	executor := newTaskExecutor(run, opening, account)

	session, fired, problems := executor.mailSessionFor(ctx)
	if len(problems) > 0 {
		sink.say("①② 那一档没过（发过枪=%v）：%s ⇒ ⑤ 没有会话可打，这一趟停在这里",
			fired, sink.mask(problems[0]))
		return
	}
	sink.say("邮箱会话已取到（sid=%d rmkey=%d，只念长度）",
		len([]rune(session.SID.Value())), len([]rune(session.RMKey.Value())))

	cguid, rnd := mailDiskRandoms(executor.run.Random)
	count, status, serr := run.Upstream.AppMailSurplusNum(ctx, session, cguid, rnd)
	if serr != nil {
		sink.say("⑤ 转换权益没读到：%s ⇒ 今天能不能再发 ⑥ **仍是问句**（只有失败文本那一档）",
			sink.mask(serr.Error()))
		return
	}
	sink.say("⑤ 转换权益：benefitSurplusCount=%q benefitStatus=%q",
		liveProbeClip(count, 24), liveProbeClip(status, 24))
	sink.say("读数怎么用：count 非零且 status 说可用 ⇒ 重跑 ⑥ 才有产物那一段读数；" +
		"否则今天发也只是拿到一句拒绝，白花一次权益。⚠ 这一口的值与 ⑥ 的真实扣减" +
		"**是否同一本账**仍无样本（本发只念上游自己说什么）。")
}
