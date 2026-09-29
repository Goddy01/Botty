use crate::autotip::AutoTip;
use crate::config::{encode_pubkey, AppConfig};
use crate::decode::classify_tx;
use crate::filters::{check_buy, copy_sol_lamports, FilterSkip, TokenMeta};
use crate::jito::{encode_tx_b64, JitoClient};
use crate::jupiter;
use crate::live::{LiveBoard, MainnetEvent};
use crate::positions::PositionStore;
use crate::pnl::PnlStore;
use crate::profile::{CopySetup, CopyTask, FollowJob, LimitOrder, UserProfile, WatchBook};
use crate::strategy::{order_fired, sell_tokens, StratCtx};
use crate::protect::{
    authorities_revoked, creator_from_curve, expected_buy_tokens, expected_sell_sol,
    mcap_usd_from_curve, min_out_after_slip, parse_curve_reserves, parse_mint_authorities,
    sim_failed, sim_slip_rejected, simulate_tx, MintAuth,
};
use crate::rpc_read::RpcRead;
use crate::sender::HeliusSender;
use crate::swap::{
    bonding_curve_pda, build_copy_message, fee_sol_to_microlamports, pump_fun_swap_template,
    sign_legacy, sign_versioned_tx, sol_transfer_message, token_split_message, withdraw_lamports,
};
use crate::telegram::Hub;
use crate::tip::{sol_to_lamports, TipLadder};
use crate::types::{RawTx, TargetEvent, TokenCredit, Venue};
use crate::user_wallet::WalletStore;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Mutex};

const LAST_SWAP_PATH: &str = "data/last_swap.json";
const LAST_SWAP_MAX: usize = 256;
const META_TIMEOUT_MS: u64 = 120;
const LAND_WAIT_SECS: u64 = 45;

const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const USD1_MINT: &str = "USD1ttGY1N17NEEHLmELoaybftRBUSErhqYiQzvEmuB";

pub(crate) fn speed_line(detect_ms: Option<u64>, send_ms: u64) -> String {
    speed_line_ex(detect_ms, None, send_ms)
}

pub(crate) fn speed_line_ex(
    detect_ms: Option<u64>,
    quote_ms: Option<u64>,
    send_ms: u64,
) -> String {
    let clock = exec_clock();
    match (detect_ms, quote_ms) {
        (Some(d), Some(q)) => {
            format!("detect {d} ms · quote {q} ms · send {send_ms} ms · {clock}")
        }
        (Some(d), None) => format!("detect {d} ms · send {send_ms} ms · {clock}"),
        (None, Some(q)) => format!("quote {q} ms · send {send_ms} ms · {clock}"),
        (None, None) => format!("send {send_ms} ms · {clock}"),
    }
}

fn exec_clock() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let ms = dur.subsec_millis();
    format!(
        "{:02}:{:02}:{:02}.{:03} UTC",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        ms
    )
}

fn elapsed_ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

fn native_pool_copy(ev: &TargetEvent) -> bool {
    matches!(
        ev,
        TargetEvent::Buy {
            venue: Venue::PumpFun | Venue::PumpSwap | Venue::Raydium,
            ..
        } | TargetEvent::Sell {
            venue: Venue::PumpFun | Venue::PumpSwap | Venue::Raydium,
            ..
        }
    )
}

fn is_stable_mint(mint: &[u8; 32]) -> bool {
    matches!(
        encode_pubkey(mint).as_str(),
        USDC_MINT | USDT_MINT | USD1_MINT
    )
}

fn best_token_credit<'a>(credits: &'a [TokenCredit], owner: &[u8; 32]) -> Option<&'a TokenCredit> {
    credits
        .iter()
        .filter(|c| c.owner == *owner && !is_stable_mint(&c.mint))
        .max_by_key(|c| c.amount)
}

fn sol_spent_for(spent: &[([u8; 32], u64)], wallet: &[u8; 32]) -> u64 {
    spent
        .iter()
        .find(|(k, _)| k == wallet)
        .map(|(_, n)| *n)
        .unwrap_or(0)
}

fn meta_auth_known(meta: &TokenMeta) -> bool {
    meta.creator.is_some() || meta.mcap_usd.is_some() || meta.virtual_sol.is_some()
}

fn setup_needs_meta(setup: &CopySetup) -> bool {
    setup.min_mcap.is_some_and(|v| v > 0.0)
        || setup.max_mcap.is_some_and(|v| v > 0.0)
        || setup.min_token_age_s.is_some_and(|v| v > 0.0)
        || setup.max_token_age_s.is_some_and(|v| v > 0.0)
        || setup.buy_protection
        || setup.skip_deploys
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_swap_cache(path: &Path) -> HashMap<String, (RawTx, TargetEvent, [u8; 32])> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_swap_cache(path: &Path, map: &HashMap<String, (RawTx, TargetEvent, [u8; 32])>) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec(map) {
        let _ = std::fs::write(path, bytes);
    }
}

pub struct CopyEngine {
    pub rpc: RpcRead,
    pub http: reqwest::Client,
    pub book: Option<Arc<WatchBook>>,
    pub wallets: Arc<WalletStore>,
    pub positions: Arc<PositionStore>,
    pub pnl: Arc<PnlStore>,
    pub live: Arc<LiveBoard>,
    pub tg: Option<Hub>,
    pub jito: JitoClient,
    pub sender: HeliusSender,
    pub autotip: AutoTip,
    pub max_tip_lamports: u64,
    pub hash_rx: watch::Receiver<[u8; 32]>,
    pub sol_usd: Mutex<(u64, f64)>,
    pub last_swap: Mutex<HashMap<String, (RawTx, TargetEvent, [u8; 32])>>,
    last_swap_path: PathBuf,
    denylist: HashSet<[u8; 32]>,
    last_copy_ms: Mutex<HashMap<(i64, [u8; 32]), u64>>,
}

impl CopyEngine {
    pub fn from_parts(
        cfg: &AppConfig,
        http: reqwest::Client,
        rpc: RpcRead,
        book: Option<Arc<WatchBook>>,
        wallets: Arc<WalletStore>,
        live: Arc<LiveBoard>,
        tg: Option<Hub>,
        pnl: Arc<PnlStore>,
        hash_rx: watch::Receiver<[u8; 32]>,
    ) -> Result<Self> {
        let sender = HeliusSender::new(http.clone());
        sender.spawn_ping_loop();
        Ok(Self {
            rpc,
            http: http.clone(),
            book,
            wallets,
            positions: Arc::new(PositionStore::open("data/user_positions")?),
            pnl,
            live,
            tg,
            jito: JitoClient::new(http.clone(), cfg.jito_url.clone()),
            sender,
            autotip: AutoTip::new(http),
            max_tip_lamports: cfg.tips.max,
            hash_rx,
            sol_usd: Mutex::new((0, 150.0)),
            last_swap: Mutex::new(load_swap_cache(Path::new(LAST_SWAP_PATH))),
            last_swap_path: PathBuf::from(LAST_SWAP_PATH),
            denylist: cfg.denylist.clone(),
            last_copy_ms: Mutex::new(HashMap::new()),
        })
    }

