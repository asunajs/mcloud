//! 产物账本测试。
//!
//! 重点覆盖三类**不会报错**的失效：
//! ① 漏记（少一个字段也入了账 ⇒ 那东西将来没人删）；
//! ② 错记（把用户原有的文件当成自己的 ⇒ 真删用户的东西）；
//! ③ 顺序不定（清单每次不一样 ⇒ 日志 diff 永远在变、测试钉不住）。

use super::*;

fn acct(s: &str) -> AccountId {
    AccountId::new(s)
}

fn art(account: &str, id: &str) -> Artifact {
    Artifact::cloud_file(acct(account), id)
}

const RUN: RunId = RunId(1);

// ══════════════════════════ 记账 ══════════════════════════

#[test]
fn register_then_pending_returns_it() {
    let l = ArtifactLedger::new();
    l.register(RUN, art("19****01", "F1")).unwrap();

    assert_eq!(l.pending(RUN), vec![art("19****01", "F1")]);
    assert_eq!(l.count(RUN), 1);
    assert_eq!(l.total_pending(), 1);
}

/// 登记幂等：重试同一次上传拿到同一个 ID，计数不该变两番。
#[test]
fn registering_the_same_artifact_twice_counts_once() {
    let l = ArtifactLedger::new();
    l.register(RUN, art("19****01", "F1")).unwrap();
    l.register(RUN, art("19****01", "F1")).unwrap();

    assert_eq!(l.count(RUN), 1, "同一条记两次不该变两笔");
}

/// 三条字段少任何一条都必须**报错**，而不是静默入账。
///
/// 这是"漏记"那一类的入口：登记不全 = 一定有东西留在用户网盘上却没人知道。
/// ⚠ 少报一条的表现不是崩溃，而是**那个文件永远不删**。
#[test]
fn incomplete_registration_is_rejected() {
    let l = ArtifactLedger::new();

    // 缺 kind
    let no_kind = Artifact::new(ArtifactKind::new(""), acct("19****01"), "F1");
    assert!(l.register(RUN, no_kind).is_err(), "空 kind 不该入账");

    // 缺 account
    let no_acct = Artifact::cloud_file(acct(""), "F1");
    assert!(l.register(RUN, no_acct).is_err(), "空 account 不该入账");

    // 缺 id
    let no_id = Artifact::cloud_file(acct("19****01"), "");
    assert!(l.register(RUN, no_id).is_err(), "空 id 不该入账");

    // 一条都没进去
    assert_eq!(l.count(RUN), 0, "被拒的登记不该留下痕迹");
    assert_eq!(l.total_pending(), 0);
}

// ══════════════════════════ 错记防护（最重要）══════════════════════════

/// ⭐ **`pre_existing = true` 的东西绝不能入账** —— 这条是防误删的唯一防线。
///
/// 上游没有"这个文件是谁创建的"这种归属信息，`batchTrash` 又只认 `fileId`。
/// 所以把用户**原本就有**的文件（秒传命中 / 撞上同名目录）记进去，
/// 就会按 ID 删掉一个属于用户的对象。
///
/// 变异测试：把 `register_created` 里的 `if pre_existing` 判断去掉，
/// 这条必须红。
#[test]
fn pre_existing_files_are_never_registered() {
    let l = ArtifactLedger::new();

    let added = l
        .register_created(RUN, art("19****01", "USER-OWN-FILE"), true)
        .unwrap();

    assert!(!added, "本来就存在的文件不该算作本 run 的产物");
    assert_eq!(
        l.count(RUN),
        0,
        "⚠ 用户原有的文件被登记了 —— 清理会把它删掉"
    );
    assert!(l.pending(RUN).is_empty());
}

/// 反向：真的新造出来的必须记上（否则就是漏记）。
#[test]
fn created_files_are_registered() {
    let l = ArtifactLedger::new();
    let added = l
        .register_created(RUN, art("19****01", "NEW-1"), false)
        .unwrap();

    assert!(added, "新造的文件必须记上");
    assert_eq!(l.pending(RUN), vec![art("19****01", "NEW-1")]);
}

