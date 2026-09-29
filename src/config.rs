use crate::tip::TipLadder;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Deserialize)]
pub struct FileConfig {
    #[serde(default = "default_endpoint")]
    pub laserstream_endpoint: String,
    #[serde(default = "default_rpc")]
    pub rpc_url: String,
    /// `rpc` = Developer logsSubscribe + getTransaction. `laserstream` = Business Geyser.
    #[serde(default = "default_watch")]
    pub watch: String,
    #[serde(default)]
    pub wallets: Vec<String>,
    #[serde(default)]
    pub denylist_mints: Vec<String>,
    #[serde(default = "default_max_sol")]
    pub max_sol: f64,
    #[serde(default = "default_cooldown_ms")]
    pub cooldown_ms: u64,
    pub our_pubkey: String,
    #[serde(default = "default_tip_floor")]
    pub tip_floor_sol: f64,
    #[serde(default = "default_tip_mid")]
    pub tip_mid_sol: f64,
    #[serde(default = "default_tip_high")]
    pub tip_high_sol: f64,
    #[serde(default = "default_tip_max")]
    pub max_tip_sol: f64,
    /// Telegram user ids that may /start the bot. Friends not listed are rejected.
    #[serde(default)]
    pub telegram_allow_user_ids: Vec<i64>,
    /// Telegram @usernames (with or without @). Matched case-insensitively.
    #[serde(default)]
    pub telegram_allow_usernames: Vec<String>,
    /// Optional group/chat ids that receive alerts without each friend running /start.
    #[serde(default)]
    pub telegram_chat_ids: Vec<i64>,
    #[serde(default = "default_jito")]
    pub jito_block_engine: String,
}

fn default_jito() -> String {
    "https://mainnet.block-engine.jito.wtf/api/v1/bundles".into()
}

fn default_endpoint() -> String {
    "https://laserstream-mainnet-fra.helius-rpc.com".into()
}

fn default_rpc() -> String {
    String::new()
}

