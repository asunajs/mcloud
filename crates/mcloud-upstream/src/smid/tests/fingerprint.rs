//! 特征体：哪些字段随随机源变、哪些必须一字不差。

use super::*;

// ==================== 特征体 ====================

#[test]
fn bound_fields_come_from_the_fingerprint_not_from_the_rng() {
    // 两次用**完全不同的随机源**：稳定字段必须一字不差，只有 volatile 的变。
    // 这就是"特征绑定到账号"在本模块里的落点 —— 旧实现这里每次都重掷。
    let upload = upload();
    let a = fingerprint_target(&fp(), &upload, &mut seq_rng(1));
    let b = fingerprint_target(&fp(), &upload, &mut seq_rng(200));

    for field in [
        "platform",
        "clientSize",
        "res",
        "canvas",
        "plugins",
        "ua",
        "url",
        "maxTouchPoints",
        "connectionRtt",
        "cpucount",
        "protocol",
        "organization",
        "appId",
        "os",
        "version",
        "sdkver",
        "subVersion",
        "rtype",
        "status",
        "timezone",
        "time",
        "referer",
    ] {
        assert_eq!(a[field], b[field], "稳定字段 {field} 不该随随机源变");
    }
    for field in ["smid", "vpw", "trees"] {
        assert_ne!(
            a[field], b[field],
            "{field} 内含现生成的 uuid，必须每次不同"
        );
    }
}

#[test]
fn fingerprint_generation_draws_every_field_from_its_pool() {
    let generated = SmFingerprint::generate(&mut seq_rng(3));
    assert!(
        SCREENS
            .iter()
            .any(|(w, h)| (*w as i64) == generated.screen_width as i64
                && (*h as i64) == generated.screen_height as i64),
        "屏幕尺寸要落在旧池子里：{:?}",
        (generated.screen_width, generated.screen_height)
    );
    assert!(PLATFORMS.contains(&generated.platform.as_str()));
    assert!(DPRS.contains(&generated.dpr));
    assert!(MAX_TOUCH_POINTS.contains(&generated.max_touch_points));
    assert!(CONNECTION_RTT.contains(&generated.connection_rtt));
    assert!(CPU_COUNTS.contains(&generated.cpucount));
    assert_eq!(generated.canvas.len(), 8, "canvas 是 md5 前 8 位");
    // 换一份种子就该换个长相（指纹若恒定，"绑定账号"就没有意义）
    assert_ne!(generated, SmFingerprint::generate(&mut seq_rng(140)));
}

#[test]
fn fingerprint_survives_a_store_round_trip() {
    let original = SmFingerprint::generate(&mut seq_rng(9));
    let text = original.to_json_string().unwrap();
    assert_eq!(SmFingerprint::from_json_str(&text).unwrap(), original);
    // 库里如果是半截 JSON，要报错而不是悄悄换一份长相
    assert!(SmFingerprint::from_json_str("{\"ua\":").is_err());
}

#[test]
fn old_device_id_goes_into_the_box_field() {
    let upload = SmUpload {
        time_digits: "20260919153000",
        now_millis: 1,
        old_device_id: "B1990000000000000000000000000000000000000000000001",
    };
    let target = fingerprint_target(&fp(), &upload, &mut seq_rng(1));
    assert_eq!(target["box"], upload.old_device_id);
}

#[test]
fn client_size_and_res_encode_screen_and_dpr() {
    let f = fp();
    assert_eq!(f.client_size(), "0_0_0_0_375_667_375_667");
    assert_eq!(f.res(), "375_667_2_2");
    // dpr 只出现在 res 的第 3 段
    let other = f.clone().with_canvas("deadbeef");
    assert_eq!(other.res(), f.res());
    assert_eq!(other.canvas, "deadbeef");
}
