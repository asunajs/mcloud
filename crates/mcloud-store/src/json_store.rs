//! JSON 文件存储：账号、运行历史、账号状态。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use mcloud_core::{Account, AccountId, Error, Result, RunId, RunRecord, Store};

/// 运行历史保留条数。
const HISTORY_LIMIT: usize = 200;

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccountsFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: Vec<Account>,
}

/// 基于目录的 JSON 存储。
pub struct JsonStore {
    dir: PathBuf,
}

impl JsonStore {
    /// 打开（必要时创建）数据目录。
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// 原子写：先写临时文件再 rename，避免半截文件。
    ///
    /// 覆盖已有文件时保留一份 `.bak`。
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if path.exists() {
            let bak = path.with_extension("bak");
            let _ = std::fs::copy(path, bak);
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_json<T: for<'de> Deserialize<'de> + Default>(&self, name: &str) -> Result<T> {
        let p = self.path(name);
        if !p.exists() {
            return Ok(T::default());
        }
        let s = std::fs::read_to_string(&p)?;
        if s.trim().is_empty() {
            return Ok(T::default());
        }
        serde_json::from_str(&s).map_err(Error::from)
    }

    fn write_json<T: Serialize>(&self, name: &str, v: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(v)?;
        self.write_atomic(&self.path(name), &bytes)
    }
}

#[async_trait]
impl Store for JsonStore {
    async fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.read_json::<AccountsFile>("accounts.json")?.accounts)
    }

    async fn account(&self, id: &AccountId) -> Result<Option<Account>> {
        Ok(self.accounts().await?.into_iter().find(|a| &a.id == id))
    }

    async fn save_account(&self, account: &Account) -> Result<()> {
        let mut file = self.read_json::<AccountsFile>("accounts.json")?;
        file.version = 1;
        match file.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(slot) => *slot = account.clone(),
            None => file.accounts.push(account.clone()),
        }
        self.write_json("accounts.json", &file)
    }

    async fn remove_account(&self, id: &AccountId) -> Result<()> {
        let mut file = self.read_json::<AccountsFile>("accounts.json")?;
        file.accounts.retain(|a| &a.id != id);
        self.write_json("accounts.json", &file)
    }

    async fn append_run(&self, record: &RunRecord) -> Result<()> {
        let mut runs = self.read_json::<Vec<RunRecord>>("history.json")?;
        runs.push(record.clone());
        // 只保留最近 N 条，避免文件无限增长
        if runs.len() > HISTORY_LIMIT {
            let cut = runs.len() - HISTORY_LIMIT;
            runs.drain(..cut);
        }
        self.write_json("history.json", &runs)
    }

    async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>> {
        let mut runs = self.read_json::<Vec<RunRecord>>("history.json")?;
        // 倒序：最近的在前
        runs.reverse();
        runs.truncate(limit);
        Ok(runs)
    }

    async fn run(&self, id: RunId) -> Result<Option<RunRecord>> {
        Ok(self
            .read_json::<Vec<RunRecord>>("history.json")?
            .into_iter()
            .find(|r| r.run_id == id))
    }

    async fn account_state(&self, id: &AccountId) -> Result<serde_json::Value> {
        let all = self.read_json::<serde_json::Map<String, serde_json::Value>>("state.json")?;
        Ok(all
            .get(id.as_str())
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()> {
        let mut all = self.read_json::<serde_json::Map<String, serde_json::Value>>("state.json")?;
        all.insert(id.as_str().to_string(), state.clone());
        self.write_json("state.json", &all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{AccountSettings, AuthInfo, DeviceId, TaskOutcome};

    fn tmpdir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("mcloud-store-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn account(phone: &str) -> Account {
        Account {
            id: AccountId::new(phone),
            nickname: None,
            auth: AuthInfo {
                raw: "raw".into(),
                platform: "android".into(),
                phone: phone.into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("dev".into()),
            settings: AccountSettings::default(),
        }
    }

    #[tokio::test]
    async fn accounts_roundtrip_and_upsert() {
        let d = tmpdir("acct");
        let s = JsonStore::open(&d).unwrap();
        assert!(s.accounts().await.unwrap().is_empty());

        s.save_account(&account("13800000001")).await.unwrap();
        s.save_account(&account("13800000002")).await.unwrap();
        assert_eq!(s.accounts().await.unwrap().len(), 2);

        // 同 id 再存是更新而非追加
        let mut a = account("13800000001");
        a.nickname = Some("小明".into());
        s.save_account(&a).await.unwrap();
        let all = s.accounts().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            all.iter()
                .find(|x| x.id.as_str() == "13800000001")
                .unwrap()
                .nickname
                .as_deref(),
            Some("小明")
        );

        s.remove_account(&AccountId::new("13800000002"))
            .await
            .unwrap();
        assert_eq!(s.accounts().await.unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn runs_are_newest_first() {
        let d = tmpdir("runs");
        let s = JsonStore::open(&d).unwrap();
        for i in 1..=3u64 {
            s.append_run(&RunRecord {
                run_id: RunId::new(i),
                task: "sign".into(),
                account: "138".into(),
                started_at_ms: 0,
                duration_ms: 1,
                outcome: TaskOutcome::success("ok"),
                details: serde_json::Value::Null,
            })
            .await
            .unwrap();
        }
        let runs = s.runs(10).await.unwrap();
        assert_eq!(runs[0].run_id, RunId::new(3), "最近的应在最前");
        assert_eq!(runs.len(), 3);
        assert_eq!(s.run(RunId::new(2)).await.unwrap().unwrap().task, "sign");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn history_is_capped() {
        let d = tmpdir("cap");
        let s = JsonStore::open(&d).unwrap();
        for i in 1..=(HISTORY_LIMIT as u64 + 20) {
            s.append_run(&RunRecord {
                run_id: RunId::new(i),
                task: "sign".into(),
                account: "138".into(),
                started_at_ms: 0,
                duration_ms: 1,
                outcome: TaskOutcome::success("ok"),
                details: serde_json::Value::Null,
            })
            .await
            .unwrap();
        }
        assert_eq!(s.runs(usize::MAX).await.unwrap().len(), HISTORY_LIMIT);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn account_state_is_scoped_per_account() {
        let d = tmpdir("state");
        let s = JsonStore::open(&d).unwrap();
        let id = AccountId::new("13800000001");
        assert!(s.account_state(&id).await.unwrap().is_null());

        s.save_account_state(&id, &serde_json::json!({"deviceId":"abc"}))
            .await
            .unwrap();
        assert_eq!(
            s.account_state(&id).await.unwrap()["deviceId"],
            serde_json::json!("abc")
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
