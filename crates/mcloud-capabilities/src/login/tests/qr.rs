// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

use serde_json::json;

#[tokio::test]
async fn qr_login_walks_not_scanned_scanned_success() {
    let client = ScriptedClient::new(vec![
        json!({"success": false, "code": "200059541", "message": "未找到设备的二维码信息"}),
        json!({"success": false, "code": "200059548", "message": "二维码信息未授权"}),
        json!({"success": true, "code": "0000", "data": user_info().to_string()}),
    ]);
    let upstream = Upstream::new(&client);
    let mut qr = QrLogin::start(
        PcPlatform::Windows,
        "62AA2BDA29F99744536E142A83CA5B03-ENDIN",
        &mut SeqRandom(0),
        now(),
    );

    let mut seen = Vec::new();
    while qr.next_delay_ms(now()).is_some() {
        seen.push(
            qr.poll(&upstream, "tid", &PlainDecryptor, now())
                .await
                .unwrap(),
        );
        if seen.last().unwrap().is_terminal() {
            break;
        }
    }

    assert_eq!(
        seen,
        vec![
            QrProgress::NotScanned,
            QrProgress::Scanned,
            QrProgress::LoggedIn {
                user_info: user_info()
            }
        ]
    );
    assert_eq!(client.sent(), 3);
}

/// 客户端源码那句"登录成功后必须 clearInterval"：终态之后**不能再发请求**。
///
/// 写成断言而不是注释，是因为"忘了停轮询"正是原实现专门标出来提醒自己的坑。
#[tokio::test]
async fn qr_login_stops_polling_once_terminal() {
    let client = ScriptedClient::new(vec![
        json!({"success": true, "code": "0000", "data": user_info().to_string()}),
    ]);
    let upstream = Upstream::new(&client);
    let mut qr = QrLogin::start(PcPlatform::Windows, "dev", &mut SeqRandom(0), now());

    let first = qr
        .poll(&upstream, "tid", &PlainDecryptor, now())
        .await
        .unwrap();
    assert!(matches!(first, QrProgress::LoggedIn { .. }));
    assert_eq!(client.sent(), 1);

    // 再问十次：结论不变，而且**一次请求都没发**
    for _ in 0..10 {
        assert_eq!(
            qr.poll(&upstream, "tid", &PlainDecryptor, now())
                .await
                .unwrap(),
            first
        );
    }
    assert_eq!(client.sent(), 1, "终态之后不得再发请求");
    assert_eq!(qr.next_delay_ms(now()), None, "终态必须让调用方停下来");
}

#[tokio::test]
async fn qr_login_gives_up_at_its_own_ttl_without_asking_upstream() {
    let client = ScriptedClient::new(vec![]);
    let upstream = Upstream::new(&client);
    let mut qr = QrLogin::start(PcPlatform::Windows, "dev", &mut SeqRandom(0), now());

    let later = now() + Duration::seconds(QR_TTL_SECONDS);
    assert!(qr.is_expired(later));
    assert_eq!(qr.next_delay_ms(later), None);

    let progress = qr
        .poll(&upstream, "tid", &PlainDecryptor, later)
        .await
        .unwrap();
    assert_eq!(
        progress,
        QrProgress::Failed {
            reason: QrFailure::TtlElapsed {
                ttl_seconds: QR_TTL_SECONDS
            }
        }
    );
    assert_eq!(client.sent(), 0, "超时就不该再打扰上游");
}

#[tokio::test]
async fn qr_login_maps_the_documented_terminal_codes() {
    for (code, expect) in [
        ("200059542", QrFailure::ExpiredUpstream),
        ("200059544", QrFailure::SessionMismatch),
    ] {
        let client = ScriptedClient::new(vec![json!({"success": false, "code": code})]);
        let upstream = Upstream::new(&client);
        let mut qr = QrLogin::start(PcPlatform::Windows, "dev", &mut SeqRandom(0), now());

        assert_eq!(
            qr.poll(&upstream, "tid", &PlainDecryptor, now())
                .await
                .unwrap(),
            QrProgress::Failed { reason: expect }
        );
        assert!(qr.next_delay_ms(now()).is_none(), "失败也是终态");
    }
}

#[tokio::test]
async fn qr_login_keeps_unknown_codes_verbatim() {
    let client = ScriptedClient::new(vec![
        json!({"success": false, "code": 200999999, "message": "新码"}),
    ]);
    let upstream = Upstream::new(&client);
    let mut qr = QrLogin::start(PcPlatform::Windows, "dev", &mut SeqRandom(0), now());

    match qr
        .poll(&upstream, "tid", &PlainDecryptor, now())
        .await
        .unwrap()
    {
        QrProgress::Failed {
            reason: QrFailure::Rejected { code, message },
        } => {
            assert_eq!(code, Some(200_999_999));
            assert_eq!(message, "新码");
        }
        other => panic!("应当原样带回，实际 {other:?}"),
    }
}

#[tokio::test]
async fn qr_poll_request_carries_session_id_as_dycpwd() {
    let client = ScriptedClient::new(vec![json!({"code": "200059541"})]);
    let upstream = Upstream::new(&client);
    let mut qr = QrLogin::start(PcPlatform::Windows, "dev-1", &mut SeqRandom(0), now());
    let session_id = qr.session_id().to_string();

    qr.poll(&upstream, "tid-9", &PlainDecryptor, now())
        .await
        .unwrap();

    let request = &client.seen.borrow()[0];
    assert_eq!(request.url, "https://user-njs.yun.139.com/user/thirdlogin");
    assert!(
        request
            .headers
            .contains(&("x-yun-tid".to_string(), "tid-9".to_string())),
        "每个请求都要带新的 x-yun-tid"
    );
    let body: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
    // sID 与 dycpwd 是同一个值：出错的话轮询会一直停在"未找到设备"
    assert_eq!(body["dycpwd"], session_id);
    assert_eq!(body["pintype"], "21");
    assert_eq!(body["clientType"], "656");
}

#[test]
fn qr_payload_is_built_locally_with_the_session_id() {
    let qr = QrLogin::start(PcPlatform::MacOs, "DEV-ID", &mut SeqRandom(0), now());
    let payload = qr.qr_payload();
    assert!(payload.starts_with("https://yun.139.com/w/#/qrcLogin?dID=DEV-ID&sID="));
    assert!(payload.contains(qr.session_id()));
    assert!(payload.ends_with("cType=11&ver=4.2.1"), "{payload}");
    // sID 形状：32 位大写十六进制
    assert_eq!(qr.session_id().len(), 32);
}