/// `added` 必须能区分"记上了"与"本来就存在"——
/// 这两件事在界面上的意思**相反**。
#[test]
fn added_flag_distinguishes_created_from_pre_existing() {
    let l = ArtifactLedger::new();
    let fresh = l.register_created(RUN, art("a", "X"), false).unwrap();
    let old = l.register_created(RUN, art("a", "Y"), true).unwrap();

    assert!(fresh, "新造的：记上了");
    assert!(!old, "已有的：没记");
    assert_eq!(l.pending(RUN), vec![art("a", "X")], "只有新造的那个在账上");
}

// ══════════════════════════ 销账 ══════════════════════════

#[test]
fn mark_removed_clears_the_entry() {
    let l = ArtifactLedger::new();
    let a = art("19****01", "F1");
    l.register(RUN, a.clone()).unwrap();

    assert!(l.mark_removed(RUN, &a), "账上有这一条，应当销得掉");
    assert_eq!(l.count(RUN), 0);
    assert!(l.pending(RUN).is_empty());
}

/// 销第二次返回 `false`（幂等，不是错误）。
#[test]
fn marking_removed_twice_is_false_not_an_error() {
    let l = ArtifactLedger::new();
    let a = art("19****01", "F1");
    l.register(RUN, a.clone()).unwrap();

    assert!(l.mark_removed(RUN, &a));
    assert!(!l.mark_removed(RUN, &a), "已经销过的不该再报成功");
}

/// **三个字段都得对上**：ID 对但账号不对，销不掉。
///
/// 这条防的是"手里的三元组和当初登记的那条不一致"，而那种不一致
/// 如果被当成"删好了"，那个文件就永远留在网盘上。
#[test]
fn mark_removed_requires_all_three_fields_to_match() {
    let l = ArtifactLedger::new();
    l.register(RUN, art("19****01", "F1")).unwrap();

    // 账号不同
    assert!(
        !l.mark_removed(RUN, &art("19****02", "F1")),
        "账号对不上不该销账"
    );
    // 类型不同
    let other_kind = Artifact::new(ArtifactKind::new("cloud-note"), acct("19****01"), "F1");
    assert!(!l.mark_removed(RUN, &other_kind), "类型对不上不该销账");
    // 三个都对的仍销得掉
    assert_eq!(l.count(RUN), 1, "前两次误销不该动到真正的账");
    assert!(l.mark_removed(RUN, &art("19****01", "F1")));
}

// ══════════════════════════ 按账号切清单 ══════════════════════════

/// 清理按账号打（凭据不是同一串），所以分组必须在账本里定。
#[test]
fn pending_for_filters_by_account() {
    let l = ArtifactLedger::new();
    l.register(RUN, art("A", "F1")).unwrap();
    l.register(RUN, art("A", "F2")).unwrap();
    l.register(RUN, art("B", "F3")).unwrap();

    assert_eq!(
        l.pending_for(RUN, &acct("A")),
        vec![art("A", "F1"), art("A", "F2")]
    );
    assert_eq!(l.pending_for(RUN, &acct("B")), vec![art("B", "F3")]);
    assert!(l.pending_for(RUN, &acct("C")).is_empty(), "没有它的账");

    // 全部清单不受影响
    assert_eq!(l.count(RUN), 3);
}

/// 销账按账号隔离：销 A 的不影响 B 的。
#[test]
fn removing_one_accounts_artifact_leaves_others() {
    let l = ArtifactLedger::new();
    l.register(RUN, art("A", "F1")).unwrap();
    l.register(RUN, art("B", "F2")).unwrap();

    assert!(l.mark_removed(RUN, &art("A", "F1")));
    assert_eq!(l.pending_for(RUN, &acct("B")), vec![art("B", "F2")]);
    assert_eq!(l.count(RUN), 1);
}

// ══════════════════════════ 顺序确定性 ══════════════════════════

