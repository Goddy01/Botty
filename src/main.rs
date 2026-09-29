use anyhow::Result;
use copybot::config::{AppConfig, WatchKind};
use copybot::engine::CopyEngine;
use copybot::live::LiveBoard;
use copybot::rpc_read::RpcRead;
use copybot::telegram::Hub;
use copybot::user_wallet::WalletStore;
use copybot::watch::{refresh_blockhash, subscribe_wallets};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("check") {
        return copybot::check::run().await;
    }
    let _instance = copybot::instance::acquire()?;
    let cfg = AppConfig::load("config.toml")?;
    println!(
        "live watch={:?} rpc={} jito={}",
        cfg.watch,
        !cfg.rpc_url.is_empty(),
        !cfg.jito_url.is_empty()
    );

    let (hash_tx, hash_rx) = watch::channel([0u8; 32]);
    let rpc_refresh = cfg.rpc_url.clone();
    tokio::spawn(async move {
        refresh_blockhash(rpc_refresh, hash_tx).await;
    });

    let live = Arc::new(LiveBoard::new());
    let tg = Hub::from_config(&cfg, live.clone())?;
    let book = tg.as_ref().map(|h| h.watch_book());
    if let Some(hub) = tg.clone() {
        tokio::spawn(async move {
            if let Err(e) = hub.poll_commands().await {
                eprintln!("telegram poll stopped: {e:#}");
            }
        });
        println!(
            "telegram on (allowlist={} ids + {} names)",
            cfg.telegram_allow_user_ids.len(),
            cfg.telegram_allow_usernames.len()
        );
        println!("go live: /start to create W1, deposit SOL, add a copy setup, turn Active on");
    } else {
        eprintln!("TELEGRAM_BOT_TOKEN missing — copy alerts and W1 via Telegram will not run");
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_nodelay(true)
        .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
        .build()?;
    let rpc = RpcRead::new(http.clone(), cfg.rpc_url.clone());
    let wallets = tg
        .as_ref()
        .map(|h| h.wallet_store())
        .unwrap_or_else(|| Arc::new(WalletStore::open("data/user_wallets").expect("wallets")));
    let pnl = tg.as_ref().map(|h| h.pnl_store()).unwrap_or_else(|| {
        Arc::new(copybot::pnl::PnlStore::open("data/user_pnl").expect("pnl"))
    });
    let engine = Arc::new(CopyEngine::from_parts(
        &cfg,
        http,
        rpc,
        book.clone(),
        wallets,
        live.clone(),
        tg.clone(),
        pnl,
        hash_rx.clone(),
    )?);
    if let Some(hub) = tg.as_ref() {
        hub.bind_engine(&engine);
    }
    {
        let eng = Arc::clone(&engine);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                eng.tick_strategies().await;
            }
        });
    }

    let start_wallets = book
        .as_ref()
        .map(|b| b.union())
        .unwrap_or_else(|| cfg.wallets.clone());

    let _ = copybot::decode::pump_program();

    match cfg.watch {
        WatchKind::Rpc => {
            println!(
                "rpc_ws watches={} (live copy engine)",
                start_wallets.len()
            );
            let on_det = |det: copybot::rpc_watch::RpcDetect| {
                let mut ev = det.ev;
                if ev.blockhash == [0u8; 32] {
                    ev.blockhash = *hash_rx.borrow();
                }
                let verdict = copybot::rpc_watch::slot_verdict_ms(det.notify_to_tx_ms);
                println!(
                    "detect sig={} slot={} notify_to_tx_ms={} verdict={}",
                    det.signature, ev.slot, det.notify_to_tx_ms, verdict
                );
                engine.spawn_handle(
                    ev,
                    det.signature,
                    det.notify_to_tx_ms,
                    verdict.into(),
                );
            };
            if let Some(book) = book.clone() {
                copybot::rpc_watch::subscribe_wallets_rpc_watch(
                    cfg.rpc_url.clone(),
                    book.subscribe(),
                    on_det,
                )
                .await?;
            } else {
                copybot::rpc_watch::subscribe_wallets_rpc(
                    cfg.rpc_url.clone(),
                    &cfg.wallets,
                    on_det,
                )
                .await?;
            }
        }
        WatchKind::Laserstream => {
            let key = cfg
                .helius_api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("laserstream needs HELIUS_API_KEY"))?;
            let endpoint = cfg.laserstream_endpoint.clone();
            let mut watched: HashSet<[u8; 32]> = start_wallets;
            loop {
                if let Some(b) = book.as_ref() {
                    watched = b.union();
                }
                if watched.is_empty() {
                    println!("LaserStream idle — waiting for /watch");
                    if let Some(b) = book.as_ref() {
                        let mut rx = b.subscribe();
                        rx.changed().await.ok();
                        continue;
                    }
                    anyhow::bail!("laserstream: no wallets in config.toml");
                }
                println!("LaserStream {} wallets={}", endpoint, watched.len());
                let mut rx = book.as_ref().map(|b| b.subscribe());
                let eng = engine.clone();
                let run = subscribe_wallets(endpoint.clone(), key.clone(), &watched, move |ev| {
                    eng.spawn_handle(ev, String::new(), 0, String::new());
                });
                if let Some(rx) = rx.as_mut() {
                    tokio::select! {
                        r = run => r?,
                        _ = rx.changed() => {
                            println!("LaserStream watch list changed — resubscribe");
                        }
                    }
                } else {
                    run.await?;
                    break;
                }
            }
        }
    }
    Ok(())
}
