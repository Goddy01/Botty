use super::parse::{
    extract_mint, Command, SetupOp, SetupValue,
};
use crate::live::{LiveSnap, MainnetEvent};
use crate::pnl::{self, CalView};
use crate::profile::{
    short_wallet_label, BuyMode, CopySetup, CopyTask, LimitOrder, QuoteCurrency, StrategyMode,
    MAX_WATCHED,
};
use crate::rpc_read::fmt_sol;
use crate::user_wallet::CreatedWallet;
use crate::parse::parse_duration_secs;
use super::{BOT_NAME, BOT_USERNAME, MENU};
use std::collections::HashMap;

pub(crate) fn btn(text: &str, data: &str) -> serde_json::Value {
    serde_json::json!({ "text": text, "callback_data": data })
}

pub(crate) fn url_btn(text: &str, url: &str) -> serde_json::Value {
    serde_json::json!({ "text": text, "url": url })
}

pub fn chart_url(mint: &str) -> String {
    format!(
        "https://dexscreener.com/solana/{mint}?embed=1&loadChartSettings=0&trades=1&tabs=1&info=1&chartLeftToolbar=0&chartDefaultOnMobile=1&chartTheme=dark&theme=dark&chartStyle=1&chartType=mcap&interval=1"
    )
}

/// Bloom-style 2-column inline grid. Last row should be Back | Close.
pub fn inline_grid(rows: &[&[(&str, &str)]]) -> serde_json::Value {
    let keyboard: Vec<Vec<serde_json::Value>> = rows
        .iter()
        .map(|row| row.iter().map(|(t, d)| btn(t, d)).collect())
        .collect();
    serde_json::json!({ "inline_keyboard": keyboard })
}

pub(crate) fn inline_owned(rows: Vec<Vec<(String, String)>>) -> serde_json::Value {
    let keyboard: Vec<Vec<serde_json::Value>> = rows
        .into_iter()
        .map(|row| row.into_iter().map(|(t, d)| btn(&t, &d)).collect())
        .collect();
    serde_json::json!({ "inline_keyboard": keyboard })
}

pub(crate) fn calendar_keyboard(view: &CalView) -> serde_json::Value {
    let week = |i: u8, name: &str| {
        if view.week == i {
            format!("✅ {name}")
        } else {
            name.to_string()
        }
    };
    let usd = if view.usd {
        "🟢 USD".to_string()
    } else {
        "🔴 USD".to_string()
    };
    inline_owned(vec![
        vec![
            ("⏮ Prev".into(), "cal:p".into()),
            (pnl::month_name(view.month).into(), "cal:n".into()),
        ],
        vec![
            (week(0, "Week 1"), "cal:w:0".into()),
            (week(1, "Week 2"), "cal:w:1".into()),
        ],
        vec![
            (week(2, "Week 3"), "cal:w:2".into()),
            (week(3, "Week 4"), "cal:w:3".into()),
        ],
        vec![("📋 Generate Recap".into(), "cal:recap".into())],
        vec![
            ("← Back".into(), "p:home".into()),
            (usd, "cal:usd".into()),
            ("🔄 Refresh".into(), "cal:r".into()),
        ],
        vec![("🗑 Close".into(), "x:close".into())],
    ])
}

pub(crate) fn nav_row() -> [(&'static str, &'static str); 2] {
    [("← Back", "p:back"), ("🗑 Close", "x:close")]
}

pub fn home_keyboard() -> serde_json::Value {
    inline_grid(&[
        &[("💼 Positions", "p:positions")],
        &[("🤖 Copy Trade", "p:copy")],
        &[("⚙️ Settings", "p:settings"), ("💸 Withdraw", "p:withdraw")],
        &[("🔄 Refresh", "p:refresh"), ("📅 PnL Calendar", "p:calendar")],
        &[("🗑 Close", "x:close")],
    ])
}

#[derive(Clone, Debug, Default)]
pub struct BotSettings {
    pub degen_mode: bool,
    pub expert_mode: bool,
    pub buy_protection: bool,
    pub private_pnl: bool,
}

