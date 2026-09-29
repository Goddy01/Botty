//! Telegram bot: commands, inline UI, and Hub poll loop.

mod hub;
mod parse;
mod status;
mod ui;

#[cfg(test)]
mod tests;

pub use hub::Hub;
pub use parse::{
    command_args, extract_mint, parse_callback, parse_command, CallbackAction, CalOp, Command,
    QuotePick, Reply, SetOp, SetupOp, StratField, TradeOp, WithdrawOp,
};
pub use status::StatusCard;
pub use ui::{
    add_config_name_prompt, add_config_name_retry, add_config_prompt, chart_url, copy_keyboard,
    copy_setup_keyboard, copy_setup_text, help_text, home_keyboard, inline_grid, mass_create_prompt,
    parse_watch_input, positions_keyboard, settings_keyboard, simple_keyboard, split_keyboard,
    start_ca_arg, tasks_wallet_keyboard, tasks_wallet_success, tasks_wallet_text, wallets_keyboard,
    withdraw_keyboard, BotSettings,
};

const API: &str = "https://api.telegram.org";
const BOT_NAME: &str = "CopyThatTrader";
const BOT_USERNAME: &str = "copythattrader_bot";

/// Same Menu layout as Bloom: slash + one-line description in Telegram's Menu button.
pub const MENU: &[(&str, &str)] = &[
    ("menu", "Open the menu"),
    ("start", "Start trading with CopyThatTrader"),
    ("home", "Access the homepage"),
    ("calendar", "Access your calendar"),
    ("positions", "Access your positions"),
    ("copy", "Access your copy trading"),
    ("watch", "Add a wallet you want to copy"),
    ("unwatch", "Remove a wallet from your list"),
    ("orders", "Access your limit orders"),
    ("settings", "Access your settings"),
    ("wallets", "Access your wallets"),
    ("withdraw", "Withdraw funds from your wallet"),
    ("bots", "Access bots page"),
    ("pumpfun", "Pump.fun copy activity"),
];
