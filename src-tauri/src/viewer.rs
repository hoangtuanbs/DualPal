//! The file viewer (F3): open a file on any file system, read pages of it,
//! search it and count its lines. Thin wrappers over `dualpal_core::viewer`.

use dualpal_core::viewer::{Encoding, SearchOptions, ViewerFile};
use dualpal_core::{Error, Location};
use serde::Serialize;
use tauri::State;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

/// Largest page the UI may ask for at once.
const MAX_READ: usize = 4 << 20;

/// Open `location` for viewing. Its first page is read to guess the encoding.
#[tauri::command]
pub async fn viewer_open(state: State<'_, AppState>, location: Location) -> CmdResult<ViewerFile> {
    let fs = state.registry.get(&location.fs)?;
    state.viewer.open(fs, &location.path).await
}

/// Up to `length` bytes from `offset` of an open file, as raw bytes (an
/// `ArrayBuffer` in the UI). Empty past the end.
#[tauri::command]
pub async fn viewer_read(
    state: State<'_, AppState>,
    session: u64,
    offset: u64,
    length: usize,
) -> CmdResult<tauri::ipc::Response> {
    let bytes = state
        .viewer
        .read(session, offset, length.min(MAX_READ))
        .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// How far search `request` has looked, sent as `viewer-search-progress`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchProgress {
    request: u64,
    scanned: u64,
}

/// Find `needle` (bytes in the file's encoding) in an open file from
/// `from`, or before it with `backward`. Returns the offset of the match,
/// or null. `request` names the search for [`viewer_cancel`]. `encoding`
/// is the text encoding of a text search (matches start on a code unit and
/// ignoring case folds whole ones); null for raw bytes.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn viewer_search<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    session: u64,
    request: u64,
    needle: Vec<u8>,
    from: u64,
    backward: bool,
    ignore_case: bool,
    encoding: Option<Encoding>,
) -> CmdResult<Option<u64>> {
    use tauri::Emitter;
    let mut last = std::time::Instant::now();
    let options = SearchOptions {
        backward,
        ignore_case,
        encoding,
        ..Default::default()
    };
    state
        .viewer
        .search(session, request, &needle, from, options, |scanned| {
            if last.elapsed().as_millis() >= 150 {
                last = std::time::Instant::now();
                let _ = app.emit(
                    "viewer-search-progress",
                    SearchProgress { request, scanned },
                );
            }
        })
        .await
}

/// Line feeds in the first `upto` bytes of an open file, i.e. the number of
/// the line `upto` starts, from 0.
#[tauri::command]
pub async fn viewer_count_lines(
    state: State<'_, AppState>,
    session: u64,
    request: u64,
    upto: u64,
    encoding: Encoding,
) -> CmdResult<u64> {
    state
        .viewer
        .count_lines(session, request, upto, encoding)
        .await
}

/// Stop search or line count `request`. Does nothing if it has finished.
#[tauri::command]
pub fn viewer_cancel(state: State<'_, AppState>, request: u64) {
    state.viewer.cancel(request);
}

/// Forget an open file and its cached pages.
#[tauri::command]
pub fn viewer_close(state: State<'_, AppState>, session: u64) {
    state.viewer.close(session);
}
