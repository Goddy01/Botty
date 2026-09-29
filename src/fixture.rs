use crate::decode::{pump_program, BUY_DISC};
use crate::types::{CompiledIx, RawTx};

pub const WALLET_A: [u8; 32] = [
    2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    2,
];
pub const MINT_A: [u8; 32] = [
    3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    3,
];
pub const OUR_WALLET: [u8; 32] = [
    4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    4,
];

fn pad(id: u8) -> [u8; 32] {
    let mut a = [0u8; 32];
    a[0] = id;
    a[31] = id;
    a
}

/// Synthetic Pump.fun buy used for replay, tests, and benches.
pub fn pump_buy_tx(sol_lamports: u64, token_amount: u64) -> RawTx {
    let global = pad(10);
    let fee = pad(11);
    let bonding = pad(12);
    let assoc_bond = pad(13);
    let assoc_user = pad(14);
    let system = pad(15);
    let token = pad(16);
    let rent = pad(17);
    let event = pad(18);

    let account_keys = vec![
        global,
        fee,
        MINT_A,
        bonding,
        assoc_bond,
        assoc_user,
        WALLET_A,
        system,
        token,
        rent,
        event,
        pump_program(),
    ];

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&BUY_DISC);
    data.extend_from_slice(&token_amount.to_le_bytes());
    data.extend_from_slice(&sol_lamports.to_le_bytes());

    RawTx {
        account_keys,
        ixs: vec![CompiledIx {
            program_id_index: 11,
            accounts: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            data,
        }],
    }
}

pub fn pump_sell_tx() -> RawTx {
    let mut tx = pump_buy_tx(1_000_000, 1);
    tx.ixs[0].data = crate::decode::SELL_DISC.to_vec();
    tx.ixs[0].data.extend_from_slice(&1u64.to_le_bytes());
    tx.ixs[0].data.extend_from_slice(&1u64.to_le_bytes());
    tx
}

pub fn pump_create_then_buy_tx(sol_lamports: u64, token_amount: u64) -> RawTx {
    let mut tx = pump_buy_tx(sol_lamports, token_amount);
    tx.ixs.insert(
        0,
        CompiledIx {
            program_id_index: 11,
            accounts: vec![2, 6],
            data: crate::decode::CREATE_DISC.to_vec(),
        },
    );
    tx
}

pub fn spl_transfer_tx() -> RawTx {
    use crate::decode::token_program;
    let mut data = vec![3u8];
    data.extend_from_slice(&100u64.to_le_bytes());
    RawTx {
        account_keys: vec![pad(20), pad(21), WALLET_A, token_program(), MINT_A],
        ixs: vec![CompiledIx {
            program_id_index: 3,
            accounts: vec![0, 1, 2],
            data,
        }],
    }
}

pub fn unrelated_tx() -> RawTx {
    RawTx {
        account_keys: vec![WALLET_A, pad(99)],
        ixs: vec![CompiledIx {
            program_id_index: 1,
            accounts: vec![0],
            data: vec![1, 2, 3],
        }],
    }
}
