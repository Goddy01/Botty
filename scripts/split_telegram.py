"""Split telegram/all.rs into status / parse / ui / hub / tests modules."""
from pathlib import Path
import re
import textwrap

root = Path(__file__).resolve().parents[1] / "src" / "telegram"
lines = (root / "all.rs").read_text(encoding="utf-8").splitlines(keepends=True)


def chunk(start: int, end: int) -> str:
    """1-based inclusive start, exclusive end."""
    return "".join(lines[start - 1 : end - 1])


def make_pub_crate(src: str) -> str:
    src = re.sub(r"^fn ", "pub(crate) fn ", src, flags=re.M)
    src = re.sub(r"^enum ", "pub(crate) enum ", src, flags=re.M)
    src = re.sub(r"^struct ", "pub(crate) struct ", src, flags=re.M)
    src = re.sub(r"^pub\(crate\) pub fn ", "pub fn ", src, flags=re.M)
    src = re.sub(r"^pub\(crate\) pub struct ", "pub struct ", src, flags=re.M)
    src = re.sub(r"^pub\(crate\) pub enum ", "pub enum ", src, flags=re.M)
    return src


STATUS_IMPORTS = """use crate::config::{encode_pubkey, AppConfig, WatchKind};
use crate::rpc_read::is_placeholder_payer;
"""

PARSE_IMPORTS = """use crate::profile::{BuyMode, QuoteCurrency, StrategyMode};
"""

UI_IMPORTS = """use super::parse::{
    CalOp, Command, QuotePick, SetOp, SetupOp, SetupValue, StratField, TradeOp,
};
use crate::live::{LiveSnap, MainnetEvent};
use crate::pnl::CalView;
use crate::profile::{
    short_wallet_label, BuyMode, CopySetup, CopyTask, LimitOrder, QuoteCurrency, StrategyMode,
};
use crate::rpc_read::fmt_sol;
use crate::user_wallet::CreatedWallet;
use crate::parse::parse_duration_secs;
use super::{BOT_NAME, BOT_USERNAME, MENU};
"""

HUB_IMPORTS = """use super::parse::*;
use super::status::StatusCard;
use super::ui::*;
use super::{API, BOT_NAME, BOT_USERNAME, MENU};
use crate::config::{encode_pubkey, AppConfig, WatchKind};
use crate::engine::CopyEngine;
use crate::live::LiveBoard;
use crate::positions::PositionStore;
use crate::pnl::{self, CalView, PnlStore};
use crate::profile::{
    short_wallet_label, BuyMode, CopySetup, CopyTask, LimitOrder, ProfileDefaults, QuoteCurrency,
    StrategyMode, WatchBook, MAX_WATCHED,
};
use crate::rpc_read::{fmt_sol, is_placeholder_payer, RpcRead, WSOL_MINT};
use crate::user_wallet::{CreatedWallet, UserWallet, WalletStore};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
"""

# Include attribute lines sitting above the type definitions.
status_body = chunk(42, 136)
parse_body = make_pub_crate(chunk(137, 709))
ui_body = make_pub_crate(chunk(709, 1882))
hub_body = make_pub_crate(chunk(1882, 5302))

raw_tests = chunk(5302, len(lines) + 1)
raw_tests = re.sub(r"^#\[cfg\(test\)\]\s*", "", raw_tests)
raw_tests = re.sub(r"^mod tests \{\s*", "", raw_tests)
raw_tests = raw_tests.rstrip()
if raw_tests.endswith("}"):
    raw_tests = raw_tests[:-1]
raw_tests = textwrap.dedent(raw_tests).strip() + "\n"

consts = chunk(20, 41)

mod_rs = '''//! Telegram bot: commands, inline UI, and Hub poll loop.

mod hub;
mod parse;
mod status;
mod ui;

#[cfg(test)]
mod tests;

pub use hub::Hub;
pub use parse::{
    command_args, parse_callback, parse_command, CallbackAction, CalOp, Command, QuotePick, Reply,
    SetOp, SetupOp, StratField, TradeOp, WithdrawOp,
};
pub use status::StatusCard;
pub use ui::{
    add_config_name_prompt, add_config_name_retry, add_config_prompt, chart_url, copy_keyboard,
    copy_setup_keyboard, copy_setup_text, extract_mint, help_text, home_keyboard, inline_grid,
    mass_create_prompt, parse_watch_input, positions_keyboard, settings_keyboard, simple_keyboard,
    split_keyboard, start_ca_arg, tasks_wallet_keyboard, tasks_wallet_success, tasks_wallet_text,
    wallets_keyboard, withdraw_keyboard, BotSettings,
};

''' + consts

(root / "status.rs").write_text(STATUS_IMPORTS + "\n" + status_body, encoding="utf-8")
(root / "parse.rs").write_text(PARSE_IMPORTS + "\n" + parse_body, encoding="utf-8")
(root / "ui.rs").write_text(UI_IMPORTS + "\n" + ui_body, encoding="utf-8")
(root / "hub.rs").write_text(HUB_IMPORTS + "\n" + hub_body, encoding="utf-8")
(root / "tests.rs").write_text("use super::*;\n\n" + raw_tests, encoding="utf-8")
(root / "mod.rs").write_text(mod_rs, encoding="utf-8")
print("rewrote modules")
for name in ["status.rs", "parse.rs", "ui.rs", "hub.rs", "tests.rs"]:
    head = (root / name).read_text(encoding="utf-8").splitlines()[:8]
    print("---", name)
    print("\n".join(head))
