//! Small JSON stores in the app's config folder (favorites, settings) that
//! more than one copy of the app may change at once.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Run `f` while holding an exclusive lock on `<path>.lock`, so another
/// copy of the app changing the same store waits for it. `f` should read
/// the store again before changing it, so it never writes over a change
/// made since this copy last read it.
pub fn with_lock<T>(path: &Path, f: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    let lock = lock_path(path);
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock)?;
    file.lock()?;
    let result = f();
    let _ = file.unlock();
    result
}

/// Write `bytes` to `path` through a temporary sibling that is renamed over
/// it, so a failed write never leaves the store half-written.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(
        ".{name}.{}.dualpal-part",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_through_a_temporary_file_and_locks_a_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub/store.json");
        with_lock(&path, || write_atomic(&path, b"[1]")).unwrap();
        write_atomic(&path, b"[2]").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[2]");
        let mut names: Vec<_> = std::fs::read_dir(tmp.path().join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["store.json", "store.json.lock"]);
    }
}
