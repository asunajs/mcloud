// 存储层单元测试的公共 helper；主题用例见同目录其它文件。
use super::*;

use mcloud_core::UserStore;

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("内存库应可打开")
}

/// 测试用户。
///
/// v3 起账号必须有主人（`accounts.owner` 有外键指向 `users`，隔离也靠它过滤），
/// 所以碰账号的用例都先要有一个用户 —— 这正是生产的顺序（先 `user init` 再导入）。
const TEST_USER: &str = "test-user";

fn uid() -> UserId {
    UserId::new(TEST_USER)
}

/// 建好测试用户的库。
async fn store_with_user() -> SqliteStore {
    let s = store();
    s.upsert_user(&mcloud_core::User::new(
        TEST_USER,
        TEST_USER,
        "!",
        mcloud_core::Role::Admin,
        Utc::now(),
    ))
    .await
    .expect("建测试用户");
    s
}

fn account(id: &str) -> Account {
    Account {
        nickname: Some("账号2".into()),
        phone_masked: Some("199****0001".into()),
        // 统一归给测试用户；不设的话写入会被隔离校验拒掉
        owner: Some(uid()),
        ..Account::new(id)
    }
}

/// 建一个已归属的账号行。
///
/// 写云朵快照 / 运行记录之前必须有这一行：隔离校验靠 `accounts.owner`，
/// 账号不存在时那些写入会被正确地拒掉（这正是它该有的行为）。
async fn put_account(s: &SqliteStore, id: &str) {
    s.upsert_account(Scope::user(&uid()), &account(id))
        .await
        .unwrap();
}

// ── smid 两列 ───────────────────────────────────────────────────────────
//
// 这一组的分量比看上去重：`SmidState` 与 `AccountState` 的写入契约**相反**
// （前者窄写、后者整体覆盖），而这个相反只写在注释和 SQL 里，编译器看不见。
// 下面几条就是它唯一的执行力来源 —— 尤其 `upsert_account_state_leaves_the_smid_columns_alone`，
// schema.rs 的 v5 注释直接点名了它。

/// 探针用的设备特征文本。存储层**从不解析**它（`SmidState` 的"这一层只搬文本"），
/// 所以随手给一段任意 JSON 就是合法的 —— 这本身也是那条契约的断言之一。
const FP_V1: &str = r#"{"ua":"UA-v1","canvas":"aaaa1111"}"#;
const FP_V2: &str = r#"{"ua":"UA-v2","canvas":"bbbb2222"}"#;

/// 合成 deviceId（`199` 段，见脱敏约定）。
const DEVICE_ID_1: &str = "B1990000000000001";

/// 合成整串 `Authorization`（`199` 段手机号，见脱敏约定）。
///
/// 形状与真实值一致：`Basic base64("<前缀>:<手机号>:<token>")`。
/// 这一列存的是**原文**，所以测试里也照原文比 —— 不做任何解析。
const AUTH_V1: &str = "Basic MjoxOTkwMDAwMDAwMTphdXRoLXYx";

/// 另一个合成整串，专用来验证"回填不覆盖已有值"。
const AUTH_V2: &str = "Basic MjoxOTkwMDAwMDAwMjphdXRoLXYy";

mod accounts;
mod cloud_stats;
mod credentials;
mod isolation;
mod reopen;
mod runs;
mod smid;
mod state;
