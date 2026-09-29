use crate::profile::StrategyMode;

pub fn extract_mint(text: &str) -> Option<String> {
    let t = text
        .trim()
        .trim_matches(|c: char| c.is_whitespace() || ".,;!?<>\"'`".contains(c) || c == '\u{200b}');
    if crate::config::parse_pubkey(t).is_ok() {
        return Some(t.to_string());
    }
    for part in t.split(|c: char| c.is_whitespace() || "/?#=&".contains(c)) {
        let p = part
            .trim()
            .trim_matches(|c: char| ".,;!?<>\"'`".contains(c) || c == '\u{200b}');
        if p.len() >= 32 && crate::config::parse_pubkey(p).is_ok() {
            return Some(p.to_string());
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Menu,
    Start,
    Home,
    Calendar,
    Positions,
    Copy,
    Watch,
    Unwatch,
    MaxSol,
    CopyOn,
    CopyOff,
    Orders,
    Settings,
    Wallets,
    Withdraw,
    Bots,
    Pumpfun,
    Stop,
    Help,
}

pub fn parse_command(text: &str) -> Option<Command> {
    let token = text.split_whitespace().next()?;
    let name = token.split('@').next()?;
    Some(match name {
        "/menu" => Command::Menu,
        "/start" => Command::Start,
        "/home" => Command::Home,
        "/calendar" => Command::Calendar,
        "/positions" => Command::Positions,
        "/copy" => Command::Copy,
        "/watch" => Command::Watch,
        "/unwatch" => Command::Unwatch,
        "/maxsol" => Command::MaxSol,
        "/copyon" => Command::CopyOn,
        "/copyoff" => Command::CopyOff,
        "/orders" => Command::Orders,
        "/settings" => Command::Settings,
        "/wallets" => Command::Wallets,
        "/withdraw" => Command::Withdraw,
        "/bots" => Command::Bots,
        "/pumpfun" => Command::Pumpfun,
        "/stop" => Command::Stop,
        "/help" => Command::Help,
        _ => return None,
    })
}

pub fn command_args(text: &str) -> String {
    text.split_whitespace().skip(1).collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub text: String,
    pub keyboard: Option<serde_json::Value>,
}

impl Reply {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            keyboard: None,
        }
    }

    pub fn page(s: impl Into<String>, keyboard: serde_json::Value) -> Self {
        Self {
            text: s.into(),
            keyboard: Some(keyboard),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AwaitKind {
    Add,
    AddName(String),
    Mass,
    SetupName(usize),
    SetupTarget(usize),
    SetupValue(usize, SetupValue),
    DupTarget(usize),
    DupName(usize, String),
    StratVal(usize, StratField),
    BuyPreset(usize),
    SellPreset(usize),
    TradeBuy,
    TradeSell,
    TradeTip,
    TradeSlip,
    WalImport,
    WithdrawPct,
    WithdrawTo,
    SplitPct,
    SplitTo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetupValue {
    MinTrigger,
    MaxTrigger,
    MinMcap,
    MaxMcap,
    MinAge,
    MaxAge,
    BuyAmount,
    BuyFee,
    SellFee,
    BuySlip,
    SellSlip,
    BuyTip,
    SellTip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StratField {
    SellAt,
    Amount,
    Slip,
    Tip,
    Fee,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOp {
    Wallet,
    Name,
    Target,
    SellOnTransfer,
    MinTrigger,
    MaxTrigger,
    MinMcap,
    MaxMcap,
    MinAge,
    MaxAge,
    SkipDeploys,
    FollowSells,
    BuyOnce,
    BuyExact,
    BuyFixed,
    BuyAmount,
    BuyFee,
    SellFee,
    BuySlip,
    SellSlip,
    BuyProtect,
    SellProtect,
    AutoTip,
    BuyTip,
    SellTip,
    Duplicate,
    Strategies,
    Delete,
    Active,
    Refresh,
}

impl SetupOp {
    pub(crate) fn code(self) -> &'static str {
        match self {
            SetupOp::Wallet => "wal",
            SetupOp::Name => "nam",
            SetupOp::Target => "tgt",
            SetupOp::SellOnTransfer => "sot",
            SetupOp::MinTrigger => "mit",
            SetupOp::MaxTrigger => "mat",
            SetupOp::MinMcap => "mim",
            SetupOp::MaxMcap => "mam",
            SetupOp::MinAge => "mia",
            SetupOp::MaxAge => "maa",
            SetupOp::SkipDeploys => "skd",
            SetupOp::FollowSells => "fls",
            SetupOp::BuyOnce => "bon",
            SetupOp::BuyExact => "bxe",
            SetupOp::BuyFixed => "bfx",
            SetupOp::BuyAmount => "bam",
            SetupOp::BuyFee => "bfe",
            SetupOp::SellFee => "sfe",
            SetupOp::BuySlip => "bsl",
            SetupOp::SellSlip => "ssl",
            SetupOp::BuyProtect => "bpr",
            SetupOp::SellProtect => "spr",
            SetupOp::AutoTip => "atp",
            SetupOp::BuyTip => "btp",
            SetupOp::SellTip => "stp",
            SetupOp::Duplicate => "dup",
            SetupOp::Strategies => "stg",
            SetupOp::Delete => "del",
            SetupOp::Active => "act",
            SetupOp::Refresh => "rfr",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "wal" => SetupOp::Wallet,
            "nam" => SetupOp::Name,
            "tgt" => SetupOp::Target,
            "sot" => SetupOp::SellOnTransfer,
            "mit" => SetupOp::MinTrigger,
            "mat" => SetupOp::MaxTrigger,
            "mim" => SetupOp::MinMcap,
            "mam" => SetupOp::MaxMcap,
            "mia" => SetupOp::MinAge,
            "maa" => SetupOp::MaxAge,
            "skd" => SetupOp::SkipDeploys,
            "fls" => SetupOp::FollowSells,
            "bon" => SetupOp::BuyOnce,
            "bxe" => SetupOp::BuyExact,
            "bfx" => SetupOp::BuyFixed,
            "bam" => SetupOp::BuyAmount,
            "bfe" => SetupOp::BuyFee,
            "sfe" => SetupOp::SellFee,
            "bsl" => SetupOp::BuySlip,
            "ssl" => SetupOp::SellSlip,
            "bpr" => SetupOp::BuyProtect,
            "spr" => SetupOp::SellProtect,
            "atp" => SetupOp::AutoTip,
            "btp" => SetupOp::BuyTip,
            "stp" => SetupOp::SellTip,
            "dup" => SetupOp::Duplicate,
            "stg" => SetupOp::Strategies,
            "del" => SetupOp::Delete,
            "act" => SetupOp::Active,
            "rfr" => SetupOp::Refresh,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackAction {
    Page(Command),
    ToggleCopy,
    CopyAdd,
    CopyMass,
    CopyUpdateWallet,
    CopyPickWallet(usize),
    CopyPauseAll,
    CopyStartAll,
    CopyDeleteAll,
    CopySetup(usize),
    SetupAct { idx: usize, op: SetupOp },
    StratList { idx: usize },
    StratNew { idx: usize },
    StratOpen { idx: usize, oid: u32 },
    StratDel { idx: usize, oid: u32 },
    StratSave { idx: usize },
    StratMode { idx: usize, mode: StrategyMode },
    StratToggleProt { idx: usize },
    StratToggleHi { idx: usize },
    StratPrompt { idx: usize, field: StratField },
    QuoteUnit {
        idx: usize,
        min: bool,
        unit: QuotePick,
    },
    Close,
    PosUsd,
    OpenTrade(String),
    SetOp(SetOp),
    Cal(CalOp),
    Trade(TradeOp),
    Withdraw(WithdrawOp),
    Split(WithdrawOp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawOp {
    Pct(u8),
    Max,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeOp {
    Switch,
    Refresh,
    Wallets,
    WalToggle(usize),
    Buy(usize),
    BuyX,
    Sell(usize),
    SellX,
    Tip,
    Slip,
    Limit,
    Pnl,
    AutoTip,
    BuyProt,
    Split,
    Currency,
    Hide,
}

impl TradeOp {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sw" => TradeOp::Switch,
            "r" => TradeOp::Refresh,
            "w" => TradeOp::Wallets,
            x if x.starts_with("wa:") => {
                let i: usize = x[3..].parse().ok()?;
                if i > 7 {
                    return None;
                }
                TradeOp::WalToggle(i)
            }
            "tx" => TradeOp::BuyX,
            "sx" => TradeOp::SellX,
            "tip" => TradeOp::Tip,
            "slip" => TradeOp::Slip,
            "lo" => TradeOp::Limit,
            "pnl" => TradeOp::Pnl,
            "at" => TradeOp::AutoTip,
            "bp" => TradeOp::BuyProt,
            "sp" => TradeOp::Split,
            "cu" => TradeOp::Currency,
            "hide" => TradeOp::Hide,
            x if x.starts_with("b:") => {
                let i: usize = x[2..].parse().ok()?;
                if i > 4 {
                    return None;
                }
                TradeOp::Buy(i)
            }
            x if x.starts_with("s:") => {
                let i: usize = x[2..].parse().ok()?;
                if i > 4 {
                    return None;
                }
                TradeOp::Sell(i)
            }
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalOp {
    Prev,
    Next,
    Week(u8),
    Usd,
    Recap,
    Refresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOp {
    Slip,
    Presets,
    Currency,
    Degen,
    Expert,
    BuyProt,
    PrivatePnl,
    Flags,
    WalCreate,
    WalImport,
    WalPick,
    WalSelect(usize),
    WalExport,
    WalExportShow,
    BuyPresets,
    SellPresets,
    SpotPresets,
    CopyPresets,
    EditBuyAmt(usize),
    EditSellAmt(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotePick {
    Sol,
    Usdt,
    Clear,
}

pub fn parse_callback(data: &str) -> Option<CallbackAction> {
    if let Some(rest) = data.strip_prefix("g:") {
        let mut parts = rest.split(':');
        let idx: usize = parts.next()?.parse().ok()?;
        let op = parts.next()?;
        return Some(match op {
            "L" | "R" => CallbackAction::StratList { idx },
            "N" => CallbackAction::StratNew { idx },
            "E" => CallbackAction::StratOpen { idx, oid: 0 },
            "C" => CallbackAction::StratSave { idx },
            "O" => {
                let oid: u32 = parts.next()?.parse().ok()?;
                CallbackAction::StratOpen { idx, oid }
            }
            "X" => {
                let oid: u32 = parts.next()?.parse().ok()?;
                CallbackAction::StratDel { idx, oid }
            }
            "M" => {
                let mode = StrategyMode::parse(parts.next()?)?;
                CallbackAction::StratMode { idx, mode }
            }
            "T" => match parts.next()? {
                "p" => CallbackAction::StratToggleProt { idx },
                "h" => CallbackAction::StratToggleHi { idx },
                _ => return None,
            },
            "V" => {
                let field = match parts.next()? {
                    "sa" => StratField::SellAt,
                    "am" => StratField::Amount,
                    "sl" => StratField::Slip,
                    "ti" => StratField::Tip,
                    "fe" => StratField::Fee,
                    _ => return None,
                };
                CallbackAction::StratPrompt { idx, field }
            }
            _ => return None,
        });
    }
    if let Some(rest) = data.strip_prefix("c:w:") {
        let i: usize = rest.parse().ok()?;
        return Some(CallbackAction::CopySetup(i));
    }
    if let Some(rest) = data.strip_prefix("c:pickw:") {
        let i: usize = rest.parse().ok()?;
        if i > 7 {
            return None;
        }
        return Some(CallbackAction::CopyPickWallet(i));
    }
    if let Some(rest) = data.strip_prefix("q:") {
        let mut parts = rest.split(':');
        let idx: usize = parts.next()?.parse().ok()?;
        let which = parts.next()?;
        let unit = match parts.next()? {
            "sol" => QuotePick::Sol,
            "usd" => QuotePick::Usdt,
            "na" => QuotePick::Clear,
            _ => return None,
        };
        let min = match which {
            "mit" => true,
            "mat" => false,
            _ => return None,
        };
        return Some(CallbackAction::QuoteUnit { idx, min, unit });
    }
    if let Some(rest) = data.strip_prefix("s:") {
        let (i, op) = rest.split_once(':')?;
        let idx: usize = i.parse().ok()?;
        let op = SetupOp::parse(op)?;
        return Some(CallbackAction::SetupAct { idx, op });
    }
    if let Some(rest) = data.strip_prefix("ba:") {
        let i: usize = rest.parse().ok()?;
        if i > 4 {
            return None;
        }
        return Some(CallbackAction::SetOp(SetOp::EditBuyAmt(i)));
    }
    if let Some(rest) = data.strip_prefix("sp:") {
        let i: usize = rest.parse().ok()?;
        if i > 4 {
            return None;
        }
        return Some(CallbackAction::SetOp(SetOp::EditSellAmt(i)));
    }
    if let Some(rest) = data.strip_prefix("w:sel:") {
        let i: usize = rest.parse().ok()?;
        if i > 7 {
            return None;
        }
        return Some(CallbackAction::SetOp(SetOp::WalSelect(i)));
    }
    if let Some(rest) = data.strip_prefix("tr:") {
        return Some(CallbackAction::Trade(TradeOp::parse(rest)?));
    }
    if let Some(rest) = data.strip_prefix("cal:") {
        return Some(match rest {
            "p" => CallbackAction::Cal(CalOp::Prev),
            "n" => CallbackAction::Cal(CalOp::Next),
            "usd" => CallbackAction::Cal(CalOp::Usd),
            "recap" => CallbackAction::Cal(CalOp::Recap),
            "r" => CallbackAction::Cal(CalOp::Refresh),
            w if w.starts_with("w:") => {
                let i: u8 = w[2..].parse().ok()?;
                if i > 3 {
                    return None;
                }
                CallbackAction::Cal(CalOp::Week(i))
            }
            _ => return None,
        });
    }
    if let Some(rest) = data.strip_prefix("spl:") {
        return Some(CallbackAction::Split(match rest {
            "25" => WithdrawOp::Pct(25),
            "50" => WithdrawOp::Pct(50),
            "75" => WithdrawOp::Pct(75),
            "max" => WithdrawOp::Max,
            "x" => WithdrawOp::Custom,
            _ => return None,
        }));
    }
    if let Some(rest) = data.strip_prefix("wd:") {
        return Some(CallbackAction::Withdraw(match rest {
            "25" => WithdrawOp::Pct(25),
            "50" => WithdrawOp::Pct(50),
            "75" => WithdrawOp::Pct(75),
            "max" => WithdrawOp::Max,
            "x" => WithdrawOp::Custom,
            _ => return None,
        }));
    }
    if let Some(mint) = data.strip_prefix("pos:ca:") {
        if extract_mint(mint).is_some() {
            return Some(CallbackAction::OpenTrade(mint.to_string()));
        }
        return None;
    }
    Some(match data {
        "p:menu" => CallbackAction::Page(Command::Menu),
        "p:home" | "p:back" => CallbackAction::Page(Command::Home),
        "p:calendar" => CallbackAction::Page(Command::Calendar),
        "p:positions" => CallbackAction::Page(Command::Positions),
        "p:copy" => CallbackAction::Page(Command::Copy),
        "p:watch" => CallbackAction::Page(Command::Watch),
        "p:unwatch" => CallbackAction::Page(Command::Unwatch),
        "p:orders" => CallbackAction::Page(Command::Orders),
        "p:settings" => CallbackAction::Page(Command::Settings),
        "p:wallets" => CallbackAction::Page(Command::Wallets),
        "p:withdraw" => CallbackAction::Page(Command::Withdraw),
        "p:bots" => CallbackAction::Page(Command::Bots),
        "p:pumpfun" => CallbackAction::Page(Command::Pumpfun),
        "p:help" => CallbackAction::Page(Command::Help),
        "p:start" | "p:refresh" => CallbackAction::Page(Command::Start),
        "pos:min" | "pos:sell" => CallbackAction::Page(Command::Positions),
        "pos:usd" => CallbackAction::PosUsd,
        "t:copy" => CallbackAction::ToggleCopy,
        "set:slip" => CallbackAction::SetOp(SetOp::Slip),
        "set:pre" => CallbackAction::SetOp(SetOp::Presets),
        "set:cur" => CallbackAction::SetOp(SetOp::Currency),
        "set:dg" => CallbackAction::SetOp(SetOp::Degen),
        "set:ex" => CallbackAction::SetOp(SetOp::Expert),
        "set:bp" => CallbackAction::SetOp(SetOp::BuyProt),
        "set:pp" => CallbackAction::SetOp(SetOp::PrivatePnl),
        "set:fl" => CallbackAction::SetOp(SetOp::Flags),
        "w:new" => CallbackAction::SetOp(SetOp::WalCreate),
        "w:imp" => CallbackAction::SetOp(SetOp::WalImport),
        "w:pick" => CallbackAction::SetOp(SetOp::WalPick),
        "w:exp" => CallbackAction::SetOp(SetOp::WalExport),
        "w:show" => CallbackAction::SetOp(SetOp::WalExportShow),
        "pre:buy" => CallbackAction::SetOp(SetOp::BuyPresets),
        "pre:sell" => CallbackAction::SetOp(SetOp::SellPresets),
        "pre:spot" => CallbackAction::SetOp(SetOp::SpotPresets),
        "pre:copy" => CallbackAction::SetOp(SetOp::CopyPresets),
        "c:add" => CallbackAction::CopyAdd,
        "c:mass" => CallbackAction::CopyMass,
        "c:wallet" => CallbackAction::CopyUpdateWallet,
        "c:pickw1" => CallbackAction::CopyPickWallet(0),
        "c:pause" => CallbackAction::CopyPauseAll,
        "c:start" => CallbackAction::CopyStartAll,
        "c:delete" => CallbackAction::CopyDeleteAll,
        "x:close" | "pos:del" | "pos:dely" => CallbackAction::Close,
        _ => return None,
    })
}

