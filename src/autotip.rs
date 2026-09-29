use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TIP_FLOOR_URL: &str = "https://bundles.jito.wtf/api/v1/bundles/tip_floor";

pub struct AutoTip {
    http: reqwest::Client,
    cache: Mutex<Option<(Instant, f64)>>,
}

impl AutoTip {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            cache: Mutex::new(None),
        }
    }

    /// Lamports at ~75th–95th percentile of landed Jito tips, floored by `base` and capped by `max`.
    pub async fn tip_lamports(&self, base: u64, max: u64) -> u64 {
        let sol = match self.percentile_sol().await {
            Ok(v) => v,
            Err(_) => return clamp_tip(base, base, max),
        };
        let lamports = (sol * 1_000_000_000.0).round() as u64;
        clamp_tip(base, lamports, max)
    }

    /// Cached tip floor only — never blocks on HTTP.
    pub fn tip_lamports_cached(&self, base: u64, max: u64) -> u64 {
        let g = self.cache.lock().expect("autotip");
        if let Some((_, sol)) = *g {
            let lamports = (sol * 1_000_000_000.0).round() as u64;
            return clamp_tip(base, lamports, max);
        }
        clamp_tip(base, base, max)
    }

    async fn percentile_sol(&self) -> Result<f64> {
        {
            let g = self.cache.lock().expect("autotip");
            if let Some((at, v)) = *g {
                if at.elapsed() < Duration::from_secs(2) {
                    return Ok(v);
                }
            }
        }
        let v: Value = self
            .http
            .get(TIP_FLOOR_URL)
            .send()
            .await
            .context("tip floor http")?
            .json()
            .await
            .context("tip floor json")?;
        let row = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
        let p75 = row
            .get("landed_tips_75th_percentile")
            .and_then(|n| n.as_f64())
            .unwrap_or(0.0);
        let p95 = row
            .get("landed_tips_95th_percentile")
            .and_then(|n| n.as_f64())
            .unwrap_or(p75);
        let picked = if p95 > 0.0 {
            (p75 * 0.4) + (p95 * 0.6)
        } else {
            p75
        };
        *self.cache.lock().expect("autotip") = Some((Instant::now(), picked));
        Ok(picked)
    }
}

pub fn clamp_tip(base: u64, auto: u64, max: u64) -> u64 {
    auto.max(base).min(max).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_never_below_base() {
        assert_eq!(clamp_tip(1_000_000, 500_000, 5_000_000), 1_000_000);
        assert_eq!(clamp_tip(1_000_000, 4_000_000, 5_000_000), 4_000_000);
        assert_eq!(clamp_tip(1_000_000, 9_000_000, 5_000_000), 5_000_000);
    }
}
