use copybot::build::{build_paper_message, build_tip_burst};
use copybot::config::AppConfig;
use copybot::decode::{classify_tx, decode_pump_buy};
use copybot::fixture::{pump_buy_tx, pump_sell_tx, unrelated_tx, spl_transfer_tx, OUR_WALLET, WALLET_A};
use copybot::build::tip_lamports_of;
use copybot::hot::{on_tx, HotPath};
use copybot::risk::{check, RiskConfig, RiskState};
use copybot::tip::TipLadder;
use std::collections::HashSet;

#[test]
fn decodes_pump_buy() {
    let tx = pump_buy_tx(50_000_000, 99);
    let buy = decode_pump_buy(&tx).expect("buy");
    assert_eq!(buy.wallet, WALLET_A);
    assert_eq!(buy.token_amount, 99);
    assert_eq!(buy.max_sol_lamports, 50_000_000);
}

#[test]
fn skips_sells_and_noise() {
    assert!(decode_pump_buy(&pump_sell_tx()).is_none());
    assert!(decode_pump_buy(&unrelated_tx()).is_none());
}

#[test]
fn classifies_sell() {
    let ev = classify_tx(&pump_sell_tx()).expect("sell");
    assert!(matches!(ev, copybot::types::TargetEvent::Sell { .. }));
}

#[test]
fn classifies_transfer() {
    let ev = classify_tx(&spl_transfer_tx()).expect("xfer");
    assert!(matches!(ev, copybot::types::TargetEvent::Transfer { .. }));
}

#[test]
fn risk_caps_and_cooldown() {
    let tx = pump_buy_tx(80_000_000, 1);
    let buy = decode_pump_buy(&tx).unwrap();
    let cfg = RiskConfig {
        max_sol_lamports: 50_000_000,
        cooldown_ms: 1_000,
    };
    let mut state = RiskState::new(HashSet::new());
    let copy = check(&mut state, &cfg, &buy, 10).unwrap();
    assert_eq!(copy, 50_000_000);
    assert_eq!(
        check(&mut state, &cfg, &buy, 11).unwrap_err(),
        copybot::risk::RiskReject::Cooldown
    );
    assert!(check(&mut state, &cfg, &buy, 10 + 1_000).is_ok());
}

#[test]
fn paper_on_tx_ready() {
    let tx = pump_buy_tx(50_000_000, 1_000);
    let mut watched = HashSet::new();
    watched.insert(WALLET_A);
    let mut path = HotPath::new(
        watched,
        HashSet::new(),
        OUR_WALLET,
        50_000_000,
        0,
        [0u8; 32],
        TipLadder::from_sol(0.001, 0.003, 0.008, 0.02),
    );
    let (ready, burst) = on_tx(&mut path, &tx, 1).expect("ready");
    let msg = &burst.messages[0];
    assert_eq!(ready.copy_sol_lamports, 50_000_000);
    assert!(ready.message_bytes > 0);
    assert!(!msg.instructions.is_empty());
    assert_eq!(msg.account_keys[0], OUR_WALLET);
}

#[test]
fn paper_build_rewrites_payer() {
    let tx = pump_buy_tx(10_000_000, 2);
    let buy = decode_pump_buy(&tx).unwrap();
    let msg = build_paper_message(&tx, &buy, &OUR_WALLET, 5_000_000, 1_000_000, [0u8; 32]).unwrap();
    assert_eq!(msg.account_keys[0], OUR_WALLET);
}

