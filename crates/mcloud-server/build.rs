//! 让 `cargo` **知道 `web/dist` 变了**。
//!
//! # 为什么必须有这个文件
//!
//! `rust-embed` 在**编译期**把 `web/dist` 嵌进二进制，而 `cargo` 只跟踪
//! `build.rs` 声明过的输入 —— 它**天然不知道** `dist` 内容变了。
//!
//! ⇒ 先 `npm run build` 再 `cargo build` 时，若这一层没声明，
//! `cargo` 会认为"源文件没动"而直接复用**上一次编出来的二进制**：
//! 它打印 `Finished`、退出码 0、**一句警告都没有**，而嵌进去的还是旧前端。
//!
//! 表现是"改了前端、构建全绿、刷新页面却毫无变化"。
//! 这个坑在本仓**实际踩过两次**（都表现为"修了但没效果"），
//! 排查方向会被引到"是不是又没重启"上 —— 那正是它最贵的地方。
//!
//! # 为什么盯着**目录**而不是文件
//!
//! Vite 产物带内容哈希（`assets/index-<hash>.js`），每次构建**文件名都变**。
//! 盯着某一个具体文件等于没盯。盯目录能捕捉到增删改名。
/// 跑一条命令，成功就返回 trim 后的 stdout，否则空串。
///
/// 拿不到（没装 git、不在仓库里、非 utf8）一律给空串：**构建永远不能因此失败** ——
/// 「关于」卡片少一行，远比"编不出来"轻。
fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() {
    // ⚠ 路径相对的是**本 crate 的 Cargo.toml 所在目录**（`crates/mcloud-server/`）。
    println!("cargo::rerun-if-changed=../../web/dist");

    // ── 「关于」卡片要的版本信息（2026-10-01，用户口径："还有就是下方的更新说明"）──
    //
    // ⚠ 写**文件**而不是 `cargo::rustc-env`：提交日志是多行的，而 `cargo:` 指令
    // 一行一条 —— 塞进环境变量会把指令格式弄坏（表现是构建期莫名报错）。
    let out = std::env::var("OUT_DIR").expect("OUT_DIR 一定有");
    let write = |name: &str, body: &str| {
        let _ = std::fs::write(std::path::Path::new(&out).join(name), body);
    };
    write(
        "build-commit.txt",
        &run("git", &["rev-parse", "--short", "HEAD"]),
    );
    write(
        "build-dirty.txt",
        if run("git", &["status", "--porcelain"]).is_empty() {
            "clean"
        } else {
            "dirty"
        },
    );
    // 本地时间（用户那边是北京时间）：`date` 是 POSIX 的，两平台都有。
    write("build-time.txt", &run("date", &["+%Y-%m-%d %H:%M %Z"]));
    write(
        "build-log.txt",
        &run("git", &["log", "--pretty=format:%h %s", "-12"]),
    );

    // 提交之后要重新跑：`.git/logs/HEAD`（reflog）在每次提交/切换时都会动。
    println!("cargo::rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo::rerun-if-changed=../../.git/HEAD");
}
