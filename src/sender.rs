use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Helius Sender tip accounts (mainnet). Required for Sender Max pathways.
pub const SENDER_TIP_ACCOUNTS: &[&str] = &[
    "4ACfpUFoaSD9bfPdeu6DBt89gB6ENTeHBXCAi87NhDEE",
    "D2L6yPZ2FmmmTKPgzaMKdhu6EWZcTpLy1Vhx8uvZe7NZ",
    "9bnz4RShgq1hAnLnZbP8kbgBg1kEmcJBYQq3gQbmnSta",
    "5VY91ws6B2hMmBFRsXkoAAdsPHBJwRfBht4DXox3xkwn",
    "2nyhqdwKcJZR2vcqCyrYsaPVdAnFoJjiksCXJ7hfEYgD",
    "2q5pghRs6arqVjRvT5gfgWfWcHWmw1ZuCzphgd5KfWGJ",
    "wyvPkWjVZz1M8fHQnMMCDTQDbkManefNNhweYk5WkcF",
    "3KCKozbAaF75qEU33jtzozcJ29yJuaLJTy2jFdzUY8bT",
    "4vieeGHPYPG2MmyPRcYjdiDmmhN3ww7hsFNap8pVN3Ey",
    "4TQLFNWK8AovT1gFvda5jfw2oJeRMKEmw7aH6MGBJ3or",
];

const SENDER_ENDPOINTS: &[&str] = &[
    "http://fra-sender.helius-rpc.com/fast",
    "http://ams-sender.helius-rpc.com/fast",
    "https://sender.helius-rpc.com/fast",
];

const SENDER_PING: &[&str] = &[
    "http://fra-sender.helius-rpc.com/ping",
    "http://ams-sender.helius-rpc.com/ping",
    "https://sender.helius-rpc.com/ping",
];

#[derive(Clone)]
pub struct HeliusSender {
    http: reqwest::Client,
}

impl HeliusSender {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub fn spawn_ping_loop(&self) {
        let http = self.http.clone();
        tokio::spawn(async move {
            let mut i = 0usize;
            loop {
                let url = SENDER_PING[i % SENDER_PING.len()];
                i = i.wrapping_add(1);
                let _ = http.get(url).send().await;
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            }
        });
    }

    pub fn fire_tx(&self, b64: &str) {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                b64,
                { "encoding": "base64", "skipPreflight": true, "maxRetries": 0 }
            ]
        });
        for url in SENDER_ENDPOINTS {
            let http = self.http.clone();
            let url = (*url).to_string();
            let body = body.clone();
            tokio::spawn(async move {
                let _ = http.post(url).json(&body).send().await;
            });
        }
    }

    pub fn fire_many(&self, txs_b64: &[String]) {
        for t in txs_b64 {
            self.fire_tx(t);
        }
    }

    pub async fn send_tx(&self, b64: &str) -> Result<String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                b64,
                { "encoding": "base64", "skipPreflight": true, "maxRetries": 0 }
            ]
        });
        let res = self
            .http
            .post(SENDER_ENDPOINTS[0])
            .json(&body)
            .send()
            .await
            .context("sender http")?;
        let v: Value = res.json().await.context("sender json")?;
        if let Some(err) = v.get("error") {
            anyhow::bail!("sender: {err}");
        }
        v.get("result")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .context("sender result")
    }
}

pub fn pick_sender_tip_account() -> [u8; 32] {
    crate::tip::pick_account(SENDER_TIP_ACCOUNTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_tip_accounts_decode() {
        let pk = pick_sender_tip_account();
        assert_ne!(pk, [0u8; 32]);
    }
}
