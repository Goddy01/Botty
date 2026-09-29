use anyhow::{Context, Result};
use serde_json::{json, Value};

pub const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

#[derive(Clone, Debug)]
pub struct BalanceSnap {
    pub slot: u64,
    pub lamports: Vec<Option<u64>>,
}

#[derive(Clone, Debug)]
pub struct TokenPos {
    pub mint: String,
    pub amount: u64,
    pub decimals: u8,
    pub ui_amount: f64,
}

#[derive(Clone, Debug, Default)]
pub struct DexSnap {
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub dex: String,
    pub mcap_usd: Option<f64>,
    pub price_usd: Option<f64>,
    pub liq_usd: Option<f64>,
}

pub fn pretty_dex(id: &str, labels: &[String]) -> String {
    let id = id.to_ascii_lowercase();
    let base = match id.as_str() {
        "raydium" => "Raydium",
        "pumpfun" | "pump.fun" => "Pump.fun",
        "pumpswap" => "PumpSwap",
        "meteora" => "Meteora",
        "orca" => "Orca",
        "stonkfun" => "Stonk.fun",
        "raydium-launchlab" | "launchlab" => "Raydium LaunchLab",
        other => other,
    };
    if labels.is_empty() {
        base.to_string()
    } else {
        format!("{base} {}", labels.join(" "))
    }
}

fn merge_dex(into: &mut DexSnap, from: DexSnap) {
    if into.name.is_none() {
        into.name = from.name;
    }
    if into.symbol.is_none() {
        into.symbol = from.symbol;
    }
    if into.mcap_usd.is_none() {
        into.mcap_usd = from.mcap_usd;
    }
    if into.price_usd.is_none() {
        into.price_usd = from.price_usd;
    }
    if into.liq_usd.is_none() {
        into.liq_usd = from.liq_usd;
    }
    if into.dex.is_empty() || into.dex == "unknown" {
        if !from.dex.is_empty() && from.dex != "unknown" {
            into.dex = from.dex;
        }
    }
}

fn json_addr(tok: &Value) -> Option<&str> {
    tok.get("address")
        .and_then(|a| a.as_str())
        .or_else(|| tok.get("id").and_then(|a| a.as_str()))
}

pub fn parse_dex_token(v: &Value, mint: &str) -> Option<DexSnap> {
    let pairs = v.get("pairs")?.as_array()?;
    let pair = pairs.iter().find(|p| {
        if p.get("chainId").and_then(|c| c.as_str()) != Some("solana") {
            return false;
        }
        let base = p.get("baseToken").and_then(json_addr);
        let quote = p.get("quoteToken").and_then(json_addr);
        base == Some(mint) || quote == Some(mint)
    })?;
    let tok = pair.get("baseToken").filter(|b| json_addr(b) == Some(mint))
        .or_else(|| pair.get("quoteToken").filter(|q| json_addr(q) == Some(mint)))
        .or_else(|| pair.get("baseToken"))?;
    let labels: Vec<String> = pair
        .get("labels")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let dex_id = pair
        .get("dexId")
        .and_then(|d| d.as_str())
        .unwrap_or("unknown");
    Some(DexSnap {
        name: tok.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
        symbol: tok
            .get("symbol")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string()),
        dex: pretty_dex(dex_id, &labels),
        mcap_usd: pair
            .get("marketCap")
            .and_then(|n| n.as_f64())
            .or_else(|| pair.get("fdv").and_then(|n| n.as_f64())),
        price_usd: pair.get("priceUsd").and_then(|n| {
            n.as_f64()
                .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
        }),
        liq_usd: pair
            .get("liquidity")
            .and_then(|l| l.get("usd"))
            .and_then(|n| n.as_f64()),
    })
}

pub fn parse_jup_token(v: &Value, mint: &str) -> Option<DexSnap> {
    let row = if let Some(arr) = v.as_array() {
        arr.iter().find(|t| t.get("id").and_then(|i| i.as_str()) == Some(mint))?
    } else if v.get("id").and_then(|i| i.as_str()) == Some(mint) {
        v
    } else {
        return None;
    };
    let launch = row
        .get("launchpad")
        .and_then(|s| s.as_str())
        .or_else(|| row.get("metaLaunchpad").and_then(|s| s.as_str()))
        .unwrap_or("");
    let dex = if !launch.is_empty() {
        pretty_dex(launch, &[])
    } else if row.get("tokenProgram").and_then(|s| s.as_str()) == Some(TOKEN_2022) {
        "Token-2022".into()
    } else {
        "Solana".into()
    };
    Some(DexSnap {
        name: row.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
        symbol: row
            .get("symbol")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string()),
        dex,
        mcap_usd: row
            .get("mcap")
            .and_then(|n| n.as_f64())
            .or_else(|| row.get("fdv").and_then(|n| n.as_f64())),
        price_usd: row.get("usdPrice").and_then(|n| {
            n.as_f64()
                .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
        }),
        liq_usd: row.get("liquidity").and_then(|n| n.as_f64()),
    })
}

