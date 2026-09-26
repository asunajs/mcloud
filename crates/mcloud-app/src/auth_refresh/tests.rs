//! 判据测试。
//!
//! 判据是**纯函数**，所以能逐档钉住。这三档错了不会报错 ——
//! 只会"该刷的没刷"（直到上游回 90001）或"不该刷的天天刷"。

use super::*;

const DAY: i64 = 86_400_000;
const NOW: i64 = 1_893_456_000_000; // 某个固定的"现在"

// ── 三档 ────────────────────────────────────────────────────────────────

#[test]
fn needed_when_inside_the_lead_window() {
    // 还剩 3 天，提前量 7 天 ⇒ 该刷
    assert_eq!(decide(NOW + 3 * DAY, NOW, 7.0), RefreshDecision::Needed);
}

#[test]
fn not_yet_when_far_away() {
    // 还剩 30 天 ⇒ 不该刷，一个请求都不发
    assert_eq!(decide(NOW + 30 * DAY, NOW, 7.0), RefreshDecision::NotYet);
}

/// ⭐ 边界：刚好等于提前量时该刷（判据是 `<=`，不是 `<`）。
#[test]
fn exactly_at_the_lead_window_is_needed() {
    assert_eq!(decide(NOW + 7 * DAY, NOW, 7.0), RefreshDecision::Needed);
    // 多一毫秒就不刷
    assert_eq!(decide(NOW + 7 * DAY + 1, NOW, 7.0), RefreshDecision::NotYet);
}

#[test]
fn already_expired_is_needed() {
    assert_eq!(decide(NOW - DAY, NOW, 7.0), RefreshDecision::Needed);
}

/// ⭐ `expire == 0` 是 `Needed`，**不是** `ManualOnly`。
///
/// 参考实现两处不一致（一处当"跳过"、一处当"该刷"）。
/// 本仓取"该刷"：0 更可能是"解析不出"而不是"确实没有"，
/// 打一次不亏，漏刷才是问题。
#[test]
fn zero_expire_is_needed_not_manual() {
    assert_eq!(decide(0, NOW, 7.0), RefreshDecision::Needed);
}

/// ⭐ `ManualOnly` **只有** `expire < 0` 这一种情况。
#[test]
fn manual_only_is_strictly_negative() {
    assert_eq!(decide(-1, NOW, 7.0), RefreshDecision::ManualOnly);
    assert_eq!(decide(i64::MIN, NOW, 7.0), RefreshDecision::ManualOnly);
    // 0 与正数都不是
    assert_ne!(decide(0, NOW, 7.0), RefreshDecision::ManualOnly);
    assert_ne!(decide(NOW, NOW, 7.0), RefreshDecision::ManualOnly);
}

// ── 单位 ────────────────────────────────────────────────────────────────

/// ⭐ 判据按**毫秒**算。
///
/// 把毫秒当秒读的后果不是报错，而是**静默算错**：
/// `1893456000000` 会被当成公元 61968 年，"永远不需要刷新"。
#[test]
fn uses_milliseconds_not_seconds() {
    // 一个真实的 13 位毫秒时间戳（约 2030 年）
    let expire = 1_893_456_000_000;
    // 从这个时刻起 30 天
    let now = expire - 30 * DAY;
    assert_eq!(
        decide(expire, now, 7.0),
        RefreshDecision::NotYet,
        "还剩 30 天，不该刷"
    );
    // 若把 expire 当秒（即除以 1000），它相对于 now 会小到"早已过期" ⇒ Needed。
    // 这两种解释给出**相反**的结论，正是这个测试要钉住的。
    assert_eq!(decide(expire / 1000, now, 7.0), RefreshDecision::Needed);
}

// ── 提前量参数 ──────────────────────────────────────────────────────────

#[test]
fn lead_days_is_configurable() {
    let expire = NOW + 10 * DAY;
    assert_eq!(
        decide(expire, NOW, 7.0),
        RefreshDecision::NotYet,
        "7 天窗口外"
    );
    assert_eq!(
        decide(expire, NOW, 14.0),
        RefreshDecision::Needed,
        "14 天窗口内"
    );
}

/// 很小的提前量（等价于"到期当天才刷"）。
#[test]
fn tiny_lead_days_is_allowed() {
    let expire = NOW + DAY;
    assert_eq!(decide(expire, NOW, 0.001), RefreshDecision::NotYet);
    assert_eq!(decide(NOW + 60_000, NOW, 0.001), RefreshDecision::Needed);
}

#[test]
fn default_lead_is_seven_days() {
    assert_eq!(DEFAULT_LEAD_DAYS, 7.0);
}

// ── 落库兜底 ────────────────────────────────────────────────────────────

/// ⭐ 明显不是凭据的串要被挡住。
///
/// "拼错了但看起来有值"是最难查的一类：库里、界面上都正常显示，
/// 只是每次请求都鉴权失败。
///
/// ⚠ 但 `parse_auth` 是**刻意宽容**的（对齐 Node 的 `Buffer.from(s,"base64")`：
/// 忽略字母表以外的字符）。所以"合法 base64 但内容没有冒号"这种输入
/// **它能解出来**（platform/phone/token 都是空串）—— 那不是缺陷，
/// 是"上游塞了个空白字符"不该让整条凭据不可用这条设计的代价。
///
/// 这个测试只钉住真正的硬边界：**空的 / 纯垃圾**过不去。
#[test]
fn rejects_clearly_unusable_auth() {
    assert!(assert_usable("").is_err(), "空串必须挡住");
    // 长度模 4 余 1 的 base64 是无解的
    assert!(assert_usable("!!!!!").is_err(), "解不开的必须挡住");
    // 正确的形状要能过
    let good =
        mcloud_crypto::b64_encode(b"mobile:19900000000:SYNTH001|1|RCS|1792391642025|payload");
    assert!(assert_usable(&good).is_ok());
}

/// 宽容是有意的：能解出内容就算可用（哪怕字段是空的）。
///
/// 记下来是为了让"为什么不更严"这件事**有据可查** ——
/// 不然下一个人会以为这里漏了校验，然后加上去，
/// 于是"上游多一个空白字符"变成"凭据整条不可用"。
#[test]
fn lenient_by_design() {
    let odd = mcloud_crypto::b64_encode(b"garbage");
    assert!(
        assert_usable(&odd).is_ok(),
        "合法 base64 一律算可用 —— 与 Node 的宽容解码对齐"
    );
}
