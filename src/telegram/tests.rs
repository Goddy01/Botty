use super::*;
use super::hub::TradeView;
use super::parse::{AwaitKind, SetupValue};
use super::ui::{
    apply_setup_number, buy_amount_keyboard, format_open_position_line, fmt_bloom_price,
    parse_strat_number, presets_keyboard, sell_percentage_keyboard, trade_keyboard,
    wallets_settings_keyboard, welcome_text,
};
use crate::config::WatchKind;
use crate::pnl::{CalView, PnlStore};
use crate::positions::PositionStore;
use crate::profile::{
    CopySetup, LimitOrder, ProfileDefaults, StrategyMode, WatchBook,
};
use crate::rpc_read::RpcRead;
use crate::user_wallet::{CreatedWallet, UserWallet, WalletStore};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

fn sample_card() -> StatusCard {
    StatusCard {
        watch: WatchKind::Rpc,
        wallets: vec!["Abc".into()],
        our_pubkey: "Payer".into(),
        max_sol_lamports: 50_000_000,
        cooldown_ms: 2000,
        tips_lamports: [1_000_000, 3_000_000, 8_000_000],
    }
}

#[test]
fn menu_has_bloom_slugs() {
    let cmds: Vec<&str> = MENU.iter().map(|(c, _)| *c).collect();
    assert_eq!(cmds.first().copied(), Some("menu"));
    assert!(cmds.contains(&"start"));
    assert!(cmds.contains(&"copy"));
    assert!(cmds.contains(&"wallets"));
    assert!(cmds.contains(&"pumpfun"));
    assert_eq!(MENU.len(), 14);
}

#[test]
fn parses_menu_and_mentions() {
    assert_eq!(parse_command("/menu"), Some(Command::Menu));
    assert_eq!(parse_command("/start"), Some(Command::Start));
    assert_eq!(
        parse_command(&format!("/start@{BOT_USERNAME}")),
        Some(Command::Start)
    );
    assert_eq!(parse_command("/copy"), Some(Command::Copy));
    assert_eq!(parse_command("/watch"), Some(Command::Watch));
    assert_eq!(parse_command("/unwatch"), Some(Command::Unwatch));
    assert_eq!(command_args("/watch  Abc  "), "Abc");
    assert_eq!(parse_command("/home please"), Some(Command::Home));
    assert_eq!(parse_callback("p:calendar"), Some(CallbackAction::Page(Command::Calendar)));
    assert_eq!(parse_callback("cal:w:3"), Some(CallbackAction::Cal(CalOp::Week(3))));
    assert_eq!(parse_callback("cal:usd"), Some(CallbackAction::Cal(CalOp::Usd)));
    assert_eq!(parse_callback("t:copy"), Some(CallbackAction::ToggleCopy));
    assert_eq!(parse_callback("c:add"), Some(CallbackAction::CopyAdd));
    assert_eq!(parse_callback("c:w:0"), Some(CallbackAction::CopySetup(0)));
    assert_eq!(
        parse_callback("q:0:mit:usd"),
        Some(CallbackAction::QuoteUnit {
            idx: 0,
            min: true,
            unit: QuotePick::Usdt
        })
    );
    assert_eq!(parse_callback("c:w:12"), Some(CallbackAction::CopySetup(12)));
    assert_eq!(parse_callback("c:pause"), Some(CallbackAction::CopyPauseAll));
    assert_eq!(parse_callback("c:wallet"), Some(CallbackAction::CopyUpdateWallet));
    assert_eq!(parse_callback("c:pickw1"), Some(CallbackAction::CopyPickWallet(0)));
    assert_eq!(parse_callback("c:pickw:2"), Some(CallbackAction::CopyPickWallet(2)));
    assert_eq!(parse_callback("x:close"), Some(CallbackAction::Close));
    assert_eq!(parse_command("hello"), None);
    assert_eq!(parse_command("/sniper"), None);
    assert_eq!(parse_command("/social"), None);
    assert_eq!(parse_command("/afk"), None);
    assert_eq!(parse_command("/referral"), None);
    assert_eq!(parse_command("/bridge"), None);
    assert_eq!(parse_command("/chains"), None);
    assert_eq!(parse_command("/suggestions"), None);
}

#[test]
fn parses_allow_ids() {
    let ids = crate::config::parse_allow_ids("111, 222 333").unwrap();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&222));
    let (ids, names) =
        crate::config::parse_allow_list("@toshirothe3rd, @Gambloorr, 999").unwrap();
    assert!(ids.contains(&999));
    assert!(names.contains("toshirothe3rd"));
    assert!(names.contains("gambloorr"));
}

