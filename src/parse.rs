//! Human-readable duration and quote helpers for copy-setup fields.

const MONTH_SECS: f64 = 30.0 * 24.0 * 3600.0;

/// Parse `30s`, `5m`, `2h`, `1d`, `1mo` / `1 month`, or a plain number of seconds.
/// `m` is minutes. Months must be `mo`, `month`, or `months`.
pub fn parse_duration_secs(text: &str) -> Result<Option<f64>, String> {
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() || t == "n/a" || t == "na" || t == "none" || t == "clear" {
        return Ok(None);
    }
    if let Ok(n) = t.parse::<f64>() {
        return finite_nonneg(n).map(Some);
    }
    let mut total = 0.0_f64;
    let mut rest = t.as_str();
    let mut got = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let (num, after) = take_number(rest).ok_or_else(|| {
            "Send a duration like `30s`, `5m`, `2h`, `1d`, `1mo` (or seconds / `n/a`).".to_string()
        })?;
        let after = after.trim_start();
        let (unit, next) = take_unit(after).ok_or_else(|| {
            "Use units s, m (minutes), h, d, or mo/month. Example: `90m` or `2h 30m`.".to_string()
        })?;
        let secs = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => num,
            "m" | "min" | "mins" | "minute" | "minutes" => num * 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => num * 3600.0,
            "d" | "day" | "days" => num * 86400.0,
            "mo" | "mos" | "month" | "months" => num * MONTH_SECS,
            _ => return Err("Unknown time unit. Use s, m, h, d, or mo.".into()),
        };
        total += secs;
        got = true;
        rest = next;
    }
    if !got {
        return Err("Send a duration like `30s`, `5m`, `2h`, `1d`, `1mo`.".into());
    }
    finite_nonneg(total).map(Some)
}

fn finite_nonneg(n: f64) -> Result<f64, String> {
    if !n.is_finite() || n < 0.0 {
        return Err("Need a number ≥ 0.".into());
    }
    Ok(n)
}

fn take_number(s: &str) -> Option<(f64, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut dot = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() {
            i += 1;
        } else if c == b'.' && !dot {
            dot = true;
            i += 1;
        } else {
            break;
        }
    }
    if i == 0 {
        return None;
    }
    let n: f64 = s[..i].parse().ok()?;
    Some((n, &s[i..]))
}

fn take_unit(s: &str) -> Option<(&str, &str)> {
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    Some((&s[..i], &s[i..]))
}

pub fn sol_to_usd(sol: f64, sol_usd: f64) -> f64 {
    sol * sol_usd
}

pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse_duration_secs("n/a").unwrap(), None);
        assert_eq!(parse_duration_secs("45").unwrap(), Some(45.0));
        assert_eq!(parse_duration_secs("30s").unwrap(), Some(30.0));
        assert_eq!(parse_duration_secs("5m").unwrap(), Some(300.0));
        assert_eq!(parse_duration_secs("2h").unwrap(), Some(7200.0));
        assert_eq!(parse_duration_secs("1d").unwrap(), Some(86400.0));
        assert_eq!(parse_duration_secs("1mo").unwrap(), Some(MONTH_SECS));
        assert_eq!(parse_duration_secs("1 month").unwrap(), Some(MONTH_SECS));
        assert_eq!(parse_duration_secs("2h 30m").unwrap(), Some(9000.0));
        assert!(parse_duration_secs("xyz").is_err());
    }
}
