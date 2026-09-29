use crate::build::{PaperMessage, SYSTEM_PROGRAM};
use crate::decode::{
    pumpswap_program, pump_program, raydium_amm_program, raydium_cpmm_program, token_2022_program,
    token_program, BUY_DISC, BUY_EXACT_QUOTE_IN_DISC, BUY_EXACT_SOL_IN_DISC, SELL_DISC,
};
use crate::jito::pick_tip_account;
use crate::types::{CompiledIx, RawTx, TargetEvent, Venue};
use anyhow::{Context, Result};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

fn wsol_mint() -> [u8; 32] {
    b58("So11111111111111111111111111111111111111112")
}

pub fn associated_token_program() -> [u8; 32] {
    b58("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
}

pub fn token_program_id() -> [u8; 32] {
    b58("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
}

fn b58(s: &str) -> [u8; 32] {
    let raw = bs58::decode(s).into_vec().expect("b58");
    let mut out = [0u8; 32];
    out.copy_from_slice(&raw);
    out
}

pub fn find_pda(seeds: &[&[u8]], program: &[u8; 32]) -> [u8; 32] {
    let mut bump = 255u8;
    loop {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program);
        h.update(b"ProgramDerivedAddress");
        let hash: [u8; 32] = h.finalize().into();
        if ed25519_dalek::VerifyingKey::from_bytes(&hash).is_err() {
            return hash;
        }
        if bump == 0 {
            return hash;
        }
        bump -= 1;
    }
}

pub fn ata(owner: &[u8; 32], mint: &[u8; 32]) -> [u8; 32] {
    ata_with_program(owner, mint, &token_program_id())
}

pub fn ata_with_program(owner: &[u8; 32], mint: &[u8; 32], token_program: &[u8; 32]) -> [u8; 32] {
    find_pda(
        &[owner.as_slice(), token_program.as_slice(), mint.as_slice()],
        &associated_token_program(),
    )
}

pub fn pump_global() -> [u8; 32] {
    b58("4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf")
}

pub fn pump_fee_recipient() -> [u8; 32] {
    b58("CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM")
}

pub fn pump_event_authority() -> [u8; 32] {
    find_pda(&[b"__event_authority"], &pump_program())
}

pub fn bonding_curve_pda(mint: &[u8; 32]) -> [u8; 32] {
    find_pda(&[b"bonding-curve", mint.as_slice()], &pump_program())
}

pub fn mpl_metadata_program() -> [u8; 32] {
    b58("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s")
}

pub fn mpl_metadata_pda(mint: &[u8; 32]) -> [u8; 32] {
    let prog = mpl_metadata_program();
    find_pda(
        &[b"metadata", prog.as_slice(), mint.as_slice()],
        &prog,
    )
}

pub fn creator_vault_pda(creator: &[u8; 32]) -> [u8; 32] {
    find_pda(&[b"creator-vault", creator.as_slice()], &pump_program())
}

/// Pump.fun sell/buy account list so a strategy sell can be built without a live copied tx.
pub fn pump_fun_swap_template(mint: [u8; 32], user: [u8; 32], creator: [u8; 32]) -> (RawTx, TargetEvent) {
    let bonding = bonding_curve_pda(&mint);
    let account_keys = vec![
        pump_global(),
        pump_fee_recipient(),
        mint,
        bonding,
        ata(&bonding, &mint),
        ata(&user, &mint),
        user,
        SYSTEM_PROGRAM,
        creator_vault_pda(&creator),
        token_program_id(),
        pump_event_authority(),
        pump_program(),
    ];
    let ev = TargetEvent::Buy {
        wallet: user,
        mint,
        token_amount: 0,
        sol_lamports: 0,
        ix_index: 0,
        venue: Venue::PumpFun,
        is_deployer: false,
    };
    (
        RawTx {
            account_keys,
            ixs: vec![CompiledIx {
                program_id_index: 11,
                accounts: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
                data: SELL_DISC.to_vec(),
            }],
        },
        ev,
    )
}

fn venue_program(ev: &TargetEvent) -> [u8; 32] {
    match ev {
        TargetEvent::Buy {
            venue: Venue::PumpSwap,
            ..
        }
        | TargetEvent::Sell {
            venue: Venue::PumpSwap,
            ..
        } => pumpswap_program(),
        TargetEvent::Buy {
            venue: Venue::Raydium,
            ..
        }
        | TargetEvent::Sell {
            venue: Venue::Raydium,
            ..
        } => raydium_amm_program(),
        _ => pump_program(),
    }
}

/// Rewrite leader swap ix so the copier is the user / ATA, plus compute budget + Jito tip.
pub fn build_copy_message(
    tx: &RawTx,
    ev: &TargetEvent,
    our_pubkey: &[u8; 32],
    copy_sol_lamports: u64,
    sell_tokens: u64,
    min_tokens_out: u64,
    min_sol_out: u64,
    tip_lamports: u64,
    priority_microlamports: u64,
    blockhash: [u8; 32],
    buy: bool,
) -> Option<PaperMessage> {
    let src = tx.ixs.get(ev.ix_index())?;
    let mint = ev.mint();
    let token_prog = src
        .accounts
        .iter()
        .filter_map(|i| tx.account_keys.get(*i as usize).copied())
        .find(|k| *k == token_program() || *k == token_2022_program())
        .unwrap_or_else(token_program_id);
    let our_ata = ata_with_program(our_pubkey, &mint, &token_prog);
    let is_pswap = matches!(
        ev,
        TargetEvent::Buy {
            venue: Venue::PumpSwap,
            ..
        } | TargetEvent::Sell {
            venue: Venue::PumpSwap,
            ..
        }
    );
    let our_wsol = ata(our_pubkey, &wsol_mint());
    let tip = pick_tip_account();
    let swap_pid = venue_program(ev);

    let mut account_keys: Vec<[u8; 32]> = vec![
        *our_pubkey,
        tip,
        SYSTEM_PROGRAM,
        compute_budget_program(),
        swap_pid,
    ];

    let mut swap_accounts = Vec::with_capacity(src.accounts.len());
    for (n, acc_i) in src.accounts.iter().enumerate() {
        let mut key = *tx.account_keys.get(*acc_i as usize)?;
        if is_pswap {
            match n {
                1 => key = *our_pubkey,
                5 => key = our_ata,
                6 => key = our_wsol,
                _ => {}
            }
        } else {
            if n == 5 {
                key = our_ata;
            }
            if n == 6 {
                key = *our_pubkey;
            }
        }
        let idx = push_key(&mut account_keys, key) as u8;
        swap_accounts.push(idx);
    }

    let mut data = Vec::with_capacity(24);
    if buy {
        if src.data.len() >= 8
            && (src.data[..8] == BUY_EXACT_SOL_IN_DISC || src.data[..8] == BUY_EXACT_QUOTE_IN_DISC)
        {
            data.extend_from_slice(&src.data[..8]);
            data.extend_from_slice(&copy_sol_lamports.to_le_bytes());
            data.extend_from_slice(&min_tokens_out.to_le_bytes());
            if src.data.len() > 24 {
                data.extend_from_slice(&src.data[24..]);
            }
        } else {
            data.extend_from_slice(&BUY_DISC);
            data.extend_from_slice(&min_tokens_out.to_le_bytes());
            data.extend_from_slice(&copy_sol_lamports.to_le_bytes());
        }
    } else {
        data.extend_from_slice(&SELL_DISC);
        data.extend_from_slice(&sell_tokens.to_le_bytes());
        data.extend_from_slice(&min_sol_out.to_le_bytes());
    }

    let mut tip_data = Vec::with_capacity(12);
    tip_data.extend_from_slice(&2u32.to_le_bytes());
    tip_data.extend_from_slice(&tip_lamports.to_le_bytes());

    let mut price_ix = vec![3u8];
    price_ix.extend_from_slice(&priority_microlamports.to_le_bytes());
    let mut cu_limit = vec![2u8];
    let cu = if is_pswap { 700_000u32 } else { 400_000u32 };
    cu_limit.extend_from_slice(&cu.to_le_bytes());

    let swap_pid_index = push_key(&mut account_keys, swap_pid) as u8;
    let ata_prog_index = push_key(&mut account_keys, associated_token_program()) as u8;
    let tok_index = push_key(&mut account_keys, token_prog) as u8;
    let keg_index = push_key(&mut account_keys, token_program_id()) as u8;
    let mint_index = push_key(&mut account_keys, mint) as u8;
    let our_ata_index = push_key(&mut account_keys, our_ata) as u8;
    let wsol_mint_index = push_key(&mut account_keys, wsol_mint()) as u8;
    let our_wsol_index = push_key(&mut account_keys, our_wsol) as u8;

    let mut instructions = vec![
        CompiledIx {
            program_id_index: 3,
            accounts: vec![],
            data: cu_limit,
        },
        CompiledIx {
            program_id_index: 3,
            accounts: vec![],
            data: price_ix,
        },
        CompiledIx {
            program_id_index: 2,
            accounts: vec![0, 1],
            data: tip_data,
        },
        CompiledIx {
            program_id_index: ata_prog_index,
            accounts: vec![0, our_ata_index, 0, mint_index, 2, tok_index],
            data: vec![1],
        },
    ];
    if is_pswap && buy {
        let mut wrap = Vec::with_capacity(12);
        wrap.extend_from_slice(&2u32.to_le_bytes());
        wrap.extend_from_slice(&copy_sol_lamports.to_le_bytes());
        instructions.push(CompiledIx {
            program_id_index: ata_prog_index,
            accounts: vec![0, our_wsol_index, 0, wsol_mint_index, 2, keg_index],
            data: vec![1],
        });
        instructions.push(CompiledIx {
            program_id_index: 2,
            accounts: vec![0, our_wsol_index],
            data: wrap,
        });
        instructions.push(CompiledIx {
            program_id_index: keg_index,
            accounts: vec![our_wsol_index],
            data: vec![17],
        });
    }
    instructions.push(CompiledIx {
        program_id_index: swap_pid_index,
        accounts: swap_accounts,
        data,
    });
    if is_pswap && buy {
        instructions.push(CompiledIx {
            program_id_index: keg_index,
            accounts: vec![our_wsol_index, 0, 0],
            data: vec![9],
        });
    }

    Some(PaperMessage {
        account_keys,
        recent_blockhash: blockhash,
        instructions,
    })
}

fn compute_budget_program() -> [u8; 32] {
    b58("ComputeBudget111111111111111111111111111111")
}

fn push_key(keys: &mut Vec<[u8; 32]>, key: [u8; 32]) -> usize {
    if let Some((i, _)) = keys.iter().enumerate().find(|(_, k)| **k == key) {
        return i;
    }
    keys.push(key);
    keys.len() - 1
}

pub fn compact_u16(data: &[u8]) -> Option<(u16, usize)> {
    let a = *data.first()?;
    if a < 0x80 {
        return Some((a as u16, 1));
    }
    let b = *data.get(1)?;
    if b < 0x80 {
        return Some(((a as u16 & 0x7f) | ((b as u16) << 7), 2));
    }
    let c = *data.get(2)?;
    Some((
        (a as u16 & 0x7f) | ((b as u16 & 0x7f) << 7) | ((c as u16) << 14),
        3,
    ))
}

/// Sign the first signature slot of a serialized legacy or v0 transaction.
pub fn sign_versioned_tx(tx: &mut [u8], seed: &[u8; 32]) -> Result<()> {
    let (n, nlen) = compact_u16(tx).context("signature count")?;
    anyhow::ensure!(n >= 1, "no signatures");
    let msg_off = nlen + (n as usize) * 64;
    anyhow::ensure!(tx.len() > msg_off, "short transaction");
    let signing = SigningKey::from_bytes(seed);
    let sig = signing.sign(&tx[msg_off..]);
    tx[nlen..nlen + 64].copy_from_slice(&sig.to_bytes());
    Ok(())
}

/// Compact-u16 + legacy message + ed25519 signature (payer = first key).
pub fn sign_legacy(msg: &PaperMessage, seed: &[u8; 32]) -> Vec<u8> {
    let signing = SigningKey::from_bytes(seed);
    let message = serialize_message(msg);
    let sig = signing.sign(&message);
    let mut out = Vec::new();
    write_shortvec(1, &mut out);
    out.extend_from_slice(&sig.to_bytes());
    out.extend_from_slice(&message);
    out
}

fn is_readonly_key(k: &[u8; 32]) -> bool {
    *k == SYSTEM_PROGRAM
        || *k == compute_budget_program()
        || *k == token_program_id()
        || *k == token_program()
        || *k == token_2022_program()
        || *k == associated_token_program()
        || *k == pump_program()
        || *k == pumpswap_program()
        || *k == raydium_amm_program()
        || *k == raydium_cpmm_program()
        || *k == mpl_metadata_program()
        || *k == b58("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ")
        || *k == b58("SysvarRent111111111111111111111111111111111")
        || *k == b58("SysvarC1ock11111111111111111111111111111111")
        || *k == b58("Sysvar1nstructions1111111111111111111111111")
        || *k == pump_event_authority()
}

fn pack_legacy_accounts(msg: &PaperMessage) -> (u8, Vec<[u8; 32]>, Vec<CompiledIx>) {
    let keys = &msg.account_keys;
    let signer = keys.first().copied().unwrap_or([0u8; 32]);
    let mut program_ids = std::collections::HashSet::new();
    for ix in &msg.instructions {
        if let Some(k) = keys.get(ix.program_id_index as usize) {
            program_ids.insert(*k);
        }
    }
    for k in keys {
        if is_readonly_key(k) {
            program_ids.insert(*k);
        }
    }
    program_ids.remove(&signer);

    let mut writable = Vec::new();
    let mut readonly = Vec::new();
    for k in keys.iter().skip(1) {
        if *k == signer {
            continue;
        }
        if program_ids.contains(k) {
            if !readonly.contains(k) {
                readonly.push(*k);
            }
        } else if !writable.contains(k) {
            writable.push(*k);
        }
    }
    for k in &program_ids {
        if !readonly.contains(k) && *k != signer {
            readonly.push(*k);
        }
    }

    let mut new_keys = Vec::with_capacity(1 + writable.len() + readonly.len());
    new_keys.push(signer);
    new_keys.extend_from_slice(&writable);
    new_keys.extend_from_slice(&readonly);
    let remap = |old: u8| -> u8 {
        let k = keys[old as usize];
        new_keys
            .iter()
            .position(|x| *x == k)
            .unwrap_or(0) as u8
    };
    let ixs: Vec<CompiledIx> = msg
        .instructions
        .iter()
        .map(|ix| CompiledIx {
            program_id_index: remap(ix.program_id_index),
            accounts: ix.accounts.iter().copied().map(remap).collect(),
            data: ix.data.clone(),
        })
        .collect();
    (readonly.len() as u8, new_keys, ixs)
}

fn serialize_message(msg: &PaperMessage) -> Vec<u8> {
    let (ro_unsigned, keys, ixs) = pack_legacy_accounts(msg);
    let mut out = Vec::new();
    out.push(1u8);
    out.push(0u8);
    out.push(ro_unsigned);
    write_shortvec(keys.len() as u16, &mut out);
    for k in &keys {
        out.extend_from_slice(k);
    }
    out.extend_from_slice(&msg.recent_blockhash);
    write_shortvec(ixs.len() as u16, &mut out);
    for ix in &ixs {
        out.push(ix.program_id_index);
        write_shortvec(ix.accounts.len() as u16, &mut out);
        out.extend_from_slice(&ix.accounts);
        write_shortvec(ix.data.len() as u16, &mut out);
        out.extend_from_slice(&ix.data);
    }
    out
}

fn write_shortvec(n: u16, out: &mut Vec<u8>) {
    let mut value = n as u32;
    while value >= 0x80 {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub const ACCOUNT_RENT: u64 = 890_880;
pub const TX_FEE: u64 = 5_000;

pub fn spendable_lamports(balance: u64) -> u64 {
    balance.saturating_sub(ACCOUNT_RENT.saturating_add(TX_FEE))
}

pub fn withdraw_lamports(balance: u64, pct: f64) -> u64 {
    let spend = spendable_lamports(balance);
    if spend == 0 {
        return 0;
    }
    if pct >= 99.999 {
        spend
    } else {
        let p = pct.clamp(0.0, 100.0);
        ((spend as f64) * (p / 100.0)).floor() as u64
    }
}

pub fn sol_transfer_message(
    from: [u8; 32],
    to: [u8; 32],
    lamports: u64,
    blockhash: [u8; 32],
) -> PaperMessage {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    PaperMessage {
        account_keys: vec![from, to, SYSTEM_PROGRAM],
        recent_blockhash: blockhash,
        instructions: vec![CompiledIx {
            program_id_index: 2,
            accounts: vec![0, 1],
            data,
        }],
    }
}

pub fn token_split_message(
    from: [u8; 32],
    dest: [u8; 32],
    mint: [u8; 32],
    amount: u64,
    blockhash: [u8; 32],
) -> PaperMessage {
    let src_ata = ata(&from, &mint);
    let dest_ata = ata(&dest, &mint);
    let tok = token_program_id();
    let ata_prog = associated_token_program();
    let create = vec![1u8];
    let mut xfer = Vec::with_capacity(12);
    xfer.extend_from_slice(&3u32.to_le_bytes());
    xfer.extend_from_slice(&amount.to_le_bytes());
    PaperMessage {
        account_keys: vec![from, src_ata, dest_ata, dest, mint, SYSTEM_PROGRAM, tok, ata_prog],
        recent_blockhash: blockhash,
        instructions: vec![
            CompiledIx {
                program_id_index: 7,
                accounts: vec![0, 2, 3, 4, 5, 6],
                data: create,
            },
            CompiledIx {
                program_id_index: 6,
                accounts: vec![1, 2, 0],
                data: xfer,
            },
        ],
    }
}

pub fn fee_sol_to_microlamports(sol: f64) -> u64 {
    let lamports = (sol * 1_000_000_000.0).max(0.0) as u64;
    lamports
        .saturating_mul(1_000_000)
        .saturating_div(200_000)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::classify_tx;
    use crate::fixture::{pump_buy_tx, OUR_WALLET};

    #[test]
    fn builds_and_signs() {
        let tx = pump_buy_tx(50_000_000, 1);
        let ev = classify_tx(&tx).unwrap();
        let msg = build_copy_message(
            &tx,
            &ev,
            &OUR_WALLET,
            10_000_000,
            0,
            900,
            0,
            1_000_000,
            1000,
            [9u8; 32],
            true,
        )
        .unwrap();
        let seed = [4u8; 32];
        let signed = sign_legacy(&msg, &seed);
        assert!(signed.len() > 64);
        let swap = msg.instructions.last().unwrap();
        assert_eq!(&swap.data[8..16], &900u64.to_le_bytes());
        assert_eq!(&swap.data[16..24], &10_000_000u64.to_le_bytes());
        let wire = serialize_message(&msg);
        assert_eq!(wire[0], 1);
        assert_eq!(wire[1], 0);
        assert!(wire[2] >= 3, "programs must be readonly unsigned");
    }

    #[test]
    fn sell_encodes_min_sol() {
        let tx = pump_buy_tx(50_000_000, 1);
        let ev = classify_tx(&tx).unwrap();
        let msg = build_copy_message(
            &tx,
            &ev,
            &OUR_WALLET,
            0,
            50,
            0,
            42,
            1_000_000,
            1000,
            [9u8; 32],
            false,
        )
        .unwrap();
        let swap = msg.instructions.last().unwrap();
        assert_eq!(&swap.data[8..16], &50u64.to_le_bytes());
        assert_eq!(&swap.data[16..24], &42u64.to_le_bytes());
    }

    #[test]
    fn pump_template_rewrites_as_sell() {
        let mint = [3u8; 32];
        let user = OUR_WALLET;
        let (tx, ev) = pump_fun_swap_template(mint, user, [9u8; 32]);
        let msg = build_copy_message(
            &tx,
            &ev,
            &user,
            0,
            50,
            0,
            42,
            1_000_000,
            1000,
            [9u8; 32],
            false,
        )
        .unwrap();
        let swap = msg.instructions.last().unwrap();
        assert_eq!(&swap.data[..8], &SELL_DISC);
        assert_eq!(&swap.data[8..16], &50u64.to_le_bytes());
        assert_eq!(tx.account_keys[2], mint);
        assert_eq!(tx.account_keys[3], bonding_curve_pda(&mint));
        let packed = serde_json::to_vec(&(tx.clone(), ev, [1u8; 32])).unwrap();
        let back: (RawTx, TargetEvent, [u8; 32]) = serde_json::from_slice(&packed).unwrap();
        assert_eq!(back.0.account_keys[2], mint);
    }

    #[test]
    fn signs_versioned_first_slot() {
        let seed = [7u8; 32];
        let msg = b"v0-message-bytes";
        let mut tx = vec![1u8];
        tx.extend_from_slice(&[0u8; 64]);
        tx.extend_from_slice(msg);
        sign_versioned_tx(&mut tx, &seed).unwrap();
        assert_ne!(&tx[1..65], &[0u8; 64]);
    }

    #[test]
    fn withdraw_keeps_rent() {
        assert_eq!(withdraw_lamports(ACCOUNT_RENT + TX_FEE, 100.0), 0);
        assert_eq!(withdraw_lamports(ACCOUNT_RENT + TX_FEE + 1_000_000, 100.0), 1_000_000);
        assert_eq!(withdraw_lamports(ACCOUNT_RENT + TX_FEE + 1_000_000, 50.0), 500_000);
        assert_eq!(withdraw_lamports(ACCOUNT_RENT + TX_FEE + 1_000_000, 25.0), 250_000);
        let msg = sol_transfer_message([1u8; 32], [2u8; 32], 100, [3u8; 32]);
        assert_eq!(msg.instructions[0].data[..4], 2u32.to_le_bytes());
        let split = token_split_message([1u8; 32], [2u8; 32], [3u8; 32], 77, [4u8; 32]);
        assert_eq!(split.instructions.len(), 2);
        assert_eq!(split.instructions[0].data, vec![1u8]);
        assert_eq!(&split.instructions[1].data[..4], &3u32.to_le_bytes());
        assert_eq!(&split.instructions[1].data[4..12], &77u64.to_le_bytes());
    }
}