#[test]
fn bloom_submenus_are_two_columns() {
    for kb in [
        wallets_keyboard(),
        simple_keyboard(),
    ] {
        let rows = kb["inline_keyboard"].as_array().expect("rows");
        assert!(rows.len() >= 2);
        for row in rows {
            assert_eq!(row.as_array().expect("row").len(), 2);
        }
        let last = rows.last().unwrap().as_array().unwrap();
        assert!(last[0]["text"].as_str().unwrap().contains("Back"));
        assert!(last[1]["text"].as_str().unwrap().contains("Close"));
    }
    let set = settings_keyboard(&BotSettings::default());
    let flat = serde_json::to_string(&set).unwrap();
    assert!(flat.contains("👛 Wallets"));
    assert!(flat.contains("🔧 Presets"));
    assert!(!flat.contains("Slippage"));
    assert!(flat.contains("🔴 Buy Protection"));
    assert!(flat.contains("🔴 Private PNL"));
    assert!(!flat.contains("Fee"));
    assert!(!flat.contains("Currency"));
    assert!(!flat.contains("Degen"));
    assert!(!flat.contains("Expert"));
    assert!(!flat.contains("🇺🇸"));
    assert!(!flat.contains("🇨🇳"));
    assert!(!flat.contains("🔴 Copies"));
    let wkb = wallets_settings_keyboard(&["🟢 W1 · 0 SOL".into()]);
    let wflat = serde_json::to_string(&wkb).unwrap();
    assert!(wflat.contains("💵 Create Wallet"));
    assert!(wflat.contains("🔑 Import Wallet"));
    assert!(wflat.contains("Export Private Key"));
    assert!(!wflat.contains("Cashback"));
    assert!(!wflat.contains("PF "));
    assert_eq!(parse_callback("set:dg"), Some(CallbackAction::SetOp(SetOp::Degen)));
    assert_eq!(parse_callback("w:new"), Some(CallbackAction::SetOp(SetOp::WalCreate)));
    assert_eq!(parse_callback("w:exp"), Some(CallbackAction::SetOp(SetOp::WalExport)));
    assert_eq!(parse_callback("w:show"), Some(CallbackAction::SetOp(SetOp::WalExportShow)));
    let pre = serde_json::to_string(&presets_keyboard()).unwrap();
    assert!(pre.contains("Buy Presets"));
    assert!(pre.contains("Sell Presets"));
    assert!(!pre.contains("Spot Presets"));
    assert!(!pre.contains("Copy Presets"));
    assert!(!pre.contains("AFK"));
    assert!(!pre.contains("Social"));
    let buy = serde_json::to_string(&buy_amount_keyboard(&crate::profile::default_buy_presets())).unwrap();
    assert!(buy.contains("1.8 SOL"));
    assert!(buy.contains("4 SOL"));
    assert!(buy.contains("5.05 SOL"));
    assert!(buy.contains("3.5 SOL"));
    assert!(buy.contains("10 SOL"));
    assert_eq!(parse_callback("pre:buy"), Some(CallbackAction::SetOp(SetOp::BuyPresets)));
    assert_eq!(parse_callback("ba:0"), Some(CallbackAction::SetOp(SetOp::EditBuyAmt(0))));
    let sell = serde_json::to_string(&sell_percentage_keyboard(&crate::profile::default_sell_presets())).unwrap();
    assert!(sell.contains("20 %"));
    assert!(sell.contains("40 %"));
    assert!(sell.contains("70 %"));
    assert!(sell.contains("50 %"));
    assert!(sell.contains("100 %"));
    assert_eq!(parse_callback("pre:sell"), Some(CallbackAction::SetOp(SetOp::SellPresets)));
    assert_eq!(parse_callback("sp:4"), Some(CallbackAction::SetOp(SetOp::EditSellAmt(4))));
    assert_eq!(parse_callback("tr:sw"), Some(CallbackAction::Trade(TradeOp::Switch)));
    assert_eq!(parse_callback("tr:b:0"), Some(CallbackAction::Trade(TradeOp::Buy(0))));
    assert_eq!(parse_callback("tr:s:3"), Some(CallbackAction::Trade(TradeOp::Sell(3))));
    let mint = "So11111111111111111111111111111111111111112";
    assert_eq!(extract_mint(mint).as_deref(), Some(mint));
    assert_eq!(
        extract_mint(&format!("https://pump.fun/coin/{mint}")).as_deref(),
        Some(mint)
    );
    assert_eq!(
        extract_mint("FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5?").as_deref(),
        Some("FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5")
    );
    assert!(fmt_bloom_price(Some(0.0001672)).starts_with("$0.0"));
    assert_eq!(parse_callback("tr:at"), Some(CallbackAction::Trade(TradeOp::AutoTip)));
    assert_eq!(parse_callback("tr:hide"), Some(CallbackAction::Trade(TradeOp::Hide)));
    assert_eq!(parse_callback("wd:25"), Some(CallbackAction::Withdraw(WithdrawOp::Pct(25))));
    assert_eq!(parse_callback("wd:max"), Some(CallbackAction::Withdraw(WithdrawOp::Max)));
    assert_eq!(parse_callback("wd:x"), Some(CallbackAction::Withdraw(WithdrawOp::Custom)));
    assert_eq!(parse_callback("spl:25"), Some(CallbackAction::Split(WithdrawOp::Pct(25))));
    assert_eq!(parse_callback("spl:max"), Some(CallbackAction::Split(WithdrawOp::Max)));
    assert_eq!(parse_callback("spl:x"), Some(CallbackAction::Split(WithdrawOp::Custom)));
    let wkb = serde_json::to_string(&withdraw_keyboard()).unwrap();
    assert!(wkb.contains("25%"));
    assert!(wkb.contains("50%"));
    assert!(wkb.contains("75%"));
    assert!(wkb.contains("Max"));
    assert!(wkb.contains("wd:x"));
    let buy_kb = serde_json::to_string(&trade_keyboard(
        false,
        &crate::profile::default_buy_presets(),
        &crate::profile::default_sell_presets(),
        0.001,
        10.0,
        false,
        false,
        1,
        mint,
    ))
    .unwrap();
    assert!(buy_kb.contains("1.8 SOL"));
    assert!(buy_kb.contains("Switch to Sell"));
    assert!(buy_kb.contains("dexscreener.com/solana/"));
    assert!(buy_kb.contains("chartType=mcap"));
    assert!(buy_kb.contains("Auto Tip"));
    assert!(!buy_kb.contains("Hide Position"));
    assert!(!buy_kb.contains("Currency"));
    assert!(!buy_kb.contains("20 %"));
    let sell_kb = serde_json::to_string(&trade_keyboard(
        true,
        &crate::profile::default_buy_presets(),
        &crate::profile::default_sell_presets(),
        0.001,
        10.0,
        false,
        true,
        1,
        mint,
    ))
    .unwrap();
    assert!(sell_kb.contains("20 %"));
    assert!(sell_kb.contains("100 %"));
    assert!(sell_kb.contains("Switch to Buy"));
    assert!(!sell_kb.contains("1.8 SOL"));
}

#[test]
fn start_menu_has_requested_items() {
    let kb = home_keyboard();
    let rows = kb["inline_keyboard"].as_array().expect("rows");
    let labels: Vec<&str> = rows
        .iter()
        .flat_map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|b| b["text"].as_str().unwrap())
        })
        .collect();
    assert_eq!(
        labels,
        [
            "💼 Positions",
            "🤖 Copy Trade",
            "⚙️ Settings",
            "💸 Withdraw",
            "🔄 Refresh",
            "📅 PnL Calendar",
            "🗑 Close",
        ]
    );
    assert!(!labels.iter().any(|l| l.contains("Limit Orders")));
    assert_eq!(rows[0].as_array().unwrap().len(), 1);
    assert_eq!(rows.last().unwrap().as_array().unwrap().len(), 1);
    assert_eq!(parse_callback("p:refresh"), Some(CallbackAction::Page(Command::Start)));
}

#[test]
fn tasks_wallet_screen_matches_bloom() {
    let text = tasks_wallet_text();
    assert!(text.contains("Manage your config wallet"));
    assert!(text.contains("Click the button to switch"));
    assert!(text.contains("Last updated"));
    let kb = tasks_wallet_keyboard(&[("W1 • 0 SOL".into(), 0)], "p:copy");
    let rows = kb["inline_keyboard"].as_array().unwrap();
    assert_eq!(rows[0][0]["text"], "W1 • 0 SOL");
    assert_eq!(rows[0][0]["callback_data"], "c:pickw:0");
    assert!(rows[1][0]["text"].as_str().unwrap().contains("Back"));
    assert!(rows[1][1]["text"].as_str().unwrap().contains("Close"));
    assert!(tasks_wallet_success().contains("created successfully"));
}

