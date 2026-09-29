use super::{API, BOT_NAME, BOT_USERNAME, MENU};
use crate::config::AppConfig;
use crate::engine::CopyEngine;
use crate::live::{LiveBoard, LiveSnap};
use crate::positions::PositionStore;
use crate::pnl::{self, CalView, PnlStore};
use crate::profile::{
    BuyMode, LimitOrder, ProfileDefaults, QuoteCurrency, StrategyMode, WatchBook, MAX_WATCHED,
};
use crate::rpc_read::{fmt_sol, is_placeholder_payer, RpcRead, WSOL_MINT};
use crate::user_wallet::{UserWallet, WalletStore};
use super::parse::*;
use super::status::StatusCard;
use super::ui::*;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Hub {
    pub(crate) token: String,
    pub(crate) http: reqwest::Client,
    pub(crate) allow: HashSet<i64>,
    pub(crate) allow_names: HashSet<String>,
    pub(crate) chats: Arc<Mutex<HashSet<i64>>>,
    pub(crate) status: StatusCard,
    pub(crate) rpc: RpcRead,
    pub(crate) live: Arc<LiveBoard>,
    pub(crate) wallets: Arc<WalletStore>,
    pub(crate) profiles: Arc<WatchBook>,
    pub(crate) positions: Arc<PositionStore>,
    pub(crate) pnl: Arc<PnlStore>,
    pub(crate) cal: Arc<Mutex<HashMap<i64, CalView>>>,
    pub(crate) awaiting: Arc<Mutex<HashMap<i64, AwaitKind>>>,
    pub(crate) await_prompt: Arc<Mutex<HashMap<i64, i64>>>,
    pub(crate) last_bot_msg: Arc<Mutex<HashMap<i64, i64>>>,
    pub(crate) strat_drafts: Arc<Mutex<HashMap<i64, (usize, LimitOrder)>>>,
    pub(crate) trade: Arc<Mutex<HashMap<i64, TradeView>>>,
    pub(crate) withdraw_pct: Arc<Mutex<HashMap<i64, f64>>>,
    pub(crate) split_pct: Arc<Mutex<HashMap<i64, f64>>>,
    pub(crate) engine: Arc<Mutex<Option<std::sync::Weak<CopyEngine>>>>,
    pub(crate) tasks_wallet_back: Arc<Mutex<HashMap<i64, Option<usize>>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct TradeView {
    pub(crate) mint: String,
    pub(crate) sell: bool,
    pub(crate) note: String,
}

impl Hub {
    pub fn from_config(cfg: &AppConfig, live: Arc<LiveBoard>) -> Result<Option<Self>> {
        let Some(token) = cfg.telegram_bot_token.clone() else {
            return Ok(None);
        };
        anyhow::ensure!(
            !cfg.telegram_allow_user_ids.is_empty() || !cfg.telegram_allow_usernames.is_empty(),
            "TELEGRAM_BOT_TOKEN set but allowlist is empty — add @usernames or numeric ids"
        );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
            .build()
            .context("telegram http")?;
        Ok(Some(Self {
            rpc: RpcRead::new(http.clone(), cfg.rpc_url.clone()),
            token,
            http,
            allow: cfg.telegram_allow_user_ids.clone(),
            allow_names: cfg.telegram_allow_usernames.clone(),
            chats: Arc::new(Mutex::new(cfg.telegram_chat_ids.clone())),
            status: StatusCard::from_config(cfg),
            live,
            wallets: Arc::new(WalletStore::open("data/user_wallets")?),
            profiles: Arc::new(WatchBook::open(
                "data/user_profiles",
                ProfileDefaults::from_config(cfg.max_sol_lamports, cfg.cooldown_ms),
            )?),
            positions: Arc::new(PositionStore::open("data/user_positions")?),
            pnl: Arc::new(PnlStore::open("data/user_pnl")?),
            cal: Arc::new(Mutex::new(HashMap::new())),
            awaiting: Arc::new(Mutex::new(HashMap::new())),
            await_prompt: Arc::new(Mutex::new(HashMap::new())),
            last_bot_msg: Arc::new(Mutex::new(HashMap::new())),
            strat_drafts: Arc::new(Mutex::new(HashMap::new())),
            trade: Arc::new(Mutex::new(HashMap::new())),
            withdraw_pct: Arc::new(Mutex::new(HashMap::new())),
            split_pct: Arc::new(Mutex::new(HashMap::new())),
            engine: Arc::new(Mutex::new(None)),
            tasks_wallet_back: Arc::new(Mutex::new(HashMap::new())),
        }))
    }

    pub fn watch_book(&self) -> Arc<WatchBook> {
        self.profiles.clone()
    }

    pub fn wallet_store(&self) -> Arc<WalletStore> {
        self.wallets.clone()
    }

    pub fn pnl_store(&self) -> Arc<PnlStore> {
        self.pnl.clone()
    }

    pub fn bind_engine(&self, engine: &Arc<CopyEngine>) {
        *self.engine.lock().expect("engine") = Some(Arc::downgrade(engine));
    }

    pub async fn notify_user(&self, chat_id: i64, text: &str) {
        if chat_id == 0 {
            return;
        }
        if let Err(e) = self.send(chat_id, text).await {
            eprintln!("telegram send chat={chat_id}: {e:#}");
        }
    }

    pub fn allowed(&self, user_id: i64, username: Option<&str>) -> bool {
        if self.allow.contains(&user_id) {
            return true;
        }
        username
            .map(crate::config::normalize_username)
            .is_some_and(|n| !n.is_empty() && self.allow_names.contains(&n))
    }

    pub fn subscribe(&self, chat_id: i64) {
        self.chats.lock().expect("chats").insert(chat_id);
    }

    pub fn unsubscribe(&self, chat_id: i64) -> bool {
        self.chats.lock().expect("chats").remove(&chat_id)
    }

    pub(crate) fn set_awaiting(&self, user_id: i64, kind: Option<AwaitKind>) {
        let mut g = self.awaiting.lock().expect("awaiting");
        match kind {
            Some(k) => {
                g.insert(user_id, k);
            }
            None => {
                g.remove(&user_id);
            }
        }
    }

    pub(crate) fn remember_prompt(&self, user_id: i64, message_id: i64) {
        if message_id != 0 {
            self.await_prompt
                .lock()
                .expect("prompt")
                .insert(user_id, message_id);
            self.remember_bot_msg(user_id, message_id);
        }
    }

    pub(crate) fn forget_prompt(&self, user_id: i64) {
        self.await_prompt.lock().expect("prompt").remove(&user_id);
    }

    pub(crate) fn remember_bot_msg(&self, user_id: i64, message_id: i64) {
        if message_id != 0 {
            self.last_bot_msg
                .lock()
                .expect("last_bot")
                .insert(user_id, message_id);
        }
    }

    pub(crate) fn last_bot_msg(&self, user_id: i64) -> Option<i64> {
        self.last_bot_msg.lock().expect("last_bot").get(&user_id).copied()
    }

    pub(crate) fn forget_bot_msg(&self, user_id: i64, message_id: i64) {
        let mut g = self.last_bot_msg.lock().expect("last_bot");
        if g.get(&user_id).copied() == Some(message_id) {
            g.remove(&user_id);
        }
    }

    pub(crate) fn prompt_message(&self, user_id: i64) -> Option<i64> {
        self.await_prompt.lock().expect("prompt").get(&user_id).copied()
    }

    pub(crate) fn awaiting_kind(&self, user_id: i64) -> Option<AwaitKind> {
        self.awaiting.lock().expect("awaiting").get(&user_id).cloned()
    }

    pub async fn dispatch_text(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Option<Reply> {
        if let Some(cmd) = parse_command(text) {
            self.set_awaiting(user_id, None);
            let args = command_args(text);
            return Some(
                self.reply_for_args(user_id, username, chat_id, cmd, &args)
                    .await,
            );
        }
        match self.awaiting_kind(user_id) {
            Some(AwaitKind::Add) => {
                Some(self.finish_add_config(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::AddName(addr)) => {
                Some(self.finish_add_config_name(user_id, username, chat_id, &addr, text).await)
            }
            Some(AwaitKind::SetupName(idx)) => {
                Some(self.finish_setup_name(user_id, username, chat_id, idx, text).await)
            }
            Some(AwaitKind::SetupTarget(idx)) => {
                Some(self.finish_setup_target(user_id, username, chat_id, idx, text).await)
            }
            Some(AwaitKind::SetupValue(idx, field)) => {
                Some(self.finish_setup_value(user_id, username, chat_id, idx, field, text).await)
            }
            Some(AwaitKind::DupTarget(idx)) => {
                Some(self.finish_dup_target(user_id, username, chat_id, idx, text).await)
            }
            Some(AwaitKind::DupName(idx, addr)) => {
                Some(self.finish_dup_name(user_id, username, chat_id, idx, &addr, text).await)
            }
            Some(AwaitKind::Mass) => {
                Some(self.finish_mass_create(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::StratVal(idx, field)) => {
                Some(self.finish_strat_value(user_id, username, chat_id, idx, field, text).await)
            }
            Some(AwaitKind::BuyPreset(i)) => {
                Some(self.finish_buy_preset(user_id, username, chat_id, i, text).await)
            }
            Some(AwaitKind::SellPreset(i)) => {
                Some(self.finish_sell_preset(user_id, username, chat_id, i, text).await)
            }
            Some(AwaitKind::TradeBuy) => {
                Some(self.finish_trade_buy(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::TradeSell) => {
                Some(self.finish_trade_sell(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::TradeTip) => {
                Some(self.finish_trade_tip(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::TradeSlip) => {
                Some(self.finish_trade_slip(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::WalImport) => {
                Some(self.finish_wallet_import(user_id, username, chat_id, text).await)
            }
            Some(AwaitKind::WithdrawPct) => {
                Some(self.finish_withdraw_pct(user_id, username, text).await)
            }
            Some(AwaitKind::WithdrawTo) => {
                Some(self.finish_withdraw_to(user_id, username, text).await)
            }
            Some(AwaitKind::SplitPct) => {
                Some(self.finish_split_pct(user_id, username, text).await)
            }
            Some(AwaitKind::SplitTo) => {
                Some(self.finish_split_to(user_id, username, text).await)
            }
            None => {
                if !self.allowed(user_id, username) {
                    return Some(Self::deny_reply(user_id));
                }
                if let Some(mint) = extract_mint(text) {
                    return Some(self.open_trade_for_mint(user_id, chat_id, mint).await);
                }
                let guess = text.trim();
                if guess.len() >= 32 && guess.len() <= 50 {
                    return Some(Reply::text(
                        "Could not parse that as a Solana mint. Paste the contract address by itself, with no extra text.",
                    ));
                }
                None
            }
        }
    }

    pub(crate) async fn finish_add_config(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let addr = text.split_whitespace().next().unwrap_or("");
        if crate::config::parse_pubkey(addr).is_err() {
            return Reply::text(format!(
                "That is not a Solana wallet address.\n\n{}",
                add_config_prompt()
            ));
        }
        let added = self.cmd_watch_named(user_id, chat_id, addr, None).await;
        if added.starts_with("Could not add wallet") {
            self.set_awaiting(user_id, None);
            return Reply::text(added);
        }
        self.set_awaiting(user_id, Some(AwaitKind::AddName(addr.into())));
        Reply::page(
            add_config_name_prompt(addr),
            serde_json::json!({
                "inline_keyboard": [
                    [btn("← Back", "p:copy"), btn("🗑 Close", "x:close")],
                ]
            }),
        )
    }

    pub(crate) async fn finish_add_config_name(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        addr: &str,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let raw = text.trim();
        if crate::config::parse_pubkey(raw.split_whitespace().next().unwrap_or("")).is_ok() {
            return Reply::page(
                format!("{}\n\n{}", add_config_name_retry(), add_config_name_prompt(addr)),
                serde_json::json!({
                    "inline_keyboard": [
                        [btn("← Back", "p:copy"), btn("🗑 Close", "x:close")],
                    ]
                }),
            );
        }
        let name = crate::profile::sanitize_watch_name(raw);
        if name.is_empty() {
            return Reply::page(
                format!("{}\n\n{}", add_config_name_retry(), add_config_name_prompt(addr)),
                serde_json::json!({
                    "inline_keyboard": [
                        [btn("← Back", "p:copy"), btn("🗑 Close", "x:close")],
                    ]
                }),
            );
        }
        self.set_awaiting(user_id, None);
        let labeled = self
            .cmd_watch_named(user_id, chat_id, addr, Some(&name))
            .await;
        if labeled.starts_with("Could not add wallet") {
            return Reply::text(labeled);
        }
        self.copy_page_reply(user_id, None).await
    }

    pub(crate) async fn finish_mass_create(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return Reply::text(format!(
                "Paste at least one Solana wallet address.\n\n{}",
                mass_create_prompt()
            ));
        }
        if lines.len() > MAX_WATCHED {
            return Reply::text(format!(
                "Too many addresses ({}/{}). Send at most {MAX_WATCHED}.\n\n{}",
                lines.len(),
                MAX_WATCHED,
                mass_create_prompt()
            ));
        }
        let mut ok = Vec::new();
        let mut bad = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if crate::config::parse_pubkey(line).is_ok() {
                ok.push(*line);
            } else {
                bad.push(i + 1);
            }
        }
        if !bad.is_empty() {
            return Reply::text(format!(
                "Line(s) {} are not Solana wallet addresses.\n\n{}",
                bad.iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                mass_create_prompt()
            ));
        }
        match self.profiles.add_watches(user_id, chat_id, &ok) {
            Ok((p, added, already)) => {
                self.set_awaiting(user_id, None);
                let note = format!(
                    "✅ Mass create: added {added}, already on list {already}. Your list: {} wallet(s).",
                    p.watched.len()
                );
                self.copy_page_reply(user_id, Some(&note)).await
            }
            Err(e) => Reply::text(format!(
                "Could not add wallets: {e:#}\n\n{}",
                mass_create_prompt()
            )),
        }
    }

    pub(crate) async fn apply_copy_bulk(
        &self,
        user_id: i64,
        chat_id: i64,
        action: CallbackAction,
    ) -> Reply {
        let note = match action {
            CallbackAction::CopyPauseAll => {
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| {
                    p.copy_enabled = false;
                    for t in &mut p.tasks {
                        t.setup.active = false;
                    }
                });
                Some("⏹ All setups paused.")
            }
            CallbackAction::CopyStartAll => {
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| {
                    p.copy_enabled = true;
                    for t in &mut p.tasks {
                        t.setup.active = true;
                    }
                });
                Some("▶️ All setups started.")
            }
            CallbackAction::CopyDeleteAll => {
                let _ = self.profiles.clear_watches(user_id, chat_id);
                Some("🗑 All setups deleted.")
            }
            _ => None,
        };
        self.copy_page_reply(user_id, note).await
    }

    pub(crate) fn copy_kb_for(&self, user_id: i64) -> serde_json::Value {
        let p = self.profiles.get(user_id);
        let on = p.as_ref().map(|x| x.copy_enabled).unwrap_or(true);
        match p {
            Some(p) => copy_keyboard_tasks(on, &p.tasks),
            None => copy_keyboard(on, &[], &HashMap::new()),
        }
    }

    pub(crate) async fn copy_page_reply(&self, user_id: i64, note: Option<&str>) -> Reply {
        self.resume_copies_if_any_setup_active(user_id);
        Reply::page(self.page_copy_note(user_id, note).await, self.copy_kb_for(user_id))
    }

    pub(crate) fn resume_copies_if_any_setup_active(&self, user_id: i64) {
        let Some(p) = self.profiles.get(user_id) else {
            return;
        };
        if p.copy_enabled || !p.tasks.iter().any(|t| t.setup.active) {
            return;
        }
        let chat_id = p.chat_id;
        let _ = self.profiles.patch_profile(user_id, chat_id, |p| p.copy_enabled = true);
    }

    pub(crate) async fn page_copy_setup(&self, user_id: i64, idx: usize) -> Reply {
        let _ = self.profiles.reload_from_disk(user_id);
        self.resume_copies_if_any_setup_active(user_id);
        let Some(p) = self.profiles.get(user_id) else {
            return self.copy_page_reply(user_id, Some("No copy setups yet.")).await;
        };
        let Some(task) = p.tasks.get(idx).cloned() else {
            return self.copy_page_reply(user_id, Some("That setup is gone. Refresh.")).await;
        };
        let (wallet_label, extra) = match self.ensure_wallet(user_id) {
            Some(w) => {
                let bal = self.user_balance(&w).await;
                let tokens = if self.rpc.configured() {
                    self.rpc
                        .token_positions(&w.pubkey)
                        .await
                        .map(|p| p.len())
                        .unwrap_or(0)
                } else {
                    0
                };
                let extra = format!(
                    "\n💰 W1 balance: {} · token accounts: {tokens}\n",
                    bal_label(bal)
                );
                (w.label, extra)
            }
            None => ("W1".into(), String::new()),
        };
        Reply::page(
            copy_setup_text(&wallet_label, &task, BOT_USERNAME, Some(&extra)),
            copy_setup_keyboard(idx, &wallet_label, &task),
        )
    }

    pub(crate) fn setup_prompt_kb(idx: usize) -> serde_json::Value {
        serde_json::json!({
            "inline_keyboard": [[btn("← Back", &format!("c:w:{idx}"))]]
        })
    }

    pub(crate) fn set_strat_draft(&self, user_id: i64, idx: usize, order: LimitOrder) {
        self.strat_drafts
            .lock()
            .expect("drafts")
            .insert(user_id, (idx, order));
    }

    pub(crate) fn strat_draft(&self, user_id: i64, idx: usize) -> LimitOrder {
        let mut g = self.strat_drafts.lock().expect("drafts");
        let e = g.entry(user_id).or_insert((idx, LimitOrder::default()));
        e.0 = idx;
        e.1.clone()
    }

    pub(crate) fn patch_strat_draft(&self, user_id: i64, idx: usize, f: impl FnOnce(&mut LimitOrder)) {
        let mut g = self.strat_drafts.lock().expect("drafts");
        let e = g.entry(user_id).or_insert((idx, LimitOrder::default()));
        e.0 = idx;
        f(&mut e.1);
    }

    pub(crate) async fn page_strategies(&self, user_id: i64, idx: usize) -> Reply {
        let _ = self.profiles.reload_from_disk(user_id);
        let Some(p) = self.profiles.get(user_id) else {
            return self.copy_page_reply(user_id, Some("No copy setups yet.")).await;
        };
        let Some(task) = p.tasks.get(idx) else {
            return self.copy_page_reply(user_id, Some("That setup is gone. Refresh.")).await;
        };
        Reply::page(strategies_text(task), strategies_keyboard(idx, task))
    }

    pub(crate) fn page_limit_order(&self, user_id: i64, idx: usize) -> Reply {
        let draft = self.strat_draft(user_id, idx);
        Reply::page(limit_order_text(), limit_order_keyboard(idx, &draft))
    }

    pub(crate) async fn apply_strat(
        &self,
        user_id: i64,
        chat_id: i64,
        action: CallbackAction,
    ) -> Reply {
        match action {
            CallbackAction::StratList { idx } => self.page_strategies(user_id, idx).await,
            CallbackAction::StratNew { idx } => {
                self.set_strat_draft(user_id, idx, LimitOrder::default());
                self.page_limit_order(user_id, idx)
            }
            CallbackAction::StratOpen { idx, oid } => {
                if oid != 0 {
                    let order = self
                        .profiles
                        .get(user_id)
                        .and_then(|p| p.tasks.get(idx).cloned())
                        .and_then(|t| t.orders.into_iter().find(|o| o.id == oid))
                        .unwrap_or_default();
                    self.set_strat_draft(user_id, idx, order);
                }
                self.page_limit_order(user_id, idx)
            }
            CallbackAction::StratDel { idx, oid } => {
                let _ = self.profiles.update_task(user_id, chat_id, idx, |t| {
                    t.orders.retain(|o| o.id != oid);
                });
                self.page_strategies(user_id, idx).await
            }
            CallbackAction::StratSave { idx } => {
                let draft = self.strat_draft(user_id, idx);
                let _ = self.profiles.update_task(user_id, chat_id, idx, |t| {
                    if draft.id == 0 {
                        let next = t.orders.iter().map(|o| o.id).max().unwrap_or(0) + 1;
                        let mut o = draft.clone();
                        o.id = next;
                        if o.mode == StrategyMode::ProfitLoss {
                            o.trigger_pct = o.sell_at_pct;
                        }
                        if o.mode == StrategyMode::FixedMcap {
                            o.mcap_usd = o.sell_at_pct;
                        }
                        t.orders.push(o);
                    } else if let Some(ex) = t.orders.iter_mut().find(|o| o.id == draft.id) {
                        *ex = draft.clone();
                        if ex.mode == StrategyMode::ProfitLoss {
                            ex.trigger_pct = ex.sell_at_pct;
                        }
                        if ex.mode == StrategyMode::FixedMcap {
                            ex.mcap_usd = ex.sell_at_pct;
                        }
                    }
                });
                self.page_strategies(user_id, idx).await
            }
            CallbackAction::StratMode { idx, mode } => {
                if matches!(mode, StrategyMode::ProfitLoss | StrategyMode::FixedMcap) {
                    self.patch_strat_draft(user_id, idx, |d| {
                        d.mode = mode;
                        d.picked = true;
                    });
                }
                self.page_limit_order(user_id, idx)
            }
            CallbackAction::StratToggleProt { idx } => {
                self.patch_strat_draft(user_id, idx, |d| d.sell_protection = !d.sell_protection);
                self.page_limit_order(user_id, idx)
            }
            CallbackAction::StratToggleHi { idx } => {
                self.patch_strat_draft(user_id, idx, |d| d.high_impact = !d.high_impact);
                self.page_limit_order(user_id, idx)
            }
            CallbackAction::StratPrompt { idx, field } => {
                self.set_awaiting(user_id, Some(AwaitKind::StratVal(idx, field)));
                let prompt: String = match field {
                    StratField::SellAt => match self.strat_draft(user_id, idx).mode {
                        StrategyMode::FixedMcap => "Send target market cap in USD.".into(),
                        _ => sell_at_pnl_prompt(),
                    },
                    StratField::Amount => "Send sell amount percent.".into(),
                    StratField::Slip => "Send slippage percent.".into(),
                    StratField::Tip => "Send tip in SOL.".into(),
                    StratField::Fee => "Send priority fee in SOL.".into(),
                };
                Reply::page(
                    prompt,
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", &format!("g:{idx}:E"))]]
                    }),
                )
            }
            _ => self.copy_page_reply(user_id, None).await,
        }
    }

    pub(crate) async fn finish_strat_value(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        idx: usize,
        field: StratField,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let n: f64 = match parse_strat_number(text) {
            Some(n) => n,
            None => {
                return Reply::page(
                    "Send a number.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", &format!("g:{idx}:E"))]]
                    }),
                );
            }
        };
        self.set_awaiting(user_id, None);
        self.patch_strat_draft(user_id, idx, |d| match field {
            StratField::SellAt => d.sell_at_pct = n,
            StratField::Amount => d.sell_amount_pct = n.max(0.0).min(100.0),
            StratField::Slip => d.slippage_pct = n.max(0.0).min(100.0),
            StratField::Tip => d.tip_sol = n.max(0.0),
            StratField::Fee => d.fee_sol = n.max(0.0),
        });
        let _ = chat_id;
        self.page_limit_order(user_id, idx)
    }

    pub(crate) async fn apply_setup_op(
        &self,
        user_id: i64,
        chat_id: i64,
        idx: usize,
        op: SetupOp,
    ) -> Reply {
        match op {
            SetupOp::Wallet => self.page_tasks_wallet(user_id, Some(idx)).await,
            SetupOp::Refresh => self.page_copy_setup(user_id, idx).await,
            SetupOp::Name => {
                self.set_awaiting(user_id, Some(AwaitKind::SetupName(idx)));
                Reply::page(
                    "Send a new config name (e.g. Aaliya).",
                    Self::setup_prompt_kb(idx),
                )
            }
            SetupOp::Target => {
                self.set_awaiting(user_id, Some(AwaitKind::SetupTarget(idx)));
                Reply::page(
                    "Send the new target wallet address.",
                    Self::setup_prompt_kb(idx),
                )
            }
            SetupOp::MinTrigger | SetupOp::MaxTrigger => {
                let which = if op == SetupOp::MinTrigger { "mit" } else { "mat" };
                Reply::page(
                    "Set this threshold in SOL or USDT, or clear it.",
                    serde_json::json!({
                        "inline_keyboard": [
                            [
                                btn("SOL", &format!("q:{idx}:{which}:sol")),
                                btn("USDT", &format!("q:{idx}:{which}:usd")),
                                btn("n/a", &format!("q:{idx}:{which}:na")),
                            ],
                            [btn("← Back", &format!("c:w:{idx}"))],
                        ]
                    }),
                )
            }
            SetupOp::MinMcap
            | SetupOp::MaxMcap
            | SetupOp::MinAge
            | SetupOp::MaxAge
            | SetupOp::BuyAmount
            | SetupOp::BuyFee
            | SetupOp::SellFee
            | SetupOp::BuySlip
            | SetupOp::SellSlip
            | SetupOp::BuyTip
            | SetupOp::SellTip => {
                let field = match op {
                    SetupOp::MinMcap => SetupValue::MinMcap,
                    SetupOp::MaxMcap => SetupValue::MaxMcap,
                    SetupOp::MinAge => SetupValue::MinAge,
                    SetupOp::MaxAge => SetupValue::MaxAge,
                    SetupOp::BuyAmount => SetupValue::BuyAmount,
                    SetupOp::BuyFee => SetupValue::BuyFee,
                    SetupOp::SellFee => SetupValue::SellFee,
                    SetupOp::BuySlip => SetupValue::BuySlip,
                    SetupOp::SellSlip => SetupValue::SellSlip,
                    SetupOp::BuyTip => SetupValue::BuyTip,
                    SetupOp::SellTip => SetupValue::SellTip,
                    _ => unreachable!(),
                };
                self.set_awaiting(user_id, Some(AwaitKind::SetupValue(idx, field)));
                Reply::page(setup_value_prompt(field), Self::setup_prompt_kb(idx))
            }
            SetupOp::Strategies => self.page_strategies(user_id, idx).await,
            SetupOp::Duplicate => {
                self.set_awaiting(user_id, Some(AwaitKind::DupTarget(idx)));
                Reply::page(
                    "Duplicate: send the new target wallet address.",
                    Self::setup_prompt_kb(idx),
                )
            }
            SetupOp::Delete => match self.profiles.delete_task(user_id, chat_id, idx) {
                Ok(_) => self.copy_page_reply(user_id, None).await,
                Err(e) => Reply::text(format!("Could not delete: {e:#}")),
            },
            SetupOp::SellOnTransfer
            | SetupOp::SkipDeploys
            | SetupOp::FollowSells
            | SetupOp::BuyOnce
            | SetupOp::BuyProtect
            | SetupOp::SellProtect
            | SetupOp::AutoTip
            | SetupOp::BuyExact
            | SetupOp::BuyFixed => {
                let res = self.profiles.update_task(user_id, chat_id, idx, |t| match op {
                    SetupOp::SellOnTransfer => t.setup.sell_on_transfer = !t.setup.sell_on_transfer,
                    SetupOp::SkipDeploys => t.setup.skip_deploys = !t.setup.skip_deploys,
                    SetupOp::FollowSells => t.setup.follow_sells = !t.setup.follow_sells,
                    SetupOp::BuyOnce => t.setup.buy_once = !t.setup.buy_once,
                    SetupOp::BuyProtect => t.setup.buy_protection = !t.setup.buy_protection,
                    SetupOp::SellProtect => t.setup.sell_protection = !t.setup.sell_protection,
                    SetupOp::AutoTip => t.setup.auto_tip = !t.setup.auto_tip,
                    SetupOp::BuyExact => t.setup.buy_mode = BuyMode::Exact,
                    SetupOp::BuyFixed => t.setup.buy_mode = BuyMode::Fixed,
                    _ => {}
                });
                match res {
                    Ok(_) => self.page_copy_setup(user_id, idx).await,
                    Err(e) => Reply::text(format!("Could not update: {e:#}")),
                }
            }
            SetupOp::Active => {
                let res = self.profiles.patch_profile(user_id, chat_id, |p| {
                    if let Some(t) = p.tasks.get_mut(idx) {
                        t.setup.active = !t.setup.active;
                        if t.setup.active {
                            p.copy_enabled = true;
                        }
                    }
                });
                match res {
                    Ok(_) => self.page_copy_setup(user_id, idx).await,
                    Err(e) => Reply::text(format!("Could not update: {e:#}")),
                }
            }
        }
    }

    pub(crate) async fn finish_setup_name(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        idx: usize,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let name = crate::profile::sanitize_watch_name(text);
        if name.is_empty() || crate::config::parse_pubkey(text.trim()).is_ok() {
            return Reply::page(
                "Send a short name (e.g. Aaliya), not a wallet address.",
                Self::setup_prompt_kb(idx),
            );
        }
        self.set_awaiting(user_id, None);
        let _ = self.profiles.update_task(user_id, chat_id, idx, |t| t.name = name);
        self.page_copy_setup(user_id, idx).await
    }

    pub(crate) async fn finish_setup_target(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        idx: usize,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let addr = text.split_whitespace().next().unwrap_or("");
        if crate::config::parse_pubkey(addr).is_err() {
            return Reply::page(
                "That is not a Solana wallet address. Send the target wallet.",
                Self::setup_prompt_kb(idx),
            );
        }
        self.set_awaiting(user_id, None);
        let _ = self
            .profiles
            .update_task(user_id, chat_id, idx, |t| {
                if let Ok(k) = crate::config::parse_pubkey(addr) {
                    t.address = crate::config::encode_pubkey(&k);
                }
            });
        self.page_copy_setup(user_id, idx).await
    }

    pub(crate) async fn finish_setup_value(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        idx: usize,
        field: SetupValue,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let parsed = parse_setup_number(text, field);
        match parsed {
            Err(msg) => Reply::page(msg, Self::setup_prompt_kb(idx)),
            Ok(v) => {
                self.set_awaiting(user_id, None);
                let _ = self.profiles.update_task(user_id, chat_id, idx, |t| {
                    apply_setup_number(&mut t.setup, field, v);
                });
                self.page_copy_setup(user_id, idx).await
            }
        }
    }

    pub(crate) async fn finish_dup_target(
        &self,
        user_id: i64,
        username: Option<&str>,
        _chat_id: i64,
        idx: usize,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let addr = text.split_whitespace().next().unwrap_or("");
        if crate::config::parse_pubkey(addr).is_err() {
            return Reply::page(
                "That is not a Solana wallet. Send the new target address.",
                Self::setup_prompt_kb(idx),
            );
        }
        let key = crate::config::parse_pubkey(addr).unwrap();
        let encoded = crate::config::encode_pubkey(&key);
        self.set_awaiting(user_id, Some(AwaitKind::DupName(idx, encoded.clone())));
        Reply::page(
            format!("Duplicate: send a unique name for `{encoded}`."),
            Self::setup_prompt_kb(idx),
        )
    }

    pub(crate) async fn finish_dup_name(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        idx: usize,
        addr: &str,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let name = crate::profile::sanitize_watch_name(text);
        if name.is_empty() || crate::config::parse_pubkey(text.trim()).is_ok() {
            return Reply::page(
                "Send a unique short name (not a wallet address).",
                Self::setup_prompt_kb(idx),
            );
        }
        self.set_awaiting(user_id, None);
        match self
            .profiles
            .duplicate_task_as(user_id, chat_id, idx, addr, &name)
        {
            Ok((_, new_idx)) => self.page_copy_setup(user_id, new_idx).await,
            Err(e) => Reply::page(
                format!("Could not duplicate: {e:#}\nSend another name."),
                Self::setup_prompt_kb(idx),
            ),
        }
    }

    pub(crate) async fn apply_quote_unit(
        &self,
        user_id: i64,
        chat_id: i64,
        idx: usize,
        min: bool,
        unit: QuotePick,
    ) -> Reply {
        match unit {
            QuotePick::Clear => {
                let _ = self.profiles.update_task(user_id, chat_id, idx, |t| {
                    if min {
                        t.setup.min_trigger_buy = None;
                    } else {
                        t.setup.max_trigger_buy = None;
                    }
                });
                self.page_copy_setup(user_id, idx).await
            }
            QuotePick::Sol | QuotePick::Usdt => {
                let q = if unit == QuotePick::Sol {
                    QuoteCurrency::Sol
                } else {
                    QuoteCurrency::Usdt
                };
                let _ = self.profiles.update_task(user_id, chat_id, idx, |t| {
                    if min {
                        t.setup.min_trigger_quote = q;
                    } else {
                        t.setup.max_trigger_quote = q;
                    }
                });
                let field = if min {
                    SetupValue::MinTrigger
                } else {
                    SetupValue::MaxTrigger
                };
                self.set_awaiting(user_id, Some(AwaitKind::SetupValue(idx, field)));
                Reply::page(
                    format!(
                        "Send the {} trigger in {} (or `n/a`).",
                        if min { "min" } else { "max" },
                        q.label()
                    ),
                    Self::setup_prompt_kb(idx),
                )
            }
        }
    }

    pub async fn reply_for(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        cmd: Command,
    ) -> Reply {
        self.reply_for_args(user_id, username, chat_id, cmd, "").await
    }

    pub async fn reply_for_args(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        cmd: Command,
        args: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            return Self::deny_reply(user_id);
        }
        let _ = self.profiles.ensure(user_id, chat_id);
        if cmd == Command::Calendar {
            return self.page_calendar_reply(user_id).await;
        }
        if cmd == Command::Positions {
            return self.page_positions_reply(user_id).await;
        }
        if cmd == Command::Start {
            self.subscribe(chat_id);
            if let Some(mint) = start_ca_arg(args) {
                return self.open_trade_for_mint(user_id, chat_id, mint).await;
            }
            let text = self.page_start(user_id).await;
            return Reply::page(text, home_keyboard());
        }
        if cmd == Command::Withdraw {
            return self.page_withdraw_reply(user_id, None).await;
        }
        if cmd == Command::Orders {
            return self.page_orders_reply(user_id).await;
        }
        if cmd == Command::Bots {
            return self.page_bots_reply(user_id).await;
        }
        if cmd == Command::Pumpfun {
            return self.page_pumpfun_reply(user_id).await;
        }
        let text = match cmd {
            Command::Menu => {
                "📋 menu\n\nCustomize the bot with the buttons below.".into()
            }
            Command::Start => unreachable!(),
            Command::Home => self.page_home(user_id).await,
            Command::Calendar => unreachable!(),
            Command::Positions => unreachable!(),
            Command::Copy => self.page_copy(user_id).await,
            Command::Watch => self.cmd_watch(user_id, chat_id, args).await,
            Command::Unwatch => self.cmd_unwatch(user_id, args).await,
            Command::MaxSol => self.cmd_maxsol(user_id, chat_id, args),
            Command::CopyOn => self.cmd_copy_toggle(user_id, chat_id, true),
            Command::CopyOff => self.cmd_copy_toggle(user_id, chat_id, false),
            Command::Orders => unreachable!(),
            Command::Settings => settings_text(),
            Command::Wallets => self.page_wallets(user_id).await,
            Command::Withdraw => unreachable!(),
            Command::Bots => unreachable!(),
            Command::Pumpfun => unreachable!(),
            Command::Stop => {
                if self.unsubscribe(chat_id) {
                    "🔕 alerts off for this chat. Your watch list is unchanged.".into()
                } else {
                    "🤷 you were not subscribed.".into()
                }
            }
            Command::Help => help_text(),
        };
        let copy_on = self
            .profiles
            .get(user_id)
            .map(|p| p.copy_enabled)
            .unwrap_or(true);
        let kb = match cmd {
            Command::Copy
            | Command::Watch
            | Command::Unwatch
            | Command::MaxSol
            | Command::CopyOn
            | Command::CopyOff => Some(self.copy_kb_for(user_id)),
            Command::Settings => Some(settings_keyboard(&self.bot_settings(user_id))),
            Command::Wallets => Some(self.wallets_settings_kb(user_id).await),
            other => keyboard_for(other, copy_on),
        };
        match kb {
            Some(kb) => Reply::page(text, kb),
            None => Reply::text(text),
        }
    }

    pub(crate) fn deny_reply(user_id: i64) -> Reply {
        Reply::text(format!(
            "🚫 not on the allowlist. your telegram user id is `{user_id}` — send that to the operator"
        ))
    }

    pub(crate) async fn page_start(&self, user_id: i64) -> String {
        match self.wallets.get_or_create(user_id) {
            Ok(created) => {
                let bal = self.user_balance(&created.wallet).await;
                let px = self.rpc.sol_usd().await.ok();
                welcome_text(&created, bal, px)
            }
            Err(e) => format!(
                "🚀 Start trading with {BOT_NAME}.\n\nCould not create your wallet: {e:#}"
            ),
        }
    }

    pub(crate) async fn page_home(&self, user_id: i64) -> String {
        let live = self.live.snapshot_user(user_id);
        let rpc = self.rpc_line(&self.live.snapshot()).await;
        let profile = self.profiles.get(user_id);
        let yours = match self.wallets.get_or_create(user_id) {
            Ok(created) => {
                let bal = self.user_balance(&created.wallet).await;
                let w = &created.wallet;
                let note = if created.created {
                    "\n✨ W1 was just created (you started before wallets existed)."
                } else {
                    ""
                };
                format!("{note}\n👛 {}: `{}`\n💰 Balance: {}", w.label, w.pubkey, bal_label(bal))
            }
            Err(e) => format!("\nwallet store: {e:#}"),
        };
        let watched = profile.as_ref().map(|p| p.watched.len()).unwrap_or(0);
        let copy_on = profile.as_ref().map(|p| p.copy_enabled).unwrap_or(true);
        format!(
            "🏠 homepage\n\n👀 Your copy list: {watched} wallet(s)\n🪞 Copies: {}\n⚡ Your copy signals: {}\n⚪️ Your skips: {}\n{}{}\n\n`/watch ADDRESS` to follow a trader.\n📋 Tap /menu for copy, wallets, settings.",
            if copy_on { "🟢 on" } else { "⏸️ paused" },
            live.copies,
            live.skips,
            rpc,
            yours
        )
    }

    pub(crate) async fn page_wallets(&self, user_id: i64) -> String {
        let _ = self.wallets.get_or_create(user_id);
        let all = self.wallets.wallets_for(user_id);
        let active = self.wallets.active_index(user_id);
        let entries: Vec<(bool, String, String)> = all
            .into_iter()
            .enumerate()
            .map(|(i, w)| (i == active, w.label, w.pubkey))
            .collect();
        wallets_settings_text(&entries)
    }

    pub(crate) async fn wallets_settings_kb(&self, user_id: i64) -> serde_json::Value {
        let _ = self.wallets.get_or_create(user_id);
        let all = self.wallets.wallets_for(user_id);
        let active = self.wallets.active_index(user_id);
        let mut picks = Vec::new();
        for (i, w) in all.iter().enumerate() {
            let bal = self.user_balance(w).await;
            let sol = match bal {
                Some(n) => trim_num(n as f64 / 1_000_000_000.0),
                None => "0".into(),
            };
            let dot = if i == active { "🟢" } else { "⚪️" };
            picks.push(format!("{dot} {} · {sol} SOL", w.label));
        }
        wallets_settings_keyboard(&picks)
    }

    pub(crate) async fn apply_set_op(&self, user_id: i64, chat_id: i64, op: SetOp) -> Reply {
        let _ = self.profiles.ensure(user_id, chat_id);
        match op {
            SetOp::Slip => Reply::page(
                "Send a slippage percent, or tap Back.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "p:settings")]]
                }),
            ),
            SetOp::Presets => Reply::page(presets_text(), presets_keyboard()),
            SetOp::BuyPresets => {
                let amounts = self
                    .profiles
                    .get(user_id)
                    .map(|p| p.buy_presets)
                    .unwrap_or_else(crate::profile::default_buy_presets);
                Reply::page(buy_amount_text(), buy_amount_keyboard(&amounts))
            }
            SetOp::SellPresets => {
                let pcts = self
                    .profiles
                    .get(user_id)
                    .map(|p| p.sell_presets)
                    .unwrap_or_else(crate::profile::default_sell_presets);
                Reply::page(sell_percentage_text(), sell_percentage_keyboard(&pcts))
            }
            SetOp::SpotPresets | SetOp::CopyPresets => Reply::page(presets_text(), presets_keyboard()),
            SetOp::EditBuyAmt(i) => {
                self.set_awaiting(user_id, Some(AwaitKind::BuyPreset(i)));
                Reply::page(
                    "Send a buy amount in SOL.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "pre:buy")]]
                    }),
                )
            }
            SetOp::EditSellAmt(i) => {
                self.set_awaiting(user_id, Some(AwaitKind::SellPreset(i)));
                Reply::page(
                    "Send a sell percentage (0–100).",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "pre:sell")]]
                    }),
                )
            }
            SetOp::Currency | SetOp::Flags => self.settings_reply(user_id),
            SetOp::Degen => {
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| p.degen_mode = !p.degen_mode);
                self.settings_reply(user_id)
            }
            SetOp::Expert => {
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| p.expert_mode = !p.expert_mode);
                self.settings_reply(user_id)
            }
            SetOp::BuyProt => {
                let _ = self
                    .profiles
                    .patch_profile(user_id, chat_id, |p| p.settings_buy_protection = !p.settings_buy_protection);
                self.settings_reply(user_id)
            }
            SetOp::PrivatePnl => {
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| p.private_pnl = !p.private_pnl);
                self.settings_reply(user_id)
            }
            SetOp::WalCreate => {
                match self.wallets.create_new(user_id) {
                    Ok(created) => {
                        let mut text = self.page_wallets(user_id).await;
                        let sk = created
                            .wallet
                            .secret_key_b58()
                            .unwrap_or_default();
                        text = format!(
                            "✅ Created {}.\nAddress:\n`{}`\n\n\
🔐 Private key — save this now. It is shown only this once.\n`{sk}`\n\
Import this in Phantom / Solflare. Anyone with it can take your SOL.\n\n{text}",
                            created.wallet.label, created.wallet.pubkey
                        );
                        Reply::page(text, self.wallets_settings_kb(user_id).await)
                    }
                    Err(e) => {
                        let mut text = self.page_wallets(user_id).await;
                        text = format!("Could not create wallet: {e:#}\n\n{text}");
                        Reply::page(text, self.wallets_settings_kb(user_id).await)
                    }
                }
            }
            SetOp::WalImport => {
                self.set_awaiting(user_id, Some(AwaitKind::WalImport));
                Reply::page(
                    "🔑 Import Wallet\n\n\
Paste a Solana private key (not the public address):\n\
• Phantom / Solflare secret (base58)\n\
• JSON array from a Solana `id.json`\n\
• 32-byte seed (base58 or hex)\n\n\
This adds a new wallet. Your current wallet is kept.\n\
Your message is deleted after import.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "p:wallets")]]
                    }),
                )
            }
            SetOp::WalPick => {
                let text = self.page_wallets(user_id).await;
                Reply::page(text, self.wallets_settings_kb(user_id).await)
            }
            SetOp::WalSelect(i) => {
                let _ = self.wallets.select(user_id, i);
                let text = self.page_wallets(user_id).await;
                Reply::page(text, self.wallets_settings_kb(user_id).await)
            }
            SetOp::WalExport => Reply::page(
                "🔐 Export Private Key\n\n\
Anyone with this key can take the SOL in this wallet.\n\
Only reveal it in a private chat, then store it offline.\n\n\
Tap Show Key to reveal the Phantom / Solflare secret key.",
                serde_json::json!({
                    "inline_keyboard": [
                        [btn("🔐 Show Key", "w:show")],
                        [btn("← Back", "p:wallets"), btn("🗑 Close", "x:close")],
                    ]
                }),
            ),
            SetOp::WalExportShow => self.page_export_key(user_id).await,
        }
    }

    pub(crate) async fn finish_wallet_import(
        &self,
        user_id: i64,
        username: Option<&str>,
        _chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        match self.wallets.import_secret(user_id, text) {
            Ok(w) => {
                self.set_awaiting(user_id, None);
                let mut page = self.page_wallets(user_id).await;
                page = format!(
                    "✅ Added {}.\nAddress:\n`{}`\n\nYour other wallets are unchanged.\n\n{page}",
                    w.label, w.pubkey
                );
                Reply::page(page, self.wallets_settings_kb(user_id).await)
            }
            Err(_) => Reply::page(
                "That is not a Solana private key. Paste a Phantom/Solflare secret, JSON array, or 32-byte seed.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "p:wallets")]]
                }),
            ),
        }
    }

    pub(crate) async fn page_export_key(&self, user_id: i64) -> Reply {
        let kb = serde_json::json!({
            "inline_keyboard": [
                [btn("← Back", "p:wallets"), btn("🗑 Close", "x:close")],
            ]
        });
        match self.wallets.get_or_create(user_id) {
            Ok(created) => match created.wallet.secret_key_b58() {
                Some(sk) => Reply::page(
                    format!(
                        "🔐 {} private key\n\n\
Address:\n`{}`\n\n\
Secret key (base58, seed + pubkey):\n`{sk}`\n\n\
Import this in Phantom / Solflare. Delete this message when you have saved it.\n\
Anyone with this key can take your SOL.",
                        created.wallet.label,
                        created.wallet.pubkey
                    ),
                    kb,
                ),
                None => Reply::page(
                    "🔐 Could not encode this wallet’s private key.",
                    kb,
                ),
            },
            Err(e) => Reply::page(
                format!("🔐 Could not load a wallet: {e:#}"),
                kb,
            ),
        }
    }

    pub(crate) async fn finish_buy_preset(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        i: usize,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send a buy amount in SOL.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "pre:buy")]]
                }),
            );
        };
        if n < 0.0 {
            return Reply::page(
                "Need a number ≥ 0.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "pre:buy")]]
                }),
            );
        }
        self.set_awaiting(user_id, None);
        let _ = self.profiles.patch_profile(user_id, chat_id, |p| {
            if let Some(slot) = p.buy_presets.get_mut(i) {
                *slot = n;
            }
        });
        let amounts = self
            .profiles
            .get(user_id)
            .map(|p| p.buy_presets)
            .unwrap_or_else(crate::profile::default_buy_presets);
        Reply::page(buy_amount_text(), buy_amount_keyboard(&amounts))
    }

    pub(crate) async fn finish_sell_preset(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        i: usize,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send a sell percentage (0–100).",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "pre:sell")]]
                }),
            );
        };
        if !(0.0..=100.0).contains(&n) {
            return Reply::page(
                "Need a percentage from 0 to 100.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "pre:sell")]]
                }),
            );
        }
        self.set_awaiting(user_id, None);
        let _ = self.profiles.patch_profile(user_id, chat_id, |p| {
            if let Some(slot) = p.sell_presets.get_mut(i) {
                *slot = n;
            }
        });
        let pcts = self
            .profiles
            .get(user_id)
            .map(|p| p.sell_presets)
            .unwrap_or_else(crate::profile::default_sell_presets);
        Reply::page(sell_percentage_text(), sell_percentage_keyboard(&pcts))
    }

    pub(crate) fn bot_settings(&self, user_id: i64) -> BotSettings {
        self.profiles
            .get(user_id)
            .map(|p| BotSettings {
                degen_mode: p.degen_mode,
                expert_mode: p.expert_mode,
                buy_protection: p.settings_buy_protection,
                private_pnl: p.private_pnl,
            })
            .unwrap_or_default()
    }

    pub(crate) fn settings_reply(&self, user_id: i64) -> Reply {
        Reply::page(settings_text(), settings_keyboard(&self.bot_settings(user_id)))
    }

    pub(crate) async fn page_copy(&self, user_id: i64) -> String {
        self.resume_copies_if_any_setup_active(user_id);
        self.page_copy_note(user_id, None).await
    }

    pub(crate) async fn page_tasks_wallet(&self, user_id: i64, back_setup: Option<usize>) -> Reply {
        self.tasks_wallet_back
            .lock()
            .expect("twb")
            .insert(user_id, back_setup);
        let _ = self.wallets.get_or_create(user_id);
        let all = self.wallets.wallets_for(user_id);
        let active = self.wallets.active_index(user_id);
        let mut picks = Vec::new();
        for (i, w) in all.iter().enumerate() {
            let bal = self.user_balance(w).await;
            let sol = match bal {
                Some(0) | None => "0".into(),
                Some(n) => fmt_sol(n),
            };
            let mark = if i == active { "🟢" } else { "⚪️" };
            picks.push((format!("{mark} {} • {sol} SOL", w.label), i));
        }
        let back = match back_setup {
            Some(idx) => format!("c:w:{idx}"),
            None => "p:copy".into(),
        };
        Reply::page(tasks_wallet_text(), tasks_wallet_keyboard(&picks, &back))
    }

    pub(crate) async fn pick_tasks_wallet(&self, user_id: i64, i: usize) -> Reply {
        let _ = self.wallets.get_or_create(user_id);
        let note = match self.wallets.select(user_id, i) {
            Ok(w) => format!("✅ Copy wallet is now {} (`{}`).", w.label, w.pubkey),
            Err(e) => format!("Could not switch wallet: {e:#}"),
        };
        let back = self
            .tasks_wallet_back
            .lock()
            .expect("twb")
            .remove(&user_id)
            .flatten();
        match back {
            Some(idx) => {
                let mut page = self.page_copy_setup(user_id, idx).await;
                page.text = format!("{note}\n\n{}", page.text);
                page
            }
            None => self.copy_page_reply(user_id, Some(&note)).await,
        }
    }

    pub(crate) async fn page_copy_note(&self, user_id: i64, note: Option<&str>) -> String {
        let profile = self.profiles.get(user_id);
        let copy_wallet = match self.wallets.get_or_create(user_id) {
            Ok(created) => format!("└ {}: `{}`", created.wallet.label, created.wallet.pubkey),
            Err(e) => format!("└ Could not load W1: {e:#}"),
        };
        let copy_on = profile.as_ref().map(|p| p.copy_enabled).unwrap_or(true);
        let setups = match profile.as_ref() {
            Some(p) if !p.tasks.is_empty() => p
                .tasks
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mark = on_dot(t.setup.active);
                    format!("{mark} {}. {}\n`{}`", i + 1, t.display_name(), t.address)
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let mut lines = vec![
            "🔎 Copy Trading".into(),
            String::new(),
            "Automatically mirror trades from selected wallets in real time.".into(),
            String::new(),
            "👛 Copy Wallet:".into(),
            copy_wallet,
            String::new(),
            "🟢 Setup is active".into(),
            "🔴 Setup is inactive".into(),
        ];
        if !copy_on {
            lines.push(String::new());
            lines.push(
                "⏸ Copies paused. Tap Start All, or Active on a setup, to resume.".into(),
            );
        }
        lines.push(String::new());
        lines.push("Active Setups:".into());
        if !setups.is_empty() {
            lines.push(setups);
        }
        lines.push(String::new());
        lines.push(
            "🔄 Update Tasks Wallet: Update the active wallet for all existing tasks.".into(),
        );
        lines.push(String::new());
        lines.push(format!("Manage your tasks on {BOT_NAME} Manager."));
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        if let Some(note) = note {
            if !note.is_empty() {
                lines.push(String::new());
                lines.push(note.into());
            }
        }
        lines.join("\n")
    }

    pub(crate) async fn cmd_watch(&self, user_id: i64, chat_id: i64, args: &str) -> String {
        match parse_watch_input(args) {
            None if args.split_whitespace().next().unwrap_or("").is_empty() => {
                "👀 Add a wallet to your copy list.\n\nUsage: `/watch BASE58_ADDRESS` or `/watch Name BASE58_ADDRESS`".into()
            }
            None => "Could not add wallet: need a Solana wallet address".into(),
            Some((name, addr)) => self.cmd_watch_named(user_id, chat_id, &addr, name.as_deref()).await,
        }
    }

    pub(crate) async fn cmd_watch_named(
        &self,
        user_id: i64,
        chat_id: i64,
        addr: &str,
        name: Option<&str>,
    ) -> String {
        match self.profiles.add_watch_named(user_id, chat_id, addr, name) {
            Ok(p) => {
                let shown = p.watched.last().cloned().unwrap_or_else(|| addr.into());
                let label = p.watch_label(&shown);
                format!(
                    "✅ Watching {label} (`{shown}`)\n\n👀 Your list: {} wallet(s). Copies are {}.\n📡 The feed will pick this wallet up shortly.",
                    p.watched.len(),
                    if p.copy_enabled { "🟢 on" } else { "⏸️ paused" }
                )
            }
            Err(e) => format!("Could not add wallet: {e:#}"),
        }
    }

    pub(crate) async fn cmd_unwatch(&self, user_id: i64, args: &str) -> String {
        let addr = args.split_whitespace().next().unwrap_or("");
        if addr.is_empty() {
            return "🚫 Remove a wallet from your copy list.\n\nUsage: `/unwatch BASE58_ADDRESS`".into();
        }
        match self.profiles.remove_watch(user_id, addr) {
            Ok(Some((p, true))) => format!("🗑️ Removed. Your list: {} wallet(s).", p.watched.len()),
            Ok(Some((_, false))) => "🤷 That address was not on your list.".into(),
            Ok(None) => "👀 You have no watch list yet. Use `/watch ADDRESS`.".into(),
            Err(e) => format!("Could not remove wallet: {e:#}"),
        }
    }

    pub(crate) fn cmd_maxsol(&self, user_id: i64, chat_id: i64, args: &str) -> String {
        let raw = args.split_whitespace().next().unwrap_or("");
        if raw.is_empty() {
            let cur = self
                .profiles
                .get(user_id)
                .map(|p| p.max_sol)
                .unwrap_or_else(|| self.status.max_sol());
            return format!("💰 Your copy size is {cur:.4} SOL.\n\nUsage: `/maxsol 0.05`");
        }
        let Ok(n) = raw.parse::<f64>() else {
            return "Need a number. Example: `/maxsol 0.05`".into();
        };
        match self.profiles.set_max_sol(user_id, chat_id, n) {
            Ok(p) => format!("✅ Your copy size is now {:.4} SOL.", p.max_sol),
            Err(e) => format!("Could not set size: {e:#}"),
        }
    }

    pub(crate) fn cmd_copy_toggle(&self, user_id: i64, chat_id: i64, on: bool) -> String {
        match self.profiles.set_copy_enabled(user_id, chat_id, on) {
            Ok(p) if p.copy_enabled => {
                "🟢 Copies on. You will get alerts for wallets you watch.".into()
            }
            Ok(_) => "⏸️ Copies paused. Your watch list is kept.".into(),
            Err(e) => format!("Could not update: {e:#}"),
        }
    }

    pub(crate) async fn page_positions(&self, user_id: i64) -> String {
        let p = self.profiles.get(user_id);
        let usd = p.as_ref().map(|x| x.positions_usd).unwrap_or(false);
        let hide_pnl = p.as_ref().map(|x| x.private_pnl).unwrap_or(false);
        let mut lines = vec![
            "📊 Open Positions".into(),
            String::new(),
        ];
        let owners: Vec<String> = {
            let ws = self.wallets.wallets_for(user_id);
            if ws.is_empty() {
                self.ensure_pubkey(user_id).into_iter().collect()
            } else {
                ws.into_iter().map(|w| w.pubkey).collect()
            }
        };
        if owners.is_empty() {
            lines.push("No open positions yet!".into());
            lines.push(
                "Start your trading journey by pasting a contract address in chat.".into(),
            );
            lines.push(String::new());
            lines.push(format!("Manage your positions on {BOT_NAME} Manager."));
            lines.push(String::new());
            lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
            return lines.join("\n");
        }
        let mut rpc_pos = Vec::new();
        for owner in &owners {
            if let Ok(mut pos) = self.rpc.token_positions(owner).await {
                rpc_pos.append(&mut pos);
            }
        }
        let mut merged: std::collections::HashMap<String, crate::rpc_read::TokenPos> =
            std::collections::HashMap::new();
        for p in rpc_pos {
            if p.amount == 0 || p.ui_amount < 0.000001 {
                continue;
            }
            merged
                .entry(p.mint.clone())
                .and_modify(|e| {
                    e.amount = e.amount.saturating_add(p.amount);
                    e.ui_amount += p.ui_amount;
                })
                .or_insert(p);
        }
        let mut rpc_pos: Vec<_> = merged.into_values().collect();
        rpc_pos.sort_by(|a, b| {
            b.ui_amount
                .partial_cmp(&a.ui_amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let lots = self.positions.get(user_id);
        if rpc_pos.is_empty() {
            lines.push("No open positions yet!".into());
            lines.push(
                "Start your trading journey by pasting a contract address in chat.".into(),
            );
        } else {
            let sol_usd = self.rpc.sol_usd().await.unwrap_or(150.0);
            let mut shown = 0usize;
            for p in &rpc_pos {
                if p.mint == WSOL_MINT || is_placeholder_payer(&p.mint) {
                    continue;
                }
                let card = self.rpc.token_card(&p.mint).await;
                let ticker = card
                    .symbol
                    .or(card.name)
                    .unwrap_or_else(|| "Token".into());
                let (lamports, curve_mcap) =
                    self.rpc.pump_curve_quote(&p.mint, p.amount, sol_usd).await;
                let mcap = curve_mcap.or(card.mcap_usd);
                let sol = if lamports > 0 {
                    lamports as f64 / 1_000_000_000.0
                } else if let Some(px) = card.price_usd {
                    p.ui_amount * px / sol_usd.max(1.0)
                } else {
                    0.0
                };
                let pnl = lots
                    .lots
                    .get(&p.mint)
                    .and_then(|lot| match (lot.entry_mcap, mcap) {
                        (Some(entry), Some(cur)) if entry > 0.0 => {
                            Some((cur - entry) / entry * 100.0)
                        }
                        _ => None,
                    })
                    .unwrap_or(0.0);
                lines.push(format_open_position_line(
                    &ticker,
                    pnl,
                    sol,
                    usd,
                    sol_usd,
                    hide_pnl,
                    Some(&p.mint),
                ));
                shown += 1;
                if shown >= 12 {
                    break;
                }
            }
            if shown == 0 {
                lines.push("No open positions yet!".into());
                lines.push(
                    "Start your trading journey by pasting a contract address in chat.".into(),
                );
            } else if rpc_pos.len() > shown {
                lines.push(format!("… {} more", rpc_pos.len() - shown));
            }
        }
        lines.push(String::new());
        lines.push(format!("Manage your positions on {BOT_NAME} Manager."));
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        lines.join("\n")
    }

    pub(crate) fn positions_kb_for(&self, user_id: i64) -> serde_json::Value {
        let usd = self
            .profiles
            .get(user_id)
            .map(|p| p.positions_usd)
            .unwrap_or(false);
        positions_keyboard(usd)
    }

    pub(crate) async fn page_positions_reply(&self, user_id: i64) -> Reply {
        Reply::page(self.page_positions(user_id).await, self.positions_kb_for(user_id))
    }

    pub(crate) async fn open_trade_for_mint(&self, user_id: i64, chat_id: i64, mint: String) -> Reply {
        let _ = self.profiles.ensure(user_id, chat_id);
        let _ = self.wallets.get_or_create(user_id);
        self.set_trade(
            user_id,
            TradeView {
                mint,
                sell: false,
                note: String::new(),
            },
        );
        self.page_trade(user_id).await
    }

    pub(crate) fn set_trade(&self, user_id: i64, view: TradeView) {
        self.trade.lock().expect("trade").insert(user_id, view);
    }

    pub(crate) fn trade_view(&self, user_id: i64) -> Option<TradeView> {
        self.trade.lock().expect("trade").get(&user_id).cloned()
    }

    pub(crate) fn patch_trade(&self, user_id: i64, f: impl FnOnce(&mut TradeView)) {
        let mut g = self.trade.lock().expect("trade");
        if let Some(v) = g.get_mut(&user_id) {
            f(v);
        }
    }

    pub(crate) async fn page_trade(&self, user_id: i64) -> Reply {
        let Some(view) = self.trade_view(user_id) else {
            return Reply::text("Paste a contract address to open a trade.");
        };
        let p = self.profiles.get(user_id);
        let buy = p
            .as_ref()
            .map(|x| x.buy_presets)
            .unwrap_or_else(crate::profile::default_buy_presets);
        let sellp = p
            .as_ref()
            .map(|x| x.sell_presets)
            .unwrap_or_else(crate::profile::default_sell_presets);
        let tip = p.as_ref().map(|x| x.trade_tip_sol).unwrap_or(0.001);
        let slip = p.as_ref().map(|x| x.trade_slippage_pct).unwrap_or(10.0);
        let bp = p.as_ref().map(|x| x.settings_buy_protection).unwrap_or(false);
        let auto_tip = p.as_ref().map(|x| x.trade_auto_tip).unwrap_or(false);
        let w = self.wallets.get(user_id).ok().flatten();
        let n_w = self.wallets.wallets_for(user_id).len();
        let wallets_n = self
            .profiles
            .get(user_id)
            .map(|p| p.trade_wallet_indices(n_w).len() as u32)
            .unwrap_or(if w.is_some() { 1 } else { 0 });
        let bal = if let Some(ref wallet) = w {
            self.user_balance(wallet).await
        } else {
            None
        };
        let card = self.rpc.token_card(&view.mint).await;
        let name = card.name;
        let ticker = card.symbol;
        let sol_usd = self.rpc.sol_usd().await.unwrap_or(150.0);
        let (_, curve_mcap) = self.rpc.pump_curve_quote(&view.mint, 0, sol_usd).await;
        let mcap = curve_mcap.or(card.mcap_usd);
        let price = card.price_usd;
        let liq = card.liq_usd;
        let dex = if curve_mcap.is_some() {
            "Pump.fun".to_string()
        } else if !card.dex.is_empty() {
            card.dex
        } else {
            "Solana".to_string()
        };
        let auth = self.rpc.mint_auth(&view.mint).await;
        let renounced = auth.as_ref().map(|a| a.mint_revoked).unwrap_or(false);
        let freeze_off = auth.as_ref().map(|a| a.freeze_revoked).unwrap_or(false);
        let side = if view.sell { "📈 Sell" } else { "📉 Buy" };
        let title_name = match (name, ticker) {
            (Some(n), Some(t)) => format!("{n} · ${t}"),
            (Some(n), None) => n,
            (None, Some(t)) => format!("${t}"),
            (None, None) => "Unknown token".into(),
        };
        let sol_bal = match bal {
            Some(n) => format!("{:.4}", n as f64 / 1_000_000_000.0),
            None => "0".into(),
        };
        let wlabel = w.map(|x| x.label).unwrap_or_else(|| "W1".into());
        let mint = &view.mint;
        let share = format!("https://t.me/share/url?url=https://dexscreener.com/solana/{mint}");
        let mut lines = vec![
            side.to_string(),
            String::new(),
            format!("🪙 {title_name}"),
            format!("`{mint}`"),
            format!("[📤 Share Token]({share})"),
            String::new(),
            "⚙️ Token Data".into(),
            format!("📊 Market Cap: {}", fmt_mcap_short(mcap)),
            format!("💵 Price: {}", fmt_bloom_price(price)),
            format!("💧 Liquidity: {}", fmt_mcap_short(liq)),
            format!("{} Renounced", auth_dot(renounced)),
            format!("{} Freeze", auth_dot(freeze_off)),
            String::new(),
            "🙁 No active limit orders".into(),
            String::new(),
            format!("⭐ {wlabel}: {sol_bal} SOL"),
            format!("Dex: {dex}"),
            format!(
                "[CA](https://solscan.io/token/{mint}) · [DEX](https://dexscreener.com/solana/{mint}) · [GMGN](https://gmgn.ai/sol/token/{mint}) · [PF](https://pump.fun/coin/{mint}) · [BRD](https://birdeye.so/token/{mint}?chain=solana)"
            ),
        ];
        if !view.note.is_empty() {
            lines.push(String::new());
            lines.push(view.note.clone());
        }
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        Reply::page(
            lines.join("\n"),
            trade_keyboard(
                view.sell,
                &buy,
                &sellp,
                tip,
                slip,
                bp,
                auto_tip,
                wallets_n,
                &view.mint,
            ),
        )
    }

    pub(crate) async fn apply_trade(&self, user_id: i64, chat_id: i64, op: TradeOp) -> Reply {
        if matches!(op, TradeOp::Wallets | TradeOp::WalToggle(_)) {
            if let TradeOp::WalToggle(i) = op {
                let n = self.wallets.wallets_for(user_id).len();
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| {
                    p.toggle_trade_wallet(i, n);
                });
            }
            return self.page_trade_wallets(user_id, chat_id).await;
        }
        let Some(view) = self.trade_view(user_id) else {
            return Reply::text("Paste a contract address to open a trade.");
        };
        match op {
            TradeOp::Switch => {
                self.patch_trade(user_id, |v| {
                    v.sell = !v.sell;
                    v.note.clear();
                });
                self.page_trade(user_id).await
            }
            TradeOp::Refresh => self.page_trade(user_id).await,
            TradeOp::Wallets | TradeOp::WalToggle(_) => unreachable!(),
            TradeOp::Buy(i) => {
                let amt = self
                    .profiles
                    .get(user_id)
                    .map(|p| p.buy_presets[i])
                    .unwrap_or(0.0);
                self.run_trade_buy(user_id, chat_id, &view.mint, amt).await
            }
            TradeOp::Sell(i) => {
                let pct = self
                    .profiles
                    .get(user_id)
                    .map(|p| p.sell_presets[i])
                    .unwrap_or(0.0);
                self.run_trade_sell(user_id, chat_id, &view.mint, pct).await
            }
            TradeOp::BuyX => {
                self.set_awaiting(user_id, Some(AwaitKind::TradeBuy));
                Reply::page(
                    "Send a buy amount in SOL.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "tr:r")]]
                    }),
                )
            }
            TradeOp::SellX => {
                self.set_awaiting(user_id, Some(AwaitKind::TradeSell));
                Reply::page(
                    "Send a sell percentage (0–100).",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "tr:r")]]
                    }),
                )
            }
            TradeOp::Tip => {
                self.set_awaiting(user_id, Some(AwaitKind::TradeTip));
                Reply::page(
                    "Send buy tip in SOL.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "tr:r")]]
                    }),
                )
            }
            TradeOp::Slip => {
                self.set_awaiting(user_id, Some(AwaitKind::TradeSlip));
                Reply::page(
                    "Send slippage percent.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "tr:r")]]
                    }),
                )
            }
            TradeOp::Limit => self.page_trade(user_id).await,
            TradeOp::Pnl => self.page_trade_pnl(user_id).await,
            TradeOp::AutoTip => {
                let _ = self
                    .profiles
                    .patch_profile(user_id, chat_id, |p| p.trade_auto_tip = !p.trade_auto_tip);
                self.page_trade(user_id).await
            }
            TradeOp::BuyProt => {
                let _ = self.profiles.patch_profile(user_id, chat_id, |p| {
                    p.settings_buy_protection = !p.settings_buy_protection;
                });
                self.page_trade(user_id).await
            }
            TradeOp::Split => self.page_split_reply(user_id, None).await,
            TradeOp::Currency => {
                self.patch_trade(user_id, |v| {
                    v.note = "Not used on this bot.".into();
                });
                self.page_trade(user_id).await
            }
            TradeOp::Hide => Reply::page(
                "👁 Hidden.",
                serde_json::json!({ "inline_keyboard": [] }),
            ),
        }
    }

    pub(crate) async fn run_trade_buy(&self, user_id: i64, chat_id: i64, mint: &str, sol: f64) -> Reply {
        let lamports = (sol * 1_000_000_000.0).round() as u64;
        let note = match self.engine_arc() {
            Some(eng) => match eng.manual_buy(user_id, chat_id, mint, lamports).await {
                Ok(n) => format!("🟢 {n}"),
                Err(e) => format!("⚪️ buy failed: {e:#}"),
            },
            None => "⚪️ engine not ready".into(),
        };
        self.patch_trade(user_id, |v| {
            v.sell = false;
            v.note = note;
        });
        self.page_trade(user_id).await
    }

    pub(crate) async fn run_trade_sell(&self, user_id: i64, chat_id: i64, mint: &str, pct: f64) -> Reply {
        let note = match self.engine_arc() {
            Some(eng) => match eng.manual_sell(user_id, chat_id, mint, pct).await {
                Ok(n) => format!("🟢 {n}"),
                Err(e) => format!("⚪️ sell failed: {e:#}"),
            },
            None => "⚪️ engine not ready".into(),
        };
        self.patch_trade(user_id, |v| {
            v.sell = true;
            v.note = note;
        });
        self.page_trade(user_id).await
    }

    pub(crate) fn engine_arc(&self) -> Option<Arc<CopyEngine>> {
        self.engine
            .lock()
            .ok()?
            .as_ref()
            .and_then(|w| w.upgrade())
    }

    pub(crate) async fn finish_trade_buy(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send a buy amount in SOL.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        };
        if n <= 0.0 {
            return Reply::page(
                "Need a number > 0.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        }
        self.set_awaiting(user_id, None);
        let mint = self.trade_view(user_id).map(|v| v.mint).unwrap_or_default();
        self.run_trade_buy(user_id, chat_id, &mint, n).await
    }

    pub(crate) async fn finish_trade_sell(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send a sell percentage (0–100).",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        };
        if !(0.0..=100.0).contains(&n) || n == 0.0 {
            return Reply::page(
                "Need a percentage from 0 to 100.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        }
        self.set_awaiting(user_id, None);
        let mint = self.trade_view(user_id).map(|v| v.mint).unwrap_or_default();
        self.run_trade_sell(user_id, chat_id, &mint, n).await
    }

    pub(crate) async fn finish_trade_tip(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send buy tip in SOL.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        };
        if n < 0.0 {
            return Reply::page(
                "Need a number ≥ 0.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        }
        self.set_awaiting(user_id, None);
        let _ = self.profiles.patch_profile(user_id, chat_id, |p| p.trade_tip_sol = n);
        self.page_trade(user_id).await
    }

    pub(crate) async fn finish_trade_slip(
        &self,
        user_id: i64,
        username: Option<&str>,
        chat_id: i64,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send slippage percent.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        };
        if !(0.0..=100.0).contains(&n) {
            return Reply::page(
                "Need a percentage from 0 to 100.",
                serde_json::json!({ "inline_keyboard": [[btn("← Back", "tr:r")]] }),
            );
        }
        self.set_awaiting(user_id, None);
        let _ = self
            .profiles
            .patch_profile(user_id, chat_id, |p| p.trade_slippage_pct = n);
        self.page_trade(user_id).await
    }

    pub(crate) fn cal_view(&self, user_id: i64) -> CalView {
        let mut g = self.cal.lock().expect("cal");
        *g.entry(user_id)
            .or_insert_with(|| CalView::for_unix(pnl::now_unix()))
    }

    pub(crate) fn set_cal_view(&self, user_id: i64, view: CalView) {
        self.cal.lock().expect("cal").insert(user_id, view);
    }

    pub(crate) fn calendar_stamp() -> String {
        let dur = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = dur.as_secs();
        let ms = dur.subsec_millis();
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            (secs / 3600) % 24,
            (secs / 60) % 60,
            secs % 60,
            ms
        )
    }

    pub(crate) async fn page_trade_pnl(&self, user_id: i64) -> Reply {
        let Some(view) = self.trade_view(user_id) else {
            return Reply::text("Paste a contract address to open a trade.");
        };
        if self.profiles.get(user_id).map(|p| p.private_pnl).unwrap_or(false) {
            return Reply::page(
                format!(
                    "🌸 PnL hidden\n\nPrivate PNL is on. Turn it off in Settings to show this card.\n\n🕐 Last updated: {}",
                    last_updated_stamp()
                ),
                serde_json::json!({
                    "inline_keyboard": [
                        [btn("🔄 Refresh", "tr:pnl"), btn("← Back", "tr:r")],
                        [btn("🗑 Close", "x:close")],
                    ]
                }),
            );
        }
        let fills = self.pnl.fills_for_mint(user_id, &view.mint);
        let sol_usd = self.rpc.sol_usd().await.unwrap_or(150.0);
        let hold = self.positions.holding(user_id, &view.mint);
        let (curve_sol, _) = self.rpc.pump_curve_quote(&view.mint, hold, sol_usd).await;
        let mut remain = if curve_sol > 0 {
            Some(curve_sol as f64 / 1_000_000_000.0)
        } else {
            None
        };
        if remain.is_none() {
            if let Some(w) = self.wallets.get(user_id).ok().flatten() {
                if let Ok(pos) = self.rpc.token_positions(&w.pubkey).await {
                    if let Some(p) = pos.into_iter().find(|p| p.mint == view.mint) {
                        if let Some(snap) = self.rpc.dex_token(&view.mint).await {
                            if let Some(px) = snap.price_usd {
                                if sol_usd > 0.0 && p.ui_amount > 0.0 {
                                    remain = Some(px * p.ui_amount / sol_usd);
                                }
                            }
                        }
                    }
                }
            }
        }
        let (name, ticker) = self.rpc.token_identity(&view.mint).await;
        let title = match (name, ticker) {
            (Some(n), Some(t)) => format!("{n} · ${t}"),
            (Some(n), None) => n,
            (None, Some(t)) => format!("${t}"),
            (None, None) => BOT_NAME.to_string(),
        };
        let text = pnl::render_token_pnl(
            &title,
            &view.mint,
            &fills,
            remain,
            &Self::calendar_stamp(),
        );
        Reply::page(
            text,
            serde_json::json!({
                "inline_keyboard": [
                    [btn("🔄 Refresh", "tr:pnl"), btn("← Back", "tr:r")],
                    [btn("🗑 Close", "x:close")],
                ]
            }),
        )
    }

    pub(crate) async fn page_calendar_reply(&self, user_id: i64) -> Reply {
        if self.profiles.get(user_id).map(|p| p.private_pnl).unwrap_or(false) {
            let view = self.cal_view(user_id);
            return Reply::page(
                format!(
                    "📅 PnL Calendar\n\nPrivate PNL is on. Totals are hidden.\nTurn it off in Settings to show this month.\n\n🕐 Last updated: {}",
                    Self::calendar_stamp()
                ),
                calendar_keyboard(&view),
            );
        }
        let view = self.cal_view(user_id);
        let fills = self.pnl.fills(user_id);
        let sol_usd = self.rpc.sol_usd().await.unwrap_or(150.0);
        let text = pnl::render_calendar(&fills, &view, sol_usd, &Self::calendar_stamp());
        Reply::page(text, calendar_keyboard(&view))
    }

    pub(crate) async fn apply_cal(&self, user_id: i64, op: CalOp) -> Reply {
        let mut view = self.cal_view(user_id);
        match op {
            CalOp::Prev => {
                let (y, m) = pnl::add_month(view.year, view.month, -1);
                view.year = y;
                view.month = m;
                view.recap = false;
            }
            CalOp::Next => {
                let (y, m) = pnl::add_month(view.year, view.month, 1);
                view.year = y;
                view.month = m;
                view.recap = false;
            }
            CalOp::Week(w) => {
                view.week = w.min(3);
                view.recap = false;
            }
            CalOp::Usd => view.usd = !view.usd,
            CalOp::Recap => view.recap = true,
            CalOp::Refresh => {}
        }
        self.set_cal_view(user_id, view);
        self.page_calendar_reply(user_id).await
    }

    pub(crate) fn page_orders(&self, user_id: i64) -> String {
        let p = self.profiles.get(user_id);
        let max = p.as_ref().map(|x| x.max_sol).unwrap_or_else(|| self.status.max_sol());
        let cd = p
            .as_ref()
            .map(|x| x.cooldown_ms)
            .unwrap_or(self.status.cooldown_ms);
        let mut lines = vec![
            "📝 Limit orders".into(),
            String::new(),
            "Profit/Loss and Fixed Mcap orders on your copy setups. They sell when the trigger hits.".into(),
            String::new(),
        ];
        let mut n = 0usize;
        if let Some(p) = p.as_ref() {
            for t in &p.tasks {
                for (i, o) in t.orders.iter().enumerate() {
                    n += 1;
                    lines.push(format!(
                        "{}. {} — {}\n└ sell {} · trigger {}",
                        n,
                        t.display_name(),
                        o.list_title(i + 1),
                        fmt_pct2(o.sell_amount_pct),
                        if o.mode == StrategyMode::FixedMcap {
                            format!("${}", trim_num(o.mcap_usd.max(o.sell_at_pct)))
                        } else {
                            fmt_pct2(o.pnl_trigger())
                        }
                    ));
                }
            }
        }
        if n == 0 {
            lines.push("No orders yet. Open a copy setup → Strategies → Create Limit Order.".into());
        }
        lines.push(String::new());
        lines.push(format!(
            "Copy size: {:.4} SOL · cooldown: {} ms",
            max, cd
        ));
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        lines.join("\n")
    }

    pub(crate) fn orders_keyboard(&self, user_id: i64) -> serde_json::Value {
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        if let Some(p) = self.profiles.get(user_id) {
            for (ti, t) in p.tasks.iter().enumerate() {
                for (i, o) in t.orders.iter().enumerate() {
                    if rows.len() >= 16 {
                        break;
                    }
                    rows.push(vec![btn(
                        &format!("{} · {}", t.display_name(), o.list_title(i + 1)),
                        &format!("g:{ti}:O:{}", o.id),
                    )]);
                }
            }
        }
        rows.push(vec![btn("🪞 Copy Trade", "p:copy")]);
        rows.push(vec![btn("← Back", "p:home"), btn("🗑 Close", "x:close")]);
        serde_json::json!({ "inline_keyboard": rows })
    }

    pub(crate) async fn page_orders_reply(&self, user_id: i64) -> Reply {
        Reply::page(self.page_orders(user_id), self.orders_keyboard(user_id))
    }

    pub(crate) async fn page_trade_wallets(&self, user_id: i64, chat_id: i64) -> Reply {
        let _ = self.profiles.ensure(user_id, chat_id);
        let _ = self.wallets.get_or_create(user_id);
        let all = self.wallets.wallets_for(user_id);
        let mask = self
            .profiles
            .get(user_id)
            .map(|p| p.trade_wallets)
            .unwrap_or(1);
        let mut lines = vec![
            "💰 Active wallets".into(),
            String::new(),
            "Toggled wallets are used together on CA buy, sell, and split.".into(),
            String::new(),
        ];
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        for (i, w) in all.iter().enumerate() {
            let on = mask & (1u8 << i) != 0;
            let bal = self.user_balance(w).await;
            lines.push(format!(
                "{} {} · {} · `{}`",
                on_dot(on),
                w.label,
                bal_label(bal),
                w.pubkey
            ));
            rows.push(vec![btn(
                &format!("{} {}", on_dot(on), w.label),
                &format!("tr:wa:{i}"),
            )]);
        }
        rows.push(vec![btn("← Back", "tr:r"), btn("🗑 Close", "x:close")]);
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        Reply::page(lines.join("\n"), serde_json::json!({ "inline_keyboard": rows }))
    }

    pub(crate) async fn page_bots_reply(&self, user_id: i64) -> Reply {
        let live = self.live.snapshot_user(user_id);
        let profile = self.profiles.get(user_id);
        let copy_on = profile.as_ref().map(|p| p.copy_enabled).unwrap_or(true);
        let tasks = profile.as_ref().map(|p| p.tasks.len()).unwrap_or(0);
        let active = profile
            .as_ref()
            .map(|p| p.tasks.iter().filter(|t| t.setup.active).count())
            .unwrap_or(0);
        let engine_on = self.engine_arc().is_some();
        let w = match self.wallets.get_or_create(user_id) {
            Ok(c) => format!("{} `{}`", c.wallet.label, c.wallet.pubkey),
            Err(e) => format!("wallet error: {e:#}"),
        };
        let text = format!(
            "🤖 Bots\n\n\
This instance: {BOT_NAME} (@{BOT_USERNAME})\n\
Engine: {}\n\
Copies: {}\n\
Setups: {active} active / {tasks} total\n\
Signals this session: {} copied · {} skipped\n\
Last watched slot: {}\n\
Payer: {w}\n\n\
Pause or start copies here. Open Copy Trade to edit setups.\n\
🕐 Last updated: {}",
            if engine_on { "🟢 running" } else { "🔴 not attached" },
            if copy_on { "🟢 on" } else { "⏸ paused" },
            live.copies,
            live.skips,
            live.last_slot,
            last_updated_stamp()
        );
        let pause = if copy_on {
            btn("⏹ Pause copies", "c:pause")
        } else {
            btn("▶️ Start copies", "c:start")
        };
        Reply::page(
            text,
            serde_json::json!({
                "inline_keyboard": [
                    [btn("🪞 Copy Trade", "p:copy"), btn("💼 Positions", "p:positions")],
                    [pause, btn("🔄 Refresh", "p:bots")],
                    [btn("⚙️ Settings", "p:settings")],
                    [btn("← Back", "p:home"), btn("🗑 Close", "x:close")],
                ]
            }),
        )
    }

    pub(crate) async fn page_pumpfun_reply(&self, user_id: i64) -> Reply {
        let live = self.live.snapshot_user(user_id);
        let n = self
            .profiles
            .get(user_id)
            .map(|p| p.watched.len())
            .unwrap_or(0);
        let w = self.ensure_wallet(user_id);
        let addr = w.map(|x| x.pubkey).unwrap_or_default();
        let mut lines = vec![
            "🎃 Pump.fun".into(),
            String::new(),
            "Copies from Pump.fun buys on wallets you watch.".into(),
            String::new(),
        ];
        if !addr.is_empty() {
            lines.push(format!("Payer: `{addr}`"));
        }
        lines.push(format!("Watched wallets: {n}"));
        lines.push(format!(
            "Session: {} copied · {} skipped",
            live.copies, live.skips
        ));
        lines.push(String::new());
        lines.push(recent_copies(&live, 5));
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        Reply::page(
            lines.join("\n"),
            serde_json::json!({
                "inline_keyboard": [
                    [btn("🔄 Refresh", "p:pumpfun"), btn("🪞 Copy Trade", "p:copy")],
                    [btn("← Back", "p:home"), btn("🗑 Close", "x:close")],
                ]
            }),
        )
    }

    pub(crate) async fn page_withdraw_reply(&self, user_id: i64, note: Option<&str>) -> Reply {
        let Some(w) = self.ensure_wallet(user_id) else {
            return Reply::page(
                "💸 Withdraw\n\nCould not create W1. Try /start.",
                withdraw_keyboard(),
            );
        };
        let bal = self.user_balance(&w).await.unwrap_or(0);
        let spend = crate::swap::spendable_lamports(bal);
        let mut lines = vec![
            "💸 Withdraw".into(),
            String::new(),
            format!("{}: `{}`", w.label, w.pubkey),
            format!("💰 Balance: {}", bal_label(Some(bal))),
            format!(
                "📤 Spendable: {:.4} SOL (rent + fee stay in the wallet)",
                spend as f64 / 1_000_000_000.0
            ),
            String::new(),
            "Choose how much SOL to send:".into(),
        ];
        if let Some(n) = note {
            lines.push(String::new());
            lines.push(n.into());
        }
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        Reply::page(lines.join("\n"), withdraw_keyboard())
    }

    pub(crate) fn set_withdraw_pct(&self, user_id: i64, pct: f64) {
        self.withdraw_pct.lock().expect("wd").insert(user_id, pct);
    }

    pub(crate) fn take_withdraw_pct(&self, user_id: i64) -> Option<f64> {
        self.withdraw_pct.lock().expect("wd").remove(&user_id)
    }

    pub(crate) fn prompt_withdraw_dest(&self, pct: f64) -> Reply {
        let label = if pct >= 99.999 {
            "Max".to_string()
        } else {
            format!("{}%", trim_num(pct))
        };
        Reply::page(
            format!("Withdraw {label} of spendable SOL.\n\nSend the Solana address that should receive it."),
            serde_json::json!({
                "inline_keyboard": [[btn("← Back", "p:withdraw")]]
            }),
        )
    }

    pub(crate) async fn apply_withdraw(&self, user_id: i64, op: WithdrawOp) -> Reply {
        match op {
            WithdrawOp::Custom => {
                self.set_awaiting(user_id, Some(AwaitKind::WithdrawPct));
                Reply::page(
                    "Send a withdraw percent (1–100), or Max as 100.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "p:withdraw")]]
                    }),
                )
            }
            WithdrawOp::Max => {
                self.set_withdraw_pct(user_id, 100.0);
                self.set_awaiting(user_id, Some(AwaitKind::WithdrawTo));
                self.prompt_withdraw_dest(100.0)
            }
            WithdrawOp::Pct(n) => {
                let pct = n as f64;
                self.set_withdraw_pct(user_id, pct);
                self.set_awaiting(user_id, Some(AwaitKind::WithdrawTo));
                self.prompt_withdraw_dest(pct)
            }
        }
    }

    pub(crate) async fn finish_withdraw_pct(
        &self,
        user_id: i64,
        username: Option<&str>,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send a withdraw percent (1–100).",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "p:withdraw")]]
                }),
            );
        };
        if n <= 0.0 || n > 100.0 {
            return Reply::page(
                "Need a percentage from 1 to 100.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "p:withdraw")]]
                }),
            );
        }
        self.set_withdraw_pct(user_id, n);
        self.set_awaiting(user_id, Some(AwaitKind::WithdrawTo));
        self.prompt_withdraw_dest(n)
    }

    pub(crate) async fn finish_withdraw_to(
        &self,
        user_id: i64,
        username: Option<&str>,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let addr = text.split_whitespace().next().unwrap_or("");
        if crate::config::parse_pubkey(addr).is_err() {
            return Reply::page(
                "That is not a Solana address. Send the destination wallet.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "p:withdraw")]]
                }),
            );
        }
        let Some(pct) = self.take_withdraw_pct(user_id) else {
            self.set_awaiting(user_id, None);
            return self.page_withdraw_reply(user_id, Some("Pick a percentage first.")).await;
        };
        self.set_awaiting(user_id, None);
        let note = match self.engine_arc() {
            Some(eng) => match eng.manual_withdraw(user_id, addr, pct).await {
                Ok(n) => format!("🟢 {n}"),
                Err(e) => format!("⚪️ withdraw failed: {e:#}"),
            },
            None => "⚪️ engine not ready".into(),
        };
        self.page_withdraw_reply(user_id, Some(&note)).await
    }

    pub(crate) async fn page_split_reply(&self, user_id: i64, note: Option<&str>) -> Reply {
        let Some(view) = self.trade_view(user_id) else {
            return Reply::text("Paste a contract address to split tokens.");
        };
        let hold_ui = if let Some(w) = self.ensure_wallet(user_id) {
            self.rpc
                .token_positions(&w.pubkey)
                .await
                .ok()
                .and_then(|p| p.into_iter().find(|t| t.mint == view.mint))
                .map(|t| t.ui_amount)
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let mut lines = vec![
            "🧬 Split tokens".into(),
            String::new(),
            format!("`{}`", view.mint),
            format!("Bag: {hold_ui}"),
            String::new(),
            "Choose how much of this token to send:".into(),
        ];
        if let Some(n) = note {
            lines.push(String::new());
            lines.push(n.into());
        }
        lines.push(String::new());
        lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
        Reply::page(lines.join("\n"), split_keyboard())
    }

    pub(crate) fn set_split_pct(&self, user_id: i64, pct: f64) {
        self.split_pct.lock().expect("split").insert(user_id, pct);
    }

    pub(crate) fn take_split_pct(&self, user_id: i64) -> Option<f64> {
        self.split_pct.lock().expect("split").remove(&user_id)
    }

    pub(crate) fn prompt_split_dest(&self, pct: f64) -> Reply {
        let label = if pct >= 99.999 {
            "Max".to_string()
        } else {
            format!("{}%", trim_num(pct))
        };
        Reply::page(
            format!("Split {label} of this token.\n\nSend the Solana address that should receive it."),
            serde_json::json!({
                "inline_keyboard": [[btn("← Back", "tr:sp")]]
            }),
        )
    }

    pub(crate) async fn apply_split(&self, user_id: i64, op: WithdrawOp) -> Reply {
        if self.trade_view(user_id).is_none() {
            return Reply::text("Paste a contract address to split tokens.");
        }
        match op {
            WithdrawOp::Custom => {
                self.set_awaiting(user_id, Some(AwaitKind::SplitPct));
                Reply::page(
                    "Send a split percent (1–100), or Max as 100.",
                    serde_json::json!({
                        "inline_keyboard": [[btn("← Back", "tr:sp")]]
                    }),
                )
            }
            WithdrawOp::Max => {
                self.set_split_pct(user_id, 100.0);
                self.set_awaiting(user_id, Some(AwaitKind::SplitTo));
                self.prompt_split_dest(100.0)
            }
            WithdrawOp::Pct(n) => {
                let pct = n as f64;
                self.set_split_pct(user_id, pct);
                self.set_awaiting(user_id, Some(AwaitKind::SplitTo));
                self.prompt_split_dest(pct)
            }
        }
    }

    pub(crate) async fn finish_split_pct(
        &self,
        user_id: i64,
        username: Option<&str>,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let Some(n) = parse_strat_number(text) else {
            return Reply::page(
                "Send a split percent (1–100).",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "tr:sp")]]
                }),
            );
        };
        if n <= 0.0 || n > 100.0 {
            return Reply::page(
                "Need a percentage from 1 to 100.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "tr:sp")]]
                }),
            );
        }
        self.set_split_pct(user_id, n);
        self.set_awaiting(user_id, Some(AwaitKind::SplitTo));
        self.prompt_split_dest(n)
    }

    pub(crate) async fn finish_split_to(
        &self,
        user_id: i64,
        username: Option<&str>,
        text: &str,
    ) -> Reply {
        if !self.allowed(user_id, username) {
            self.set_awaiting(user_id, None);
            return Self::deny_reply(user_id);
        }
        let addr = text.split_whitespace().next().unwrap_or("");
        if crate::config::parse_pubkey(addr).is_err() {
            return Reply::page(
                "That is not a Solana address. Send the destination wallet.",
                serde_json::json!({
                    "inline_keyboard": [[btn("← Back", "tr:sp")]]
                }),
            );
        }
        let Some(pct) = self.take_split_pct(user_id) else {
            self.set_awaiting(user_id, None);
            return self.page_split_reply(user_id, Some("Pick a percentage first.")).await;
        };
        let Some(mint) = self.trade_view(user_id).map(|v| v.mint) else {
            self.set_awaiting(user_id, None);
            return Reply::text("Paste a contract address to split tokens.");
        };
        self.set_awaiting(user_id, None);
        let note = match self.engine_arc() {
            Some(eng) => match eng.manual_split(user_id, &mint, addr, pct).await {
                Ok(n) => format!("🟢 {n}"),
                Err(e) => format!("⚪️ split failed: {e:#}"),
            },
            None => "⚪️ engine not ready".into(),
        };
        self.page_split_reply(user_id, Some(&note)).await
    }

    pub(crate) fn ensure_wallet(&self, user_id: i64) -> Option<UserWallet> {
        self.wallets.get_or_create(user_id).ok().map(|c| c.wallet)
    }

    pub(crate) fn ensure_pubkey(&self, user_id: i64) -> Option<String> {
        self.ensure_wallet(user_id).map(|w| w.pubkey)
    }

    pub(crate) async fn user_balance(&self, wallet: &UserWallet) -> Option<u64> {
        self.rpc
            .balances(&[wallet.pubkey.clone()])
            .await
            .ok()
            .and_then(|snap| snap.lamports.first().copied().flatten())
    }

    pub(crate) async fn rpc_line(&self, live: &LiveSnap) -> String {
        if !self.rpc.configured() {
            return format!(
                "rpc: not configured\nlast watched slot: {}",
                live.last_slot
            );
        }
        match self
            .rpc
            .balances(&[self.status.our_pubkey.clone()])
            .await
        {
            Ok(snap) => format!(
                "rpc: ok\nslot: {}\nlast watched slot: {}",
                snap.slot, live.last_slot
            ),
            Err(e) => format!("rpc: {e:#}\nlast watched slot: {}", live.last_slot),
        }
    }

    pub async fn broadcast(&self, text: &str) {
        let chats: Vec<i64> = self
            .chats
            .lock()
            .expect("chats")
            .iter()
            .copied()
            .collect();
        for chat in chats {
            if let Err(e) = self.send(chat, text).await {
                eprintln!("telegram send chat={chat}: {e:#}");
            }
        }
    }

    /// Alerts only people who watch this wallet and have copies on.
    pub async fn notify_watchers(&self, wallet: &str, text: &str) {
        let subscribed: HashSet<i64> = self.chats.lock().expect("chats").clone();
        for f in self.profiles.followers_of(wallet) {
            if !f.copy_enabled || f.chat_id == 0 {
                continue;
            }
            if !subscribed.contains(&f.chat_id) {
                continue;
            }
            if let Err(e) = self.send(f.chat_id, text).await {
                eprintln!("telegram send chat={}: {e:#}", f.chat_id);
            }
        }
    }

    pub async fn send(&self, chat_id: i64, text: &str) -> Result<()> {
        self.send_reply(chat_id, &Reply::text(text)).await.map(|_| ())
    }

    pub async fn send_reply(&self, chat_id: i64, reply: &Reply) -> Result<i64> {
        let url = format!("{API}/bot{}/sendMessage", self.token);
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": reply.text,
            "parse_mode": "Markdown",
            "disable_web_page_preview": true,
        });
        if let Some(kb) = &reply.keyboard {
            body["reply_markup"] = kb.clone();
        }
        let res = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("telegram sendMessage")?;
        let status = res.status();
        let raw = res.text().await.unwrap_or_default();
        if !status.is_success() && raw.to_ascii_lowercase().contains("parse") {
            body.as_object_mut().map(|o| o.remove("parse_mode"));
            let res = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .context("telegram sendMessage retry")?;
            let status = res.status();
            let raw = res.text().await.unwrap_or_default();
            anyhow::ensure!(status.is_success(), "sendMessage {status}: {raw}");
            let id = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v.get("result")?.get("message_id")?.as_i64())
                .unwrap_or(0);
            return Ok(id);
        }
        anyhow::ensure!(status.is_success(), "sendMessage {status}: {raw}");
        let id = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("result")?.get("message_id")?.as_i64())
            .unwrap_or(0);
        Ok(id)
    }

    pub(crate) async fn edit_reply(&self, chat_id: i64, message_id: i64, reply: &Reply) -> Result<()> {
        let url = format!("{API}/bot{}/editMessageText", self.token);
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": reply.text,
            "parse_mode": "Markdown",
            "disable_web_page_preview": true,
        });
        if let Some(kb) = &reply.keyboard {
            body["reply_markup"] = kb.clone();
        }
        let res = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("telegram editMessageText")?;
        let status = res.status();
        let raw = res.text().await.unwrap_or_default();
        let lower = raw.to_ascii_lowercase();
        if lower.contains("message is not modified") {
            return Ok(());
        }
        if !status.is_success() && lower.contains("parse") {
            body.as_object_mut().map(|o| o.remove("parse_mode"));
            let res = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .context("telegram editMessageText retry")?;
            let status = res.status();
            let raw = res.text().await.unwrap_or_default();
            if raw.to_ascii_lowercase().contains("message is not modified") {
                return Ok(());
            }
            anyhow::ensure!(status.is_success(), "editMessageText {status}: {raw}");
            return Ok(());
        }
        anyhow::ensure!(status.is_success(), "editMessageText {status}: {raw}");
        Ok(())
    }

    pub(crate) async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<()> {
        self.post_ok(
            "deleteMessage",
            serde_json::json!({ "chat_id": chat_id, "message_id": message_id }),
        )
        .await
    }

    pub(crate) async fn answer_callback(&self, id: &str) -> Result<()> {
        self.post_ok(
            "answerCallbackQuery",
            serde_json::json!({ "callback_query_id": id }),
        )
        .await
    }

    /// Puts the Bloom-style list under Telegram's Menu button.
    pub async fn install_menu(&self) -> Result<()> {
        let commands: Vec<serde_json::Value> = MENU
            .iter()
            .map(|(command, description)| {
                serde_json::json!({ "command": command, "description": description })
            })
            .collect();
        self.post_ok(
            "setMyCommands",
            serde_json::json!({ "commands": commands }),
        )
        .await?;
        self.post_ok(
            "setChatMenuButton",
            serde_json::json!({ "menu_button": { "type": "commands" } }),
        )
        .await?;
        // setMyName is rate-limited for ~24h. Do not call it on every restart.
        Ok(())
    }

    pub(crate) async fn post_ok(&self, method: &str, body: serde_json::Value) -> Result<()> {
        let url = format!("{API}/bot{}/{method}", self.token);
        let res = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("telegram {method}"))?;
        let status = res.status();
        let parsed: ApiOk = res.json().await.with_context(|| format!("telegram {method} json"))?;
        anyhow::ensure!(
            status.is_success() && parsed.ok,
            "{method} {status}: {:?}",
            parsed.description
        );
        Ok(())
    }

    pub async fn poll_commands(&self) -> Result<()> {
        let menu = self.clone();
        tokio::spawn(async move {
            match menu.install_menu().await {
                Ok(()) => eprintln!("telegram menu installed ({} commands)", MENU.len()),
                Err(e) => eprintln!("telegram menu: {e:#}"),
            }
        });
        eprintln!("telegram poll started");
        let mut offset: i64 = 0;
        loop {
            match self.get_updates(offset).await {
                Ok(updates) => {
                    for u in updates {
                        offset = offset.max(u.update_id + 1);
                        if let Some(cb) = u.callback_query {
                            if let Err(e) = self.handle_callback(cb).await {
                                eprintln!("telegram callback: {e:#}");
                            }
                            continue;
                        }
                        if let Some(m) = u.message {
                            let Some(user) = m.from else {
                                continue;
                            };
                            let Some(text) = m.text else {
                                continue;
                            };
                            let user_id = user.id;
                            let username = user.username;
                            let chat_id = m.chat.id;
                            let user_msg_id = m.message_id;
                            let answering = self.awaiting_kind(user_id).is_some()
                                && parse_command(&text).is_none();
                            let prompt_id = self.prompt_message(user_id);
                            if let Some(reply) = self
                                .dispatch_text(user_id, username.as_deref(), chat_id, &text)
                                .await
                            {
                                let _ = self.delete_message(chat_id, user_msg_id).await;
                                if answering {
                                    if let Some(pid) = prompt_id {
                                        if self.edit_reply(chat_id, pid, &reply).await.is_err() {
                                            match self.send_reply(chat_id, &reply).await {
                                                Ok(nid) => {
                                                    let _ = self.delete_message(chat_id, pid).await;
                                                    self.forget_prompt(user_id);
                                                    if self.awaiting_kind(user_id).is_some() {
                                                        self.remember_prompt(user_id, nid);
                                                    } else {
                                                        self.remember_bot_msg(user_id, nid);
                                                    }
                                                }
                                                Err(e) => eprintln!("telegram reply: {e:#}"),
                                            }
                                        } else if self.awaiting_kind(user_id).is_some() {
                                            self.remember_prompt(user_id, pid);
                                        } else {
                                            self.forget_prompt(user_id);
                                            self.remember_bot_msg(user_id, pid);
                                        }
                                    } else {
                                        match self.send_reply(chat_id, &reply).await {
                                            Ok(nid) => {
                                                if self.awaiting_kind(user_id).is_some() {
                                                    self.remember_prompt(user_id, nid);
                                                } else {
                                                    self.remember_bot_msg(user_id, nid);
                                                }
                                            }
                                            Err(e) => eprintln!("telegram reply: {e:#}"),
                                        }
                                    }
                                } else {
                                    if let Some(pid) = prompt_id {
                                        let _ = self.delete_message(chat_id, pid).await;
                                        self.forget_prompt(user_id);
                                        self.forget_bot_msg(user_id, pid);
                                    }
                                    if let Some(prev) = self.last_bot_msg(user_id) {
                                        if Some(prev) != prompt_id {
                                            let _ = self.delete_message(chat_id, prev).await;
                                            self.forget_bot_msg(user_id, prev);
                                        }
                                    }
                                    match self.send_reply(chat_id, &reply).await {
                                        Ok(nid) => {
                                            if self.awaiting_kind(user_id).is_some() {
                                                self.remember_prompt(user_id, nid);
                                            } else {
                                                self.remember_bot_msg(user_id, nid);
                                            }
                                        }
                                        Err(e) => eprintln!("telegram reply: {e:#}"),
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("telegram getUpdates: {e:#}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    pub(crate) async fn handle_callback(&self, cb: CallbackQuery) -> Result<()> {
        let _ = self.answer_callback(&cb.id).await;
        let user_id = cb.from.id;
        let username = cb.from.username.as_deref();
        let Some(msg) = cb.message else {
            return Ok(());
        };
        let chat_id = msg.chat.id;
        let Some(data) = cb.data.as_deref() else {
            return Ok(());
        };
        let Some(action) = parse_callback(data) else {
            return Ok(());
        };
        if !matches!(
            &action,
            CallbackAction::CopyAdd
                | CallbackAction::CopyMass
                | CallbackAction::QuoteUnit { .. }
                | CallbackAction::StratPrompt { .. }
        ) {
            self.set_awaiting(user_id, None);
        }
        let remember_bot = !matches!(
            &action,
            CallbackAction::Close | CallbackAction::Trade(TradeOp::Hide)
        );
        match action {
            CallbackAction::Close => {
                self.forget_prompt(user_id);
                self.forget_bot_msg(user_id, msg.message_id);
                if self.delete_message(chat_id, msg.message_id).await.is_err() {
                    let closed = Reply::page(
                        "🗑 Closed.",
                        serde_json::json!({ "inline_keyboard": [] }),
                    );
                    let _ = self.edit_reply(chat_id, msg.message_id, &closed).await;
                }
            }
            CallbackAction::PosUsd => {
                let _ = self.profiles.ensure(user_id, chat_id);
                let _ = self
                    .profiles
                    .patch_profile(user_id, chat_id, |p| p.positions_usd = !p.positions_usd);
                let reply = self.page_positions_reply(user_id).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::OpenTrade(mint) => {
                let reply = self.open_trade_for_mint(user_id, chat_id, mint).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::ToggleCopy => {
                let on = self
                    .profiles
                    .get(user_id)
                    .map(|p| p.copy_enabled)
                    .unwrap_or(true);
                let _ = self
                    .reply_for_args(
                        user_id,
                        username,
                        chat_id,
                        if on { Command::CopyOff } else { Command::CopyOn },
                        "",
                    )
                    .await;
                let merged = self.settings_reply(user_id);
                if let Err(e) = self.edit_reply(chat_id, msg.message_id, &merged).await {
                    self.send_reply(chat_id, &merged).await?;
                    let _ = e;
                }
            }
            CallbackAction::CopyAdd => {
                self.set_awaiting(user_id, Some(AwaitKind::Add));
                let reply = Reply::page(
                    add_config_prompt(),
                    serde_json::json!({ "inline_keyboard": [] }),
                );
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::CopyMass => {
                self.set_awaiting(user_id, Some(AwaitKind::Mass));
                let reply = Reply::page(
                    mass_create_prompt(),
                    serde_json::json!({ "inline_keyboard": [] }),
                );
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::CopyUpdateWallet => {
                let reply = self.page_tasks_wallet(user_id, None).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::CopyPickWallet(i) => {
                let reply = self.pick_tasks_wallet(user_id, i).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::CopyPauseAll
            | CallbackAction::CopyStartAll
            | CallbackAction::CopyDeleteAll => {
                let reply = self.apply_copy_bulk(user_id, chat_id, action).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::CopySetup(i) => {
                let reply = self.page_copy_setup(user_id, i).await;
                if let Err(e) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    eprintln!("telegram edit setup: {e:#}");
                    if let Err(e2) = self.send_reply(chat_id, &reply).await {
                        eprintln!("telegram send setup: {e2:#}");
                    }
                }
            }
            CallbackAction::StratList { .. }
            | CallbackAction::StratNew { .. }
            | CallbackAction::StratOpen { .. }
            | CallbackAction::StratDel { .. }
            | CallbackAction::StratSave { .. }
            | CallbackAction::StratMode { .. }
            | CallbackAction::StratToggleProt { .. }
            | CallbackAction::StratToggleHi { .. }
            | CallbackAction::StratPrompt { .. } => {
                let reply = self.apply_strat(user_id, chat_id, action).await;
                if let Err(e) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    eprintln!("telegram edit strat: {e:#}");
                    if let Err(e2) = self.send_reply(chat_id, &reply).await {
                        eprintln!("telegram send strat: {e2:#}");
                    }
                }
            }
            CallbackAction::SetupAct { idx, op } => {
                let reply = self.apply_setup_op(user_id, chat_id, idx, op).await;
                if let Err(e) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    eprintln!("telegram edit setup: {e:#}");
                    if let Err(e2) = self.send_reply(chat_id, &reply).await {
                        eprintln!("telegram send setup: {e2:#}");
                    }
                }
            }
            CallbackAction::QuoteUnit { idx, min, unit } => {
                let reply = self.apply_quote_unit(user_id, chat_id, idx, min, unit).await;
                if let Err(e) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    eprintln!("telegram edit quote: {e:#}");
                    if let Err(e2) = self.send_reply(chat_id, &reply).await {
                        eprintln!("telegram send quote: {e2:#}");
                    }
                }
            }
            CallbackAction::SetOp(op) => {
                let reply = self.apply_set_op(user_id, chat_id, op).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::Cal(op) => {
                let reply = self.apply_cal(user_id, op).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::Trade(TradeOp::Hide) => {
                self.forget_prompt(user_id);
                self.forget_bot_msg(user_id, msg.message_id);
                if self.delete_message(chat_id, msg.message_id).await.is_err() {
                    let hidden = Reply::page(
                        "👁 Hidden.",
                        serde_json::json!({ "inline_keyboard": [] }),
                    );
                    let _ = self.edit_reply(chat_id, msg.message_id, &hidden).await;
                }
            }
            CallbackAction::Trade(op) => {
                let reply = self.apply_trade(user_id, chat_id, op).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::Withdraw(op) => {
                let reply = self.apply_withdraw(user_id, op).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::Split(op) => {
                let reply = self.apply_split(user_id, op).await;
                if let Err(_) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    self.send_reply(chat_id, &reply).await?;
                }
            }
            CallbackAction::Page(cmd) => {
                let reply = self
                    .reply_for_args(user_id, username, chat_id, cmd, "")
                    .await;
                if let Err(e) = self.edit_reply(chat_id, msg.message_id, &reply).await {
                    eprintln!("telegram edit page: {e:#}");
                    self.send_reply(chat_id, &reply).await?;
                }
            }
        }
        if self.awaiting_kind(user_id).is_some() {
            self.remember_prompt(user_id, msg.message_id);
        } else if remember_bot {
            self.remember_bot_msg(user_id, msg.message_id);
        }
        Ok(())
    }

    pub(crate) async fn get_updates(&self, offset: i64) -> Result<Vec<Update>> {
        let url = format!("{API}/bot{}/getUpdates", self.token);
        let res = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "offset": offset,
                "timeout": 20,
                "allowed_updates": ["message", "callback_query"],
            }))
            .send()
            .await
            .context("telegram getUpdates")?;
        let status = res.status();
        let parsed: ApiResponse = res.json().await.context("telegram getUpdates json")?;
        anyhow::ensure!(
            status.is_success() && parsed.ok,
            "getUpdates {status}: {:?}",
            parsed.description
        );
        Ok(parsed.result)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiOk {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiResponse {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    result: Vec<Update>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Update {
    update_id: i64,
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CallbackQuery {
    id: String,
    from: User,
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Message {
    #[serde(default)]
    message_id: i64,
    chat: Chat,
    #[serde(default)]
    from: Option<User>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Chat {
    id: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct User {
    id: i64,
    #[serde(default)]
    username: Option<String>,
}

