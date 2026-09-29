use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILLS: usize = 20_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PnlFill {
    pub unix: u64,
    pub sol: f64,
    #[serde(default)]
    pub mint: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct UserPnl {
    #[serde(default)]
    fills: Vec<PnlFill>,
}

pub struct PnlStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl PnlStore {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self {
            dir,
            lock: Mutex::new(()),
        })
    }

    pub fn record(&self, user_id: i64, sol: f64, mint: &str) -> Result<()> {
        self.record_at(user_id, now_unix(), sol, mint)
    }

    pub fn record_at(&self, user_id: i64, unix: u64, sol: f64, mint: &str) -> Result<()> {
        if !sol.is_finite() || sol == 0.0 {
            return Ok(());
        }
        let _g = self.lock.lock().expect("pnl");
        let mut p = self.load(user_id);
        p.fills.push(PnlFill {
            unix,
            sol,
            mint: mint.to_string(),
        });
        if p.fills.len() > MAX_FILLS {
            let drop_n = p.fills.len() - MAX_FILLS;
            p.fills.drain(0..drop_n);
        }
        self.write(user_id, &p)
    }

    pub fn fills(&self, user_id: i64) -> Vec<PnlFill> {
        self.load(user_id).fills
    }

    pub fn fills_for_mint(&self, user_id: i64, mint: &str) -> Vec<PnlFill> {
        self.load(user_id)
            .fills
            .into_iter()
            .filter(|f| f.mint == mint)
            .collect()
    }

    fn load(&self, user_id: i64) -> UserPnl {
        let path = self.dir.join(format!("{user_id}.json"));
        fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn write(&self, user_id: i64, p: &UserPnl) -> Result<()> {
        let path = self.dir.join(format!("{user_id}.json"));
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string(p).context("serialize pnl")?;
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }
}

/// Unix days → proleptic Gregorian (Hinnant).
pub fn unix_to_ymd(unix: u64) -> (i32, u32, u32) {
    let z = (unix / 86400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

pub fn week_index(day: u32) -> u8 {
    ((day.saturating_sub(1)) / 7).min(3) as u8
}

pub fn week_span(year: i32, month: u32, week: u8) -> (u32, u32) {
    let dim = days_in_month(year, month);
    let start = 1 + u32::from(week) * 7;
    let end = if week >= 3 { dim } else { (start + 6).min(dim) };
    (start.min(dim), end)
}

pub fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        _ => "December",
    }
}

pub fn add_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let mut m = month as i32 + delta;
    let mut y = year;
    while m < 1 {
        m += 12;
        y -= 1;
    }
    while m > 12 {
        m -= 12;
        y += 1;
    }
    (y, m as u32)
}

/// Day → net SOL for that UTC day in the month.
pub fn daily_sol(fills: &[PnlFill], year: i32, month: u32) -> HashMap<u32, f64> {
    let mut out: HashMap<u32, f64> = HashMap::new();
    for f in fills {
        let (y, m, d) = unix_to_ymd(f.unix);
        if y == year && m == month {
            *out.entry(d).or_default() += f.sol;
        }
    }
    out
}

pub fn month_total(daily: &HashMap<u32, f64>) -> f64 {
    daily.values().copied().sum()
}

pub fn best_day(daily: &HashMap<u32, f64>) -> Option<(u32, f64)> {
    daily
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(d, v)| (*d, *v))
}

pub fn bought_sold(fills: &[PnlFill]) -> (f64, f64, usize) {
    let mut bought = 0.0;
    let mut sold = 0.0;
    for f in fills {
        if f.sol < 0.0 {
            bought += -f.sol;
        } else {
            sold += f.sol;
        }
    }
    (bought, sold, fills.len())
}

