use anyhow::{Context, Result};
use base64::Engine;
use serde_json::{json, Value};

pub const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68TJvxLQar4uLukomCqGjqWNsIUEvn2iA8",
];

pub fn pick_tip_account() -> [u8; 32] {
    crate::tip::pick_account(JITO_TIP_ACCOUNTS)
}

const JITO_REGIONS: &[&str] = &[
    "https://mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://amsterdam.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://frankfurt.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://tokyo.mainnet.block-engine.jito.wtf/api/v1/bundles",
];

#[derive(Clone)]
pub struct JitoClient {
    http: reqwest::Client,
    urls: Vec<String>,
}

impl JitoClient {
    pub fn new(http: reqwest::Client, url: String) -> Self {
        let mut urls: Vec<String> = JITO_REGIONS.iter().map(|s| (*s).to_string()).collect();
        if !url.is_empty() && !urls.iter().any(|u| u == &url) {
            urls.insert(0, url);
        }
        Self { http, urls }
    }

    pub fn fire_bundle(&self, txs_b64: &[String]) {
        if txs_b64.is_empty() || self.urls.is_empty() {
            return;
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [txs_b64, { "encoding": "base64" }]
        });
        for url in &self.urls {
            let http = self.http.clone();
            let url = url.clone();
            let body = body.clone();
            tokio::spawn(async move {
                let _ = http.post(url).json(&body).send().await;
            });
        }
    }

    pub async fn send_bundle(&self, txs_b64: &[String]) -> Result<String> {
        anyhow::ensure!(!self.urls.is_empty(), "jito url empty");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [
                txs_b64,
                { "encoding": "base64" }
            ]
        });
        let res = self
            .http
            .post(&self.urls[0])
            .json(&body)
            .send()
            .await
            .context("jito http")?;
        let v: Value = res.json().await.context("jito json")?;
        if let Some(err) = v.get("error") {
            anyhow::bail!("jito: {err}");
        }
        v.get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .context("jito result")
    }
}

pub fn first_sig_b58(b64: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let (n, nlen) = crate::swap::compact_u16(&raw)?;
    if n == 0 {
        return None;
    }
    let start = nlen;
    let end = start.checked_add(64)?;
    let sig = raw.get(start..end)?;
    Some(bs58::encode(sig).into_string())
}

pub fn encode_tx_b64(raw: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(raw)
}