/// ⭐ 清单顺序**必须稳定**。
///
/// 内层是 `HashSet`，遍历顺序不保证稳定（每个进程哈希种子都不同）。
/// 不排序的表现不是报错，而是"清理日志逐行 diff 永远在变"。
///
/// 这条用**大量条目**压：条目多到哈希顺序几乎不可能恰好等于字典序。
#[test]
fn pending_is_deterministically_ordered() {
    let l = ArtifactLedger::new();
    // 乱序登记，且跨越多个账号与类型
    for id in ["F9", "F1", "F5", "F3", "F7", "F2", "F8", "F4", "F6", "F10"] {
        l.register(RUN, art("acct-b", id)).unwrap();
        l.register(RUN, art("acct-a", id)).unwrap();
    }

    let first = l.pending(RUN);
    assert_eq!(first.len(), 20);

    // 重复取多次必须逐字相同
    for _ in 0..5 {
        assert_eq!(l.pending(RUN), first, "同一份账两次给出的顺序不同");
    }

    // 且确实是排好序的（按 Artifact 的字段序）
    let mut sorted_copy = first.clone();
    sorted_copy.sort_unstable();
    assert_eq!(first, sorted_copy, "返回的清单必须是排好序的");

    // 再验一次"确实按 ID 排"（同一账号内）
    let a_ids: Vec<&str> = first
        .iter()
        .filter(|x| x.account == acct("acct-a"))
        .map(|x| x.id.as_str())
        .collect();
    let mut expected = a_ids.clone();
    expected.sort_unstable();
    assert_eq!(a_ids, expected, "同账号内应按 ID 字典序");
}

// ══════════════════════════ 交回未清的账 ══════════════════════════

/// ⭐ `forget` 必须**把没清的东西交回来**。
///
/// 账本消失不等于用户网盘上的文件消失。旧项目的外溢与漏清，
/// 本质都是"没人负责这批东西"。
#[test]
fn forget_hands_back_what_is_still_pending() {
    let l = ArtifactLedger::new();
    l.register(RUN, art("A", "F1")).unwrap();
    l.register(RUN, art("A", "F2")).unwrap();

    let leftover = l.forget(RUN);
    assert_eq!(
        leftover,
        vec![art("A", "F1"), art("A", "F2")],
        "忘掉账本时剩下的东西必须交回来"
    );
    assert_eq!(l.count(RUN), 0, "之后不再跟踪这个 run");
    assert!(l.pending(RUN).is_empty());
}

/// 清干净之后再 `forget`：交回空清单（此时可以不管）。
#[test]
fn forget_after_cleanup_hands_back_nothing() {
    let l = ArtifactLedger::new();
    let a = art("A", "F1");
    l.register(RUN, a.clone()).unwrap();
    l.mark_removed(RUN, &a);

    assert!(l.forget(RUN).is_empty(), "清完了就没什么要交回的");
}

/// `forget` 一个从没记过账的 run：空清单，不是错误。
#[test]
fn forgetting_an_unknown_run_is_empty() {
    let l = ArtifactLedger::new();
    assert!(l.forget(RUN).is_empty());
    assert!(l.forget(RunId(999)).is_empty());
}

// ══════════════════════════ 多 run 隔离 ══════════════════════════

/// 账本按 run 隔离：一次运行的产物不该串到另一次。
#[test]
fn runs_are_isolated_from_each_other() {
    let l = ArtifactLedger::new();
    l.register(RunId(1), art("A", "F1")).unwrap();
    l.register(RunId(2), art("A", "F2")).unwrap();

    assert_eq!(l.pending(RunId(1)), vec![art("A", "F1")]);
    assert_eq!(l.pending(RunId(2)), vec![art("A", "F2")]);
    assert_eq!(l.total_pending(), 2);

    // 忘掉一个，另一个不受影响
    assert_eq!(l.forget(RunId(1)), vec![art("A", "F1")]);
    assert_eq!(l.pending(RunId(2)), vec![art("A", "F2")]);
}

/// `runs()` 升序，供诊断与"还有几笔没清"的读数。
#[test]
fn runs_lists_tracked_ids_in_order() {
    let l = ArtifactLedger::new();
    for id in [7u64, 2, 9, 4] {
        l.register(RunId(id), art("A", "F")).unwrap();
    }
    assert_eq!(l.runs(), vec![RunId(2), RunId(4), RunId(7), RunId(9)]);
}

