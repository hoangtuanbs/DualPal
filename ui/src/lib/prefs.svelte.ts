// The person's settings as the UI uses them: loaded once, changed through
// the Settings screen, applied at once (theme, text size, row density on the
// document; key bindings for the keyboard) and saved field by field.

import { api, type Settings } from "./api";
import { effectiveBindings } from "./keymap";

/** Theme, text size and row density as attributes of the root element, for
 *  app.css. "System" leaves the theme to prefers-color-scheme. */
export function applyAppearance(s: Settings, root: HTMLElement | undefined = globalThis.document?.documentElement) {
  if (!root) return;
  if (s.theme === "light" || s.theme === "dark") root.dataset.theme = s.theme;
  else delete root.dataset.theme;
  if (s.fontSize === "small" || s.fontSize === "large") root.dataset.fontSize = s.fontSize;
  else delete root.dataset.fontSize;
  if (s.density === "compact") root.dataset.density = "compact";
  else delete root.dataset.density;
}

class Prefs {
  settings = $state<Settings>({ editor: null, diffTool: null });
  /** Every command's keys: the defaults with the person's changes on top. */
  bindings = $derived(effectiveBindings(this.settings.keyBindings));

  /** Read the saved settings and apply them. */
  async load(): Promise<Settings> {
    this.settings = await api.settings();
    applyAppearance(this.settings);
    return this.settings;
  }

  /** Change some settings: applied at once, then saved. On a failed save the
   *  saved settings are read back and the error is thrown. */
  async update(patch: Partial<Settings>): Promise<void> {
    this.settings = { ...this.settings, ...patch };
    applyAppearance(this.settings);
    const seq = ++this.#seq;
    try {
      const saved = await api.updateSettings(patch);
      // A later change is on its way; its answer will include this one.
      if (seq === this.#seq) this.settings = saved;
    } catch (e) {
      await this.load().catch(() => {});
      throw e;
    }
    applyAppearance(this.settings);
  }

  #seq = 0;
}

export const prefs = new Prefs();
