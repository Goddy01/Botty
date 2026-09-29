use crate::rpc_read::RpcRead;
use crate::types::{CompiledIx, GeyserTx, RawTx};
use anyhow::Result;
use std::collections::HashSet;
use tokio::sync::watch;

pub fn raw_from_account_keys_and_ixs(
    account_keys: Vec<[u8; 32]>,
    instructions: impl IntoIterator<Item = (u8, Vec<u8>, Vec<u8>)>,
) -> RawTx {
    RawTx {
        account_keys,
        ixs: instructions
            .into_iter()
            .map(|(program_id_index, accounts, data)| CompiledIx {
                program_id_index,
                accounts,
                data,
            })
            .collect(),
    }
}

/// Refresh recent blockhash from RPC so live txs can land.
pub async fn refresh_blockhash(rpc_url: String, tx: watch::Sender<[u8; 32]>) {
    if rpc_url.is_empty() {
        return;
    }
    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let rpc = RpcRead::new(http, rpc_url);
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(400));
    loop {
        interval.tick().await;
        if let Ok(h) = rpc.latest_blockhash().await {
            let _ = tx.send(h);
        }
    }
}

#[cfg(feature = "laserstream")]
mod grpc {
    use super::*;
    use anyhow::Context;
    use futures_util::StreamExt;
    use std::time::Duration;
    use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
    use yellowstone_grpc_proto::geyser::{
        subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
        SubscribeRequestFilterSlots, SubscribeRequestFilterTransactions, SubscribeUpdate,
    };

    /// Yellowstone / LaserStream (Geyser) → compact `GeyserTx`. No parsed-tx HTTP.
    pub fn parse_subscribe_update(update: &SubscribeUpdate) -> Option<GeyserTx> {
        let UpdateOneof::Transaction(tx_update) = update.update_oneof.as_ref()? else {
            return None;
        };
        let info = tx_update.transaction.as_ref()?;
        let inner = info.transaction.as_ref()?;
        let msg = inner.message.as_ref()?;

        let mut keys = Vec::with_capacity(msg.account_keys.len());
        for k in &msg.account_keys {
            if k.len() != 32 {
                return None;
            }
            let mut a = [0u8; 32];
            a.copy_from_slice(k);
            keys.push(a);
        }

        let mut ixs = Vec::with_capacity(msg.instructions.len());
        for ix in &msg.instructions {
            ixs.push(CompiledIx {
                program_id_index: ix.program_id_index as u8,
                accounts: ix.accounts.clone(),
                data: ix.data.clone(),
            });
        }

        let mut blockhash = [0u8; 32];
        if msg.recent_blockhash.len() == 32 {
            blockhash.copy_from_slice(&msg.recent_blockhash);
        }

        Some(GeyserTx {
            raw: RawTx {
                account_keys: keys,
                ixs,
            },
            slot: tx_update.slot,
            blockhash,
            token_credits: Vec::new(),
            token_debits: Vec::new(),
            sol_spent: Vec::new(),
        })
    }

    pub async fn subscribe_wallets(
        endpoint: String,
        x_token: String,
        wallets: &HashSet<[u8; 32]>,
        mut on_ev: impl FnMut(GeyserTx),
    ) -> Result<()> {
        let mut client = GeyserGrpcClient::build_from_shared(endpoint)?
            .x_token(Some(x_token))?
            .tls_config(ClientTlsConfig::new().with_native_roots())?
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .http2_keep_alive_interval(Duration::from_secs(15))
            .keep_alive_while_idle(true)
            .http2_adaptive_window(true)
            .max_decoding_message_size(32 * 1024 * 1024)
            .connect()
            .await
            .context("laserstream / geyser connect")?;

        let include: Vec<String> = wallets.iter().map(crate::config::encode_pubkey).collect();
        let mut request = SubscribeRequest::default();
        // Processed = Geyser after the node executes, not confirmed. Required for Business speed.
        request.commitment = Some(CommitmentLevel::Processed as i32);
        request.slots.insert(
            "slots".into(),
            SubscribeRequestFilterSlots {
                filter_by_commitment: Some(true),
                ..Default::default()
            },
        );
        request.transactions.insert(
            "wallets".into(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                account_include: include,
                ..Default::default()
            },
        );

        let (_sink, mut stream) = client.subscribe_with_request(Some(request)).await?;
        while let Some(item) = stream.next().await {
            let update = item.context("geyser item")?;
            if let Some(ev) = parse_subscribe_update(&update) {
                on_ev(ev);
            }
        }
        Ok(())
    }
}

#[cfg(feature = "laserstream")]
pub use grpc::{parse_subscribe_update, subscribe_wallets};

#[cfg(not(feature = "laserstream"))]
pub async fn subscribe_wallets(
    _endpoint: String,
    _x_token: String,
    _wallets: &HashSet<[u8; 32]>,
    _on_ev: impl FnMut(GeyserTx),
) -> Result<()> {
    anyhow::bail!(
        "on the Frankfurt VPS run: cargo run --release --features laserstream"
    )
}
