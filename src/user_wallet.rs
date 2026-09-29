use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result};
use base64::Engine;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ENC_PREFIX: &str = "enc:v1:";

fn store_key_path(dir: &Path) -> PathBuf {
    dir.join(".master.key")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserWallet {
    pub telegram_user_id: i64,
    pub label: String,
    pub pubkey: String,
    /// In memory: plaintext base58 seed. On disk: `enc:v1:…` ciphertext.
    pub secret_seed: String,
    pub created_unix: u64,
}

impl UserWallet {
    /// 64-byte Solana secret key (seed + pubkey) as base58 — Phantom / Solflare import.
    pub fn secret_key_b58(&self) -> Option<String> {
        let seed = bs58::decode(&self.secret_seed).into_vec().ok()?;
        let pubk = bs58::decode(&self.pubkey).into_vec().ok()?;
        if seed.len() != 32 || pubk.len() != 32 {
            return None;
        }
        let mut full = Vec::with_capacity(64);
        full.extend_from_slice(&seed);
        full.extend_from_slice(&pubk);
        Some(bs58::encode(full).into_string())
    }
}

pub struct CreatedWallet {
    pub wallet: UserWallet,
    pub created: bool,
}

const MAX_WALLETS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WalletPack {
    telegram_user_id: i64,
    #[serde(default)]
    active: usize,
    wallets: Vec<UserWallet>,
}

impl WalletPack {
    fn empty(user_id: i64) -> Self {
        Self {
            telegram_user_id: user_id,
            active: 0,
            wallets: Vec::new(),
        }
    }

    fn from_single(w: UserWallet) -> Self {
        Self {
            telegram_user_id: w.telegram_user_id,
            active: 0,
            wallets: vec![w],
        }
    }

    fn active(&self) -> Option<&UserWallet> {
        if self.wallets.is_empty() {
            return None;
        }
        self.wallets.get(self.active.min(self.wallets.len() - 1))
    }

    fn decrypt_in_place(&mut self, key: &[u8; 32]) -> Result<()> {
        for w in &mut self.wallets {
            w.secret_seed = decrypt_seed(key, &w.secret_seed)?;
        }
        Ok(())
    }

    fn for_disk(&self, key: &[u8; 32]) -> Result<WalletPack> {
        let mut out = self.clone();
        for w in &mut out.wallets {
            w.secret_seed = encrypt_seed(key, &w.secret_seed)?;
        }
        Ok(out)
    }
}

pub struct WalletStore {
    dir: PathBuf,
    key: [u8; 32],
    lock: Mutex<()>,
}

impl WalletStore {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let key = load_or_create_master_key(&store_key_path(&dir))?;
        Ok(Self {
            dir,
            key,
            lock: Mutex::new(()),
        })
    }

    pub fn get(&self, user_id: i64) -> Result<Option<UserWallet>> {
        Ok(self.load_pack(user_id)?.active().cloned())
    }

    pub fn get_at(&self, user_id: i64, idx: usize) -> Result<Option<UserWallet>> {
        Ok(self.load_pack(user_id)?.wallets.get(idx).cloned())
    }

    pub fn wallets_for(&self, user_id: i64) -> Vec<UserWallet> {
        self.load_pack(user_id)
            .map(|p| p.wallets)
            .unwrap_or_default()
    }

    pub fn active_index(&self, user_id: i64) -> usize {
        let Ok(p) = self.load_pack(user_id) else {
            return 0;
        };
        if p.wallets.is_empty() {
            0
        } else {
            p.active.min(p.wallets.len() - 1)
        }
    }

    /// First `/start` creates W1. Later calls return the active wallet.
    pub fn get_or_create(&self, user_id: i64) -> Result<CreatedWallet> {
        let _g = self.lock.lock().expect("wallet store");
        let mut pack = self.load_pack(user_id)?;
        if let Some(wallet) = pack.active().cloned() {
            return Ok(CreatedWallet {
                wallet,
                created: false,
            });
        }
        let wallet = generate(user_id, "W1");
        pack.wallets.push(wallet.clone());
        pack.active = 0;
        self.write_pack(&pack)?;
        Ok(CreatedWallet {
            wallet,
            created: true,
        })
    }

    /// Mint another key (W2, W3, …). Makes it the active wallet.
    pub fn create_new(&self, user_id: i64) -> Result<CreatedWallet> {
        let _g = self.lock.lock().expect("wallet store");
        let mut pack = self.load_pack(user_id)?;
        if pack.wallets.is_empty() {
            let wallet = generate(user_id, "W1");
            pack.wallets.push(wallet.clone());
            pack.active = 0;
            self.write_pack(&pack)?;
            return Ok(CreatedWallet {
                wallet,
                created: true,
            });
        }
        anyhow::ensure!(
            pack.wallets.len() < MAX_WALLETS,
            "wallet limit ({MAX_WALLETS})"
        );
        let wallet = generate(user_id, &next_label(&pack.wallets));
        pack.wallets.push(wallet.clone());
        pack.active = pack.wallets.len() - 1;
        self.write_pack(&pack)?;
        Ok(CreatedWallet {
            wallet,
            created: true,
        })
    }

    pub fn select(&self, user_id: i64, idx: usize) -> Result<UserWallet> {
        let _g = self.lock.lock().expect("wallet store");
        let mut pack = self.load_pack(user_id)?;
        anyhow::ensure!(!pack.wallets.is_empty(), "no wallets");
        anyhow::ensure!(idx < pack.wallets.len(), "wallet index");
        pack.active = idx;
        let wallet = pack.wallets[idx].clone();
        self.write_pack(&pack)?;
        Ok(wallet)
    }

    /// Add a key as a new wallet. Existing wallets are kept. Active wallet is unchanged.
    pub fn import_secret(&self, user_id: i64, raw: &str) -> Result<UserWallet> {
        let seed = parse_solana_secret(raw)?;
        let signing = SigningKey::from_bytes(&seed);
        let pubkey = signing.verifying_key().to_bytes();
        let pk = bs58::encode(pubkey).into_string();
        let _g = self.lock.lock().expect("wallet store");
        let mut pack = self.load_pack(user_id)?;
        if let Some(existing) = pack.wallets.iter().find(|w| w.pubkey == pk).cloned() {
            return Ok(existing);
        }
        anyhow::ensure!(
            pack.wallets.len() < MAX_WALLETS,
            "wallet limit ({MAX_WALLETS})"
        );
        let wallet = UserWallet {
            telegram_user_id: user_id,
            label: next_label(&pack.wallets),
            pubkey: pk,
            secret_seed: bs58::encode(seed).into_string(),
            created_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        pack.wallets.push(wallet.clone());
        self.write_pack(&pack)?;
        Ok(wallet)
    }

    pub fn list(&self) -> Result<Vec<UserWallet>> {
        let mut out = Vec::new();
        if !self.dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.dir).with_context(|| format!("read {}", self.dir.display()))? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(user_id) = stem.parse::<i64>() else {
                continue;
            };
            out.extend(self.load_pack(user_id)?.wallets);
        }
        Ok(out)
    }

    fn load_pack(&self, user_id: i64) -> Result<WalletPack> {
        let path = self.path(user_id);
        let Some(raw) = fs::read_to_string(&path).ok() else {
            return Ok(WalletPack::empty(user_id));
        };
        let mut pack = if let Ok(pack) = serde_json::from_str::<WalletPack>(&raw) {
            if !pack.wallets.is_empty() {
                pack
            } else {
                serde_json::from_str::<UserWallet>(&raw)
                    .map(WalletPack::from_single)
                    .unwrap_or_else(|_| WalletPack::empty(user_id))
            }
        } else {
            serde_json::from_str::<UserWallet>(&raw)
                .map(WalletPack::from_single)
                .unwrap_or_else(|_| WalletPack::empty(user_id))
        };
        pack.decrypt_in_place(&self.key)?;
        Ok(pack)
    }

    fn write_pack(&self, pack: &WalletPack) -> Result<()> {
        let path = self.path(pack.telegram_user_id);
        let tmp = path.with_extension("json.tmp");
        let disk = pack.for_disk(&self.key)?;
        let body = serde_json::to_string_pretty(&disk).context("serialize wallets")?;
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }

    pub(crate) fn path(&self, user_id: i64) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }
}

