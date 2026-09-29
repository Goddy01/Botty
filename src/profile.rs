use crate::config::{encode_pubkey, parse_pubkey};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::watch;

pub const MAX_WATCHED: usize = 90;

#[derive(Clone, Copy, Debug)]
pub struct ProfileDefaults {
    pub max_sol: f64,
    pub cooldown_ms: u64,
}

impl ProfileDefaults {
    pub fn from_config(max_sol_lamports: u64, cooldown_ms: u64) -> Self {
        Self {
            max_sol: max_sol_lamports as f64 / 1_000_000_000.0,
            cooldown_ms,
        }
    }
}

fn default_zero() -> f64 {
    0.0
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub enum BuyMode {
    Exact,
    #[default]
    Fixed,
}

impl BuyMode {
    pub fn label(self) -> &'static str {
        match self {
            BuyMode::Exact => "Buy Exact",
            BuyMode::Fixed => "Buy Fixed",
        }
    }
}

impl<'de> Deserialize<'de> for BuyMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "Exact" => BuyMode::Exact,
            _ => BuyMode::Fixed,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum QuoteCurrency {
    #[default]
    Sol,
    Usdt,
}

impl QuoteCurrency {
    pub fn label(self) -> &'static str {
        match self {
            QuoteCurrency::Sol => "SOL",
            QuoteCurrency::Usdt => "USDT",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopySetup {
    #[serde(default)]
    pub sell_on_transfer: bool,
    #[serde(default)]
    pub min_trigger_buy: Option<f64>,
    #[serde(default)]
    pub min_trigger_quote: QuoteCurrency,
    #[serde(default)]
    pub max_trigger_buy: Option<f64>,
    #[serde(default)]
    pub max_trigger_quote: QuoteCurrency,
    #[serde(default)]
    pub min_mcap: Option<f64>,
    #[serde(default)]
    pub max_mcap: Option<f64>,
    #[serde(default)]
    pub min_token_age_s: Option<f64>,
    #[serde(default)]
    pub max_token_age_s: Option<f64>,
    #[serde(default)]
    pub skip_deploys: bool,
    #[serde(default)]
    pub follow_sells: bool,
    #[serde(default)]
    pub buy_once: bool,
    #[serde(default)]
    pub buy_mode: BuyMode,
    #[serde(default = "default_zero")]
    pub buy_amount: f64,
    #[serde(default = "default_zero")]
    pub buy_fee_sol: f64,
    #[serde(default = "default_zero")]
    pub sell_fee_sol: f64,
    #[serde(default = "default_zero")]
    pub buy_slippage_pct: f64,
    #[serde(default = "default_zero")]
    pub sell_slippage_pct: f64,
    #[serde(default)]
    pub buy_protection: bool,
    #[serde(default)]
    pub sell_protection: bool,
    #[serde(default)]
    pub auto_tip: bool,
    #[serde(default = "default_zero")]
    pub buy_tip_sol: f64,
    #[serde(default = "default_zero")]
    pub sell_tip_sol: f64,
    #[serde(default)]
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum StrategyMode {
    DevSell,
    Time,
    Trailing,
    #[default]
    ProfitLoss,
    FixedMcap,
    McapVariation,
}

impl StrategyMode {
    pub fn code(self) -> &'static str {
        match self {
            StrategyMode::DevSell => "ds",
            StrategyMode::Time => "tm",
            StrategyMode::Trailing => "tr",
            StrategyMode::ProfitLoss => "pl",
            StrategyMode::FixedMcap => "fm",
            StrategyMode::McapVariation => "mv",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ds" => StrategyMode::DevSell,
            "tm" => StrategyMode::Time,
            "tr" => StrategyMode::Trailing,
            "pl" => StrategyMode::ProfitLoss,
            "fm" => StrategyMode::FixedMcap,
            "mv" => StrategyMode::McapVariation,
            _ => return None,
        })
    }

    pub fn btn_label(self) -> &'static str {
        match self {
            StrategyMode::DevSell => "DevSell",
            StrategyMode::Time => "Time",
            StrategyMode::Trailing => "Trailing",
            StrategyMode::ProfitLoss => "Profit/Loss",
            StrategyMode::FixedMcap => "Fixed Mcap",
            StrategyMode::McapVariation => "Mcap Variation",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitOrder {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub mode: StrategyMode,
    #[serde(default)]
    pub picked: bool,
    #[serde(default = "default_zero")]
    pub sell_amount_pct: f64,
    #[serde(default = "default_zero")]
    pub trigger_pct: f64,
    #[serde(default = "default_zero")]
    pub sell_at_pct: f64,
    #[serde(default = "default_zero")]
    pub time_secs: f64,
    #[serde(default = "default_zero")]
    pub mcap_usd: f64,
    #[serde(default = "default_zero")]
    pub tip_sol: f64,
    #[serde(default = "default_zero")]
    pub slippage_pct: f64,
    #[serde(default = "default_zero")]
    pub fee_sol: f64,
    #[serde(default)]
    pub sell_protection: bool,
    #[serde(default)]
    pub high_impact: bool,
}

impl Default for LimitOrder {
    fn default() -> Self {
        Self {
            id: 0,
            mode: StrategyMode::ProfitLoss,
            picked: false,
            sell_amount_pct: 0.0,
            trigger_pct: 0.0,
            sell_at_pct: 0.0,
            time_secs: 0.0,
            mcap_usd: 0.0,
            tip_sol: 0.0,
            slippage_pct: 0.0,
            fee_sol: 0.0,
            sell_protection: false,
            high_impact: false,
        }
    }
}

impl LimitOrder {
    pub fn list_title(&self, n: usize) -> String {
        let kind = if self.trigger_pct < 0.0 || (self.picked && self.mode == StrategyMode::ProfitLoss && self.sell_at_pct < 0.0)
        {
            "Stop Loss"
        } else if self.mode == StrategyMode::DevSell {
            "DevSell"
        } else if self.mode == StrategyMode::Time {
            "Time"
        } else if self.mode == StrategyMode::Trailing {
            "Trailing"
        } else if self.mode == StrategyMode::FixedMcap {
            "Fixed Mcap"
        } else if self.mode == StrategyMode::McapVariation {
            "Mcap Variation"
        } else {
            "Take Profit"
        };
        format!("{kind} (#{n})")
    }

    pub fn pnl_trigger(&self) -> f64 {
        if self.trigger_pct != 0.0 {
            self.trigger_pct
        } else {
            self.sell_at_pct
        }
    }
}

impl Default for CopySetup {
    fn default() -> Self {
        Self {
            sell_on_transfer: false,
            min_trigger_buy: None,
            min_trigger_quote: QuoteCurrency::Sol,
            max_trigger_buy: None,
            max_trigger_quote: QuoteCurrency::Sol,
            min_mcap: None,
            max_mcap: None,
            min_token_age_s: None,
            max_token_age_s: None,
            skip_deploys: false,
            follow_sells: false,
            buy_once: false,
            buy_mode: BuyMode::Fixed,
            buy_amount: 0.0,
            buy_fee_sol: 0.0,
            sell_fee_sol: 0.0,
            buy_slippage_pct: 0.0,
            sell_slippage_pct: 0.0,
            buy_protection: false,
            sell_protection: false,
            auto_tip: false,
            buy_tip_sol: 0.0,
            sell_tip_sol: 0.0,
            active: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopyTask {
    pub address: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub setup: CopySetup,
    #[serde(default)]
    pub orders: Vec<LimitOrder>,
}

impl CopyTask {
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            short_wallet_label(&self.address)
        } else {
            self.name.clone()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserProfile {
    pub telegram_user_id: i64,
    #[serde(default)]
    pub chat_id: i64,
    #[serde(default)]
    pub watched: Vec<String>,
    /// Optional display names for copy setups, keyed by base58 address.
    #[serde(default)]
    pub watch_names: HashMap<String, String>,
    #[serde(default)]
    pub tasks: Vec<CopyTask>,
    #[serde(default = "default_max_sol")]
    pub max_sol: f64,
    #[serde(default = "default_cooldown_ms")]
    pub cooldown_ms: u64,
    #[serde(default = "default_copy_on")]
    pub copy_enabled: bool,
    #[serde(default)]
    pub degen_mode: bool,
    #[serde(default)]
    pub expert_mode: bool,
    #[serde(default)]
    pub private_pnl: bool,
    #[serde(default)]
    pub positions_usd: bool,
    #[serde(default)]
    pub settings_buy_protection: bool,
    #[serde(default = "default_buy_presets")]
    pub buy_presets: [f64; 5],
    #[serde(default = "default_sell_presets")]
    pub sell_presets: [f64; 5],
    #[serde(default = "default_trade_tip")]
    pub trade_tip_sol: f64,
    #[serde(default = "default_trade_slip")]
    pub trade_slippage_pct: f64,
    #[serde(default)]
    pub trade_auto_tip: bool,
    /// Bitmask of wallets used for CA buys/sells/splits (bit 0 = W1).
    #[serde(default = "default_trade_wallets")]
    pub trade_wallets: u8,
}

fn default_max_sol() -> f64 {
    0.05
}

fn default_cooldown_ms() -> u64 {
    2_000
}

fn default_copy_on() -> bool {
    true
}

pub fn default_buy_presets() -> [f64; 5] {
    [1.8, 4.0, 5.05, 3.5, 10.0]
}

pub fn default_sell_presets() -> [f64; 5] {
    [20.0, 40.0, 70.0, 50.0, 100.0]
}

fn default_trade_tip() -> f64 {
    0.001
}

fn default_trade_slip() -> f64 {
    10.0
}

fn default_trade_wallets() -> u8 {
    1
}

impl UserProfile {
    fn new(user_id: i64, chat_id: i64, defaults: ProfileDefaults) -> Self {
        Self {
            telegram_user_id: user_id,
            chat_id,
            watched: Vec::new(),
            watch_names: HashMap::new(),
            tasks: Vec::new(),
            max_sol: defaults.max_sol,
            cooldown_ms: defaults.cooldown_ms,
            copy_enabled: true,
            degen_mode: false,
            expert_mode: false,
            private_pnl: false,
            positions_usd: false,
            settings_buy_protection: false,
            buy_presets: default_buy_presets(),
            sell_presets: default_sell_presets(),
            trade_tip_sol: default_trade_tip(),
            trade_slippage_pct: default_trade_slip(),
            trade_auto_tip: false,
            trade_wallets: default_trade_wallets(),
        }
    }

    pub fn hydrate(&mut self) {
        if self.tasks.is_empty() && !self.watched.is_empty() {
            self.tasks = self
                .watched
                .iter()
                .map(|a| CopyTask {
                    address: a.clone(),
                    name: self
                        .watch_names
                        .get(a)
                        .cloned()
                        .unwrap_or_default(),
                    setup: CopySetup::default(),
                    orders: Vec::new(),
                })
                .collect();
        }
        for t in &mut self.tasks {
            t.setup.sell_slippage_pct = t.setup.sell_slippage_pct.clamp(0.0, 100.0);
        }
        if self.trade_wallets == 0 {
            self.trade_wallets = default_trade_wallets();
        }
        self.sync_lists();
    }

    pub fn trade_wallet_indices(&self, n_wallets: usize) -> Vec<usize> {
        let n = n_wallets.min(8);
        let mut v: Vec<usize> = (0..n)
            .filter(|i| self.trade_wallets & (1u8 << i) != 0)
            .collect();
        if v.is_empty() && n > 0 {
            v.push(0);
        }
        v
    }

    pub fn toggle_trade_wallet(&mut self, i: usize, n_wallets: usize) {
        let n = n_wallets.min(8);
        if i >= n {
            return;
        }
        let bit = 1u8 << i;
        if self.trade_wallets & bit != 0 {
            let next = self.trade_wallets & !bit;
            if (0..n).any(|j| next & (1u8 << j) != 0) {
                self.trade_wallets = next;
            }
        } else {
            self.trade_wallets |= bit;
        }
    }

    fn sync_lists(&mut self) {
        self.watched = self.tasks.iter().map(|t| t.address.clone()).collect();
        self.watch_names.clear();
        for t in &self.tasks {
            if !t.name.is_empty() {
                self.watch_names.insert(t.address.clone(), t.name.clone());
            }
        }
    }

    pub fn max_sol_lamports(&self) -> u64 {
        (self.max_sol * 1_000_000_000.0).round() as u64
    }

    pub fn watch_label(&self, addr: &str) -> String {
        if let Some(t) = self.tasks.iter().rev().find(|t| t.address == addr) {
            return t.display_name();
        }
        self.watch_names
            .get(addr)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| short_wallet_label(addr))
    }
}

pub fn short_wallet_label(addr: &str) -> String {
    let mut chars = addr.chars();
    let head: String = chars.by_ref().take(8).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

pub fn sanitize_watch_name(name: &str) -> String {
    let trimmed: String = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(16).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[derive(Clone, Debug)]
pub struct Follower {
    pub user_id: i64,
    pub chat_id: i64,
    pub max_sol_lamports: u64,
    pub copy_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct FollowJob {
    pub user_id: i64,
    pub chat_id: i64,
    pub task_idx: usize,
    pub task: CopyTask,
    pub max_sol_lamports: u64,
    pub cooldown_ms: u64,
    pub wallet_idx: Option<usize>,
}

pub struct WatchBook {
    dir: PathBuf,
    defaults: ProfileDefaults,
    lock: Mutex<BookInner>,
    wallets_tx: watch::Sender<HashSet<[u8; 32]>>,
    _keep_rx: watch::Receiver<HashSet<[u8; 32]>>,
}

struct BookInner {
    by_user: HashMap<i64, UserProfile>,
}

impl WatchBook {
    pub fn open(dir: impl AsRef<Path>, defaults: ProfileDefaults) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let mut by_user = HashMap::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let mut p: UserProfile = serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", path.display()))?;
            p.hydrate();
            by_user.insert(p.telegram_user_id, p);
        }
        let union = union_keys(&by_user);
        let (wallets_tx, keep_rx) = watch::channel(union);
        Ok(Self {
            dir,
            defaults,
            lock: Mutex::new(BookInner { by_user }),
            wallets_tx,
            _keep_rx: keep_rx,
        })
    }

    pub fn subscribe(&self) -> watch::Receiver<HashSet<[u8; 32]>> {
        self.wallets_tx.subscribe()
    }

    pub fn union(&self) -> HashSet<[u8; 32]> {
        let g = self.lock.lock().expect("profiles");
        union_keys(&g.by_user)
    }

    pub fn get(&self, user_id: i64) -> Option<UserProfile> {
        let mut p = self
            .lock
            .lock()
            .expect("profiles")
            .by_user
            .get(&user_id)
            .cloned()?;
        p.hydrate();
        Some(p)
    }

    pub fn all_profiles(&self) -> Vec<UserProfile> {
        let g = self.lock.lock().expect("profiles");
        g.by_user
            .values()
            .cloned()
            .map(|mut p| {
                p.hydrate();
                p
            })
            .collect()
    }

    pub fn ensure(&self, user_id: i64, chat_id: i64) -> Result<UserProfile> {
        let mut g = self.lock.lock().expect("profiles");
        if let Some(p) = g.by_user.get_mut(&user_id) {
            p.hydrate();
            if chat_id != 0 && p.chat_id != chat_id {
                p.chat_id = chat_id;
                let snap = p.clone();
                drop(g);
                self.write(&snap)?;
                return Ok(snap);
            }
            return Ok(p.clone());
        }
        let p = UserProfile::new(user_id, chat_id, self.defaults);
        g.by_user.insert(user_id, p.clone());
        drop(g);
        self.write(&p)?;
        Ok(p)
    }

    pub fn add_watch(&self, user_id: i64, chat_id: i64, raw: &str) -> Result<UserProfile> {
        self.add_watch_named(user_id, chat_id, raw, None)
    }

    pub fn add_watch_named(
        &self,
        user_id: i64,
        chat_id: i64,
        raw: &str,
        name: Option<&str>,
    ) -> Result<UserProfile> {
        let key = parse_pubkey(raw).context("wallet address")?;
        let addr = encode_pubkey(&key);
        let label = name
            .map(sanitize_watch_name)
            .filter(|n| !n.is_empty());
        let mut g = self.lock.lock().expect("profiles");
        let p = g
            .by_user
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id, chat_id, self.defaults));
        p.hydrate();
        if chat_id != 0 {
            p.chat_id = chat_id;
        }
        if let Some(n) = &label {
            if let Some(t) = p.tasks.iter_mut().find(|t| t.address == addr) {
                t.name = n.clone();
                p.sync_lists();
                let snap = p.clone();
                drop(g);
                self.write(&snap)?;
                return Ok(snap);
            }
        } else if p.tasks.iter().any(|t| t.address == addr) {
            let snap = p.clone();
            drop(g);
            self.write(&snap)?;
            return Ok(snap);
        }
        anyhow::ensure!(
            p.tasks.len() < MAX_WATCHED,
            "watch list full ({MAX_WATCHED} wallets). /unwatch one first"
        );
        p.tasks.push(CopyTask {
            address: addr,
            name: label.unwrap_or_default(),
            setup: CopySetup::default(),
            orders: Vec::new(),
        });
        p.sync_lists();
        let snap = p.clone();
        let union = union_keys(&g.by_user);
        drop(g);
        self.write(&snap)?;
        let _ = self.wallets_tx.send(union);
        Ok(snap)
    }

    /// Adds unique valid wallets. Returns (profile, newly_added, already_present).
    pub fn add_watches(
        &self,
        user_id: i64,
        chat_id: i64,
        raws: &[&str],
    ) -> Result<(UserProfile, usize, usize)> {
        let mut addrs = Vec::new();
        for raw in raws {
            let key = parse_pubkey(raw).context("wallet address")?;
            addrs.push(encode_pubkey(&key));
        }
        let mut g = self.lock.lock().expect("profiles");
        let p = g
            .by_user
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id, chat_id, self.defaults));
        p.hydrate();
        if chat_id != 0 {
            p.chat_id = chat_id;
        }
        let mut added = 0usize;
        let mut already = 0usize;
        for addr in addrs {
            if p.tasks.iter().any(|t| t.address == addr) {
                already += 1;
                continue;
            }
            anyhow::ensure!(
                p.tasks.len() < MAX_WATCHED,
                "watch list full ({MAX_WATCHED} wallets). /unwatch one first"
            );
            p.tasks.push(CopyTask {
                address: addr,
                name: String::new(),
                setup: CopySetup::default(),
                orders: Vec::new(),
            });
            added += 1;
        }
        p.sync_lists();
        let snap = p.clone();
        let union = union_keys(&g.by_user);
        drop(g);
        self.write(&snap)?;
        let _ = self.wallets_tx.send(union);
        Ok((snap, added, already))
    }

    /// `Ok(None)` = no profile. `Ok(Some((profile, removed)))`.
    pub fn remove_watch(&self, user_id: i64, raw: &str) -> Result<Option<(UserProfile, bool)>> {
        let key = parse_pubkey(raw).context("wallet address")?;
        let addr = encode_pubkey(&key);
        let mut g = self.lock.lock().expect("profiles");
        let Some(p) = g.by_user.get_mut(&user_id) else {
            return Ok(None);
        };
        p.hydrate();
        let before = p.tasks.len();
        p.tasks.retain(|t| t.address != addr);
        p.sync_lists();
        let removed = p.tasks.len() != before;
        let snap = p.clone();
        if !removed {
            return Ok(Some((snap, false)));
        }
        let union = union_keys(&g.by_user);
        drop(g);
        self.write(&snap)?;
        let _ = self.wallets_tx.send(union);
        Ok(Some((snap, true)))
    }

    pub fn set_max_sol(&self, user_id: i64, chat_id: i64, max_sol: f64) -> Result<UserProfile> {
        anyhow::ensure!(max_sol > 0.0 && max_sol <= 10.0, "max sol must be between 0 and 10");
        self.update(user_id, chat_id, |p| p.max_sol = max_sol)
    }

    pub fn set_copy_enabled(&self, user_id: i64, chat_id: i64, on: bool) -> Result<UserProfile> {
        self.update(user_id, chat_id, |p| p.copy_enabled = on)
    }

    pub fn patch_profile(
        &self,
        user_id: i64,
        chat_id: i64,
        f: impl FnOnce(&mut UserProfile),
    ) -> Result<UserProfile> {
        self.update(user_id, chat_id, f)
    }

    pub fn clear_watches(&self, user_id: i64, chat_id: i64) -> Result<UserProfile> {
        let snap = self.update(user_id, chat_id, |p| {
            p.hydrate();
            p.tasks.clear();
            p.sync_lists();
        })?;
        let g = self.lock.lock().expect("profiles");
        let union = union_keys(&g.by_user);
        drop(g);
        let _ = self.wallets_tx.send(union);
        Ok(snap)
    }

    pub fn followers_of(&self, wallet: &str) -> Vec<Follower> {
        let g = self.lock.lock().expect("profiles");
        g.by_user
            .values()
            .filter(|p| {
                p.copy_enabled
                    && p.tasks.iter().any(|t| t.address == wallet && t.setup.active)
            })
            .map(|p| Follower {
                user_id: p.telegram_user_id,
                chat_id: p.chat_id,
                max_sol_lamports: p.max_sol_lamports(),
                copy_enabled: p.copy_enabled,
            })
            .collect()
    }

    pub fn followers_matching(&self, wallet: &str) -> Vec<FollowJob> {
        let g = self.lock.lock().expect("profiles");
        let mut out = Vec::new();
        for p in g.by_user.values() {
            if !p.copy_enabled {
                continue;
            }
            for (task_idx, task) in p.tasks.iter().enumerate() {
                if task.address == wallet && task.setup.active {
                    out.push(FollowJob {
                        user_id: p.telegram_user_id,
                        chat_id: p.chat_id,
                        task_idx,
                        task: task.clone(),
                        max_sol_lamports: p.max_sol_lamports(),
                        cooldown_ms: p.cooldown_ms,
                        wallet_idx: None,
                    });
                }
            }
        }
        out
    }

    pub fn name_taken(&self, user_id: i64, name: &str, except_idx: Option<usize>) -> bool {
        let Some(p) = self.get(user_id) else {
            return false;
        };
        p.tasks.iter().enumerate().any(|(i, t)| {
            Some(i) != except_idx && t.display_name().eq_ignore_ascii_case(name)
        })
    }

    pub fn reload_from_disk(&self, user_id: i64) -> Result<Option<UserProfile>> {
        let path = self.dir.join(format!("{user_id}.json"));
        if !path.exists() {
            return Ok(self.get(user_id));
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut p: UserProfile =
            serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        p.hydrate();
        let mut g = self.lock.lock().expect("profiles");
        g.by_user.insert(user_id, p.clone());
        Ok(Some(p))
    }

    pub fn update_task(
        &self,
        user_id: i64,
        chat_id: i64,
        idx: usize,
        f: impl FnOnce(&mut CopyTask),
    ) -> Result<UserProfile> {
        let mut g = self.lock.lock().expect("profiles");
        let p = g
            .by_user
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id, chat_id, self.defaults));
        p.hydrate();
        if chat_id != 0 {
            p.chat_id = chat_id;
        }
        let task = p
            .tasks
            .get_mut(idx)
            .ok_or_else(|| anyhow::anyhow!("setup gone"))?;
        f(task);
        p.sync_lists();
        let snap = p.clone();
        let union = union_keys(&g.by_user);
        drop(g);
        self.write(&snap)?;
        let _ = self.wallets_tx.send(union);
        Ok(snap)
    }

    pub fn delete_task(&self, user_id: i64, chat_id: i64, idx: usize) -> Result<UserProfile> {
        let mut g = self.lock.lock().expect("profiles");
        let p = g
            .by_user
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id, chat_id, self.defaults));
        p.hydrate();
        if chat_id != 0 {
            p.chat_id = chat_id;
        }
        anyhow::ensure!(idx < p.tasks.len(), "setup gone");
        p.tasks.remove(idx);
        p.sync_lists();
        let snap = p.clone();
        let union = union_keys(&g.by_user);
        drop(g);
        self.write(&snap)?;
        let _ = self.wallets_tx.send(union);
        Ok(snap)
    }

