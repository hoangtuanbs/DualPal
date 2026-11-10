//! Total size of a folder, for the Space key and the Info panel.
//!
//! [`calculate`] walks the folder through the [`FileSystem`] trait, so it
//! works the same on the local disk, a server or a bucket.

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{EntryKind, Location, Registry};

/// What a folder holds, as found by [`calculate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderSize {
    /// Bytes in the regular files below the folder.
    pub bytes: u64,
    /// Files and other non-folder items below the folder, symbolic links included.
    pub files: u64,
    /// Folders below the folder, not counting the folder itself.
    pub folders: u64,
    /// Subfolders that could not be read and were left out.
    pub skipped: u64,
}

/// Add up the folder at `location`.
///
/// Symbolic links are not followed: a link (even one to a folder) counts as
/// one item with no bytes, so a link back up the tree cannot make the walk
/// endless. Only `location` itself may be a link to a folder, and is then
/// followed, since the caller named it. A subfolder that cannot be read is counted in `skipped` and the
/// walk goes on. Failing to read the folder itself is an error. The walk stops
/// with [`Error::Cancelled`] once `cancel` is set.
pub async fn calculate(
    registry: &Registry,
    location: &Location,
    cancel: &CancellationToken,
) -> Result<FolderSize> {
    let fs = registry.get(&location.fs)?;
    let root = tokio::select! {
        r = fs.stat(&location.path) => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    if !root.is_dir() {
        return Err(Error::Invalid(format!("not a folder: {}", location.path)));
    }
    let mut total = FolderSize::default();
    let mut pending = vec![location.path.clone()];
    // The folder itself is the first one popped.
    let mut is_root = true;
    while let Some(dir) = pending.pop() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let listing = tokio::select! {
            r = fs.list(&dir) => r,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let entries = match listing {
            Ok(entries) => entries,
            Err(e) if is_root => return Err(e),
            Err(_) => {
                total.skipped += 1;
                continue;
            }
        };
        is_root = false;
        for e in entries {
            if e.kind == EntryKind::Dir && !e.symlink {
                total.folders += 1;
                pending.push(e.path);
            } else {
                total.files += 1;
                if e.kind == EntryKind::File && !e.symlink {
                    total.bytes += e.size;
                }
            }
        }
    }
    Ok(total)
}