fn load_or_create_master_key(path: &Path) -> Result<[u8; 32]> {
    if let Ok(b64) = std::env::var("WALLET_MASTER_KEY") {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .context("WALLET_MASTER_KEY base64")?;
        anyhow::ensure!(raw.len() == 32, "WALLET_MASTER_KEY must decode to 32 bytes");
        let mut key = [0u8; 32];
        key.copy_from_slice(&raw);
        return Ok(key);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let b64 = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .context("wallet master key base64")?;
        anyhow::ensure!(raw.len() == 32, "master key must decode to 32 bytes");
        let mut key = [0u8; 32];
        key.copy_from_slice(&raw);
        return Ok(key);
    }
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);
    let tmp = path.with_extension("key.tmp");
    fs::write(&tmp, &b64).with_context(|| format!("write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
    Ok(key)
}

fn encrypt_seed(key_bytes: &[u8; 32], plain_b58: &str) -> Result<String> {
    if plain_b58.starts_with(ENC_PREFIX) {
        return Ok(plain_b58.to_string());
    }
    let key = Key::<Aes256Gcm>::from_slice(key_bytes);
    let cipher = Aes256Gcm::new(key);
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plain_b58.as_bytes())
        .map_err(|e| anyhow::anyhow!("encrypt seed: {e}"))?;
    let mut blob = Vec::with_capacity(12 + ct.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ct);
    Ok(format!(
        "{ENC_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    ))
}

fn decrypt_seed(key_bytes: &[u8; 32], stored: &str) -> Result<String> {
    let Some(rest) = stored.strip_prefix(ENC_PREFIX) else {
        return Ok(stored.to_string());
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(rest)
        .context("encrypted seed base64")?;
    anyhow::ensure!(blob.len() > 12, "encrypted seed too short");
    let (nonce_bytes, ct) = blob.split_at(12);
    let key = Key::<Aes256Gcm>::from_slice(key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);
    let plain = cipher
        .decrypt(nonce, ct)
        .map_err(|_| anyhow::anyhow!("decrypt seed failed — wrong WALLET_MASTER_KEY / .master.key?"))?;
    String::from_utf8(plain).context("seed utf8")
}

fn generate(user_id: i64, label: &str) -> UserWallet {
    let signing = SigningKey::generate(&mut OsRng);
    let secret = signing.to_bytes();
    let pubkey = signing.verifying_key().to_bytes();
    UserWallet {
        telegram_user_id: user_id,
        label: label.into(),
        pubkey: bs58::encode(pubkey).into_string(),
        secret_seed: bs58::encode(secret).into_string(),
        created_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

fn next_label(wallets: &[UserWallet]) -> String {
    let mut used = std::collections::HashSet::new();
    for w in wallets {
        if let Some(n) = w.label.strip_prefix('W').and_then(|s| s.parse::<u32>().ok()) {
            used.insert(n);
        }
    }
    let mut i = 1u32;
    while used.contains(&i) {
        i += 1;
    }
    format!("W{i}")
}

/// Accept Phantom/Solflare 64-byte base58, 32-byte seed, JSON byte array, or hex.
pub fn parse_solana_secret(raw: &str) -> Result<[u8; 32]> {
    let t = raw.trim();
    anyhow::ensure!(!t.is_empty(), "empty key");
    if t.starts_with('[') {
        return seed_from_bytes(&json_key_bytes(t)?);
    }
    let hex = t.strip_prefix("0x").unwrap_or(t);
    if hex.len() % 2 == 0
        && (hex.len() == 64 || hex.len() == 128)
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let h = hex.as_bytes();
        let mut i = 0;
        while i < h.len() {
            let s = std::str::from_utf8(&h[i..i + 2]).context("hex")?;
            bytes.push(u8::from_str_radix(s, 16).context("hex byte")?);
            i += 2;
        }
        return seed_from_bytes(&bytes);
    }
    let bytes = bs58::decode(t)
        .into_vec()
        .map_err(|_| anyhow::anyhow!("not a Solana secret key"))?;
    seed_from_bytes(&bytes)
}

fn json_key_bytes(t: &str) -> Result<Vec<u8>> {
    let vals: Vec<u64> = serde_json::from_str(t).context("key JSON")?;
    anyhow::ensure!(
        vals.iter().all(|n| *n <= 255),
        "key JSON bytes out of range"
    );
    Ok(vals.into_iter().map(|n| n as u8).collect())
}

fn seed_from_bytes(bytes: &[u8]) -> Result<[u8; 32]> {
    match bytes.len() {
        32 => {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(bytes);
            Ok(seed)
        }
        64 => {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes[..32]);
            let signing = SigningKey::from_bytes(&seed);
            let pubk = signing.verifying_key().to_bytes();
            anyhow::ensure!(pubk.as_slice() == &bytes[32..], "secret key pubkey mismatch");
            Ok(seed)
        }
        _ => anyhow::bail!("need a 32-byte seed or 64-byte secret key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> WalletStore {
        let dir = std::env::temp_dir().join(format!(
            "copybot-wallets-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        WalletStore::open(dir).unwrap()
    }

    #[test]
    fn first_create_then_same_pubkey() {
        let store = tmp_store();
        let a = store.get_or_create(42).unwrap();
        assert!(a.created);
        assert_eq!(a.wallet.label, "W1");
        assert_eq!(bs58::decode(&a.wallet.pubkey).into_vec().unwrap().len(), 32);
        assert_eq!(
            bs58::decode(&a.wallet.secret_seed).into_vec().unwrap().len(),
            32
        );
        let sk = a.wallet.secret_key_b58().unwrap();
        assert_eq!(bs58::decode(&sk).into_vec().unwrap().len(), 64);
        let b = store.get_or_create(42).unwrap();
        assert!(!b.created);
        assert_eq!(a.wallet.pubkey, b.wallet.pubkey);
        let other = store.get_or_create(99).unwrap();
        assert_ne!(a.wallet.pubkey, other.wallet.pubkey);
    }

    #[test]
    fn seeds_encrypted_at_rest() {
        let store = tmp_store();
        let a = store.get_or_create(7).unwrap();
        let path = store.path(7);
        let disk = fs::read_to_string(&path).unwrap();
        assert!(disk.contains(ENC_PREFIX), "disk should use enc:v1");
        assert!(
            !disk.contains(&a.wallet.secret_seed),
            "plaintext seed must not appear on disk"
        );
        let again = store.get(7).unwrap().unwrap();
        assert_eq!(again.secret_seed, a.wallet.secret_seed);
    }

    #[test]
    fn import_phantom_json_and_base58() {
        let store = tmp_store();
        let a = store.get_or_create(1).unwrap();
        let seed_b58 = a.wallet.secret_seed.clone();
        let full = a.wallet.secret_key_b58().unwrap();
        let b = store.import_secret(2, &full).unwrap();
        assert_eq!(b.pubkey, a.wallet.pubkey);
        let c = store.import_secret(3, &seed_b58).unwrap();
        assert_eq!(c.pubkey, a.wallet.pubkey);
        let seed = bs58::decode(&seed_b58).into_vec().unwrap();
        let mut arr = Vec::with_capacity(64);
        arr.extend_from_slice(&seed);
        arr.extend_from_slice(&bs58::decode(&a.wallet.pubkey).into_vec().unwrap());
        let json = format!(
            "[{}]",
            arr.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let d = store.import_secret(4, &json).unwrap();
        assert_eq!(d.pubkey, a.wallet.pubkey);
    }

    #[test]
    fn create_new_mints_w2_and_selects_it() {
        let store = tmp_store();
        let w1 = store.get_or_create(5).unwrap();
        let w2 = store.create_new(5).unwrap();
        assert_eq!(w2.wallet.label, "W2");
        assert_ne!(w1.wallet.pubkey, w2.wallet.pubkey);
        assert_eq!(store.active_index(5), 1);
        let back = store.select(5, 0).unwrap();
        assert_eq!(back.pubkey, w1.wallet.pubkey);
        assert_eq!(store.active_index(5), 0);
    }

    #[test]
    fn loads_legacy_single_wallet_file() {
        let store = tmp_store();
        let w = generate(11, "W1");
        let path = store.path(11);
        // Legacy plaintext on disk — load decrypts (passthrough) then next write encrypts.
        fs::write(&path, serde_json::to_string_pretty(&w).unwrap()).unwrap();
        let got = store.get(11).unwrap().unwrap();
        assert_eq!(got.pubkey, w.pubkey);
        assert_eq!(got.secret_seed, w.secret_seed);
        store.select(11, 0).unwrap();
        let disk = fs::read_to_string(&path).unwrap();
        assert!(disk.contains(ENC_PREFIX));
    }
}
