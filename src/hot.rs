use crate::build::{build_tip_burst, paper_bytes_len, PaperMessage};
use crate::decode::decode_pump_buy;
use crate::risk::{check, RiskConfig, RiskState};
use crate::tip::{TipBurst, TipLadder};
use crate::types::RawTx;
use std::collections::HashSet;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct PaperReady {
    pub wallet: [u8; 32],
    pub mint: [u8; 32],
    pub copy_sol_lamports: u64,
    pub detect_to_ready_us: u64,
    pub message_bytes: usize,
    pub tips_lamports: [u64; 3],
}

pub struct HotPath {
    pub watched: HashSet<[u8; 32]>,
    pub our_pubkey: [u8; 32],
    pub risk: RiskState,
    pub risk_cfg: RiskConfig,
    pub blockhash: [u8; 32],
    pub tips: TipLadder,
}

impl HotPath {
    pub fn new(
        watched: HashSet<[u8; 32]>,
        denylist: HashSet<[u8; 32]>,
        our_pubkey: [u8; 32],
        max_sol_lamports: u64,
        cooldown_ms: u64,
        blockhash: [u8; 32],
        tips: TipLadder,
    ) -> Self {
        Self {
            watched,
            our_pubkey,
            risk: RiskState::new(denylist),
            risk_cfg: RiskConfig {
                max_sol_lamports,
                cooldown_ms,
            },
            blockhash,
            tips,
        }
    }
}

/// Detect → decode → risk → three tip rungs. No await.
#[inline]
pub fn on_tx(
    path: &mut HotPath,
    tx: &RawTx,
    now_ms: u64,
) -> Option<(PaperReady, TipBurst<PaperMessage>)> {
    let t0 = Instant::now();
    let buy = decode_pump_buy(tx)?;
    if !path.watched.contains(&buy.wallet) {
        return None;
    }
    let copy_sol = check(&mut path.risk, &path.risk_cfg, &buy, now_ms).ok()?;
    let rungs = path.tips.rungs();
    let messages = build_tip_burst(
        tx,
        &buy,
        &path.our_pubkey,
        copy_sol,
        rungs,
        path.blockhash,
    )?;
    let burst = TipBurst {
        tips_lamports: rungs,
        messages,
    };
    let detect_to_ready_us = t0.elapsed().as_micros() as u64;
    Some((
        PaperReady {
            wallet: buy.wallet,
            mint: buy.mint,
            copy_sol_lamports: copy_sol,
            detect_to_ready_us,
            message_bytes: paper_bytes_len(&burst.messages[2]),
            tips_lamports: rungs,
        },
        burst,
    ))
}

pub fn paper_alert(ready: &PaperReady, extra: Option<&str>) -> String {
    let sol = ready.copy_sol_lamports as f64 / 1_000_000_000.0;
    let tips: Vec<String> = ready
        .tips_lamports
        .iter()
        .map(|t| format!("{:.4}", *t as f64 / 1_000_000_000.0))
        .collect();
    let mut s = format!(
        "mainnet copy signal\nwallet: `{}`\nmint: `{}`\nsol: {sol:.4}\ntips: {} SOL\ndetect→ready: {} µs",
        crate::config::encode_pubkey(&ready.wallet),
        crate::config::encode_pubkey(&ready.mint),
        tips.join(" / "),
        ready.detect_to_ready_us
    );
    if let Some(x) = extra {
        if !x.is_empty() {
            s.push('\n');
            s.push_str(x);
        }
    }
    s
}
