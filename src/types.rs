use serde::{Deserialize, Serialize};

/// Compact compiled instruction. Indices refer to `RawTx.account_keys`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledIx {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

/// Transaction shape used by decode / risk / build. No RPC types on the hot path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawTx {
    pub account_keys: Vec<[u8; 32]>,
    pub ixs: Vec<CompiledIx>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PumpBuy {
    pub wallet: [u8; 32],
    pub mint: [u8; 32],
    pub token_amount: u64,
    pub max_sol_lamports: u64,
    pub ix_index: usize,
}

#[derive(Clone, Debug, Default)]
pub struct TokenCredit {
    pub owner: [u8; 32],
    pub mint: [u8; 32],
    pub amount: u64,
}

/// One LaserStream / Geyser transaction after local decode. No HTTP.
#[derive(Clone, Debug)]
pub struct GeyserTx {
    pub raw: RawTx,
    pub slot: u64,
    pub blockhash: [u8; 32],
    pub token_credits: Vec<TokenCredit>,
    pub token_debits: Vec<TokenCredit>,
    pub sol_spent: Vec<([u8; 32], u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Venue {
    PumpFun,
    PumpSwap,
    Raydium,
    Router,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetEvent {
    Buy {
        wallet: [u8; 32],
        mint: [u8; 32],
        token_amount: u64,
        sol_lamports: u64,
        ix_index: usize,
        venue: Venue,
        is_deployer: bool,
    },
    Sell {
        wallet: [u8; 32],
        mint: [u8; 32],
        token_amount: u64,
        ix_index: usize,
        venue: Venue,
    },
    Transfer {
        wallet: [u8; 32],
        mint: [u8; 32],
        token_amount: u64,
        ix_index: usize,
    },
    Create {
        wallet: [u8; 32],
        mint: [u8; 32],
        ix_index: usize,
    },
}

impl TargetEvent {
    pub fn wallet(&self) -> [u8; 32] {
        match *self {
            TargetEvent::Buy { wallet, .. }
            | TargetEvent::Sell { wallet, .. }
            | TargetEvent::Transfer { wallet, .. }
            | TargetEvent::Create { wallet, .. } => wallet,
        }
    }

    pub fn mint(&self) -> [u8; 32] {
        match *self {
            TargetEvent::Buy { mint, .. }
            | TargetEvent::Sell { mint, .. }
            | TargetEvent::Transfer { mint, .. }
            | TargetEvent::Create { mint, .. } => mint,
        }
    }

    pub fn ix_index(&self) -> usize {
        match *self {
            TargetEvent::Buy { ix_index, .. }
            | TargetEvent::Sell { ix_index, .. }
            | TargetEvent::Transfer { ix_index, .. }
            | TargetEvent::Create { ix_index, .. } => ix_index,
        }
    }
}
