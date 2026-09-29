use crate::decode::{pump_program, BUY_DISC};
use crate::types::{CompiledIx, PumpBuy, RawTx};

pub const TIP_PLACEHOLDER: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 1,
];

pub const SYSTEM_PROGRAM: [u8; 32] = [0u8; 32];
pub const MIN_TIP_LAMPORTS: u64 = 1_000_000;

/// Unsigned paper message. No Solana SDK types on the hot path.
#[derive(Clone, Debug)]
pub struct PaperMessage {
    pub account_keys: Vec<[u8; 32]>,
    pub recent_blockhash: [u8; 32],
    pub instructions: Vec<CompiledIx>,
}

impl PaperMessage {
    /// Same copy buy, new bribe. Finds the SystemProgram tip transfer (not CU budget).
    pub fn with_tip(&self, tip_lamports: u64, blockhash: [u8; 32]) -> Self {
        let mut out = self.clone();
        out.recent_blockhash = blockhash;
        for ix in out.instructions.iter_mut() {
            if ix.accounts.len() == 2 && ix.data.len() >= 12 {
                let tag = u32::from_le_bytes(ix.data[0..4].try_into().unwrap_or([0; 4]));
                if tag == 2 {
                    ix.data[4..12].copy_from_slice(&tip_lamports.to_le_bytes());
                    break;
                }
            }
        }
        out
    }

    pub fn byte_len(&self) -> usize {
        32 + self.account_keys.len() * 32
            + self
                .instructions
                .iter()
                .map(|ix| 2 + ix.accounts.len() + ix.data.len())
                .sum::<usize>()
    }
}

/// Assemble an unsigned paper copy buy. Does not sign or send.
pub fn build_paper_message(
    tx: &RawTx,
    buy: &PumpBuy,
    our_pubkey: &[u8; 32],
    copy_sol_lamports: u64,
    tip_lamports: u64,
    blockhash: [u8; 32],
) -> Option<PaperMessage> {
    let src = tx.ixs.get(buy.ix_index)?;
    let mut account_keys: Vec<[u8; 32]> = Vec::with_capacity(src.accounts.len() + 2);
    account_keys.push(*our_pubkey);
    account_keys.push(TIP_PLACEHOLDER);
    account_keys.push(SYSTEM_PROGRAM);
    account_keys.push(pump_program());

    let mut pump_accounts = Vec::with_capacity(src.accounts.len());
    for (n, acc_i) in src.accounts.iter().enumerate() {
        let key = if n == 6 {
            *our_pubkey
        } else {
            *tx.account_keys.get(*acc_i as usize)?
        };
        let idx = push_key(&mut account_keys, key) as u8;
        pump_accounts.push(idx);
    }

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&BUY_DISC);
    data.extend_from_slice(&buy.token_amount.to_le_bytes());
    data.extend_from_slice(&copy_sol_lamports.to_le_bytes());

    let mut tip_data = Vec::with_capacity(12);
    tip_data.extend_from_slice(&2u32.to_le_bytes());
    tip_data.extend_from_slice(&tip_lamports.to_le_bytes());

    Some(PaperMessage {
        account_keys,
        recent_blockhash: blockhash,
        instructions: vec![
            CompiledIx {
                program_id_index: 2,
                accounts: vec![0, 1],
                data: tip_data,
            },
            CompiledIx {
                program_id_index: 3,
                accounts: pump_accounts,
                data,
            },
        ],
    })
}

fn push_key(keys: &mut Vec<[u8; 32]>, key: [u8; 32]) -> usize {
    if let Some((i, _)) = keys.iter().enumerate().find(|(_, k)| **k == key) {
        return i;
    }
    keys.push(key);
    keys.len() - 1
}

/// One account layout, three bribes.
pub fn build_tip_burst(
    tx: &RawTx,
    buy: &PumpBuy,
    our_pubkey: &[u8; 32],
    copy_sol_lamports: u64,
    tips: [u64; 3],
    blockhash: [u8; 32],
) -> Option<[PaperMessage; 3]> {
    let base = build_paper_message(tx, buy, our_pubkey, copy_sol_lamports, tips[0], blockhash)?;
    Some([
        base.clone(),
        base.with_tip(tips[1], blockhash),
        base.with_tip(tips[2], blockhash),
    ])
}

pub fn paper_bytes_len(msg: &PaperMessage) -> usize {
    msg.byte_len()
}

pub fn tip_lamports_of(msg: &PaperMessage) -> Option<u64> {
    let data = msg.instructions.first()?.data.as_slice();
    if data.len() < 12 {
        return None;
    }
    Some(u64::from_le_bytes(data[4..12].try_into().ok()?))
}
