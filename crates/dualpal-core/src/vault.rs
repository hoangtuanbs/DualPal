//! The optional master password for saved connection secrets.
//!
//! Without a master password, secrets (server passwords, key passphrases, S3
//! secret keys) live in the system store: the macOS Keychain, the Windows
//! Credential Manager or the Linux Secret Service. Anyone who can use the
//! signed-in account can read them there.
//!
//! With a master password, [`ProtectedSecrets`] moves them out of the system
//! store into one encrypted file, `secrets.vault`. The password is stretched
//! with Argon2id into a 256-bit key and the secrets are sealed with
//! AES-256-GCM. The file's header (format, Argon2 settings and salt) is
//! authenticated too, so it cannot be changed to weaken the key. The password
//! itself is never stored: a wrong one simply fails to open the file.
//!
//! The vault starts locked on every run of the app. Until the person types
//! the master password, reading or saving a secret fails with
//! [`Error::Locked`], which the UI answers with an unlock prompt. A secret
//! removed while locked is dropped at the next unlock ([`ProtectedSecrets::retain`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::connections::SecretStore;
use crate::error::{Error, Result};

/// Shortest master password accepted.
pub const MIN_PASSWORD_LEN: usize = 8;

const FORMAT: u32 = 1;
const ALGORITHM: &str = "argon2id+aes-256-gcm";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// Argon2id cost. Stored in the vault file so it can be raised later without
/// breaking older files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdfParams {
    /// Memory in KiB.
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes: well above the OWASP minimum, about a quarter of a
    /// second on a laptop.
    fn default() -> Self {
        Self {
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

impl KdfParams {
    /// Cheap settings for tests only.
    pub fn insecure_for_tests() -> Self {
        Self {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        }
    }
}

/// What the UI shows about the master password.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    /// A master password protects the saved secrets.
    pub protected: bool,
    /// The master password was entered in this run (always true when not
    /// protected).
    pub unlocked: bool,
}

/// The vault file on disk.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultFile {
    format: u32,
    algorithm: String,
    kdf: KdfParams,
    salt: String,
    nonce: String,
    /// Encrypted JSON object of secret key → secret.
    data: String,
}

impl VaultFile {
    /// Authenticated with the data: changing any of it makes decryption fail.
    fn header(&self) -> Vec<u8> {
        format!(
            "dualpal-vault|{}|{}|{}|{}|{}|{}",
            self.format,
            self.algorithm,
            self.kdf.memory_kib,
            self.kdf.iterations,
            self.kdf.parallelism,
            self.salt
        )
        .into_bytes()
    }
}

/// Secrets held in memory while unlocked. Wiped when dropped.
#[derive(Default)]
struct Secrets(BTreeMap<String, String>);

impl Drop for Secrets {
    fn drop(&mut self) {
        for v in self.0.values_mut() {
            v.zeroize();
        }
    }
}

struct Open {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    kdf: KdfParams,
    secrets: Secrets,
}

enum State {
    /// No master password: secrets live in the system store.
    Off,
    /// Protected and not yet unlocked.
    Locked,
    Unlocked(Open),
}

/// A [`SecretStore`] that keeps secrets in the system store, or, once a
/// master password is set, in an encrypted vault file.
///
/// Several running copies of the app may share the vault file. Every change
/// takes an exclusive lock on `secrets.vault.lock`, reloads the file and only
/// then applies itself, so one window never overwrites another's change. A
/// master password changed, set or removed elsewhere is noticed on the next
/// use: the vault locks (or switches to the system store) accordingly.
pub struct ProtectedSecrets {
    /// The system store (Keychain, Credential Manager, Secret Service).
    system: Arc<dyn SecretStore>,
    path: PathBuf,
    kdf: KdfParams,
    state: Mutex<State>,
}

/// The parsed, still encrypted vault file.
struct Sealed {
    file: VaultFile,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    data: Vec<u8>,
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b)
        .map_err(|e| Error::Unsupported(format!("a secure random number source ({e})")))?;
    Ok(b)
}

fn derive(password: &str, salt: &[u8], kdf: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(kdf.memory_kib, kdf.iterations, kdf.parallelism, Some(32))
        .map_err(|e| Error::Invalid(format!("master password settings: {e}")))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| Error::Invalid(format!("master password: {e}")))?;
    Ok(key)
}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key).expect("a 32-byte key")
}

