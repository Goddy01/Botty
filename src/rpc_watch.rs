use crate::config::{encode_pubkey, parse_pubkey, rpc_ws_url};
use crate::types::{CompiledIx, GeyserTx, RawTx};
use anyhow::{Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Semaphore, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const SIG_CACHE: usize = 256;
const FETCH_TRIES: usize = 8;
const FETCH_GAP_MS: u64 = 10;
const MAX_INFLIGHT_FETCH: usize = 24;
const FETCH_QUEUE: usize = 128;

#[derive(Clone, Debug)]
pub struct RpcDetect {
    pub ev: GeyserTx,
    pub signature: String,
    pub notify_to_fetch_ms: u64,
    pub fetch_ms: u64,
    pub notify_to_tx_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogsNotify {
    pub signature: String,
    pub slot: u64,
    pub logs: Vec<String>,
}

enum FetchOut {
    Hit(RpcDetect),
    Miss { sig: String, fetch_ms: u64 },
    Fail { sig: String, err: String },
}

/// Rough vs a ~400ms Solana slot. This is detect lag, not fill lag.
pub fn slot_verdict_ms(notify_to_tx_ms: u64) -> &'static str {
    if notify_to_tx_ms < 80 {
        "maybe_same_slot"
    } else if notify_to_tx_ms < 400 {
        "likely_next_slot"
    } else {
        "two_plus_slots"
    }
}

pub fn parse_logs_notification(v: &Value) -> Option<LogsNotify> {
    if v.get("method").and_then(|m| m.as_str()) != Some("logsNotification") {
        return None;
    }
    let result = v.get("params")?.get("result")?;
    let slot = result.get("context")?.get("slot")?.as_u64()?;
    let value = result.get("value")?;
    if !value.get("err").map(|e| e.is_null()).unwrap_or(false) {
        return None;
    }
    let signature = value.get("signature")?.as_str()?.to_string();
    let logs = value
        .get("logs")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(LogsNotify {
        signature,
        slot,
        logs,
    })
}

fn pubkey_from_json(v: &Value) -> Option<[u8; 32]> {
    let s = if let Some(s) = v.as_str() {
        s
    } else {
        v.get("pubkey")?.as_str()?
    };
    parse_pubkey(s).ok()
}

fn compiled_ix_from_json(ix: &Value) -> Option<CompiledIx> {
    let program_id_index = ix.get("programIdIndex")?.as_u64()? as u8;
    let accounts = ix
        .get("accounts")?
        .as_array()?
        .iter()
        .map(|a| a.as_u64().map(|n| n as u8))
        .collect::<Option<Vec<u8>>>()?;
    let data = bs58::decode(ix.get("data")?.as_str()?)
        .into_vec()
        .ok()?;
    Some(CompiledIx {
        program_id_index,
        accounts,
        data,
    })
}

fn read_short_u16(data: &[u8], i: &mut usize) -> Option<u16> {
    let mut val: u32 = 0;
    let mut shift = 0;
    for _ in 0..3 {
        let b = *data.get(*i)?;
        *i += 1;
        val |= ((b & 0x7f) as u32) << shift;
        if b & 0x80 == 0 {
            return u16::try_from(val).ok();
        }
        shift += 7;
    }
    None
}

fn read_bytes<'a>(data: &'a [u8], i: &mut usize, n: usize) -> Option<&'a [u8]> {
    let end = i.checked_add(n)?;
    let slice = data.get(*i..end)?;
    *i = end;
    Some(slice)
}

fn parse_compiled_ixs(data: &[u8], i: &mut usize) -> Option<Vec<CompiledIx>> {
    let n = read_short_u16(data, i)? as usize;
    let mut ixs = Vec::with_capacity(n);
    for _ in 0..n {
        let program_id_index = *data.get(*i)?;
        *i += 1;
        let n_acc = read_short_u16(data, i)? as usize;
        let accounts = read_bytes(data, i, n_acc)?.to_vec();
        let n_data = read_short_u16(data, i)? as usize;
        let ix_data = read_bytes(data, i, n_data)?.to_vec();
        ixs.push(CompiledIx {
            program_id_index,
            accounts,
            data: ix_data,
        });
    }
    Some(ixs)
}

