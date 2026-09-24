//! 生成 `test/golden/datetime-format-vectors.json`。
//!
//! 这是**一次性数据变换**，不参与任何构建（同 `gen-*-vectors.rs` 那几份）。
//!
//! # 为什么要有这份向量
//!
//! `mcloud-io` 把时间列存成 **TEXT**，而 SQL 里对它做**字符串比较与排序**：
//!
//! ```sql
//! DELETE FROM sessions WHERE expires_at <= ?1        -- store/user.rs
//! SELECT ... FROM users ORDER BY created_at ASC, id ASC
//! DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE started_at < ?)
//! ```
//!
//! 而 chrono 的 `to_rfc3339()` 对 UTC 给 `+00:00`，Go 的 `time.RFC3339Nano`
//! 给 `Z` —— 同一列里混两种写法时，"同一时刻的先后"会取决于后缀字符
//! （`+` 是 0x2B、`Z` 是 0x5A ⇒ 同刻的 Rust 行永远排在 Go 行之前）。
//! 亚秒位数的取舍也不同（chrono AutoSi 取 0/3/6/9 位，Go 去掉末尾 0）。
//!
//! ⇒ Go 侧必须**逐字节**复刻 chrono 的输出，让这一列始终只有一种写法。
//! 复刻的规则**不能靠读文档推** —— 所以由 Rust 侧真跑一遍，把答案钉下来。
//!
//! # 为什么生成侧是 Rust
//!
//! 跨语言对拍要求**生成侧 ≠ 消费侧**：用 Go 生成再用 Go 验证是自证，
//! 两边一起错时照样绿。这里尤其明显 —— 要验证的正是"Go 的写法对不对"，
//! 用 Go 产出期望值等于什么都没验证。
//!
//! # 怎么离线重新生成
//!
//! ```text
//! mkdir -p /tmp/mcloud-dt/src
//! cp scripts/gen-datetime-format-vectors.rs /tmp/mcloud-dt/src/main.rs
//! cat > /tmp/mcloud-dt/Cargo.toml <<'TOML'
//! [package]
//! name = "mcloud-dt"
//! version = "0.0.0"
//! edition = "2021"
//!
//! [dependencies]
//! chrono = "0.4.45"
//! TOML
//! cd /tmp/mcloud-dt
//! CARGO_NET_OFFLINE=true cargo run --quiet > /tmp/datetime-format-vectors.json
//! ```
//!
//! ⚠ `chrono` 版本必须与 `Cargo.lock` 里解析到的**同一份**（0.4.45）——
//! 小数位数的取舍是版本相关的实现细节。

use chrono::{DateTime, Utc};
use std::fmt::Write as _;

fn main() {
    let mut emit: Vec<String> = Vec::new();
    let mut parse: Vec<String> = Vec::new();

    // ── 一、chrono 输出什么格式 ──────────────────────────────────────────
    //
    // 覆盖的正是"小数位数怎么选"那件事：0 位 / 3 位 / 6 位 / 9 位，
    // 以及各自的边界（刚好整除与差一点）。
    let emit_cases: Vec<(&str, i64, u32)> = vec![
        ("epoch", 0, 0),
        ("whole-second", 1_758_372_000, 0),
        // nanos % 1_000_000 == 0 ⇒ 3 位
        ("milli-100", 1_758_372_000, 100_000_000),
        ("milli-500", 1_758_372_000, 500_000_000),
        ("milli-001", 1_758_372_000, 1_000_000),
        ("milli-120", 1_758_372_000, 120_000_000),
        // nanos % 1_000 == 0 但 % 1_000_000 != 0 ⇒ 6 位
        ("micro-000001", 1_758_372_000, 1_000),
        ("micro-123456", 1_758_372_000, 123_456_000),
        ("micro-999999", 1_758_372_000, 999_999_000),
        // 其余 ⇒ 9 位
        ("nano-000000001", 1_758_372_000, 1),
        ("nano-000000010", 1_758_372_000, 10),
        ("nano-123456789", 1_758_372_000, 123_456_789),
        ("nano-001234567", 1_758_372_000, 1_234_567),
        // 负时间戳（epoch 之前）：验证 `-` 号不参与小数位数判断
        ("pre-epoch", -1, 0),
        ("pre-epoch-milli", -1, 500_000_000),
        // 年边界
        ("year-2000", 946_684_800, 0),
        ("year-2100", 4_102_444_800, 0),
    ];

    for (label, secs, nanos) in emit_cases {
        let dt = DateTime::from_timestamp(secs, nanos)
            .unwrap_or_else(|| panic!("{label}: ({secs}, {nanos}) 应当是合法时间"));
        emit.push(format!(
            "    {{ \"label\": \"{label}\", \"secs\": {secs}, \"nanos\": {nanos}, \
             \"text\": {} }}",
            quote(&dt.to_rfc3339())
        ));
    }

    // ── 二、chrono 收哪些串 ──────────────────────────────────────────────
    //
    // 读老库时，Go 侧必须**至少**吃下 chrono 写出来的那几种。
    // 这里把 chrono 的宽严边界钉下来：比它宽 ⇒ Go 会把坏数据当合法的读进来。
    let parse_inputs: Vec<&str> = vec![
        "2026-09-20T12:00:00Z",
        "2026-09-20T12:00:00+00:00",
        "2026-09-20T12:00:00.5Z",
        "2026-09-20T12:00:00.500Z",
        "2026-09-20T12:00:00.123456789Z",
        "2026-09-20T12:00:00.1234567890Z",
        "2026-09-20T12:00:00+08:00",
        "2026-09-20T12:00:00-05:30",
        "2026-09-20T12:00:00+0800",
        "2026-09-20 12:00:00Z",
        "2026-09-20T12:00:00",
        "2026-09-20t12:00:00z",
        "2026-09-20T12:00:00z",
        "20260920T120000Z",
        "2026-09-20T12:00:00.Z",
        "2026-09-20T24:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-09-20T12:00:60Z",
        "",
    ];

    for raw in parse_inputs {
        let parsed = DateTime::parse_from_rfc3339(raw).ok();
        let (ok, utc) = match parsed {
            Some(dt) => (
                "true",
                quote(&dt.with_timezone(&Utc).to_rfc3339()),
            ),
            None => ("false", "null".to_string()),
        };
        parse.push(format!(
            "    {{ \"input\": {}, \"ok\": {ok}, \"utc\": {utc} }}",
            quote(raw)
        ));
    }

    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(
        out,
        "  \"$comment\": \"由 chrono 0.4.45 的 to_rfc3339() / parse_from_rfc3339() 真跑产出\
         （生成器 = scripts/gen-datetime-format-vectors.rs）。⚠ 生成侧必须是 Rust：\
         要验证的正是 Go 侧写得对不对，用 Go 产出期望值等于自证。\
         $emit 钉的是**逐字节**的写法（混两种写法会让 SQL 的字符串比较与排序出错），\
         $parse 钉的是读老库时的宽严边界。\","
    );
    out.push_str("  \"$emit\": [\n");
    out.push_str(&emit.join(",\n"));
    out.push_str("\n  ],\n");
    out.push_str("  \"$parse\": [\n");
    out.push_str(&parse.join(",\n"));
    out.push_str("\n  ]\n}\n");

    print!("{out}");
}

/// 最小可用的 JSON 字符串转义（输入全是 ASCII 控制字符之外的可见字符）。
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
