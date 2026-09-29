use crate::config::{AppConfig, WatchKind};
use crate::profile::{ProfileDefaults, WatchBook};
use crate::rpc_read::{fmt_sol, RpcRead, MAINNET_GENESIS};
use crate::user_wallet::WalletStore;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub async fn run() -> Result<()> {
    let mut fail = 0usize;
    println!("live readiness check (run from the copybot folder)\n");

    let env_ok = Path::new(".env").exists();
    line(
        env_ok,
        "`.env` file",
        if env_ok {
            "present"
        } else {
            "missing — copy `.env.example` to `.env` and fill keys"
        },
    );
    if !env_ok {
        fail += 1;
    }

    let cfg = match AppConfig::load("config.toml") {
        Ok(c) => {
            line(true, "config.toml", "loaded");
            c
        }
        Err(e) => {
            line(false, "config.toml", &format!("{e:#}"));
            println!("\n{fail} blocking issue(s). Fix the FAIL lines and run: cargo run -- check");
            anyhow::bail!("not ready");
        }
    };

    let helius = cfg.helius_api_key.is_some();
    line(
        helius || !cfg.rpc_url.is_empty(),
        "RPC",
        if helius {
            "HELIUS_API_KEY set"
        } else {
            "using rpc_url / RPC_URL"
        },
    );

    match cfg.watch {
        WatchKind::Rpc => line(true, "watch", "rpc (logsSubscribe)"),
        WatchKind::Laserstream => {
            let ok = cfg.helius_api_key.is_some();
            line(
                ok,
                "watch",
                if ok {
                    "laserstream — needs `cargo run --release --features laserstream`"
                } else {
                    "laserstream but HELIUS_API_KEY missing"
                },
            );
            if !ok {
                fail += 1;
            }
        }
    }

    let token_ok = cfg.telegram_bot_token.is_some();
    line(
        token_ok,
        "TELEGRAM_BOT_TOKEN",
        if token_ok {
            "set"
        } else {
            "missing — bot cannot /start or create W1"
        },
    );
    if !token_ok {
        fail += 1;
    }

    let allow = cfg.telegram_allow_user_ids.len() + cfg.telegram_allow_usernames.len();
    line(
        allow > 0,
        "Telegram allowlist",
        &format!(
            "{} id(s), {} @name(s)",
            cfg.telegram_allow_user_ids.len(),
            cfg.telegram_allow_usernames.len()
        ),
    );
    if allow == 0 {
        fail += 1;
    }

    line(!cfg.jito_url.is_empty(), "Jito URL", "set");

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()?;
    let rpc = RpcRead::new(http.clone(), cfg.rpc_url.clone());

    match rpc.genesis_hash().await {
        Ok(h) if h == MAINNET_GENESIS => line(true, "Solana cluster", "mainnet-beta"),
        Ok(h) => {
            line(false, "Solana cluster", &format!("not mainnet ({h})"));
            fail += 1;
        }
        Err(e) => {
            line(false, "RPC probe", &format!("{e:#}"));
            fail += 1;
        }
    }
    match rpc.latest_blockhash().await {
        Ok(_) => line(true, "getLatestBlockhash", "ok"),
        Err(e) => {
            line(false, "getLatestBlockhash", &format!("{e:#}"));
            fail += 1;
        }
    }

    match probe_jito_tips(&http).await {
        Ok(_) => line(true, "Jito tip floor API", "reachable"),
        Err(e) => line(false, "Jito tip floor API", &format!("{e:#} (AutoTip may fail)")),
    }

    if let Some(token) = cfg.telegram_bot_token.as_deref() {
        match telegram_get_me(&http, token).await {
            Ok(name) => line(true, "Telegram getMe", &format!("@{name}")),
            Err(e) => {
                line(false, "Telegram getMe", &format!("{e:#}"));
                fail += 1;
            }
        }
    }

    let wallets = WalletStore::open("data/user_wallets")?;
    let listed = wallets.list().unwrap_or_default();
    if listed.is_empty() {
        line(
            false,
            "W1 copy wallets",
            "none yet — an allowlisted user must /start the bot",
        );
    } else {
        line(true, "W1 copy wallets", &format!("{} on disk", listed.len()));
        for w in &listed {
            let bal = rpc.balances(&[w.pubkey.clone()]).await.ok();
            let lamports = bal.and_then(|s| s.lamports.first().copied().flatten());
            let funded = lamports.unwrap_or(0) > 0;
            line(
                funded,
                &format!("  user {} {}", w.telegram_user_id, w.label),
                &format!("`{}`  {}", w.pubkey, match lamports {
                    Some(n) => format!("{} SOL", fmt_sol(n)),
                    None => "balance unknown".into(),
                }),
            );
            if !funded {
                fail += 1;
            }
        }
    }

    let book = WatchBook::open(
        "data/user_profiles",
        ProfileDefaults::from_config(cfg.max_sol_lamports, cfg.cooldown_ms),
    )?;
    let union = book.union();
    let mut active = 0usize;
    // Walk users via union size + open each file is awkward; use followers isn't enough.
    // Count from disk:
    let profiles_dir = Path::new("data/user_profiles");
    if profiles_dir.exists() {
        for entry in std::fs::read_dir(profiles_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(p) = serde_json::from_str::<crate::profile::UserProfile>(&raw) else {
                continue;
            };
            for t in &p.tasks {
                if p.copy_enabled && t.setup.active {
                    active += 1;
                }
            }
        }
    }
    line(
        active > 0,
        "Active copy setups",
        &format!("{active} active, {} watched pubkey(s)", union.len()),
    );
    if active == 0 {
        println!("    add a target in Telegram, keep Active on, do not Pause All");
    }
    println!("    never run a second copybot — Telegram 409 means two getUpdates loops");

    println!();
    if fail == 0 {
        println!("READY — you can `cargo run --release` from this folder.");
    } else {
        println!("{fail} issue(s). Fix FAIL lines, then run: cargo run -- check");
        anyhow::bail!("not ready");
    }
    Ok(())
}

fn line(ok: bool, name: &str, detail: &str) {
    let mark = if ok { "OK  " } else { "FAIL" };
    println!("{mark}  {name}: {detail}");
}

async fn telegram_get_me(http: &reqwest::Client, token: &str) -> Result<String> {
    let url = format!("https://api.telegram.org/bot{token}/getMe");
    let v: Value = http.get(url).send().await?.json().await?;
    anyhow::ensure!(v.get("ok").and_then(|o| o.as_bool()) == Some(true), "{v}");
    Ok(v["result"]["username"]
        .as_str()
        .unwrap_or("unknown")
        .to_string())
}

async fn probe_jito_tips(http: &reqwest::Client) -> Result<()> {
    let v: Value = http
        .get("https://bundles.jito.wtf/api/v1/bundles/tip_floor")
        .send()
        .await?
        .json()
        .await?;
    anyhow::ensure!(v.is_array() || v.is_object(), "unexpected tip floor body");
    Ok(())
}