#[test]
fn add_config_prompt_asks_only_for_wallet() {
    let text = add_config_prompt();
    assert!(text.contains("Please send the following"));
    assert!(text.contains("Solana wallet address"));
    assert!(!text.contains("fomo.family"));
    assert!(!text.contains("username"));
    assert!(add_config_name_prompt("9LXW").contains("send a name"));
    let mass = mass_create_prompt();
    assert!(mass.contains("maximum 90"));
    assert!(mass.contains("One wallet address per line"));
    assert!(mass.contains("Solana addresses only"));
}

#[test]
fn positions_menu_matches_bloom_layout() {
    let kb = positions_keyboard(false);
    let rows = kb["inline_keyboard"].as_array().expect("rows");
    let labels: Vec<&str> = rows
        .iter()
        .flat_map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|b| b["text"].as_str().unwrap())
        })
        .collect();
    assert_eq!(
        labels,
        [
            "🏠 Homepage",
            "🔴 USD",
            "🔄 Refresh",
            "🗑 Delete",
        ]
    );
    assert!(!labels.iter().any(|s| s.contains("Min Value")));
    assert!(!labels.iter().any(|s| s.contains("Sell Position")));
    assert_eq!(
        format_open_position_line("Licker", 11.80, 0.1152, false, 150.0, false, None),
        "💰 Licker 🟢 11.80% - 0.1152 SOL"
    );
    assert_eq!(
        format_open_position_line("Licker", -12.5, 0.01, false, 150.0, false, None),
        "💰 Licker 🔴 -12.50% - 0.0100 SOL"
    );
    assert_eq!(
        format_open_position_line("Licker", 11.80, 0.1152, true, 150.0, false, None),
        "💰 Licker 🟢 11.80% - $17.28"
    );
    assert_eq!(
        format_open_position_line("Licker", 11.80, 0.1152, false, 150.0, true, None),
        "💰 Licker ••• - 0.1152 SOL"
    );
    let linked = format_open_position_line(
        "Licker",
        11.80,
        0.1152,
        false,
        150.0,
        false,
        Some("FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5"),
    );
    assert!(linked.contains("[Licker](https://t.me/copythattrader_bot?start=ca_FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5)"));
    assert_eq!(
        parse_callback("pos:ca:FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5"),
        Some(CallbackAction::OpenTrade(
            "FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5".into()
        ))
    );
    assert_eq!(
        start_ca_arg("ca_FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5"),
        Some("FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5".into())
    );
    assert_eq!(parse_callback("pos:min"), Some(CallbackAction::Page(Command::Positions)));
    assert_eq!(parse_callback("pos:usd"), Some(CallbackAction::PosUsd));
    assert_eq!(parse_callback("pos:del"), Some(CallbackAction::Close));
    assert_eq!(parse_callback("p:positions"), Some(CallbackAction::Page(Command::Positions)));
    assert_eq!(positions_keyboard(true)["inline_keyboard"][0][1]["text"], "🟢 USD");
    let del = serde_json::to_string(&positions_keyboard(false)).unwrap();
    assert!(del.contains("x:close"));
    assert!(!del.contains("pos:del"));
}

