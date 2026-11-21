//! Writing a file under a temporary name and putting it in place once it
//! is complete, so a failed or cut-off write never leaves the target
//! half-written.

use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::FileSystem;
use crate::error::{Error, Result};

/// The hidden name a file is written under until its copy is complete.
/// It is new each time, so it never belongs to another file.
pub fn part_path(fs: &dyn FileSystem, dst: &str) -> String {
    let parent = fs.parent(dst).unwrap_or_default();
    let name = fs.file_name(dst).unwrap_or_default();
    let tag = &Uuid::new_v4().simple().to_string()[..12];
    // Keep within the usual 255-byte limit on a name.
    let part = if name.len() <= 200 {
        format!(".{name}.{tag}.dualpal-part")
    } else {
        format!(".dualpal-{tag}.part")
    };
    fs.join(&parent, &part)
}

/// Rename `from` onto the existing file `to`. Where the backend's rename
/// will not replace a file (some servers), the old file is moved aside
/// first, removed once the new one is in place, and put back if the new
/// one cannot be. If it cannot be put back either, the error says where
/// it was left.
pub async fn rename_over(fs: &dyn FileSystem, from: &str, to: &str) -> Result<()> {
    let first = match fs.rename(from, to).await {
        Ok(()) => return Ok(()),
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(e) => e,
    };
    // A temporary name like a partial copy's, so auto-sync ignores it too.
    let aside = part_path(fs, to);
    if fs.rename(to, &aside).await.is_err() {
        return Err(first);
    }
    match fs.rename(from, to).await {
        Ok(()) => {
            let _ = fs.remove_file(&aside).await;
            Ok(())
        }
        Err(e) => match fs.rename(&aside, to).await {
            Ok(()) => Err(e),
            Err(back) => Err(Error::Several(vec![
                e,
                Error::Io {
                    path: aside,
                    source: std::io::Error::other(format!(
                        "the old {to} was left here and could not be put back ({back})"
                    )),
                },
            ])),
        },
    }
}

/// Write `bytes` to `path`, replacing any file there. Unless the backend
/// only shows a file once it is complete (S3), the bytes go to a
/// temporary sibling first, which is renamed over the target at the end:
/// a failed write leaves the old file as it was. A replaced file keeps its
/// permission bits.
pub async fn write_file_atomic(fs: &dyn FileSystem, path: &str, bytes: &[u8]) -> Result<()> {
    let existing = match fs.stat(path).await {
        Ok(e) if e.is_dir() => return Err(Error::Invalid(format!("not a file: {path}"))),
        Ok(e) => Some(e),
        Err(Error::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    if fs.atomic_writes() {
        return write_all(fs, path, bytes).await;
    }
    let tmp = part_path(fs, path);
    let result = async {
        write_all(fs, &tmp, bytes).await?;
        if let Some(mode) = existing.as_ref().and_then(|e| e.mode) {
            let _ = fs.set_mode(&tmp, mode & 0o7777).await;
        }
        if existing.is_some() {
            rename_over(fs, &tmp, path).await
        } else {
            fs.rename(&tmp, path).await
        }
    }
    .await;
    if result.is_err() {
        let _ = fs.remove_file(&tmp).await;
    }
    result
}

async fn write_all(fs: &dyn FileSystem, path: &str, bytes: &[u8]) -> Result<()> {
    let mut w = fs.open_write(path).await?;
    w.write_all(bytes).await.map_err(|e| Error::io(path, e))?;
    w.flush().await.map_err(|e| Error::io(path, e))?;
    w.shutdown().await.map_err(|e| Error::io(path, e))
}