fn parse_message_accounts_ixs(
    data: &[u8],
    i: &mut usize,
) -> Option<(Vec<[u8; 32]>, Vec<CompiledIx>, [u8; 32])> {
    // header: num_required_signatures, num_readonly_signed, num_readonly_unsigned
    let _ = read_bytes(data, i, 3)?;
    let n_keys = read_short_u16(data, i)? as usize;
    let mut keys = Vec::with_capacity(n_keys);
    for _ in 0..n_keys {
        let raw = read_bytes(data, i, 32)?;
        let mut k = [0u8; 32];
        k.copy_from_slice(raw);
        keys.push(k);
    }
    let mut blockhash = [0u8; 32];
    blockhash.copy_from_slice(read_bytes(data, i, 32)?);
    let ixs = parse_compiled_ixs(data, i)?;
    Some((keys, ixs, blockhash))
}

fn skip_address_table_lookups(data: &[u8], i: &mut usize) -> Option<()> {
    let n = read_short_u16(data, i)? as usize;
    for _ in 0..n {
        let _ = read_bytes(data, i, 32)?;
        let n_w = read_short_u16(data, i)? as usize;
        let _ = read_bytes(data, i, n_w)?;
        let n_r = read_short_u16(data, i)? as usize;
        let _ = read_bytes(data, i, n_r)?;
    }
    Some(())
}

/// Legacy or v0 wire tx → static keys, top-level ixs, blockhash.
pub fn parse_solana_tx_wire(bytes: &[u8]) -> Option<(Vec<[u8; 32]>, Vec<CompiledIx>, [u8; 32])> {
    let mut i = 0usize;
    let n_sig = read_short_u16(bytes, &mut i)? as usize;
    let _ = read_bytes(bytes, &mut i, n_sig.checked_mul(64)?)?;
    let first = *bytes.get(i)?;
    if first & 0x80 != 0 {
        let version = first & 0x7f;
        i += 1;
        if version != 0 {
            return None;
        }
        let parsed = parse_message_accounts_ixs(bytes, &mut i)?;
        skip_address_table_lookups(bytes, &mut i)?;
        return Some(parsed);
    }
    parse_message_accounts_ixs(bytes, &mut i)
}

fn parse_json_message(msg: &Value) -> Option<(Vec<[u8; 32]>, Vec<CompiledIx>, [u8; 32])> {
    let mut keys = Vec::new();
    for k in msg.get("accountKeys")?.as_array()? {
        keys.push(pubkey_from_json(k)?);
    }
    let mut ixs = Vec::new();
    if let Some(arr) = msg.get("instructions").and_then(|a| a.as_array()) {
        for ix in arr {
            ixs.push(compiled_ix_from_json(ix)?);
        }
    }
    let mut blockhash = [0u8; 32];
    if let Some(h) = msg.get("recentBlockhash").and_then(|s| s.as_str()) {
        if let Ok(b) = parse_pubkey(h) {
            blockhash = b;
        }
    }
    Some((keys, ixs, blockhash))
}

