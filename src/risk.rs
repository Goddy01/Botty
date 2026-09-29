use crate::types::PumpBuy;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct RiskConfig {
    pub max_sol_lamports: u64,
    pub cooldown_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RiskState {
    last_copy_ms: HashMap<[u8; 32], u64>,
    denylist: HashSet<[u8; 32]>,
}

impl RiskState {
    pub fn new(denylist: HashSet<[u8; 32]>) -> Self {
        Self {
            last_copy_ms: HashMap::new(),
            denylist,
        }
    }

    pub fn insert_denylist(&mut self, mint: [u8; 32]) {
        self.denylist.insert(mint);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskReject {
    OverMax,
    Cooldown,
    Denylist,
    ZeroSize,
}

/// Pure risk check. `copy_sol` is min(leader spend, cap).
#[inline]
pub fn check(
    state: &mut RiskState,
    cfg: &RiskConfig,
    buy: &PumpBuy,
    now_ms: u64,
) -> Result<u64, RiskReject> {
    if state.denylist.contains(&buy.mint) {
        return Err(RiskReject::Denylist);
    }
    if buy.max_sol_lamports == 0 {
        return Err(RiskReject::ZeroSize);
    }
    if let Some(prev) = state.last_copy_ms.get(&buy.wallet) {
        if now_ms.saturating_sub(*prev) < cfg.cooldown_ms {
            return Err(RiskReject::Cooldown);
        }
    }
    let copy_sol = buy.max_sol_lamports.min(cfg.max_sol_lamports);
    if copy_sol == 0 {
        return Err(RiskReject::OverMax);
    }
    state.last_copy_ms.insert(buy.wallet, now_ms);
    Ok(copy_sol)
}