fn default_watch() -> String {
    "rpc".into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchKind {
    Rpc,
    Laserstream,
}

fn default_max_sol() -> f64 {
    0.05
}

fn default_cooldown_ms() -> u64 {
    2_000
}

fn default_tip_floor() -> f64 {
    0.001
}

fn default_tip_mid() -> f64 {
    0.003
}

fn default_tip_high() -> f64 {
    0.008
}

fn default_tip_max() -> f64 {
    0.02
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub watch: WatchKind,
    pub laserstream_endpoint: String,
    pub rpc_url: String,
    pub helius_api_key: Option<String>,
    pub wallets: HashSet<[u8; 32]>,
    pub denylist: HashSet<[u8; 32]>,
    pub max_sol_lamports: u64,
    pub cooldown_ms: u64,
    pub our_pubkey: [u8; 32],
    pub tips: TipLadder,
    pub telegram_bot_token: Option<String>,
    pub telegram_allow_user_ids: HashSet<i64>,
    pub telegram_allow_usernames: HashSet<String>,
    pub telegram_chat_ids: HashSet<i64>,
    pub jito_url: String,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let file: FileConfig = toml::from_str(&raw).context("parse config.toml")?;
        load_dotenv();
        Self::from_file(file)
    }

    pub fn from_file(file: FileConfig) -> Result<Self> {
        let mut wallets = HashSet::new();
        for w in &file.wallets {
            wallets.insert(parse_pubkey(w).with_context(|| format!("wallet {w}"))?);
        }
        let mut denylist = HashSet::new();
        for m in &file.denylist_mints {
            denylist.insert(parse_pubkey(m).with_context(|| format!("denylist {m}"))?);
        }
        let our_pubkey = parse_pubkey(&file.our_pubkey).context("our_pubkey")?;
        let max_sol_lamports = (file.max_sol * 1_000_000_000.0).round() as u64;
        let helius_api_key = std::env::var("HELIUS_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        let laserstream_endpoint = std::env::var("LASERSTREAM_ENDPOINT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or(file.laserstream_endpoint);
        let mut rpc_url = std::env::var("RPC_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or(file.rpc_url);
        if rpc_url.is_empty() {
            if let Some(key) = helius_api_key.as_ref() {
                rpc_url = helius_rpc_http(key);
            }
        }
        let watch_raw = std::env::var("WATCH")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or(file.watch);
        let watch = match watch_raw.to_ascii_lowercase().as_str() {
            "rpc" | "developer" | "ws" => WatchKind::Rpc,
            "laserstream" | "geyser" | "grpc" => WatchKind::Laserstream,
            other => anyhow::bail!("watch must be rpc or laserstream, got {other}"),
        };
        let telegram_bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        let (env_ids, env_names) = match std::env::var("TELEGRAM_ALLOW_USER_IDS")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(raw) => parse_allow_list(&raw)?,
            None => (HashSet::new(), HashSet::new()),
        };
        let extra_names = match std::env::var("TELEGRAM_ALLOW_USERNAMES")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(raw) => parse_allow_list(&raw)?.1,
            None => HashSet::new(),
        };
        let telegram_allow_user_ids = if env_ids.is_empty() && std::env::var("TELEGRAM_ALLOW_USER_IDS").ok().filter(|s| !s.is_empty()).is_none() {
            file.telegram_allow_user_ids.into_iter().collect()
        } else {
            env_ids
        };
        let mut telegram_allow_usernames: HashSet<String> = file
            .telegram_allow_usernames
            .into_iter()
            .map(|s| normalize_username(&s))
            .filter(|s| !s.is_empty())
            .collect();
        telegram_allow_usernames.extend(env_names);
        telegram_allow_usernames.extend(extra_names);
        let telegram_chat_ids = match std::env::var("TELEGRAM_CHAT_IDS")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(raw) => parse_allow_ids(&raw)?,
            None => file.telegram_chat_ids.into_iter().collect(),
        };
        if rpc_url.is_empty() {
            anyhow::bail!("need rpc_url in config.toml or RPC_URL / HELIUS_API_KEY in .env");
        }
        let jito_url = std::env::var("JITO_BLOCK_ENGINE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or(file.jito_block_engine);
        anyhow::ensure!(!jito_url.is_empty(), "need jito_block_engine or JITO_BLOCK_ENGINE");
        Ok(Self {
            watch,
            laserstream_endpoint,
            rpc_url,
            helius_api_key,
            wallets,
            denylist,
            max_sol_lamports,
            cooldown_ms: file.cooldown_ms,
            our_pubkey,
            tips: TipLadder::from_sol(
                file.tip_floor_sol,
                file.tip_mid_sol,
                file.tip_high_sol,
                file.max_tip_sol,
            ),
            telegram_bot_token,
            telegram_allow_user_ids,
            telegram_allow_usernames,
            telegram_chat_ids,
            jito_url,
        })
    }
}

/// KEY=value lines from `.env` if present. Does not override a var already in the process.
fn load_dotenv() {
    let Ok(text) = fs::read_to_string(".env") else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"');
        if k.is_empty() || std::env::var_os(k).is_some() {
            continue;
        }
        std::env::set_var(k, v);
    }
}

pub fn normalize_username(raw: &str) -> String {
    raw.trim().trim_start_matches('@').to_ascii_lowercase()
}

/// Numeric ids and/or @usernames, comma or space separated.
pub fn parse_allow_list(raw: &str) -> Result<(HashSet<i64>, HashSet<String>)> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for part in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p.chars().all(|c| c.is_ascii_digit() || c == '-') && p.parse::<i64>().is_ok() {
            ids.insert(p.parse().unwrap());
            continue;
        }
        let name = normalize_username(p);
        anyhow::ensure!(!name.is_empty(), "empty telegram username");
        names.insert(name);
    }
    Ok((ids, names))
}

pub fn parse_allow_ids(raw: &str) -> Result<HashSet<i64>> {
    Ok(parse_allow_list(raw)?.0)
}

pub fn parse_pubkey(s: &str) -> Result<[u8; 32]> {
    let bytes = bs58::decode(s.trim())
        .into_vec()
        .context("base58 decode")?;
    anyhow::ensure!(bytes.len() == 32, "pubkey must be 32 bytes");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn encode_pubkey(bytes: &[u8; 32]) -> String {
    bs58::encode(bytes).into_string()
}

pub fn helius_rpc_http(api_key: &str) -> String {
    format!("https://mainnet.helius-rpc.com/?api-key={api_key}")
}

pub fn rpc_ws_url(rpc_url: &str) -> String {
    if let Some(rest) = rpc_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if rpc_url.starts_with("wss://") || rpc_url.starts_with("ws://") {
        rpc_url.to_string()
    } else {
        format!("wss://{rpc_url}")
    }
}