#[test]
fn copy_menu_matches_bloom_layout() {
    let kb = copy_keyboard(true, &[], &HashMap::new());
    let rows = kb["inline_keyboard"].as_array().expect("rows");
    let labels: Vec<&str> = rows
        .iter()
        .flat_map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|b| b["text"].as_str().unwrap())
        })
        .collect();
    assert_eq!(
        labels,
        [
            "🆕 Add new config",
            "🚛 Mass Create",
            "🔄 Update Tasks Wallet",
            "⏹ Pause All",
            "▶️ Start All",
            "🗑 Delete All",
            "← Back",
            "🔄 Refresh",
            "🗑 Close",
        ]
    );
    assert_eq!(rows[1].as_array().unwrap().len(), 1);
    assert_eq!(rows[2].as_array().unwrap().len(), 3);
    assert_eq!(rows.last().unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn status_and_wallets_render() {
    let card = sample_card();
    assert!(card.render().contains("0.0500 SOL"));
    assert!(card.wallets_text().contains("Abc"));
    assert!(card.settings_text().contains("mainnet"));
    assert!(card.home_text().contains("/menu"));
    assert!(card.render().contains("mainnet-beta"));
}

fn test_hub() -> Hub {
    let http = reqwest::Client::new();
    let dir = std::env::temp_dir().join(format!(
        "copybot-tg-wallets-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let profiles = std::env::temp_dir().join(format!(
        "copybot-tg-profiles-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let posdir = std::env::temp_dir().join(format!(
        "copybot-tg-pos-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pnldir = std::env::temp_dir().join(format!(
        "copybot-tg-pnl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    Hub {
        token: "x".into(),
        http: http.clone(),
        allow: HashSet::from([42]),
        allow_names: HashSet::from(["toshirothe3rd".into()]),
        chats: Arc::new(Mutex::new(HashSet::new())),
        status: sample_card(),
        rpc: RpcRead::new(http, String::new()).with_sol_usd(150.0),
        live: Arc::new(crate::live::LiveBoard::new()),
        wallets: Arc::new(WalletStore::open(dir).unwrap()),
        profiles: Arc::new(
            WatchBook::open(
                profiles,
                ProfileDefaults {
                    max_sol: 0.05,
                    cooldown_ms: 2000,
                },
            )
            .unwrap(),
        ),
        positions: Arc::new(PositionStore::open(posdir).unwrap()),
        pnl: Arc::new(PnlStore::open(pnldir).unwrap()),
        cal: Arc::new(Mutex::new(HashMap::new())),
        awaiting: Arc::new(Mutex::new(HashMap::new())),
        await_prompt: Arc::new(Mutex::new(HashMap::new())),
        last_bot_msg: Arc::new(Mutex::new(HashMap::new())),
        strat_drafts: Arc::new(Mutex::new(HashMap::new())),
        trade: Arc::new(Mutex::new(HashMap::new())),
        withdraw_pct: Arc::new(Mutex::new(HashMap::new())),
        split_pct: Arc::new(Mutex::new(HashMap::new())),
        engine: Arc::new(Mutex::new(None)),
        tasks_wallet_back: Arc::new(Mutex::new(HashMap::new())),
    }
}

#[test]
fn tracks_prompt_and_last_bot_ids() {
    let hub = test_hub();
    hub.remember_prompt(42, 10);
    assert_eq!(hub.prompt_message(42), Some(10));
    assert_eq!(hub.last_bot_msg(42), Some(10));
    hub.forget_prompt(42);
    assert_eq!(hub.prompt_message(42), None);
    hub.forget_bot_msg(42, 10);
    assert_eq!(hub.last_bot_msg(42), None);
    hub.remember_bot_msg(42, 11);
    hub.forget_bot_msg(42, 99);
    assert_eq!(hub.last_bot_msg(42), Some(11));
}

#[tokio::test]
async fn deny_unknown_user() {
    let hub = test_hub();
    let reply = hub.reply_for(7, None, 7, Command::Start).await;
    assert!(reply.text.contains("not on the allowlist"));
    assert!(hub.chats.lock().unwrap().is_empty());
    let ok = hub.reply_for(42, None, 99, Command::Start).await;
    assert!(ok.text.contains("Welcome to"));
    assert!(ok.text.contains("blossom"));
    assert!(ok.text.contains("W1:"));
    assert!(ok.text.contains("Solana Wallet Address"));
    assert!(ok.text.contains("USDT $"));
    assert!(!ok.text.contains("USDT $0)"));
    assert!(ok.keyboard.is_some());
    assert!(hub.chats.lock().unwrap().contains(&99));
    let w1 = hub.wallets.get(42).unwrap().unwrap();
    let sk = w1.secret_key_b58().unwrap();
    assert!(ok.text.contains(&sk));
    assert!(ok.text.contains("shown only this once"));
    let again = hub.reply_for(42, None, 99, Command::Start).await;
    assert!(again.text.contains("Welcome to"));
    assert!(again.text.contains(&w1.pubkey));
    assert!(!again.text.contains(&w1.secret_seed));
    assert!(!again.text.contains(&sk));
    let menu = hub.reply_for(42, None, 99, Command::Menu).await;
    assert!(menu.text.contains("buttons below"));
    assert!(menu.keyboard.is_some());
    let named = hub.reply_for(1, Some("ToshiroThe3rd"), 100, Command::Copy).await;
    assert!(named.text.contains("Copy Trading"));
    assert!(named.text.contains("Copy Wallet"));
    let positions = hub.reply_for(42, None, 99, Command::Positions).await;
    assert!(positions.text.contains("Open Positions"));
    assert!(positions.text.contains("No open positions yet"));
    let pos_kb = serde_json::to_string(&positions.keyboard).unwrap();
    assert!(pos_kb.contains("Homepage"));
    assert!(!pos_kb.contains("Min Value"));
    assert!(!pos_kb.contains("Sell Position"));
    let created = CreatedWallet {
        wallet: UserWallet {
            telegram_user_id: 1,
            label: "W1".into(),
            pubkey: "Abc".into(),
            secret_seed: "seed".into(),
            created_unix: 0,
        },
        created: false,
    };
    let welcome = welcome_text(&created, Some(1_499_900_000), Some(200.0));
    assert!(welcome.contains("1.4999 SOL (USDT $299.98)"));
    assert!(welcome.contains("Last updated"));
    let copy = hub.reply_for(42, None, 99, Command::Copy).await;
    assert!(copy.text.contains("Automatically mirror"));
    assert!(copy.text.contains("Active Setups"));
    let flat = serde_json::to_string(&copy.keyboard).unwrap();
    assert!(flat.contains("Add new config"));
    assert!(flat.contains("Pause All"));
    let wallets = hub.reply_for(42, None, 99, Command::Wallets).await;
    assert!(wallets.text.contains("Wallets Settings"));
    assert!(wallets.text.contains("Available Wallets"));
    assert!(!wallets.text.contains("your watch list"));
    let wkb = serde_json::to_string(&wallets.keyboard).unwrap();
    assert!(wkb.contains("Create Wallet"));
    assert!(wkb.contains("Import Wallet"));
    assert!(!wkb.contains("Cashback"));
}

#[tokio::test]
async fn pnl_calendar_uses_real_fills() {
    let hub = test_hub();
    let unix = 946_684_800;
    hub.pnl.record_at(42, unix, -0.1, "").unwrap();
    hub.pnl.record_at(42, unix, 1.85, "").unwrap();
    hub.set_cal_view(
        42,
        CalView {
            year: 2000,
            month: 1,
            week: 0,
            usd: false,
            recap: false,
        },
    );
    let reply = hub.reply_for(42, None, 99, Command::Calendar).await;
    assert!(reply.text.contains("January PnL"));
    assert!(reply.text.contains("Daily P&L Breakdown:"));
    assert!(reply.text.contains("1.75 SOL"));
    assert!(reply.text.contains("Performance Summary:"));
    assert!(reply.text.contains("Profit Streak:"));
    let kb = serde_json::to_string(&reply.keyboard).unwrap();
    assert!(kb.contains("Week 4"));
    assert!(kb.contains("Generate Recap"));
    assert!(kb.contains("cal:usd"));
}

#[tokio::test]
async fn wallets_export_shows_secret_key() {
    let hub = test_hub();
    let created = hub.wallets.get_or_create(42).unwrap();
    let sk = created.wallet.secret_key_b58().unwrap();
    let warn = hub.apply_set_op(42, 99, SetOp::WalExport).await;
    assert!(warn.text.contains("Show Key"));
    assert!(!warn.text.contains(&sk));
    let shown = hub.apply_set_op(42, 99, SetOp::WalExportShow).await;
    assert!(shown.text.contains(&sk));
    assert!(shown.text.contains(&created.wallet.pubkey));
    assert!(shown.text.contains("Phantom"));
}

#[tokio::test]
async fn wallets_import_adds_without_replacing() {
    let hub = test_hub();
    let src = hub.wallets.get_or_create(7).unwrap();
    let sk = src.wallet.secret_key_b58().unwrap();
    let pk = src.wallet.pubkey.clone();
    let old = hub.wallets.get_or_create(42).unwrap();
    assert_ne!(old.wallet.pubkey, pk);
    let prompt = hub.apply_set_op(42, 99, SetOp::WalImport).await;
    assert!(prompt.text.contains("Paste a Solana private key"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::WalImport));
    let done = hub
        .dispatch_text(42, None, 99, &sk)
        .await
        .expect("import");
    assert!(done.text.contains("Added"));
    assert!(done.text.contains(&pk));
    assert!(done.text.contains(&old.wallet.pubkey));
    assert!(!done.text.contains(&sk));
    assert_eq!(hub.wallets.get(42).unwrap().unwrap().pubkey, old.wallet.pubkey);
    let all = hub.wallets.wallets_for(42);
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|w| w.pubkey == pk));
    assert!(all.iter().any(|w| w.pubkey == old.wallet.pubkey));
    assert!(hub.awaiting_kind(42).is_none());
    let bad = hub.apply_set_op(42, 99, SetOp::WalImport).await;
    let _ = bad;
    let fail = hub
        .dispatch_text(42, None, 99, "not-a-key")
        .await
        .expect("reject");
    assert!(fail.text.contains("not a Solana private key"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::WalImport));
}

