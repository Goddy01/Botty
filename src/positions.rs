use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lot {
    pub amount: u64,
    #[serde(default)]
    pub entry_mcap: Option<f64>,
    #[serde(default)]
    pub bought_at_unix: u64,
    #[serde(default)]
    pub peak_mcap: Option<f64>,
    #[serde(default)]
    pub task_idx: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UserPositions {
    pub telegram_user_id: i64,
    /// task_idx -> copied mint base58
    #[serde(default)]
    pub copied_mints: HashMap<usize, HashSet<String>>,
    /// mint base58 -> token amount still held from copies
    #[serde(default)]
    pub holdings: HashMap<String, u64>,
    #[serde(default)]
    pub lots: HashMap<String, Lot>,
}

pub struct PositionStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl PositionStore {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self {
            dir,
            lock: Mutex::new(()),
        })
    }

    pub fn get(&self, user_id: i64) -> UserPositions {
        let path = self.path(user_id);
        if !path.exists() {
            return UserPositions {
                telegram_user_id: user_id,
                ..Default::default()
            };
        }
        fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(UserPositions {
                telegram_user_id: user_id,
                ..Default::default()
            })
    }

    pub fn already_copied(&self, user_id: i64, task_idx: usize, mint: &str) -> bool {
        self.get(user_id)
            .copied_mints
            .get(&task_idx)
            .is_some_and(|s| s.contains(mint))
    }

    pub fn holding(&self, user_id: i64, mint: &str) -> u64 {
        let p = self.get(user_id);
        p.lots
            .get(mint)
            .map(|l| l.amount)
            .or_else(|| p.holdings.get(mint).copied())
            .unwrap_or(0)
    }

    pub fn lot(&self, user_id: i64, mint: &str) -> Option<Lot> {
        let p = self.get(user_id);
        if let Some(l) = p.lots.get(mint) {
            return Some(l.clone());
        }
        let amount = *p.holdings.get(mint)?;
        if amount == 0 {
            return None;
        }
        Some(Lot {
            amount,
            entry_mcap: None,
            bought_at_unix: 0,
            peak_mcap: None,
            task_idx: 0,
        })
    }

    pub fn record_buy(
        &self,
        user_id: i64,
        task_idx: usize,
        mint: &str,
        amount: u64,
        mcap: Option<f64>,
    ) -> Result<()> {
        let _g = self.lock.lock().expect("positions");
        let mut p = self.get(user_id);
        p.telegram_user_id = user_id;
        p.copied_mints
            .entry(task_idx)
            .or_default()
            .insert(mint.to_string());
        *p.holdings.entry(mint.to_string()).or_default() =
            p.holdings.get(mint).copied().unwrap_or(0).saturating_add(amount);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let lot = p.lots.entry(mint.to_string()).or_insert(Lot {
            amount: 0,
            entry_mcap: mcap,
            bought_at_unix: now,
            peak_mcap: mcap,
            task_idx,
        });
        lot.amount = lot.amount.saturating_add(amount);
        lot.task_idx = task_idx;
        if lot.entry_mcap.is_none() {
            lot.entry_mcap = mcap;
        }
        if let Some(m) = mcap {
            lot.peak_mcap = Some(lot.peak_mcap.unwrap_or(0.0).max(m));
        }
        self.write(&p)
    }

    pub fn bump_peak(&self, user_id: i64, mint: &str, mcap: f64) -> Result<()> {
        let _g = self.lock.lock().expect("positions");
        let mut p = self.get(user_id);
        if let Some(lot) = p.lots.get_mut(mint) {
            lot.peak_mcap = Some(lot.peak_mcap.unwrap_or(0.0).max(mcap));
            self.write(&p)?;
        }
        Ok(())
    }

    pub fn record_sell(&self, user_id: i64, mint: &str) -> Result<()> {
        self.record_sell_amount(user_id, mint, u64::MAX)
    }

    pub fn record_sell_amount(&self, user_id: i64, mint: &str, sold: u64) -> Result<()> {
        let _g = self.lock.lock().expect("positions");
        let mut p = self.get(user_id);
        p.telegram_user_id = user_id;
        let have = p
            .lots
            .get(mint)
            .map(|l| l.amount)
            .or_else(|| p.holdings.get(mint).copied())
            .unwrap_or(0);
        let left = have.saturating_sub(sold);
        if left == 0 {
            p.holdings.remove(mint);
            p.lots.remove(mint);
        } else {
            p.holdings.insert(mint.to_string(), left);
            if let Some(lot) = p.lots.get_mut(mint) {
                lot.amount = left;
            }
        }
        self.write(&p)
    }

    fn write(&self, p: &UserPositions) -> Result<()> {
        let path = self.path(p.telegram_user_id);
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(p).context("serialize positions")?;
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }

    fn path(&self, user_id: i64) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn buy_once_and_sell_clears() {
        let dir = std::env::temp_dir().join(format!(
            "copybot-pos-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = PositionStore::open(dir).unwrap();
        assert!(!store.already_copied(1, 0, "mint"));
        store.record_buy(1, 0, "mint", 10, None).unwrap();
        assert!(store.already_copied(1, 0, "mint"));
        assert_eq!(store.holding(1, "mint"), 10);
        store.record_sell(1, "mint").unwrap();
        assert_eq!(store.holding(1, "mint"), 0);
        assert!(store.already_copied(1, 0, "mint"));
    }
}