fn wrong_password() -> Error {
    Error::AuthFailed("the master password is not correct".into())
}

fn damaged() -> Error {
    Error::Invalid("the saved password vault is damaged".into())
}

fn check_new_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::Invalid(format!(
            "the master password needs at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

impl ProtectedSecrets {
    /// Use the vault at `path` if it exists (starting locked), otherwise the
    /// system store.
    pub fn open(path: impl AsRef<Path>, system: Arc<dyn SecretStore>) -> Self {
        Self::with_kdf(path, system, KdfParams::default())
    }

    /// Like [`Self::open`], with the Argon2 cost used for new vaults.
    pub fn with_kdf(path: impl AsRef<Path>, system: Arc<dyn SecretStore>, kdf: KdfParams) -> Self {
        let path = path.as_ref().to_path_buf();
        let state = if path.exists() {
            State::Locked
        } else {
            State::Off
        };
        Self {
            system,
            path,
            kdf,
            state: Mutex::new(state),
        }
    }

    pub fn status(&self) -> VaultStatus {
        let mut state = self.state.lock().unwrap();
        let _ = self.refresh(&mut state);
        match &*state {
            State::Off => VaultStatus {
                protected: false,
                unlocked: true,
            },
            State::Locked => VaultStatus {
                protected: true,
                unlocked: false,
            },
            State::Unlocked(_) => VaultStatus {
                protected: true,
                unlocked: true,
            },
        }
    }

    /// Set a master password: move the secrets of `keys` from the system
    /// store into a new vault, then remove them from the system store. When
    /// the system store cannot be read or emptied, nothing changes.
    pub fn enable(&self, password: &str, keys: &[String]) -> Result<()> {
        check_new_password(password)?;
        let _file = self.file_lock()?;
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        if !matches!(*state, State::Off) {
            return Err(Error::Invalid(
                "a master password is already set; change it instead".into(),
            ));
        }
        let mut secrets = Secrets::default();
        for k in keys {
            if let Some(s) = self.system.get_strict(k)? {
                secrets.0.insert(k.clone(), s);
            }
        }
        let salt = random::<SALT_LEN>()?;
        let open = Open {
            key: derive(password, &salt, self.kdf)?,
            salt,
            kdf: self.kdf,
            secrets,
        };
        self.write(&open)?;
        // Only now that the vault is on disk: nothing is lost if writing failed.
        let mut removed: Vec<&String> = Vec::new();
        for k in open.secrets.0.keys() {
            if let Err(e) = self.system.delete_strict(k) {
                // A copy readable without the master password would remain:
                // undo the move rather than claim protection.
                for r in &removed {
                    let _ = self.system.set_durable(r, &open.secrets.0[*r]);
                }
                self.remove_file()?;
                return Err(e);
            }
            removed.push(k);
        }
        *state = State::Unlocked(open);
        Ok(())
    }

    /// Enter the master password for this run of the app.
    pub fn unlock(&self, password: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        match &*state {
            State::Off => Ok(()),
            // Already open: still say whether the password was right.
            State::Unlocked(_) => self.read(password).map(|_| ()),
            State::Locked => {
                let open = self.read(password)?;
                *state = State::Unlocked(open);
                Ok(())
            }
        }
    }

    /// Drop the secrets of every key not in `keys`, e.g. of servers removed
    /// while the vault was locked. Needs the vault unlocked; does nothing
    /// otherwise.
    pub fn retain(&self, keys: &[String]) -> Result<()> {
        self.modify(|secrets| {
            let before = secrets.0.len();
            let gone: Vec<String> = secrets
                .0
                .keys()
                .filter(|k| !keys.contains(k))
                .cloned()
                .collect();
            for k in gone {
                if let Some(mut v) = secrets.0.remove(&k) {
                    v.zeroize();
                }
            }
            secrets.0.len() != before
        })
        .or_else(|e| match e {
            Error::Locked => Ok(()),
            e => Err(e),
        })
    }

    /// Forget the key until the master password is entered again.
    pub fn lock(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(*state, State::Unlocked(_)) {
            *state = State::Locked;
        }
    }

    /// Replace the master password. The current one is checked even when
    /// the vault is unlocked.
    pub fn change(&self, current: &str, new: &str) -> Result<()> {
        check_new_password(new)?;
        let _file = self.file_lock()?;
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        if matches!(*state, State::Off) {
            return Err(Error::Invalid("no master password is set".into()));
        }
        let mut open = self.read(current)?;
        let salt = random::<SALT_LEN>()?;
        open.key = derive(new, &salt, self.kdf)?;
        open.salt = salt;
        open.kdf = self.kdf;
        self.write(&open)?;
        *state = State::Unlocked(open);
        Ok(())
    }

    /// Stop using a master password: move the secrets back into the system
    /// store and delete the vault. All or nothing: when a secret cannot be
    /// saved durably in the system store, or the vault cannot be deleted, the
    /// copies already made are removed again and protection stays on.
    pub fn disable(&self, password: &str) -> Result<()> {
        let _file = self.file_lock()?;
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        if matches!(*state, State::Off) {
            return Ok(());
        }
        let open = self.read(password)?;
        let mut copied: Vec<&String> = Vec::new();
        let undo = |copied: &[&String]| {
            for k in copied {
                let _ = self.system.delete_strict(k);
            }
        };
        for (k, v) in &open.secrets.0 {
            if let Err(e) = self.system.set_durable(k, v) {
                undo(&copied);
                return Err(e);
            }
            copied.push(k);
        }
        if let Err(e) = self.remove_file() {
            undo(&copied);
            return Err(e);
        }
        *state = State::Off;
        Ok(())
    }

    /// For a forgotten master password: delete the vault and every secret in
    /// it. Saved servers stay; their passwords must be entered again.
    pub fn reset(&self) -> Result<()> {
        let _file = self.file_lock()?;
        let mut state = self.state.lock().unwrap();
        self.remove_file()?;
        *state = State::Off;
        Ok(())
    }

    /// Change the unlocked secrets under the file lock, starting from what is
    /// on disk now. `f` returns whether it changed anything.
    fn modify(&self, f: impl FnOnce(&mut Secrets) -> bool) -> Result<()> {
        let _file = self.file_lock()?;
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        match &mut *state {
            State::Off => Err(Error::Invalid("no master password is set".into())),
            State::Locked => Err(Error::Locked),
            State::Unlocked(open) => {
                if f(&mut open.secrets) {
                    if let Err(e) = self.write(open) {
                        // Memory must match the file: read it back.
                        let _ = self.reload(&mut state);
                        return Err(e);
                    }
                }
                Ok(())
            }
        }
    }

    /// Catch up with changes another running copy of the app made to the
    /// vault file.
    fn refresh(&self, state: &mut State) -> Result<()> {
        match state {
            State::Off if self.path.exists() => *state = State::Locked,
            State::Locked if !self.path.exists() => *state = State::Off,
            State::Unlocked(_) => self.reload(state)?,
            _ => {}
        }
        Ok(())
    }

    /// Re-read the vault file with the key in memory.
    fn reload(&self, state: &mut State) -> Result<()> {
        let State::Unlocked(open) = &mut *state else {
            return Ok(());
        };
        if !self.path.exists() {
            // Removed elsewhere: back to the system store.
            *state = State::Off;
            return Ok(());
        }
        let sealed = self.parse()?;
        if sealed.salt != open.salt || sealed.file.kdf != open.kdf {
            // The master password was changed elsewhere: ask for it again.
            *state = State::Locked;
            return Ok(());
        }
        open.secrets = Self::decrypt(&sealed, &open.key)?;
        Ok(())
    }

    /// An exclusive lock on `secrets.vault.lock`, held while the vault file
    /// is read and replaced. Released when dropped.
    fn file_lock(&self) -> Result<std::fs::File> {
        let lock_path = self.path.with_extension("vault.lock");
        if let Some(dir) = lock_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
        }
        let io = |e| Error::io(lock_path.to_string_lossy(), e);
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(io)?;
        f.lock().map_err(io)?;
        Ok(f)
    }

    fn remove_file(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(self.path.to_string_lossy(), e)),
        }
    }

    fn parse(&self) -> Result<Sealed> {
        let bytes =
            std::fs::read(&self.path).map_err(|e| Error::io(self.path.to_string_lossy(), e))?;
        let file: VaultFile = serde_json::from_slice(&bytes).map_err(|_| damaged())?;
        if file.format != FORMAT || file.algorithm != ALGORITHM {
            return Err(Error::Unsupported(
                "this saved password vault (made by a newer Dual Pal?)".into(),
            ));
        }
        let salt: [u8; SALT_LEN] = B64
            .decode(&file.salt)
            .ok()
            .and_then(|s| s.try_into().ok())
            .ok_or_else(damaged)?;
        let nonce: [u8; NONCE_LEN] = B64
            .decode(&file.nonce)
            .ok()
            .and_then(|s| s.try_into().ok())
            .ok_or_else(damaged)?;
        let data = B64.decode(&file.data).map_err(|_| damaged())?;
        Ok(Sealed {
            file,
            salt,
            nonce,
            data,
        })
    }

    fn decrypt(sealed: &Sealed, key: &[u8; 32]) -> Result<Secrets> {
        let plain = Zeroizing::new(
            cipher(key)
                .decrypt(
                    &sealed.nonce.into(),
                    Payload {
                        msg: &sealed.data,
                        aad: &sealed.file.header(),
                    },
                )
                .map_err(|_| wrong_password())?,
        );
        let map: BTreeMap<String, String> =
            serde_json::from_slice(&plain).map_err(|_| damaged())?;
        Ok(Secrets(map))
    }

    /// Open the vault file with `password`.
    fn read(&self, password: &str) -> Result<Open> {
        let sealed = self.parse()?;
        let key = derive(password, &sealed.salt, sealed.file.kdf)?;
        let secrets = Self::decrypt(&sealed, &key)?;
        Ok(Open {
            key,
            salt: sealed.salt,
            kdf: sealed.file.kdf,
            secrets,
        })
    }

    /// Seal `open` with a fresh nonce and replace the vault file.
    fn write(&self, open: &Open) -> Result<()> {
        let nonce = random::<NONCE_LEN>()?;
        let mut file = VaultFile {
            format: FORMAT,
            algorithm: ALGORITHM.into(),
            kdf: open.kdf,
            salt: B64.encode(open.salt),
            nonce: B64.encode(nonce),
            data: String::new(),
        };
        let plain = Zeroizing::new(
            serde_json::to_vec(&open.secrets.0).map_err(|e| Error::Invalid(e.to_string()))?,
        );
        let sealed = cipher(&open.key)
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: &plain,
                    aad: &file.header(),
                },
            )
            .map_err(|_| Error::Invalid("could not encrypt the saved passwords".into()))?;
        file.data = B64.encode(sealed);
        let json = serde_json::to_vec_pretty(&file).map_err(|e| Error::Invalid(e.to_string()))?;
        write_private(&self.path, &json)
    }
}