pub fn settings_keyboard(s: &BotSettings) -> serde_json::Value {
    let buy = format!("{} Buy Protection", on_dot(s.buy_protection));
    let pnl = format!("{} Private PNL", on_dot(s.private_pnl));
    serde_json::json!({
        "inline_keyboard": [
            [btn("👛 Wallets", "p:wallets")],
            [btn("🔧 Presets", "set:pre")],
            [btn(&buy, "set:bp"), btn(&pnl, "set:pp")],
            [btn("← Back", "p:back"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub fn wallets_settings_keyboard(picks: &[String]) -> serde_json::Value {
    let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
    if picks.is_empty() {
        rows.push(vec![btn("🟢 W1 · 0 SOL", "w:sel:0")]);
    } else {
        for (i, pick) in picks.iter().enumerate() {
            rows.push(vec![btn(pick, &format!("w:sel:{i}"))]);
        }
    }
    rows.push(vec![
        btn("💵 Create Wallet", "w:new"),
        btn("🔑 Import Wallet", "w:imp"),
    ]);
    rows.push(vec![btn("🔐 Export Private Key", "w:exp")]);
    rows.push(vec![
        btn("← Back", "p:settings"),
        btn("🗑 Close", "x:close"),
    ]);
    serde_json::json!({ "inline_keyboard": rows })
}

pub(crate) fn settings_text() -> String {
    format!(
        "⚙️ Settings\n\n\
Customize the bot behavior using the settings buttons below.\n\n\
🟢 Mode is active\n\
🔴 Mode is inactive\n\n\
Manage your settings on {BOT_NAME} Manager.\n\n\
📄 Docs\n\n\
🕐 Last updated: {}",
        last_updated_stamp()
    )
}

pub(crate) fn wallets_settings_text(entries: &[(bool, String, String)]) -> String {
    let mut lines = vec![
        "💵 Wallets Settings".into(),
        String::new(),
        "Manage your wallets quickly and easily.".into(),
        String::new(),
        "👛 Available Wallets".into(),
    ];
    if entries.is_empty() {
        lines.push("└ none".into());
    } else {
        for (on, label, pk) in entries {
            let mark = if *on { "🟢 " } else { "" };
            lines.push(format!("└ {mark}{label} -"));
            lines.push(format!("`{pk}`"));
        }
    }
    lines.push(String::new());
    lines.push(format!("Create and manage your wallets on {BOT_NAME} Manager."));
    lines.push(String::new());
    lines.push("📄 Docs".into());
    lines.push(String::new());
    lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
    lines.join("\n")
}

pub(crate) fn presets_text() -> String {
    format!(
        "🔧 Presets\n\n\
Preconfigured trading settings to execute faster, consistent trades with one tap\n\n\
📄 Docs\n\n\
🕐 Last updated: {}",
        last_updated_stamp()
    )
}

pub(crate) fn presets_keyboard() -> serde_json::Value {
    serde_json::json!({
        "inline_keyboard": [
            [btn("💰 Buy Presets", "pre:buy"), btn("🔧 Sell Presets", "pre:sell")],
            [btn("← Back", "p:settings"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub(crate) fn buy_amount_text() -> String {
    format!(
        "🔧 Buy Amount\n\n\
These values are displayed as Quick Buy buttons when placing a trade.\n\n\
Set your buy size by clicking the buttons below to choose your desired amount.\n\n\
🕐 Last updated: {}",
        last_updated_stamp()
    )
}

pub(crate) fn sell_percentage_text() -> String {
    format!(
        "🔧 Sell Percentage (%)\n\n\
These values are displayed as Quick Sell buttons when closing a position.\n\n\
Set your sell percentage by clicking the buttons below to choose your desired amount.\n\n\
🕐 Last updated: {}",
        last_updated_stamp()
    )
}

pub(crate) fn sell_percentage_keyboard(pcts: &[f64; 5]) -> serde_json::Value {
    let a0 = format!("💰 {} %", trim_num(pcts[0]));
    let a1 = format!("💰 {} %", trim_num(pcts[1]));
    let a2 = format!("💰 {} %", trim_num(pcts[2]));
    let a3 = format!("💰 {} %", trim_num(pcts[3]));
    let a4 = format!("💰 {} %", trim_num(pcts[4]));
    serde_json::json!({
        "inline_keyboard": [
            [btn(&a0, "sp:0"), btn(&a1, "sp:1"), btn(&a2, "sp:2")],
            [btn(&a3, "sp:3"), btn(&a4, "sp:4")],
            [btn("← Back", "set:pre"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub(crate) fn fmt_mcap_short(usd: Option<f64>) -> String {
    match usd {
        None => "—".into(),
        Some(n) if n.abs() >= 1_000_000.0 => format!("${:.2}M", n / 1_000_000.0),
        Some(n) if n.abs() >= 1_000.0 => format!("${:.2}K", n / 1_000.0),
        Some(n) => format!("${:.2}", n),
    }
}

pub(crate) fn fmt_bloom_price(p: Option<f64>) -> String {
    let Some(p) = p else {
        return "—".into();
    };
    if p.abs() >= 0.01 {
        return format!("${:.4}", p)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string();
    }
    let s = format!("{p:.20}");
    let rest = s.strip_prefix("0.").unwrap_or(&s);
    let zeros = rest.chars().take_while(|c| *c == '0').count();
    let digits: String = rest.chars().skip(zeros).take(5).collect();
    if zeros >= 2 && !digits.is_empty() {
        format!("$0.0{}{}", sub_digits(zeros), digits)
    } else {
        format!("${p:.8}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

pub(crate) fn sub_digits(n: usize) -> String {
    const SUB: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    n.to_string()
        .chars()
        .filter_map(|c| c.to_digit(10).map(|d| SUB[d as usize]))
        .collect()
}

pub(crate) fn auth_dot(ok: bool) -> &'static str {
    if ok {
        "🟢"
    } else {
        "🔴"
    }
}

pub(crate) fn trade_keyboard(
    sell: bool,
    buy: &[f64; 5],
    sellp: &[f64; 5],
    tip: f64,
    slip: f64,
    bp: bool,
    auto_tip: bool,
    wallets: u32,
    mint: &str,
) -> serde_json::Value {
    let switch = if sell {
        "🔄 Switch to Buy"
    } else {
        "🔄 Switch to Sell"
    };
    let (s0, s1, s2, s3, s4, sx, d0, d1, d2, d3, d4, dx) = if sell {
        (
            format!("💰 {} %", trim_num(sellp[0])),
            format!("💰 {} %", trim_num(sellp[1])),
            format!("💰 {} %", trim_num(sellp[2])),
            format!("💰 {} %", trim_num(sellp[3])),
            format!("💰 {} %", trim_num(sellp[4])),
            "💰 X %".to_string(),
            "tr:s:0",
            "tr:s:1",
            "tr:s:2",
            "tr:s:3",
            "tr:s:4",
            "tr:sx",
        )
    } else {
        (
            format!("💰 {} SOL", trim_num(buy[0])),
            format!("💰 {} SOL", trim_num(buy[1])),
            format!("💰 {} SOL", trim_num(buy[2])),
            format!("💰 {} SOL", trim_num(buy[3])),
            format!("💰 {} SOL", trim_num(buy[4])),
            "💰 X SOL".to_string(),
            "tr:b:0",
            "tr:b:1",
            "tr:b:2",
            "tr:b:3",
            "tr:b:4",
            "tr:tx",
        )
    };
    let wallets = format!("💰 Active Wallets: {wallets}");
    let tip_l = format!("💰 Buy Tip: {} SOL", trim_num(tip));
    let slip_l = format!("🛍 Slippage: {}%", trim_num(slip));
    let bp_l = format!("{} Buy Protection", on_dot(bp));
    let at_l = format!("{} Auto Tip ({} SOL)", on_dot(auto_tip), trim_num(tip));
    let chart = chart_url(mint);
    serde_json::json!({
        "inline_keyboard": [
            [btn(switch, "tr:sw"), btn("🔄 Refresh", "tr:r")],
            [btn(&wallets, "tr:w")],
            [btn(&s0, d0), btn(&s1, d1), btn(&s2, d2)],
            [btn(&s3, d3), btn(&s4, d4), btn(&sx, dx)],
            [btn(&tip_l, "tr:tip"), btn(&slip_l, "tr:slip")],
            [btn("🌸 Generate PnL", "tr:pnl")],
            [btn(&at_l, "tr:at"), btn(&bp_l, "tr:bp")],
            [btn("🧬 Split tokens", "tr:sp")],
            [url_btn("📊 View Chart", &chart)],
            [btn("← Back", "p:home"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub(crate) fn buy_amount_keyboard(amounts: &[f64; 5]) -> serde_json::Value {
    let a0 = format!("💵 {} SOL", trim_num(amounts[0]));
    let a1 = format!("💵 {} SOL", trim_num(amounts[1]));
    let a2 = format!("💵 {} SOL", trim_num(amounts[2]));
    let a3 = format!("💵 {} SOL", trim_num(amounts[3]));
    let a4 = format!("💵 {} SOL", trim_num(amounts[4]));
    serde_json::json!({
        "inline_keyboard": [
            [btn(&a0, "ba:0"), btn(&a1, "ba:1"), btn(&a2, "ba:2")],
            [btn(&a3, "ba:3"), btn(&a4, "ba:4")],
            [btn("← Back", "set:pre"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub fn add_config_prompt() -> String {
    "Please send the following:\n\n• A Solana wallet address (e.g. 7xXtg2C…W8pAsU) — copies that wallet directly".into()
}

pub fn add_config_name_prompt(addr: &str) -> String {
    format!(
        "✅ Wallet added:\n`{addr}`\n\nNow send a name for this setup (e.g. Pablo)."
    )
}

pub fn add_config_name_retry() -> &'static str {
    "Send a short name for this wallet (e.g. Aaliya).\nDo not paste another Solana address."
}

/// Last token must be a Solana address. Anything before it is an optional setup name.
pub fn parse_watch_input(text: &str) -> Option<(Option<String>, String)> {
    let parts: Vec<&str> = text.split_whitespace().filter(|s| !s.is_empty()).collect();
    let addr = parts.last()?;
    if crate::config::parse_pubkey(addr).is_err() {
        return None;
    }
    let name = if parts.len() > 1 {
        let joined = parts[..parts.len() - 1].join(" ");
        let n = crate::profile::sanitize_watch_name(&joined);
        if n.is_empty() {
            None
        } else {
            Some(n)
        }
    } else {
        None
    };
    Some((name, (*addr).into()))
}

pub fn tasks_wallet_success() -> &'static str {
    "✅ Tasks wallet created successfully."
}

pub fn tasks_wallet_text() -> String {
    format!(
        "🌸 Manage your config wallet\n\nClick the button to switch to another wallet.\n\n🕐 Last updated: {}",
        last_updated_stamp()
    )
}

pub fn tasks_wallet_keyboard(picks: &[(String, usize)], back: &str) -> serde_json::Value {
    let mut rows: Vec<Vec<serde_json::Value>> = picks
        .iter()
        .map(|(label, i)| vec![btn(label, &format!("c:pickw:{i}"))])
        .collect();
    if rows.is_empty() {
        rows.push(vec![btn("🟢 W1 • 0 SOL", "c:pickw:0")]);
    }
    rows.push(vec![btn("← Back", back), btn("🗑 Close", "x:close")]);
    serde_json::json!({ "inline_keyboard": rows })
}

pub fn mass_create_prompt() -> String {
    let example = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    format!(
        "Please paste your list of wallet addresses below (maximum {MAX_WATCHED} addresses):\n\nFormat:\n• One wallet address per line\n• Solana addresses only\n\nExample:\n{example}\n{example}\n{example}"
    )
}

pub fn copy_keyboard(
    copy_on: bool,
    wallets: &[String],
    names: &HashMap<String, String>,
) -> serde_json::Value {
    let mark = if copy_on { "🟢" } else { "🔴" };
    let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut row = Vec::new();
    for (i, addr) in wallets.iter().enumerate() {
        let name = names
            .get(addr)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| short_wallet_label(addr));
        row.push(btn(&format!("{mark} {name}"), &format!("c:w:{i}")));
        if row.len() == 3 {
            rows.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows.push(vec![
        btn("🆕 Add new config", "c:add"),
        btn("🚛 Mass Create", "c:mass"),
    ]);
    rows.push(vec![btn("🔄 Update Tasks Wallet", "c:wallet")]);
    rows.push(vec![
        btn("⏹ Pause All", "c:pause"),
        btn("▶️ Start All", "c:start"),
        btn("🗑 Delete All", "c:delete"),
    ]);
    rows.push(vec![btn("← Back", "p:start"), btn("🔄 Refresh", "p:copy")]);
    rows.push(vec![btn("🗑 Close", "x:close")]);
    serde_json::json!({ "inline_keyboard": rows })
}

pub(crate) fn copy_keyboard_tasks(global_on: bool, tasks: &[CopyTask]) -> serde_json::Value {
    let mut names = HashMap::new();
    let mut wallets = Vec::new();
    for t in tasks {
        wallets.push(t.address.clone());
        names.insert(t.address.clone(), t.display_name());
    }
    let mut kb = copy_keyboard(global_on, &wallets, &names);
    if let Some(rows) = kb.get_mut("inline_keyboard").and_then(|v| v.as_array_mut()) {
        let mut i = 0usize;
        for row in rows.iter_mut() {
            if let Some(btns) = row.as_array_mut() {
                for b in btns {
                    let data = b.get("callback_data").and_then(|d| d.as_str()).unwrap_or("");
                    if data.starts_with("c:w:") {
                        if let Some(t) = tasks.get(i) {
                            let mark = on_dot(t.setup.active);
                            b["text"] = serde_json::Value::String(format!("{mark} {}", t.display_name()));
                        }
                        i += 1;
                    }
                }
            }
        }
    }
    kb
}

pub(crate) fn on_dot(on: bool) -> &'static str {
    if on {
        "🟢"
    } else {
        "🔴"
    }
}

pub(crate) fn trim_num(n: f64) -> String {
    let s = format!("{n:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(crate) fn fmt_na(v: Option<f64>) -> String {
    match v {
        None => "0".into(),
        Some(n) if n.abs() >= 1_000.0 => format!("{:.2}K", n / 1_000.0),
        Some(n) => trim_num(n),
    }
}

#[allow(dead_code)]
pub(crate) fn fmt_na_sol(v: Option<f64>) -> String {
    match v {
        None => "0 SOL".into(),
        Some(n) => format!("{} SOL", trim_num(n)),
    }
}

pub(crate) fn fmt_trigger(v: Option<f64>, q: QuoteCurrency) -> String {
    match v {
        None => format!("0 {}", q.label()),
        Some(n) => format!("{} {}", trim_num(n), q.label()),
    }
}

pub(crate) fn fmt_age(v: Option<f64>) -> String {
    match v {
        None => "0".into(),
        Some(n) if n >= 86_400.0 => format!("{:.0}d", n / 86_400.0),
        Some(n) if n >= 3_600.0 => format!("{:.0}h", n / 3_600.0),
        Some(n) if n >= 60.0 => format!("{:.0}m", n / 60.0),
        Some(n) => format!("{}s", trim_num(n)),
    }
}

pub(crate) fn mid_addr(addr: &str) -> String {
    let n = addr.chars().count();
    if n <= 10 {
        return addr.into();
    }
    let start: String = addr.chars().take(4).collect();
    let end: String = addr.chars().rev().take(4).collect::<String>().chars().rev().collect();
    format!("{start}…{end}")
}

pub(crate) fn md_esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('*', "\\*")
        .replace('`', "\\`")
        .replace('[', "\\[")
}

pub(crate) fn setup_cb(idx: usize, op: SetupOp) -> String {
    format!("s:{idx}:{}", op.code())
}

pub(crate) fn sbtn(idx: usize, op: SetupOp, text: &str) -> serde_json::Value {
    btn(text, &setup_cb(idx, op))
}

pub fn copy_setup_text(wallet_label: &str, task: &CopyTask, bot: &str, extra: Option<&str>) -> String {
    let s = &task.setup;
    let status = on_dot(s.active);
    let mev = if !s.buy_protection && !s.sell_protection {
        "Disabled".into()
    } else {
        format!("{} / {}", on_dot(s.buy_protection), on_dot(s.sell_protection))
    };
    format!(
        "🔎 Copy Trading Setup\n\n\
⚙️ Config processor: @{bot}\n\
👛 Copy wallet: {wallet}\n\
🔍 Config name: {cfg}\n\n\
🎯 Target Wallet\n\
- Status: {status} {active}\n\
└ Address:\n\
`{addr}`\n\n\
🔧 Configuration\n\
- Buy Config: {buy}\n\
- Buy Once: {once}\n\
- Follow Sells: {sells}\n\
└ MEV Protect: {mev}\n\n\
💰 Fees & Slippage\n\
- Priority Fee (Buy / Sell): {bfee} / {sfee}\n\
- Slippage (Buy / Sell): {bslip}% / {sslip}%\n\
- MEV Protect (Buy / Sell): {bprot} / {sprot}\n\
└ Tip (Buy / Sell): {btip} / {stip}\n\n\
🟢 Setup is active\n\
🔴 Setup is inactive\n\n\
Edit this task on {BOT_NAME} Manager.\n\
{extra}\n\
🕐 Last updated: {stamp}",
        active = if s.active { "Active" } else { "Inactive" },
        addr = task.address,
        buy = s.buy_mode.label(),
        once = on_dot(s.buy_once),
        sells = on_dot(s.follow_sells),
        bfee = trim_num(s.buy_fee_sol),
        sfee = trim_num(s.sell_fee_sol),
        bslip = trim_num(s.buy_slippage_pct),
        sslip = trim_num(s.sell_slippage_pct),
        bprot = on_dot(s.buy_protection),
        sprot = on_dot(s.sell_protection),
        btip = trim_num(s.buy_tip_sol),
        stip = trim_num(s.sell_tip_sol),
        extra = extra.unwrap_or(""),
        stamp = last_updated_stamp(),
        bot = md_esc(bot),
        wallet = md_esc(wallet_label),
        cfg = md_esc(&task.display_name()),
    )
}

pub fn copy_setup_keyboard(idx: usize, wallet_label: &str, task: &CopyTask) -> serde_json::Value {
    let s = &task.setup;
    let name = task.display_name();
    let target = mid_addr(&task.address);
    let rows = vec![
        vec![
            sbtn(idx, SetupOp::Wallet, &format!("👛 Wallet: {wallet_label}")),
            sbtn(idx, SetupOp::Name, &format!("🔍 Config Name: {name}")),
        ],
        vec![sbtn(
            idx,
            SetupOp::Target,
            &format!("🎯 Target Wallet: {target}"),
        )],
        vec![sbtn(
            idx,
            SetupOp::SellOnTransfer,
            &format!("{} Sell On Transfer", on_dot(s.sell_on_transfer)),
        )],
        vec![
            sbtn(
                idx,
                SetupOp::MinTrigger,
                &format!("Min Trigger Buy: {}", fmt_trigger(s.min_trigger_buy, s.min_trigger_quote)),
            ),
            sbtn(
                idx,
                SetupOp::MaxTrigger,
                &format!("Max Trigger Buy: {}", fmt_trigger(s.max_trigger_buy, s.max_trigger_quote)),
            ),
        ],
        vec![
            sbtn(idx, SetupOp::MinMcap, &format!("Min MCap: {}", fmt_na(s.min_mcap))),
            sbtn(idx, SetupOp::MaxMcap, &format!("Max MCap: {}", fmt_na(s.max_mcap))),
        ],
        vec![
            sbtn(
                idx,
                SetupOp::MinAge,
                &format!("Min Token Age: {}", fmt_age(s.min_token_age_s)),
            ),
            sbtn(
                idx,
                SetupOp::MaxAge,
                &format!("Max Token Age: {}", fmt_age(s.max_token_age_s)),
            ),
        ],
        vec![
            sbtn(
                idx,
                SetupOp::SkipDeploys,
                &format!("{} Skip Deploys", on_dot(s.skip_deploys)),
            ),
            sbtn(
                idx,
                SetupOp::FollowSells,
                &format!("{} Follow Sells", on_dot(s.follow_sells)),
            ),
        ],
        vec![sbtn(
            idx,
            SetupOp::BuyOnce,
            &format!("{} Buy Tokens Once", on_dot(s.buy_once)),
        )],
        vec![
            sbtn(
                idx,
                SetupOp::BuyExact,
                &format!("{} Buy Exact", on_dot(s.buy_mode == BuyMode::Exact)),
            ),
            sbtn(
                idx,
                SetupOp::BuyFixed,
                &format!("{} Buy Fixed", on_dot(s.buy_mode == BuyMode::Fixed)),
            ),
        ],
        vec![sbtn(
            idx,
            SetupOp::BuyAmount,
            &format!("Buy Amount: {} SOL", trim_num(s.buy_amount)),
        )],
        vec![
            sbtn(idx, SetupOp::BuyFee, &format!("Buy Fee: {} SOL", trim_num(s.buy_fee_sol))),
            sbtn(idx, SetupOp::SellFee, &format!("Sell Fee: {} SOL", trim_num(s.sell_fee_sol))),
        ],
        vec![
            sbtn(
                idx,
                SetupOp::BuySlip,
                &format!("Buy Slippage: {}%", trim_num(s.buy_slippage_pct)),
            ),
            sbtn(
                idx,
                SetupOp::SellSlip,
                &format!("Sell Slippage: {}%", trim_num(s.sell_slippage_pct)),
            ),
        ],
        vec![
            sbtn(
                idx,
                SetupOp::BuyProtect,
                &format!("{} Buy Protection", on_dot(s.buy_protection)),
            ),
            sbtn(
                idx,
                SetupOp::SellProtect,
                &format!("{} Sell Protection", on_dot(s.sell_protection)),
            ),
            sbtn(idx, SetupOp::AutoTip, &format!("{} Auto Tip", on_dot(s.auto_tip))),
        ],
        vec![
            sbtn(idx, SetupOp::BuyTip, &format!("💰 Buy Tip: {} SOL", trim_num(s.buy_tip_sol))),
            sbtn(idx, SetupOp::SellTip, &format!("💰 Sell Tip: {} SOL", trim_num(s.sell_tip_sol))),
        ],
        vec![
            sbtn(idx, SetupOp::Duplicate, "📋 Duplicate"),
            sbtn(idx, SetupOp::Strategies, "📈 Strategies"),
            sbtn(idx, SetupOp::Delete, "🗑 Delete"),
        ],
        vec![
            sbtn(idx, SetupOp::Active, &format!("{} Active", on_dot(s.active))),
            sbtn(idx, SetupOp::Refresh, "🔄 Refresh"),
        ],
        vec![btn("← Back", "p:copy")],
    ];
    serde_json::json!({ "inline_keyboard": rows })
}

pub(crate) fn fmt_pct2(n: f64) -> String {
    format!("{n:.2}%")
}

pub(crate) fn order_card(order: &LimitOrder, n: usize) -> String {
    let title = order.list_title(n);
    let icon = if title.starts_with("Stop") {
        "📉"
    } else {
        "📈"
    };
    format!(
        "{icon} {title}\n\
- Sell Amount: {}\n\
- Trigger at: {}\n\
- Tip: {} SOL\n\
- Slippage: {}\n\
└ Fee: {} SOL",
        fmt_pct2(order.sell_amount_pct),
        fmt_pct2(order.pnl_trigger()),
        trim_num(order.tip_sol),
        fmt_pct2(order.slippage_pct),
        trim_num(order.fee_sol),
    )
}

pub(crate) fn strategies_text(task: &CopyTask) -> String {
    let mut lines = vec!["🔎 Strategies".into(), String::new()];
    if task.orders.is_empty() {
        lines.push("No orders yet. Create a limit order to add Take Profit or Stop Loss.".into());
    } else {
        for (i, o) in task.orders.iter().enumerate() {
            lines.push(order_card(o, i + 1));
            lines.push(String::new());
        }
    }
    lines.push(format!("Manage your orders on {BOT_NAME} Manager."));
    lines.push(String::new());
    lines.push("📄 Docs".into());
    lines.push(String::new());
    lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
    lines.join("\n")
}

pub(crate) fn strategies_keyboard(idx: usize, task: &CopyTask) -> serde_json::Value {
    let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
    for (i, o) in task.orders.iter().enumerate() {
        rows.push(vec![
            btn(&format!("📈 {}", o.list_title(i + 1)), &format!("g:{idx}:O:{}", o.id)),
            btn("🗑 Delete Order", &format!("g:{idx}:X:{}", o.id)),
        ]);
    }
    rows.push(vec![btn("🆕 Create Limit Order", &format!("g:{idx}:N"))]);
    rows.push(vec![
        btn("← Back", &format!("c:w:{idx}")),
        btn("🔄 Refresh", &format!("g:{idx}:R")),
    ]);
    serde_json::json!({ "inline_keyboard": rows })
}

pub(crate) fn limit_order_text() -> String {
    "🔎 Create Limit Order".into()
}

pub(crate) fn sell_at_pnl_prompt() -> String {
    "Enter your target value:\n\
• Take Profit: e.g. 50%, 150%.\n\
• Stop Loss: e.g. –20%, –50%.\n\
\n\
Examples:\n\
• Profit %: 100%: your holdings will be sold at 2x.\n\
• Loss %: –50%: your holdings will be sold when the price drops by 50%.\n\
\n\
Notes:\n\
• Only one % value (TP or SL) per limit order."
        .into()
}

pub(crate) fn parse_strat_number(text: &str) -> Option<f64> {
    let t = text
        .trim()
        .trim_end_matches('%')
        .replace(['–', '—', '−'], "-")
        .replace(',', "")
        .trim()
        .to_string();
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

pub(crate) fn mode_dot(draft: &LimitOrder, mode: StrategyMode) -> &'static str {
    on_dot(draft.picked && draft.mode == mode)
}

pub(crate) fn limit_order_keyboard(idx: usize, draft: &LimitOrder) -> serde_json::Value {
    serde_json::json!({
        "inline_keyboard": [
            [
                btn(&format!("{} Profit/Loss", mode_dot(draft, StrategyMode::ProfitLoss)), &format!("g:{idx}:M:pl")),
                btn(&format!("{} Fixed Mcap", mode_dot(draft, StrategyMode::FixedMcap)), &format!("g:{idx}:M:fm")),
            ],
            [btn(&format!("💵 Sell at: {}", fmt_pct2(draft.sell_at_pct)), &format!("g:{idx}:V:sa"))],
            [
                btn(&format!("💰 Sell Amount: {}", fmt_pct2(draft.sell_amount_pct)), &format!("g:{idx}:V:am")),
                btn(&format!("🛍️ Slippage: {}", fmt_pct2(draft.slippage_pct)), &format!("g:{idx}:V:sl")),
            ],
            [
                btn(&format!("{} Sell Protection", on_dot(draft.sell_protection)), &format!("g:{idx}:T:p")),
                btn(&format!("{} High Impact", on_dot(draft.high_impact)), &format!("g:{idx}:T:h")),
            ],
            [
                btn(&format!("💵 Tip: {} SOL", trim_num(draft.tip_sol)), &format!("g:{idx}:V:ti")),
                btn(&format!("💵 Fee: {} SOL", trim_num(draft.fee_sol)), &format!("g:{idx}:V:fe")),
            ],
            [
                btn("← Back", &format!("g:{idx}:L")),
                btn("🆕 Create Order", &format!("g:{idx}:C")),
            ],
            [btn("🗑 Close", "x:close")],
        ]
    })
}

pub(crate) fn setup_value_prompt(field: SetupValue) -> String {
    match field {
        SetupValue::MinTrigger => "Send min trigger buy amount, or `n/a`.".into(),
        SetupValue::MaxTrigger => "Send max trigger buy amount, or `n/a`.".into(),
        SetupValue::MinMcap => "Send min market cap in USD, or `n/a`.".into(),
        SetupValue::MaxMcap => "Send max market cap in USD, or `n/a`.".into(),
        SetupValue::MinAge => {
            "Send min token age (`30s`, `5m`, `2h`, `1d`, `1mo`) or seconds. `m` = minutes. Or `n/a`."
                .into()
        }
        SetupValue::MaxAge => {
            "Send max token age (`30s`, `5m`, `2h`, `1d`, `1mo`) or seconds. `m` = minutes. Or `n/a`."
                .into()
        }
        SetupValue::BuyAmount => "Send buy amount in SOL.".into(),
        SetupValue::BuyFee => "Send buy priority fee in SOL.".into(),
        SetupValue::SellFee => "Send sell priority fee in SOL.".into(),
        SetupValue::BuySlip => "Send buy slippage percent (0–100).".into(),
        SetupValue::SellSlip => "Send sell slippage percent (0–100).".into(),
        SetupValue::BuyTip => "Send buy tip in SOL.".into(),
        SetupValue::SellTip => "Send sell tip in SOL.".into(),
    }
}

pub(crate) fn parse_setup_number(text: &str, field: SetupValue) -> std::result::Result<Option<f64>, String> {
    if matches!(field, SetupValue::MinAge | SetupValue::MaxAge) {
        return parse_duration_secs(text);
    }
    let t = text.trim().to_ascii_lowercase();
    let optional = matches!(
        field,
        SetupValue::MinTrigger
            | SetupValue::MaxTrigger
            | SetupValue::MinMcap
            | SetupValue::MaxMcap
            | SetupValue::MinAge
            | SetupValue::MaxAge
    );
    if optional && (t.is_empty() || t == "n/a" || t == "na" || t == "none" || t == "clear") {
        return Ok(None);
    }
    let n: f64 = t
        .parse()
        .map_err(|_| "Send a number (or `n/a`).".to_string())?;
    if !n.is_finite() || n < 0.0 {
        return Err("Need a number ≥ 0.".into());
    }
    Ok(Some(n))
}

pub(crate) fn apply_setup_number(setup: &mut CopySetup, field: SetupValue, v: Option<f64>) {
    match field {
        SetupValue::MinTrigger => setup.min_trigger_buy = v,
        SetupValue::MaxTrigger => setup.max_trigger_buy = v,
        SetupValue::MinMcap => setup.min_mcap = v,
        SetupValue::MaxMcap => setup.max_mcap = v,
        SetupValue::MinAge => setup.min_token_age_s = v,
        SetupValue::MaxAge => setup.max_token_age_s = v,
        SetupValue::BuyAmount => setup.buy_amount = v.unwrap_or(setup.buy_amount),
        SetupValue::BuyFee => setup.buy_fee_sol = v.unwrap_or(setup.buy_fee_sol),
        SetupValue::SellFee => setup.sell_fee_sol = v.unwrap_or(setup.sell_fee_sol),
        SetupValue::BuySlip => {
            setup.buy_slippage_pct = v.unwrap_or(setup.buy_slippage_pct).clamp(0.0, 100.0);
        }
        SetupValue::SellSlip => {
            setup.sell_slippage_pct = v.unwrap_or(setup.sell_slippage_pct).clamp(0.0, 100.0);
        }
        SetupValue::BuyTip => setup.buy_tip_sol = v.unwrap_or(setup.buy_tip_sol),
        SetupValue::SellTip => setup.sell_tip_sol = v.unwrap_or(setup.sell_tip_sol),
    }
}

pub fn split_keyboard() -> serde_json::Value {
    serde_json::json!({
        "inline_keyboard": [
            [btn("25%", "spl:25"), btn("50%", "spl:50"), btn("75%", "spl:75")],
            [btn("Max", "spl:max"), btn("X", "spl:x")],
            [btn("← Back", "tr:r"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub fn withdraw_keyboard() -> serde_json::Value {
    serde_json::json!({
        "inline_keyboard": [
            [btn("25%", "wd:25"), btn("50%", "wd:50"), btn("75%", "wd:75")],
            [btn("Max", "wd:max"), btn("X", "wd:x")],
            [btn("← Back", "p:home"), btn("🗑 Close", "x:close")],
        ]
    })
}

pub fn wallets_keyboard() -> serde_json::Value {
    inline_grid(&[
        &[("💸 Withdraw", "p:withdraw"), ("🪞 Copy", "p:copy")],
        &[("👀 Watch", "p:watch"), ("⚙️ Settings", "p:settings")],
        &nav_row(),
    ])
}

pub fn simple_keyboard() -> serde_json::Value {
    inline_grid(&[
        &[("🏠 Home", "p:home"), ("⚙️ Settings", "p:settings")],
        &nav_row(),
    ])
}

pub fn positions_keyboard(usd: bool) -> serde_json::Value {
    let usd_l = if usd { "🟢 USD" } else { "🔴 USD" };
    inline_grid(&[
        &[
            ("🏠 Homepage", "p:start"),
            (usd_l, "pos:usd"),
            ("🔄 Refresh", "p:positions"),
        ],
        &[("🗑 Delete", "x:close")],
    ])
}

pub(crate) fn md_link_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('_', "\\_")
        .replace('*', "\\*")
}

pub(crate) fn format_open_position_line(
    name: &str,
    pnl_pct: f64,
    sol: f64,
    usd: bool,
    sol_usd: f64,
    hide_pnl: bool,
    mint: Option<&str>,
) -> String {
    let pnl = if hide_pnl {
        "•••".to_string()
    } else {
        let dot = if pnl_pct >= 0.0 { "🟢" } else { "🔴" };
        format!("{dot} {pnl_pct:.2}%")
    };
    let bag = if usd {
        format!("${:.2}", sol * sol_usd)
    } else {
        format!("{sol:.4} SOL")
    };
    let label = if let Some(m) = mint {
        format!(
            "[{}](https://t.me/{BOT_USERNAME}?start=ca_{m})",
            md_link_label(name)
        )
    } else {
        name.to_string()
    };
    format!("💰 {label} {pnl} - {bag}")
}

pub fn start_ca_arg(args: &str) -> Option<String> {
    let t = args.trim();
    if t.is_empty() {
        return None;
    }
    extract_mint(t.strip_prefix("ca_").unwrap_or(t))
}

pub(crate) fn keyboard_for(cmd: Command, copy_on: bool) -> Option<serde_json::Value> {
    Some(match cmd {
        Command::Menu | Command::Help | Command::Home | Command::Start => home_keyboard(),
        Command::Settings => settings_keyboard(&BotSettings::default()),
        Command::Copy | Command::Watch | Command::Unwatch | Command::MaxSol
        | Command::CopyOn | Command::CopyOff => copy_keyboard(copy_on, &[], &HashMap::new()),
        Command::Wallets => wallets_settings_keyboard(&["🟢 W1 · 0 SOL".into()]),
        Command::Withdraw => withdraw_keyboard(),
        Command::Positions => positions_keyboard(false),
        Command::Orders | Command::Bots | Command::Pumpfun => simple_keyboard(),
        Command::Calendar => calendar_keyboard(&CalView::for_unix(pnl::now_unix())),
        Command::Stop => return None,
    })
}

pub fn help_text() -> String {
    let mut lines = vec![
        format!("✨ {BOT_NAME} (@{BOT_USERNAME})"),
        "👇 Tap a command, or type it.".into(),
        String::new(),
    ];
    for (cmd, desc) in MENU {
        let icon = menu_icon(cmd);
        lines.push(format!("{icon} /{cmd} — {desc}"));
    }
    lines.push("💰 /maxsol 0.05 — Set your copy size in SOL".into());
    lines.push("▶️ /copyon  ⏸️ /copyoff — Pause copies without dropping wallets".into());
    lines.join("\n")
}

pub(crate) fn menu_icon(cmd: &str) -> &'static str {
    match cmd {
        "menu" => "📋",
        "start" => "🚀",
        "home" => "🏠",
        "calendar" => "📅",
        "positions" => "📊",
        "copy" => "🪞",
        "watch" => "👀",
        "unwatch" => "🚫",
        "orders" => "📝",
        "settings" => "⚙️",
        "wallets" => "👛",
        "withdraw" => "💸",
        "bots" => "🤖",
        "pumpfun" => "🎃",
        _ => "•",
    }
}

pub(crate) fn bal_label(lamports: Option<u64>) -> String {
    match lamports {
        Some(n) => format!("{} SOL", fmt_sol(n)),
        None => "—".into(),
    }
}

pub(crate) fn event_line(e: &MainnetEvent) -> String {
    let kind = if e.copied { "copy" } else { "skip" };
    format!(
        "{kind}  slot {}  {} SOL  `{}`\nmint `{}`\nsig `{}`\ndetect {} ms · {}",
        e.slot,
        fmt_sol(e.sol_lamports),
        e.wallet,
        e.mint,
        e.signature,
        e.notify_to_tx_ms,
        e.verdict
    )
}

pub(crate) fn last_updated_stamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let micros = dur.subsec_micros();
    format!(
        "{:02}:{:02}:{:02}.{:06} UTC",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        micros
    )
}

pub(crate) fn recent_copies(live: &LiveSnap, n: usize) -> String {
    let copies: Vec<&MainnetEvent> = live.events.iter().filter(|e| e.copied).take(n).collect();
    if copies.is_empty() {
        return "🌙 no copy signals this session yet".into();
    }
    copies.into_iter().map(event_line).collect::<Vec<_>>().join("\n\n")
}

pub(crate) fn welcome_text(created: &CreatedWallet, bal: Option<u64>, sol_usd: Option<f64>) -> String {
    let w = &created.wallet;
    let sol = match bal {
        Some(0) | None => "0".into(),
        Some(n) => fmt_sol(n),
    };
    let usd = match (bal.unwrap_or(0), sol_usd) {
        (n, Some(px)) if px.is_finite() && px > 0.0 => {
            format!("${:.2}", n as f64 / 1_000_000_000.0 * px)
        }
        _ => "—".into(),
    };
    let mut lines = vec![
        format!("🌸 Welcome to {BOT_NAME}!"),
        String::new(),
        "Let your trading journey blossom with us!".into(),
        String::new(),
        format!("🔥 {BOT_NAME} Manager is live. Launch it now."),
        String::new(),
        "🟣 Your Solana Wallet Address:".into(),
        format!("├ {}: `{}`", w.label, w.pubkey),
        format!("└ Balance: {sol} SOL (USDT {usd})"),
    ];
    if created.created {
        if let Some(sk) = w.secret_key_b58() {
            lines.push(String::new());
            lines.push("🔐 Private key — save this now. It is shown only this once.".into());
            lines.push(format!("`{sk}`"));
            lines.push("Import this in Phantom / Solflare. Anyone with it can take your SOL.".into());
        }
    }
    if bal.unwrap_or(0) == 0 {
        lines.push(String::new());
        lines.push("🔴 You currently have no SOL in your wallet.".into());
        lines.push("Deposit SOL to your address to start trading.".into());
    }
    lines.push(String::new());
    lines.push(format!("🕐 Last updated: {}", last_updated_stamp()));
    lines.join("\n")
}

