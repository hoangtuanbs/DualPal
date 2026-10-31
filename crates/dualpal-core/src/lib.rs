//! Platform-independent core of Dual Pal.
//!
//! * [`vfs`] — a virtual file system abstraction. The local disk, SFTP
//!   servers and S3 buckets all sit behind the same [`vfs::FileSystem`] trait.
//! * [`ops`] — a background operation queue (copy, move, delete, rename,
//!   create folder) that reports live progress and supports cancellation.
//! * [`activity`] — a persistent log of finished operations.
//! * [`connections`] — saved SFTP and S3 connections with their favorites.
//! * [`remote_edit`] — edit remote files in a local app, uploading on save.
//!
//! Nothing in this crate depends on Tauri, so it can be tested on any OS and
//! reused by other front ends (CLI, tests, a future sync engine).

pub mod activity;
pub mod connections;
pub mod error;
pub mod ops;
pub mod places;
pub mod remote_edit;
pub mod settings;
pub mod vfs;

pub use activity::{ActivityLog, ActivityRecord};
pub use error::{Error, Result};
pub use ops::{
    ConflictPolicy, Operation, OperationInfo, OperationQueue, OperationStatus, Progress,
};
pub use vfs::{Entry, EntryKind, FileSystem, Location, Registry};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
