use crate::types::{CompiledIx, PumpBuy, RawTx, TargetEvent, Venue};

/// Pump.fun program: `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`
pub fn pump_program() -> [u8; 32] {
    b58_once("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
}

pub fn pumpswap_program() -> [u8; 32] {
    b58_once("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")
}

pub fn raydium_amm_program() -> [u8; 32] {
    b58_once("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8")
}

pub fn raydium_cpmm_program() -> [u8; 32] {
    b58_once("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C")
}

pub fn token_program() -> [u8; 32] {
    b58_once("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
}

pub fn token_2022_program() -> [u8; 32] {
    b58_once("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
}

fn b58_once(s: &'static str) -> [u8; 32] {
    use std::sync::OnceLock;
    static MAP: OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, [u8; 32]>>> =
        OnceLock::new();
    let map = MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = map.lock().expect("b58");
    *g.entry(s).or_insert_with(|| {
        let raw = bs58::decode(s).into_vec().expect("program b58");
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw);
        out
    })
}

/// Anchor `global:buy`
pub const BUY_DISC: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];
/// Anchor `global:buy_exact_sol_in` (current Pump.fun / Bloom inner ix)
pub const BUY_EXACT_SOL_IN_DISC: [u8; 8] = [0x38, 0xfc, 0x74, 0x08, 0x9e, 0xdf, 0xcd, 0x5f];
/// Anchor `global:buy_exact_quote_in` (PumpSwap AMM)
pub const BUY_EXACT_QUOTE_IN_DISC: [u8; 8] = [0xc6, 0x2e, 0x15, 0x52, 0xb4, 0xd9, 0xe8, 0x70];
/// Anchor `global:sell`
pub const SELL_DISC: [u8; 8] = [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];
/// Anchor `global:create`
pub const CREATE_DISC: [u8; 8] = [0x18, 0x1e, 0xc8, 0x28, 0x05, 0x1c, 0x07, 0x77];

const BUY_DATA_LEN: usize = 8 + 8 + 8;
const MINT_ACC: usize = 2;
const USER_ACC: usize = 6;

const SPL_TRANSFER: u8 = 3;
const SPL_TRANSFER_CHECKED: u8 = 12;

fn program_of(tx: &RawTx, ix: &CompiledIx) -> Option<[u8; 32]> {
    tx.account_keys.get(ix.program_id_index as usize).copied()
}

/// Decode a Pump.fun buy. Sells and unknown programs return `None`.
#[inline]
pub fn decode_pump_buy(tx: &RawTx) -> Option<PumpBuy> {
    match classify_tx(tx)? {
        TargetEvent::Buy {
            wallet,
            mint,
            token_amount,
            sol_lamports,
            ix_index,
            venue: Venue::PumpFun,
            ..
        } => Some(PumpBuy {
            wallet,
            mint,
            token_amount,
            max_sol_lamports: sol_lamports,
            ix_index,
        }),
        _ => None,
    }
}

pub fn classify_tx(tx: &RawTx) -> Option<TargetEvent> {
    let pump = pump_program();
    let pswap = pumpswap_program();
    let ray = raydium_amm_program();
    let cpmm = raydium_cpmm_program();
    let tok = token_program();
    let tok22 = token_2022_program();

    let mut create: Option<TargetEvent> = None;
    let mut buy: Option<TargetEvent> = None;
    let mut sell: Option<TargetEvent> = None;
    let mut transfer: Option<TargetEvent> = None;

    for (ix_index, ix) in tx.ixs.iter().enumerate() {
        let Some(pid) = program_of(tx, ix) else {
            continue;
        };
        let data = ix.data.as_slice();
        if pid == pump || pid == pswap {
            if data.len() >= 8 && data[..8] == CREATE_DISC {
                if let Some(ev) = pump_create(tx, ix, ix_index) {
                    create = Some(ev);
                }
            }
            if data.len() >= BUY_DATA_LEN && is_pump_buy_disc(data) {
                if let Some(ev) = pump_like_buy(tx, ix, ix_index, venue_of(pid, pump, pswap)) {
                    buy = Some(ev);
                }
            }
            if data.len() >= 8 && data[..8] == SELL_DISC {
                if let Some(ev) = pump_like_sell(tx, ix, ix_index, venue_of(pid, pump, pswap)) {
                    sell = Some(ev);
                }
            }
        } else if pid == ray || pid == cpmm {
            if let Some(ev) = raydium_swap(tx, ix, ix_index) {
                match ev {
                    TargetEvent::Buy { .. } => buy = Some(ev),
                    TargetEvent::Sell { .. } => sell = Some(ev),
                    _ => {}
                }
            }
        } else if pid == tok || pid == tok22 {
            if let Some(ev) = spl_transfer(tx, ix, ix_index) {
                transfer = Some(ev);
            }
        }
    }

    if let Some(s) = sell {
        return Some(s);
    }
    if let Some(mut b) = buy {
        if let TargetEvent::Buy {
            mint,
            ref mut is_deployer,
            ..
        } = b
        {
            if let Some(c) = &create {
                if c.mint() == mint {
                    *is_deployer = true;
                }
            }
        }
        return Some(b);
    }
    if let Some(t) = transfer {
        return Some(t);
    }
    create
}

fn venue_of(pid: [u8; 32], pump: [u8; 32], pswap: [u8; 32]) -> Venue {
    if pid == pswap {
        Venue::PumpSwap
    } else if pid == pump {
        Venue::PumpFun
    } else {
        Venue::Raydium
    }
}

fn is_pump_buy_disc(data: &[u8]) -> bool {
    data.len() >= 8
        && (data[..8] == BUY_DISC
            || data[..8] == BUY_EXACT_SOL_IN_DISC
            || data[..8] == BUY_EXACT_QUOTE_IN_DISC)
}

fn pump_user_and_mint(venue: Venue, nacc: usize) -> Option<(usize, usize)> {
    match venue {
        Venue::PumpSwap => {
            if nacc < 16 {
                return None;
            }
            Some((1, 3))
        }
        _ => {
            if nacc <= USER_ACC {
                return None;
            }
            Some((USER_ACC, MINT_ACC))
        }
    }
}

fn pump_like_buy(tx: &RawTx, ix: &CompiledIx, ix_index: usize, venue: Venue) -> Option<TargetEvent> {
    let (user_i, mint_i) = pump_user_and_mint(venue, ix.accounts.len())?;
    let mint = *tx.account_keys.get(*ix.accounts.get(mint_i)? as usize)?;
    let wallet = *tx.account_keys.get(*ix.accounts.get(user_i)? as usize)?;
    let data = ix.data.as_slice();
    let (token_amount, sol_lamports) = if data[..8] == BUY_EXACT_SOL_IN_DISC
        || data[..8] == BUY_EXACT_QUOTE_IN_DISC
    {
        (
            u64::from_le_bytes(data[16..24].try_into().ok()?),
            u64::from_le_bytes(data[8..16].try_into().ok()?),
        )
    } else {
        (
            u64::from_le_bytes(data[8..16].try_into().ok()?),
            u64::from_le_bytes(data[16..24].try_into().ok()?),
        )
    };
    Some(TargetEvent::Buy {
        wallet,
        mint,
        token_amount,
        sol_lamports,
        ix_index,
        venue,
        is_deployer: false,
    })
}

fn pump_like_sell(tx: &RawTx, ix: &CompiledIx, ix_index: usize, venue: Venue) -> Option<TargetEvent> {
    let (user_i, mint_i) = pump_user_and_mint(venue, ix.accounts.len())?;
    let mint = *tx.account_keys.get(*ix.accounts.get(mint_i)? as usize)?;
    let wallet = *tx.account_keys.get(*ix.accounts.get(user_i)? as usize)?;
    let data = ix.data.as_slice();
    let token_amount = if data.len() >= 16 {
        u64::from_le_bytes(data[8..16].try_into().ok()?)
    } else {
        0
    };
    Some(TargetEvent::Sell {
        wallet,
        mint,
        token_amount,
        ix_index,
        venue,
    })
}

fn pump_create(tx: &RawTx, ix: &CompiledIx, ix_index: usize) -> Option<TargetEvent> {
    let mint_i = *ix.accounts.get(0)? as usize;
    let user_i = *ix.accounts.last()? as usize;
    let mint = *tx.account_keys.get(mint_i)?;
    let wallet = *tx.account_keys.get(user_i)?;
    Some(TargetEvent::Create {
        wallet,
        mint,
        ix_index,
    })
}

fn raydium_swap(tx: &RawTx, ix: &CompiledIx, ix_index: usize) -> Option<TargetEvent> {
    let data = ix.data.as_slice();
    if data.is_empty() {
        return None;
    }
    let amount_in = if data[0] == 9 && data.len() >= 17 {
        u64::from_le_bytes(data[1..9].try_into().ok()?)
    } else if data.len() >= 24 && data[..8] == BUY_DISC {
        u64::from_le_bytes(data[16..24].try_into().ok()?)
    } else {
        return None;
    };
    let wallet = *tx.account_keys.get(*ix.accounts.first()? as usize)?;
    let mint = *tx.account_keys.get(*ix.accounts.get(1)? as usize)?;
    // Heuristic: lamport-scale amounts are buys; huge token amounts are sells.
    if amount_in > 0 && amount_in < 50_000_000_000 {
        Some(TargetEvent::Buy {
            wallet,
            mint,
            token_amount: 0,
            sol_lamports: amount_in,
            ix_index,
            venue: Venue::Raydium,
            is_deployer: false,
        })
    } else {
        Some(TargetEvent::Sell {
            wallet,
            mint,
            token_amount: amount_in,
            ix_index,
            venue: Venue::Raydium,
        })
    }
}

fn spl_transfer(tx: &RawTx, ix: &CompiledIx, ix_index: usize) -> Option<TargetEvent> {
    let data = ix.data.as_slice();
    if data.is_empty() {
        return None;
    }
    let (tag, amount) = if data[0] == SPL_TRANSFER && data.len() >= 9 {
        (
            SPL_TRANSFER,
            u64::from_le_bytes(data[1..9].try_into().ok()?),
        )
    } else if data[0] == SPL_TRANSFER_CHECKED && data.len() >= 10 {
        (
            SPL_TRANSFER_CHECKED,
            u64::from_le_bytes(data[1..9].try_into().ok()?),
        )
    } else {
        return None;
    };
    let _ = tag;
    // Transfer: source, dest, owner. TransferChecked: source, mint, dest, owner.
    let wallet = if data[0] == SPL_TRANSFER_CHECKED {
        *tx.account_keys.get(*ix.accounts.get(3)? as usize)?
    } else {
        *tx.account_keys.get(*ix.accounts.get(2)? as usize)?
    };
    let mint = if data[0] == SPL_TRANSFER_CHECKED {
        *tx.account_keys.get(*ix.accounts.get(1)? as usize)?
    } else {
        [0u8; 32]
    };
    Some(TargetEvent::Transfer {
        wallet,
        mint,
        token_amount: amount,
        ix_index,
    })
}

/// Fast program-id probe used by benches.
#[inline]
pub fn has_pump_program(tx: &RawTx) -> bool {
    tx.ixs.iter().any(|ix| {
        tx.account_keys
            .get(ix.program_id_index as usize)
            .is_some_and(|k| *k == pump_program())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_32_bytes() {
        assert_eq!(pump_program().len(), 32);
        assert_ne!(pumpswap_program(), pump_program());
    }

    #[test]
    fn create_then_buy_is_deployer() {
        let tx = crate::fixture::pump_create_then_buy_tx(1_000_000, 10);
        let ev = classify_tx(&tx).expect("buy");
        match ev {
            TargetEvent::Buy { is_deployer, .. } => assert!(is_deployer),
            _ => panic!("expected buy"),
        }
    }

    #[test]
    fn buy_exact_sol_in_is_a_copyable_buy() {
        let mut tx = crate::fixture::pump_buy_tx(500_000_000, 99);
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_EXACT_SOL_IN_DISC);
        data.extend_from_slice(&500_000_000u64.to_le_bytes());
        data.extend_from_slice(&12u64.to_le_bytes());
        tx.ixs[0].data = data;
        let ev = classify_tx(&tx).expect("buy");
        match ev {
            TargetEvent::Buy {
                sol_lamports,
                token_amount,
                wallet,
                is_deployer,
                ..
            } => {
                assert_eq!(sol_lamports, 500_000_000);
                assert_eq!(token_amount, 12);
                assert_eq!(wallet, crate::fixture::WALLET_A);
                assert!(!is_deployer);
            }
            _ => panic!("expected buy"),
        }
        assert!(decode_pump_buy(&tx).is_some());
    }

    #[test]
    fn pumpswap_buy_uses_user_index_1() {
        let mut keys = vec![[0u8; 32]; 17];
        keys[1] = crate::fixture::WALLET_A;
        keys[3] = crate::fixture::MINT_A;
        keys[6] = [9u8; 32];
        keys[16] = pumpswap_program();
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_DISC);
        data.extend_from_slice(&11u64.to_le_bytes());
        data.extend_from_slice(&22_000_000u64.to_le_bytes());
        let tx = RawTx {
            account_keys: keys,
            ixs: vec![CompiledIx {
                program_id_index: 16,
                accounts: (0u8..17).collect(),
                data,
            }],
        };
        match classify_tx(&tx).expect("buy") {
            TargetEvent::Buy {
                wallet,
                mint,
                sol_lamports,
                venue,
                ..
            } => {
                assert_eq!(wallet, crate::fixture::WALLET_A);
                assert_eq!(mint, crate::fixture::MINT_A);
                assert_eq!(sol_lamports, 22_000_000);
                assert_eq!(venue, Venue::PumpSwap);
            }
            _ => panic!("expected pumpswap buy"),
        }
    }
}
