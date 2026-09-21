//! `test/golden/password-hash-vectors.json` 的生成器 —— **参考实现侧**的一次性脚本。
//!
//! # 为什么生成器是 Rust 而不是 Go
//!
//! 这份向量的用途是钉住「Go 侧算出的 Argon2id 与 Rust 侧**逐字节相同**」。
//! 用 Go 生成、再用 Go 验证，是**自证**：两边一起错时它照样绿。
//! 所以生成侧必须是**另一份实现**，且是迁移前一直在跑的那一份（RustCrypto argon2 0.5.3）。
//!
//! # 为什么不放进 crate 里
//!
//! Rust 侧已冻结（只读，见 `README.md`）。这是一次性数据变换，
//! 不是业务代码，所以放在 `scripts/` 且**不参与任何构建**。
//!
//! # 怎么重新生成（离线，不需要网络）
//!
//! ```bash
//! mkdir -p /tmp/mcloud-argon2-vector/src
//! cp scripts/gen-password-hash-vectors.rs /tmp/mcloud-argon2-vector/src/main.rs
//! cd /tmp/mcloud-argon2-vector
//! cat > Cargo.toml <<'TOML'
//! [package]
//! name = "mcloud-argon2-vector"
//! version = "0.0.0"
//! edition = "2021"
//! [dependencies]
//! argon2 = "0.5.3"
//! base64ct = "1.8.3"   # 只为扫盐长度边界时手工编码盐
//! TOML
//! CARGO_NET_OFFLINE=true cargo run --quiet > ../password-hash-vectors.json
//! ```
//!
//! ⚠ `argon2` 的版本要**钉死**：PHC 串里的 `v=19` 与参数顺序由它决定，
//! 换个大版本可能改默认参数 —— 那会让这份向量变成"上一次迁移的化石"。

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, Params};
use base64ct::Encoding as _;

/// 与 `mcloud-core` 的 `user.rs` 里那个测试用 `SeqRandom` **同构**：从 seed 起逐字节 +1。
///
/// 不直接用 `OsRng`：向量要能**复现**，盐必须由种子决定。
/// 这也顺带把「盐由 `Random` 注入」这条设计钉进了向量 —— 若 Go 侧改用库自带的随机源，
/// 这份向量立刻对不上。
struct SeqRandom(u8);

impl SeqRandom {
    fn fill(&mut self, dst: &mut [u8]) {
        for b in dst.iter_mut() {
            *b = self.0;
            self.0 = self.0.wrapping_add(1);
        }
    }
}

fn hash_of(plain: &str, seed: u8) -> String {
    let mut rng = SeqRandom(seed);
    let mut salt_bytes = [0u8; 16];
    rng.fill(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes).expect("16 字节盐必然可编码");
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .expect("默认参数必然可哈希")
        .to_string()
}