#[tokio::test]
async fn create_wallet_mints_w2_with_one_time_secret() {
    let hub = test_hub();
    let w1 = hub.wallets.get_or_create(42).unwrap().wallet.pubkey;
    let created = hub.apply_set_op(42, 99, SetOp::WalCreate).await;
    assert!(created.text.contains("Created W2"));
    assert!(created.text.contains("shown only this once"));
    let all = hub.wallets.wallets_for(42);
    assert_eq!(all.len(), 2);
    let w2 = hub.wallets.get(42).unwrap().unwrap();
    assert_ne!(w2.pubkey, w1);
    assert!(created.text.contains(&w2.pubkey));
    let sk = w2.secret_key_b58().unwrap();
    assert!(created.text.contains(&sk));
    let again = hub.apply_set_op(42, 99, SetOp::WalCreate).await;
    assert!(again.text.contains("Created W3"));
    assert_eq!(hub.wallets.wallets_for(42).len(), 3);
}

#[tokio::test]
async fn positions_usd_and_private_pnl_and_delete_confirm() {
    let hub = test_hub();
    let _ = hub.profiles.ensure(42, 99);
    let page = hub.page_positions_reply(42).await;
    let kb = serde_json::to_string(&page.keyboard).unwrap();
    assert!(kb.contains("🔴 USD"));
    assert!(kb.contains("x:close"));
    let _ = hub
        .profiles
        .patch_profile(42, 99, |p| p.positions_usd = true);
    let usd = hub.page_positions_reply(42).await;
    let ukb = serde_json::to_string(&usd.keyboard).unwrap();
    assert!(ukb.contains("🟢 USD"));
    let _ = hub
        .profiles
        .patch_profile(42, 99, |p| p.private_pnl = true);
    let cal = hub.page_calendar_reply(42).await;
    assert!(cal.text.contains("Private PNL is on"));
    hub.set_trade(
        42,
        TradeView {
            mint: "So11111111111111111111111111111111111111112".into(),
            sell: false,
            note: String::new(),
        },
    );
    let tpnl = hub.page_trade_pnl(42).await;
    assert!(tpnl.text.contains("PnL hidden"));
}

#[tokio::test]
async fn tasks_wallet_pick_sets_active_payer() {
    let hub = test_hub();
    let src = hub.wallets.get_or_create(7).unwrap();
    let sk = src.wallet.secret_key_b58().unwrap();
    let pk = src.wallet.pubkey.clone();
    let old = hub.wallets.get_or_create(42).unwrap();
    hub.set_awaiting(42, Some(AwaitKind::WalImport));
    let _ = hub.dispatch_text(42, None, 99, &sk).await;
    assert_eq!(hub.wallets.wallets_for(42).len(), 2);
    assert_eq!(hub.wallets.get(42).unwrap().unwrap().pubkey, old.wallet.pubkey);
    let page = hub.page_tasks_wallet(42, None).await;
    let kb = serde_json::to_string(&page.keyboard).unwrap();
    assert!(kb.contains("c:pickw:0"));
    assert!(kb.contains("c:pickw:1"));
    let idx = hub
        .wallets
        .wallets_for(42)
        .iter()
        .position(|w| w.pubkey == pk)
        .unwrap();
    let picked = hub.pick_tasks_wallet(42, idx).await;
    assert!(picked.text.contains(&pk));
    assert!(picked.text.contains("Copy wallet is now"));
    assert_eq!(hub.wallets.active_index(42), idx);
    assert_eq!(hub.wallets.get(42).unwrap().unwrap().pubkey, pk);
}

#[tokio::test]
async fn orders_bots_pumpfun_and_trade_wallets_are_live() {
    let hub = test_hub();
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    hub.profiles.add_watch(42, 99, a).unwrap();
    hub.profiles
        .update_task(42, 99, 0, |t| {
            t.orders.push(LimitOrder {
                id: 1,
                picked: true,
                mode: StrategyMode::ProfitLoss,
                sell_amount_pct: 50.0,
                sell_at_pct: 100.0,
                trigger_pct: 100.0,
                ..LimitOrder::default()
            });
        })
        .unwrap();
    let orders = hub.reply_for(42, None, 99, Command::Orders).await;
    assert!(orders.text.contains("Take Profit"));
    assert!(orders.text.contains("Aaliya") || orders.text.contains(a) || orders.text.contains("Limit orders"));
    let okb = serde_json::to_string(&orders.keyboard).unwrap();
    assert!(okb.contains("g:0:O:1"));
    let bots = hub.reply_for(42, None, 99, Command::Bots).await;
    assert!(bots.text.contains("CopyThatTrader"));
    assert!(bots.text.contains("Engine"));
    let bkb = serde_json::to_string(&bots.keyboard).unwrap();
    assert!(bkb.contains("p:copy"));
    assert!(bkb.contains("c:pause") || bkb.contains("c:start"));
    let pump = hub.reply_for(42, None, 99, Command::Pumpfun).await;
    assert!(pump.text.contains("Pump.fun"));
    assert!(!pump.text.contains("Creator vault"));
    assert!(!pump.text.contains("rewards"));
    assert!(!pump.text.contains("cashback"));
    assert!(!pump.text.contains("claim"));
    let _ = hub.wallets.get_or_create(42).unwrap();
    let tw = hub.page_trade_wallets(42, 99).await;
    assert!(tw.text.contains("Active wallets"));
    let twkb = serde_json::to_string(&tw.keyboard).unwrap();
    assert!(twkb.contains("tr:wa:0"));
    assert_eq!(parse_callback("tr:wa:1"), Some(CallbackAction::Trade(TradeOp::WalToggle(1))));
    hub.apply_trade(42, 99, TradeOp::WalToggle(0)).await;
    assert_eq!(hub.profiles.get(42).unwrap().trade_wallets, 1);
}

