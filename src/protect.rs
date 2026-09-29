use crate::types::TargetEvent;
use anyhow::{Context, Result};
use serde_json::{json, Value};

pub const BUY_TAX_MAX_PCT: f64 = 5.0;

#[derive(Clone, Debug)]
pub struct MintAuth {
    pub mint_revoked: bool,
    pub freeze_revoked: bool,
}

/// SPL mint: COption mint authority at 0, freeze at 46. 0 = None.
pub fn parse_mint_authorities(data: &[u8]) -> Option<MintAuth> {
    if data.len() < 82 {
        return None;
    }
    let mint_opt = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let freeze_opt = u32::from_le_bytes(data[46..50].try_into().ok()?);
    Some(MintAuth {
        mint_revoked: mint_opt == 0,
        freeze_revoked: freeze_opt == 0,
    })
}

pub fn authorities_revoked(auth: &MintAuth) -> bool {
    auth.mint_revoked && auth.freeze_revoked
}

pub fn implied_slip_pct(expected_out: f64, simulated_out: f64) -> f64 {
    if expected_out <= 0.0 {
        return 100.0;
    }
    ((expected_out - simulated_out) / expected_out * 100.0).max(0.0)
}

pub fn slip_ok(expected_out: f64, simulated_out: f64, max_pct: f64) -> bool {
    implied_slip_pct(expected_out, simulated_out) <= max_pct.max(0.0)
}

pub fn buy_tax_ok(expected_out: f64, simulated_out: f64) -> bool {
    slip_ok(expected_out, simulated_out, BUY_TAX_MAX_PCT)
}

pub fn sell_slip_ok(expected_out: f64, simulated_out: f64, max_pct: f64) -> bool {
    slip_ok(expected_out, simulated_out, max_pct.clamp(0.0, 100.0))
}

/// Tokens / lamports still accepted after `slip_pct` worse than the quote.
pub fn min_out_after_slip(expected: u64, slip_pct: f64) -> u64 {
    let slip = slip_pct.clamp(0.0, 100.0);
    let keep = (100.0 - slip) / 100.0;
    ((expected as f64) * keep).floor() as u64
}

#[derive(Clone, Copy, Debug)]
pub struct CurveReserves {
    pub virtual_token: u64,
    pub virtual_sol: u64,
    pub supply: u64,
}

pub fn parse_curve_reserves(data: &[u8]) -> Option<CurveReserves> {
    if data.len() < 8 + 40 {
        return None;
    }
    Some(CurveReserves {
        virtual_token: u64::from_le_bytes(data[8..16].try_into().ok()?),
        virtual_sol: u64::from_le_bytes(data[16..24].try_into().ok()?),
        supply: u64::from_le_bytes(data[40..48].try_into().ok()?),
    })
}

/// Pump.fun constant-product quote after the 1% protocol fee.
pub fn curve_tokens_out(virtual_sol: u64, virtual_token: u64, sol_in: u64) -> u64 {
    if virtual_sol == 0 || virtual_token == 0 || sol_in == 0 {
        return 0;
    }
    let vs = virtual_sol as u128;
    let vt = virtual_token as u128;
    let din = (sol_in as u128).saturating_mul(99) / 100;
    if din == 0 {
        return 0;
    }
    let new_sol = vs.saturating_add(din);
    if new_sol == 0 {
        return 0;
    }
    let new_token = vs.saturating_mul(vt) / new_sol;
    vt.saturating_sub(new_token) as u64
}

pub fn curve_sol_out(virtual_sol: u64, virtual_token: u64, tokens_in: u64) -> u64 {
    if virtual_sol == 0 || virtual_token == 0 || tokens_in == 0 {
        return 0;
    }
    let vs = virtual_sol as u128;
    let vt = virtual_token as u128;
    let tin = tokens_in as u128;
    let new_token = vt.saturating_add(tin);
    if new_token == 0 {
        return 0;
    }
    let new_sol = vs.saturating_mul(vt) / new_token;
    let raw = vs.saturating_sub(new_sol);
    (raw.saturating_mul(99) / 100) as u64
}