    pub fn duplicate_task_as(
        &self,
        user_id: i64,
        chat_id: i64,
        idx: usize,
        new_addr: &str,
        new_name: &str,
    ) -> Result<(UserProfile, usize)> {
        let key = parse_pubkey(new_addr).context("wallet address")?;
        let addr = encode_pubkey(&key);
        let name = sanitize_watch_name(new_name);
        anyhow::ensure!(!name.is_empty(), "need a unique config name");
        anyhow::ensure!(
            !self.name_taken(user_id, &name, None),
            "that name is already used"
        );
        let mut g = self.lock.lock().expect("profiles");
        let p = g
            .by_user
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id, chat_id, self.defaults));
        p.hydrate();
        if chat_id != 0 {
            p.chat_id = chat_id;
        }
        anyhow::ensure!(idx < p.tasks.len(), "setup gone");
        anyhow::ensure!(
            p.tasks.len() < MAX_WATCHED,
            "watch list full ({MAX_WATCHED} wallets)"
        );
        let mut copy = p.tasks[idx].clone();
        copy.address = addr;
        copy.name = name;
        let insert_at = idx + 1;
        p.tasks.insert(insert_at, copy);
        p.sync_lists();
        let snap = p.clone();
        let union = union_keys(&g.by_user);
        drop(g);
        self.write(&snap)?;
        let _ = self.wallets_tx.send(union);
        Ok((snap, insert_at))
    }

    fn update(
        &self,
        user_id: i64,
        chat_id: i64,
        f: impl FnOnce(&mut UserProfile),
    ) -> Result<UserProfile> {
        let mut g = self.lock.lock().expect("profiles");
        let p = g
            .by_user
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id, chat_id, self.defaults));
        if chat_id != 0 {
            p.chat_id = chat_id;
        }
        f(p);
        p.hydrate();
        let snap = p.clone();
        drop(g);
        self.write(&snap)?;
        Ok(snap)
    }

    fn write(&self, profile: &UserProfile) -> Result<()> {
        let path = self.dir.join(format!("{}.json", profile.telegram_user_id));
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(profile).context("serialize profile")?;
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }
}