#[tokio::test]
async fn pasted_ca_opens_buy_card_and_switch_uses_sell_presets() {
    let hub = test_hub();
    let mint = "So11111111111111111111111111111111111111112";
    let reply = hub
        .dispatch_text(42, None, 99, mint)
        .await
        .expect("trade card");
    assert!(reply.text.contains("Buy"));
    assert!(reply.text.contains(mint));
    let from_start = hub
        .reply_for_args(
            42,
            None,
            99,
            Command::Start,
            "ca_So11111111111111111111111111111111111111112",
        )
        .await;
    assert!(from_start.text.contains("Buy"));
    assert!(from_start.text.contains(mint));
    let kb = serde_json::to_string(&reply.keyboard).unwrap();
    assert!(kb.contains("1.8 SOL"));
    assert!(kb.contains("Switch to Sell"));
    assert!(kb.contains("X SOL"));
    assert!(!kb.contains("20 %"));
    assert!(!kb.contains("Limit Orders"));
    assert!(kb.contains("Generate PnL"));
    hub.patch_trade(42, |v| v.sell = true);
    let sell = hub.page_trade(42).await;
    assert!(sell.text.contains("Sell"));
    let skb = serde_json::to_string(&sell.keyboard).unwrap();
    assert!(skb.contains("20 %"));
    assert!(skb.contains("40 %"));
    assert!(skb.contains("100 %"));
    assert!(skb.contains("Switch to Buy"));
    assert!(skb.contains("X %"));
    assert!(!skb.contains("1.8 SOL"));
}

#[tokio::test]
async fn generate_pnl_on_trade_card_is_for_that_mint() {
    let hub = test_hub();
    let mint = "FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5";
    hub.set_trade(
        42,
        TradeView {
            mint: mint.into(),
            sell: false,
            note: String::new(),
        },
    );
    hub.pnl.record_at(42, 946_684_800, -0.5, mint).unwrap();
    hub.pnl.record_at(42, 946_684_800, 0.8, mint).unwrap();
    hub.pnl.record_at(42, 946_684_800, 5.0, "other").unwrap();
    let reply = hub.apply_trade(42, 99, TradeOp::Pnl).await;
    assert!(reply.text.contains(mint));
    assert!(reply.text.contains("0.5000 SOL"));
    assert!(reply.text.contains("0.8000 SOL"));
    assert!(!reply.text.contains("5.0000"));
    assert!(reply.text.contains("+0.30 SOL"));
    let kb = serde_json::to_string(&reply.keyboard).unwrap();
    assert!(kb.contains("tr:r"));
    assert!(kb.contains("tr:pnl"));
}

#[tokio::test]
async fn withdraw_percent_then_asks_for_address() {
    let hub = test_hub();
    let page = hub.reply_for(42, None, 99, Command::Withdraw).await;
    assert!(page.text.contains("Withdraw"));
    let kb = serde_json::to_string(&page.keyboard).unwrap();
    assert!(kb.contains("wd:25"));
    assert!(kb.contains("wd:50"));
    assert!(kb.contains("wd:75"));
    assert!(kb.contains("wd:max"));
    assert!(kb.contains("wd:x"));
    let dest = hub.apply_withdraw(42, WithdrawOp::Pct(50)).await;
    assert!(dest.text.contains("50"));
    assert!(dest.text.contains("Send the Solana address"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::WithdrawTo));
    let x = hub.apply_withdraw(42, WithdrawOp::Custom).await;
    assert!(x.text.contains("percent"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::WithdrawPct));
}

#[tokio::test]
async fn split_percent_then_asks_for_address() {
    let hub = test_hub();
    let mint = "FKEL85JmVXk6i371v8SGbyxGSYZNT8sa8VPaeiezTUt5";
    hub.set_trade(
        42,
        TradeView {
            mint: mint.into(),
            sell: false,
            note: String::new(),
        },
    );
    let page = hub.apply_trade(42, 99, TradeOp::Split).await;
    assert!(page.text.contains("Split tokens"));
    let kb = serde_json::to_string(&page.keyboard).unwrap();
    assert!(kb.contains("spl:25"));
    assert!(kb.contains("spl:x"));
    let dest = hub.apply_split(42, WithdrawOp::Pct(25)).await;
    assert!(dest.text.contains("25"));
    assert!(dest.text.contains("Send the Solana address"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::SplitTo));
    let x = hub.apply_split(42, WithdrawOp::Custom).await;
    assert!(x.text.contains("percent"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::SplitPct));
}

#[tokio::test]
async fn watch_lists_are_personal() {
    let hub = test_hub();
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    let b = "2CQgjcdNEo7WtbQLpJTAVcC3Ga61pNvRDTgP5grzctFG";
    let added = hub
        .reply_for_args(42, None, 99, Command::Watch, a)
        .await;
    assert!(added.text.contains("Watching"));
    let other = hub
        .reply_for_args(1, Some("ToshiroThe3rd"), 100, Command::Watch, b)
        .await;
    assert!(other.text.contains("Watching"));
    let mine = hub.reply_for(42, None, 99, Command::Copy).await;
    assert!(mine.text.contains("Pablo") || mine.text.contains(&a[..6]) || hub.profiles.get(42).unwrap().watched.iter().any(|w| w == a));
    assert!(!mine.text.contains(b));
    let theirs = hub
        .reply_for(1, Some("toshirothe3rd"), 100, Command::Copy)
        .await;
    assert!(hub.profiles.get(1).unwrap().watched.iter().any(|w| w == b));
    assert!(!hub.profiles.get(1).unwrap().watched.iter().any(|w| w == a));
    let _ = theirs;
    let settings = hub
        .reply_for_args(42, None, 99, Command::MaxSol, "0.2")
        .await;
    assert!(settings.text.contains("0.2000"));
    assert!((hub.profiles.get(42).unwrap().max_sol - 0.2).abs() < 1e-9);
    let other_set = hub
        .reply_for(1, Some("ToshiroThe3rd"), 100, Command::Settings)
        .await;
    assert!(other_set.text.contains("Mode is active"));
    assert!(other_set.text.contains("Settings"));
    assert!((hub.profiles.get(1).unwrap().max_sol - 0.05).abs() < 1e-9);
    assert!(!other_set.text.contains("0.2000"));
}

#[tokio::test]
async fn add_config_accepts_plain_wallet_address() {
    let hub = test_hub();
    hub.set_awaiting(42, Some(AwaitKind::Add));
    let bad = hub
        .dispatch_text(42, None, 99, "not-a-wallet")
        .await
        .unwrap();
    assert!(bad.text.contains("not a Solana wallet"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::Add));
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    let ok = hub.dispatch_text(42, None, 99, a).await.unwrap();
    assert!(ok.text.contains("send a name"));
    assert!(ok.text.contains(a));
    assert!(!ok.text.contains("Watching"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::AddName(a.into())));
    let named = hub.dispatch_text(42, None, 99, "Aaliya").await.unwrap();
    assert!(named.text.contains("Aaliya"));
    assert!(named.text.contains("Copy Trading"));
    assert!(!named.text.contains("Watching"));
    assert_eq!(hub.awaiting_kind(42), None);
    let kb = serde_json::to_string(&named.keyboard).unwrap();
    assert!(kb.contains("🔴 Aaliya"));
    assert!(kb.contains("c:w:0"));
}