#[test]
fn hot_path_speed_bars() {
    let tx = pump_buy_tx(50_000_000, 1_000);
    let mut watched = HashSet::new();
    watched.insert(WALLET_A);
    let mut path = HotPath::new(
        watched,
        HashSet::new(),
        OUR_WALLET,
        50_000_000,
        0,
        [0u8; 32],
        TipLadder::from_sol(0.001, 0.003, 0.008, 0.02),
    );
    for _ in 0..200 {
        let _ = on_tx(&mut path, &tx, 1);
    }
    let mut decode_ns = Vec::with_capacity(2_000);
    for _ in 0..2_000 {
        let t = std::time::Instant::now();
        let _ = decode_pump_buy(&tx);
        decode_ns.push(t.elapsed().as_nanos() as u64);
    }
    decode_ns.sort_unstable();
    let decode_p50_us = decode_ns[decode_ns.len() / 2] / 1_000;
    assert!(
        decode_p50_us < 50,
        "decode p50 {decode_p50_us}us exceeds 50us"
    );

    let mut on_ns = Vec::with_capacity(2_000);
    for _ in 0..2_000 {
        let t = std::time::Instant::now();
        let _ = on_tx(&mut path, &tx, 1);
        on_ns.push(t.elapsed().as_nanos() as u64);
    }
    on_ns.sort_unstable();
    let on_p50_us = on_ns[on_ns.len() / 2] / 1_000;
    assert!(on_p50_us < 200, "on_tx p50 {on_p50_us}us exceeds 200us");
}

#[test]
fn config_live_needs_rpc() {
    match AppConfig::from_file(copybot::config::FileConfig {
        laserstream_endpoint: "https://laserstream-mainnet-fra.helius-rpc.com".into(),
        rpc_url: String::new(),
        watch: "rpc".into(),
        wallets: vec![],
        denylist_mints: vec![],
        max_sol: 0.05,
        cooldown_ms: 2000,
        our_pubkey: "So11111111111111111111111111111111111111112".into(),
        tip_floor_sol: 0.001,
        tip_mid_sol: 0.003,
        tip_high_sol: 0.008,
        max_tip_sol: 0.02,
        telegram_allow_user_ids: vec![],
        telegram_allow_usernames: vec![],
        telegram_chat_ids: vec![],
        jito_block_engine: "https://mainnet.block-engine.jito.wtf/api/v1/bundles".into(),
    }) {
        Err(e) => assert!(e.to_string().contains("rpc_url")),
        Ok(cfg) => assert!(!cfg.rpc_url.is_empty()),
    }
}

#[test]
fn config_live_ok() {
    let cfg = AppConfig::from_file(copybot::config::FileConfig {
        laserstream_endpoint: "https://laserstream-mainnet-fra.helius-rpc.com".into(),
        rpc_url: "https://mainnet.helius-rpc.com/?api-key=test".into(),
        watch: "rpc".into(),
        wallets: vec![],
        denylist_mints: vec![],
        max_sol: 0.05,
        cooldown_ms: 2000,
        our_pubkey: "So11111111111111111111111111111111111111112".into(),
        tip_floor_sol: 0.001,
        tip_mid_sol: 0.003,
        tip_high_sol: 0.008,
        max_tip_sol: 0.02,
        telegram_allow_user_ids: vec![],
        telegram_allow_usernames: vec![],
        telegram_chat_ids: vec![],
        jito_block_engine: "https://mainnet.block-engine.jito.wtf/api/v1/bundles".into(),
    })
    .unwrap();
    assert_eq!(cfg.max_sol_lamports, 50_000_000);
    assert_eq!(cfg.watch, copybot::config::WatchKind::Rpc);
    assert!(!cfg.jito_url.is_empty());
}

#[test]
fn rpc_get_transaction_json_feeds_hot_path() {
    use copybot::config::encode_pubkey;
    use copybot::rpc_watch::geyser_from_rpc_result;
    let tx = pump_buy_tx(50_000_000, 99);
    let keys: Vec<String> = tx.account_keys.iter().map(encode_pubkey).collect();
    let ixs: Vec<serde_json::Value> = tx
        .ixs
        .iter()
        .map(|ix| {
            serde_json::json!({
                "programIdIndex": ix.program_id_index,
                "accounts": ix.accounts,
                "data": bs58::encode(&ix.data).into_string(),
            })
        })
        .collect();
    let result = serde_json::json!({
        "slot": 42,
        "transaction": {
            "message": {
                "accountKeys": keys,
                "recentBlockhash": encode_pubkey(&[9u8; 32]),
                "instructions": ixs,
            }
        }
    });
    let ev = geyser_from_rpc_result(&result).expect("parse rpc tx");
    let buy = decode_pump_buy(&ev.raw).expect("pump buy");
    assert_eq!(buy.wallet, WALLET_A);
    assert_eq!(ev.slot, 42);
}