pub fn expected_buy_tokens(
    ev: &TargetEvent,
    copy_sol: u64,
    virtual_sol: Option<u64>,
    virtual_token: Option<u64>,
) -> u64 {
    if let (Some(vs), Some(vt)) = (virtual_sol, virtual_token) {
        let quoted = curve_tokens_out(vs, vt, copy_sol);
        if quoted > 0 {
            return quoted;
        }
    }
    if let TargetEvent::Buy {
        token_amount,
        sol_lamports,
        ..
    } = *ev
    {
        if sol_lamports > 0 && token_amount > 0 && copy_sol > 0 {
            return ((token_amount as u128)
                .saturating_mul(copy_sol as u128)
                / sol_lamports as u128) as u64;
        }
    }
    0
}

pub fn expected_sell_sol(
    tokens: u64,
    virtual_sol: Option<u64>,
    virtual_token: Option<u64>,
) -> u64 {
    if let (Some(vs), Some(vt)) = (virtual_sol, virtual_token) {
        return curve_sol_out(vs, vt, tokens);
    }
    0
}

pub fn sim_logs(sim: &Value) -> String {
    sim.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("logs"))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

pub fn sim_slip_rejected(sim: &Value) -> bool {
    let logs = sim_logs(sim).to_ascii_lowercase();
    logs.contains("slippage")
        || logs.contains("too much")
        || logs.contains("exceeded")
        || logs.contains("tax")
}

/// Pump bonding curve: after 8-byte disc, virtual_token, virtual_sol, real_token, real_sol, supply.
pub fn mcap_usd_from_curve(data: &[u8], sol_usd: f64) -> Option<f64> {
    let c = parse_curve_reserves(data)?;
    if c.virtual_token == 0 {
        return None;
    }
    let price_sol = (c.virtual_sol as f64 / 1e9) / (c.virtual_token as f64);
    Some(price_sol * c.supply as f64 * sol_usd)
}

pub fn creator_from_curve(data: &[u8]) -> Option<[u8; 32]> {
    // disc 8 + 5 u64 (40) + complete bool + padding to pubkey.
    let start = 8 + 40 + 1;
    if data.len() < start + 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&data[start..start + 32]);
    Some(out)
}

pub async fn simulate_tx(
    http: &reqwest::Client,
    rpc_url: &str,
    tx_b64: &str,
) -> Result<Value> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "simulateTransaction",
        "params": [tx_b64, {
            "encoding": "base64",
            "commitment": "processed",
            "sigVerify": false,
            "innerInstructions": true
        }]
    });
    let res = http.post(rpc_url).json(&body).send().await.context("simulate http")?;
    let v: Value = res.json().await.context("simulate json")?;
    if let Some(err) = v.get("error") {
        anyhow::bail!("simulate: {err}");
    }
    Ok(v)
}

pub fn sim_failed(v: &Value) -> bool {
    v.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("err"))
        .map(|e| !e.is_null())
        .unwrap_or(true)
}

pub fn event_is_buy(ev: &TargetEvent) -> bool {
    matches!(ev, TargetEvent::Buy { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_coption_none() {
        let mut data = vec![0u8; 82];
        let a = parse_mint_authorities(&data).unwrap();
        assert!(authorities_revoked(&a));
        data[0] = 1;
        let a = parse_mint_authorities(&data).unwrap();
        assert!(!a.mint_revoked);
        data[0] = 0;
        data[46] = 1;
        let a = parse_mint_authorities(&data).unwrap();
        assert!(!a.freeze_revoked);
    }

    #[test]
    fn tax_gate() {
        assert!(buy_tax_ok(100.0, 96.0));
        assert!(!buy_tax_ok(100.0, 90.0));
        assert!(sell_slip_ok(100.0, 86.0, 15.0));
        assert!(!sell_slip_ok(100.0, 80.0, 15.0));
        assert!(slip_ok(100.0, 50.0, 55.0));
        assert!(!slip_ok(100.0, 40.0, 55.0));
        assert_eq!(min_out_after_slip(1000, 10.0), 900);
        assert_eq!(min_out_after_slip(1000, 0.0), 1000);
        assert_eq!(min_out_after_slip(1000, 100.0), 0);
    }

    #[test]
    fn curve_quotes_move_with_size() {
        let vs = 30_000_000_000;
        let vt = 1_073_000_000_000_000;
        let small = curve_tokens_out(vs, vt, 10_000_000);
        let big = curve_tokens_out(vs, vt, 100_000_000);
        assert!(small > 0 && big > small);
        let sol = curve_sol_out(vs, vt, small);
        assert!(sol > 0);
    }
}
