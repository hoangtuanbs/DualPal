//! The optional master password over saved connection secrets.

use std::sync::Arc;

use dualpal_core::connections::{MemorySecrets, SecretStore};
use dualpal_core::vault::{KdfParams, ProtectedSecrets, VaultStatus};
use dualpal_core::Error;

const PW: &str = "correct horse battery";

fn setup() -> (tempfile::TempDir, Arc<MemorySecrets>, ProtectedSecrets) {
    let dir = tempfile::tempdir().unwrap();
    let system = Arc::new(MemorySecrets::default());
    let vault = ProtectedSecrets::with_kdf(
        dir.path().join("secrets.vault"),
        system.clone(),
        KdfParams::insecure_for_tests(),
    );
    (dir, system, vault)
}

fn reopen(dir: &tempfile::TempDir, system: Arc<MemorySecrets>) -> ProtectedSecrets {
    ProtectedSecrets::with_kdf(
        dir.path().join("secrets.vault"),
        system,
        KdfParams::insecure_for_tests(),
    )
}

#[test]
fn without_a_master_password_secrets_stay_in_the_system_store() {
    let (dir, system, vault) = setup();
    assert_eq!(
        vault.status(),
        VaultStatus {
            protected: false,
            unlocked: true
        }
    );
    vault.set("a", "pw-a").unwrap();
    assert_eq!(system.get("a").unwrap().as_deref(), Some("pw-a"));
    assert_eq!(vault.get("a").unwrap().as_deref(), Some("pw-a"));
    vault.delete("a").unwrap();
    assert_eq!(system.get("a").unwrap(), None);
    assert!(!dir.path().join("secrets.vault").exists());
}

