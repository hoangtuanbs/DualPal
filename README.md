# Dual Pal

A fast, cross-platform dual-pane file manager for **macOS and Windows** (Linux works too), built with [Tauri 2](https://tauri.app): a Rust core and a Svelte web UI.

> Status: early development. Step 1 (foundation) is in place: local browsing in two panes, background copy / move / delete / rename / new folder with live progress, cancellation and an activity history.

## Layout

```
crates/dualpal-core   Platform-independent Rust core (no Tauri dependency)
  src/vfs/            Virtual file system: FileSystem trait, registry, local disk backend
  src/ops/            Operation queue: copy, move, delete, rename, create folder
  src/activity.rs     Persistent activity log (JSON lines)
src-tauri/            Tauri shell: commands + progress events wiring the core to the UI
ui/                   Svelte 5 + TypeScript front end (built with Vite)
```

### Core concepts

- **`FileSystem`** — every browsable place (local disk now; SFTP and S3 next) implements this async trait. A **`Location`** is `{ fs, path }`, so the UI, the operation queue and the upcoming compare/sync engine work the same across local and remote storage. Copies between different file systems stream through the core.
- **`OperationQueue`** — operations run in the background (two at a time by default), report progress (items, bytes, current file) as events, can be cancelled, and clean up partial files. Conflicts are handled per operation: `fail` (default), `skip` or `overwrite`.
- **`ActivityLog`** — every finished operation is recorded and kept across restarts (`activity.jsonl` in the app data folder).

## Development

Prerequisites: [Rust](https://rustup.rs) (stable), Node.js 22, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS (Xcode Command Line Tools on macOS; WebView2 + MSVC build tools on Windows).

```sh
npm install
npm run tauri dev        # run the app with hot reload
npm run tauri build      # build an installer (.dmg/.app on macOS, .msi/.exe on Windows)
cargo test --workspace   # core tests
npm run check            # UI type-check
```

## Keyboard

| Key | Action |
| --- | --- |
| Tab | Switch pane |
| ↑ ↓ / Page Up / Page Down / Home / End | Move cursor |
| Enter | Open folder, or open file in its default app |
| Backspace, Ctrl/⌘ ↑ | Enclosing folder |
| Space / Insert | Mark item |
| Ctrl/⌘ A | Mark all |
| F2 | Rename |
| F5 / F6 | Copy / move to the other pane |
| F7 | New folder |
| F8 / Delete | Move to Trash |
| Shift F8 / Shift Delete | Delete permanently (asks first) |
| + / − | Select / deselect by name pattern (e.g. `*.jpg; *.png`) |
| Ctrl/⌘ 1 / Ctrl/⌘ 2 | List / icon view |
| Ctrl/⌘ T, Ctrl/⌘ W | New tab / close tab |
| Ctrl Tab, Ctrl Shift Tab | Next / previous tab |
| Ctrl/⌘ R | Refresh |
| Ctrl/⌘ Shift . | Show hidden files |

## Roadmap

1. **Foundation** — Tauri app, VFS layer, operation queue, activity log. ✅
2. **Dual-pane browser** — tabs, list/icon views, sidebar (devices, favorites), select by pattern or tag, compress, preview panel (audio/video, images, PDF, quick text edit), Move to Trash.
3. **Remote connections** — SFTP, then S3, each with nested favorites; edit remote files in your preferred editor with automatic upload on save.
4. **Compare & power tools** — folder compare and one/two-way sync (saved as favorites), synchronized browsing, external file compare tools, multi-rename presets, command palette.

## Test builds

Every push to `main` and every pull request runs the **Build installers** workflow (it can also be started by hand from the Actions tab). Download the installer from the run's **Artifacts** section:

- **macOS** — `DualPal-macOS` contains a `.dmg` that runs on Apple Silicon and Intel. The app is not notarized yet, so after dragging it to Applications run `xattr -cr "/Applications/Dual Pal.app"` once (or right-click → Open) to get past Gatekeeper.
- **Windows** — `DualPal-Windows` contains an `.msi` and a setup `.exe`. SmartScreen may warn about an unknown publisher; choose *More info → Run anyway*.
- **Linux** — `DualPal-Linux` contains an `.AppImage` and a `.deb`.