// ══════════════════════════ 形状与安全 ══════════════════════════

/// 序列化用 `camelCase`（新增类型一律如此）。
#[test]
fn artifact_serializes_in_camel_case() {
    let j = serde_json::to_value(art("19****01", "F1")).unwrap();
    assert_eq!(j["kind"], "cloud-file");
    assert_eq!(j["account"], "19****01");
    assert_eq!(j["id"], "F1");
    // 不该冒出一个 snake_case 的键
    assert!(j.get("Kind").is_none());
}

/// ⭐ 诊断文本**绝不能**带凭据。
///
/// 账本条目会进日志与 run 记录，一句"顺手带上 token 方便排查"
/// 就能把凭据写进日志。
#[test]
fn artifact_display_carries_no_credential() {
    let a = art("19****01", "F1");
    let text = a.to_string();

    assert_eq!(text, "cloud-file#19****01/F1");
    for forbidden in ["auth", "token", "Basic", "Authorization"] {
        assert!(
            !text
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase()),
            "诊断文本里出现了 {forbidden}：{text}"
        );
    }
}

/// 账号在诊断文本里是**原样**的掩码号（它本身就是掩码的，不是凭据）。
#[test]
fn artifact_display_uses_the_masked_account_verbatim() {
    let a = art("19****01", "F1");
    assert!(a.to_string().contains("19****01"));
}

/// 类型是**开放**的：新增一种产物不需要改任何既有代码。
#[test]
fn artifact_kind_is_open_ended() {
    let l = ArtifactLedger::new();
    // 一个本仓今天还不存在的类型
    let note = Artifact::new(ArtifactKind::new("cloud-note"), acct("A"), "N1");
    l.register(RUN, note.clone()).unwrap();

    assert_eq!(l.pending(RUN), vec![note.clone()]);
    assert!(l.mark_removed(RUN, &note));
}

/// 零值账本可用（不 panic）。
///
/// 与 `CancelRegistry` 同样取舍：一个还没 `new()` 的账本不该让进程崩，
/// 但它返回的"没有这笔账"**不能**被当成"清理干净了"——
/// 所以 `Default` 只是让它可用，正常路径仍必须显式构造并共享。
#[test]
fn default_ledger_is_usable_and_empty() {
    let l = ArtifactLedger::default();
    assert_eq!(l.count(RUN), 0);
    assert!(l.pending(RUN).is_empty());
    assert!(l.runs().is_empty());
    assert_eq!(l.total_pending(), 0);
    // 登记也应当能用
    l.register(RUN, art("A", "F1")).unwrap();
    assert_eq!(l.count(RUN), 1);
}