#[test]
fn enabling_moves_secrets_out_of_the_system_store_into_an_encrypted_file() {
    let (dir, system, vault) = setup();
    system.set("a", "pw-a").unwrap();
    system.set("b", "pw-b").unwrap();
    system.set("unrelated", "keep").unwrap();
    vault
        .enable(PW, &["a".into(), "b".into(), "missing".into()])
        .unwrap();

    assert_eq!(system.get("a").unwrap(), None);
    assert_eq!(system.get("b").unwrap(), None);
    assert_eq!(system.get("unrelated").unwrap().as_deref(), Some("keep"));
    assert_eq!(vault.get("a").unwrap().as_deref(), Some("pw-a"));
    assert_eq!(vault.get("missing").unwrap(), None);

    let file = std::fs::read_to_string(dir.path().join("secrets.vault")).unwrap();
    assert!(!file.contains("pw-a") && !file.contains("pw-b"), "{file}");
    assert!(!file.contains(PW));
    assert!(file.contains("argon2id"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("secrets.vault"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "vault must be private, mode {mode:o}");
    }
}

#[test]
fn a_new_run_starts_locked_until_the_master_password_is_entered() {
    let (dir, system, vault) = setup();
    system.set("a", "pw-a").unwrap();
    vault.enable(PW, &["a".into()]).unwrap();
    drop(vault);

    let vault = reopen(&dir, system);
    assert_eq!(
        vault.status(),
        VaultStatus {
            protected: true,
            unlocked: false
        }
    );
    assert!(matches!(vault.get("a"), Err(Error::Locked)));
    assert!(matches!(vault.set("a", "x"), Err(Error::Locked)));

    let err = vault.unlock("wrong password").unwrap_err();
    assert_eq!(err.kind(), "authFailed");
    assert!(matches!(vault.get("a"), Err(Error::Locked)));

    vault.unlock(PW).unwrap();
    assert!(vault.status().unlocked);
    assert_eq!(vault.get("a").unwrap().as_deref(), Some("pw-a"));
}

#[test]
fn new_secrets_are_saved_in_the_vault_and_survive_a_restart() {
    let (dir, system, vault) = setup();
    vault.enable(PW, &[]).unwrap();
    vault.set("new", "pw-new").unwrap();
    assert_eq!(system.get("new").unwrap(), None);
    drop(vault);

    let vault = reopen(&dir, system);
    vault.unlock(PW).unwrap();
    assert_eq!(vault.get("new").unwrap().as_deref(), Some("pw-new"));
}

#[test]
fn locking_forgets_the_key() {
    let (_dir, _system, vault) = setup();
    vault.enable(PW, &[]).unwrap();
    vault.set("a", "pw-a").unwrap();
    vault.lock();
    assert!(matches!(vault.get("a"), Err(Error::Locked)));
    vault.unlock(PW).unwrap();
    assert_eq!(vault.get("a").unwrap().as_deref(), Some("pw-a"));
}

#[test]
fn secrets_of_servers_removed_while_locked_are_dropped_at_unlock_even_after_a_restart() {
    let (dir, system, vault) = setup();
    vault.enable(PW, &[]).unwrap();
    vault.set("a", "pw-a").unwrap();
    vault.set("b", "pw-b").unwrap();
    drop(vault);

    // Removed while locked, then the app quits before anyone unlocks.
    let vault = reopen(&dir, system.clone());
    vault.delete("a").unwrap();
    drop(vault);

    let vault = reopen(&dir, system.clone());
    vault.unlock(PW).unwrap();
    vault.retain(&["b".into()]).unwrap();
    assert_eq!(vault.get("a").unwrap(), None);
    assert_eq!(vault.get("b").unwrap().as_deref(), Some("pw-b"));
    drop(vault);

    let vault = reopen(&dir, system.clone());
    vault.unlock(PW).unwrap();
    assert_eq!(vault.get("a").unwrap(), None);
    vault.disable(PW).unwrap();
    assert_eq!(system.get("a").unwrap(), None);
    assert_eq!(system.get("b").unwrap().as_deref(), Some("pw-b"));
}

/// A system store that refuses some strict operations, like a keyring that
/// is locked or unavailable.
#[derive(Default)]
struct Flaky {
    inner: MemorySecrets,
    fail_read: bool,
    fail_delete: bool,
    fail_durable: bool,
}

impl SecretStore for Flaky {
    fn get(&self, key: &str) -> dualpal_core::Result<Option<String>> {
        self.inner.get(key)
    }
    fn set(&self, key: &str, secret: &str) -> dualpal_core::Result<()> {
        self.inner.set(key, secret)
    }
    fn delete(&self, key: &str) -> dualpal_core::Result<()> {
        self.inner.delete(key)
    }
    fn get_strict(&self, key: &str) -> dualpal_core::Result<Option<String>> {
        if self.fail_read {
            return Err(Error::Unsupported("reading the keyring".into()));
        }
        self.inner.get(key)
    }
    fn set_durable(&self, key: &str, secret: &str) -> dualpal_core::Result<()> {
        if self.fail_durable && key == "b" {
            return Err(Error::Unsupported("saving to the keyring".into()));
        }
        self.inner.set(key, secret)
    }
    fn delete_strict(&self, key: &str) -> dualpal_core::Result<()> {
        if self.fail_delete && key == "b" {
            return Err(Error::Unsupported("removing from the keyring".into()));
        }
        self.inner.delete(key)
    }
}

fn flaky(f: Flaky) -> (tempfile::TempDir, Arc<Flaky>, ProtectedSecrets) {
    let dir = tempfile::tempdir().unwrap();
    f.inner.set("a", "pw-a").unwrap();
    f.inner.set("b", "pw-b").unwrap();
    let system = Arc::new(f);
    let vault = ProtectedSecrets::with_kdf(
        dir.path().join("secrets.vault"),
        system.clone(),
        KdfParams::insecure_for_tests(),
    );
    (dir, system, vault)
}

#[test]
fn protection_is_not_claimed_when_the_system_store_cannot_be_read() {
    let (dir, system, vault) = flaky(Flaky {
        fail_read: true,
        ..Default::default()
    });
    assert!(vault.enable(PW, &["a".into(), "b".into()]).is_err());
    assert!(!vault.status().protected);
    assert!(!dir.path().join("secrets.vault").exists());
    assert_eq!(system.get("a").unwrap().as_deref(), Some("pw-a"));
}

#[test]
fn protection_is_undone_when_a_password_cannot_be_removed_from_the_system_store() {
    let (dir, system, vault) = flaky(Flaky {
        fail_delete: true,
        ..Default::default()
    });
    assert!(vault.enable(PW, &["a".into(), "b".into()]).is_err());
    assert!(!vault.status().protected);
    assert!(!dir.path().join("secrets.vault").exists());
    // "a" was removed before "b" failed: it is put back, nothing is lost.
    assert_eq!(system.get("a").unwrap().as_deref(), Some("pw-a"));
    assert_eq!(system.get("b").unwrap().as_deref(), Some("pw-b"));
}

#[test]
fn turning_it_off_keeps_the_vault_when_the_system_store_cannot_keep_a_password() {
    let (dir, system, vault) = flaky(Flaky {
        fail_durable: true,
        ..Default::default()
    });
    vault.enable(PW, &["a".into(), "b".into()]).unwrap();
    assert!(vault.disable(PW).is_err());
    assert!(vault.status().protected);
    assert!(dir.path().join("secrets.vault").exists());
    // No copy of "a" was left readable without the master password.
    assert_eq!(system.get("a").unwrap(), None);
    assert_eq!(vault.get("b").unwrap().as_deref(), Some("pw-b"));
}

#[test]
fn two_running_copies_do_not_overwrite_each_other() {
    let (dir, system, one) = setup();
    one.enable(PW, &[]).unwrap();
    let two = reopen(&dir, system.clone());
    two.unlock(PW).unwrap();

    one.set("a", "pw-a").unwrap();
    two.set("b", "pw-b").unwrap();
    assert_eq!(two.get("a").unwrap().as_deref(), Some("pw-a"));
    assert_eq!(one.get("b").unwrap().as_deref(), Some("pw-b"));
    one.delete("a").unwrap();
    assert_eq!(two.get("a").unwrap(), None);

    let three = reopen(&dir, system.clone());
    three.unlock(PW).unwrap();
    assert_eq!(three.get("b").unwrap().as_deref(), Some("pw-b"));

    // A new master password set in one copy locks the others.
    one.change(PW, "another password").unwrap();
    assert!(matches!(two.get("b"), Err(Error::Locked)));
    assert!(!two.status().unlocked);
    two.unlock("another password").unwrap();
    assert_eq!(two.get("b").unwrap().as_deref(), Some("pw-b"));

    // Turned off in one copy: the others use the system store again.
    one.disable("another password").unwrap();
    assert!(!two.status().protected);
    assert_eq!(two.get("b").unwrap().as_deref(), Some("pw-b"));
}

#[test]
fn changing_the_master_password_needs_the_current_one() {
    let (dir, system, vault) = setup();
    vault.enable(PW, &[]).unwrap();
    vault.set("a", "pw-a").unwrap();
    assert_eq!(
        vault
            .change("not it at all", "another password")
            .unwrap_err()
            .kind(),
        "authFailed"
    );
    vault.change(PW, "another password").unwrap();
    drop(vault);

    let vault = reopen(&dir, system);
    assert_eq!(vault.unlock(PW).unwrap_err().kind(), "authFailed");
    vault.unlock("another password").unwrap();
    assert_eq!(vault.get("a").unwrap().as_deref(), Some("pw-a"));
}

#[test]
fn short_master_passwords_are_refused() {
    let (_dir, _system, vault) = setup();
    assert_eq!(vault.enable("short", &[]).unwrap_err().kind(), "invalid");
    assert!(!vault.status().protected);
}

#[test]
fn turning_it_off_moves_secrets_back_to_the_system_store() {
    let (dir, system, vault) = setup();
    system.set("a", "pw-a").unwrap();
    vault.enable(PW, &["a".into()]).unwrap();
    assert_eq!(
        vault.disable("wrong password").unwrap_err().kind(),
        "authFailed"
    );
    assert!(vault.status().protected);

    vault.disable(PW).unwrap();
    assert!(!vault.status().protected);
    assert!(!dir.path().join("secrets.vault").exists());
    assert_eq!(system.get("a").unwrap().as_deref(), Some("pw-a"));
    assert_eq!(vault.get("a").unwrap().as_deref(), Some("pw-a"));
}

#[test]
fn reset_for_a_forgotten_password_deletes_the_vault() {
    let (dir, system, vault) = setup();
    system.set("a", "pw-a").unwrap();
    vault.enable(PW, &["a".into()]).unwrap();
    vault.lock();
    vault.reset().unwrap();
    assert!(!vault.status().protected);
    assert!(!dir.path().join("secrets.vault").exists());
    assert_eq!(vault.get("a").unwrap(), None);
}

#[test]
fn a_tampered_vault_does_not_open() {
    let (dir, system, vault) = setup();
    vault.enable(PW, &[]).unwrap();
    vault.set("a", "pw-a").unwrap();
    drop(vault);

    // Weakening the stored Argon2 cost changes the authenticated header.
    let path = dir.path().join("secrets.vault");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["kdf"]["iterations"] = serde_json::json!(2);
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let vault = reopen(&dir, system.clone());
    assert!(vault.unlock(PW).is_err());

    std::fs::write(&path, b"not json").unwrap();
    let vault = reopen(&dir, system);
    assert_eq!(vault.unlock(PW).unwrap_err().kind(), "invalid");
}
