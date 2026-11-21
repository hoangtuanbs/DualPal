//! The built-in file compare view: compare two files on any file systems,
//! read their lines, copy changes across and save each side. Thin wrappers
//! over `dualpal_core::diff`.

use dualpal_core::diff::{DiffLine, DiffOptions, DiffOutcome, DiffState, Side};
use dualpal_core::viewer::Encoding;
use dualpal_core::{Error, Location};
use tauri::State;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

/// Most lines the UI may ask for at once.
const MAX_LINES: usize = 5_000;

/// Compare `left` and `right`. Text files come back as an open compare
/// (`kind: "text"`); binary files and files too big to compare say so.
/// `encodings` overrides the detected encoding of either side. `request`
/// names the work for [`diff_cancel`].
#[tauri::command]
pub async fn diff_open(
    state: State<'_, AppState>,
    request: u64,
    left: Location,
    right: Location,
    options: Option<DiffOptions>,
    encodings: Option<[Option<Encoding>; 2]>,
) -> CmdResult<DiffOutcome> {
    let lfs = state.registry.get(&left.fs)?;
    let rfs = state.registry.get(&right.fs)?;
    state
        .diff
        .open(
            request,
            (lfs, left.path),
            (rfs, right.path),
            options.unwrap_or_default(),
            encodings.unwrap_or_default(),
        )
        .await
}

/// Lines `from..from + count` of one side, with their intra-line changes.
#[tauri::command]
pub async fn diff_lines(
    state: State<'_, AppState>,
    session: u64,
    side: Side,
    from: usize,
    count: usize,
) -> CmdResult<Vec<DiffLine>> {
    state
        .diff
        .lines(session, side, from, count.min(MAX_LINES))
        .await
}

/// Compare again with other options.
#[tauri::command]
pub async fn diff_set_options(
    state: State<'_, AppState>,
    session: u64,
    options: DiffOptions,
) -> CmdResult<DiffState> {
    state.diff.set_options(session, options).await
}

/// Read one side as another encoding.
#[tauri::command]
pub async fn diff_set_encoding(
    state: State<'_, AppState>,
    session: u64,
    side: Side,
    encoding: Encoding,
) -> CmdResult<DiffState> {
    state.diff.set_encoding(session, side, encoding).await
}

/// Copy change `hunk` to side `to`. `revision` is the one the UI shows.
#[tauri::command]
pub async fn diff_apply(
    state: State<'_, AppState>,
    session: u64,
    revision: u64,
    hunk: usize,
    to: Side,
) -> CmdResult<DiffState> {
    state.diff.apply(session, revision, hunk, to).await
}

/// Write one side back to its file (an upload for files on servers).
#[tauri::command]
pub async fn diff_save(
    state: State<'_, AppState>,
    session: u64,
    side: Side,
) -> CmdResult<DiffState> {
    state.diff.save(session, side).await
}

/// Stop comparing `request`. Does nothing if it has finished.
#[tauri::command]
pub fn diff_cancel(state: State<'_, AppState>, request: u64) {
    state.diff.cancel(request);
}

/// Forget an open compare.
#[tauri::command]
pub fn diff_close(state: State<'_, AppState>, session: u64) {
    state.diff.close(session);
}
