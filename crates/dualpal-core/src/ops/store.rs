//! Operations kept across restarts (`queue.json` in the app data folder).
//!
//! Copies and moves that have not finished are written here, with the
//! temporary files their copies were writing, so that the next run of the app
//! can offer to resume or discard them. The file is rewritten through a
//! temporary file and a rename, so a crash never leaves half of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Operation, TransferOptions};

/// One operation as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedOperation {
    pub id: Uuid,
    pub operation: Operation,
    #[serde(default)]
    pub options: TransferOptions,
    pub queued_ms: i64,
    /// Temporary files being written, by the path they will be renamed to.
    /// All are on the operation's destination file system.
    #[serde(default)]
    pub parts: BTreeMap<String, String>,
}

impl SavedOperation {
    /// Id of the file system the operation writes to.
    pub fn destination_fs(&self) -> Option<&str> {
        match &self.operation {
            Operation::Copy { destination, .. } | Operation::Move { destination, .. } => {
                Some(&destination.fs)
            }
            Operation::CopyItems { items, .. } => items.first().map(|i| i.target.fs.as_str()),
            _ => None,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    operations: Vec<SavedOperation>,
}

pub(crate) struct QueueStore {
    path: Option<PathBuf>,
    ops: Mutex<Vec<SavedOperation>>,
    /// Serializes writes, so an older snapshot never lands after a newer one.
    write: Mutex<()>,
}

impl QueueStore {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            ops: Mutex::new(Vec::new()),
            write: Mutex::new(()),
        }
    }

    /// Open `path`; a missing or unreadable file starts empty.
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let ops = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<File>(&b).ok())
            .map(|f| f.operations)
            .unwrap_or_default();
        Self {
            path: Some(path),
            ops: Mutex::new(ops),
            write: Mutex::new(()),
        }
    }

    pub fn all(&self) -> Vec<SavedOperation> {
        self.ops.lock().unwrap().clone()
    }

    pub fn get(&self, id: Uuid) -> Option<SavedOperation> {
        self.ops
            .lock()
            .unwrap()
            .iter()
            .find(|o| o.id == id)
            .cloned()
    }

    pub fn insert(&self, op: SavedOperation) {
        {
            let mut ops = self.ops.lock().unwrap();
            ops.retain(|o| o.id != op.id);
            ops.push(op);
        }
        self.save();
    }

    pub fn remove(&self, id: Uuid) -> Option<SavedOperation> {
        let removed = {
            let mut ops = self.ops.lock().unwrap();
            let i = ops.iter().position(|o| o.id == id)?;
            Some(ops.remove(i))
        };
        self.save();
        removed
    }

    /// Note that `part` is being written for `target`, or that it is gone
    /// (`part` = None).
    pub fn set_part(&self, id: Uuid, target: &str, part: Option<&str>) {
        let changed = {
            let mut ops = self.ops.lock().unwrap();
            let Some(op) = ops.iter_mut().find(|o| o.id == id) else {
                return;
            };
            match part {
                Some(p) => {
                    op.parts
                        .insert(target.to_string(), p.to_string())
                        .as_deref()
                        != Some(p)
                }
                None => op.parts.remove(target).is_some(),
            }
        };
        if changed {
            self.save();
        }
    }

    pub fn part(&self, id: Uuid, target: &str) -> Option<String> {
        self.ops
            .lock()
            .unwrap()
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| o.parts.get(target).cloned())
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let _guard = self.write.lock().unwrap();
        let file = File {
            operations: self.all(),
        };
        if let Err(e) = write_atomic(path, &file) {
            eprintln!("could not save {}: {e}", path.display());
        }
    }
}

/// Write `value` as JSON to `path` through a temporary file and a rename.
fn write_atomic(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&json)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}
