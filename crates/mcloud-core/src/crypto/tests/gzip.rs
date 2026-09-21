// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

// ── gzip ────────────────────────────────────────────────────────────────

#[test]
fn gzip_sets_os_byte_to_19_and_roundtrips() {
    use std::io::Read as _;
    let v = vectors();
    for c in &v.gzip {
        let gz = gzip_with_os19(c.json.as_bytes()).unwrap();
        assert_eq!(gz[GZIP_OS_INDEX], 19, "OS 字节必须是 19（用例 {}）", c.name);
        assert_eq!(&gz[0..2], &[0x1f, 0x8b], "gzip magic 不对");

        let mut dec = flate2::read::GzDecoder::new(std::io::Cursor::new(&gz));
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        assert_eq!(s, c.json, "解压后应等于原 JSON（用例 {}）", c.name);
    }
}

/// Node 原样产出的 base64 也必须能被我们解开（证明头部兼容）。
#[test]
fn gzip_decodes_node_produced_vector() {
    use std::io::Read as _;
    let v = vectors();
    let c = &v.gzip[0];
    let raw = b64_decode(&c.base64).unwrap();
    let mut dec = flate2::read::GzDecoder::new(std::io::Cursor::new(&raw));
    let mut s = String::new();
    dec.read_to_string(&mut s).unwrap();
    assert_eq!(s, c.json);
    assert_eq!(s.len(), c.plain_len);
}
