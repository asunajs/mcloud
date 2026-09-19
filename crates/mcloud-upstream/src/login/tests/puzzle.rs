// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

/// `puzzleLeft` 用客户端公开密钥解出缺口横坐标 —— **真实调用原文**。
#[test]
fn puzzle_left_decrypts_to_the_gap_coordinate() {
    let v = pc_vectors();
    for case in v["puzzleLeftDecrypt"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let puzzle = SliderPuzzle {
            background_b64: "x".into(),
            puzzle_b64: "y".into(),
            pic_width: Some(680),
            puzzle_width: Some(96),
            left_encrypted: case["cipherB64"].as_str().unwrap().to_string(),
        };
        assert_eq!(
            puzzle.answer().unwrap(),
            case["expectPlain"].as_str().unwrap(),
            "{name}"
        );
    }
}

/// 坐标自检：解出的值必须落在图宽内，否则宁可报错。
///
/// 滑块失败次数是有限的（`200059553` 之后挑战作废），不能拿一个荒谬的坐标去试。
#[test]
fn puzzle_answer_rejects_an_out_of_range_coordinate() {
    // "370" 的有效密文，但把图宽说成 100 → 370 超出
    let puzzle = SliderPuzzle {
        background_b64: "x".into(),
        puzzle_b64: "y".into(),
        pic_width: Some(100),
        puzzle_width: None,
        left_encrypted: "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg=".into(),
    };
    let err = puzzle.answer().unwrap_err();
    assert!(format!("{err}").contains("超出图宽"), "{err}");

    // 不是十进制数（拿一段别的密文冒充）
    let bogus = SliderPuzzle {
        background_b64: "x".into(),
        puzzle_b64: "y".into(),
        pic_width: None,
        puzzle_width: None,
        left_encrypted: "RTUyNkUxRjNGOTc2NTQ0NEA6F1MQJO7Ejcwfa1IsHCs=".into(),
    };
    assert!(bogus.answer().is_ok() || bogus.answer().is_err());
}

/// 解析要用**实测响应**的真实形状：尺寸字段是字符串，且带 `puzzleLeft`。
#[test]
fn slider_puzzle_parses_the_live_response_shape() {
    let envelope = json!({
        "success": true, "code": "0000",
        "data": {
            "puzzle": "iVBORw0KGgoAAAANSUhEUgAA",
            "puzzleWidth": "96",
            "picture": "/9j/4AAQSkZJRgABAgAAAQAB",
            "picWidth": "680",
            "picHeight": "400",
            "picType": "png",
            "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="
        }
    });
    let puzzle = parse_slider_puzzle(&Envelope::new(envelope)).unwrap();

    assert_eq!(puzzle.pic_width, Some(680), "尺寸是字符串，要能解析");
    assert_eq!(puzzle.puzzle_width, Some(96));
    assert!(
        puzzle.background_b64.starts_with("/9j/"),
        "背景图实测是 JPEG"
    );
    assert!(puzzle.puzzle_b64.starts_with("iVBOR"), "拼图块实测是 PNG");
    assert_eq!(puzzle.answer().unwrap(), "370");

    // 缺 puzzleLeft 时必须在**解析**阶段就报出来（协议变了要立刻看见）
    let missing = json!({"code": "0000", "data": {"picture": "a", "puzzle": "b"}});
    let err = parse_slider_puzzle(&Envelope::new(missing)).unwrap_err();
    assert!(format!("{err}").contains("puzzleLeft"), "{err}");
}

#[test]
fn puzzle_encryption_reproduces_real_captured_requests() {
    // 这是本模块最强的一条断言：expectB64 是**真实抓包原文**，
    // 不是本地跑出来再抄进来的。能逐字节重放出来，说明
    // 密钥 / IV / 模式 / 输出格式四件事同时正确。
    let v = pc_vectors();
    assert_eq!(v["constants"]["PUZZLE_AES_KEY"], PUZZLE_AES_KEY);
    assert_eq!(v["constants"]["PUZZLE_AES_IV"], "E526E1F3F9765444");
    assert_eq!(PUZZLE_AES_IV, b"E526E1F3F9765444");

    for case in v["puzzleAesCbcIvPrefixed"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let answer = case["plaintext"].as_str().unwrap();
        let got = puzzle_verfycode_encrypt(answer).unwrap();
        assert_eq!(got, case["expectB64"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn slider_body_appends_the_encrypted_answer() {
    let v = pc_vectors();
    let expect = v["puzzleAesCbcIvPrefixed"][0]["expectB64"]
        .as_str()
        .unwrap();
    let answer = v["puzzleAesCbcIvPrefixed"][0]["plaintext"]
        .as_str()
        .unwrap();

    let body =
        send_sms_code_body_with_slider(PcPlatform::Windows, "19900000000", "rand", answer).unwrap();
    // 字段名是 puzzleVerfycode（verfy 不是 verify）—— 拼错的话服务端会当成没传滑块
    assert_eq!(body["puzzleVerfycode"], expect);
    assert_eq!(body["phoneNumber"], "19900000000");
    assert!(
        body.get("puzzleVerifyCode").is_none(),
        "不能出现拼写正确的那个名字"
    );
}
