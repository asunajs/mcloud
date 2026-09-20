// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

// ── Random trait ────────────────────────────────────────────────────────

#[test]
fn seeded_rng_is_reproducible() {
    let mut a = SeededRng::new(123);
    let mut b = SeededRng::new(123);
    let ua = a.uuid_v4();
    let ub = b.uuid_v4();
    assert_eq!(ua, ub, "同种子必须产出同序列");
    assert_eq!(ua.len(), 36);
    assert_eq!(ua.as_bytes()[14], b'4', "UUID 版本位应为 4");
}

#[test]
fn seeded_rng_below_stays_in_range() {
    let mut rng = SeededRng::new(5);
    for _ in 0..500 {
        assert!(rng.below(8) < 8);
    }
    assert!(rng.below(1) == 0);
}

#[test]
fn uuid_bits_are_set_correctly() {
    let mut rng = SeededRng::new(99);
    for _ in 0..50 {
        let u = rng.uuid_v4();
        let b = u.as_bytes();
        assert_eq!(b[14], b'4');
        let variant = u.as_bytes()[19];
        assert!(
            matches!(variant, b'8' | b'9' | b'a' | b'b'),
            "variant 位不对: {u}"
        );
    }
}
