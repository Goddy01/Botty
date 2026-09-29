use crate::positions::Lot;
use crate::profile::{LimitOrder, StrategyMode};

pub struct StratCtx {
    pub mcap: Option<f64>,
    pub lot: Lot,
    pub now_unix: u64,
}

pub fn sell_tokens(hold: u64, pct: f64) -> u64 {
    if hold == 0 || pct <= 0.0 {
        return 0;
    }
    if pct >= 100.0 {
        return hold;
    }
    ((hold as f64) * pct / 100.0).floor() as u64
}

pub fn order_fired(order: &LimitOrder, ctx: &StratCtx) -> bool {
    if order.sell_amount_pct <= 0.0 {
        return false;
    }
    match order.mode {
        // Legacy profiles may still carry these; they are not operable.
        StrategyMode::DevSell | StrategyMode::McapVariation => false,
        StrategyMode::Time => {
            if order.time_secs <= 0.0 || ctx.lot.bought_at_unix == 0 {
                return false;
            }
            let age = ctx.now_unix.saturating_sub(ctx.lot.bought_at_unix) as f64;
            age >= order.time_secs
        }
        StrategyMode::Trailing => {
            let Some(peak) = ctx.lot.peak_mcap.filter(|p| *p > 0.0) else {
                return false;
            };
            let Some(cur) = ctx.mcap else {
                return false;
            };
            let drop_pct = if order.trigger_pct != 0.0 {
                order.trigger_pct.abs()
            } else {
                order.sell_at_pct.abs()
            };
            if drop_pct <= 0.0 {
                return false;
            }
            let drawdown = (peak - cur) / peak * 100.0;
            drawdown >= drop_pct
        }
        StrategyMode::ProfitLoss => {
            let Some(entry) = ctx.lot.entry_mcap else {
                return false;
            };
            let Some(cur) = ctx.mcap else {
                return false;
            };
            if entry <= 0.0 {
                return false;
            }
            let pnl = (cur - entry) / entry * 100.0;
            let trig = order.pnl_trigger();
            if trig > 0.0 {
                pnl >= trig
            } else if trig < 0.0 {
                pnl <= trig
            } else {
                false
            }
        }
        StrategyMode::FixedMcap => {
            order.mcap_usd > 0.0 && ctx.mcap.map(|m| m >= order.mcap_usd).unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lot(entry: f64) -> Lot {
        Lot {
            amount: 100,
            entry_mcap: Some(entry),
            bought_at_unix: 1_000,
            peak_mcap: Some(entry),
            task_idx: 0,
        }
    }

    fn ctx(mcap: f64, lot: Lot) -> StratCtx {
        StratCtx {
            mcap: Some(mcap),
            lot,
            now_unix: 1_000,
        }
    }

    #[test]
    fn zeros_never_fire() {
        let o = LimitOrder::default();
        assert!(!order_fired(&o, &ctx(200.0, lot(100.0))));
        assert_eq!(sell_tokens(100, 0.0), 0);
        assert_eq!(sell_tokens(100, 52.0), 52);
        assert_eq!(sell_tokens(100, 100.0), 100);
    }

    #[test]
    fn take_profit_and_stop() {
        let mut o = LimitOrder::default();
        o.sell_amount_pct = 50.0;
        o.sell_at_pct = 100.0;
        o.mode = StrategyMode::ProfitLoss;
        let mut c = ctx(199.0, lot(100.0));
        assert!(!order_fired(&o, &c));
        c.mcap = Some(200.0);
        assert!(order_fired(&o, &c));
        o.sell_at_pct = -20.0;
        o.trigger_pct = 0.0;
        c.mcap = Some(80.0);
        assert!(order_fired(&o, &c));
    }

    #[test]
    fn time_and_trailing_fire() {
        let mut o = LimitOrder::default();
        o.sell_amount_pct = 100.0;
        o.mode = StrategyMode::Time;
        o.time_secs = 30.0;
        let mut c = StratCtx {
            mcap: Some(100.0),
            lot: lot(100.0),
            now_unix: 1_020,
        };
        assert!(!order_fired(&o, &c));
        c.now_unix = 1_040;
        assert!(order_fired(&o, &c));

        o.mode = StrategyMode::Trailing;
        o.trigger_pct = 10.0;
        o.time_secs = 0.0;
        let mut l = lot(100.0);
        l.peak_mcap = Some(200.0);
        c.lot = l;
        c.mcap = Some(190.0);
        assert!(!order_fired(&o, &c));
        c.mcap = Some(180.0);
        assert!(order_fired(&o, &c));
    }

    #[test]
    fn legacy_modes_never_fire() {
        let mut o = LimitOrder::default();
        o.sell_amount_pct = 100.0;
        o.trigger_pct = 10.0;
        o.mcap_usd = 1.0;
        let c = ctx(200.0, lot(100.0));
        o.mode = StrategyMode::DevSell;
        assert!(!order_fired(&o, &c));
        o.mode = StrategyMode::McapVariation;
        assert!(!order_fired(&o, &c));
    }
}
