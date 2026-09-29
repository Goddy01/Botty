use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const MAX_EVENTS: usize = 12;

#[derive(Clone, Debug)]
pub struct MainnetEvent {
    pub slot: u64,
    pub signature: String,
    pub copied: bool,
    pub wallet: String,
    pub mint: String,
    pub sol_lamports: u64,
    pub notify_to_tx_ms: u64,
    pub verdict: String,
}

#[derive(Clone, Debug)]
pub struct LiveSnap {
    pub last_slot: u64,
    pub copies: u64,
    pub skips: u64,
    pub events: Vec<MainnetEvent>,
}

pub struct LiveBoard {
    inner: Mutex<Inner>,
    by_user: Mutex<HashMap<i64, Inner>>,
}

struct Inner {
    last_slot: u64,
    copies: u64,
    skips: u64,
    events: VecDeque<MainnetEvent>,
}

impl Inner {
    fn empty() -> Self {
        Self {
            last_slot: 0,
            copies: 0,
            skips: 0,
            events: VecDeque::new(),
        }
    }

    fn push(&mut self, ev: MainnetEvent) {
        if ev.slot > self.last_slot {
            self.last_slot = ev.slot;
        }
        if ev.copied {
            self.copies += 1;
        } else {
            self.skips += 1;
        }
        self.events.push_front(ev);
        while self.events.len() > MAX_EVENTS {
            self.events.pop_back();
        }
    }

    fn snap(&self) -> LiveSnap {
        LiveSnap {
            last_slot: self.last_slot,
            copies: self.copies,
            skips: self.skips,
            events: self.events.iter().cloned().collect(),
        }
    }
}

impl LiveBoard {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::empty()),
            by_user: Mutex::new(HashMap::new()),
        }
    }

    pub fn record(&self, ev: MainnetEvent) {
        self.inner.lock().expect("live").push(ev);
    }

    pub fn record_user(&self, user_id: i64, ev: MainnetEvent) {
        self.inner.lock().expect("live").push(ev.clone());
        self.by_user
            .lock()
            .expect("live-user")
            .entry(user_id)
            .or_insert_with(Inner::empty)
            .push(ev);
    }

    pub fn bump_slot(&self, slot: u64) {
        let mut g = self.inner.lock().expect("live");
        if slot > g.last_slot {
            g.last_slot = slot;
        }
    }

    pub fn snapshot(&self) -> LiveSnap {
        self.inner.lock().expect("live").snap()
    }

    pub fn snapshot_user(&self, user_id: i64) -> LiveSnap {
        match self.by_user.lock().expect("live-user").get(&user_id) {
            Some(inner) => inner.snap(),
            None => LiveSnap {
                last_slot: self.inner.lock().expect("live").last_slot,
                copies: 0,
                skips: 0,
                events: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_and_counts() {
        let board = LiveBoard::new();
        for i in 0..15 {
            board.record(MainnetEvent {
                slot: 100 + i,
                signature: format!("s{i}"),
                copied: i % 2 == 0,
                wallet: "w".into(),
                mint: "m".into(),
                sol_lamports: 1,
                notify_to_tx_ms: 10,
                verdict: "ok".into(),
            });
        }
        let snap = board.snapshot();
        assert_eq!(snap.events.len(), 12);
        assert_eq!(snap.events[0].signature, "s14");
        assert_eq!(snap.last_slot, 114);
        assert_eq!(snap.copies, 8);
        assert_eq!(snap.skips, 7);
        board.record_user(
            1,
            MainnetEvent {
                slot: 200,
                signature: "mine".into(),
                copied: true,
                wallet: "w".into(),
                mint: "m".into(),
                sol_lamports: 2,
                notify_to_tx_ms: 1,
                verdict: "ok".into(),
            },
        );
        assert_eq!(board.snapshot_user(1).copies, 1);
        assert_eq!(board.snapshot_user(2).copies, 0);
        assert_eq!(board.snapshot_user(2).events.len(), 0);
    }
}
