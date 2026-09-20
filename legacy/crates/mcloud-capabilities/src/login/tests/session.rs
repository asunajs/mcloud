// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

use mcloud_core::UserId;

use serde_json::json;

#[test]
fn auth_is_rebuilt_exactly_like_the_client_does() {
    // utils.js L230: "Basic " + btoa("pc:" + account + ":" + authToken)
    let auth = auth_from_user_info(&user_info()).unwrap();
    assert_eq!(
        auth,
        format!(
            "Basic {}",
            b64("pc:19900000000:SYNTH001|1|RCS|1893456000000|AAAA")
        )
    );
    assert!(auth.starts_with("Basic "));
    // 而且它必须是既有解析器认得的形状
    let parsed = mcloud_core::parse_auth(&auth).unwrap();
    assert_eq!(parsed.platform.as_deref(), Some("pc"));
    assert_eq!(parsed.phone.as_deref(), Some("19900000000"));
    assert_eq!(parsed.expire_millis(), 1_893_456_000_000);
}

#[test]
fn missing_fields_are_reported_by_name() {
    // 上游改了字段名时，必须立刻说出缺哪个 —— 而不是拼出一个空 auth 往下跑
    for missing in ["account", "authToken"] {
        let mut info = user_info();
        info[missing] = json!("");
        let err = auth_from_user_info(&info).unwrap_err();
        assert!(format!("{err}").contains(missing), "{err}");
    }
}

#[test]
fn app_prefix_produces_mobile_not_pc() {
    // App 端实测：Basic base64("mobile:<手机号>:<authToken>")
    // （App 日志 1076 处 Authorization 解码后全是这个形状）
    let auth = auth_from_user_info_with_prefix(&user_info(), "mobile").unwrap();
    assert_eq!(
        auth,
        format!(
            "Basic {}",
            b64("mobile:19900000000:SYNTH001|1|RCS|1893456000000|AAAA")
        )
    );
    // 前缀进得了既有解析器，所以 App 的登录结果不需要第二套模型
    let parsed = mcloud_core::parse_auth(&auth).unwrap();
    assert_eq!(parsed.platform.as_deref(), Some("mobile"));
    assert_eq!(parsed.phone.as_deref(), Some("19900000000"));
    assert_eq!(parsed.expire_millis(), 1_893_456_000_000);

    // 反证：不能和 PC 的那个撞成一样
    assert_ne!(auth, auth_from_user_info(&user_info()).unwrap());
}

#[test]
fn app_session_payload_is_understood_without_faking_user_id() {
    // App 的会话载荷用 `userDomainId`，PC 用 `userId` —— 是同一个东西。
    // 这里钉住"两种名字都认"，免得有人为了复用而在 App 侧伪造一个 `userId`。
    let mut info = user_info();
    let domain = info["userId"].clone();
    info.as_object_mut().unwrap().remove("userId");
    info["userDomainId"] = domain.clone();

    let (_, state) =
        account_and_state_from_login_with_prefix(Scope::Local, "DEV", &info, "mobile", now())
            .unwrap();
    assert_eq!(state.user_domain_id, domain.as_str().map(str::to_string));
    assert_eq!(
        state.jwt_token.as_deref(),
        Some("SYNTH001|1|RCS|1893456000000|AAAA")
    );
}

#[test]
fn login_result_maps_to_an_owned_account_and_state() {
    let user = UserId::new("u1");
    let (account, state) = account_and_state_from_login(
        Scope::user(&user),
        "62AA2BDA29F99744536E142A83CA5B03-ENDIN",
        &user_info(),
        now(),
    )
    .unwrap();

    assert_eq!(account.owner.as_ref(), Some(&user));
    assert_eq!(account.id.as_str(), "19****00");
    assert_eq!(account.phone_masked.as_deref(), Some("19****00"));
    assert_eq!(account.platform.as_deref(), Some("pc"));
    assert_eq!(account.auth_expire, Some(1_893_456_000_000));
    assert!(account.is_visible_to(&user));
    assert!(!account.is_visible_to(&UserId::new("别人")));

    assert_eq!(state.account_id, account.id);
    assert_eq!(
        state.jwt_token.as_deref(),
        Some("SYNTH001|1|RCS|1893456000000|AAAA")
    );
    assert_eq!(
        state.device_id.as_deref(),
        Some("62AA2BDA29F99744536E142A83CA5B03-ENDIN")
    );
    assert_eq!(state.user_domain_id.as_deref(), Some("1990000000000000001"));
    assert_eq!(state.updated_at, Some(now()));
}

#[test]
fn account_id_derives_from_the_same_masking_path_as_legacy_import() {
    // 两条路（旧数据导入 / 登录）必须得出同一个 AccountId，
    // 否则同一个云盘账号会被当成两个
    let (account, _) = account_and_state_from_login(
        Scope::user(&UserId::new("u1")),
        "dev",
        &json!({"account": "19900000000", "authToken": "T|1|R|1|A"}),
        now(),
    )
    .unwrap();
    assert_eq!(
        account.id,
        AccountId::new(mcloud_core::account_label(None, "19900000000"))
    );
}
