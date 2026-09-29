use crate::rpc_read::WSOL_MINT;
use anyhow::{Context, Result};
use futures_util::future::select_ok;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

const ENDPOINTS: &[(&str, &str)] = &[
    (
        "https://lite-api.jup.ag/swap/v1/quote",
        "https://lite-api.jup.ag/swap/v1/swap",
    ),
    (
        "https://api.jup.ag/swap/v1/quote",
        "https://api.jup.ag/swap/v1/swap",
    ),
];

pub fn wsol() -> &'static str {
    WSOL_MINT
}

pub fn slip_bps(pct: f64) -> u64 {
    ((pct.max(0.0) * 100.0).round() as u64).clamp(1, 5_000)
}

/// Tip lamports and/or fixed CU price — never Jupiter auto priority levels.
pub fn swap_prio_body(tip_lamports: u64, fee_lamports: u64) -> Value {
    if tip_lamports > 0 {
        json!({ "jitoTipLamports": tip_lamports.max(1) })
    } else {
        json!(fee_lamports.max(1))
    }
}

pub fn fee_to_cu_price_micro(fee_lamports: u64) -> u64 {
    fee_lamports
        .saturating_mul(1_000_000)
        .saturating_div(200_000)
        .max(1)
}

#[derive(Debug)]
pub struct SwapBuilt {
    pub raw: Vec<u8>,
    pub out: u64,
    pub quote_ms: u64,
}

pub async fn swap_tx(
    http: &reqwest::Client,
    user: &str,
    input_mint: &str,
    output_mint: &str,
    amount: u64,
    slippage_pct: f64,
    tip_lamports: u64,
    fee_lamports: u64,
) -> Result<SwapBuilt> {
    anyhow::ensure!(amount > 0, "swap amount is 0");
    let bps = slip_bps(slippage_pct);
    let t0 = std::time::Instant::now();
    type Fut<'a> = Pin<Box<dyn Future<Output = Result<(Vec<u8>, u64)>> + Send + 'a>>;
    let futs: Vec<Fut<'_>> = ENDPOINTS
        .iter()
        .map(|(quote_url, swap_url)| {
            let http = http.clone();
            let user = user.to_string();
            let input_mint = input_mint.to_string();
            let output_mint = output_mint.to_string();
            Box::pin(async move {
                quote_swap(
                    &http,
                    quote_url,
                    swap_url,
                    &user,
                    &input_mint,
                    &output_mint,
                    amount,
                    bps,
                    tip_lamports,
                    fee_lamports,
                )
                .await
            }) as Fut<'_>
        })
        .collect();
    match select_ok(futs).await {
        Ok(((raw, out), _)) => Ok(SwapBuilt {
            raw,
            out,
            quote_ms: t0.elapsed().as_millis() as u64,
        }),
        Err(e) => Err(e).context("jupiter quote/swap"),
    }
}

async fn quote_swap(
    http: &reqwest::Client,
    quote_url: &str,
    swap_url: &str,
    user: &str,
    input_mint: &str,
    output_mint: &str,
    amount: u64,
    bps: u64,
    tip_lamports: u64,
    fee_lamports: u64,
) -> Result<(Vec<u8>, u64)> {
    let url = format!(
        "{quote_url}?inputMint={input_mint}&outputMint={output_mint}&amount={amount}&slippageBps={bps}&swapMode=ExactIn"
    );
    let quote: Value = http
        .get(&url)
        .send()
        .await
        .context("jup quote http")?
        .json()
        .await
        .context("jup quote json")?;
    if quote.get("outAmount").is_none() {
        anyhow::bail!(
            "no swap route ({})",
            quote
                .get("error")
                .or_else(|| quote.get("message"))
                .cloned()
                .unwrap_or(quote)
        );
    }
    let out: u64 = quote
        .get("outAmount")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| quote.get("outAmount").and_then(|n| n.as_u64()))
        .context("outAmount")?;

    // Prefer tip + fixed CU price in one request; fall back to tip-only if Jupiter rejects.
    let attempts: Vec<Value> = if tip_lamports > 0 && fee_lamports > 0 {
        vec![
            json!({
                "userPublicKey": user,
                "quoteResponse": quote,
                "wrapAndUnwrapSol": true,
                "dynamicComputeUnitLimit": true,
                "prioritizationFeeLamports": swap_prio_body(tip_lamports, 0),
                "computeUnitPriceMicroLamports": fee_to_cu_price_micro(fee_lamports),
            }),
            json!({
                "userPublicKey": user,
                "quoteResponse": quote,
                "wrapAndUnwrapSol": true,
                "dynamicComputeUnitLimit": true,
                "prioritizationFeeLamports": swap_prio_body(tip_lamports, 0),
            }),
        ]
    } else {
        vec![json!({
            "userPublicKey": user,
            "quoteResponse": quote,
            "wrapAndUnwrapSol": true,
            "dynamicComputeUnitLimit": true,
            "prioritizationFeeLamports": swap_prio_body(tip_lamports, fee_lamports),
        })]
    };

    let mut last = anyhow::anyhow!("swapTransaction missing");
    for body in attempts {
        let swap: Value = http
            .post(swap_url)
            .json(&body)
            .send()
            .await
            .context("jup swap http")?
            .json()
            .await
            .context("jup swap json")?;
        if let Some(b64) = swap.get("swapTransaction").and_then(|s| s.as_str()) {
            use base64::Engine;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .context("swap tx b64")?;
            return Ok((raw, out));
        }
        last = anyhow::anyhow!(
            "swapTransaction missing ({})",
            swap.get("error")
                .or_else(|| swap.get("message"))
                .cloned()
                .unwrap_or(swap)
        );
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slip_bps_clamps() {
        assert_eq!(slip_bps(10.0), 1000);
        assert_eq!(slip_bps(0.0), 1);
        assert_eq!(slip_bps(100.0), 5000);
    }

    #[test]
    fn tip_is_exact_jito_lamports() {
        let v = swap_prio_body(12_000_000, 5_000_000);
        assert_eq!(v["jitoTipLamports"], 12_000_000);
        assert!(v.get("priorityLevelWithMaxLamports").is_none());
    }

    #[test]
    fn fee_only_is_fixed_integer() {
        let v = swap_prio_body(0, 5_000_000);
        assert_eq!(v, json!(5_000_000));
    }

    #[test]
    fn cu_price_from_fee() {
        assert!(fee_to_cu_price_micro(5_000_000) > 1);
    }

    #[tokio::test]
    async fn live_jupiter_exact_tip_builds() {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .unwrap();
        let user = "Q9mMMPPSbibQ1QE9uhNtRPdUwqS1crvQti1bneGzUot";
        let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let built = swap_tx(
            &http,
            user,
            WSOL_MINT,
            usdc,
            10_000_000,
            1.0,
            12_000_000,
            5_000_000,
        )
        .await
        .expect("jupiter exact tip (+ optional cu) must build");
        assert!(built.out > 0);
        assert!(built.raw.len() > 100);
    }
}