fn union_keys(by_user: &HashMap<i64, UserProfile>) -> HashSet<[u8; 32]> {
    let mut out = HashSet::new();
    for p in by_user.values() {
        for w in &p.watched {
            if let Ok(k) = parse_pubkey(w) {
                out.insert(k);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_book() -> WatchBook {
        let dir = std::env::temp_dir().join(format!(
            "copybot-profiles-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        WatchBook::open(
            dir,
            ProfileDefaults {
                max_sol: 0.05,
                cooldown_ms: 2000,
            },
        )
        .unwrap()
    }

    #[test]
    fn personal_watch_lists_do_not_leak() {
        let book = tmp_book();
        let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
        let b = "2CQgjcdNEo7WtbQLpJTAVcC3Ga61pNvRDTgP5grzctFG";
        book.add_watch(1, 10, a).unwrap();
        book.update_task(1, 10, 0, |t| t.setup.active = true).unwrap();
        book.add_watch(2, 20, b).unwrap();
        book.update_task(2, 20, 0, |t| t.setup.active = true).unwrap();
        assert_eq!(book.get(1).unwrap().watched, vec![a.to_string()]);
        assert_eq!(book.get(2).unwrap().watched, vec![b.to_string()]);
        assert_eq!(book.followers_of(a).len(), 1);
        assert_eq!(book.followers_of(a)[0].user_id, 1);
        assert!(book.followers_of(a).iter().all(|f| f.chat_id == 10));
        assert_eq!(book.union().len(), 2);
        let removed = book.remove_watch(1, a).unwrap();
        assert!(removed.unwrap().1);
        assert!(book.get(1).unwrap().watched.is_empty());
        assert_eq!(book.union().len(), 1);
    }

    #[test]
    fn reload_from_disk() {
        let dir = std::env::temp_dir().join(format!(
            "copybot-profiles-reload-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let defaults = ProfileDefaults {
            max_sol: 0.05,
            cooldown_ms: 2000,
        };
        let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
        {
            let book = WatchBook::open(&dir, defaults).unwrap();
            book.add_watch(7, 70, a).unwrap();
            book.set_max_sol(7, 70, 0.2).unwrap();
        }
        let book = WatchBook::open(&dir, defaults).unwrap();
        let p = book.get(7).unwrap();
        assert_eq!(p.watched, vec![a.to_string()]);
        assert!((p.max_sol - 0.2).abs() < 1e-9);
        assert_eq!(p.chat_id, 70);
    }

    #[test]
    fn named_watch_persists() {
        let book = tmp_book();
        let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
        book.add_watch_named(1, 10, a, Some("Aaliya")).unwrap();
        let p = book.get(1).unwrap();
        assert_eq!(p.watch_label(a), "Aaliya");
        book.remove_watch(1, a).unwrap();
        assert!(book.get(1).unwrap().watch_names.is_empty());
    }

    #[test]
    fn duplicate_requires_new_wallet_and_unique_name() {
        let book = tmp_book();
        let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
        let b = "2CQgjcdNEo7WtbQLpJTAVcC3Ga61pNvRDTgP5grzctFG";
        book.add_watch_named(1, 10, a, Some("Aaliya")).unwrap();
        book.update_task(1, 10, 0, |t| {
            t.setup.buy_amount = 1.25;
            t.setup.buy_mode = BuyMode::Exact;
        })
        .unwrap();
        let err = book.duplicate_task_as(1, 10, 0, b, "Aaliya").unwrap_err();
        assert!(err.to_string().contains("already used"));
        let (_, new_idx) = book.duplicate_task_as(1, 10, 0, b, "Boston").unwrap();
        assert_eq!(new_idx, 1);
        let p = book.get(1).unwrap();
        assert_eq!(p.tasks.len(), 2);
        assert_eq!(p.tasks[1].address, b);
        assert_eq!(p.tasks[1].name, "Boston");
        assert!((p.tasks[1].setup.buy_amount - 1.25).abs() < 1e-9);
        assert_eq!(p.tasks[1].setup.buy_mode, BuyMode::Exact);
    }

    #[test]
    fn trade_wallet_mask_keeps_one_on() {
        let mut p = UserProfile::new(
            1,
            1,
            ProfileDefaults {
                max_sol: 0.05,
                cooldown_ms: 2000,
            },
        );
        assert_eq!(p.trade_wallet_indices(2), vec![0]);
        p.toggle_trade_wallet(1, 2);
        assert_eq!(p.trade_wallet_indices(2), vec![0, 1]);
        p.toggle_trade_wallet(0, 2);
        assert_eq!(p.trade_wallet_indices(2), vec![1]);
        p.toggle_trade_wallet(1, 2);
        assert_eq!(p.trade_wallet_indices(2), vec![1]);
    }

    #[test]
    fn new_copy_setup_starts_zeroed() {
        let s = CopySetup::default();
        assert!(!s.active);
        assert!(!s.follow_sells);
        assert!(!s.buy_once);
        assert!(!s.sell_protection);
        assert_eq!(s.buy_amount, 0.0);
        assert_eq!(s.buy_fee_sol, 0.0);
        assert_eq!(s.sell_fee_sol, 0.0);
        assert_eq!(s.buy_slippage_pct, 0.0);
        assert_eq!(s.sell_slippage_pct, 0.0);
        assert_eq!(s.buy_tip_sol, 0.0);
        assert_eq!(s.sell_tip_sol, 0.0);
        assert_eq!(s.min_trigger_buy, None);
        assert_eq!(s.max_mcap, None);
    }

    #[test]
    fn percent_json_becomes_fixed() {
        let mode: BuyMode = serde_json::from_str("\"Percent\"").unwrap();
        assert_eq!(mode, BuyMode::Fixed);
        let mode: BuyMode = serde_json::from_str("\"Exact\"").unwrap();
        assert_eq!(mode, BuyMode::Exact);
    }
}