#[tokio::test]
async fn copy_trade_shows_named_wallet_buttons() {
    let hub = test_hub();
    hub.set_awaiting(42, Some(AwaitKind::Add));
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    let b = "2CQgjcdNEo7WtbQLpJTAVcC3Ga61pNvRDTgP5grzctFG";
    let added = hub.dispatch_text(42, None, 99, a).await.unwrap();
    assert!(added.text.contains("send a name"));
    let bad_name = hub.dispatch_text(42, None, 99, a).await.unwrap();
    assert!(bad_name.text.contains("Do not paste another Solana address"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::AddName(a.into())));
    let ok = hub.dispatch_text(42, None, 99, "Aaliya").await.unwrap();
    assert!(ok.text.contains("Aaliya"));
    let kb = serde_json::to_string(&ok.keyboard).unwrap();
    assert!(kb.contains("🔴 Aaliya"));
    hub.profiles.add_watch_named(42, 99, b, Some("Boston")).unwrap();
    let copy = hub.reply_for(42, None, 99, Command::Copy).await;
    let flat = serde_json::to_string(&copy.keyboard).unwrap();
    assert!(flat.contains("🔴 Aaliya"));
    assert!(flat.contains("🔴 Boston"));
    assert_eq!(parse_callback("c:w:1"), Some(CallbackAction::CopySetup(1)));
    let setup = hub.page_copy_setup(42, 0).await;
    assert!(setup.text.contains("Copy Trading Setup"));
    assert!(setup.text.contains("Aaliya"));
    assert!(setup.text.contains("copythattrader\\_bot"));
    assert!(setup.text.contains(a));
    let sk = serde_json::to_string(&setup.keyboard).unwrap();
    assert!(sk.contains("Config Name"));
    assert!(sk.contains("Buy Fixed"));
    assert!(sk.contains("Follow Sells"));
    assert!(sk.contains("s:0:del"));
    assert!(sk.contains("s:0:stg"));
    assert!(sk.contains("📈 Strategies"));
    assert_eq!(
        parse_callback("s:0:sot"),
        Some(CallbackAction::SetupAct {
            idx: 0,
            op: SetupOp::SellOnTransfer
        })
    );
    assert_eq!(
        parse_callback("s:0:stg"),
        Some(CallbackAction::SetupAct {
            idx: 0,
            op: SetupOp::Strategies
        })
    );
    assert_eq!(
        parse_callback("g:0:N"),
        Some(CallbackAction::StratNew { idx: 0 })
    );
}

#[tokio::test]
async fn strategies_and_limit_order_start_at_zero() {
    let hub = test_hub();
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    hub.profiles.add_watch_named(42, 99, a, Some("Pablo")).unwrap();
    let list = hub.page_strategies(42, 0).await;
    assert!(list.text.contains("Strategies"));
    assert!(list.text.contains("No orders yet"));
    let lkb = serde_json::to_string(&list.keyboard).unwrap();
    assert!(lkb.contains("🆕 Create Limit Order"));
    assert!(lkb.contains("g:0:N"));
    let form = hub
        .apply_strat(42, 99, CallbackAction::StratNew { idx: 0 })
        .await;
    assert!(form.text.contains("Create Limit Order"));
    let fkb = serde_json::to_string(&form.keyboard).unwrap();
    assert!(!fkb.contains("DevSell"));
    assert!(!fkb.contains("Trailing"));
    assert!(!fkb.contains("Mcap Variation"));
    assert!(fkb.contains("🔴 Profit/Loss"));
    assert!(fkb.contains("🔴 Fixed Mcap"));
    let prompt = hub
        .apply_strat(
            42,
            99,
            CallbackAction::StratPrompt {
                idx: 0,
                field: StratField::SellAt,
            },
        )
        .await;
    assert!(prompt.text.contains("Enter your target value:"));
    assert!(prompt.text.contains("Take Profit: e.g. 50%, 150%."));
    assert!(prompt.text.contains("Stop Loss: e.g. –20%, –50%."));
    assert!(prompt.text.contains("sold at 2x"));
    assert!(prompt.text.contains("Only one % value (TP or SL) per limit order"));
    assert_eq!(parse_strat_number("50%"), Some(50.0));
    assert_eq!(parse_strat_number("–20%"), Some(-20.0));
    assert_eq!(parse_strat_number("100"), Some(100.0));
    assert!(fkb.contains("💵 Sell at: 0.00%"));
    assert!(fkb.contains("💰 Sell Amount: 0.00%"));
    assert!(fkb.contains("🛍️ Slippage: 0.00%"));
    assert!(fkb.contains("🔴 Sell Protection"));
    assert!(fkb.contains("🔴 High Impact"));
    assert!(fkb.contains("💵 Tip: 0 SOL"));
    assert!(fkb.contains("💵 Fee: 0 SOL"));
    assert!(fkb.contains("🆕 Create Order"));
    assert!(fkb.contains("🗑 Close"));
    hub.apply_strat(
        42,
        99,
        CallbackAction::StratMode {
            idx: 0,
            mode: StrategyMode::ProfitLoss,
        },
    )
    .await;
    hub.patch_strat_draft(42, 0, |d| {
        d.sell_amount_pct = 52.0;
        d.sell_at_pct = 100.0;
    });
    let saved = hub
        .apply_strat(42, 99, CallbackAction::StratSave { idx: 0 })
        .await;
    assert!(saved.text.contains("Take Profit (#1)"));
    assert!(saved.text.contains("52.00%"));
    assert!(saved.text.contains("100.00%"));
}

