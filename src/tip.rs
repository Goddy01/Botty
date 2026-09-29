/// Same copy ix at three tip levels; at most one should land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TipLadder {
    pub floor: u64,
    pub mid: u64,
    pub high: u64,
    pub max: u64,
}

impl TipLadder {
    pub fn from_sol(floor: f64, mid: f64, high: f64, max: f64) -> Self {
        let mut s = Self {
            floor: sol_to_lamports(floor),
            mid: sol_to_lamports(mid),
            high: sol_to_lamports(high),
            max: sol_to_lamports(max).max(1_000_000),
        };
        s.normalize();
        s
    }

    pub fn rungs(self) -> [u64; 3] {
        [self.floor, self.mid, self.high]
    }

    pub fn rungs_around(base: u64, max: u64) -> [u64; 3] {
        let max = max.max(1_000_000);
        let floor = base.clamp(1_000_000, max);
        let mut mid = floor.saturating_mul(2).min(max);
        let mut high = floor.saturating_mul(4).min(max);
        if mid <= floor {
            mid = (floor + 1).min(max);
        }
        if high <= mid {
            high = (mid + 1).min(max);
        }
        [floor, mid, high]
    }

    pub fn escalate_above(&mut self, observed_high_lamports: u64) {
        if observed_high_lamports == 0 {
            return;
        }
        let bumped = observed_high_lamports
            .saturating_add(observed_high_lamports / 10)
            .max(observed_high_lamports + 1);
        self.floor = bumped.min(self.max);
        self.mid = self.floor.saturating_mul(2).min(self.max);
        self.high = self.floor.saturating_mul(4).min(self.max);
        self.normalize();
    }

    fn normalize(&mut self) {
        self.floor = self.floor.clamp(1_000_000, self.max);
        if self.mid <= self.floor {
            self.mid = self.floor.saturating_add(self.floor / 2).min(self.max);
        }
        if self.high <= self.mid {
            self.high = self.mid.saturating_add(self.mid / 2).min(self.max);
        }
        if self.mid <= self.floor {
            self.mid = (self.floor + 1).min(self.max);
        }
        if self.high <= self.mid {
            self.high = (self.mid + 1).min(self.max);
        }
    }
}

pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol * 1_000_000_000.0).round() as u64
}

#[derive(Clone, Debug)]
pub struct TipBurst<M> {
    pub tips_lamports: [u64; 3],
    pub messages: [M; 3],
}

pub async fn dispatch_three<M, F, Fut, T>(messages: [M; 3], mut send_one: F) -> [T; 3]
where
    F: FnMut(M) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let [a, b, c] = messages;
    let fa = send_one(a);
    let fb = send_one(b);
    let fc = send_one(c);
    let (ra, rb, rc) = tokio::join!(fa, fb, fc);
    [ra, rb, rc]
}

pub async fn paper_dispatch<M: Clone>(burst: &TipBurst<M>) -> [u64; 3] {
    dispatch_three(burst.tips_lamports, |tip| async move { tip }).await
}

pub fn pick_account(accounts: &[&str]) -> [u8; 32] {
    let n = accounts.len().max(1);
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0) as usize
        % n;
    for i in 0..n {
        let s = accounts[(start + i) % n];
        if let Ok(raw) = bs58::decode(s).into_vec() {
            if raw.len() == 32 {
                let mut out = [0u8; 32];
                out.copy_from_slice(&raw);
                return out;
            }
        }
    }
    let raw = bs58::decode(accounts[0]).into_vec().expect("tip account");
    let mut out = [0u8; 32];
    out.copy_from_slice(&raw);
    out
}