fn esc(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

fn main() {
    // 口令 / 盐种子的组合。刻意覆盖：最短合法长度、纯数字、长口令、含非 ASCII。
    let cases: &[(&str, u8)] = &[
        ("correct-horse", 1),
        ("correct-horse", 2),
        ("correct-horse", 7),
        ("12345678", 0),
        ("a-perfectly-ordinary-passphrase", 42),
        ("口令带中文也要能过", 9),
    ];

    println!("{{");
    println!(
        "  \"$comment\": \"由 RustCrypto argon2 0.5.3 产出（Argon2::default() = Argon2id, \
         m=19456 KiB, t=2, p=1, out=32）。生成器 = scripts/gen-password-hash-vectors.rs。\
         ⚠ 这是**跨语言对拍**用的：生成侧与消费侧必须是两份实现，否则是自证。\
         ⚠ `$verify` 的 expect 是 **Rust 侧**的事实；Go 侧对 `$crossLanguageGaps` 里\
         登记的条目行为不同（那边是人工确认，不是实测）。\","
    );
    println!(
        "  \"$params\": {{ \"algorithm\": \"argon2id\", \"version\": 19, \"mCost\": 19456, \
         \"tCost\": 2, \"pCost\": 1, \"outputLen\": 32, \"saltBytes\": 16, \
         \"saltEncoding\": \"base64 标准字母表、无填充（PHC B64）\" }},"
    );

    // 1) hash_password 的产出（盐由注入的 Random 决定）
    println!("  \"$hashPassword\": [");
    let mut first = true;
    for (plain, seed) in cases {
        if !first {
            println!(",");
        }
        first = false;
        print!(
            "    {{ \"plain\": \"{}\", \"seed\": {}, \"encoded\": \"{}\" }}",
            esc(plain),
            seed,
            esc(&hash_of(plain, *seed))
        );
    }
    println!();
    println!("  ],");

    // 2) verify_password 的**三态**：匹配 / 不匹配 / 哈希串本身非法
    //
    // 第三态是最容易被合并掉的一支：把「库里的哈希被写坏」表现成「用户密码错了」，
    // 会让故障永远查不出来。所以这里逐个钉。
    //
    // ⚠⚠ **这里的 `expect` 全部是"跑出来"的，不是"推出来"的。**
    // 下面每条都真的调一次 `verify_password`，实测结果与写下的 `expect` 不一致就 panic。
    //
    // 这个自检**已经救过一次**：`$argon2i$` 那条我先写的是 `ok`（理由："算法不同当然不匹配"
    // 的反面 —— 以为 RustCrypto 会忽略串里的算法、直接用 `Argon2::default()` 的 argon2id 去算，
    // 那就会算出与存储值相同的字节 ⇒ ok）。实测是 **mismatch**，即它**确实读了串里的算法**。
    // 推的话这条向量就把 Go 侧带沟里了。
    let base = hash_of("correct-horse", 1);
    let tweaked_m = base.replace("m=19456", "m=8");
    let tweaked_alg_i = base.replace("$argon2id$", "$argon2i$");
    let tweaked_alg_d = base.replace("$argon2id$", "$argon2d$");
    let tweaked_alg_unknown = base.replace("$argon2id$", "$argon2xyz$");
    let tweaked_ver = base.replace("$v=19$", "$v=16$");
    let bad_ver = base.replace("$v=19$", "$v=99$");
    let missing_p = base.replace(",p=1$", "$");
    let no_params = base.replace("$m=19456,t=2,p=1$", "$$");
    let extra_param = base.replace(",p=1$", ",p=1,x=1$");
    let empty_hash = base.replace("$878U+0v6GO2t1pLRqMja0KD8VQdkhMB93Wh2uOGAhmc", "$");

    // ⚠⚠ 成本参数为 0 / 过小 —— 这几条**不是**为了钉 Rust，是为了钉 **Go**。
    //
    // `golang.org/x/crypto/argon2` 的 `deriveKey` 头两行就是：
    //     if time < 1    { panic("argon2: number of rounds too small") }
    //     if threads < 1 { panic("argon2: parallelism degree too low") }
    // —— **panic，不是返回错误**。而库里的哈希串是**可能被写坏的**
    // （脏数据、迁移事故、有人手改过），在 HTTP 服务里 panic 就是一次请求打崩进程。
    // 所以 Go 侧的解析器必须自己挡住。这几条实测决定它"挡在哪、报什么"：
    // Rust 侧对同样输入返回的是**错误**还是"不匹配"，Go 侧必须一致，
    // 否则同一个串在两边的对外行为不同（一个 500、一个"用户名或口令不正确"）。
    let zero_t = base.replace("t=2", "t=0");
    let zero_p = base.replace("p=1", "p=0");
    let zero_m = base.replace("m=19456", "m=0");
    let tiny_m = base.replace("m=19456", "m=1");

    let verify_cases: Vec<(&str, &str, &str)> = vec![
        ("correct-horse", base.as_str(), "ok"),
        ("wrong-horse", base.as_str(), "mismatch"),
        // 语法就不是 PHC
        ("whatever", "not-a-phc-string", "error"),
        // ⚠ 这条是**实测**出来的：`$argon2id$v=19$m=19456,t=2,p=1$abc$def`
        // 看起来像合法的 PHC 串，但 `abc` 只解出 2 字节盐、低于下限，
        // 于是 `PasswordHash::new` 直接报 `SaltInvalid(TooShort)` —— **error，不是 mismatch**。
        // 单看字符串完全看不出来，所以必须留在向量里。
        ("whatever", "$argon2id$v=19$m=19456,t=2,p=1$abc$def", "error"),
        // m 从 19456 改成 8：用来钉住「参数是从串里读的」。
        // 若实现把参数写死成默认值，这一条会变成 ok。
        ("correct-horse", tweaked_m.as_str(), "mismatch"),
        // 算法被换成 argon2i / argon2d：**实测都是 mismatch**（它读了串里的算法）。
        ("correct-horse", tweaked_alg_i.as_str(), "mismatch"),
        ("correct-horse", tweaked_alg_d.as_str(), "mismatch"),
        // 算法名完全不存在：实测 **error**（`PasswordHash::new` 会拒掉不认识的算法标识符，
        // 而不是留到校验阶段才 mismatch）。错误是 `Algorithm`。
        ("correct-horse", tweaked_alg_unknown.as_str(), "error"),
        // 版本从 19 改成 16：实测 mismatch。
        ("correct-horse", tweaked_ver.as_str(), "mismatch"),
        // ⚠ 下面几条是**参数段解析**的分支。不测就会写成"猜"：
        //   版本号不认识 / 缺一个参数 / 整个参数段为空 / 多一个参数 / 哈希段为空。
        // 每一条都决定 Go 侧那个分支该报 error 还是 mismatch / ok。
        //
        // ⚠⚠ 其中"缺 `p=1`"那条**又一次推翻了预判**：我预判 error（"参数不全当然非法"），
        // 实测是 **ok** —— PHC 的参数是**可选、有默认值**的，而默认 p 恰好就是 1，
        // 于是算出来的字节与原串完全一致。这条若靠推，Go 侧就会多出一道 Rust 没有的校验，
        // 把一条本来能验过的存量哈希判成"格式非法"。
        ("correct-horse", bad_ver.as_str(), "error"),
        ("correct-horse", missing_p.as_str(), "ok"),
        ("correct-horse", no_params.as_str(), "error"),
        ("correct-horse", extra_param.as_str(), "error"),
        ("correct-horse", empty_hash.as_str(), "error"),
        // 成本参数边界 —— 预判先按"参数非法当然报错"写，跑一次由实测校正
        // （这个自检在本文件里已经推翻过四次预判了）。
        ("correct-horse", zero_t.as_str(), "error"),
        ("correct-horse", zero_p.as_str(), "error"),
        ("correct-horse", zero_m.as_str(), "error"),
        ("correct-horse", tiny_m.as_str(), "error"),
    ];

    // ⚠ 盐长度下限的**边界**：这一条只能扫，不能推。
    //
    // 我先按文档推成"`Salt::MIN_LENGTH` = 4 字节"，实测 **4 字节照样 `SaltInvalid(TooShort)`**
    // —— 推错了。所以这里把 3 / 4 / 6 / 8 / 12 / 16 字节逐个扫一遍，
    // 由实测结果决定 Go 侧该卡在哪。
    // 扫出来的边界会写进下面的 `$saltLengthBoundary` 段，Go 侧照着它断言。
    let mut salt_boundary: Vec<String> = Vec::new();
    for len in [3usize, 4, 6, 8, 12, 16] {
        let salt_bytes: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_add(1)).collect();
        let salt_b64 = base64ct::Base64Unpadded::encode_string(&salt_bytes);
        let encoded = format!(
            "$argon2id$v=19$m=19456,t=2,p=1${salt_b64}$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        let outcome = match PasswordHash::new(&encoded) {
            Err(e) => format!("error:{e:?}"),
            Ok(parsed) => match Argon2::default().verify_password(b"whatever", &parsed) {
                Ok(()) => "ok".to_string(),
                Err(argon2::password_hash::Error::Password) => "mismatch".to_string(),
                Err(e) => format!("error:{e:?}"),
            },
        };
        salt_boundary.push(format!(
            "    {{ \"saltBytes\": {len}, \"saltB64\": \"{salt_b64}\", \"outcome\": \"{}\" }}",
            esc(&outcome)
        ));
    }

    // ⚠ 成本参数的**边界**同样只能扫，不能推。
    //
    // `t=0` / `p=0` 实测是 error —— 而 Go 的 `argon2.IDKey` 对它们**panic**，
    // 所以 Go 侧的解析器必须自己挡（这是本文件最有价值的产出之一）。
    // 但"挡在哪"取决于下限的**确切位置**：`m=1` 实测 error、`m=8` 实测 mismatch，
    // 中间那几个值不扫出来就只能靠猜 —— 而猜错的方向恰恰是危险的：
    // **Go 放行了一个 Rust 会拒的串**（Go 会把过小的 memory 悄悄抬到下限再算，
    // 于是算出一个 Rust 根本不会算的值，表现为"不匹配"而不是"格式非法"）。
    let mut cost_boundary: Vec<String> = Vec::new();
    for (name, value) in [
        ("m", 2u32),
        ("m", 4),
        ("m", 7),
        ("m", 8),
        ("m", 16),
        ("t", 0),
        ("t", 1),
        ("t", 3),
        ("p", 0),
        ("p", 2),
    ] {
        let encoded = match name {
            "m" => base.replace("m=19456", &format!("m={value}")),
            "t" => base.replace("t=2", &format!("t={value}")),
            _ => base.replace("p=1", &format!("p={value}")),
        };
        let outcome = match PasswordHash::new(&encoded) {
            Err(e) => format!("error:{e:?}"),
            Ok(parsed) => match Argon2::default().verify_password(b"correct-horse", &parsed) {
                Ok(()) => "ok".to_string(),
                Err(argon2::password_hash::Error::Password) => "mismatch".to_string(),
                Err(e) => format!("error:{e:?}"),
            },
        };
        cost_boundary.push(format!(
            "    {{ \"param\": \"{name}\", \"value\": {value}, \"outcome\": \"{}\" }}",
            esc(&outcome)
        ));
    }

    // 逐条真跑，把实测结果与预判对上。
    //
    // ⚠ `error` 那一类还要**把错误文本带上**：`error` 这个分类太粗了 ——
    // "盐太短"与"算法名不认识"都是 error，但它们对应 Go 侧两条不同的校验，
    // 只记一个 `error` 等于没记。文本也一并进向量，Go 侧才能对到具体分支。
    let mut measured: Vec<String> = Vec::new();
    let mut mismatch_found = false;
    for (plain, encoded, expect) in &verify_cases {
        let (observed, detail) = match PasswordHash::new(encoded) {
            Err(e) => ("error", format!("{e:?}")),
            Ok(parsed) => match Argon2::default().verify_password(plain.as_bytes(), &parsed) {
                Ok(()) => ("ok", String::new()),
                Err(argon2::password_hash::Error::Password) => ("mismatch", String::new()),
                Err(e) => ("error", format!("{e:?}")),
            },
        };
        if observed != *expect {
            eprintln!("!! 预判与实测不一致：{plain:?} / {encoded:?} —— 预判 {expect}，实测 {observed}（{detail}）");
            mismatch_found = true;
        }
        measured.push(format!(
            "    {{ \"plain\": \"{}\", \"encoded\": \"{}\", \"expect\": \"{}\", \"rustError\": \"{}\" }}",
            esc(plain),
            esc(encoded),
            observed,
            esc(&detail)
        ));
    }
    assert!(!mismatch_found, "有预判与实测不一致的用例，先改预判再生成");

    println!("  \"$verify\": [");
    println!("{}", measured.join(",\n"));
    println!("  ],");

    // 盐长度下限的实测扫描结果。
    println!("  \"$saltLengthBoundary\": [");
    println!("{}", salt_boundary.join(",\n"));
    println!("  ],");

    // 成本参数下限的实测扫描结果。
    println!("  \"$costBoundary\": [");
    println!("{}", cost_boundary.join(",\n"));
    println!("  ],");

    // ⚠ 跨语言不等价登记 —— **这一段的 goExpect 是人工确认的，不是实测**。
    //
    // 它存在的理由：`$verify` 里的 expect 是 **Rust 侧**的事实，而 Go 侧有两处
    // 物理上做不到（不是"懒得做"）：
    //
    //   - `argon2d`：x/crypto 里 `argon2d` 是**私有 iota**，没有导出实现
    //     （`Key` 是 argon2i、`IDKey` 是 argon2id）；
    //   - `v=16`：`argon2.IDKey` 把版本**写死在 `initHash` 里**（`Version = 0x13`），
    //     没有任何参数可传。
    //
    // 把这两条**登记出来**而不是从 `$verify` 里删掉：删掉就没人知道
    // "Rust 那边是 mismatch、Go 这边是 error" 这个差异，下次有人拿同一个串在两端跑，
    // 会以为其中一边有 bug。差异本身是需要被记住的事实。
    //
    // ⚠ 尤其不能对 v=16 "顺手用 v=19 顶上"：那样 `$argon2id$v=16$…$<v19 生成的哈希>`
    // 会被算成**匹配** —— 等于"把版本字段改掉反而能过"。保守拒绝是唯一安全的解。
    let gaps = [
        format!(
            "    {{ \"encoded\": \"{}\", \"rustExpect\": \"mismatch\", \"goExpect\": \"error\", \
             \"reason\": \"golang.org/x/crypto/argon2 没有导出 argon2d 实现（argon2d 是私有 iota，Key=argon2i、IDKey=argon2id）\" }}",
            esc(&tweaked_alg_d)
        ),
        format!(
            "    {{ \"encoded\": \"{}\", \"rustExpect\": \"mismatch\", \"goExpect\": \"error\", \
             \"reason\": \"argon2.IDKey 把版本写死在 initHash（Version = 0x13），没有参数可传；用 v=19 顶替会让「把版本字段改成 16」反而算出匹配\" }}",
            esc(&tweaked_ver)
        ),
    ];
    println!("  \"$crossLanguageGaps\": [");
    println!("{}", gaps.join(",\n"));
    println!("  ]");
    println!("}}");

    // 把「我到底用了哪组参数」印到 stderr，别混进 JSON。
    let p = Params::DEFAULT;
    eprintln!(
        "Params::DEFAULT -> m={} t={} p={} out={:?}（None = 由实现取 32）",
        p.m_cost(),
        p.t_cost(),
        p.p_cost(),
        p.output_len()
    );

    // 自检：向量里的串必须能被自己这一侧解析回去，否则这份向量没有参考价值。
    for (plain, seed) in cases {
        let encoded = hash_of(plain, *seed);
        let parsed = PasswordHash::new(&encoded).expect("自产串必须可解析");
        assert!(
            Argon2::default()
                .verify_password(plain.as_bytes(), &parsed)
                .is_ok(),
            "自产串必须能验回去: {plain}"
        );
    }
    eprintln!("自检通过：{} 条自产串都能解析并验回。", cases.len());
}