/// Write `bytes` to `path` through a temporary file, readable by the owner only.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
    }
    let tmp = path.with_extension(format!("vault.{}.tmp", std::process::id()));
    let io = |e| Error::io(tmp.to_string_lossy(), e);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path.to_string_lossy(), e))
}

impl SecretStore for ProtectedSecrets {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let mut state = self.state.lock().unwrap();
        self.refresh(&mut state)?;
        match &*state {
            State::Off => self.system.get(key),
            State::Locked => Err(Error::Locked),
            State::Unlocked(open) => Ok(open.secrets.0.get(key).cloned()),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            self.refresh(&mut state)?;
            if matches!(*state, State::Off) {
                return self.system.set(key, secret);
            }
        }
        self.modify(|s| {
            s.0.insert(key.into(), secret.into());
            true
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            self.refresh(&mut state)?;
            if matches!(*state, State::Off) {
                return self.system.delete(key);
            }
        }
        // An entry left in the system store from before goes too.
        let _ = self.system.delete(key);
        match self.modify(|s| match s.0.remove(key) {
            Some(mut v) => {
                v.zeroize();
                true
            }
            None => false,
        }) {
            // Locked: dropped at the next unlock, which keeps only the
            // secrets of saved servers.
            Ok(()) | Err(Error::Locked) => Ok(()),
            // Turned off elsewhere in the meantime.
            Err(_) if !self.path.exists() => self.system.delete(key),
            Err(e) => Err(e),
        }
    }
}
