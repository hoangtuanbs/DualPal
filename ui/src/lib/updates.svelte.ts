// The updater state shown by the update bar, the About dialog and the
// command palette. A new release is found on GitHub Releases (the GitHub API's
// latest release), compared with the running version and announced. When the
// Tauri updater can install it (a signed latest.json is published, see
// tauri.conf.json) the bar offers "Install and restart"; otherwise it links
// to the release page to download the installer.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { api, errorMessage, type ReleaseInfo, type Settings } from "./api";
import { isCheckDue, isNewerVersion, progressPercent, releaseNotesUrl } from "./updates";

/** How often an installed update looks again for file work to finish before restarting. */
const RESTART_POLL_MS = 500;

export type UpdatePhase = "idle" | "checking" | "available" | "downloading" | "installing" | "upToDate" | "error";

export type CheckOutcome = { kind: "available"; version: string; installable: boolean } | { kind: "upToDate" };

/** A newer release: installable in the app when `update` is set, else a download link. */
type Found = { version: string; update: Update | null; release: ReleaseInfo | null };

class Updates {
  phase = $state<UpdatePhase>("idle");
  /** The release on offer. Raw: the plugin's resource is not app state to proxy. */
  update = $state.raw<Update | null>(null);
  /** The newest release on GitHub, when it is newer than this version. */
  release = $state.raw<ReleaseInfo | null>(null);
  /** The version on offer, from either source. */
  version = $state<string | null>(null);
  /** The version running now. */
  currentVersion = $state<string | null>(null);
  /** Whether the app looks for a release by itself once a day. */
  autoCheck = $state(true);
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

  /** Read the automatic-check switch from the settings. */
  async loadPreferences(): Promise<Settings> {
    const settings = await api.settings();
    this.autoCheck = settings.updateChecks !== false;
    return settings;
  }

  /**
   * The automatic check, shortly after start-up and then while the app stays
   * open: at most once a day, silent, and off when the person turned it off.
   * A release they chose to skip is not announced. A failure (no network,
   * GitHub unreachable) is only logged.
   */
  async checkAutomatically(now = Date.now()) {
    try {
      const settings = await this.loadPreferences();
      if (!this.autoCheck || !isCheckDue(settings.lastUpdateCheck, now)) return;
      // Leave an offer or an install that is already on screen alone.
      if ((this.phase === "available" && !this.dismissed) || this.phase === "downloading" || this.phase === "installing") return;
      // Record the attempt first, so a failing check is not retried on every start.
      await api.saveSettings({ ...settings, lastUpdateCheck: now });
      const found = await this.#find();
      if (found && found.version !== settings.skippedVersion) this.#offer(found);
    } catch (e) {
      console.warn("Update check failed:", errorMessage(e));
    }
  }

  /** A check the person asked for. It also shows a skipped release. Throws when the check fails. */
  async checkNow(): Promise<CheckOutcome> {
    // An install is under way: leave it alone and report what it is installing.
    if (this.update && (this.phase === "downloading" || this.phase === "installing")) {
      return { kind: "available", version: this.update.version, installable: true };
    }
    this.phase = "checking";
    this.message = null;
    try {
      const found = await this.#find();
      if (!found) {
        this.phase = "upToDate";
        this.dismissed = false;
        return { kind: "upToDate" };
      }
      this.#offer(found);
      return { kind: "available", version: found.version, installable: found.update !== null };
    } catch (e) {
      this.phase = "error";
      this.message = `Could not check for updates: ${errorMessage(e)}`;
      this.dismissed = false;
      throw e;
    }
  }

  /** Turn the automatic check on or off. */
  async setAutoCheck(on: boolean) {
    const settings = await api.settings();
    await api.saveSettings({ ...settings, updateChecks: on });
    this.autoCheck = on;
  }

  /** Do not announce the release on offer again. A newer one is announced as usual. */
  async skip() {
    const version = this.version;
    this.dismiss();
    if (!version) return;
    try {
      const settings = await api.settings();
      await api.saveSettings({ ...settings, skippedVersion: version });
    } catch (e) {
      this.message = `Could not save the choice: ${errorMessage(e)}`;
      this.phase = "error";
      this.dismissed = false;
    }
  }

  /** Open the release page, where the installers are. */
  async openReleasePage() {
    const url = this.release?.url ?? (this.version ? releaseNotesUrl(this.version) : null);
    if (url) await api.openUrl(url);
  }

  /**
   * Look for a release newer than this one. GitHub's latest release decides
   * whether there is one; the updater plugin then says whether it can be
   * installed in place. When GitHub cannot be reached, the plugin alone decides.
   */
  async #find(): Promise<Found | null> {
    let release: ReleaseInfo | null = null;
    let releaseError: unknown = null;
    try {
      release = await api.latestRelease();
    } catch (e) {
      releaseError = e;
    }
    const current = (await api.aboutInfo()).version;
    this.currentVersion = current;
    if (release && !isNewerVersion(release.version, current)) return null;

    let update: Update | null = null;
    try {
      update = await check();
    } catch (e) {
      // No signed latest.json yet: the release page is the way to upgrade.
      if (!release) throw releaseError ?? e;
    }
    if (update) return { version: update.version, update, release };
    if (release) return { version: release.version, update: null, release };
    return null;
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

  #offer(found: Found) {
    this.update = found.update;
    this.release = found.release;
    this.version = found.version;
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