fn parse_b64_transaction(txv: &Value) -> Option<(Vec<[u8; 32]>, Vec<CompiledIx>, [u8; 32])> {
    let b64 = if let Some(s) = txv.as_str() {
        s
    } else {
        txv.as_array()?.first()?.as_str()?
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .ok()?;
    parse_solana_tx_wire(&bytes)
}

fn append_loaded_addresses(keys: &mut Vec<[u8; 32]>, result: &Value) {
    let Some(loaded) = result.get("meta").and_then(|m| m.get("loadedAddresses")) else {
        return;
    };
    for list in ["writable", "readonly"] {
        if let Some(arr) = loaded.get(list).and_then(|a| a.as_array()) {
            for k in arr {
                if let Some(pk) = pubkey_from_json(k) {
                    keys.push(pk);
                }
            }
        }
    }
}

fn append_inner_ixs(ixs: &mut Vec<CompiledIx>, result: &Value) {
    let Some(groups) = result
        .get("meta")
        .and_then(|m| m.get("innerInstructions"))
        .and_then(|a| a.as_array())
    else {
        return;
    };
    for group in groups {
        if let Some(arr) = group.get("instructions").and_then(|a| a.as_array()) {
            for ix in arr {
                if let Some(c) = compiled_ix_from_json(ix) {
                    ixs.push(c);
                }
            }
        }
    }
}

/// `getTransaction` `result` — `json` message or `base64` wire bytes.
pub fn geyser_from_rpc_result(result: &Value) -> Option<GeyserTx> {
    let slot = result.get("slot")?.as_u64()?;
    let txv = result.get("transaction")?;
    let (mut keys, mut ixs, blockhash) = if txv.get("message").is_some() {
        parse_json_message(txv.get("message")?)?
    } else {
        parse_b64_transaction(txv)?
    };
    append_loaded_addresses(&mut keys, result);
    append_inner_ixs(&mut ixs, result);
    let (token_credits, token_debits, sol_spent) = parse_balance_flow(result, &keys);
    Some(GeyserTx {
        raw: RawTx {
            account_keys: keys,
            ixs,
        },
        slot,
        blockhash,
        token_credits,
        token_debits,
        sol_spent,
    })
}

fn parse_balance_flow(
    result: &Value,
    keys: &[[u8; 32]],
) -> (
    Vec<crate::types::TokenCredit>,
    Vec<crate::types::TokenCredit>,
    Vec<([u8; 32], u64)>,
) {
    use crate::rpc_read::WSOL_MINT;
    let meta = result.get("meta");
    let mut pre: std::collections::HashMap<(String, String), u64> = std::collections::HashMap::new();
    if let Some(arr) = meta.and_then(|m| m.get("preTokenBalances")).and_then(|a| a.as_array()) {
        for b in arr {
            let mint = b.get("mint").and_then(|v| v.as_str()).unwrap_or("");
            let owner = b.get("owner").and_then(|v| v.as_str()).unwrap_or("");
            let amt = b
                .get("uiTokenAmount")
                .and_then(|u| u.get("amount"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            pre.insert((owner.to_string(), mint.to_string()), amt);
        }
    }
    let mut credits = Vec::new();
    let mut debits = Vec::new();
    if let Some(arr) = meta.and_then(|m| m.get("postTokenBalances")).and_then(|a| a.as_array()) {
        for b in arr {
            let mint_s = b.get("mint").and_then(|v| v.as_str()).unwrap_or("");
            if mint_s.is_empty() || mint_s == WSOL_MINT {
                continue;
            }
            let owner_s = b.get("owner").and_then(|v| v.as_str()).unwrap_or("");
            let post = b
                .get("uiTokenAmount")
                .and_then(|u| u.get("amount"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let before = pre.get(&(owner_s.to_string(), mint_s.to_string())).copied().unwrap_or(0);
            if post == before {
                continue;
            }
            let (Ok(owner), Ok(mint)) = (parse_pubkey(owner_s), parse_pubkey(mint_s)) else {
                continue;
            };
            let row = crate::types::TokenCredit {
                owner,
                mint,
                amount: post.abs_diff(before),
            };
            if post > before {
                credits.push(row);
            } else {
                debits.push(row);
            }
        }
    }
    let mut sol_spent = Vec::new();
    let pre_sol = meta.and_then(|m| m.get("preBalances")).and_then(|a| a.as_array());
    let post_sol = meta.and_then(|m| m.get("postBalances")).and_then(|a| a.as_array());
    if let (Some(pre_sol), Some(post_sol)) = (pre_sol, post_sol) {
        for (i, key) in keys.iter().enumerate() {
            let a = pre_sol.get(i).and_then(|v| v.as_u64()).unwrap_or(0);
            let b = post_sol.get(i).and_then(|v| v.as_u64()).unwrap_or(0);
            if a > b {
                sol_spent.push((*key, a - b));
            }
        }
    }
    (credits, debits, sol_spent)
}

async fn get_transaction(
    http: &reqwest::Client,
    rpc_url: &str,
    sig: &str,
    commitment: &str,
) -> Result<Option<Value>> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [
            sig,
            {
                "encoding": "base64",
                "commitment": commitment,
                "maxSupportedTransactionVersion": 1
            }
        ]
    });
    let resp: Value = http
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("getTransaction http")?
        .json()
        .await
        .context("getTransaction json")?;
    if let Some(err) = resp.get("error") {
        anyhow::bail!("getTransaction: {err}");
    }
    Ok(match resp.get("result") {
        Some(v) if v.is_null() => None,
        Some(v) => Some(v.clone()),
        None => None,
    })
}

async fn fetch_tx_retry(
    http: &reqwest::Client,
    rpc_url: &str,
    sig: &str,
) -> Result<(Option<GeyserTx>, u64)> {
    let t0 = Instant::now();
    for i in 0..FETCH_TRIES {
        let commitment = if i + 2 < FETCH_TRIES {
            "processed"
        } else {
            "confirmed"
        };
        match get_transaction(http, rpc_url, sig, commitment).await {
            Ok(Some(result)) => {
                let ev = geyser_from_rpc_result(&result)
                    .context("parse getTransaction result")?;
                return Ok((Some(ev), t0.elapsed().as_millis() as u64));
            }
            Ok(None) => {
                if i + 1 < FETCH_TRIES {
                    tokio::time::sleep(Duration::from_millis(FETCH_GAP_MS)).await;
                }
            }
            Err(e) => {
                if i + 1 < FETCH_TRIES {
                    tokio::time::sleep(Duration::from_millis(FETCH_GAP_MS)).await;
                } else {
                    return Err(e);
                }
            }
        }
    }
    Ok((None, t0.elapsed().as_millis() as u64))
}

fn spawn_fetch(
    http: reqwest::Client,
    rpc_url: String,
    sig: String,
    slot_hint: u64,
    notified: Instant,
    out: mpsc::Sender<FetchOut>,
    sem: std::sync::Arc<Semaphore>,
) {
    tokio::spawn(async move {
        let Ok(_permit) = sem.acquire().await else {
            return;
        };
        let send = match fetch_tx_retry(&http, &rpc_url, &sig).await {
            Ok((Some(mut ev), fetch_ms)) => {
                if ev.slot == 0 {
                    ev.slot = slot_hint;
                }
                FetchOut::Hit(RpcDetect {
                    ev,
                    signature: sig,
                    notify_to_fetch_ms: 0,
                    fetch_ms,
                    notify_to_tx_ms: notified.elapsed().as_millis() as u64,
                })
            }
            Ok((None, fetch_ms)) => FetchOut::Miss { sig, fetch_ms },
            Err(e) => FetchOut::Fail {
                sig,
                err: format!("{e:#}"),
            },
        };
        let _ = out.send(send).await;
    });
}

fn handle_fetch_out(out: FetchOut, on_ev: &mut impl FnMut(RpcDetect)) {
    match out {
        FetchOut::Hit(det) => on_ev(det),
        FetchOut::Miss { sig, fetch_ms } => {
            eprintln!(
                "rpc_ws miss sig={sig} fetch_ms={fetch_ms} (tx not visible yet)"
            );
        }
        FetchOut::Fail { sig, err } => {
            eprintln!("rpc_ws getTransaction {sig}: {err}");
        }
    }
}

/// Developer-plan watch: one `logsSubscribe` (mentions) per wallet, then `getTransaction`.
pub async fn subscribe_wallets_rpc(
    rpc_url: String,
    wallets: &HashSet<[u8; 32]>,
    on_ev: impl FnMut(RpcDetect),
) -> Result<()> {
    let (_keep, rx) = watch::channel(wallets.clone());
    subscribe_wallets_rpc_watch(rpc_url, rx, on_ev).await
}

/// Same as `subscribe_wallets_rpc`, but resubscribes when the personal watch union changes.
pub async fn subscribe_wallets_rpc_watch(
    rpc_url: String,
    mut wallets_rx: watch::Receiver<HashSet<[u8; 32]>>,
    mut on_ev: impl FnMut(RpcDetect),
) -> Result<()> {
    anyhow::ensure!(!rpc_url.is_empty(), "rpc_url empty — set RPC_URL or HELIUS_API_KEY");
    let ws_url = rpc_ws_url(&rpc_url);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .pool_max_idle_per_host(8)
        .build()?;
    let mut seen: VecDeque<String> = VecDeque::new();
    let mut seen_set: HashSet<String> = HashSet::new();
    let (fetch_tx, mut fetch_rx) = mpsc::channel::<FetchOut>(FETCH_QUEUE);
    let fetch_sem = std::sync::Arc::new(Semaphore::new(MAX_INFLIGHT_FETCH));

    loop {
        let addrs: Vec<String> = {
            let set = wallets_rx.borrow_and_update().clone();
            let mut v: Vec<String> = set.iter().map(encode_pubkey).collect();
            v.sort();
            v
        };
        if addrs.is_empty() {
            eprintln!("rpc_ws idle — waiting for someone to /watch a wallet");
            wallets_rx.changed().await.context("watch set closed")?;
            continue;
        }
        let safe_ws = ws_url.split("api-key=").next().unwrap_or("wss://");
        eprintln!("rpc_ws connecting {safe_ws}…");
        let (mut ws, _) = connect_async(&ws_url)
            .await
            .context("rpc websocket connect")?;
        for (i, addr) in addrs.iter().enumerate() {
            let req = json!({
                "jsonrpc": "2.0",
                "id": i + 1,
                "method": "logsSubscribe",
                "params": [
                    { "mentions": [addr] },
                    { "commitment": "processed" }
                ]
            });
            ws.send(Message::Text(req.to_string().into()))
                .await
                .context("logsSubscribe send")?;
        }
        eprintln!(
            "rpc_ws subscribed wallets={} commitment=processed fetch=base64 async",
            addrs.len()
        );

        let mut ping = tokio::time::interval(Duration::from_secs(25));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut resub = false;
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if ws.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
                changed = wallets_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    resub = true;
                    break;
                }
                out = fetch_rx.recv() => {
                    let Some(out) = out else { break; };
                    handle_fetch_out(out, &mut on_ev);
                }
                msg = ws.next() => {
                    let Some(msg) = msg else { break; };
                    let msg = match msg {
                        Ok(m) => m,
                        Err(_) => break,
                    };
                    let text = match msg {
                        Message::Text(t) => t.to_string(),
                        Message::Ping(p) => {
                            let _ = ws.send(Message::Pong(p)).await;
                            continue;
                        }
                        Message::Close(_) => break,
                        _ => continue,
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let Some(n) = parse_logs_notification(&v) else {
                        continue;
                    };
                    if !seen_set.insert(n.signature.clone()) {
                        continue;
                    }
                    seen.push_back(n.signature.clone());
                    if seen.len() > SIG_CACHE {
                        if let Some(old) = seen.pop_front() {
                            seen_set.remove(&old);
                        }
                    }
                    spawn_fetch(
                        http.clone(),
                        rpc_url.clone(),
                        n.signature,
                        n.slot,
                        Instant::now(),
                        fetch_tx.clone(),
                        fetch_sem.clone(),
                    );
                }
            }
        }
        if resub {
            eprintln!("rpc_ws watch list changed — resubscribe");
            continue;
        }
        eprintln!("rpc_ws disconnected — reconnect in 2s");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::encode_pubkey;
    use base64::engine::general_purpose::STANDARD;

    fn write_short_u16(n: u16, out: &mut Vec<u8>) {
        let mut rem = n as u32;
        loop {
            let mut byte = (rem & 0x7f) as u8;
            rem >>= 7;
            if rem != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if rem == 0 {
                break;
            }
        }
    }

    fn encode_legacy_tx(keys: &[[u8; 32]], ixs: &[CompiledIx], blockhash: [u8; 32]) -> Vec<u8> {
        let mut out = Vec::new();
        write_short_u16(1, &mut out);
        out.extend_from_slice(&[0u8; 64]);
        out.extend_from_slice(&[1, 0, 1]);
        write_short_u16(keys.len() as u16, &mut out);
        for k in keys {
            out.extend_from_slice(k);
        }
        out.extend_from_slice(&blockhash);
        write_short_u16(ixs.len() as u16, &mut out);
        for ix in ixs {
            out.push(ix.program_id_index);
            write_short_u16(ix.accounts.len() as u16, &mut out);
            out.extend_from_slice(&ix.accounts);
            write_short_u16(ix.data.len() as u16, &mut out);
            out.extend_from_slice(&ix.data);
        }
        out
    }

    #[test]
    fn wire_legacy_roundtrip() {
        let keys = [[1u8; 32], [2u8; 32]];
        let ixs = [CompiledIx {
            program_id_index: 1,
            accounts: vec![0],
            data: vec![9, 8, 7],
        }];
        let hash = [3u8; 32];
        let bytes = encode_legacy_tx(&keys, &ixs, hash);
        let (got_keys, got_ixs, got_hash) = parse_solana_tx_wire(&bytes).expect("wire");
        assert_eq!(got_keys, keys);
        assert_eq!(got_ixs[0].program_id_index, 1);
        assert_eq!(got_ixs[0].data, vec![9, 8, 7]);
        assert_eq!(got_hash, hash);
    }

    #[test]
    fn geyser_from_base64_rpc_result() {
        let keys = [[7u8; 32], [8u8; 32]];
        let ixs = [CompiledIx {
            program_id_index: 1,
            accounts: vec![0],
            data: vec![1],
        }];
        let hash = [4u8; 32];
        let b64 = STANDARD.encode(encode_legacy_tx(&keys, &ixs, hash));
        let extra = encode_pubkey(&[5u8; 32]);
        let result = json!({
            "slot": 99,
            "transaction": [b64, "base64"],
            "meta": {
                "loadedAddresses": { "writable": [extra], "readonly": [] },
                "innerInstructions": []
            }
        });
        let ev = geyser_from_rpc_result(&result).expect("b64 rpc");
        assert_eq!(ev.slot, 99);
        assert_eq!(ev.raw.account_keys.len(), 3);
        assert_eq!(ev.raw.account_keys[2], [5u8; 32]);
        assert_eq!(ev.blockhash, hash);
    }
}
