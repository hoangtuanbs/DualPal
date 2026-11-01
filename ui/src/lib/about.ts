// Who makes Dual Pal, where to get help and how to donate.
import { api, type AboutInfo } from "./api";

export const ABOUT = {
  author: "hoangtuanbs",
  companies: ["DanaExperts Co. Ltd.", "DanaExperts Finland Oy"],
  projectUrl: "https://github.com/hoangtuanbs/DualPal",
  supportEmail: "hoangtuanbs@proton.me",
  ethereumAddress: "0xF8352b493e7750C03bde4bdA94A203B010933735",
};

const OS_NAMES: Record<string, string> = { macos: "macOS", windows: "Windows", linux: "Linux" };

export function osName(info: AboutInfo) {
  return `${OS_NAMES[info.os] ?? info.os} (${info.arch})`;
}

/** A new email to support with the app version and OS filled in. */
export function supportMailUrl(info: AboutInfo) {
  const subject = `Dual Pal ${info.version} support`;
  const body = `Describe the problem here:\n\n\n---\nDual Pal ${info.version}\n${osName(info)}`;
  return `mailto:${ABOUT.supportEmail}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`;
}

export async function emailSupport() {
  await api.openUrl(supportMailUrl(await api.aboutInfo()));
}

export function openProjectPage() {
  return api.openUrl(ABOUT.projectUrl);
}

/** Copy text to the clipboard. Returns false when the webview refuses. */
export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
