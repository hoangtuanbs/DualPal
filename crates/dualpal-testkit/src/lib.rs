//! Test kit for Dual Pal.
//!
//! Shared by the core integration tests, the Tauri command tests and the
//! desktop E2E fixtures so that every layer uses the same trees, the same
//! in-memory file system and the same way of waiting for a condition.
//!
//! Nothing in here is part of the shipped app.

pub mod contract;
pub mod eventually;
pub mod faulty;
pub mod fixture;
pub mod memfs;
pub mod queue;
pub mod remote;
pub mod secrets;
pub mod toxi;

pub use contract::{path_laws, run_contract, Caps};
pub use eventually::eventually;
pub use faulty::{FaultOp, FaultyFs};
pub use fixture::{
    hash, many_files, name_torture, read, set_mtime, small_tree, standard_tree, write, BASE_TIME_MS,
};
pub use memfs::MemFs;
pub use queue::{collect_events, queue, queue_with, run, EventRecorder};
pub use remote::{remote_env, unique_base, UniqueBase};
pub use secrets::{FailingSecrets, MemorySecrets};
