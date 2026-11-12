# Dual Pal — Automation Test Plan

Version 1.0 · 2026-11-07 · Scope: everything in this repository at commit `7aee149` (FTP/FTPS/WebDAV, auto-sync, drag and drop, "go up when a folder disappears").

This document is written so that an AI coding agent (or a person) can implement the whole automated test suite **without further context**. It names the tools, the files to create, the fixtures, every test case with an ID, the CI jobs, and the order to build them in. Each phase in [§10](#10-phased-implementation-order) is sized to be one pull request.

---

## Contents

1. [How to use this document (rules for the implementing agent)](#1-how-to-use-this-document)
2. [System under test](#2-system-under-test)
3. [Current test baseline](#3-current-test-baseline)
4. [Strategy and test layers](#4-strategy-and-test-layers)
5. [Tooling per layer](#5-tooling-per-layer)
6. [Testability changes to make first](#6-testability-changes-to-make-first)
7. [Fixtures and test data](#7-fixtures-and-test-data)
8. [Test case catalogue](#8-test-case-catalogue)
9. [CI/CD design](#9-cicd-design)
10. [Phased implementation order](#10-phased-implementation-order)
11. [Appendices](#11-appendices) — environment variables, commands, traceability matrix

---

## 1. How to use this document

### 1.1 Rules for the implementing agent

1. **Work phase by phase** ([§10](#10-phased-implementation-order)). Open one PR per phase. Do not start a phase until the previous one is green in CI.
2. **Every test carries its ID.** Rust: put the ID in the test function's doc comment, e.g. `/// VFS-CON-007` above `async fn rename_overwrites_nothing()`. TypeScript: put it in the test title, e.g. `test("UI-KEY-012 F5 copies the marked items", …)`. This keeps the traceability matrix ([§11.3](#113-traceability-matrix)) greppable: `rg "VFS-CON-007"`.
3. **Do not duplicate existing tests.** Cases marked **`EXISTS`** in the catalogue are already covered by the named test. Add the ID comment to that test instead of writing a new one. Cases marked **`EXTEND`** have partial coverage: add the missing assertions to the existing test or write a sibling test.
4. **No fixed sleeps.** Wait on a condition with a timeout (Rust: the `eventually` helper, [§7.2](#72-rust-test-kit-crate); Playwright/WebdriverIO: built-in auto-waiting `expect(...).toBe...`). A `sleep` is allowed only to *create* a time gap (e.g. to make one file newer than another), and even then prefer setting mtimes explicitly.
5. **Hermetic.** Tests must not touch the real home folder, the real app data folder, the real keychain, or the network beyond `127.0.0.1`. Use temporary directories and the overrides from [§6](#6-testability-changes-to-make-first).
6. **Production code changes are limited** to the testability hooks in [§6](#6-testability-changes-to-make-first) unless a test exposes a real bug. If it does: keep the failing test, mark it `#[ignore = "BUG: <short description>"]` / `test.fixme(...)`, and list it in the PR description. Do not change product behaviour to make a test pass without saying so.
7. **Run the repo's checks before pushing**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run check`, plus the new commands each phase adds.
8. **Priorities**: `P0` = must have for the phase to be accepted; `P1` = should have; `P2` = nice to have, may be deferred to a later phase with a note in the PR.

### 1.2 Definitions

| Term | Meaning |
|---|---|
| **Layer** | Where a test runs: L1 Rust unit, L2 Rust integration, L3 remote backend, L4 Tauri command/IPC, L5 UI unit/component, L6 UI E2E with mocked backend, L7 full-stack desktop E2E, L8 installer/packaging, L9 non-functional. |
| **Backend** | A `FileSystem` implementation: `local`, `sftp`, `ftp`, `ftps` (explicit), `ftps-implicit`, `webdav`, `s3`, `wsl` (Windows only), and test fakes `mem` / `faulty`. |
| **Mock backend** | The in-browser fake of the Tauri commands used by L5/L6 ([§7.4](#74-ui-mock-backend)). |
| **Test servers** | The containers in `docker/test-servers` (OpenSSH, pyftpdlib, WsgiDAV, moto). |

---

## 2. System under test

### 2.1 Architecture

```
┌─────────────────────────── ui/ (Svelte 5 + TypeScript, Vite) ───────────────────────────┐
│ App.svelte (toolbar, keyboard map, dialogs)  Pane / Sidebar / Inspector / CompareView    │
│ MultiRename / CommandPalette / ConnectionDialog / OperationsPanel / Insights / About     │
│ lib/api.ts  ── invoke("<command>") / listen("<event>") ──┐                               │
└──────────────────────────────────────────────────────────┼───────────────────────────────┘
                                                           │ Tauri IPC
┌──────────────────────── src-tauri/ (Tauri 2 shell) ──────┼───────────────────────────────┐
│ lib.rs  setup(): builds AppState, forwards broadcast channels as events:                 │
│         operation-updated, edit-session-updated, live-sync-updated, compare-progress     │
│ commands.rs (fs, ops, favorites, text)  remote.rs (connections, keychain, edit)          │
│ tools.rs (compare/sync, rename, diff tools, insights, live sync)                         │
└──────────────────────────────────────────────────────────┼───────────────────────────────┘
                                                           │ plain Rust calls
┌──────────────────── crates/dualpal-core (no Tauri dependency) ───────────────────────────┐
│ vfs/  FileSystem trait + Registry; local, sftp (russh), ftp/ftps (suppaftp),             │
│       s3 + webdav (opendal), davput, wsl, posix, text                                    │
│ ops/  OperationQueue (copy, move, trash, delete, compress, setMode, createDir, rename,   │
│       batchRename, sync), progress broadcast, cancellation, conflicts, temp names, resume│
│ activity.rs  compare.rs  sync.rs  livesync.rs  rename.rs  connections.rs  remote_edit.rs │
│ places.rs  settings.rs  difftools.rs  error.rs                                           │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Features to cover (from README and code)

| Area | Features |
|---|---|
| Browsing | Two panes, tabs per pane, list/icon view, sort by name/size/date, folders first, hidden files toggle, breadcrumbs, history, sidebar (volumes, WSL distros on Windows, favorites, servers + nested favorites, saved syncs), free space, "go up to the nearest existing folder when a folder disappears". |
| Selection | Cursor, Space/Insert marking, Shift-arrows, Ctrl/⌘-A, Ctrl/⌘-click, Shift-click range, select/deselect by pattern (`*.jpg; *.png`). |
| Operations | Copy (F5), move (F6), new folder (F7), rename (F2), Multi Rename (Shift F2), trash (F8/Delete), permanent delete (Shift F8 / Shift Delete, confirmed), compress to ZIP, set permissions, quick text edit (Ctrl/⌘ S), conflicts fail/skip/overwrite, background queue (2 concurrent), progress, cancel, cleanup of partial files, activity log. |
| Mouse | Right-click menus (item and empty space), drag between panes / onto folders (copy; Shift = move; Esc cancels), drop from Explorer/Finder. |
| Remote | SFTP (password, key file ± passphrase, agent; `~/.ssh/config` hosts; known_hosts check; unknown → confirm fingerprint; changed → refuse; `user@host`; UTF-16 keys), FTP/FTPS explicit/implicit (self-signed fingerprint trust), WebDAV, S3 (endpoint, region, path-style). Secrets in OS keychain only. Temp-name uploads, parallel transfers (2 on FTP), resume after dropped connection (restart file on WebDAV/S3). Remote edit (F4) with upload on save, persisted unsent edits. |
| Compare & Sync | Compare by size+date / size / content, tolerance, excludes, hidden; modes Update →, Mirror →, Two-way ⇄; per-step overrides; deletion confirmation; snapshot history per pair; first two-way sync never deletes; conflicts left alone; S3 server-stamped times; Save as Favorite; Linked browsing; Compare Files in external diff tool. |
| Auto-sync | → / ← / ⇄ toolbar toggles with confirmation; local folders watched (notify), remote polled every 10 s; panes locked while running; status bar; Stop. |
| Power tools | Command palette, Multi Rename rules (replace/regex, case, insert with tokens `{name}` `{ext}` `{parent}` `{date}` `{n}`, number, remove, extension) + presets, activity insights. |
| App shell | About, Donate (QR + address), Email Us, open URL, editor choice, diff tool choice, settings persistence. |
| Packaging | Windows `.msi` + NSIS `-setup.exe`; Linux `.deb`, `.rpm`, `.AppImage`; macOS universal `.dmg`; release workflow on `v*` tag with version check. |

### 2.3 Persistent files (app data folder)

`favorites.json`, `connections.json` (no secrets), `settings.json`, `activity.jsonl`, `syncs.json`, `rename-presets.json`, `sync-state/` (two-way snapshots), `edits/` (unsent remote edits). Cache: `preview/` (cleared at start). These are the state an E2E test must isolate and may pre-seed.

---

## 3. Current test baseline

Run today with `cargo test --workspace` (Linux CI also starts the test servers and sets `DUALPAL_TEST_*`). There are **no UI tests, no Tauri command tests, no E2E tests and no installer checks**.

| File | Tests (existing) |
|---|---|
| `crates/dualpal-core/tests/ops.rs` | copies_a_folder_tree_with_progress_totals, copy_preserves_modification_time, conflict_policies, refuses_to_copy_a_folder_into_itself, refuses_to_copy_onto_itself, moves_within_the_same_disk, move_merge_keeps_skipped_sources, deletes_recursively, delete_does_not_follow_symlinked_folders, creates_and_renames, cancels_a_running_copy_and_cleans_up, operations_wait_for_a_free_slot, operation_json_shape, moves_items_to_trash, compresses_files_and_folders_into_a_zip, reads_and_saves_text, changes_permissions_and_saving_keeps_them |
| `tests/transfers.rs` | copies_several_files_at_once_without_leftovers, carries_on_after_a_dropped_connection, a_failed_copy_leaves_the_old_file_alone, overwriting_keeps_the_old_file_permissions, names_differing_only_in_case_are_copied_one_at_a_time |
| `tests/remote.rs` | s3_backend_works_like_a_disk (opendal memory), connections_keep_secrets_out_of_the_file, sftp_backend_against_a_real_server (host key flow, wrong password, perms/times, drop+resume), remote_edits_upload_on_save, s3_backend_against_a_real_server, unsent_edits_survive_failures_and_restarts, server_names_cannot_escape_local_folders, ftp/ftps/ftps_implicit/webdav_backend_against_a_real_server |
| `tests/sync.rs` | compare_classifies_every_item, two_way_sync_remembers_what_it_did, mirror_to_a_bucket_and_replace_a_file_with_a_folder, batch_rename_handles_swaps_and_refuses_clashes |
| `tests/livesync.rs` | one_way_copies_changes_as_they_happen, both_ways_copies_each_way_and_passes_on_deletions, the_same_folder_twice_is_refused, a_deletion_in_folders_that_started_equal_is_passed_on, switching_waits_for_the_old_sync_to_stop |
| `tests/activity.rs` | persists_newest_first_and_respects_capacity, insights_bucket_by_local_day |
| `tests/places.rs` | favorites_persist_add_remove_reorder, missing_favorites_file_starts_with_standard_folders, volumes_include_the_system_disk, wsl_names_hide_docker_and_keep_spaces |
| Unit (`#[cfg(test)]`) | rename.rs: replace_plain_and_regex, case_numbering_insert_remove_and_extension, tokens_and_folders, issues_are_reported, presets_round_trip · sync.rs: first_sync_uses_dates_and_never_deletes, snapshot_tells_deleted_from_new_and_who_changed, folders_on_one_side_are_one_step_with_totals, next_snapshot_records_matches_and_results · difftools.rs: templates_split_and_fill, env_vars_expand · compare.rs: globs · posix.rs: helpers · sftp.rs: parses_ssh_config, host_field_accepts_user_at_host, key_files_in_utf16_or_with_bom_are_read · wsl.rs: splits_wsl_paths, maps_linux_paths_back |

Known gaps this plan closes: (1) remote tests **silently pass** when an env var is missing; (2) no shared contract suite, so each backend is tested by a different script; (3) nothing tests the Tauri commands, the event wiring, or the UI; (4) installers are built but never installed or launched; (5) no coverage, flake tracking or performance budget.

---

## 4. Strategy and test layers

### 4.1 Pyramid and targets

| Layer | What | Runs on | Speed budget | Target count |
|---|---|---|---|---|
| **L1** Rust unit | Pure functions inside `dualpal-core` modules (`#[cfg(test)]`) | Linux, macOS, Windows | < 10 s | ~150 |
| **L2** Rust integration | `dualpal-core/tests/*.rs` on local disk + in-memory/faulty fakes | all 3 OS | < 60 s | ~200 |
| **L3** Remote backends | Same contract suite against the docker test servers | Linux (docker) | < 5 min | ~60 × 6 backends |
| **L4** Tauri command layer | `src-tauri` commands through `tauri::test` mock runtime: argument names, JSON shapes, error `{kind,message}`, events | all 3 OS | < 60 s | ~70 |
| **L5** UI unit/component | Vitest + Testing Library for `ui/src/lib/*.ts` and components | Linux (+ Windows weekly) | < 30 s | ~150 |
| **L6** UI E2E, mocked backend | Playwright drives the Vite build in Chromium/WebKit with an in-browser fake of the commands | Linux | < 4 min | ~120 |
| **L7** Full-stack desktop E2E | WebdriverIO + `tauri-driver` drives the real app binary on real disk and the test servers | Linux (xvfb) + Windows | < 15 min | ~30 smoke flows |
| **L8** Installer / packaging | Install, launch, verify, upgrade, uninstall each bundle | Win, Linux (+ macOS smoke) | < 20 min | ~30 |
| **L9** Non-functional | Performance benchmarks, security, accessibility, visual regression, soak | Linux nightly | nightly | as listed |

Why L6 exists in addition to L7: `tauri-driver` does not support macOS and is slow; the mocked browser layer exercises every keyboard shortcut, dialog and edge state quickly and deterministically on every PR. L7 then proves the real wiring with a small set of end-to-end flows.

### 4.2 Coverage goals

- `dualpal-core` line coverage ≥ **80 %** (measured with `cargo llvm-cov` on Linux with the test servers up); `ops/`, `sync.rs`, `compare.rs`, `rename.rs` ≥ **90 %**.
- `ui/src/lib/*.ts` line coverage ≥ **85 %** (Vitest v8 coverage). Components are covered by L6 rather than by a line-coverage number.
- Every `#[tauri::command]` in `src-tauri` has at least one L4 test (enforced by `CMD-000`, a test that lists the `generate_handler!` names and fails on a command without a test).
- Every keyboard shortcut in the README table has an L6 test (enforced by `UI-KEY-000`).

### 4.3 Risk-based priorities

Highest risk = data loss or security. Those cases are all `P0`: move/delete/trash semantics, mirror/two-way deletions, conflict handling, partial-file cleanup, temp-name uploads, resume integrity, path traversal from server names, host-key and TLS trust, secrets never written to disk/logs, auto-sync never deleting in one-way mode.

---

## 5. Tooling per layer

All versions are minimums; pin exact versions in `Cargo.toml` / `package.json` when adding them.

| Layer | Tool | Why |
|---|---|---|
| L1–L3 | `cargo test` (existing) + **`cargo-nextest`** ≥ 0.9 | Per-test process isolation, retries for flake detection (`--retries 2` only in the nightly job), JUnit output (`[profile.ci.junit]`). |
| L1–L2 | **`proptest`** 1.x | Property tests for path helpers, glob matching, rename rules, sync planning. |
| L1–L4 | **`insta`** 1.x (`json` feature) | Snapshot the JSON shape of everything the UI receives (`Operation`, `OperationInfo`, `Entry`, `SyncPreview`, errors) so a serde rename breaks a test, not the UI. |
| L2 | `tempfile` (existing), **`filetime`** 0.2 | Temp trees; set mtimes deterministically instead of sleeping. |
| L2 | **`rstest`** 0.2x | Parametrise the VFS contract suite over backends. |
| L3 | Docker Compose (existing `docker/test-servers`) + **Toxiproxy** 2.x (new container) | Real protocol servers; Toxiproxy injects latency, bandwidth limits, resets and timeouts for FTP/WebDAV/S3 (SFTP already has `drop-connections`). |
| L4 | **`tauri::test`** (enable `tauri` dev-dependency feature `test`) | `mock_builder()`, `mock_context(noop_assets())`, `get_ipc_response()` call real commands without a window. |
| Coverage | **`cargo-llvm-cov`** | Works with nextest; produces lcov for upload. |
| L5 | **Vitest** 3.x + **@testing-library/svelte** 5.x + **jsdom** + `@testing-library/user-event` + `@vitest/coverage-v8` | Native Vite config reuse; Svelte 5 runes compile through the same `@sveltejs/vite-plugin-svelte`. |
| L5–L6 | **`@tauri-apps/api/mocks`** (`mockIPC`, `mockWindows`, `clearMocks`) | Official way to fake `invoke`/`listen` in a browser. |
| L6 | **Playwright** 1.5x (`@playwright/test`) + **@axe-core/playwright** | Fast headless Chromium + WebKit (WebKit approximates macOS/Linux webviews; Chromium approximates WebView2 on Windows), traces, screenshots, accessibility scans. |
| L7 | **WebdriverIO** 9.x + **`tauri-driver`** (`cargo install tauri-driver --locked`) | Official Tauri 2 WebDriver path. Linux needs `webkit2gtk-driver` (`WebKitWebDriver`) + `xvfb`; Windows needs `msedgedriver` matching the installed WebView2. macOS is not supported → covered by L6 WebKit + L8 macOS smoke. |
| L8 | Shell/PowerShell scripts + Docker (`ubuntu:24.04`, `debian:12`, `fedora:41`, `opensuse/tumbleweed`) + GitHub Windows/macOS runners | Install/uninstall in clean environments. |
| L9 | **`criterion`** 0.5 (benches), `hyperfine` for start-up time, `cargo audit` + `cargo deny`, `npm audit --omit=dev`, Playwright screenshots | Performance budget, supply chain, visual regression. |

New dev dependencies, exactly:

```toml
# crates/dualpal-core/Cargo.toml  [dev-dependencies]
proptest = "1"
insta = { version = "1", features = ["json"] }
filetime = "0.2"
rstest = "0.25"
dualpal-testkit = { path = "../dualpal-testkit" }

# src-tauri/Cargo.toml
[dev-dependencies]
tauri = { version = "2", features = ["protocol-asset", "test"] }
insta = { version = "1", features = ["json"] }
tempfile.workspace = true
```

```jsonc
// package.json devDependencies to add
"vitest": "^3", "@vitest/coverage-v8": "^3", "jsdom": "^26",
"@testing-library/svelte": "^5", "@testing-library/user-event": "^14", "@testing-library/jest-dom": "^6",
"@playwright/test": "^1.50", "@axe-core/playwright": "^4",
"@wdio/cli": "^9", "@wdio/local-runner": "^9", "@wdio/mocha-framework": "^9", "@wdio/spec-reporter": "^9"
// scripts to add
"test:unit": "vitest run --coverage",
"test:e2e": "playwright test",
"test:e2e:update": "playwright test --update-snapshots",
"test:desktop": "wdio run e2e-desktop/wdio.conf.ts"
```

---

## 6. Testability changes to make first

These are the only production-code changes the plan needs. Do them in Phase 0 ([§10](#10-phased-implementation-order)). Each must keep the app's normal behaviour unchanged when the variables are not set.

| ID | Change | Where | Why |
|---|---|---|---|
| **PREP-01** | Move the body of `.setup(...)` into `pub fn build_state(dirs: AppDirs, secrets: Arc<dyn SecretStore>) -> std::io::Result<AppState>` with `pub struct AppDirs { data: PathBuf, cache: PathBuf }`. `setup` calls it and then spawns the three event-forwarding tasks via `pub fn spawn_event_forwarders(app: &AppHandle, state: &AppState)`. Also expose `pub fn handlers() -> impl Fn(Invoke) -> bool` (or a macro) so tests register the same `generate_handler!` list. | `src-tauri/src/lib.rs` | L4 tests build a real `AppState` on temp folders with in-memory secrets. |
| **PREP-02** | Environment overrides read once at start: `DUALPAL_DATA_DIR`, `DUALPAL_CACHE_DIR` (replace `app_data_dir()` / `app_cache_dir()` when set), `DUALPAL_SECRETS=memory` (use `MemorySecrets` instead of the keychain), `DUALPAL_HOME` (home folder used by `home_location`, `~/.ssh/config`, `known_hosts`, standard favorites). Log one line to stderr when any override is active. | `src-tauri/src/lib.rs`, `remote.rs`, `commands.rs`; `dualpal-core` `places.rs` / `vfs/sftp.rs` take the home path as a parameter or via a `fn home_dir()` helper that honours `DUALPAL_HOME`. | L7/L8 run the real binary without touching the user's profile or keychain (CI runners have no unlocked keychain). |
| **PREP-03** | `DUALPAL_LIVE_POLL_MS` (default 10000) for the remote auto-sync polling interval, and `DUALPAL_WATCH_DEBOUNCE_MS` for the local watcher debounce. | `crates/dualpal-core/src/livesync.rs` (prefer a field on `LiveSync` with a `with_poll_interval()` builder; the shell reads the env var). | Remote auto-sync tests would otherwise wait 10 s per cycle. |
| **PREP-04** | `DUALPAL_DIFF_TOOL_LOG=<file>` and `DUALPAL_OPENER_LOG=<file>`: when set, `difftools::launch`, `open_path`, `open_url`, `open_file` append the command line / URL / path to the file **instead of** launching anything. | `difftools.rs`, `commands.rs`, `remote.rs` | E2E can assert "Compare Files opened X and Y" and "Email Us opened mailto:" without spawning apps. |
| **PREP-05** | Add `data-testid` attributes (list below). Keep existing `aria-label`s; tests prefer role + accessible name and fall back to test ids. | `ui/src/**/*.svelte` | Stable selectors for L6/L7. |
| **PREP-06** | Expose `window.__DUALPAL_TEST__ = { ready: Promise<void> }` resolved after `App.svelte` finishes its first `listDir` for both panes — only when `import.meta.env.MODE === "test"` or `localStorage.dualpalTest === "1"`. | `ui/src/App.svelte` | Deterministic "app ready" signal. |
| **PREP-07** | Make remote tests **fail** instead of silently returning when `DUALPAL_REQUIRE_REMOTE=1` and a `DUALPAL_TEST_*` variable is missing. Helper `fn remote_env(var) -> Option<String>` in the test kit panics in that case. CI sets `DUALPAL_REQUIRE_REMOTE=1` on Linux. | `crates/dualpal-core/tests/remote.rs` → test kit | Today a broken compose setup turns the remote suite green. |

`data-testid` list for PREP-05 (use these exact names; `{side}` is `left` or `right`):

```
pane-{side}                 the pane root
pane-{side}-list            the listbox (list or icon view)
pane-{side}-row             each row/tile (also set data-name="<entry name>")
pane-{side}-crumbs          breadcrumb nav
pane-{side}-tab             each tab; pane-{side}-tab-add; pane-{side}-tab-close
pane-{side}-status          status line (counts, free space)
pane-{side}-error           error banner
sidebar  sidebar-volume  sidebar-favorite  sidebar-server  sidebar-server-favorite  sidebar-sync
toolbar  (buttons keep their aria-labels: Copy, Move, New Folder, Rename, …)
dialog                      Dialog.svelte root (+ data-kind="confirm|prompt|connection|rename|pattern|…")
dialog-ok  dialog-cancel  dialog-input
context-menu  context-menu-item (data-action="<action>")
ops-panel  ops-item (data-status="<status>")  ops-cancel  ops-clear  activity-item
inspector  inspector-preview  inspector-editor  inspector-save
compare-view  compare-row (data-rel="<relative path>" data-state="<DiffState>")  compare-action  compare-run  compare-mode
multi-rename  multi-rename-rule  multi-rename-preview-row  multi-rename-apply
palette  palette-input  palette-item
connection-dialog  connection-protocol  connection-save  connection-test
live-bar  live-stop
about-dialog  donate-address  donate-qr
edit-sessions  edit-session-item
```

---

## 7. Fixtures and test data

### 7.1 Standard local tree (`fixture::standard_tree(root)`)

Created by the test kit; every L2/L3/L7 test that needs "a realistic folder" uses it. Content is deterministic (bytes derived from a seed), mtimes are set explicitly to `BASE_TIME = 2024-01-02T03:04:05Z` plus the offset shown.

```
root/
  docs/
    readme.md                 "# Readme\n" (text, +0 s)
    report.pdf                2 KiB pseudo-random (+60 s)
    notes.txt                 "alpha\nbeta\n" (+120 s)
  photos/
    a.jpg  b.jpg  c.png       64 KiB each, valid tiny images (copy from ui/src/assets/app-icon.png for png)
    .DS_Store                 excluded by default compare options
  nested/l1/l2/l3/l4/deep.txt "deep"
  empty-dir/
  empty-file.txt              0 bytes
  big.bin                     8 MiB pseudo-random (stream-copy, progress, cancel)
  Space Name.txt              spaces
  ünïcødé-文件.txt             Unicode NFC name
  Case.txt and case.txt       (only on case-sensitive file systems; the kit skips the second on macOS/Windows)
  .hidden-file                hidden by name
  readonly.txt                mode 0444 / read-only attribute
  link-to-docs -> docs        symlink (Unix; Windows only when symlink creation is allowed)
  script.sh                   mode 0755
```

Variants: `small_tree` (3 files, 1 folder) for fast tests; `many_files(n)` (n flat files, default 10 000) for performance; `name_torture()` (names with `#`, `%`, `&`, `'`, `[]`, emoji, 255-byte names, trailing dots/spaces on non-Windows, leading `-`).

### 7.2 Rust test kit crate

Create `crates/dualpal-testkit` (`publish = false`, workspace member, dev-dependency of `dualpal-core` and `src-tauri`). Move shared helpers out of the existing test files into it.

```
crates/dualpal-testkit/src/
  lib.rs          re-exports
  fixture.rs      standard_tree, small_tree, many_files, name_torture, set_mtime, write, read
  queue.rs        fn queue(registry) -> OperationQueue; async fn run(q, op) -> OperationInfo (waits for terminal status);
                  fn collect_events(q) -> EventRecorder (records every OperationInfo broadcast)
  eventually.rs   async fn eventually(what: &str, timeout: Duration, cond: impl FnMut() -> bool)
  memfs.rs        MemFs: in-memory FileSystem (POSIX-style paths, mtimes, modes, hidden by dot) — fast backend for contract tests
  faulty.rs       FaultyFs<F: FileSystem>: wraps any FS; inject per-call failures:
                    fail_write_after(bytes), fail_next(op, Error), latency(op, Duration),
                    count_open_writers(), most_parallel_writers(), resumed_offsets()
                  (generalises the `Remote`/`Flaky` types in tests/transfers.rs, which should then use it)
  remote.rs       fn remote_env(var) -> Option<String> (honours DUALPAL_REQUIRE_REMOTE, PREP-07);
                  fn sftp_from_env(), ftp_from_env(var, security), webdav_from_env(), s3_from_env() -> Option<Arc<dyn FileSystem>>;
                  fn unique_base(fs) -> String  (a fresh folder like /dualpal-test/<uuid> per test, removed on Drop)
  contract.rs     the VFS contract suite as `pub async fn run_contract(fs: Arc<dyn FileSystem>, base: &str, caps: Caps)`
                  plus one `pub async fn` per case so backends can be parametrised with rstest
  secrets.rs      re-export MemorySecrets; FailingSecrets (every call errors) for error-path tests
```

`Caps` describes what a backend supports, so the contract skips (and logs) cases that do not apply: `{ modes: bool, set_mtime: bool, symlinks: bool, trash: bool, resumable: bool, atomic_writes: bool, empty_dirs: bool, case_sensitive: bool }`. Expected values: local (all true except trash on CI Linux without a desktop → test with `trash` crate's freedesktop implementation in a temp `XDG_DATA_HOME`), sftp (modes, set_mtime, symlinks, resumable), ftp/ftps (resumable; set_mtime depends on MFMT support — pyftpdlib supports it), webdav (none of modes/symlinks; set_mtime false), s3 (atomic_writes; empty_dirs emulated by marker keys; no modes; mtimes server-stamped), mem (all except trash/symlinks).

### 7.3 Remote test servers

Use the existing `docker/test-servers/compose.yaml`. Add:

1. **Healthchecks** on every service so `docker compose up --wait` replaces the log-grepping loops in `ci.yml` Use only tools already in each image (all are Alpine-based; `curl` is not installed): sftp `nc -z 127.0.0.1 22` (busybox); ftp and webdav a `python -c` socket connect to 2121 / 8088; s3 `wget -q -O /dev/null http://127.0.0.1:9000/dualpal-test` (busybox wget; succeeds only once the bucket exists, so it also replaces the `bucket dualpal-test ready` log wait).
2. **Toxiproxy** service (`ghcr.io/shopify/toxiproxy:2.9.0`, API on `127.0.0.1:8474`) with proxies: `ftp-proxy 127.0.0.1:3121 → ftp:2121`, `webdav-proxy 127.0.0.1:9088 → webdav:8088`, `s3-proxy 127.0.0.1:9900 → s3:9000`. The test kit gets `toxi::reset_peer(proxy, after_bytes)`, `toxi::latency(proxy, ms)`, `toxi::bandwidth(proxy, kbps)`, `toxi::clear()` (plain HTTP calls with `reqwest`). Env: `DUALPAL_TEST_TOXIPROXY=http://127.0.0.1:8474`. Any test that drops SFTP sessions or adds a toxic affects every client of that server, so its function name must start with `fault_` and it must remove its toxics in a drop guard; CI runs `fault_*` tests on their own with one thread (§9.2). The existing `upload_survives_a_dropped_connection` path inside `sftp_backend_against_a_real_server` should move into its own `fault_sftp_upload_survives_a_dropped_connection` test in Phase 2. (FTP passive data ports cannot all be proxied cleanly — only test control-channel faults on FTP, and data-channel faults via `FaultyFs`.)
3. A second SFTP user `keyonly` (no password, key with passphrase `dualpal-pass`) and a key file saved as UTF-16 LE with BOM in `keys/` to cover passphrase and UTF-16 key flows (`DUALPAL_TEST_SFTP_PASSPHRASE_KEY`).
4. A WebDAV path with a space and Unicode (`/dav/Shared Folder/ünï`) pre-created.
5. A second S3 bucket `dualpal-test-virtual` for virtual-host-style addressing tests (moto supports both).

### 7.4 UI mock backend

`ui/tests/mock-backend/` — a TypeScript in-memory implementation of every command in `lib/api.ts`, installed with `mockIPC` before the app loads (L5) or injected via Playwright `page.addInitScript` (L6).

```
ui/tests/mock-backend/
  index.ts        installMockBackend(scenario: Scenario): MockHandle
  fs.ts           MemTree: Map<fs, Map<path, Entry & {content?: string}>>, POSIX + Windows path flavours
  commands.ts     handlers keyed by command name; unknown command → throw {kind:"unsupported", message}
  events.ts       emit("operation-updated" | "edit-session-updated" | "live-sync-updated" | "compare-progress", payload)
  ops.ts          enqueue → emits queued → running(progress…) → completed/failed; honours conflict policy; cancel
  scenarios.ts    named scenarios: "default" (standard tree in both panes), "empty", "windows-paths",
                  "many-files-5000", "server-connected", "compare-differences", "slow-ops", "errors"
  recorder.ts     records every invoke (name, args) so tests assert e.g. expect(calls("enqueue_operation")).toContainEqual(...)
```

`MockHandle` API used by tests: `calls(name)`, `lastCall(name)`, `failNext(name, {kind, message})`, `delayNext(name, ms)`, `setTree(fs, entries)`, `emit(event, payload)`, `completeAll()`, `removeFolder(path)` (to test "folder disappears").

Rule: the mock must return the **same JSON shapes** as the Rust side. Generate a fixtures file from L4 snapshots (`src-tauri/tests/snapshots/*.snap` → `ui/tests/mock-backend/shapes.json` via `npm run gen:shapes`) and add test `UI-MOCK-001` that validates every mock response against those shapes, so the mock cannot drift.

### 7.5 Full-stack E2E environment (L7)

Per test file (WebdriverIO `beforeEach` of the spec):

```
tmp=$(mktemp -d)
DUALPAL_DATA_DIR=$tmp/data  DUALPAL_CACHE_DIR=$tmp/cache  DUALPAL_HOME=$tmp/home
DUALPAL_SECRETS=memory  DUALPAL_LIVE_POLL_MS=500
DUALPAL_DIFF_TOOL_LOG=$tmp/diff.log  DUALPAL_OPENER_LOG=$tmp/open.log
$tmp/home/  ← standard_tree under $tmp/home/work/{left,right}
$tmp/data/favorites.json ← seeded so the sidebar has "Work Left", "Work Right"
$tmp/home/.ssh/config ← Host dualpal-test (127.0.0.1:2222, user tester)
```

A small Rust binary `crates/dualpal-testkit/src/bin/make-fixture.rs` (`cargo run -p dualpal-testkit --bin make-fixture -- <dir> [standard|small|many N]`) creates trees so the TypeScript E2E code and shell scripts reuse the same generator.

---

## 8. Test case catalogue

Format: **ID** · title · layer · priority · status (`NEW` / `EXISTS: <test fn>` / `EXTEND: <test fn>`) · steps and assertions. "Assert" lists what must be checked; implementers may add more.

### 8.1 VFS contract suite (run against every backend) — `VFS-CON`

Implemented once in `dualpal-testkit/src/contract.rs`, invoked from:
- `crates/dualpal-core/tests/contract_local.rs` (local, all OS, L2)
- `crates/dualpal-core/tests/contract_mem.rs` (MemFs and opendal memory S3, L2)
- `crates/dualpal-core/tests/contract_remote.rs` (sftp, ftp, ftps, ftps-implicit, webdav, s3; L3; one `#[rstest]` case per backend; replaces the per-backend `exercise`/`ftp_round_trip` bodies in `remote.rs`, which keep only backend-specific checks)

Each case runs in `unique_base(fs)`.

| ID | Title | P | Status | Steps → Assert |
|---|---|---|---|---|
| VFS-CON-001 | list empty folder | P0 | EXTEND: remote.rs `exercise` | create_dir → list is empty |
| VFS-CON-002 | create, list, stat file | P0 | EXTEND | write 5 bytes → list has 1 entry; stat kind File, size 5, name/path consistent with `join` |
| VFS-CON-003 | stat folder is_dir | P0 | EXTEND | |
| VFS-CON-004 | stat missing → `notFound` | P0 | EXTEND | error kind `notFound` (not `io`) for missing file and missing parent |
| VFS-CON-005 | exists true/false | P1 | NEW | |
| VFS-CON-006 | read back exact bytes (0 B, 1 B, 8 MiB random) | P0 | NEW | sha256 equal |
| VFS-CON-007 | open_write truncates existing | P0 | EXTEND | write "long content" then "x" → reads "x" |
| VFS-CON-008 | create_dir on existing → `alreadyExists` | P1 | NEW | |
| VFS-CON-009 | create_dir with missing parent | P1 | NEW | documents behaviour: error `notFound` or creates; assert per Caps and make consistent across backends (raise a bug if not) |
| VFS-CON-010 | remove_file | P0 | EXTEND | gone from list; stat notFound |
| VFS-CON-011 | remove_dir on empty | P0 | EXTEND | |
| VFS-CON-012 | remove_dir on non-empty fails, nothing deleted | P0 | NEW | error; child still present |
| VFS-CON-013 | rename file within folder | P0 | EXTEND | |
| VFS-CON-014 | rename folder with contents | P0 | EXTEND | all children reachable under new name |
| VFS-CON-015 | rename onto existing name | P0 | NEW | documents behaviour (error or replace) per backend; the ops layer must never rely on overwrite-by-rename (see OPS-COPY-020) |
| VFS-CON-016 | hidden detection | P1 | EXTEND | `.hidden` → `hidden: true` |
| VFS-CON-017 | modified_ms is set and near now (±5 min) after write | P1 | NEW | skip when backend stamps none |
| VFS-CON-018 | set_modified round trip within 2 s | P1 | EXTEND (sftp only today) | Caps.set_mtime |
| VFS-CON-019 | set_mode round trip | P1 | EXTEND (sftp only today) | Caps.modes; other backends → `unsupported` |
| VFS-CON-020 | open_read_at offset | P0 | NEW | read from 3 of "abcdef" = "def"; offset > len → `invalid` |
| VFS-CON-021 | open_write_at resumes | P0 | NEW | Caps.resumable: write first half, open_write_at(half), write rest → full sha matches; otherwise `unsupported` |
| VFS-CON-022 | name torture | P0 | NEW | every name from `name_torture()` the backend allows: create, list shows exact name, read, rename, delete |
| VFS-CON-023 | Unicode NFC/NFD | P1 | NEW | create NFC name; list returns a name that `stat` accepts (macOS local may normalise) |
| VFS-CON-024 | join/parent/file_name/is_within laws (proptest) | P0 | NEW | `parent(join(d,n)) == d`, `file_name(join(d,n)) == n`, `is_within(join(d,n), d)`, `!is_within(d, join(d,n))`, `is_within("/a/bc","/a/b") == false` |
| VFS-CON-025 | listing 1 000 entries complete | P1 | NEW | count exact (pagination on S3/WebDAV) |
| VFS-CON-026 | concurrent writers | P1 | NEW | `parallel_transfers()` writers at once, all files correct |
| VFS-CON-027 | symlink reported | P2 | NEW | Caps.symlinks: `symlink: true`, kind = target kind; dangling link → kind Other, no error listing parent |
| VFS-CON-028 | empty folders survive | P1 | NEW | S3 marker semantics: create empty dir, list parent shows it, remove_dir removes it |
| VFS-CON-029 | display_name and id stable | P2 | NEW | |
| VFS-CON-030 | errors serialise as `{kind, message}` | P0 | NEW | for each Error variant `serde_json::to_value` → snapshot (insta) |

### 8.2 Local backend and path helpers — `VFS-LOC`, `VFS-PATH`, `VFS-WSL`, `VFS-TXT`

| ID | Title | Layer | P | Status | Steps → Assert |
|---|---|---|---|---|---|
| VFS-LOC-001 | readonly flag reflects permissions | L2 | P1 | NEW | `readonly.txt` → `readonly: true` |
| VFS-LOC-002 | Windows hidden attribute counts as hidden | L2 (Windows) | P1 | NEW | `attrib +h` file → hidden |
| VFS-LOC-003 | trash moves to OS trash | L2 | P1 | EXISTS: ops.rs moves_items_to_trash | |
| VFS-LOC-004 | permission denied maps to `permissionDenied` | L2 (Unix) | P1 | NEW | chmod 000 dir → list error kind |
| VFS-LOC-005 | Windows drive roots and UNC paths | L2 (Windows) | P1 | NEW | `parent("C:\\")` is None; `join("C:\\", "x")`; `\\?\` long paths > 260 chars list/copy |
| VFS-LOC-006 | case-insensitive FS: rename `a.txt` → `A.txt` | L2 (Win/mac) | P0 | NEW | rename succeeds and listing shows `A.txt` |
| VFS-PATH-001 | `is_safe_local_name` table | L1 | P0 | EXTEND: remote.rs server_names_cannot_escape_local_folders | `""`, `.`, `..`, `a/b`, `a\0b` false; Windows `a\b`, `C:x` false; normal true |
| VFS-PATH-002 | `safe_local_name` never unsafe (proptest) | L1 | P0 | NEW | for any string s: `is_safe_local_name(safe_local_name(s))` |
| VFS-PATH-003 | posix helpers | L1 | P1 | EXISTS: posix.rs helpers | |
| VFS-WSL-001 | split/map WSL paths | L1 | P1 | EXISTS: wsl.rs tests | |
| VFS-WSL-002 | WSL distros listed on Windows runner with WSL | L2 (Windows, optional) | P2 | NEW | skip when `wsl.exe -l -q` fails |
| VFS-TXT-001 | read_text truncates at max_bytes | L1 | P0 | EXTEND: ops.rs reads_and_saves_text | `truncated: true`, text length ≤ max |
| VFS-TXT-002 | binary detection (NUL byte) | L1 | P0 | NEW | `binary: true` |
| VFS-TXT-003 | invalid UTF-8 / UTF-16 BOM / CRLF preserved | L1 | P1 | NEW | write_text of read_text output is byte-identical for CRLF files |
| VFS-TXT-004 | truncation never splits a UTF-8 char | L1 | P1 | NEW | |
| VFS-TXT-005 | write_text keeps mode | L2 | P1 | EXISTS: ops.rs changes_permissions_and_saving_keeps_them | |

### 8.3 Operation queue — `OPS`

All in `crates/dualpal-core/tests/ops.rs` / `transfers.rs` (local + FaultyFs) unless noted. Cases marked *(×remote)* also run in `contract_remote.rs` through the queue.

**Copy**

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| OPS-COPY-001 | copy tree with progress totals | P0 | EXISTS: copies_a_folder_tree_with_progress_totals | |
| OPS-COPY-002 | progress events are monotonic | P0 | NEW | recorded `OperationInfo` stream: status queued→running→completed; `bytes_done`, `items_done` never decrease; final equals totals; `current` names a real file |
| OPS-COPY-003 | preserves mtime | P0 | EXISTS: copy_preserves_modification_time | (×remote where Caps.set_mtime) |
| OPS-COPY-004 | preserves mode bits | P1 | NEW | `script.sh` stays 0755 (Unix, sftp) |
| OPS-COPY-005 | conflict fail / skip / overwrite | P0 | EXISTS: conflict_policies | (×remote) |
| OPS-COPY-006 | conflict fail stops before any write | P0 | NEW | with 3 sources where 2nd conflicts: 1st copied, 2nd untouched, 3rd not copied; error kind `alreadyExists`; no temp files |
| OPS-COPY-007 | folder into itself refused | P0 | EXISTS: refuses_to_copy_a_folder_into_itself | |
| OPS-COPY-008 | onto itself refused | P0 | EXISTS: refuses_to_copy_onto_itself | |
| OPS-COPY-009 | folder merge into existing folder | P0 | NEW | existing dest folder with extra file: extra kept, new files added, conflicts per policy |
| OPS-COPY-010 | file over folder / folder over file | P0 | NEW | both directions error `alreadyExists` with fail; overwrite replaces correctly or errors clearly — never deletes the other kind silently with `skip` |
| OPS-COPY-011 | symlink copied as link, not followed into loop | P1 | NEW | `link-to-docs` + a link to its own parent: copy terminates |
| OPS-COPY-012 | empty file and empty folder copied | P0 | NEW | |
| OPS-COPY-013 | cancel mid-copy cleans partial file | P0 | EXISTS: cancels_a_running_copy_and_cleans_up | (×remote) |
| OPS-COPY-014 | remote target uses temp name then renames | P0 | EXISTS: copies_several_files_at_once_without_leftovers | on a backend without `atomic_writes`, the **final** name never appears until its bytes are complete (FaultyFs snapshots the listing while writing: the final path is either absent or has the full size and sha), and no temporary file remains after completion, failure or cancellation. Temporary names *during* the write are expected and must not fail the test. On `atomic_writes` backends (S3) no temporary name is used at all |
| OPS-COPY-015 | parallel transfers respect `parallel_transfers()` | P0 | EXTEND: copies_several_files_at_once_without_leftovers | max concurrent writers == limit (2 for FTP-like fake, N for SFTP-like) |
| OPS-COPY-016 | resume after drop | P0 | EXISTS: carries_on_after_a_dropped_connection | resumed offset recorded; final sha equal (×sftp via drop-connections, ×ftp via toxiproxy reset) |
| OPS-COPY-017 | non-resumable backend restarts the file | P0 | NEW | Faulty wrapper with `resumable_writes=false`, `atomic_writes=false`: retried from 0, final sha equal (×webdav, ×s3 via toxiproxy) |
| OPS-COPY-018 | failed copy leaves old file | P0 | EXISTS: a_failed_copy_leaves_the_old_file_alone | |
| OPS-COPY-019 | overwrite keeps old permissions | P1 | EXISTS: overwriting_keeps_the_old_file_permissions | |
| OPS-COPY-020 | overwrite on backend whose rename does not replace | P0 | NEW | fake FS where rename onto existing errors: overwrite still succeeds, old file never lost |
| OPS-COPY-021 | case-only differing names copied one at a time | P1 | EXISTS: names_differing_only_in_case_are_copied_one_at_a_time | |
| OPS-COPY-022 | persistent failure (disk full) reports failed, no retry loop | P0 | NEW | FaultyFs `broken`: status failed within 5 s, error kind io, partial removed |
| OPS-COPY-023 | cross-FS copy local→remote→remote→local | P0 | EXTEND: remote.rs exercise | server→server between two different backends (sftp→s3, ftp→webdav) sha equal |
| OPS-COPY-024 | server file names unsafe for local disk | P0 | EXTEND: server_names_cannot_escape_local_folders | remote names `..`, `a\b`, `con`(Windows) are sanitised; nothing written outside destination |
| OPS-COPY-025 | 8 MiB stream does not buffer whole file | P2 | NEW | peak RSS delta < 32 MiB for 256 MiB copy (nightly) |

**Move / delete / trash / other**

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| OPS-MOVE-001 | move within same disk is a rename | P0 | EXISTS: moves_within_the_same_disk | |
| OPS-MOVE-002 | move across FS = copy then delete source | P0 | NEW | local→MemFs: source gone only after all bytes verified; failure mid-way leaves source intact |
| OPS-MOVE-003 | merge keeps skipped sources | P0 | EXISTS: move_merge_keeps_skipped_sources | |
| OPS-MOVE-004 | cancel during cross-FS move keeps source | P0 | NEW | |
| OPS-DEL-001 | recursive delete | P0 | EXISTS: deletes_recursively | (×remote) |
| OPS-DEL-002 | delete does not follow symlinked folders | P0 | EXISTS: delete_does_not_follow_symlinked_folders | |
| OPS-DEL-003 | partial failure reports and keeps the rest consistent | P1 | NEW | read-only subfolder: status failed, error names the path, other targets deleted |
| OPS-TRASH-001 | trash local | P1 | EXISTS: moves_items_to_trash | |
| OPS-TRASH-002 | trash on remote → `unsupported`, nothing removed | P0 | NEW | |
| OPS-ZIP-001 | compress files and folders | P1 | EXISTS: compresses_files_and_folders_into_a_zip | |
| OPS-ZIP-002 | compress Unicode names, empty folders, 0-byte files | P1 | NEW | unzip with `async_zip` and compare tree |
| OPS-ZIP-003 | compress remote sources into local zip | P2 | NEW (×remote) | |
| OPS-ZIP-004 | destination exists → `alreadyExists` | P1 | NEW | |
| OPS-MODE-001 | set mode | P1 | EXISTS: changes_permissions_and_saving_keeps_them | |
| OPS-DIR-001 | create dir and rename | P0 | EXISTS: creates_and_renames | |
| OPS-REN-001 | rename to existing name refused | P0 | NEW | error `alreadyExists`, both files intact |
| OPS-REN-002 | rename to invalid name (`a/b`, empty) → `invalid` | P0 | NEW | |
| OPS-BREN-001 | batch rename swaps and refuses clashes | P0 | EXISTS: sync.rs batch_rename_handles_swaps_and_refuses_clashes | |
| OPS-Q-001 | waits for a free slot (max 2) | P0 | EXISTS: operations_wait_for_a_free_slot | |
| OPS-Q-002 | cancel a queued (not yet running) op | P0 | NEW | status cancelled, never ran |
| OPS-Q-003 | cancel unknown id → false | P2 | NEW | |
| OPS-Q-004 | clear finished keeps running/queued | P1 | NEW | |
| OPS-Q-005 | every finished op writes one activity record | P0 | NEW | completed, failed, cancelled each recorded with kind, bytes, items, error |
| OPS-Q-006 | operation JSON shape | P0 | EXISTS: operation_json_shape | convert to insta snapshot of every variant |
| OPS-Q-007 | OperationInfo JSON shape snapshot | P0 | NEW | insta |
| OPS-Q-008 | 50 ops enqueued at once all finish | P1 | NEW | no deadlock, ≤ 2 running at any moment |

### 8.4 Activity, places, settings — `ACT`, `PLC`, `SET`

| ID | Title | Layer | P | Status | Assert |
|---|---|---|---|---|---|
| ACT-001 | persists newest first, capacity | L2 | P0 | EXISTS: persists_newest_first_and_respects_capacity | |
| ACT-002 | insights bucket by local day | L2 | P1 | EXISTS: insights_bucket_by_local_day | |
| ACT-003 | corrupt/truncated line in activity.jsonl is skipped | L2 | P0 | NEW | file with a half-written last line opens; valid records kept |
| ACT-004 | clear empties file and memory | L2 | P1 | NEW | |
| ACT-005 | insights: by kind, by place, speed, largest, recent failures | L2 | P1 | NEW | handcrafted records → exact numbers; DST boundary with utc offset change |
| ACT-006 | unwritable log path → falls back to memory (shell) | L4 | P2 | NEW | |
| PLC-001 | favorites add/remove/reorder persist | L2 | P0 | EXISTS: favorites_persist_add_remove_reorder | |
| PLC-002 | missing favorites file → standard folders | L2 | P1 | EXISTS | |
| PLC-003 | corrupt favorites.json → standard folders, file not overwritten until a change | L2 | P1 | NEW | |
| PLC-004 | reorder out of range → error, list unchanged | L2 | P2 | NEW | |
| PLC-005 | volumes include system disk | L2 | P1 | EXISTS | |
| PLC-006 | available_space returns Some for home | L2 | P2 | NEW | |
| PLC-007 | WSL names | L1 | P2 | EXISTS | |
| SET-001 | defaults when file missing | L2 | P1 | NEW | |
| SET-002 | round trip editor + diff_tool | L2 | P1 | NEW | |
| SET-003 | corrupt settings.json → defaults | L2 | P1 | NEW | |
| SET-004 | unknown fields ignored (forward compatible) | L2 | P2 | NEW | |

### 8.5 Connections, secrets, sign-in — `CON`

| ID | Title | Layer | P | Status | Assert |
|---|---|---|---|---|---|
| CON-001 | secrets never written to connections.json | L2 | P0 | EXISTS: connections_keep_secrets_out_of_the_file | extend to all protocols (sftp password, key passphrase, ftp, webdav, s3 secret key): grep file for the secret |
| CON-002 | save/list/get/remove/reorder persist | L2 | P0 | NEW | |
| CON-003 | removing a connection removes its secret | L2 | P0 | NEW | MemorySecrets empty afterwards |
| CON-004 | add/remove server favorites | L2 | P1 | NEW | |
| CON-005 | `fs_id` stable and unique | L1 | P1 | NEW | |
| CON-006 | `uses_secret` per auth method | L1 | P1 | NEW | password yes; agent no; key file yes only when passphrase |
| CON-007 | Endpoint JSON shape per protocol | L1 | P0 | NEW | insta snapshot; matches `Endpoint` type in api.ts (see UI-MOCK-001) |
| CON-008 | secret store failure surfaces an error, no plaintext fallback | L2 | P0 | NEW | FailingSecrets: save returns error; file has no secret |
| CON-009 | SSH unknown host → `UnknownHost` with fingerprint; trust wrong fp refused; right fp learned | L3 | P0 | EXISTS: sftp_backend_against_a_real_server | uses `known_hosts` in temp HOME (PREP-02) |
| CON-010 | SSH changed host key → `HostKeyChanged`, no connection | L3 | P0 | NEW | write a different key for `[127.0.0.1]:2222` into temp known_hosts |
| CON-011 | wrong password → `authFailed` | L3 | P0 | EXISTS | |
| CON-012 | key file auth | L3 | P0 | EXISTS | |
| CON-013 | key with passphrase: right / wrong / missing | L3 | P0 | NEW | uses `keyonly` user (§7.3) |
| CON-014 | UTF-16 key file | L1/L3 | P1 | EXISTS: key_files_in_utf16_or_with_bom_are_read | add L3 sign-in with that file |
| CON-015 | agent auth | L3 | P2 | NEW | start `ssh-agent` in test, `ssh-add` the key, connect with agent method |
| CON-016 | ssh config hosts parsed, `user@host` accepted | L1 | P1 | EXISTS: parses_ssh_config, host_field_accepts_user_at_host | add `Include`, wildcard `Host *`, `IdentityFile ~` expansion |
| CON-017 | connection refused / wrong port → `connection` kind within 10 s | L3 | P0 | NEW | every protocol |
| CON-018 | FTPS self-signed: first connect asks with fingerprint; trust remembered; changed cert refused | L3 | P0 | EXTEND: ftps_backend_against_a_real_server | |
| CON-019 | FTPS implicit on 990-style port | L3 | P1 | EXISTS: ftps_implicit_backend_against_a_real_server | |
| CON-020 | FTP anonymous (empty user) | L3 | P2 | NEW | enable anonymous read-only folder in pyftpdlib |
| CON-021 | WebDAV wrong password → `authFailed` | L3 | P0 | NEW | |
| CON-022 | S3 wrong secret → `authFailed`; wrong bucket → `notFound` | L3 | P0 | NEW | moto accepts any secret (see the note in `remote.rs` s3_backend_against_a_real_server), so the wrong-bucket half runs against moto, and the wrong-secret half needs a server that checks signatures: add a `minio` service (`minio/minio`, `127.0.0.1:9100`, `DUALPAL_TEST_S3_AUTH=http://127.0.0.1:9100\|dualpal-auth\|minio\|minio-secret`) used only by this case. Without that variable the wrong-secret half is skipped with a note, not passed |
| CON-023 | S3 path-style vs virtual-host style | L3 | P1 | NEW | |
| CON-024 | connect timeout via toxiproxy latency 30 s → error, not hang | L3 | P1 | NEW | |
| CON-025 | disconnect unregisters FS and stops edit sessions on it | L4 | P0 | NEW | `registry.ids()` no longer contains it; editor sessions on that fs stopped |

### 8.6 Remote edit — `EDIT`

| ID | Title | Layer | P | Status | Assert |
|---|---|---|---|---|---|
| EDIT-001 | edit uploads on save | L2/L3 | P0 | EXISTS: remote_edits_upload_on_save | |
| EDIT-002 | unsent edits survive failure and restart | L2 | P0 | EXISTS: unsent_edits_survive_failures_and_restarts | |
| EDIT-003 | status transitions watching→uploading→uploaded, events broadcast | L2 | P0 | NEW | subscribe; recorded sequence |
| EDIT-004 | rapid successive saves coalesce, last content wins | L2 | P1 | NEW | 5 writes in 200 ms → server has final content; ≤ 2 uploads |
| EDIT-005 | stop session stops uploading | L2 | P1 | NEW | |
| EDIT-006 | upload_now forces upload | L2 | P1 | NEW | |
| EDIT-007 | remote file changed meanwhile → no silent overwrite (document behaviour) | L2 | P1 | NEW | if the product overwrites, mark as `BUG` per rule 6 and raise with the owner |
| EDIT-008 | local edit copies isolated per server/path (same name on two servers) | L2 | P1 | NEW | |
| EDIT-009 | editor path with spaces launched correctly | L4 | P2 | NEW | via PREP-04 opener log |

### 8.7 Compare — `CMP`

`crates/dualpal-core/tests/compare.rs` (new file; move the compare part of `sync.rs` here or reference it).

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| CMP-001 | classifies every DiffState | P0 | EXISTS: sync.rs compare_classifies_every_item | |
| CMP-002 | `by: size` ignores dates for equality but reports newer side | P0 | NEW | |
| CMP-003 | `by: content` reads only when sizes match; equal-size different bytes → `different` | P0 | NEW | count reads with FaultyFs |
| CMP-004 | tolerance_ms: 1999 ms equal, 2001 ms not (default 2000) | P0 | NEW | set mtimes with filetime |
| CMP-005 | default excludes (.DS_Store, Thumbs.db, desktop.ini, `._*`) | P0 | NEW | |
| CMP-006 | custom exclude patterns, case-insensitive, `?` wildcard | P1 | EXTEND: compare.rs globs | proptest vs reference regex implementation |
| CMP-007 | include_hidden false hides dot files and hidden attr | P1 | NEW | |
| CMP-008 | kindMismatch file vs folder | P0 | NEW | |
| CMP-009 | nested only-on-one-side folder reported once with totals | P0 | EXISTS: folders_on_one_side_are_one_step_with_totals | |
| CMP-010 | cancel compare stops quickly | P0 | NEW | `many_files(20000)`: cancel token → returns `Cancelled` < 2 s |
| CMP-011 | progress callback counts folders | P1 | NEW | |
| CMP-012 | compare local vs each remote backend | P0 | NEW (L3) | identical trees → all `same` (S3: server-stamped times handled) |
| CMP-013 | case-insensitive local vs case-sensitive remote names | P1 | NEW | `Case.txt` vs `case.txt` handling documented |

### 8.8 Sync — `SYNC`

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| SYNC-001 | plan matrix: mode × DiffState → action | P0 | NEW | Table-driven over all 3 modes × 7 states × {no snapshot, snapshot says unchanged-left, unchanged-right, both changed}: expected `SyncAction` + `SyncReason`. Write the expected table in the test file as data; it is the spec. Update never deletes; Mirror deletes right-only; Two-way follows snapshot. |
| SYNC-002 | first two-way sync never deletes | P0 | EXISTS: first_sync_uses_dates_and_never_deletes | |
| SYNC-003 | snapshot distinguishes deleted vs new, who changed | P0 | EXISTS: snapshot_tells_deleted_from_new_and_who_changed | |
| SYNC-004 | two-way remembers what it did | P0 | EXISTS: two_way_sync_remembers_what_it_did | |
| SYNC-005 | conflict (both changed) is skipped and reported | P0 | NEW | both files untouched after run |
| SYNC-006 | overrides change step actions; `allows()` rejects illegal overrides | P0 | NEW | e.g. override `deleteLeft` on a right-only item is rejected |
| SYNC-007 | mirror to bucket, replace file with folder | P0 | EXISTS: mirror_to_a_bucket_and_replace_a_file_with_a_folder | |
| SYNC-008 | S3 server-stamped times do not cause ping-pong | P0 | NEW | two-way local⇄s3, sync, sync again → second plan has no copy steps |
| SYNC-009 | next_snapshot records matches and results | P1 | EXISTS | |
| SYNC-010 | failed step not recorded as done → retried next time | P0 | NEW | FaultyFs fails one copy; next plan proposes it again |
| SYNC-011 | forget history → next two-way sync behaves as first | P1 | NEW | |
| SYNC-012 | snapshot file keyed per pair (`fnv`), pairs independent | P1 | NEW | |
| SYNC-013 | sync favorites save/list/remove persist | P1 | NEW | |
| SYNC-014 | plan is consumed once (`settle`); running a stale plan id errors | P1 | NEW | |
| SYNC-015 | Totals (bytes/items per direction) correct | P1 | NEW | |
| SYNC-016 | mirror across every remote backend | P0 | NEW (L3) | after mirror, compare → all same |
| SYNC-017 | two-way property test | P1 | NEW | proptest: random edits on both sides of a MemFs pair (no conflicts) → after sync both sides equal; with conflicts → conflicting paths untouched |

### 8.9 Auto-sync (live) — `LIVE`

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| LIVE-001 | one-way copies changes as they happen | P0 | EXISTS: one_way_copies_changes_as_they_happen | |
| LIVE-002 | both ways copies each way and passes on deletions | P0 | EXISTS | |
| LIVE-003 | same folder twice refused | P0 | EXISTS | |
| LIVE-004 | deletion in folders that started equal passed on | P0 | EXISTS | |
| LIVE-005 | switching waits for old sync to stop | P0 | EXISTS | |
| LIVE-006 | one-way never deletes on target | P0 | NEW | delete in source → target keeps file |
| LIVE-007 | one-folder-inside-the-other refused (`/a` and `/a/b`) | P0 | NEW | |
| LIVE-008 | remote side polled (PREP-03 at 300 ms) | P0 | NEW (L2 with MemFs registered as non-local, L3 with sftp) | change on remote appears locally within 3 polls |
| LIVE-009 | status broadcast: syncing → watching; failure → failed with message | P0 | NEW | FaultyFs failure |
| LIVE-010 | stop ends watcher; later changes not copied | P0 | NEW | |
| LIVE-011 | burst of 500 file creations coalesced into few syncs, all copied | P1 | NEW | |
| LIVE-012 | rename in source shows as add+delete (both) / add (one-way) | P1 | NEW | |
| LIVE-013 | each live sync appears as an operation (activity) | P1 | NEW | |
| LIVE-014 | source folder deleted while running → failed state, no deletion on target | P0 | NEW | |

### 8.10 Multi Rename — `REN`

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| REN-001 | replace plain and regex, `$1` groups | P0 | EXISTS: replace_plain_and_regex | |
| REN-002 | case, numbering, insert, remove, extension | P0 | EXISTS: case_numbering_insert_remove_and_extension | |
| REN-003 | tokens and folders | P0 | EXISTS: tokens_and_folders | |
| REN-004 | issues reported (clash, empty, invalid chars) | P0 | EXISTS: issues_are_reported | |
| REN-005 | presets round trip | P1 | EXISTS: presets_round_trip | |
| REN-006 | built-in presets present and valid | P1 | NEW | each built-in preset applies without issues to `small_tree` names |
| REN-007 | invalid regex → issue, not panic | P0 | NEW | `(` pattern |
| REN-008 | scope name/extension/whole for every rule | P1 | NEW | table-driven |
| REN-009 | `{date}` uses item mtime in a fixed format | P1 | NEW | |
| REN-010 | number padding, negative step, start 0 | P2 | NEW | |
| REN-011 | rule order matters and is preserved through JSON | P1 | NEW | |
| REN-012 | proptest: preview never yields two equal targets without an issue | P0 | NEW | |
| REN-013 | case-only rename on case-insensitive FS works | P0 | NEW (L2 Win/mac) | |
| REN-014 | removing a preset; saving with existing name replaces | P2 | NEW | |

### 8.11 Diff tools — `DIFF`

| ID | Title | P | Status | Assert |
|---|---|---|---|---|
| DIFF-001 | template split and fill | P1 | EXISTS: templates_split_and_fill | |
| DIFF-002 | env vars expand | P1 | EXISTS: env_vars_expand | |
| DIFF-003 | quotes and spaces in paths and template | P0 | NEW | `"C:\Program Files\X\x.exe" "{left}" "{right}"` with paths containing spaces and quotes |
| DIFF-004 | template missing `{left}`/`{right}` → error | P1 | NEW | |
| DIFF-005 | detect finds a fake tool on PATH | P1 | NEW | put `meld` (shell script) in a temp dir on PATH → detected |
| DIFF-006 | compare_files downloads remote files first | P1 | NEW (L4) | PREP-04 log lists two local paths whose contents equal the remote files |

### 8.12 Tauri command layer — `CMD` (L4)

New folder `src-tauri/tests/` with `common/mod.rs` that builds the app:

```rust
pub fn app() -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let state = dualpal_app::build_state(
        dualpal_app::AppDirs { data: tmp.path().join("data"), cache: tmp.path().join("cache") },
        std::sync::Arc::new(dualpal_core::connections::MemorySecrets::default()),
    ).unwrap();
    let app = dualpal_app::register_handlers(tauri::test::mock_builder())
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (app, tmp)
}
pub fn call<T: serde::de::DeserializeOwned>(app: &App, cmd: &str, args: serde_json::Value) -> Result<T, serde_json::Value>
// uses tauri::test::get_ipc_response with a WebviewWindow built by tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
```

| ID | Title | P | Assert |
|---|---|---|---|
| CMD-000 | every command in `generate_handler!` has a test | P0 | parse `src-tauri/src/lib.rs` handler list; each name appears in some `CMD-*` test (grep the test sources) |
| CMD-001 | `file_systems` lists `local` | P0 | |
| CMD-002 | `home_location` honours `DUALPAL_HOME` | P0 | |
| CMD-003 | `list_dir` returns camelCase Entry JSON | P0 | insta snapshot of one entry (with times redacted) |
| CMD-004 | `list_dir` on missing folder → `{kind:"notFound", message}` | P0 | error is an object with exactly `kind` and `message` (plus documented extras for unknownHost) |
| CMD-005 | `parent_location` root → null | P1 | |
| CMD-006 | `enqueue_operation` copy → `operation-updated` events emitted to the webview | P0 | listen on the mock window; receive running and completed payloads |
| CMD-007 | `cancel_operation`, `operations`, `clear_finished_operations` | P0 | |
| CMD-008 | `activity`, `clear_activity`, `activity_insights(days, utcOffsetMinutes)` argument names | P0 | wrong arg name → error (guards UI/Rust drift) |
| CMD-009 | `volumes`, `available_space` | P2 | |
| CMD-010 | favorites add/remove/reorder return the new list | P1 | |
| CMD-011 | `open_path` / `open_url` go to opener log (PREP-04); `open_url` rejects non-http(s)/mailto schemes | P0 | `file:///etc/passwd`, `javascript:` refused |
| CMD-012 | `about_info` has version equal to `tauri.conf.json` version | P1 | |
| CMD-013 | `stat`, `read_text(maxBytes)`, `write_text` | P0 | |
| CMD-014 | `connections` never includes secrets in its JSON | P0 | save with secret "S3CR3T" → JSON of `connections` lacks it |
| CMD-015 | `save_connection` / `remove_connection` / `reorder_connections` | P0 | |
| CMD-016 | `connect` unknown host → error kind `unknownHost` with `fingerprint`; `connect(trust)` succeeds (needs L3 servers; skip otherwise) | P0 | |
| CMD-017 | `connect` with `remember: true` stores the secret; `false` does not | P0 | |
| CMD-018 | `disconnect` | P0 | see CON-025 |
| CMD-019 | connection favorites add/remove | P1 | |
| CMD-020 | `ssh_config_hosts` reads `$DUALPAL_HOME/.ssh/config` | P1 | |
| CMD-021 | `open_file(editor)` on local → opener log; on remote → EditSession returned, `edit-session-updated` emitted | P0 | |
| CMD-022 | `edit_sessions`, `stop_edit_session`, `upload_edit_session` | P1 | |
| CMD-023 | `preview_path` local returns same path; remote downloads into cache/preview | P1 | file exists and content matches |
| CMD-024 | `settings` / `save_settings` round trip | P1 | |
| CMD-025 | `sync_plan` → SyncPreview snapshot; `compare-progress` events | P0 | |
| CMD-026 | `cancel_compare` during a big compare | P1 | |
| CMD-027 | `run_sync(plan, overrides)` returns an operation id; result on disk | P0 | |
| CMD-028 | `forget_sync_history` | P2 | |
| CMD-029 | sync favorites CRUD | P1 | |
| CMD-030 | `rename_preview(rules, inputs, others)` | P0 | |
| CMD-031 | rename presets CRUD | P2 | |
| CMD-032 | `diff_tools`, `compare_files` (opener log) | P1 | |
| CMD-033 | `live_sync_start/stop/status` + `live-sync-updated` (null on stop) | P0 | |
| CMD-034 | capabilities file grants only what the UI needs | P1 | parse `src-tauri/capabilities/default.json`; snapshot permissions list so additions are reviewed |
| CMD-035 | CSP in `tauri.conf.json` has no `unsafe-eval` and no remote origins | P1 | |

### 8.13 UI unit and component tests — `UI-UNIT`, `UI-CMP` (L5)

Setup: `vitest.config.ts` at repo root reusing `vite.config.ts`, `environment: "jsdom"`, `setupFiles: ["ui/tests/setup.ts"]` (installs `@testing-library/jest-dom`, `mockWindows("main")`, `installMockBackend("default")`, `clearMocks()` after each). Tests live next to sources as `*.test.ts` or in `ui/tests/unit/`. Files that use runes (`*.svelte.ts`) are imported normally; the Svelte plugin compiles them.

| ID | Title | P | Assert |
|---|---|---|---|
| UI-UNIT-001 | `formatBytes` boundaries | P0 | 0, 1 ("1 byte"), 999, 1000 ("1.0 KB"), 9999, 10 000 ("10 KB"), 1e12, 1e15 (stays TB) |
| UI-UNIT-002 | `formatDate`/`formatShortDate` null → "" | P1 | |
| UI-UNIT-003 | `extension` | P0 | `.bashrc` → "", `a.TAR.GZ` → "gz", `noext` → "" |
| UI-UNIT-004 | `category` / `glyph` table, `.app` folder is app | P1 | |
| UI-UNIT-005 | `thumbnailUrl` only local files, image ext, ≤ 50 MB | P1 | mock `convertFileSrc` |
| UI-UNIT-006 | `patternMatcher` | P0 | `*.jpg; *.png` matches `A.JPG`; `?` single char; `,` separator; regex specials in names (`a+b.txt`, `[1].txt`) treated literally; empty pattern matches nothing |
| UI-UNIT-007 | `errorKind` / `errorMessage` | P0 | object, string, null, Error instance |
| UI-UNIT-008 | `api.*` passes exact argument names | P0 | for each api function: call → `calls(name)[0].args` equals expected object (guards against renames; must agree with CMD tests) |
| UI-UNIT-009 | `PaneState.visible` sorting | P0 | folders first; name sort natural (`file2` < `file10`), case-insensitive; size/modified asc/desc; ties by name |
| UI-UNIT-010 | `PaneState` hidden filter | P0 | |
| UI-UNIT-011 | `PaneState.targets` = marked or cursor | P0 | |
| UI-UNIT-012 | `PaneState.open` history/back/forward | P1 | |
| UI-UNIT-013 | `PaneState` folder disappears → nearest existing ancestor | P0 | mock: listDir(notFound) for `/a/b/c`, `/a/b` → opens `/a`; error message shown |
| UI-UNIT-014 | `PaneState.navigated` hook fires (linked browsing) | P1 | |
| UI-UNIT-015 | `drag.svelte.ts` state transitions | P1 | start, over target, Shift toggles move, Esc cancels |
| UI-UNIT-016 | `about.ts` constants (email, donate address) | P1 | address equals README address `0xF8352b493e7750C03bde4bdA94A203B010933735` |
| UI-CMP-001 | `Dialog` focus trap, Esc cancels, Enter confirms | P0 | |
| UI-CMP-002 | `ContextMenu` keyboard navigation, closes on outside click | P1 | |
| UI-CMP-003 | `CommandPalette` fuzzy filter, arrow keys, Enter runs, lists servers/favorites/syncs | P0 | |
| UI-CMP-004 | `ConnectionDialog` fields per protocol (SFTP auth methods, FTP security, WebDAV URL, S3 endpoint/region/path-style); validation errors; secret field type=password; save calls `save_connection` with correct Endpoint shape | P0 | |
| UI-CMP-005 | `ConnectionDialog` ssh-config host picker fills fields | P1 | |
| UI-CMP-006 | `MultiRename` live preview, issues highlighted, apply disabled with issues, preset save/load | P0 | |
| UI-CMP-007 | `CompareView` renders states, per-row action change, deletions require confirmation, double-click changed file → `compare_files` | P0 | |
| UI-CMP-008 | `OperationsPanel` statuses, progress bar %, cancel, clear, activity tab | P0 | |
| UI-CMP-009 | `Inspector` preview by type, quick edit dirty state + Ctrl/⌘ S save, binary/truncated warning | P1 | |
| UI-CMP-010 | `Sidebar` sections, collapse server favorites, connected dot, remove asks | P1 | |
| UI-CMP-011 | `EditSessions` bar statuses | P2 | |
| UI-CMP-012 | `Insights` renders numbers from fixture | P2 | |
| UI-CMP-013 | `AboutDialog` version, Email Us → `open_url("mailto:…")`, donate QR + copy address | P2 | |
| UI-MOCK-001 | mock backend shapes match Rust snapshots | P0 | see §7.4 |

### 8.14 UI E2E with mocked backend — `UI-E2E`, `UI-KEY`, `UI-MOUSE` (L6)

Setup: `playwright.config.ts` at repo root; `webServer: { command: "npx vite build --mode test && npx vite preview --port 4173 --strictPort", url: "http://localhost:4173" }` (`--mode test` is what turns on the PREP-06 ready signal; a plain `npm run build` is production mode and `window.__DUALPAL_TEST__` would never exist); projects `chromium` and `webkit`; `use: { trace: "retain-on-failure", screenshot: "only-on-failure" }`. Each test: `await page.addInitScript(installMockBackendScript, scenario)` (which also sets `localStorage.dualpalTest = "1"` as a second switch, so a production build can be tested too) then `page.goto("/")`, then `await page.evaluate(() => window.__DUALPAL_TEST__.ready)`. Helper page object `ui/tests/e2e/pom/app.ts` with `pane(side)`, `row(side, name)`, `press(keys)`, `toolbar(label)`, `dialog()`, `ops()`. On macOS-style shortcuts, tests use `ControlOrMeta`.

**Keyboard map (one test per README row)** — each asserts the UI result *and* the mock call.

| ID | Key | Assert |
|---|---|---|
| UI-KEY-000 | (meta) | every row of the README keyboard table has a UI-KEY test (parse README) |
| UI-KEY-001 | Tab | active pane switches (aria/active class) |
| UI-KEY-002 | ↑ ↓ PgUp PgDn Home End | cursor index moves correctly, clamps at ends, scrolls into view |
| UI-KEY-003 | Enter on folder / file | folder: `list_dir` new path; file: `open_file` called |
| UI-KEY-004 | Backspace, Ctrl/⌘ ↑ | parent opened, previous folder under cursor |
| UI-KEY-005 | Space / Insert | toggles mark, cursor moves down |
| UI-KEY-006 | Shift ↑ / Shift ↓ | marks passed items |
| UI-KEY-007 | Ctrl/⌘ A | all visible marked |
| UI-KEY-008 | Menu key / Shift F10 | context menu for cursor item |
| UI-KEY-009 | F2 | rename prompt pre-filled with name stem selected; Enter → `enqueue_operation {type:"rename"}` |
| UI-KEY-010 | Shift F2 | Multi Rename opens with marked items |
| UI-KEY-011 | F4 | `open_file(editor:true)` |
| UI-KEY-012 | F5 | copy marked to other pane folder: `enqueue_operation {type:"copy", sources, destination}`; conflict prompt flow when mock reports alreadyExists |
| UI-KEY-013 | F6 | move |
| UI-KEY-014 | F7 | new folder prompt → `createDir` |
| UI-KEY-015 | F8 / Delete | `trash` on targets, no confirmation |
| UI-KEY-016 | Shift F8 / Shift Delete | confirmation dialog names items; Cancel → no call; OK → `delete` |
| UI-KEY-017 | + / − | pattern prompt; marks/unmarks matching |
| UI-KEY-018 | Ctrl/⌘ 1 / 2 | list / icon view |
| UI-KEY-019 | Ctrl/⌘ T, Ctrl/⌘ W | new tab (same folder), close tab; last tab cannot be closed |
| UI-KEY-020 | Ctrl Tab, Ctrl Shift Tab | next / previous tab with wrap |
| UI-KEY-021 | Ctrl/⌘ I | inspector toggles |
| UI-KEY-022 | Ctrl/⌘ S in editor | `write_text` with edited text |
| UI-KEY-023 | Ctrl/⌘ R | `list_dir` again |
| UI-KEY-024 | Ctrl/⌘ Shift . | hidden files shown |
| UI-KEY-025 | Ctrl/⌘ Shift P | palette opens and focuses input |
| UI-KEY-026 | Ctrl/⌘ Shift Y | compare view with `sync_plan(left,right)` |
| UI-KEY-027 | Ctrl/⌘ Shift D | `compare_files` with the two cursor files |
| UI-KEY-028 | Ctrl/⌘ Shift L | linked browsing toggles; entering folder in one pane enters same name in other |
| UI-KEY-029 | shortcuts ignored while typing in an input/editor | F5 inside rename prompt does not copy |
| UI-KEY-030 | shortcuts disabled while auto-sync locks panes (`frozen`) | F5/F6/F8 make no calls |

**Mouse**

| ID | Title | P | Assert |
|---|---|---|---|
| UI-MOUSE-001 | click sets cursor; Ctrl/⌘-click marks; Shift-click range | P0 | |
| UI-MOUSE-002 | double-click folder opens, file opens | P0 | |
| UI-MOUSE-003 | right-click item menu actions (each item → expected call) | P0 | data-action per item: open, edit, copy-to-other, move-to-other, rename, compress, copy-path (clipboard), compare, trash, delete |
| UI-MOUSE-004 | right-click with several marked acts on all | P0 | |
| UI-MOUSE-005 | right-click empty space: new folder, refresh, select | P1 | |
| UI-MOUSE-006 | drag to other pane copies; Shift on drop moves; Esc cancels | P0 | |
| UI-MOUSE-007 | drag onto folder in same pane copies into it; onto itself does nothing | P0 | |
| UI-MOUSE-008 | external file drop (simulate Tauri drag-drop event) copies to target folder | P1 | mock emits `tauri://drag-drop` with paths |
| UI-MOUSE-009 | column header click sorts, second click reverses | P1 | |
| UI-MOUSE-010 | breadcrumb click navigates | P1 | |
| UI-MOUSE-011 | dragging the divider between the panes resizes them; arrow keys on it move it; double-click evens them; the split survives a reload | P1 | |

**Flows**

| ID | Title | P | Assert |
|---|---|---|---|
| UI-E2E-001 | app loads, both panes list home, status shows counts and free space | P0 | |
| UI-E2E-002 | operations panel shows live progress from `operation-updated` events and final state | P0 | mock emits progress series; % increases; completed item listed in Activity |
| UI-E2E-003 | failed operation shows error message; retry not automatic | P0 | |
| UI-E2E-004 | conflict flow: copy onto existing → dialog Skip/Overwrite/Cancel → re-enqueue with policy | P0 | |
| UI-E2E-005 | pane refreshes after operation completes in its folder | P0 | |
| UI-E2E-006 | folder disappears (mock removes) → pane goes to nearest existing parent with notice | P0 | |
| UI-E2E-007 | favorites: ★ adds current folder, appears in sidebar, remove asks | P1 | |
| UI-E2E-008 | add SFTP server: dialog → save → sidebar entry → connect → unknown host fingerprint dialog → Trust → pane shows server root | P0 | |
| UI-E2E-009 | wrong password → error in dialog, can retry, "remember" stores (save_connection args) | P0 | |
| UI-E2E-010 | changed host key → blocking warning, no trust button | P0 | |
| UI-E2E-011 | FTPS self-signed certificate trust dialog | P1 | |
| UI-E2E-012 | S3 and WebDAV dialog round trip | P1 | |
| UI-E2E-013 | server favorite ★ under server; disconnect returns panes on that server to local | P1 | |
| UI-E2E-014 | edit remote file: F4 → Editing bar shows watching → mock emits uploaded → status updates; stop | P1 | |
| UI-E2E-015 | Compare & Sync: open, choose mode Update/Mirror/Two-way, change a step action, deletions confirm, Synchronize → `run_sync(plan, overrides)` | P0 | overrides map contains changed rows only |
| UI-E2E-016 | Compare options: by content, exclude patterns, hidden toggle → re-plan with options | P1 | |
| UI-E2E-017 | Save sync as favorite → Syncs sidebar → open favorite reconnects server and opens compare | P1 | |
| UI-E2E-018 | Auto-sync → shows confirmation naming both folders; Cancel → no call; OK → `live_sync_start(from,to,"oneWay")`; live bar visible; panes locked; Stop → `live_sync_stop`, unlock | P0 | also ← and ⇄ ("both") |
| UI-E2E-019 | auto-sync failure event shows failed state in bar | P1 | |
| UI-E2E-020 | Multi Rename end to end with preset | P0 | |
| UI-E2E-021 | select by pattern | P1 | |
| UI-E2E-022 | quick edit in Inspector: edit, dirty marker, Ctrl/⌘ S, navigating away with unsaved changes asks | P1 | |
| UI-E2E-023 | compress marked items → name prompt → `compress` | P1 | |
| UI-E2E-024 | command palette: run "New Folder", open server, open sync favorite, "Choose Compare App…" | P1 | |
| UI-E2E-025 | Editor… sets settings.editor via `save_settings` | P2 | |
| UI-E2E-026 | Activity insights view renders from `activity_insights` | P2 | |
| UI-E2E-027 | About and Donate dialogs; Email Us → `open_url("mailto:hoangtuanbs@proton.me…")` | P2 | |
| UI-E2E-028 | narrow window (800×480 min size): names readable, toolbar wraps, no horizontal scroll in panes | P1 | |
| UI-E2E-029 | Windows path scenario: `C:\Users\…` breadcrumbs and parent navigation | P1 | scenario "windows-paths" |
| UI-E2E-030 | 5 000 entries render and scroll; cursor End reaches last | P1 | |
| UI-E2E-031 | unsaved state survives nothing unexpected: reload restores favorites/servers from backend, not localStorage | P2 | |

### 8.15 Full-stack desktop E2E — `DESK` (L7)

`e2e-desktop/` with `wdio.conf.ts`: `capabilities: [{ "tauri:options": { application: "<repo>/target/debug/dualpal[.exe]" } }]`, `onPrepare` builds once with `npx tauri build --debug --no-bundle`, `beforeSession` spawns `tauri-driver` (and on Linux `Xvfb :99` + `DISPLAY=:99`), sets the env from §7.5 per spec file. Specs reuse selectors from PREP-05. Keep this suite small: it proves the wiring.

| ID | Title | P | Assert |
|---|---|---|---|
| DESK-001 | app starts, window title "Dual Pal", both panes list `$DUALPAL_HOME` | P0 | |
| DESK-002 | F5 copy local→local, file on disk with same sha, Activity shows record, `activity.jsonl` has a line | P0 | |
| DESK-003 | F6 move, F7 new folder, F2 rename reflected on disk | P0 | |
| DESK-004 | Shift Delete confirm deletes on disk; F8 moves to trash (Linux: temp `XDG_DATA_HOME/Trash`) | P0 | |
| DESK-005 | cancel a big copy from the Operations panel → no partial file on disk | P0 | `big.bin` 256 MiB |
| DESK-006 | favorites persisted across restart | P0 | restart app with same data dir |
| DESK-007 | SFTP: add server (key file), trust host, browse, upload, download; sha equal | P0 | test servers |
| DESK-008 | FTP, WebDAV, S3: connect and copy one file each way | P0 | |
| DESK-009 | Compare & Sync mirror local → S3, then compare shows all same | P0 | |
| DESK-010 | Auto-sync one way local→local: create file in left on disk → appears right within 5 s | P0 | |
| DESK-011 | Auto-sync local→SFTP with `DUALPAL_LIVE_POLL_MS=500` | P1 | |
| DESK-012 | remote edit: F4 → opener log has local copy path; modify that file → server content updated | P0 | |
| DESK-013 | Compare Files → diff log has two paths | P1 | |
| DESK-014 | folder deleted on disk while shown → pane goes to parent | P0 | |
| DESK-015 | drag and drop between panes (WebDriver actions) copies | P1 | |
| DESK-016 | connection secret with `remember` in memory store survives within session; `connections.json` lacks it | P0 | |
| DESK-017 | dropped SFTP connection mid-upload (`drop-connections`) → operation completes | P1 | |
| DESK-018 | Windows only: sidebar shows drives; WSL entry when available | P2 | |
| DESK-019 | quick edit text file in Inspector and save → on disk | P1 | |
| DESK-020 | app restart with unsent edit in `edits/` → Editing bar offers upload | P1 | |

### 8.16 Installers and packaging — `INS` (L8)

Run after `build.yml` produces artifacts. New workflow `installers-verify.yml` (see §9) downloads them. Scripts in `scripts/installer-tests/`.

**Common checks on every package**

| ID | Title | P | Assert |
|---|---|---|---|
| INS-000 | artifact names match README patterns | P0 | `Dual.Pal_<v>_x64-setup.exe`, `Dual.Pal_<v>_x64_en-US.msi`, `Dual.Pal_<v>_amd64.deb`, `Dual.Pal-<v>-1.x86_64.rpm`, `*.AppImage`, `*.dmg`; `<v>` equals `tauri.conf.json` version and root `Cargo.toml` `workspace.package.version` and `package.json` version |
| INS-001 | version consistency | P0 | the three versions equal (run in CI job on every PR, cheap) |
| INS-002 | binary reports version | P1 | `about_info` via L7 or `strings` check of the binary for the version |
| INS-003 | size budget | P2 | each installer < 25 MB (adjust after first measurement; fail on +20 % regression) |

**Linux** (`scripts/installer-tests/linux.sh <artifact-dir>`; each distro in a fresh container, `xvfb-run` for launch)

| ID | Title | P | Assert |
|---|---|---|---|
| INS-LNX-001 | `.deb` installs on Ubuntu 24.04 and Debian 12 with `apt install ./x.deb` | P0 | exit 0; dependencies resolved (libwebkit2gtk-4.1-0, libgtk-3-0) |
| INS-LNX-002 | `.deb` contents | P0 | `/usr/bin/dualpal` (or the binary name Tauri uses — read `dpkg -L` and pin it), `.desktop` file valid (`desktop-file-validate`), icons in `hicolor` sizes |
| INS-LNX-003 | launch smoke | P0 | `xvfb-run -a timeout 20 dualpal` with `DUALPAL_DATA_DIR` temp: process alive after 10 s, no panic in stderr, data dir created; kill → exit |
| INS-LNX-004 | `.deb` upgrade from previous release | P1 | install latest GitHub release `.deb`, then new one; data dir preserved |
| INS-LNX-005 | `.deb` remove/purge leaves no binary | P0 | |
| INS-LNX-006 | `.rpm` installs on Fedora 41 (`dnf install`) and openSUSE Tumbleweed (`zypper --no-gpg-checks in`) | P0 | |
| INS-LNX-007 | `.rpm` launch smoke + erase | P0 | |
| INS-LNX-008 | AppImage runs on Ubuntu 22.04 and 24.04 after `chmod +x` (with `APPIMAGE_EXTRACT_AND_RUN=1` in containers without FUSE) | P0 | launch smoke |
| INS-LNX-009 | AppImage contains desktop file and icon at root | P1 | `--appimage-extract` |
| INS-LNX-010 | `lintian` on `.deb` and `rpmlint` on `.rpm`: no errors (warnings allowed, recorded) | P2 | |

**Windows** (`scripts/installer-tests/windows.ps1`, GitHub `windows-latest`, also `windows-2022`)

| ID | Title | P | Assert |
|---|---|---|---|
| INS-WIN-001 | NSIS silent per-user install `setup.exe /S` without admin | P0 | exit 0; `%LOCALAPPDATA%\Dual Pal\dualpal.exe` (pin real path after first run) exists; Start Menu shortcut exists; uninstall entry under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall` with `DisplayVersion` = version |
| INS-WIN-002 | NSIS launch smoke | P0 | start with `DUALPAL_DATA_DIR`, wait 10 s, process alive, window titled "Dual Pal" exists (`Get-Process | ? MainWindowTitle -eq 'Dual Pal'`), stop |
| INS-WIN-003 | NSIS uninstall silent | P0 | uninstaller `/S`: files and registry key gone; user data in `%APPDATA%\com.hoangtuanbs.dualpal` kept (document) |
| INS-WIN-004 | MSI install `msiexec /i x.msi /qn /l*v log.txt` (admin) | P0 | exit 0; `Program Files\Dual Pal\…exe`; `HKLM` uninstall entry |
| INS-WIN-005 | MSI launch smoke | P0 | |
| INS-WIN-006 | MSI upgrade from previous release keeps one entry | P1 | same UpgradeCode; only one ARP entry after upgrade |
| INS-WIN-007 | MSI uninstall `msiexec /x … /qn` | P0 | |
| INS-WIN-008 | WebView2 bootstrapper behaviour documented | P2 | on a runner with WebView2 present the install does not download |
| INS-WIN-009 | executable has icon and version resource | P1 | `(Get-Item exe).VersionInfo.ProductVersion` = version; `FileDescription` "Dual Pal" |
| INS-WIN-010 | Authenticode status recorded (unsigned today) | P2 | `Get-AuthenticodeSignature` → record in job summary; becomes P0 once signing exists |

**macOS** (`scripts/installer-tests/macos.sh`, `macos-latest`, smoke only)

| ID | Title | P | Assert |
|---|---|---|---|
| INS-MAC-001 | `.dmg` mounts (`hdiutil attach -nobrowse`) and contains `Dual Pal.app` + Applications link | P0 | |
| INS-MAC-002 | app is universal | P0 | `lipo -archs` lists `x86_64 arm64` |
| INS-MAC-003 | ad-hoc signature valid | P1 | `codesign --verify --deep --strict` |
| INS-MAC-004 | `Info.plist` version, bundle id `com.hoangtuanbs.dualpal`, `LSMinimumSystemVersion` 11.0 | P0 | |
| INS-MAC-005 | launch smoke after `xattr -cr` | P1 | `open -W -n` with timeout, process alive 10 s |

**Release workflow**

| ID | Title | P | Assert |
|---|---|---|---|
| INS-REL-001 | tag ≠ version fails the job | P1 | `act` or a dry-run job running the same bash step with a fake `GITHUB_REF_NAME` |
| INS-REL-002 | release job collects every installer from nested folders | P1 | run the `find installers -type f` step against a fixture directory |

### 8.17 Non-functional — `PERF`, `SEC`, `A11Y`, `VIS`, `REL`

| ID | Title | Layer | P | Assert / budget |
|---|---|---|---|---|
| PERF-001 | list 10 000-entry local folder | criterion | P1 | median < 150 ms (Linux runner); fail on +25 % vs stored baseline |
| PERF-002 | compare two 20 000-file trees (size+date) | criterion | P1 | < 2 s |
| PERF-003 | copy 1 GiB local→local throughput | nightly | P2 | ≥ 70 % of `cp` on same runner |
| PERF-004 | SFTP upload 256 MiB throughput | nightly L3 | P2 | record; alert on −30 % |
| PERF-005 | app cold start to first listing | L7 nightly | P1 | < 3 s (Linux xvfb), measured via `__DUALPAL_TEST__.ready` timestamp |
| PERF-006 | UI renders 5 000 rows | L6 | P1 | `ready` → last row visible after End < 1 s |
| PERF-007 | memory: 30 min auto-sync soak | nightly | P2 | RSS growth < 50 MB |
| SEC-001 | path traversal from server names | L2/L3 | P0 | see OPS-COPY-024, VFS-PATH-* |
| SEC-002 | secrets never in files or logs | L2/L4/L7 | P0 | after full E2E run, grep data dir, cache dir and captured stderr for every test secret → none |
| SEC-003 | host key / TLS trust flows | L3 | P0 | CON-009, CON-010, CON-018 |
| SEC-004 | `open_url` scheme allow-list | L4 | P0 | CMD-011 |
| SEC-005 | CSP and capabilities reviewed | L4 | P1 | CMD-034, CMD-035 |
| SEC-006 | asset protocol scope `**` documented risk; preview of a remote HTML/SVG file cannot run script | L6/L7 | P1 | open SVG with `<script>` in Inspector → no script execution (CSP) |
| SEC-007 | `cargo audit`, `cargo deny check`, `npm audit --omit=dev --audit-level=high` | CI | P0 | no unignored high/critical advisories |
| SEC-008 | test servers bind 127.0.0.1 only | CI | P1 | parse compose ports |
| A11Y-001 | axe scan of main window, each dialog, compare view, multi rename | L6 | P1 | no `serious`/`critical` violations (baseline existing ones in an allow-list file and burn down) |
| A11Y-002 | full keyboard operability: every toolbar action reachable without mouse | L6 | P1 | |
| A11Y-003 | focus visible and returned to the pane after dialogs close | L6 | P1 | |
| VIS-001 | screenshot baselines: main window list/icon view, compare view, connection dialog, about/donate, light and dark | L6 chromium/Linux only | P2 | `toHaveScreenshot({ maxDiffPixelRatio: 0.01 })`; baselines committed under `ui/tests/e2e/__screenshots__`; reference images in the project folder (`main-window.png`, `step3-*.png`) are for humans, not baselines |
| REL-001 | flake detection | nightly | P1 | nightly runs nextest with `--retries 2` and Playwright `--repeat-each 3`; any test that passed only on retry is listed in the job summary and an issue is opened |
| REL-002 | remote suite under toxiproxy latency 200 ms | nightly L3 | P2 | whole contract suite passes with latency |

---

## 9. CI/CD design

### 9.1 Workflows

Keep `build.yml` as is (it produces installers). Restructure `ci.yml` into jobs and add two workflows.

```
ci.yml (push main, pull_request)
├─ lint            ubuntu   fmt, clippy, npm run check, cargo deny, npm audit, version consistency (INS-001)
├─ rust            matrix [ubuntu-24.04, windows-latest, macos-latest]   nextest L1+L2 (no remote), L4 commands
├─ rust-remote     ubuntu   docker compose up --wait; DUALPAL_REQUIRE_REMOTE=1; nextest L3 (contract_remote, remote.rs)
├─ coverage        ubuntu   needs rust-remote' setup; cargo llvm-cov nextest --workspace --lcov; vitest coverage; upload + threshold gate
├─ ui-unit         ubuntu   npm ci; npm run test:unit
├─ ui-e2e          ubuntu   npx playwright install --with-deps chromium webkit; npm run test:e2e
├─ desktop-e2e     matrix [ubuntu-24.04, windows-latest]   tauri-driver + wdio (L7); Linux also starts test servers
└─ build-app       existing "npx tauri build --debug --no-bundle" (can be merged into desktop-e2e, which builds the same binary)

installers-verify.yml (workflow_run: "Build installers" completed, success; also workflow_dispatch with run-id input)
├─ linux           matrix distro [ubuntu:24.04, debian:12, fedora:41, opensuse/tumbleweed, ubuntu:22.04 (AppImage)]
├─ windows         matrix [windows-latest, windows-2022] × [nsis, msi]
└─ macos           macos-latest dmg smoke

nightly.yml (schedule "0 2 * * *", workflow_dispatch)
├─ flake hunt      nextest --retries 2 --profile nightly; playwright --repeat-each 3
├─ remote-chaos    compose + toxiproxy; REL-002; CON-024
├─ perf            cargo bench (criterion) compared to baseline in gh-pages or artifact; PERF-003..007
└─ upgrade         INS-LNX-004, INS-WIN-006 against the latest GitHub release
```

### 9.2 Key job snippets

`rust-remote` (replaces the current "Test SSH and S3 servers" step):

```yaml
  rust-remote:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: taiki-e/install-action@v2
        with: { tool: cargo-nextest }
      - run: sudo apt-get update && sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev
      - name: Start test servers
        run: docker compose -f docker/test-servers/compose.yaml up -d --build --wait --wait-timeout 180
      - name: Export server settings
        run: scripts/ci/remote-env.sh >> "$GITHUB_ENV"   # the same exports as today, plus DUALPAL_REQUIRE_REMOTE=1, TOXIPROXY
      - name: Remote tests (shared servers, in parallel)
        run: cargo nextest run -p dualpal-core --profile ci -E '(binary(remote) | binary(contract_remote) | binary(transfers)) - test(/fault_/)'
      - name: Fault tests (cut or slow down every connection, so one at a time)
        run: cargo nextest run -p dualpal-core --profile ci -E '(binary(remote) | binary(contract_remote)) & test(/fault_/)' --test-threads 1
      - if: failure()
        run: docker compose -f docker/test-servers/compose.yaml logs > servers.log
      - if: always()
        uses: actions/upload-artifact@v4
        with: { name: rust-remote-results, path: "target/nextest/ci/junit.xml\nservers.log" }
```

`.config/nextest.toml`:

```toml
[profile.default]
slow-timeout = { period = "30s", terminate-after = 4 }

[profile.ci]
fail-fast = false
retries = 0
[profile.ci.junit]
path = "junit.xml"

[profile.nightly]
retries = 2
fail-fast = false

# Remote tests share servers: limit their parallelism.
# Tests that run DUALPAL_TEST_SFTP_DROP or add a Toxiproxy toxic affect every
# connection to that server, so they must be named `fault_*` and run in a
# separate, single-threaded invocation (see the rust-remote job). Running them
# alongside other remote tests makes those tests fail at random.
[[profile.default.overrides]]
filter = "binary(remote) | binary(contract_remote)"
threads-required = 2
test-group = "remote"

[test-groups]
remote = { max-threads = 4 }
```

`desktop-e2e` (Linux part):

```yaml
      - run: sudo apt-get install -y webkit2gtk-driver xvfb
      - run: cargo install tauri-driver --locked
      - run: npx tauri build --debug --no-bundle
      - run: xvfb-run -a npm run test:desktop
        env: { DUALPAL_E2E_BINARY: target/debug/dualpal }
      - if: failure()
        uses: actions/upload-artifact@v4
        with: { name: desktop-e2e-${{ runner.os }}, path: e2e-desktop/artifacts }
```

Windows part: install `msedgedriver` matching WebView2 (`npx --yes msedgedriver-tool` or download by the version from `Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' pv`), pass `--native-driver <path>` to `tauri-driver`.

### 9.3 Gates

| Gate | Blocks merge |
|---|---|
| lint, rust (3 OS), rust-remote, ui-unit, ui-e2e | yes |
| coverage thresholds (§4.2) | yes, after Phase 4 |
| desktop-e2e Linux | yes; Windows: yes after two weeks without flakes |
| installers-verify | runs on `main` and tags; release job must `needs:` a successful verify (change `build.yml` release job to depend on an `installers-verify` reusable workflow call) |
| nightly | never blocks; opens/updates an issue labelled `nightly-failure` |

Artifacts kept on failure: nextest JUnit, Playwright `test-results/` (traces), WebdriverIO screenshots + app stderr, docker server logs, installer logs (`msiexec /l*v`, `dpkg` output).

---

## 10. Phased implementation order

Each phase = one PR, mergeable on its own, with the acceptance criteria listed. Estimated sizes assume an AI agent working steadily.

| Phase | Content | Acceptance | Size |
|---|---|---|---|
| **0. Groundwork** | PREP-01…PREP-07; `dualpal-testkit` crate with fixture, queue, eventually, memfs, faulty, remote helpers; move existing helpers into it (no behaviour change); `.config/nextest.toml`; add ID doc comments to every `EXISTS` test | `cargo test --workspace` green on 3 OS; app runs normally without env vars; with `DUALPAL_DATA_DIR` it writes only there; remote tests fail when `DUALPAL_REQUIRE_REMOTE=1` and servers absent | M |
| **1. Core gaps (L1/L2)** | All `NEW`/`EXTEND` cases in §8.2–§8.11 that do not need servers: VFS contract on local + mem, OPS, ACT/PLC/SET, CON L1/L2, EDIT, CMP, SYNC (incl. SYNC-001 matrix and proptests), LIVE (with PREP-03), REN, DIFF; insta snapshots for JSON shapes | all P0/P1 of those sections implemented; nextest green on 3 OS; runtime < 90 s | L |
| **2. Remote (L3)** | compose healthchecks, toxiproxy, `keyonly` user, extra fixtures; `contract_remote.rs` over 6 backends; CON-009…024; OPS ×remote; CMP-012; SYNC-016; restructure `ci.yml` into `lint`, `rust`, `rust-remote` | remote job green with `DUALPAL_REQUIRE_REMOTE=1`; total < 5 min | L |
| **3. Commands (L4)** | `src-tauri/tests/` with `common::app()`; CMD-000…035 | every command tested; CMD-000 passes | M |
| **4. UI unit (L5) + coverage** | Vitest setup, mock backend (§7.4) incl. shape generation and UI-MOCK-001; UI-UNIT-*, UI-CMP-*; coverage job + thresholds | `npm run test:unit` green; ts coverage ≥ 85 %; core coverage ≥ 80 % | L |
| **5. UI E2E mocked (L6)** | Playwright config, page objects, UI-KEY-*, UI-MOUSE-*, UI-E2E-*, A11Y-001…003; `ui-e2e` CI job | all P0 + P1 green in chromium and webkit; < 4 min | L |
| **6. Desktop E2E (L7)** | `e2e-desktop/` WebdriverIO + tauri-driver, `make-fixture` bin, DESK-001…020; `desktop-e2e` job (Linux blocking, Windows non-blocking first) | Linux green 5 consecutive runs | M |
| **7. Installers (L8)** | `scripts/installer-tests/{linux.sh,windows.ps1,macos.sh}`, `installers-verify.yml`, INS-*; make release depend on verify | verify green on a `main` build; a deliberately broken version string fails INS-001 | M |
| **8. Non-functional (L9)** | criterion benches + baseline, nightly workflow, SEC-*, VIS-*, REL-*, PERF-* | nightly runs end to end; first baselines committed | M |

Dependencies: 0 → everything. 1 and 3 can run in parallel after 0. 2 after 0 (uses testkit). 4 needs 3's snapshots for UI-MOCK-001 (or generate shapes from core snapshots in 1). 5 after 4. 6 after 0 (PREP-02/05/06). 7 independent after 0 (PREP-02 for launch smoke). 8 last.

---

## 11. Appendices

### 11.1 Environment variables

| Variable | Used by | Meaning |
|---|---|---|
| `DUALPAL_TEST_SFTP` | L3 | `user@host:port` (CI: `tester@127.0.0.1:2222`) |
| `DUALPAL_TEST_SFTP_KEY` | L3 | private key file path (chmod 600) |
| `DUALPAL_TEST_SFTP_PASSWORD` | L3 | password for the same user (`dualpal`) |
| `DUALPAL_TEST_SFTP_PASSPHRASE_KEY` | L3 (new) | `path|passphrase` for user `keyonly` |
| `DUALPAL_TEST_SFTP_DROP` | L3 | command that cuts SSH sessions (`docker exec dualpal-test-sftp drop-connections`) |
| `DUALPAL_TEST_FTP` / `_FTPS` / `_FTPS_IMPLICIT` | L3 | `user:pass@host:port` (2121 / 2990 / 2991) |
| `DUALPAL_TEST_WEBDAV` | L3 | `user:pass@http://127.0.0.1:8088/dav` |
| `DUALPAL_TEST_S3` | L3 | `endpoint|bucket|access|secret` |
| `DUALPAL_TEST_TOXIPROXY` | L3 (new) | `http://127.0.0.1:8474` |
| `DUALPAL_REQUIRE_REMOTE` | L3 (new) | `1` → missing server variables fail tests |
| `DUALPAL_DATA_DIR`, `DUALPAL_CACHE_DIR` | app (new) | override app data / cache folder |
| `DUALPAL_HOME` | app (new) | override home folder (also `~/.ssh`) |
| `DUALPAL_SECRETS` | app (new) | `memory` → in-memory secret store |
| `DUALPAL_LIVE_POLL_MS`, `DUALPAL_WATCH_DEBOUNCE_MS` | app (new) | auto-sync timing |
| `DUALPAL_DIFF_TOOL_LOG`, `DUALPAL_OPENER_LOG` | app (new) | record instead of launching |
| `DUALPAL_E2E_BINARY` | L7 | path to the built app binary |

### 11.2 Commands cheat sheet

```sh
# L1+L2 (fast, any OS)
cargo nextest run --workspace -E 'not (binary(remote) | binary(contract_remote))'
# L3
docker compose -f docker/test-servers/compose.yaml up -d --build --wait
eval "$(scripts/ci/remote-env.sh --export)"            # exports DUALPAL_TEST_* and DUALPAL_REQUIRE_REMOTE=1
cargo nextest run -p dualpal-core -E 'binary(remote) | binary(contract_remote)'
# L4
cargo nextest run -p dualpal
# L5
npm run test:unit
# L6
npx playwright install --with-deps chromium webkit && npm run test:e2e
# L7 (Linux)
cargo install tauri-driver --locked && npx tauri build --debug --no-bundle && xvfb-run -a npm run test:desktop
# L8 (Linux, after downloading build artifacts to ./installers)
scripts/installer-tests/linux.sh ./installers
# Coverage
cargo llvm-cov nextest --workspace --lcov --output-path lcov.info
# Snapshots
cargo insta review
```

### 11.3 Traceability matrix

| Feature (README) | Test IDs |
|---|---|
| Two-pane browsing, tabs, views, sort, hidden | UI-UNIT-009…012, UI-KEY-001/002/018…021/024, UI-MOUSE-009/010, UI-E2E-001, DESK-001 |
| Folder disappears → nearest parent | UI-UNIT-013, UI-E2E-006, DESK-014 |
| Selection and patterns | UI-UNIT-006/011, UI-KEY-005…007/017, UI-MOUSE-001, UI-E2E-021 |
| Copy / move / conflicts / progress / cancel | OPS-COPY-*, OPS-MOVE-*, OPS-Q-*, CMD-006/007, UI-KEY-012/013, UI-E2E-002…005, DESK-002/003/005 |
| Trash / delete | OPS-DEL-*, OPS-TRASH-*, UI-KEY-015/016, DESK-004 |
| Rename / new folder / compress / permissions | OPS-REN-*, OPS-DIR-001, OPS-ZIP-*, OPS-MODE-001, UI-KEY-009/014, UI-E2E-023 |
| Quick text edit | VFS-TXT-*, CMD-013, UI-CMP-009, UI-KEY-022, UI-E2E-022, DESK-019 |
| Right-click menus, drag and drop | UI-MOUSE-003…008, DESK-015 |
| SFTP sign-in, host keys, ssh config | CON-009…016, CMD-016/017/020, UI-E2E-008…010, DESK-007 |
| FTP/FTPS, WebDAV, S3 | VFS-CON-* ×backend, CON-017…024, UI-E2E-011/012, DESK-008 |
| Secrets in keychain only | CON-001/003/008, CMD-014, SEC-002, DESK-016 |
| Temp names, parallel, resume | OPS-COPY-014…017/020, DESK-017, REL-002 |
| Remote edit | EDIT-*, CMD-021/022, UI-E2E-014, DESK-012/020 |
| Compare & Sync | CMP-*, SYNC-*, CMD-025…029, UI-CMP-007, UI-E2E-015…017, DESK-009 |
| Auto-sync | LIVE-*, CMD-033, UI-E2E-018/019, UI-KEY-030, DESK-010/011 |
| Linked browsing | UI-UNIT-014, UI-KEY-028 |
| Compare Files | DIFF-*, CMD-032, UI-KEY-027, DESK-013 |
| Multi Rename | REN-*, OPS-BREN-001, CMD-030/031, UI-CMP-006, UI-E2E-020 |
| Command palette | UI-CMP-003, UI-KEY-025, UI-E2E-024 |
| Activity & insights | ACT-*, OPS-Q-005, CMD-008, UI-CMP-012, UI-E2E-026 |
| About / Donate / Email | UI-UNIT-016, UI-CMP-013, UI-E2E-027, CMD-011/012 |
| Favorites (local, server, sync) | PLC-*, CON-004, SYNC-013, CMD-010/019/029, UI-E2E-007/013/017, DESK-006 |
| Windows / WSL specifics | VFS-LOC-002/005/006, VFS-WSL-*, UI-E2E-029, DESK-018 |
| Installers & release | INS-* |

### 11.4 Definition of done for the whole plan

- All `P0` and `P1` cases implemented or explicitly deferred with a linked issue.
- CI gates in §9.3 active; nightly running.
- `rg -o "\b[A-Z0-9]+(-[A-Z0-9]+)*-[0-9]{3}\b" docs/testing/AUTOMATION_TEST_PLAN.md | sort -u` vs the same over `crates src-tauri ui e2e-desktop scripts` shows every P0/P1 ID present in test code (add this as `scripts/ci/trace-check.sh`, run in `lint`).
- README "Development" section lists the new test commands.