#[derive(Clone)]
pub struct RpcRead {
    http: reqwest::Client,
    url: String,
    sol_usd_override: Option<f64>,
}

impl RpcRead {
    pub fn new(http: reqwest::Client, url: String) -> Self {
        Self {
            http,
            url,
            sol_usd_override: None,
        }
    }

    pub fn with_sol_usd(mut self, px: f64) -> Self {
        self.sol_usd_override = Some(px);
        self
    }

    pub fn configured(&self) -> bool {
        !self.url.is_empty()
    }

    pub fn url_ref(&self) -> &str {
        &self.url
    }

    pub async fn account_data(&self, pubkey: &str) -> Result<Vec<u8>> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [pubkey, { "encoding": "base64" }]
        });
        let v = self.post(body).await?;
        let b64 = v
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.get("data"))
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.as_str())
            .context("account data")?;
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("account b64")
    }

    pub async fn account_data_bytes_opt(&self, pubkey: &str) -> Result<Option<Vec<u8>>> {
        match self.account_data(pubkey).await {
            Ok(d) => Ok(Some(d)),
            Err(_) => Ok(None),
        }
    }

    pub async fn oldest_sig_time(&self, address: &str) -> Result<Option<i64>> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignaturesForAddress",
            "params": [address, { "limit": 1 }]
        });
        let v = self.post(body).await?;
        let arr = v.get("result").and_then(|r| r.as_array()).cloned().unwrap_or_default();
        Ok(arr.first().and_then(|s| s.get("blockTime")).and_then(|n| n.as_i64()))
    }

    pub async fn sol_usd(&self) -> Result<f64> {
        if let Some(px) = self.sol_usd_override {
            anyhow::ensure!(px.is_finite() && px > 0.0, "sol usd override");
            return Ok(px);
        }
        let urls: &[(&str, fn(&Value) -> Option<f64>)] = &[
            (
                "https://lite-api.jup.ag/price/v2?ids=So11111111111111111111111111111111111111112",
                parse_jup_price,
            ),
            (
                "https://api.jup.ag/price/v2?ids=So11111111111111111111111111111111111111112",
                parse_jup_price,
            ),
            (
                "https://api.binance.com/api/v3/ticker/price?symbol=SOLUSDT",
                parse_binance_price,
            ),
            (
                "https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd",
                parse_coingecko_price,
            ),
        ];
        let mut last = anyhow::anyhow!("no sol usd source");
        for (url, parse) in urls {
            match self
                .http
                .get(*url)
                .timeout(std::time::Duration::from_secs(4))
                .send()
                .await
            {
                Ok(res) => match res.json::<Value>().await {
                    Ok(v) => {
                        if let Some(px) = parse(&v).or_else(|| parse_jup_v3_price(&v)) {
                            return Ok(px);
                        }
                        last = anyhow::anyhow!("empty sol usd from {url}");
                    }
                    Err(e) => last = e.into(),
                },
                Err(e) => last = e.into(),
            }
        }
        Err(last).context("sol usd")
    }

    pub async fn balances(&self, keys: &[String]) -> Result<BalanceSnap> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        if keys.is_empty() {
            return Ok(BalanceSnap {
                slot: 0,
                lamports: vec![],
            });
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [keys, { "encoding": "base64" }],
        });
        let v = self.post(body).await?;
        parse_balances(&v).context("parse getMultipleAccounts")
    }

    pub async fn token_positions(&self, owner: &str) -> Result<Vec<TokenPos>> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let mut all = Vec::new();
        let mut last_err = None;
        for program in [TOKEN_PROGRAM, TOKEN_2022] {
            let body = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getTokenAccountsByOwner",
                "params": [
                    owner,
                    { "programId": program },
                    { "encoding": "jsonParsed" }
                ],
            });
            match self.post(body).await {
                Ok(v) => match parse_token_positions(&v) {
                    Some(part) => all.extend(part),
                    None => last_err = Some(anyhow::anyhow!("parse getTokenAccountsByOwner")),
                },
                Err(e) => last_err = Some(e),
            }
        }
        if all.is_empty() {
            if let Some(e) = last_err {
                return Err(e);
            }
            return Ok(Vec::new());
        }
        all.sort_by(|a, b| {
            b.ui_amount
                .partial_cmp(&a.ui_amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(all)
    }

    pub async fn latest_blockhash(&self) -> Result<[u8; 32]> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getLatestBlockhash",
            "params": [{ "commitment": "confirmed" }]
        });
        let v = self.post(body).await?;
        let hash = v
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.get("blockhash"))
            .and_then(|s| s.as_str())
            .context("blockhash")?;
        crate::config::parse_pubkey(hash)
    }

    pub async fn genesis_hash(&self) -> Result<String> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getGenesisHash",
            "params": [],
        });
        let v = self.post(body).await?;
        v.get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .context("parse getGenesisHash")
    }

    async fn post(&self, body: Value) -> Result<Value> {
        let res = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .context("rpc http")?;
        let status = res.status();
        let v: Value = res.json().await.context("rpc json")?;
        if let Some(err) = v.get("error") {
            anyhow::bail!("rpc {status}: {err}");
        }
        anyhow::ensure!(status.is_success(), "rpc {status}: {v}");
        Ok(v)
    }

    pub async fn token_ticker(&self, mint: &str) -> Option<String> {
        let card = self.token_card(mint).await;
        card.symbol.or(card.name)
    }

    pub async fn token_identity(&self, mint: &str) -> (Option<String>, Option<String>) {
        let card = self.token_card(mint).await;
        (card.name, card.symbol)
    }

    pub async fn token_card(&self, mint: &str) -> DexSnap {
        let mut card = DexSnap::default();
        if let Ok(pk) = crate::config::parse_pubkey(mint) {
            let meta = crate::swap::mpl_metadata_pda(&pk);
            if let Ok(data) = self
                .account_data(&crate::config::encode_pubkey(&meta))
                .await
            {
                let (name, symbol) = parse_mpl_identity(&data);
                card.name = name;
                card.symbol = symbol;
            }
        }
        if let Some(snap) = self.dex_token(mint).await {
            merge_dex(&mut card, snap);
        }
        if card.name.is_none() || card.symbol.is_none() || card.mcap_usd.is_none() {
            if let Some(snap) = self.jup_token(mint).await {
                merge_dex(&mut card, snap);
            }
        }
        if card.dex.is_empty() || card.dex == "unknown" {
            card.dex = if card.name.is_some() || card.symbol.is_some() {
                "Solana".into()
            } else {
                String::new()
            };
        }
        card
    }

    pub async fn dex_token(&self, mint: &str) -> Option<DexSnap> {
        let url = format!("https://api.dexscreener.com/latest/dex/tokens/{mint}");
        let v: Value = self.http.get(url).send().await.ok()?.json().await.ok()?;
        parse_dex_token(&v, mint)
    }

    pub async fn jup_token(&self, mint: &str) -> Option<DexSnap> {
        let url = format!("https://lite-api.jup.ag/tokens/v2/search?query={mint}");
        let v: Value = self.http.get(url).send().await.ok()?.json().await.ok()?;
        parse_jup_token(&v, mint)
    }

    pub async fn mint_auth(&self, mint: &str) -> Option<crate::protect::MintAuth> {
        let data = self.account_data(mint).await.ok()?;
        crate::protect::parse_mint_authorities(&data)
    }

    pub async fn send_b64_tx(&self, b64: &str) -> Result<String> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [b64, { "encoding": "base64", "skipPreflight": true, "maxRetries": 2 }]
        });
        let v = self.post(body).await?;
        v.get("result")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .context("sendTransaction result")
    }

    /// `Ok(())` once the sig is processed/confirmed with no `err`. Missing sig is `Ok(false)`.
    pub async fn signature_ok(&self, sig: &str) -> Result<bool> {
        anyhow::ensure!(self.configured(), "rpc url empty");
        let v = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSignatureStatuses",
                "params": [[sig], { "searchTransactionHistory": true }]
            }))
            .await?;
        let st = v
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|a| a.get(0));
        let Some(st) = st.filter(|x| !x.is_null()) else {
            return Ok(false);
        };
        if let Some(err) = st.get("err").filter(|e| !e.is_null()) {
            anyhow::bail!("on-chain {err}");
        }
        Ok(st.get("confirmationStatus").and_then(|s| s.as_str()).is_some()
            || st.get("slot").and_then(|n| n.as_u64()).is_some())
    }

    pub async fn wait_landed(&self, sig: &str, secs: u64) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            if self.signature_ok(sig).await? {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("not confirmed `{sig}`");
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    pub async fn pump_curve_quote(
        &self,
        mint: &str,
        tokens: u64,
        sol_usd: f64,
    ) -> (u64, Option<f64>) {
        let Ok(pk) = crate::config::parse_pubkey(mint) else {
            return (0, None);
        };
        let bond = crate::config::encode_pubkey(&crate::swap::bonding_curve_pda(&pk));
        let Ok(data) = self.account_data(&bond).await else {
            return (0, None);
        };
        let curve = crate::protect::parse_curve_reserves(&data);
        let sol = crate::protect::expected_sell_sol(
            tokens,
            curve.map(|c| c.virtual_sol),
            curve.map(|c| c.virtual_token),
        );
        let mcap = crate::protect::mcap_usd_from_curve(&data, sol_usd);
        (sol, mcap)
    }
}

