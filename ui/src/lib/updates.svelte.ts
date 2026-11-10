// The updater state shown by the update bar, the About dialog and the
// command palette. Checks and installs use the Tauri updater plugin; the
// release comes from GitHub Releases (see tauri.conf.json).
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { api, errorMessage } from "./api";
import { isCheckDue, progressPercent } from "./updates";

/** How often an installed update looks again for file work to finish before restarting. */
const RESTART_POLL_MS = 500;

export type UpdatePhase = "idle" | "checking" | "available" | "downloading" | "installing" | "upToDate" | "error";

export type CheckOutcome = { kind: "available"; version: string } | { kind: "upToDate" };

class Updates {
  phase = $state<UpdatePhase>("idle");
  /** The release on offer. Raw: the plugin's resource is not app state to proxy. */
  update = $state.raw<Update | null>(null);
  /** Text for the bar: the result of a manual check, or why an install failed. */
  message = $state<string | null>(null);
  progress = $state<number | null>(null);
  /** Set by "Later" or the close button. Cleared by the next check. */
  dismissed = $state(false);
  /** An install was asked for while file operations are running. */
  waitingForOperations = $state(false);

  /** The update whose download finished, so a retry does not fetch it again. */
  #downloaded: Update | null = null;
  #busy: () => number = () => 0;

  /** How many file operations are running; the app sets this once. */
  setBusyCheck(count: () => number) {
    this.#busy = count;
  }

  /**
   * The check at start-up: at most once a day, and silent. A failure (no
   * network, no release yet, no signing key) is only logged.
   */
  async checkOnStartup(now = Date.now()) {
    try {
      const settings = await api.settings();
      if (!isCheckDue(settings.lastUpdateCheck, now)) return;
      // Record the attempt first, so a failing check is not retried on every start.
      await api.saveSettings({ ...settings, lastUpdateCheck: now });
      const update = await check();
      if (update) this.#offer(update);
    } catch (e) {
      console.warn("Update check failed:", errorMessage(e));
    }
  }

  /** A check the person asked for. Throws when the check fails. */
  async checkNow(): Promise<CheckOutcome> {
    // An install is under way: leave it alone and report what it is installing.
    if (this.update && (this.phase === "downloading" || this.phase === "installing")) {
      return { kind: "available", version: this.update.version };
    }
    this.phase = "checking";
    this.message = null;
    try {
      const update = await check();
      if (!update) {
        this.phase = "upToDate";
        this.dismissed = false;
        return { kind: "upToDate" };
      }
      this.#offer(update);
      return { kind: "available", version: update.version };
    } catch (e) {
      this.phase = "error";
      this.message = `Could not check for updates: ${errorMessage(e)}`;
      this.dismissed = false;
      throw e;
    }
  }

  dismiss() {
    this.dismissed = true;
    this.waitingForOperations = false;
    if (this.phase === "upToDate" || this.phase === "error") this.phase = "idle";
  }

  /** Leave the wait for file operations: the update stays on offer. */
  keepWaiting() {
    this.waitingForOperations = false;
  }

  /**
   * Download, install and restart. Unless `confirmed`, waits while file
   * operations are running and asks the person first.
   */
  async install(confirmed = false) {
    const update = this.update;
    if (!update || this.phase === "downloading" || this.phase === "installing") return;
    this.dismissed = false;
    this.message = null;
    try {
      if (this.#holdForOperations(confirmed)) return;
      if (this.#downloaded !== update) {
        this.phase = "downloading";
        this.progress = 0;
        let total: number | null = null;
        let done = 0;
        await update.download((event) => {
          if (event.event === "Started") total = event.data.contentLength ?? null;
          else if (event.event === "Progress") {
            done += event.data.chunkLength;
            this.progress = progressPercent(done, total);
          }
        });
        this.#downloaded = update;
      }
      // Operations may have started while the download ran.
      if (this.#holdForOperations(confirmed)) return;
      this.phase = "installing";
      await update.install();
      // Work that began while the installer ran finishes before the restart.
      while (!confirmed && this.#busy() > 0) await new Promise((r) => setTimeout(r, RESTART_POLL_MS));
      // macOS and Linux: the new version starts after a relaunch. Windows
      // quits while its installer runs, so this line is not reached there.
      await relaunch();
      // Only reached if the restart did not happen: do not stay "installing".
      this.phase = "idle";
    } catch (e) {
      this.phase = "error";
      this.message = `The update failed: ${errorMessage(e)}`;
    }
  }

  #offer(update: Update) {
    this.update = update;
    this.phase = "available";
    this.message = null;
    this.progress = null;
    this.dismissed = false;
    this.waitingForOperations = false;
    this.#downloaded = null;
  }

  #holdForOperations(confirmed: boolean): boolean {
    if (confirmed || this.#busy() === 0) {
      this.waitingForOperations = false;
      return false;
    }
    this.waitingForOperations = true;
    this.phase = "available";
    return true;
  }
}

export const updates = new Updates();