/// 账本可跨线程共享（`Arc` + 内部锁），且并发登记不丢条目。
#[test]
fn concurrent_registration_loses_nothing() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let mut handles = Vec::new();

    for t in 0..8 {
        let l = std::sync::Arc::clone(&l);
        handles.push(std::thread::spawn(move || {
            for i in 0..50 {
                l.register(RUN, art(&format!("acct-{t}"), &format!("F{i}")))
                    .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(l.total_pending(), 400, "并发登记一条都不能丢");
    // 每个账号各有 50 条
    for t in 0..8 {
        assert_eq!(l.pending_for(RUN, &acct(&format!("acct-{t}"))).len(), 50);
    }
}

// ══════════════════════════ 任务侧窄句柄 ══════════════════════════

/// 窄句柄自动带上 `run_id` 与 `account` —— 任务不需要（也不能）自己传。
#[test]
fn recorder_fills_run_and_account_automatically() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(std::sync::Arc::clone(&l), RunId(42), acct("19****01"));

    assert!(rec.register_cloud_file("F1", false).unwrap());
    assert_eq!(rec.pending_count(), 1);
    assert_eq!(l.pending(RunId(42)), vec![art("19****01", "F1")]);
    // 换一个 run 的账本是空的（没有串台）
    assert_eq!(l.count(RunId(43)), 0);
}

/// 窄句柄的 `pre_existing` 一样受防线保护。
#[test]
fn recorder_respects_the_pre_existing_guard() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(std::sync::Arc::clone(&l), RUN, acct("A"));

    assert!(!rec.register_cloud_file("USER-OWN", true).unwrap());
    assert_eq!(rec.pending_count(), 0, "用户原有的文件不该入账");
}

/// `pending`/`pending_count` 只看**本账号**的账。
#[test]
fn recorder_sees_only_its_own_account() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec_a = ArtifactRecorder::new(std::sync::Arc::clone(&l), RUN, acct("A"));

    rec_a.register_cloud_file("F1", false).unwrap();
    // 另一个账号往同一个 run 里记
    l.register(RUN, art("B", "F2")).unwrap();

    assert_eq!(rec_a.pending_count(), 1);
    assert_eq!(rec_a.pending(), vec![art("A", "F1")], "看不到别人的产物");
    assert_eq!(l.count(RUN), 2, "但账本总量是两条");
}

/// 链尾清理后销账：清单归零（否则每次运行都误报"留了 N 个"）。
#[test]
fn recorder_can_settle_after_cleaning_up() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(std::sync::Arc::clone(&l), RUN, acct("A"));

    rec.register_cloud_file("F1", false).unwrap();
    rec.register_cloud_file("F2", false).unwrap();
    assert_eq!(rec.pending_count(), 2);

    let removed = rec.mark_cloud_files_removed(&["F1".to_string(), "F2".to_string()]);
    assert_eq!(removed, 2, "两条都该销得掉");
    assert_eq!(rec.pending_count(), 0, "清完了清单必须归零");
}

/// 销账返回**成功条数**：只删掉一部分时能看出来。
#[test]
fn removing_a_batch_reports_how_many_settled() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(std::sync::Arc::clone(&l), RUN, acct("A"));
    rec.register_cloud_file("F1", false).unwrap();

    // F2 账上没有 ⇒ 只销掉 1 条
    let removed = rec.mark_cloud_files_removed(&["F1".to_string(), "F2".to_string()]);
    assert_eq!(removed, 1, "只有 1 条真的销掉了");
}

/// ⭐ 窄句柄**不能**销别的账号的账。
///
/// 跨账号销账在上游是 401/拒绝，在这里若被允许就是一次**静默的漏记**
/// —— 那条记录从账上消失，但它对应的文件还在别人的网盘里。
#[test]
fn recorder_cannot_settle_another_accounts_artifact() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(std::sync::Arc::clone(&l), RUN, acct("A"));
    l.register(RUN, art("B", "F2")).unwrap();

    assert!(!rec.mark_removed(&art("B", "F2")), "不该销掉别的账号的账");
    assert_eq!(l.count(RUN), 1, "那条记录必须还在");
}

/// 销账要求三个字段完全对上，对不上就是 `false`（不是"删好了"）。
#[test]
fn recorder_settle_requires_exact_match() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(std::sync::Arc::clone(&l), RUN, acct("A"));
    rec.register_cloud_file("F1", false).unwrap();

    // 类型不同 ⇒ 销不掉
    let wrong_kind = Artifact::new(ArtifactKind::new("cloud-note"), acct("A"), "F1");
    assert!(!rec.mark_removed(&wrong_kind));
    // ID 不同 ⇒ 销不掉
    assert!(!rec.mark_cloud_file_removed("F9"));
    // 完全对上 ⇒ 销得掉
    assert!(rec.mark_cloud_file_removed("F1"));
}

/// 窄句柄的 `Debug` 不吐凭据，也不把账本内部摊开。
#[test]
fn recorder_debug_is_redacted() {
    let l = std::sync::Arc::new(ArtifactLedger::new());
    let rec = ArtifactRecorder::new(l, RUN, acct("19****01"));
    let text = format!("{rec:?}");

    assert!(text.contains("19****01"), "账号是掩码号，可以出现");
    assert!(!text.contains("ArtifactLedger"), "不该暴露账本内部：{text}");
}