    pub fn spawn_handle(
        self: &Arc<Self>,
        ev: crate::types::GeyserTx,
        signature: String,
        notify_to_tx_ms: u64,
        verdict: String,
    ) {
        let eng = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = eng
                .handle(&ev, &signature, notify_to_tx_ms, &verdict)
                .await
            {
                eprintln!("copy engine: {e:#}");
            }
        });
    }

    pub async fn handle(
        self: &Arc<Self>,
        g: &crate::types::GeyserTx,
        signature: &str,
        notify_to_tx_ms: u64,
        verdict: &str,
    ) -> Result<()> {
        let raw = &g.raw;
        let slot = g.slot;
        let blockhash = g.blockhash;
        let classified = classify_tx(raw);
        let mut ev = classified;
        let mut jobs = Vec::new();

        if let Some(book) = self.book.as_ref() {
            let mut seen = std::collections::HashSet::new();
            let mut owners: Vec<[u8; 32]> = g
                .token_credits
                .iter()
                .chain(g.token_debits.iter())
                .map(|c| c.owner)
                .collect();
            owners.sort_unstable();
            owners.dedup();
            for owner in owners {
                if !seen.insert(owner) {
                    continue;
                }
                let js = book.followers_matching(&encode_pubkey(&owner));
                if js.is_empty() {
                    continue;
                }
                jobs = js;
                if let Some(credit) = best_token_credit(&g.token_credits, &owner) {
                    let native = classified.as_ref().is_some_and(|c| {
                        c.wallet() == credit.owner && native_pool_copy(c)
                    });
                    ev = Some(if native {
                        classified.clone().expect("classified present")
                    } else {
                        eprintln!(
                            "copy any-venue buy wallet=`{}` mint=`{}` sig={signature}",
                            encode_pubkey(&credit.owner),
                            encode_pubkey(&credit.mint)
                        );
                        TargetEvent::Buy {
                            wallet: credit.owner,
                            mint: credit.mint,
                            token_amount: credit.amount,
                            sol_lamports: sol_spent_for(&g.sol_spent, &credit.owner),
                            ix_index: classified
                                .as_ref()
                                .filter(|c| c.wallet() == credit.owner)
                                .map(|c| c.ix_index())
                                .unwrap_or(0),
                            venue: Venue::Router,
                            is_deployer: false,
                        }
                    });
                } else if let Some(debit) = best_token_credit(&g.token_debits, &owner) {
                    let native = classified.as_ref().is_some_and(|c| {
                        matches!(c, TargetEvent::Sell { .. }) && c.wallet() == debit.owner
                    });
                    ev = Some(if native {
                        classified.clone().expect("classified present")
                    } else {
                        eprintln!(
                            "copy any-venue sell wallet=`{}` mint=`{}` sig={signature}",
                            encode_pubkey(&debit.owner),
                            encode_pubkey(&debit.mint)
                        );
                        TargetEvent::Sell {
                            wallet: debit.owner,
                            mint: debit.mint,
                            token_amount: debit.amount,
                            ix_index: classified
                                .as_ref()
                                .filter(|c| c.wallet() == debit.owner)
                                .map(|c| c.ix_index())
                                .unwrap_or(0),
                            venue: Venue::Router,
                        }
                    });
                } else {
                    eprintln!(
                        "copy skip: follower `{}` only stable/WSOL flow sig={signature}",
                        encode_pubkey(&owner)
                    );
                    jobs.clear();
                    continue;
                }
                break;
            }
        }
        if jobs.is_empty() {
            jobs = ev
                .as_ref()
                .and_then(|e| {
                    self.book
                        .as_ref()
                        .map(|b| b.followers_matching(&encode_pubkey(&e.wallet())))
                })
                .unwrap_or_default();
        }
        let Some(ev) = ev else {
            self.live.bump_slot(slot);
            eprintln!("copy skip: could not classify sig={signature}");
            return Ok(());
        };
        if jobs.is_empty() {
            self.live.bump_slot(slot);
            eprintln!(
                "copy skip: no active follower for `{}` sig={signature}",
                encode_pubkey(&ev.wallet())
            );
            return Ok(());
        }
        let empty_meta = TokenMeta {
            mint: ev.mint(),
            creator: None,
            mcap_usd: None,
            age_secs: None,
            mint_auth_revoked: false,
            freeze_auth_revoked: false,
            virtual_sol: None,
            virtual_token: None,
        };
        let sol_usd = self.sol_usd().await;
        let needs_meta = jobs.iter().any(|j| setup_needs_meta(&j.task.setup));
        let meta = if needs_meta {
            match tokio::time::timeout(
                Duration::from_millis(META_TIMEOUT_MS),
                self.token_meta(raw, &ev, sol_usd),
            )
            .await
            {
                Ok(Ok(m)) => m,
                _ => empty_meta,
            }
        } else {
            empty_meta
        };
        for job in jobs {
            match &ev {
                TargetEvent::Buy { .. } => {
                    self.handle_buy(raw, &ev, &meta, &job, sol_usd, slot, signature, blockhash, notify_to_tx_ms, verdict)
                        .await;
                }
                TargetEvent::Sell { mint, .. } => {
                    if job.task.setup.follow_sells {
                        self.handle_exit(
                            raw,
                            &ev,
                            &meta,
                            &job,
                            encode_pubkey(mint).as_str(),
                            blockhash,
                            "follow sell",
                        )
                        .await;
                    }
                }
                TargetEvent::Transfer { mint, .. } => {
                    if job.task.setup.sell_on_transfer {
                        self.handle_exit(
                            raw,
                            &ev,
                            &meta,
                            &job,
                            encode_pubkey(mint).as_str(),
                            blockhash,
                            "sell on transfer",
                        )
                        .await;
                    }
                }
                TargetEvent::Create { .. } => {}
            }
            if !matches!(ev, TargetEvent::Create { .. }) {
                self.eval_job_strategies(raw, &ev, &meta, &job, blockhash)
                    .await;
            }
        }
        {
            let mint = encode_pubkey(&ev.mint());
            self.remember_swap(mint, raw.clone(), ev, blockhash).await;
        }
        Ok(())
    }

    async fn remember_swap(
        &self,
        mint: String,
        raw: RawTx,
        ev: TargetEvent,
        blockhash: [u8; 32],
    ) {
        let snap;
        {
            let mut g = self.last_swap.lock().await;
            g.insert(mint, (raw, ev, blockhash));
            while g.len() > LAST_SWAP_MAX {
                if let Some(k) = g.keys().next().cloned() {
                    g.remove(&k);
                } else {
                    break;
                }
            }
            snap = g.clone();
        }
        let path = self.last_swap_path.clone();
        tokio::task::spawn_blocking(move || save_swap_cache(&path, &snap));
    }

    async fn handle_buy(
        self: &Arc<Self>,
        raw: &RawTx,
        ev: &TargetEvent,
        meta: &TokenMeta,
        job: &FollowJob,
        sol_usd: f64,
        slot: u64,
        signature: &str,
        blockhash: [u8; 32],
        notify_to_tx_ms: u64,
        verdict: &str,
    ) {
        let mint_s = encode_pubkey(&ev.mint());
        if self.denylist.contains(&ev.mint()) {
            self.record(
                job,
                slot,
                signature,
                false,
                encode_pubkey(&ev.wallet()),
                mint_s.clone(),
                0,
                notify_to_tx_ms,
                FilterSkip::Denylist.as_str(),
            );
            self.alert(
                job,
                &format!(
                    "skip `{mint_s}`: {}\n{}",
                    FilterSkip::Denylist.as_str(),
                    speed_line(Some(notify_to_tx_ms), 0)
                ),
            )
            .await;
            return;
        }
        if job.cooldown_ms > 0 {
            let key = (job.user_id, ev.wallet());
            let now = now_ms();
            let g = self.last_copy_ms.lock().await;
            if let Some(prev) = g.get(&key) {
                if now.saturating_sub(*prev) < job.cooldown_ms {
                    self.record(
                        job,
                        slot,
                        signature,
                        false,
                        encode_pubkey(&ev.wallet()),
                        mint_s.clone(),
                        0,
                        notify_to_tx_ms,
                        FilterSkip::Cooldown.as_str(),
                    );
                    drop(g);
                    self.alert(
                        job,
                        &format!(
                            "skip `{mint_s}`: {}\n{}",
                            FilterSkip::Cooldown.as_str(),
                            speed_line(Some(notify_to_tx_ms), 0)
                        ),
                    )
                    .await;
                    return;
                }
            }
        }
        let already = self
            .positions
            .already_copied(job.user_id, job.task_idx, &mint_s);
        match check_buy(&job.task.setup, ev, meta, already, sol_usd) {
            Err(skip) => {
                self.record(
                    job,
                    slot,
                    signature,
                    false,
                    encode_pubkey(&ev.wallet()),
                    mint_s.clone(),
                    0,
                    notify_to_tx_ms,
                    skip.as_str(),
                );
                self.alert(
                    job,
                    &format!(
                        "skip `{}`: {}\n{}",
                        mint_s,
                        skip.as_str(),
                        speed_line(Some(notify_to_tx_ms), 0)
                    ),
                )
                .await;
            }
            Ok(uncapped) => {
                let copy_sol = copy_sol_lamports(&job.task.setup, {
                    if let TargetEvent::Buy { sol_lamports, .. } = *ev {
                        sol_lamports
                    } else {
                        uncapped
                    }
                }, job.max_sol_lamports);
                if copy_sol == 0 {
                    self.alert(
                        job,
                        &format!(
                            "skip `{mint_s}`: buy size is 0 (set Buy Amount, or copy size)\n{}",
                            speed_line(Some(notify_to_tx_ms), 0)
                        ),
                    )
                    .await;
                    return;
                }
                let expected = expected_buy_tokens(
                    ev,
                    copy_sol,
                    meta.virtual_sol,
                    meta.virtual_token,
                );
                let min_tokens = min_out_after_slip(expected, job.task.setup.buy_slippage_pct);
                let t0 = Instant::now();
                match self
                    .execute_buy(
                        raw,
                        ev,
                        &job.task.setup,
                        job,
                        copy_sol,
                        min_tokens,
                        expected,
                        blockhash,
                        meta,
                    )
                    .await
                {
                    Ok((note, quote_ms, sig)) => {
                        let send_ms = elapsed_ms(t0).saturating_sub(quote_ms.unwrap_or(0));
                        {
                            let mut g = self.last_copy_ms.lock().await;
                            g.insert((job.user_id, ev.wallet()), now_ms());
                        }
                        self.record(
                            job,
                            slot,
                            signature,
                            true,
                            encode_pubkey(&ev.wallet()),
                            mint_s.clone(),
                            copy_sol,
                            notify_to_tx_ms,
                            verdict,
                        );
                        self.alert(
                            job,
                            &format!(
                                "copy buy submitted `{mint_s}` {note}\n{}",
                                speed_line_ex(Some(notify_to_tx_ms), quote_ms, send_ms)
                            ),
                        )
                        .await;
                        let eng = Arc::clone(self);
                        let job = job.clone();
                        let mint_s = mint_s.clone();
                        let held = min_tokens.max(expected).max(1);
                        let mcap = meta.mcap_usd;
                        tokio::spawn(async move {
                            match eng.rpc.wait_landed(&sig, LAND_WAIT_SECS).await {
                                Ok(()) => {
                                    let _ = eng.positions.record_buy(
                                        job.user_id,
                                        job.task_idx,
                                        &mint_s,
                                        held,
                                        mcap,
                                    );
                                    let _ = eng.pnl.record(
                                        job.user_id,
                                        -(copy_sol as f64 / 1_000_000_000.0),
                                        &mint_s,
                                    );
                                    eng.alert(
                                        &job,
                                        &format!("copy buy landed `{mint_s}` `{sig}`"),
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    eng.alert(
                                        &job,
                                        &format!("copy buy did not land `{mint_s}` `{sig}`: {e:#}"),
                                    )
                                    .await;
                                }
                            }
                        });
                    }
                    Err(e) => {
                        let send_ms = elapsed_ms(t0);
                        self.record(
                            job,
                            slot,
                            signature,
                            false,
                            encode_pubkey(&ev.wallet()),
                            mint_s.clone(),
                            0,
                            notify_to_tx_ms,
                            "exec fail",
                        );
                        self.alert(
                            job,
                            &format!(
                                "buy failed `{mint_s}`: {e:#}\n{}",
                                speed_line(Some(notify_to_tx_ms), send_ms)
                            ),
                        )
                        .await;
                    }
                }
            }
        }
    }

    async fn handle_exit(
        self: &Arc<Self>,
        raw: &RawTx,
        ev: &TargetEvent,
        meta: &TokenMeta,
        job: &FollowJob,
        mint: &str,
        blockhash: [u8; 32],
        reason: &str,
    ) {
        let mut hold = self.positions.holding(job.user_id, mint);
        if let Ok(w) = self.payer(job.user_id, job.wallet_idx) {
            if let Ok(pos) = self.rpc.token_positions(&w.pubkey).await {
                if let Some(p) = pos.into_iter().find(|p| p.mint == mint) {
                    hold = p.amount;
                }
            }
        }
        if hold == 0 {
            return;
        }
        let t0 = Instant::now();
        match self
            .execute_sell(raw, ev, &job.task.setup, job, hold, meta, blockhash, None)
            .await
        {
            Ok((note, sig, pnl_sol)) => {
                let send_ms = elapsed_ms(t0);
                self.alert(
                    job,
                    &format!(
                        "{reason} submitted `{mint}` {note}\n{}",
                        speed_line(None, send_ms)
                    ),
                )
                .await;
                self.spawn_confirm_sell(job.clone(), mint.to_string(), sig, hold, pnl_sol, true);
            }
            Err(e) => {
                let send_ms = elapsed_ms(t0);
                self.alert(
                    job,
                    &format!(
                        "sell failed `{mint}`: {e:#}\n{}",
                        speed_line(None, send_ms)
                    ),
                )
                .await;
            }
        }
    }

    fn spawn_confirm_sell(
        self: &Arc<Self>,
        job: FollowJob,
        mint: String,
        sig: String,
        tokens: u64,
        pnl_sol: f64,
        clear_all: bool,
    ) {
        let eng = Arc::clone(self);
        tokio::spawn(async move {
            match eng.rpc.wait_landed(&sig, LAND_WAIT_SECS).await {
                Ok(()) => {
                    if clear_all {
                        let _ = eng.positions.record_sell(job.user_id, &mint);
                    } else {
                        let _ = eng
                            .positions
                            .record_sell_amount(job.user_id, &mint, tokens);
                    }
                    let _ = eng.pnl.record(job.user_id, pnl_sol, &mint);
                    eng.alert(&job, &format!("sell landed `{mint}` `{sig}`"))
                        .await;
                }
                Err(e) => {
                    eng.alert(
                        &job,
                        &format!("sell did not land `{mint}` `{sig}`: {e:#}"),
                    )
                    .await;
                }
            }
        });
    }

    async fn execute_buy(
        &self,
        raw: &RawTx,
        ev: &TargetEvent,
        setup: &CopySetup,
        job: &FollowJob,
        copy_sol: u64,
        min_tokens: u64,
        _expected_tokens: u64,
        blockhash: [u8; 32],
        meta: &TokenMeta,
    ) -> Result<(String, Option<u64>, String)> {
        let w = self.payer(job.user_id, job.wallet_idx)?;
        let pk = crate::config::parse_pubkey(&w.pubkey)?;
        let seed = seed32(&w.secret_seed)?;
        if setup.buy_protection
            && meta_auth_known(meta)
            && !authorities_revoked(&MintAuth {
                mint_revoked: meta.mint_auth_revoked,
                freeze_revoked: meta.freeze_auth_revoked,
            })
        {
            anyhow::bail!("mint or freeze authority still set");
        }
        if !native_pool_copy(ev) {
            let out = encode_pubkey(&ev.mint());
            let (note, _, quote_ms, sig) = self
                .execute_jupiter(
                    job.user_id,
                    job.wallet_idx,
                    setup,
                    jupiter::wsol(),
                    &out,
                    copy_sol,
                    setup.buy_protection,
                    meta,
                )
                .await?;
            return Ok((note, Some(quote_ms), sig));
        }
        let tip = self.tip_for(setup, true).await;
        let fee = fee_sol_to_microlamports(setup.buy_fee_sol);
        let blockhash = self.cached_blockhash(blockhash);
        let msg = build_copy_message(
            raw,
            ev,
            &pk,
            copy_sol,
            0,
            min_tokens,
            0,
            tip,
            fee,
            blockhash,
            true,
        )
        .context("build buy")?;
        let rungs = TipLadder::rungs_around(tip, self.max_tip_lamports);
        let mut bundle = Vec::with_capacity(3);
        for t in rungs {
            let m = msg.with_tip(t, blockhash);
            let signed = sign_legacy(&m, &seed);
            bundle.push(encode_tx_b64(&signed));
        }
        if setup.buy_protection {
            let sim = simulate_tx(&self.http, &self.rpc.url_ref(), &bundle[0]).await?;
            if sim_failed(&sim) {
                anyhow::bail!("buy sim failed (slip {}%)", setup.buy_slippage_pct);
            }
            if sim_slip_rejected(&sim) {
                anyhow::bail!("buy tax/slip over {}%", setup.buy_slippage_pct);
            }
        }
        let (note, sig) = self.send_signed_bundle(&bundle).await?;
        Ok((note, None, sig))
    }

    async fn send_signed_b64(&self, b64: &str) -> Result<(String, String)> {
        self.send_signed_bundle(&[b64.to_string()]).await
    }

    async fn send_signed_bundle(&self, txs_b64: &[String]) -> Result<(String, String)> {
        anyhow::ensure!(!txs_b64.is_empty(), "empty bundle");
        let primary = txs_b64[0].clone();
        let sig = crate::jito::first_sig_b58(&primary).context("send produced no signature")?;
        for t in txs_b64 {
            self.jito.fire_bundle(std::slice::from_ref(t));
            self.sender.fire_tx(t);
        }
        if txs_b64.len() > 1 {
            self.jito.fire_bundle(txs_b64);
        }
        let rpc = self.rpc.clone();
        let primary_rpc = primary.clone();
        tokio::spawn(async move {
            let _ = rpc.send_b64_tx(&primary_rpc).await;
        });
        Ok((format!("sent `{sig}`"), sig))
    }

    async fn execute_jupiter(
        &self,
        user_id: i64,
        wallet_idx: Option<usize>,
        setup: &CopySetup,
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        protect: bool,
        meta: &TokenMeta,
    ) -> Result<(String, u64, u64, String)> {
        let w = self.payer(user_id, wallet_idx)?;
        let seed = seed32(&w.secret_seed)?;
        if protect
            && meta_auth_known(meta)
            && !authorities_revoked(&MintAuth {
                mint_revoked: meta.mint_auth_revoked,
                freeze_revoked: meta.freeze_auth_revoked,
            })
        {
            anyhow::bail!("mint or freeze authority still set");
        }
        let tip = self.tip_for(setup, input_mint == jupiter::wsol()).await;
        let fee = sol_to_lamports(if input_mint == jupiter::wsol() {
            setup.buy_fee_sol
        } else {
            setup.sell_fee_sol
        });
        let slip = if input_mint == jupiter::wsol() {
            setup.buy_slippage_pct
        } else {
            setup.sell_slippage_pct
        };
        let mut last = anyhow::anyhow!("jupiter swap");
        for _ in 0..2 {
            let built = jupiter::swap_tx(
                &self.http,
                &w.pubkey,
                input_mint,
                output_mint,
                amount,
                slip,
                tip,
                fee,
            )
            .await?;
            let mut raw = built.raw;
            sign_versioned_tx(&mut raw, &seed)?;
            let swap_b64 = encode_tx_b64(&raw);
            if protect {
                let sim = simulate_tx(&self.http, &self.rpc.url_ref(), &swap_b64).await?;
                if sim_failed(&sim) {
                    last = anyhow::anyhow!("swap sim failed");
                    continue;
                }
            }
            match self.send_signed_bundle(&[swap_b64]).await {
                Ok((note, sig)) => return Ok((note, built.out, built.quote_ms, sig)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Returns `(note, signature, expected_pnl_sol)`. Does not book positions or PnL.
    async fn execute_sell(
        &self,
        raw: &RawTx,
        ev: &TargetEvent,
        setup: &CopySetup,
        job: &FollowJob,
        tokens: u64,
        meta: &TokenMeta,
        blockhash: [u8; 32],
        order: Option<&LimitOrder>,
    ) -> Result<(String, String, f64)> {
        let w = self.payer(job.user_id, job.wallet_idx)?;
        let pk = crate::config::parse_pubkey(&w.pubkey)?;
        let seed = seed32(&w.secret_seed)?;
        let slip = order
            .map(|o| o.slippage_pct)
            .unwrap_or(setup.sell_slippage_pct)
            .clamp(0.0, 100.0);
        let fee_sol = order.map(|o| o.fee_sol).unwrap_or(setup.sell_fee_sol);
        let tip_sol = order.map(|o| o.tip_sol).unwrap_or(setup.sell_tip_sol);
        let protect = order
            .map(|o| o.sell_protection && !o.high_impact)
            .unwrap_or(setup.sell_protection);
        let mut tokens = tokens;
        if let Ok(pos) = self.rpc.token_positions(&w.pubkey).await {
            if let Some(p) = pos
                .into_iter()
                .find(|p| p.mint == encode_pubkey(&ev.mint()))
            {
                if p.amount > 0 {
                    tokens = p.amount;
                }
            }
        }
        anyhow::ensure!(tokens > 0, "no tokens to sell");
        let mint_s = encode_pubkey(&ev.mint());
        let tip = {
            let base = sol_to_lamports(tip_sol);
            if setup.auto_tip {
                self.autotip
                    .tip_lamports(base, self.max_tip_lamports)
                    .await
            } else {
                base.max(1).min(self.max_tip_lamports)
            }
        };
        let fee = fee_sol_to_microlamports(fee_sol);
        let blockhash = self.cached_blockhash(blockhash);
        if native_pool_copy(ev) {
            let expected_sol = expected_sell_sol(tokens, meta.virtual_sol, meta.virtual_token);
            let min_sol = min_out_after_slip(expected_sol, slip);
            let native = (|| -> Result<String> {
                let msg = build_copy_message(
                    raw,
                    ev,
                    &pk,
                    0,
                    tokens,
                    0,
                    min_sol,
                    tip,
                    fee,
                    blockhash,
                    false,
                )
                .context("build sell")?;
                let signed = sign_legacy(&msg, &seed);
                Ok(encode_tx_b64(&signed))
            })();
            match native {
                Ok(b64) => {
                    let sim_ok = if protect {
                        match simulate_tx(&self.http, &self.rpc.url_ref(), &b64).await {
                            Ok(sim) if sim_failed(&sim) => false,
                            Ok(sim) if sim_slip_rejected(&sim) => false,
                            Ok(_) => true,
                            Err(_) => false,
                        }
                    } else {
                        true
                    };
                    if sim_ok {
                        if let Ok((note, sig)) = self.send_signed_b64(&b64).await {
                            return Ok((
                                note,
                                sig,
                                expected_sol as f64 / 1_000_000_000.0,
                            ));
                        }
                    }
                }
                Err(e) => eprintln!("native sell build: {e:#}"),
            }
        }
        let (note, out_sol, _, sig) = self
            .execute_jupiter(
                job.user_id,
                job.wallet_idx,
                setup,
                &mint_s,
                jupiter::wsol(),
                tokens,
                protect,
                meta,
            )
            .await?;
        Ok((note, sig, out_sol as f64 / 1_000_000_000.0))
    }

    fn cached_blockhash(&self, fallback: [u8; 32]) -> [u8; 32] {
        let h = *self.hash_rx.borrow();
        if h != [0u8; 32] {
            h
        } else if fallback != [0u8; 32] {
            fallback
        } else {
            h
        }
    }

    async fn tip_for(&self, setup: &CopySetup, buy: bool) -> u64 {
        let base = sol_to_lamports(if buy {
            setup.buy_tip_sol
        } else {
            setup.sell_tip_sol
        });
        if setup.auto_tip {
            self.autotip.tip_lamports_cached(base, self.max_tip_lamports)
        } else {
            base.max(1).min(self.max_tip_lamports)
        }
    }

    async fn token_meta(&self, raw: &RawTx, ev: &TargetEvent, sol_usd: f64) -> Result<TokenMeta> {
        let mint = ev.mint();
        let mint_s = encode_pubkey(&mint);
        let mut meta = TokenMeta {
            mint,
            creator: None,
            mcap_usd: None,
            age_secs: None,
            mint_auth_revoked: false,
            freeze_auth_revoked: false,
            virtual_sol: None,
            virtual_token: None,
        };
        if !self.rpc.configured() {
            return Ok(meta);
        }
        if let Ok(data) = self.rpc.account_data(&mint_s).await {
            if let Some(auth) = parse_mint_authorities(&data) {
                meta.mint_auth_revoked = auth.mint_revoked;
                meta.freeze_auth_revoked = auth.freeze_revoked;
            }
        }
        if let Some(ix) = raw.ixs.get(ev.ix_index()) {
            if let Some(&bond_i) = ix.accounts.get(3) {
                if let Some(pk) = raw.account_keys.get(bond_i as usize) {
                    if let Ok(data) = self.rpc.account_data(&encode_pubkey(pk)).await {
                        if let Some(c) = parse_curve_reserves(&data) {
                            meta.virtual_sol = Some(c.virtual_sol);
                            meta.virtual_token = Some(c.virtual_token);
                        }
                        meta.mcap_usd = mcap_usd_from_curve(&data, sol_usd);
                        meta.creator = creator_from_curve(&data);
                    }
                }
            }
        }
        if let Ok(Some(block_time)) = self.rpc.oldest_sig_time(&mint_s).await {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            meta.age_secs = Some((now - block_time).max(0) as f64);
        }
        Ok(meta)
    }

    async fn token_meta_for_mint(&self, mint: [u8; 32], sol_usd: f64) -> TokenMeta {
        let mint_s = encode_pubkey(&mint);
        let mut meta = TokenMeta {
            mint,
            creator: None,
            mcap_usd: None,
            age_secs: None,
            mint_auth_revoked: false,
            freeze_auth_revoked: false,
            virtual_sol: None,
            virtual_token: None,
        };
        if !self.rpc.configured() {
            return meta;
        }
        if let Ok(data) = self.rpc.account_data(&mint_s).await {
            if let Some(auth) = parse_mint_authorities(&data) {
                meta.mint_auth_revoked = auth.mint_revoked;
                meta.freeze_auth_revoked = auth.freeze_revoked;
            }
        }
        let bond = encode_pubkey(&bonding_curve_pda(&mint));
        if let Ok(data) = self.rpc.account_data(&bond).await {
            if let Some(c) = parse_curve_reserves(&data) {
                meta.virtual_sol = Some(c.virtual_sol);
                meta.virtual_token = Some(c.virtual_token);
            }
            meta.mcap_usd = mcap_usd_from_curve(&data, sol_usd);
            meta.creator = creator_from_curve(&data);
        }
        meta
    }

    async fn eval_job_strategies(
        self: &Arc<Self>,
        raw: &RawTx,
        ev: &TargetEvent,
        meta: &TokenMeta,
        job: &FollowJob,
        blockhash: [u8; 32],
    ) {
        if job.task.orders.is_empty() {
            return;
        }
        let mint = encode_pubkey(&ev.mint());
        let hold = self.positions.holding(job.user_id, &mint);
        if hold == 0 {
            return;
        }
        if let Some(m) = meta.mcap_usd {
            let _ = self.positions.bump_peak(job.user_id, &mint, m);
        }
        let Some(lot) = self.positions.lot(job.user_id, &mint) else {
            return;
        };
        if lot.task_idx != job.task_idx && lot.task_idx != 0 {
            // still allow — same user bag
        }
        let ctx = StratCtx {
            mcap: meta.mcap_usd,
            lot,
            now_unix: now_unix(),
        };
        let mut orders = job.task.orders.clone();
        orders.sort_by(|a, b| a.pnl_trigger().partial_cmp(&b.pnl_trigger()).unwrap_or(std::cmp::Ordering::Equal));
        for order in &orders {
            if !order_fired(order, &ctx) {
                continue;
            }
            let qty = sell_tokens(hold, order.sell_amount_pct);
            if qty == 0 {
                continue;
            }
            let t0 = Instant::now();
            match self
                .execute_sell(
                    raw,
                    ev,
                    &job.task.setup,
                    job,
                    qty,
                    meta,
                    blockhash,
                    Some(order),
                )
                .await
            {
                Ok((note, sig, pnl_sol)) => {
                    let send_ms = elapsed_ms(t0);
                    self.alert(
                        job,
                        &format!(
                            "{} submitted `{mint}` {note}\n{}",
                            order.list_title(order.id.max(1) as usize),
                            speed_line(None, send_ms)
                        ),
                    )
                    .await;
                    self.spawn_confirm_sell(
                        job.clone(),
                        mint.clone(),
                        sig,
                        qty,
                        pnl_sol,
                        false,
                    );
                    break;
                }
                Err(e) => {
                    let send_ms = elapsed_ms(t0);
                    self.alert(
                        job,
                        &format!(
                            "strategy sell failed `{mint}`: {e:#}\n{}",
                            speed_line(None, send_ms)
                        ),
                    )
                    .await;
                }
            }
        }
    }

    pub async fn tick_strategies(self: &Arc<Self>) {
        let Some(book) = &self.book else {
            return;
        };
        let profiles = book.all_profiles();
        let templates = self.last_swap.lock().await.clone();
        let sol_usd = self.sol_usd().await;
        let mut meta_by_mint: HashMap<String, TokenMeta> = HashMap::new();
        for p in profiles {
            for (task_idx, task) in p.tasks.iter().enumerate() {
                if !task.setup.active || task.orders.is_empty() {
                    continue;
                }
                let user_pk = self
                    .wallets
                    .get(p.telegram_user_id)
                    .ok()
                    .flatten()
                    .and_then(|w| crate::config::parse_pubkey(&w.pubkey).ok())
                    .unwrap_or([0u8; 32]);
                let pos = self.positions.get(p.telegram_user_id);
                let mints: Vec<String> = pos.lots.keys().cloned().chain(pos.holdings.keys().cloned()).collect();
                for mint in mints {
                    let Some(lot) = self.positions.lot(p.telegram_user_id, &mint) else {
                        continue;
                    };
                    if lot.task_idx != task_idx && lot.amount > 0 && lot.task_idx != 0 {
                        continue;
                    }
                    let Ok(mint_pk) = crate::config::parse_pubkey(&mint) else {
                        continue;
                    };
                    let job = FollowJob {
                        user_id: p.telegram_user_id,
                        chat_id: p.chat_id,
                        task_idx,
                        task: task.clone(),
                        max_sol_lamports: p.max_sol_lamports(),
                        cooldown_ms: p.cooldown_ms,
                        wallet_idx: None,
                    };
                    let meta = if let Some(m) = meta_by_mint.get(&mint) {
                        m.clone()
                    } else {
                        let m = self.token_meta_for_mint(mint_pk, sol_usd).await;
                        meta_by_mint.insert(mint.clone(), m.clone());
                        m
                    };
                    let (raw, ev, blockhash) = if let Some(t) = templates.get(&mint).cloned() {
                        t
                    } else {
                        let creator = meta.creator.unwrap_or([0u8; 32]);
                        let (raw, ev) = pump_fun_swap_template(mint_pk, user_pk, creator);
                        let blockhash = self.rpc.latest_blockhash().await.unwrap_or([0u8; 32]);
                        self.remember_swap(mint.clone(), raw.clone(), ev, blockhash)
                            .await;
                        (raw, ev, blockhash)
                    };
                    if encode_pubkey(&ev.mint()) != mint {
                        continue;
                    }
                    self.eval_job_strategies(&raw, &ev, &meta, &job, blockhash)
                        .await;
                }
            }
        }
    }

    async fn sol_usd(&self) -> f64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        {
            let g = self.sol_usd.lock().await;
            if now.saturating_sub(g.0) < 5 {
                return g.1;
            }
        }
        let px = self.rpc.sol_usd().await.unwrap_or(150.0);
        *self.sol_usd.lock().await = (now, px);
        px
    }

    fn record(
        &self,
        job: &FollowJob,
        slot: u64,
        signature: &str,
        copied: bool,
        wallet: String,
        mint: String,
        sol_lamports: u64,
        notify_to_tx_ms: u64,
        verdict: &str,
    ) {
        self.live.record_user(
            job.user_id,
            MainnetEvent {
                slot,
                signature: signature.into(),
                copied,
                wallet,
                mint,
                sol_lamports,
                notify_to_tx_ms,
                verdict: verdict.into(),
            },
        );
    }

    async fn alert(&self, job: &FollowJob, text: &str) {
        if let Some(hub) = &self.tg {
            hub.notify_user(job.chat_id, text).await;
        }
    }

    fn payer(&self, user_id: i64, idx: Option<usize>) -> Result<crate::user_wallet::UserWallet> {
        match idx {
            Some(i) => self
                .wallets
                .get_at(user_id, i)?
                .context("no wallet at that slot"),
            None => self.wallets.get(user_id)?.context("no W1 wallet"),
        }
    }

    fn trade_idxs(&self, user_id: i64) -> Vec<usize> {
        let n = self.wallets.wallets_for(user_id).len();
        self.book
            .as_ref()
            .and_then(|b| b.get(user_id))
            .map(|p| p.trade_wallet_indices(n))
            .unwrap_or_else(|| if n == 0 { Vec::new() } else { vec![0] })
    }

    fn wallet_label(&self, user_id: i64, idx: usize) -> String {
        self.wallets
            .get_at(user_id, idx)
            .ok()
            .flatten()
            .map(|w| w.label)
            .unwrap_or_else(|| format!("W{}", idx + 1))
    }

    pub async fn manual_buy(
        &self,
        user_id: i64,
        chat_id: i64,
        mint: &str,
        sol_lamports: u64,
    ) -> Result<String> {
        let idxs = self.trade_idxs(user_id);
        anyhow::ensure!(!idxs.is_empty(), "no wallet");
        let mut notes = Vec::new();
        let mut any_ok = false;
        for i in idxs {
            let label = self.wallet_label(user_id, i);
            match self
                .manual_buy_on(user_id, chat_id, mint, sol_lamports, i)
                .await
            {
                Ok(n) => {
                    any_ok = true;
                    notes.push(format!("{label}: {n}"));
                }
                Err(e) => notes.push(format!("{label}: {e:#}")),
            }
        }
        anyhow::ensure!(any_ok, notes.join(" · "));
        Ok(notes.join("\n"))
    }

    async fn manual_buy_on(
        &self,
        user_id: i64,
        chat_id: i64,
        mint: &str,
        sol_lamports: u64,
        wallet_idx: usize,
    ) -> Result<String> {
        anyhow::ensure!(sol_lamports > 0, "buy size is 0");
        let t0 = Instant::now();
        let mint_pk = crate::config::parse_pubkey(mint)?;
        let w = self.payer(user_id, Some(wallet_idx))?;
        let user_pk = crate::config::parse_pubkey(&w.pubkey)?;
        let sol_usd = self.sol_usd().await;
        let meta = self.token_meta_for_mint(mint_pk, sol_usd).await;
        let creator = meta.creator.unwrap_or([0u8; 32]);
        let (raw, ev) = pump_fun_swap_template(mint_pk, user_pk, creator);
        let blockhash = self.rpc.latest_blockhash().await.unwrap_or([0u8; 32]);
        self.remember_swap(mint.to_string(), raw.clone(), ev, blockhash)
            .await;
        let job = self.manual_job(user_id, chat_id, mint, Some(wallet_idx));
        let expected = expected_buy_tokens(&ev, sol_lamports, meta.virtual_sol, meta.virtual_token);
        if expected == 0 {
            let (note, out, quote_ms, _) = self
                .execute_jupiter(
                    user_id,
                    Some(wallet_idx),
                    &job.task.setup,
                    jupiter::wsol(),
                    mint,
                    sol_lamports,
                    job.task.setup.buy_protection,
                    &meta,
                )
                .await?;
            let held = out.max(1);
            let _ = self
                .positions
                .record_buy(user_id, 0, mint, held, meta.mcap_usd);
            let _ = self
                .pnl
                .record(user_id, -(sol_lamports as f64 / 1_000_000_000.0), mint);
            return Ok(format!(
                "buy {} SOL — {note}\n{}",
                sol_lamports as f64 / 1e9,
                speed_line_ex(None, Some(quote_ms), elapsed_ms(t0).saturating_sub(quote_ms))
            ));
        }
        let min_tokens = min_out_after_slip(expected, job.task.setup.buy_slippage_pct);
        let (note, quote_ms, _) = self
            .execute_buy(
                &raw,
                &ev,
                &job.task.setup,
                &job,
                sol_lamports,
                min_tokens,
                expected,
                blockhash,
                &meta,
            )
            .await?;
        let held = min_tokens.max(expected).max(1);
        let _ = self
            .positions
            .record_buy(user_id, 0, mint, held, meta.mcap_usd);
        let _ = self
            .pnl
            .record(user_id, -(sol_lamports as f64 / 1_000_000_000.0), mint);
        Ok(format!(
            "buy {} SOL — {note}\n{}",
            sol_lamports as f64 / 1e9,
            speed_line_ex(None, quote_ms, elapsed_ms(t0).saturating_sub(quote_ms.unwrap_or(0)))
        ))
    }

    pub async fn manual_sell(
        self: &Arc<Self>,
        user_id: i64,
        chat_id: i64,
        mint: &str,
        pct: f64,
    ) -> Result<String> {
        let idxs = self.trade_idxs(user_id);
        anyhow::ensure!(!idxs.is_empty(), "no wallet");
        let mut notes = Vec::new();
        let mut any_ok = false;
        for i in idxs {
            let label = self.wallet_label(user_id, i);
            match self.manual_sell_on(user_id, chat_id, mint, pct, i).await {
                Ok(n) => {
                    any_ok = true;
                    notes.push(format!("{label}: {n}"));
                }
                Err(e) => notes.push(format!("{label}: {e:#}")),
            }
        }
        anyhow::ensure!(any_ok, notes.join(" · "));
        Ok(notes.join("\n"))
    }

    async fn manual_sell_on(
        self: &Arc<Self>,
        user_id: i64,
        chat_id: i64,
        mint: &str,
        pct: f64,
        wallet_idx: usize,
    ) -> Result<String> {
        anyhow::ensure!((0.0..=100.0).contains(&pct) && pct > 0.0, "sell % must be 0–100");
        let t0 = Instant::now();
        let w = self.payer(user_id, Some(wallet_idx))?;
        let mut hold = 0u64;
        if let Ok(pos) = self.rpc.token_positions(&w.pubkey).await {
            if let Some(p) = pos.into_iter().find(|p| p.mint == mint) {
                hold = p.amount;
            }
        }
        if hold == 0 {
            hold = self.positions.holding(user_id, mint);
        }
        anyhow::ensure!(hold > 0, "no tokens to sell");
        let tokens = if pct >= 99.999 {
            hold
        } else {
            ((hold as f64) * (pct / 100.0)).floor() as u64
        };
        anyhow::ensure!(tokens > 0, "sell size is 0");
        let mint_pk = crate::config::parse_pubkey(mint)?;
        let user_pk = crate::config::parse_pubkey(&w.pubkey)?;
        let sol_usd = self.sol_usd().await;
        let meta = self.token_meta_for_mint(mint_pk, sol_usd).await;
        let job = self.manual_job(user_id, chat_id, mint, Some(wallet_idx));
        let clear_all = pct >= 99.999;
        if meta.virtual_sol.is_none() {
            let (note, out_sol, quote_ms, sig) = self
                .execute_jupiter(
                    user_id,
                    Some(wallet_idx),
                    &job.task.setup,
                    mint,
                    jupiter::wsol(),
                    tokens,
                    job.task.setup.sell_protection,
                    &meta,
                )
                .await?;
            let pnl_sol = out_sol as f64 / 1_000_000_000.0;
            self.spawn_confirm_sell(
                job,
                mint.to_string(),
                sig,
                tokens,
                pnl_sol,
                clear_all,
            );
            return Ok(format!(
                "sell {pct}% submitted — {note}\n{}",
                speed_line_ex(None, Some(quote_ms), elapsed_ms(t0).saturating_sub(quote_ms))
            ));
        }
        let creator = meta.creator.unwrap_or([0u8; 32]);
        let (raw, ev) = pump_fun_swap_template(mint_pk, user_pk, creator);
        let blockhash = self.rpc.latest_blockhash().await.unwrap_or([0u8; 32]);
        self.remember_swap(mint.to_string(), raw.clone(), ev, blockhash)
            .await;
        let (note, sig, pnl_sol) = self
            .execute_sell(
                &raw,
                &ev,
                &job.task.setup,
                &job,
                tokens,
                &meta,
                blockhash,
                None,
            )
            .await?;
        self.spawn_confirm_sell(
            job,
            mint.to_string(),
            sig,
            tokens,
            pnl_sol,
            clear_all,
        );
        Ok(format!(
            "sell {pct}% submitted — {note}\n{}",
            speed_line(None, elapsed_ms(t0))
        ))
    }

    fn manual_job(
        &self,
        user_id: i64,
        chat_id: i64,
        mint: &str,
        wallet_idx: Option<usize>,
    ) -> FollowJob {
        let p = self.book.as_ref().and_then(|b| b.get(user_id));
        FollowJob {
            user_id,
            chat_id,
            task_idx: 0,
            task: CopyTask {
                address: mint.into(),
                name: "trade".into(),
                setup: trade_setup(p.as_ref()),
                orders: Vec::new(),
            },
            max_sol_lamports: p.as_ref().map(|x| x.max_sol_lamports()).unwrap_or(u64::MAX),
            cooldown_ms: p.as_ref().map(|x| x.cooldown_ms).unwrap_or(0),
            wallet_idx,
        }
    }

    pub async fn manual_withdraw(&self, user_id: i64, dest: &str, pct: f64) -> Result<String> {
        anyhow::ensure!(pct > 0.0 && pct <= 100.0, "withdraw % must be 1–100");
        let dest_pk = crate::config::parse_pubkey(dest).context("destination address")?;
        let w = self.wallets.get(user_id)?.context("no W1 wallet")?;
        anyhow::ensure!(dest.trim() != w.pubkey, "destination is this wallet");
        let from_pk = crate::config::parse_pubkey(&w.pubkey)?;
        let seed = seed32(&w.secret_seed)?;
        let snap = self.rpc.balances(&[w.pubkey.clone()]).await?;
        let bal = snap
            .lamports
            .first()
            .copied()
            .flatten()
            .unwrap_or(0);
        let lamports = withdraw_lamports(bal, pct);
        anyhow::ensure!(lamports > 0, "not enough SOL (need rent + fee left in the wallet)");
        let blockhash = self.rpc.latest_blockhash().await?;
        let msg = sol_transfer_message(from_pk, dest_pk, lamports, blockhash);
        let signed = sign_legacy(&msg, &seed);
        let b64 = encode_tx_b64(&signed);
        let (note, _) = self.send_signed_b64(&b64).await?;
        Ok(format!(
            "sent {:.4} SOL to `{dest}` — {note}",
            lamports as f64 / 1_000_000_000.0
        ))
    }

    pub async fn manual_split(
        &self,
        user_id: i64,
        mint: &str,
        dest: &str,
        pct: f64,
    ) -> Result<String> {
        let idxs = self.trade_idxs(user_id);
        anyhow::ensure!(!idxs.is_empty(), "no wallet");
        let mut notes = Vec::new();
        let mut any_ok = false;
        for i in idxs {
            let label = self.wallet_label(user_id, i);
            match self.manual_split_on(user_id, mint, dest, pct, i).await {
                Ok(n) => {
                    any_ok = true;
                    notes.push(format!("{label}: {n}"));
                }
                Err(e) => notes.push(format!("{label}: {e:#}")),
            }
        }
        anyhow::ensure!(any_ok, notes.join(" · "));
        Ok(notes.join("\n"))
    }

    async fn manual_split_on(
        &self,
        user_id: i64,
        mint: &str,
        dest: &str,
        pct: f64,
        wallet_idx: usize,
    ) -> Result<String> {
        anyhow::ensure!(pct > 0.0 && pct <= 100.0, "split % must be 1–100");
        let dest_pk = crate::config::parse_pubkey(dest).context("destination address")?;
        let mint_pk = crate::config::parse_pubkey(mint).context("mint")?;
        let w = self.payer(user_id, Some(wallet_idx))?;
        anyhow::ensure!(dest.trim() != w.pubkey, "destination is this wallet");
        let from_pk = crate::config::parse_pubkey(&w.pubkey)?;
        let seed = seed32(&w.secret_seed)?;
        let hold = self
            .rpc
            .token_positions(&w.pubkey)
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.mint == mint)
            .map(|p| p.amount)
            .unwrap_or(0);
        let qty = sell_tokens(hold, pct);
        anyhow::ensure!(qty > 0, "no tokens of this CA to split");
        let blockhash = self.rpc.latest_blockhash().await?;
        let msg = token_split_message(from_pk, dest_pk, mint_pk, qty, blockhash);
        let signed = sign_legacy(&msg, &seed);
        let b64 = encode_tx_b64(&signed);
        let (note, _) = self.send_signed_b64(&b64).await?;
        let _ = self.positions.record_sell_amount(user_id, mint, qty);
        Ok(format!("split {pct}% ({qty} tokens) to `{dest}` — {note}"))
    }
}

fn trade_setup(p: Option<&UserProfile>) -> CopySetup {
    let mut s = CopySetup::default();
    if let Some(p) = p {
        s.buy_slippage_pct = p.trade_slippage_pct;
        s.sell_slippage_pct = p.trade_slippage_pct.clamp(0.0, 100.0);
        s.buy_tip_sol = p.trade_tip_sol;
        s.sell_tip_sol = p.trade_tip_sol;
        s.auto_tip = p.trade_auto_tip;
        s.buy_protection = p.settings_buy_protection;
        s.sell_protection = p.settings_buy_protection;
    }
    s
}

fn seed32(b58: &str) -> Result<[u8; 32]> {
    let v = bs58::decode(b58).into_vec().context("seed b58")?;
    anyhow::ensure!(v.len() == 32, "seed len");
    let mut s = [0u8; 32];
    s.copy_from_slice(&v);
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::{speed_line, speed_line_ex};

    #[test]
    fn speed_line_shows_detect_and_send_ms() {
        let s = speed_line(Some(42), 18);
        assert!(s.contains("detect 42 ms"));
        assert!(s.contains("send 18 ms"));
        let q = speed_line_ex(Some(42), Some(55), 18);
        assert!(q.contains("quote 55 ms"));
        assert!(s.contains("UTC"));
        let manual = speed_line(None, 9);
        assert!(manual.contains("send 9 ms"));
        assert!(!manual.contains("detect"));
    }
}