pub fn render_token_pnl(
    title: &str,
    mint: &str,
    fills: &[PnlFill],
    remain_sol: Option<f64>,
    stamp: &str,
) -> String {
    let (bought, sold, n) = bought_sold(fills);
    let n_buy = fills.iter().filter(|f| f.sol < 0.0).count();
    let n_sell = fills.iter().filter(|f| f.sol > 0.0).count();
    let remain = remain_sol.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
    let pnl = sold + remain - bought;
    let pct = if bought > 1e-9 {
        pnl / bought * 100.0
    } else {
        0.0
    };
    let sign = if pnl >= 0.0 { "🟢" } else { "🔴" };
    let mut lines = vec![
        format!("🌸 {title} PnL"),
        String::new(),
        format!("`{mint}`"),
        String::new(),
        format!("{n_buy} buy(s) · {n_sell} sell(s)"),
        format!("Spent: {:.4} SOL", bought),
        format!("Sold: {:.4} SOL", sold),
        format!("Still holding: {:.4} SOL", remain),
        String::new(),
        format!("{sign} PnL: {} ({:+.1}%)", fmt_header_sol(pnl), pct),
    ];
    if n == 0 && remain <= 0.0 {
        lines = vec![
            format!("🌸 {title} PnL"),
            String::new(),
            format!("`{mint}`"),
            String::new(),
            "No trades for this CA on this wallet yet.".into(),
            "Buys and sells on this token will show up here.".into(),
        ];
    }
    lines.push(String::new());
    lines.push(format!("🕐 Last updated: {stamp}"));
    lines.join("\n")
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug)]
pub struct CalView {
    pub year: i32,
    pub month: u32,
    pub week: u8,
    pub usd: bool,
    pub recap: bool,
}

impl CalView {
    pub fn for_unix(unix: u64) -> Self {
        let (year, month, day) = unix_to_ymd(unix);
        Self {
            year,
            month,
            week: week_index(day),
            usd: false,
            recap: false,
        }
    }
}

fn fmt_sol_cell(sol: f64) -> String {
    if sol.abs() < 5e-4 {
        "+ 0 SOL".into()
    } else if sol >= 0.0 {
        format!("+ {:.2} SOL", sol)
    } else {
        format!("- {:.2} SOL", -sol)
    }
}

fn fmt_usd_cell(usd: f64) -> String {
    if usd.abs() < 5e-3 {
        "+ $0.00".into()
    } else if usd >= 0.0 {
        format!("+ ${:.2}", usd)
    } else {
        format!("- ${:.2}", -usd)
    }
}

fn fmt_usd_best(usd: f64) -> String {
    let a = usd.abs();
    let sign = if usd < 0.0 { "-" } else { "" };
    if a >= 1000.0 {
        format!("{sign}${:.2}K", a / 1000.0)
    } else {
        format!("{sign}${:.2}", a)
    }
}

fn fmt_header_sol(sol: f64) -> String {
    if sol.abs() < 5e-4 {
        "+0.00 SOL".into()
    } else if sol >= 0.0 {
        format!("+{:.2} SOL", sol)
    } else {
        format!("-{:.2} SOL", -sol)
    }
}

fn day_icon(v: f64) -> &'static str {
    if v > 5e-4 {
        "🟩"
    } else if v < -5e-4 {
        "🟥"
    } else {
        "⬜️"
    }
}

pub fn render_calendar(
    fills: &[PnlFill],
    view: &CalView,
    sol_usd: f64,
    stamp: &str,
) -> String {
    let daily = daily_sol(fills, view.year, view.month);
    let total = month_total(&daily);
    let (w0, w1) = week_span(view.year, view.month, view.week);
    let mname = month_name(view.month);
    let px = if sol_usd.is_finite() && sol_usd > 0.0 {
        sol_usd
    } else {
        150.0
    };
    let header_amt = if view.usd {
        fmt_usd_best(total * px)
    } else {
        fmt_header_sol(total)
    };
    let mut lines = vec![
        format!("📅 {mname} PnL {header_amt}"),
        "Daily P&L Breakdown:".into(),
        String::new(),
    ];
    for d in w0..=w1 {
        let v = daily.get(&d).copied().unwrap_or(0.0);
        let cell = if view.usd {
            fmt_usd_cell(v * px)
        } else {
            fmt_sol_cell(v)
        };
        lines.push(format!("{} {} {}", day_icon(v), d, cell));
    }
    let best = best_day(&daily);
    let best_line = match best {
        Some((d, v)) => {
            format!("- Best Day: {:02}/{:02} — {}", view.month, d, fmt_usd_best(v * px))
        }
        None => "- Best Day: —".into(),
    };
    let best_month = match best {
        Some((d, v)) => format!(
            "- Best Day ({mname}): {:02}/{:02} — {}",
            view.month,
            d,
            fmt_usd_best(v * px)
        ),
        None => format!("- Best Day ({mname}): —"),
    };
    let (ty, tm, td) = unix_to_ymd(now_unix());
    let streak = profit_streak(&daily, ty, tm, td, view.year, view.month);
    lines.push(String::new());
    lines.push("📊 Performance Summary:".into());
    lines.push(best_line);
    lines.push(best_month);
    lines.push(format!("└ Profit Streak: {streak} days"));
    if view.recap {
        let n = fills
            .iter()
            .filter(|f| {
                let (y, m, _) = unix_to_ymd(f.unix);
                y == view.year && m == view.month
            })
            .count();
        lines.push(String::new());
        lines.push("📋 Recap".into());
        lines.push(format!("Fills this month: {n}"));
        lines.push(format!("Net: {}", fmt_header_sol(total)));
    }
    lines.push(String::new());
    lines.push(format!("🕐 Last updated: {stamp}"));
    lines.join("\n")
}

