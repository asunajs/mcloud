// 由 legacy.rs 的 `mod tests` 按主题拆出；夹具与 helper 见同目录 mod.rs。
use super::*;

use serde_json::json;

// ───────────────────────── 账号引用 ─────────────────────────

/// 旧 `findAccount`：**纯数字字符串按序号解释**，手机号字符串永远匹配不到。
#[test]
fn legacy_target_resolution_reproduces_find_account() {
    let views = fixture_views();
    let resolve = |value: Value| resolve_legacy_target(&views, Some(&value));

    assert_eq!(resolve_legacy_target(&views, None), LegacyTarget::All);

    assert_eq!(resolve(json!(1)), LegacyTarget::Index(1));
    assert_eq!(resolve(json!(2)), LegacyTarget::Index(2));
    assert_eq!(resolve(json!("1")), LegacyTarget::Index(1), "纯数字串=序号");
    assert_eq!(resolve(json!("账号1")), LegacyTarget::Named(2));
    assert_eq!(resolve(json!("账号2")), LegacyTarget::Named(1));

    // 越界序号
    assert_eq!(resolve(json!(0)), LegacyTarget::Unresolved("0".into()));
    assert_eq!(resolve(json!(3)), LegacyTarget::Unresolved("3".into()));
    assert_eq!(resolve(json!(-1)), LegacyTarget::Unresolved("-1".into()));

    // ★ 手机号字符串：被当成序号（19 亿），必然越界 —— 这就是那个陷阱
    assert_eq!(
        resolve(json!("19900000002")),
        LegacyTarget::Unresolved("19900000002".into()),
        "手机号字符串会被当序号：这正是旧版静默不执行的原因"
    );
    assert_eq!(resolve(json!("")), LegacyTarget::Unresolved(String::new()));
    assert_eq!(
        resolve(json!("不存在")),
        LegacyTarget::Unresolved("不存在".into())
    );
}

/// 迁移建议与旧语义分开表达：能对应上的会**明确标出这是一次行为变更**。
#[test]
fn advice_flags_references_that_never_took_effect() {
    let views = fixture_views();
    let advise = |value: Value| advise_reference(&views, Some(&value));

    assert_eq!(advise_reference(&views, None), ReferenceAdvice::AllAccounts);
    assert_eq!(advise(json!("账号1")), ReferenceAdvice::Same { index: 2 });
    assert_eq!(advise(json!(1)), ReferenceAdvice::Same { index: 1 });

    assert_eq!(
        advise(json!("19900000002")),
        ReferenceAdvice::RecoveredFromPhone {
            digits: "19900000002".into(),
            index: 2
        },
        "越界的数字串若是已知手机号，应给出可恢复的建议"
    );
    assert_eq!(
        advise(json!("12345678901")),
        ReferenceAdvice::WasNeverEffective {
            raw: "12345678901".into()
        },
        "不认识的数字串不能猜"
    );

    // 明确的分支才给结果；拿不准的一律让调用方去报告
    assert_eq!(
        resolve_reference_strict(&views, Some(&json!("19900000002"))),
        Ok(Some(AccountId::new("19****02")))
    );
    assert!(resolve_reference_strict(&views, Some(&json!("12345678901"))).is_err());
    assert_eq!(resolve_reference_strict(&views, None), Ok(None));
}