#[test]
fn rpc_logs_notification_skips_failed() {
    let ok = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "logsNotification",
        "params": {
            "result": {
                "context": { "slot": 7 },
                "value": { "signature": "sigok", "err": null, "logs": [] }
            }
        }
    });
    let fail = serde_json::json!({
        "method": "logsNotification",
        "params": {
            "result": {
                "context": { "slot": 7 },
                "value": { "signature": "sigbad", "err": { "InstructionError": [0, "Custom"] } }
            }
        }
    });
    let parsed = copybot::rpc_watch::parse_logs_notification(&ok).expect("ok logs");
    assert_eq!(parsed.signature, "sigok");
    assert_eq!(parsed.slot, 7);
    assert!(parsed.logs.is_empty());
    assert!(copybot::rpc_watch::parse_logs_notification(&fail).is_none());
}

#[test]
fn rpc_slot_verdict_bands() {
    assert_eq!(copybot::rpc_watch::slot_verdict_ms(40), "maybe_same_slot");
    assert_eq!(copybot::rpc_watch::slot_verdict_ms(200), "likely_next_slot");
    assert_eq!(copybot::rpc_watch::slot_verdict_ms(500), "two_plus_slots");
}

#[test]
fn three_rungs_have_distinct_tips() {
    let tx = pump_buy_tx(50_000_000, 1_000);
    let mut watched = HashSet::new();
    watched.insert(WALLET_A);
    let mut path = HotPath::new(
        watched,
        HashSet::new(),
        OUR_WALLET,
        50_000_000,
        0,
        [0u8; 32],
        TipLadder::from_sol(0.001, 0.003, 0.008, 0.02),
    );
    let (ready, burst) = on_tx(&mut path, &tx, 1).unwrap();
    assert_eq!(ready.tips_lamports, [1_000_000, 3_000_000, 8_000_000]);
    assert_eq!(burst.tips_lamports, ready.tips_lamports);
    assert_eq!(tip_lamports_of(&burst.messages[0]), Some(1_000_000));
    assert_eq!(tip_lamports_of(&burst.messages[1]), Some(3_000_000));
    assert_eq!(tip_lamports_of(&burst.messages[2]), Some(8_000_000));
    assert_ne!(burst.messages[0].instructions[0].data, burst.messages[1].instructions[0].data);
}

#[test]
fn tip_burst_stamps_one_skeleton() {
    let tx = pump_buy_tx(10_000_000, 2);
    let buy = decode_pump_buy(&tx).unwrap();
    let msgs = build_tip_burst(
        &tx,
        &buy,
        &OUR_WALLET,
        5_000_000,
        [1_000_000, 3_000_000, 8_000_000],
        [9u8; 32],
    )
    .unwrap();
    assert_eq!(tip_lamports_of(&msgs[0]), Some(1_000_000));
    assert_eq!(tip_lamports_of(&msgs[1]), Some(3_000_000));
    assert_eq!(tip_lamports_of(&msgs[2]), Some(8_000_000));
    assert_eq!(msgs[0].account_keys, msgs[2].account_keys);
    assert_eq!(msgs[2].recent_blockhash, [9u8; 32]);
}

#[test]
fn escalate_lifts_floor_above_observed_winner() {
    let mut ladder = TipLadder::from_sol(0.001, 0.003, 0.008, 0.05);
    ladder.escalate_above(10_000_000);
    assert!(ladder.floor > 10_000_000);
    assert!(ladder.mid > ladder.floor);
    assert!(ladder.high > ladder.mid);
    assert!(ladder.high <= ladder.max);
}

#[tokio::test]
async fn paper_dispatch_fires_three_together() {
    let burst = copybot::tip::TipBurst {
        tips_lamports: [1, 2, 3],
        messages: [(), (), ()],
    };
    let out = copybot::tip::paper_dispatch(&burst).await;
    assert_eq!(out, [1, 2, 3]);
}
