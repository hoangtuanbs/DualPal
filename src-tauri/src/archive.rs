//! Commands for browsing archives as folders.

use std::sync::Arc;

use dualpal_core::vfs::archive::{OpenProgress, ProgressFn};
use dualpal_core::vfs::FileSystem;
use dualpal_core::{CancellationToken, Error, Location};
use serde::Serialize;
use tauri::State;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

/// An archive opened as a folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedArchive {
    /// File system id of the archive's contents.
    fs_id: String,
    /// The archive's file name.
    name: String,
    /// Where the archive file is.
    source: Location,
}

/// Progress of `open_archive` `request`, sent as `archive-progress`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveProgress {
    request: u64,
    #[serde(flatten)]
    progress: OpenProgress,
}

/// Open the archive at `location` as a read-only file system. An archive on
/// a server is downloaded first; progress goes out as `archive-progress`
/// events, and [`cancel_open_archive`] stops it.
#[tauri::command]
pub async fn open_archive<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    location: Location,
    request: u64,
) -> CmdResult<OpenedArchive> {
    use tauri::Emitter;
    let cancel = CancellationToken::new();
    state
        .archive_cancel
        .lock()
        .unwrap()
        .insert(request, cancel.clone());
    let last = std::sync::Mutex::new(std::time::Instant::now());
    let progress: ProgressFn = Arc::new(move |p: OpenProgress| {
        let mut last = last.lock().unwrap();
        if last.elapsed().as_millis() < 100 && p.done < p.total {
            return;
        }
        *last = std::time::Instant::now();
        let _ = app.emit(
            "archive-progress",
            ArchiveProgress {
                request,
                progress: p,
            },
        );
    });
    let result = state.archives.open(&location, &cancel, progress).await;
    state.archive_cancel.lock().unwrap().remove(&request);
    let fs = result?;
    Ok(OpenedArchive {
        fs_id: fs.id().to_string(),
        name: fs.name().to_string(),
        source: fs.source().clone(),
    })
}

/// Stop opening archive `request`. Does nothing if it has finished.
#[tauri::command]
pub fn cancel_open_archive(state: State<'_, AppState>, request: u64) {
    if let Some(c) = state.archive_cancel.lock().unwrap().remove(&request) {
        c.cancel();
    }
}

/// Close an open archive once no operation uses it, removing its temporary
/// files.
#[tauri::command]
pub fn close_archive(state: State<'_, AppState>, fs_id: String) {
    state.archives.close(&fs_id);
}
