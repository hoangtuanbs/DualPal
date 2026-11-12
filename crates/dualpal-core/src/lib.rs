//! Platform-independent core of Dual Pal.
//!
//! * [`vfs`] — a virtual file system abstraction. The local disk, SFTP
//!   servers and S3 buckets all sit behind the same [`vfs::FileSystem`] trait.
//! * [`ops`] — a background operation queue (copy, move, delete, rename,
//!   create folder) that reports live progress and supports cancellation.
//! * [`activity`] — a persistent log of finished operations.
//! * [`folder_size`] — the total size and item counts of a folder.
//! * [`search`] — find files by name below a folder.
//! * [`connections`] — saved SFTP and S3 connections with their favorites.
//! * [`remote_edit`] — edit remote files in a local app, uploading on save.
//! * [`compare`] and [`sync`] — folder comparison and one- or two-way sync.
//! * [`rename`] — Multi Rename rules and presets.
//! * [`difftools`] — external file compare apps.
//! * [`import`] — read saved servers from WinSCP, FileZilla and Cyberduck exports.
//!
//! Nothing in this crate depends on Tauri, so it can be tested on any OS and
//! reused by other front ends (CLI, tests).

pub mod activity;
pub mod compare;
pub mod connections;
pub mod difftools;
pub mod error;
pub mod folder_size;
pub mod import;
pub mod launch_log;
pub mod livesync;
pub mod ops;
pub mod places;
pub mod remote_edit;
pub mod rename;
pub mod search;
pub mod settings;
pub mod sync;
pub mod vfs;

pub use activity::{ActivityLog, ActivityRecord};
pub use error::{Error, Result};
pub use ops::{
    ConflictPolicy, Operation, OperationInfo, OperationQueue, OperationStatus, Progress,
};
pub use tokio_util::sync::CancellationToken;
pub use vfs::{Entry, EntryKind, FileSystem, Location, Registry};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
