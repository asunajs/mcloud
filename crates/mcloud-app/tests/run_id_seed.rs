//! 回归：**重启之后 run id 不能从 1 重来**。
//!
//! # 这条测试为什么必须存在
//!
//! 2026-10-01 用户报「为啥历史只有两条」。根因是两半拼起来的：
//!
//! 1. `Runner::new` 的编号从 **1** 开始，而 `Runner::seed_run_id`（注释写着
//!    "从历史记录续上 run id，避免重启后重复"）**全仓没有任何调用点**；
//! 2. 运行记录是按编号 **upsert** 的（`RunStore::upsert_run`：同编号就覆盖）。
//!
//! 于是每次重启之后新跑的 1、2、3… 会把**上次启动**留下的同编号记录一条条盖掉 ——
//! 历史看起来"永远只有最近这几条"，而存储限额、历史页、前端全都无辜。
//!
//! 只测 `seed_run_id` 这个函数是**不够的**（它本来就没坏，坏的是没人调它）。
//! 所以这里走完整的启动路径：写一条旧记录 → 起应用（= 重启）→ `seed_run_ids` →
//! 下一次运行的编号必须接着往后排。

use std::sync::Arc;

use mcloud_app::AppBuilder;
// `upsert_run` 是 `RunStore` 上的方法：不引入 trait 就只有"没有这个方法"的报错。
use mcloud_core::{Account, AccountId, AuthInfo, DeviceId, RunStore as _};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("mcloud-seed-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn account() -> Account {
    Account {
        id: AccountId::new("19****01"),
        nickname: None,
        auth: AuthInfo {
            raw: "Basic eA==".into(),
            platform: "pc".into(),
            phone: "19900000001".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("DEV-1".into()),
        pc_platform: None,
        user_domain_id: None,
        owner: None,
        enabled: true,
        settings: Default::default(),
    }
}

#[tokio::test]
async fn seed_run_ids_continues_after_the_largest_existing_run() {
    let dir = temp_dir("continue");

    // 上一轮"启动"留下的历史：编号 1 与 7（不是连续的 —— 续的应该是**最大值**）
    {
        let store = mcloud_store::JsonStore::open(&dir).expect("开存储");
        for (id, task) in [(1u64, "sign"), (7, "receive")] {
            let rec: mcloud_core::RunRecord = serde_json::from_value(serde_json::json!({
                "run_id": id,
                "task": task,
                "account": "19****01",
                "started_at_ms": 1_790_000_000_000i64,
                "duration_ms": 10,
                "outcome": { "status": "success", "summary": "旧记录" },
                "status": "done",
                "details": null,
            }))
            .expect("旧记录要能构造");
            store.upsert_run(&rec).await.expect("写旧记录");
        }
    }

    // 重启：新的应用实例指向同一个数据目录
    let app = AppBuilder::new()
        .data_dir(dir.clone())
        .build()
        .expect("应用应能组装");
    let handle = Arc::new(app.handle);

    // ⚠ 这一句就是当年漏掉的那一句
    handle.seed_run_ids().await.expect("续编号");

    let id = handle
        .runner
        .start("sign", account(), serde_json::json!({}));
    assert!(
        id.0 > 7,
        "重启后编号必须接在历史最大值之后（否则新记录会覆盖旧的），实际 {}",
        id.0
    );

    let _ = std::fs::remove_dir_all(&dir);
}
