// 测试公共 helper：由 legacy_import.rs 的 `mod tests` 头部拆出，各主题用例文件用 `use super::*;` 取用。

use super::*;
use crate::store::SqliteStore;
use chrono::{DateTime, Utc};
use mcloud_core::AccountId;
use mcloud_core::{Role, Scope, User, UserId, UserStore};
use std::path::PathBuf;

/// 真实脱敏夹具所在目录（`test/fixtures/legacy/`）。
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test/fixtures/legacy");

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("内存库应可打开")
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-17T00:00:00Z")
        .expect("固定时刻")
        .with_timezone(&Utc)
}

/// 测试用的导入者。
///
/// 账号从 v3 起有归属，而 `accounts.owner` 有外键指向 `users` ——
/// 也就是说**导入之前必须真的有一个用户**。这正是生产路径的要求
/// （先 `user init` 再 `migrate`），测试里同样照办，不给自己开免检通道。
const TEST_USER: &str = "test-user";

fn uid() -> UserId {
    UserId::new(TEST_USER)
}

async fn ensure_user(store: &SqliteStore) {
    store
        .upsert_user(&User::new(TEST_USER, TEST_USER, "!", Role::Admin, now()))
        .await
        .expect("建测试用户");
}

/// 导入的统一入口：建好首个用户，并把旧数据导到它名下。
///
/// 这里刻意走 `Scope::User` 而不是 `Scope::Local`：两者在导入器里的差别只是
/// "账号归谁"，而这些用例关心的正是归属正确 —— 所以用 User 范围覆盖它。
/// `Scope::Local`（账号保持无主）由 `diagnose_legacy` 的预演用例覆盖。
async fn run_import(store: &SqliteStore, source: &LegacySource) -> Result<MigrationReport> {
    ensure_user(store).await;
    import_legacy(store, Scope::user(&uid()), source, now()).await
}

/// 夹具目录本身就是合法的旧项目布局：`asign.json` 与数据文件同目录。
fn fixture_source() -> LegacySource {
    LegacySource::new(
        PathBuf::from(FIXTURES).join("asign.json"),
        PathBuf::from(FIXTURES),
    )
}

/// 把夹具整份复制到临时目录，供"改坏一个文件"的场景使用。
fn copy_fixtures() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("临时目录");
    for name in std::iter::once("asign.json").chain(LEGACY_DATA_FILES) {
        let text = std::fs::read_to_string(PathBuf::from(FIXTURES).join(name))
            .unwrap_or_else(|e| panic!("读夹具 {name} 失败: {e}"));
        std::fs::write(dir.path().join(name), text).expect("写夹具");
    }
    dir
}

fn source_of(dir: &tempfile::TempDir) -> LegacySource {
    LegacySource::new(dir.path().join("asign.json"), dir.path())
}

fn write_file(dir: &tempfile::TempDir, name: &str, body: &str) {
    std::fs::write(dir.path().join(name), body).expect("写文件");
}

fn remove_file(dir: &tempfile::TempDir, name: &str) {
    std::fs::remove_file(dir.path().join(name)).expect("删文件");
}

/// 夹具里两个账号的掩码 AccountId（前 2 + `****` + 后 2）。
const ACC_1: &str = "19****01"; // 手机号 19900000001，昵称「账号2」
const ACC_2: &str = "19****02"; // 手机号 19900000002，昵称「账号1」

fn id(raw: &str) -> AccountId {
    AccountId::new(raw)
}

mod backfill;
mod entities;
mod idempotency;
mod normalize;
mod references;