pub fn parse_balances(v: &Value) -> Option<BalanceSnap> {
    let result = v.get("result")?;
    let slot = result.get("context")?.get("slot")?.as_u64()?;
    let arr = result.get("value")?.as_array()?;
    let lamports = arr
        .iter()
        .map(|item| {
            if item.is_null() {
                Some(0)
            } else {
                item.get("lamports").and_then(|n| n.as_u64())
            }
        })
        .collect();
    Some(BalanceSnap { slot, lamports })
}

pub fn parse_token_positions(v: &Value) -> Option<Vec<TokenPos>> {
    let arr = v.get("result")?.get("value")?.as_array()?;
    let mut out = Vec::new();
    for item in arr {
        let info = item
            .get("account")?
            .get("data")?
            .get("parsed")?
            .get("info")?;
        let mint = info.get("mint")?.as_str()?.to_string();
        let tok = info.get("tokenAmount")?;
        let amount: u64 = tok.get("amount")?.as_str()?.parse().ok()?;
        let decimals = tok.get("decimals")?.as_u64()? as u8;
        let ui_amount = tok
            .get("uiAmount")
            .and_then(|n| n.as_f64())
            .unwrap_or(amount as f64 / 10f64.powi(decimals as i32));
        if amount == 0 {
            continue;
        }
        out.push(TokenPos {
            mint,
            amount,
            decimals,
            ui_amount,
        });
    }
    out.sort_by(|a, b| {
        b.ui_amount
            .partial_cmp(&a.ui_amount)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(out)
}

/// Metaplex metadata: skip key + two pubkeys, then Borsh name, then symbol.
pub fn parse_mpl_identity(data: &[u8]) -> (Option<String>, Option<String>) {
    if data.len() < 69 {
        return (None, None);
    }
    let mut i = 1 + 32 + 32;
    let Some((name, next)) = borsh_string(data, i) else {
        return (None, None);
    };
    i = next;
    let symbol = borsh_string(data, i).map(|(s, _)| s);
    let clean = |s: String| {
        let t = s.trim().trim_end_matches('\0').trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    (clean(name), symbol.and_then(clean))
}

pub fn parse_mpl_ticker(data: &[u8]) -> Option<String> {
    parse_mpl_identity(data).1
}

fn borsh_string(data: &[u8], start: usize) -> Option<(String, usize)> {
    if data.len() < start + 4 {
        return None;
    }
    let n = u32::from_le_bytes(data[start..start + 4].try_into().ok()?) as usize;
    let end = start.checked_add(4)?.checked_add(n)?;
    let raw = data.get(start + 4..end)?;
    Some((String::from_utf8_lossy(raw).into_owned(), end))
}

pub fn is_placeholder_payer(pubkey: &str) -> bool {
    pubkey == WSOL_MINT
}

pub fn fmt_sol(lamports: u64) -> String {
    format!("{:.4}", lamports as f64 / 1_000_000_000.0)
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite() && *n > 0.0)
}

pub(crate) fn parse_jup_price(v: &Value) -> Option<f64> {
    v.get("data")
        .and_then(|d| d.as_object())
        .and_then(|o| o.values().next())
        .and_then(|x| x.get("price"))
        .and_then(json_f64)
}

pub(crate) fn parse_jup_v3_price(v: &Value) -> Option<f64> {
    v.as_object()?
        .values()
        .next()?
        .get("usdPrice")
        .and_then(json_f64)
}

pub(crate) fn parse_binance_price(v: &Value) -> Option<f64> {
    v.get("price").and_then(json_f64)
}

pub(crate) fn parse_coingecko_price(v: &Value) -> Option<f64> {
    v.get("solana")
        .and_then(|s| s.get("usd"))
        .and_then(json_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raydium_cpmm_label() {
        assert_eq!(pretty_dex("raydium", &["CPMM".into()]), "Raydium CPMM");
    }

    #[test]
    fn jup_v2_search_ada_chan() {
        let v = json!([{
            "id": "DKDd3UzMwuYQ7unVbFRSVrB1uBMMe3d6RcLvneXtxTKU",
            "name": "Ada-Chan",
            "symbol": "ADACHAN",
            "mcap": 3752.66,
            "usdPrice": 0.00000375,
            "liquidity": 199.26,
            "launchpad": "stonkfun",
            "tokenProgram": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
        }]);
        let snap = parse_jup_token(&v, "DKDd3UzMwuYQ7unVbFRSVrB1uBMMe3d6RcLvneXtxTKU").unwrap();
        assert_eq!(snap.name.as_deref(), Some("Ada-Chan"));
        assert_eq!(snap.symbol.as_deref(), Some("ADACHAN"));
        assert_eq!(snap.dex, "Stonk.fun");
        assert!(snap.mcap_usd.unwrap() > 3000.0);
    }

    #[test]
    fn parses_balances_and_null_account() {
        let v = json!({
            "result": {
                "context": { "slot": 99 },
                "value": [
                    { "lamports": 1_500_000_000u64 },
                    null
                ]
            }
        });
        let snap = parse_balances(&v).unwrap();
        assert_eq!(snap.slot, 99);
        assert_eq!(snap.lamports, vec![Some(1_500_000_000), Some(0)]);
    }

    #[test]
    fn parses_token_accounts_skips_zero() {
        let v = json!({
            "result": {
                "value": [
                    {
                        "account": {
                            "data": {
                                "parsed": {
                                    "info": {
                                        "mint": "MintA",
                                        "tokenAmount": {
                                            "amount": "0",
                                            "decimals": 6,
                                            "uiAmount": 0.0
                                        }
                                    }
                                }
                            }
                        }
                    },
                    {
                        "account": {
                            "data": {
                                "parsed": {
                                    "info": {
                                        "mint": "MintB",
                                        "tokenAmount": {
                                            "amount": "2500000",
                                            "decimals": 6,
                                            "uiAmount": 2.5
                                        }
                                    }
                                }
                            }
                        }
                    }
                ]
            }
        });
        let pos = parse_token_positions(&v).unwrap();
        assert_eq!(pos.len(), 1);
        assert_eq!(pos[0].mint, "MintB");
        assert_eq!(pos[0].ui_amount, 2.5);
    }

    #[test]
    fn parses_sol_usd_from_common_feeds() {
        let jup = json!({
            "data": {
                "So11111111111111111111111111111111111111112": { "price": "187.42" }
            }
        });
        assert_eq!(parse_jup_price(&jup), Some(187.42));
        let jup3 = json!({
            "So11111111111111111111111111111111111111112": { "usdPrice": 190.1 }
        });
        assert_eq!(parse_jup_v3_price(&jup3), Some(190.1));
        assert_eq!(
            parse_binance_price(&json!({ "price": "188.05" })),
            Some(188.05)
        );
        assert_eq!(
            parse_coingecko_price(&json!({ "solana": { "usd": 191.0 } })),
            Some(191.0)
        );
    }

    #[test]
    fn mpl_ticker_from_name_then_symbol() {
        let mut data = vec![0u8; 65];
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(b"Foo");
        data.extend_from_slice(&6u32.to_le_bytes());
        data.extend_from_slice(b"Licker");
        assert_eq!(parse_mpl_ticker(&data).as_deref(), Some("Licker"));
    }

    #[test]
    fn placeholder_is_wsol() {
        assert!(is_placeholder_payer(WSOL_MINT));
        assert!(!is_placeholder_payer("Abc"));
    }
}