pub fn profit_streak(daily: &HashMap<u32, f64>, today_y: i32, today_m: u32, today_d: u32, year: i32, month: u32) -> u32 {
    if year != today_y || month != today_m {
        return 0;
    }
    let mut n = 0u32;
    let mut d = today_d;
    while d >= 1 {
        let v = daily.get(&d).copied().unwrap_or(0.0);
        if v > 0.0 {
            n += 1;
            d -= 1;
        } else {
            break;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_is_1970_01_01() {
        assert_eq!(unix_to_ymd(0), (1970, 1, 1));
        assert_eq!(unix_to_ymd(946_684_800), (2000, 1, 1));
    }

    #[test]
    fn september_weeks() {
        assert_eq!(week_span(2026, 9, 0), (1, 7));
        assert_eq!(week_span(2026, 9, 1), (8, 14));
        assert_eq!(week_span(2026, 9, 2), (15, 21));
        assert_eq!(week_span(2026, 9, 3), (22, 30));
        assert_eq!(week_index(28), 3);
    }

    #[test]
    fn daily_net() {
        let fills = vec![
            PnlFill {
                unix: 946_684_800,
                sol: -0.1,
                mint: String::new(),
            },
            PnlFill {
                unix: 946_684_800 + 100,
                sol: 0.4,
                mint: String::new(),
            },
        ];
        let d = daily_sol(&fills, 2000, 1);
        assert!((d.get(&1).copied().unwrap_or(0.0) - 0.3).abs() < 1e-9);
        assert!((month_total(&d) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn calendar_layout() {
        let fills = vec![PnlFill {
            unix: 946_684_800,
            sol: 1.75,
            mint: String::new(),
        }];
        let mut view = CalView::for_unix(946_684_800);
        view.week = 0;
        let t = render_calendar(&fills, &view, 1000.0, "00:00:00.000");
        assert!(t.contains("January PnL"));
        assert!(t.contains("Daily P&L Breakdown:"));
        assert!(t.contains("🟩 1 + 1.75 SOL"));
        assert!(t.contains("Best Day: 01/01 — $1.75K"));
        assert!(t.contains("Performance Summary:"));
        assert!(t.contains("Profit Streak:"));
    }

    #[test]
    fn token_pnl_is_mint_scoped() {
        let mint = "FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5";
        let fills = vec![
            PnlFill {
                unix: 1,
                sol: -1.0,
                mint: mint.into(),
            },
            PnlFill {
                unix: 2,
                sol: 1.25,
                mint: mint.into(),
            },
            PnlFill {
                unix: 3,
                sol: 9.0,
                mint: "other".into(),
            },
        ];
        let scoped: Vec<_> = fills.iter().filter(|f| f.mint == mint).cloned().collect();
        let t = render_token_pnl("PokeFUN", mint, &scoped, Some(0.0), "00:00:00.000");
        assert!(t.contains(mint));
        assert!(t.contains("1.0000 SOL"));
        assert!(t.contains("1.2500 SOL"));
        assert!(t.contains("+0.25 SOL"));
        assert!(!t.contains("9.0000"));
    }
}
