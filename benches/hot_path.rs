use copybot::decode::decode_pump_buy;
use copybot::fixture::{pump_buy_tx, OUR_WALLET, WALLET_A};
use copybot::hot::{on_tx, HotPath};
use copybot::tip::TipLadder;
use copybot::risk::{check, RiskConfig, RiskState};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::collections::HashSet;

fn bench_decode(c: &mut Criterion) {
    let tx = pump_buy_tx(50_000_000, 1_000_000);
    c.bench_function("decode_pump_buy", |b| {
        b.iter(|| decode_pump_buy(black_box(&tx)))
    });
}

fn bench_risk(c: &mut Criterion) {
    let tx = pump_buy_tx(50_000_000, 1_000_000);
    let buy = decode_pump_buy(&tx).unwrap();
    let cfg = RiskConfig {
        max_sol_lamports: 50_000_000,
        cooldown_ms: 0,
    };
    c.bench_function("risk_check", |b| {
        b.iter(|| {
            let mut state = RiskState::new(HashSet::new());
            check(&mut state, &cfg, black_box(&buy), 1)
        })
    });
}

fn bench_on_tx(c: &mut Criterion) {
    let tx = pump_buy_tx(50_000_000, 1_000_000);
    let mut watched = HashSet::new();
    watched.insert(WALLET_A);
    let mut path = HotPath::new(
        watched,
        HashSet::new(),
        OUR_WALLET,
        50_000_000,
        0,
        [0u8; 32],
        TipLadder::from_sol(0.001, 0.003, 0.008, 0.02),
    );
    c.bench_function("on_tx_paper", |b| {
        b.iter(|| on_tx(&mut path, black_box(&tx), 1))
    });
}

criterion_group!(benches, bench_decode, bench_risk, bench_on_tx);
criterion_main!(benches);
