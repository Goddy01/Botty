use crate::parse::{lamports_to_sol, sol_to_usd};
use crate::profile::{BuyMode, CopySetup, QuoteCurrency};
use crate::types::TargetEvent;

#[derive(Clone, Debug)]
pub struct TokenMeta {
    pub mint: [u8; 32],
    pub creator: Option<[u8; 32]>,
    pub mcap_usd: Option<f64>,
    pub age_secs: Option<f64>,
    pub mint_auth_revoked: bool,
    pub freeze_auth_revoked: bool,
    pub virtual_sol: Option<u64>,
    pub virtual_token: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterSkip {
    Inactive,
    BelowMinTrigger,
    AboveMaxTrigger,
    BelowMinMcap,
    AboveMaxMcap,
    TooNew,
    TooOld,
    SkipDeploy,
    BuyOnce,
    Denylist,
    Cooldown,
}

impl FilterSkip {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterSkip::Inactive => "inactive",
            FilterSkip::BelowMinTrigger => "below min trigger",
            FilterSkip::AboveMaxTrigger => "above max trigger",
            FilterSkip::BelowMinMcap => "below min mcap",
            FilterSkip::AboveMaxMcap => "above max mcap",
            FilterSkip::TooNew => "token too new",
            FilterSkip::TooOld => "token too old",
            FilterSkip::SkipDeploy => "skip deploys",
            FilterSkip::BuyOnce => "buy once",
            FilterSkip::Denylist => "denylist",
            FilterSkip::Cooldown => "cooldown",
        }
    }
}

pub fn copy_sol_lamports(setup: &CopySetup, target_sol_lamports: u64, cap: u64) -> u64 {
    let sized = match setup.buy_mode {
        BuyMode::Exact => target_sol_lamports,
        BuyMode::Fixed => (setup.buy_amount * 1_000_000_000.0).round() as u64,
    };
    sized.min(cap).max(0)
}

pub fn check_buy(
    setup: &CopySetup,
    ev: &TargetEvent,
    meta: &TokenMeta,
    already_copied: bool,
    sol_usd: f64,
) -> Result<u64, FilterSkip> {
    if !setup.active {
        return Err(FilterSkip::Inactive);
    }
    let TargetEvent::Buy {
        wallet,
        sol_lamports,
        is_deployer,
        ..
    } = *ev
    else {
        return Err(FilterSkip::Inactive);
    };
    if setup.skip_deploys && (is_deployer || meta.creator == Some(wallet)) {
        return Err(FilterSkip::SkipDeploy);
    }
    if setup.buy_once && already_copied {
        return Err(FilterSkip::BuyOnce);
    }
    let sol = lamports_to_sol(sol_lamports);
    let usd = sol_to_usd(sol, sol_usd);
    if let Some(min) = setup.min_trigger_buy {
        if min > 0.0 {
            let v = quote_value(sol, usd, setup.min_trigger_quote);
            if v < min {
                return Err(FilterSkip::BelowMinTrigger);
            }
        }
    }
    if let Some(max) = setup.max_trigger_buy {
        if max > 0.0 {
            let v = quote_value(sol, usd, setup.max_trigger_quote);
            if v > max {
                return Err(FilterSkip::AboveMaxTrigger);
            }
        }
    }
    if let Some(min) = setup.min_mcap {
        if min > 0.0 {
            match meta.mcap_usd {
                Some(m) if m >= min => {}
                _ => return Err(FilterSkip::BelowMinMcap),
            }
        }
    }
    if let Some(max) = setup.max_mcap {
        if max > 0.0 {
            if let Some(m) = meta.mcap_usd {
                if m > max {
                    return Err(FilterSkip::AboveMaxMcap);
                }
            }
        }
    }
    if let Some(min_age) = setup.min_token_age_s {
        if min_age > 0.0 && meta.age_secs.unwrap_or(0.0) < min_age {
            return Err(FilterSkip::TooNew);
        }
    }
    if let Some(max_age) = setup.max_token_age_s {
        if max_age > 0.0 && meta.age_secs.unwrap_or(f64::MAX) > max_age {
            return Err(FilterSkip::TooOld);
        }
    }
    Ok(copy_sol_lamports(setup, sol_lamports, u64::MAX))
}

