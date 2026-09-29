use crate::config::{encode_pubkey, AppConfig, WatchKind};
use crate::rpc_read::is_placeholder_payer;

#[derive(Clone, Debug)]
pub struct StatusCard {
    pub watch: WatchKind,
    pub wallets: Vec<String>,
    pub our_pubkey: String,
    pub max_sol_lamports: u64,
    pub cooldown_ms: u64,
    pub tips_lamports: [u64; 3],
}

impl StatusCard {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut wallets: Vec<String> = cfg.wallets.iter().map(encode_pubkey).collect();
        wallets.sort();
        Self {
            watch: cfg.watch,
            wallets,
            our_pubkey: encode_pubkey(&cfg.our_pubkey),
            max_sol_lamports: cfg.max_sol_lamports,
            cooldown_ms: cfg.cooldown_ms,
            tips_lamports: cfg.tips.rungs(),
        }
    }

    pub(crate) fn watch_label(&self) -> &'static str {
        match self.watch {
            WatchKind::Rpc => "rpc (developer ws)",
            WatchKind::Laserstream => "laserstream",
        }
    }

    pub(crate) fn max_sol(&self) -> f64 {
        self.max_sol_lamports as f64 / 1_000_000_000.0
    }

    pub fn render(&self) -> String {
        format!(
            "🟢 CopyThatTrader — Solana mainnet\n🌐 cluster: mainnet-beta\n📡 watch: {}\n💰 max copy: {:.4} SOL\n⏱ cooldown: {} ms\n👀 suggested wallets: {}",
            self.watch_label(),
            self.max_sol(),
            self.cooldown_ms,
            self.wallets.len()
        )
    }

    pub fn home_text(&self) -> String {
        format!(
            "🏠 homepage\n\n{}\n\n📋 Tap /menu for calendar, positions, copy, wallets, settings.",
            self.render()
        )
    }

    pub fn wallets_text(&self) -> String {
        let mut lines = vec![
            "👛 wallets".into(),
            format!("payer: `{}`", self.our_pubkey),
        ];
        if is_placeholder_payer(&self.our_pubkey) {
            lines.push("payer is the WSOL mint placeholder — set our_pubkey to your mainnet wallet".into());
        }
        if self.wallets.is_empty() {
            lines.push("suggested: none".into());
        } else {
            lines.push("suggested:".into());
            for (i, w) in self.wallets.iter().enumerate() {
                lines.push(format!("{}. `{w}`", i + 1));
            }
        }
        lines.join("\n")
    }

    pub fn settings_text(&self) -> String {
        let tips: Vec<String> = self
            .tips_lamports
            .iter()
            .map(|t| format!("{:.4}", *t as f64 / 1_000_000_000.0))
            .collect();
        format!(
            "⚙️ settings\n\n🌐 cluster: mainnet-beta\n📡 feed: {}\n💰 default max copy: {:.4} SOL\n⏱ default cooldown: {} ms\n💎 tip rungs: {} SOL\n\nYour size is personal. Use /maxsol 0.05",
            self.watch_label(),
            self.max_sol(),
            self.cooldown_ms,
            tips.join(" / ")
        )
    }

    pub fn copy_text(&self) -> String {
        format!(
            "🪞 copy trading\n\nYour own Pump.fun watch list. Max {:.4} SOL per copy (default).\n\n{}",
            self.max_sol(),
            self.wallets_text()
        )
    }
}
