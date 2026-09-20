// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

/// 掩码撞车必须**中止迁移**：两个账号会塌缩成一个 `AccountId`，且不会报错。
#[test]
fn mask_collision_aborts_migration() {
    let views = describe_legacy_accounts(&[
        account("19000000001", Some("甲")),
        account("19100000001", Some("乙")),
    ]);
    // 两个号码前 2 后 2 都是 19 / 01
    assert_eq!(views[0].account_id.as_str(), "19****01");
    assert_eq!(views[1].account_id.as_str(), "19****01");

    match build_identity_map(&views) {
        Err(LegacyError::MaskCollision { account_id, keys }) => {
            assert_eq!(account_id, "19****01");
            assert_eq!(keys, vec!["19000000001", "19100000001"]);
        }
        other => panic!("应报掩码冲突，实际 {other:?}"),
    }
}

#[test]
fn identity_map_is_built_for_distinct_masks() {
    let views = describe_legacy_accounts(&[
        account("19000000001", Some("甲")),
        account("19100000002", Some("乙")),
    ]);
    let map = build_identity_map(&views).expect("掩码不同应能建立映射");
    assert_eq!(map["19000000001"].as_str(), "19****01");
    assert_eq!(map["19100000002"].as_str(), "19****02");
}

/// 没有手机号的账号退化为昵称（旧 `accountKey` 的兜底链）。
#[test]
fn accounts_without_phone_fall_back_to_nickname() {
    let views = describe_legacy_accounts(&[
        LegacyAccountConfig {
            auth: "!!!not-base64!!!".into(),
            nickname: Some("备用账号".into()),
            phone: None,
        },
        LegacyAccountConfig {
            auth: String::new(),
            nickname: None,
            phone: None,
        },
    ]);
    assert_eq!(views[0].phone, "");
    assert_eq!(views[0].legacy_key, "备用账号");
    assert_eq!(views[0].account_id.as_str(), "备用账号");
    assert_eq!(views[0].label, "备用账号");

    assert_eq!(views[1].legacy_key, "account");
    assert_eq!(views[1].account_id.as_str(), "account");
    assert_eq!(views[1].label, "账号");
}