fn quote_value(sol: f64, usd: f64, q: QuoteCurrency) -> f64 {
    match q {
        QuoteCurrency::Sol => sol,
        QuoteCurrency::Usdt => usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Venue;

    fn buy(sol: u64) -> TargetEvent {
        TargetEvent::Buy {
            wallet: [1u8; 32],
            mint: [2u8; 32],
            token_amount: 1,
            sol_lamports: sol,
            ix_index: 0,
            venue: Venue::PumpFun,
            is_deployer: false,
        }
    }

    fn meta() -> TokenMeta {
        TokenMeta {
            mint: [2u8; 32],
            creator: None,
            mcap_usd: Some(50_000.0),
            age_secs: Some(3600.0),
            mint_auth_revoked: true,
            freeze_auth_revoked: true,
            virtual_sol: None,
            virtual_token: None,
        }
    }

    fn live_setup() -> CopySetup {
        let mut s = CopySetup::default();
        s.active = true;
        s
    }

    #[test]
    fn min_trigger_and_buy_once() {
        let mut s = live_setup();
        s.min_trigger_buy = Some(0.5);
        s.min_trigger_quote = QuoteCurrency::Sol;
        assert_eq!(
            check_buy(&s, &buy(100_000_000), &meta(), false, 150.0).unwrap_err(),
            FilterSkip::BelowMinTrigger
        );
        assert!(check_buy(&s, &buy(600_000_000), &meta(), false, 150.0).is_ok());
        s.buy_once = true;
        assert_eq!(
            check_buy(&s, &buy(600_000_000), &meta(), true, 150.0).unwrap_err(),
            FilterSkip::BuyOnce
        );
    }

    #[test]
    fn mcap_age_deploy() {
        let mut s = live_setup();
        s.max_mcap = Some(10_000.0);
        assert_eq!(
            check_buy(&s, &buy(1_000_000_000), &meta(), false, 150.0).unwrap_err(),
            FilterSkip::AboveMaxMcap
        );
        s.max_mcap = None;
        s.min_token_age_s = Some(7200.0);
        assert_eq!(
            check_buy(&s, &buy(1_000_000_000), &meta(), false, 150.0).unwrap_err(),
            FilterSkip::TooNew
        );
        s.min_token_age_s = None;
        s.skip_deploys = true;
        let mut m = meta();
        m.creator = Some([1u8; 32]);
        assert_eq!(
            check_buy(&s, &buy(1_000_000_000), &m, false, 150.0).unwrap_err(),
            FilterSkip::SkipDeploy
        );
    }

    #[test]
    fn exact_vs_fixed_and_unknown_mcap() {
        let mut s = CopySetup::default();
        s.buy_mode = BuyMode::Fixed;
        s.buy_amount = 0.2;
        assert_eq!(
            copy_sol_lamports(&s, 1_000_000_000, u64::MAX),
            200_000_000
        );
        s.buy_mode = BuyMode::Exact;
        assert_eq!(
            copy_sol_lamports(&s, 80_000_000, 50_000_000),
            50_000_000
        );
        s.active = true;
        s.buy_mode = BuyMode::Fixed;
        s.min_mcap = Some(1.0);
        let mut m = meta();
        m.mcap_usd = None;
        assert_eq!(
            check_buy(&s, &buy(1_000_000_000), &m, false, 150.0).unwrap_err(),
            FilterSkip::BelowMinMcap
        );
        s.min_mcap = None;
        s.max_mcap = Some(10_000.0);
        assert!(check_buy(&s, &buy(1_000_000_000), &m, false, 150.0).is_ok());
        s.max_mcap = None;
        s.max_trigger_buy = Some(0.1);
        s.max_trigger_quote = QuoteCurrency::Sol;
        assert_eq!(
            check_buy(&s, &buy(200_000_000), &meta(), false, 150.0).unwrap_err(),
            FilterSkip::AboveMaxTrigger
        );
        s.max_trigger_buy = None;
        s.max_token_age_s = Some(60.0);
        assert_eq!(
            check_buy(&s, &buy(1_000_000_000), &meta(), false, 150.0).unwrap_err(),
            FilterSkip::TooOld
        );
    }
}