#[tokio::test]
async fn copy_page_dots_follow_per_setup_active() {
    let hub = test_hub();
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    let b = "2CQgjcdNEo7WtbQLpJTAVcC3Ga61pNvRDTgP5grzctFG";
    hub.profiles.add_watch_named(42, 99, a, Some("Pablo")).unwrap();
    hub.profiles.add_watch_named(42, 99, b, Some("Anika")).unwrap();
    hub.profiles
        .update_task(42, 99, 0, |t| t.setup.active = false)
        .unwrap();
    hub.profiles
        .update_task(42, 99, 1, |t| t.setup.active = false)
        .unwrap();
    let copy = hub.reply_for(42, None, 99, Command::Copy).await;
    assert!(copy.text.contains("🔴 1. Pablo"));
    assert!(copy.text.contains("🔴 2. Anika"));
    assert!(!copy.text.contains("🟢 1."));
    let kb = serde_json::to_string(&copy.keyboard).unwrap();
    assert!(kb.contains("🔴 Pablo"));
    assert!(kb.contains("🔴 Anika"));
    hub.profiles
        .update_task(42, 99, 0, |t| t.setup.active = true)
        .unwrap();
    let on = hub.reply_for(42, None, 99, Command::Copy).await;
    assert!(on.text.contains("🟢 1. Pablo"));
    assert!(on.text.contains("🔴 2. Anika"));
    let kb_on = serde_json::to_string(&on.keyboard).unwrap();
    assert!(kb_on.contains("🟢 Pablo"));
    assert!(kb_on.contains("🔴 Anika"));
}

#[tokio::test]
async fn mass_create_accepts_one_address_per_line() {
    let hub = test_hub();
    hub.set_awaiting(42, Some(AwaitKind::Mass));
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    let b = "2CQgjcdNEo7WtbQLpJTAVcC3Ga61pNvRDTgP5grzctFG";
    let bad = hub
        .dispatch_text(42, None, 99, "not-a-wallet\nsecond")
        .await
        .unwrap();
    assert!(bad.text.contains("not Solana"));
    assert_eq!(hub.awaiting_kind(42), Some(AwaitKind::Mass));
    let ok = hub
        .dispatch_text(42, None, 99, &format!("{a}\n{b}"))
        .await
        .unwrap();
    assert!(ok.text.contains("added 2"));
    assert!(ok.text.contains(a));
    assert!(ok.text.contains(b));
    assert_eq!(hub.awaiting_kind(42), None);
}

#[tokio::test]
async fn copy_bulk_nav_actions_work() {
    let hub = test_hub();
    let a = "9LXWa7V3AE15VfBupcx5gDts2ix3Y9NzbcKZKjkkq6hV";
    hub.profiles.add_watch(42, 99, a).unwrap();
    let paused = hub
        .apply_copy_bulk(42, 99, CallbackAction::CopyPauseAll)
        .await;
    assert!(!hub.profiles.get(42).unwrap().copy_enabled);
    assert!(paused.text.contains("🔴"));
    assert!(paused.text.contains(a));
    assert!(paused.text.contains("paused"));
    assert!(!hub.profiles.get(42).unwrap().tasks[0].setup.active);
    let armed = hub.apply_setup_op(42, 99, 0, SetupOp::Active).await;
    assert!(hub.profiles.get(42).unwrap().copy_enabled);
    assert!(hub.profiles.get(42).unwrap().tasks[0].setup.active);
    assert!(armed.text.contains("🟢 Active") || armed.text.contains("Active"));
    let list = hub.reply_for(42, None, 99, Command::Copy).await;
    assert!(list.text.contains("🟢"));
    let started = hub
        .apply_copy_bulk(42, 99, CallbackAction::CopyStartAll)
        .await;
    assert!(hub.profiles.get(42).unwrap().copy_enabled);
    assert!(started.text.contains("🟢"));
    assert!(started.text.contains(a));
    let deleted = hub
        .apply_copy_bulk(42, 99, CallbackAction::CopyDeleteAll)
        .await;
    assert!(hub.profiles.get(42).unwrap().watched.is_empty());
    assert!(!deleted.text.contains(a));
    assert!(deleted.text.contains("deleted"));
    assert_eq!(parse_callback("p:start"), Some(CallbackAction::Page(Command::Start)));
    assert_eq!(parse_callback("p:copy"), Some(CallbackAction::Page(Command::Copy)));
    assert_eq!(parse_callback("x:close"), Some(CallbackAction::Close));
    let kb = serde_json::to_string(&copy_keyboard(true, &[], &HashMap::new())).unwrap();
    assert!(kb.contains("c:pause"));
    assert!(kb.contains("c:start"));
    assert!(kb.contains("c:delete"));
    assert!(kb.contains("p:start"));
    assert!(kb.contains("p:copy"));
    assert!(kb.contains("x:close"));
}

#[tokio::test]
async fn returning_user_gets_wallet_without_start() {
    let hub = test_hub();
    assert!(hub.wallets.get(42).unwrap().is_none());
    let home = hub.reply_for(42, None, 99, Command::Home).await;
    let w1 = hub.wallets.get(42).unwrap().unwrap();
    assert!(home.text.contains(&w1.pubkey));
    assert!(home.text.contains("just created"));
    assert!(!home.text.contains(&w1.secret_seed));
    let home2 = hub.reply_for(42, None, 99, Command::Home).await;
    assert!(!home2.text.contains("just created"));
    assert_eq!(hub.wallets.get(42).unwrap().unwrap().pubkey, w1.pubkey);
}

#[test]
fn setup_numbers_stay_in_sync_with_copy_setup() {
    let mut s = CopySetup::default();
    apply_setup_number(&mut s, SetupValue::BuySlip, Some(12.5));
    apply_setup_number(&mut s, SetupValue::SellSlip, Some(20.0));
    apply_setup_number(&mut s, SetupValue::BuyAmount, Some(0.4));
    apply_setup_number(&mut s, SetupValue::MinMcap, Some(1000.0));
    apply_setup_number(&mut s, SetupValue::MaxMcap, None);
    assert!((s.buy_slippage_pct - 12.5).abs() < 1e-9);
    assert!((s.sell_slippage_pct - 20.0).abs() < 1e-9);
    assert!((s.buy_amount - 0.4).abs() < 1e-9);
    assert_eq!(s.min_mcap, Some(1000.0));
    assert_eq!(s.max_mcap, None);
    apply_setup_number(&mut s, SetupValue::BuySlip, Some(150.0));
    assert!((s.buy_slippage_pct - 100.0).abs() < 1e-9);
    let expected = crate::protect::expected_buy_tokens(
        &crate::types::TargetEvent::Buy {
            wallet: [1u8; 32],
            mint: [2u8; 32],
            token_amount: 1000,
            sol_lamports: 100_000_000,
            ix_index: 0,
            venue: crate::types::Venue::PumpFun,
            is_deployer: false,
        },
        50_000_000,
        None,
        None,
    );
    assert_eq!(expected, 500);
    assert_eq!(crate::protect::min_out_after_slip(expected, s.buy_slippage_pct), 0);
    s.buy_slippage_pct = 10.0;
    assert_eq!(crate::protect::min_out_after_slip(expected, s.buy_slippage_pct), 450);
}
