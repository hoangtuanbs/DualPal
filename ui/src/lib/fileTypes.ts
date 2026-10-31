import { convertFileSrc } from "@tauri-apps/api/core";
import type { Entry, Location } from "./api";

export type FileCategory = "folder" | "image" | "video" | "audio" | "pdf" | "archive" | "code" | "text" | "app" | "file";

const EXT: Record<string, FileCategory> = {};
const add = (cat: FileCategory, exts: string) => exts.split(" ").forEach((e) => (EXT[e] = cat));
add("image", "png jpg jpeg gif webp bmp svg ico heic heif tif tiff avif");
add("video", "mp4 mov m4v mkv webm avi wmv flv mpg mpeg");
add("audio", "mp3 m4a aac wav flac ogg opus aiff alac wma");
add("pdf", "pdf");
add("archive", "zip tar gz tgz bz2 xz 7z rar dmg iso");
add("code", "js ts tsx jsx svelte vue rs go py rb php java kt swift c h cpp hpp cs sh zsh json yaml yml toml xml html css scss sql");
add("text", "txt md markdown rtf csv log ini conf cfg");
add("app", "app exe msi pkg deb rpm appimage");

export function extension(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toLowerCase() : "";
}

export function category(entry: Entry): FileCategory {
  if (entry.kind === "dir") return extension(entry.name) === "app" ? "app" : "folder";
  return EXT[extension(entry.name)] ?? "file";
}

const GLYPH: Record<FileCategory, string> = {
  folder: "📁",
  image: "🖼️",
  video: "🎬",
  audio: "🎵",
  pdf: "📕",
  archive: "📦",
  code: "📜",
  text: "📄",
  app: "🧩",
  file: "📄",
};

export function glyph(entry: Entry): string {
  return GLYPH[category(entry)];
}

/** Browsers can render these directly as <img>. */
const THUMBNAILABLE = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif"]);

/** URL for an image thumbnail, or null when the file can't be shown inline. */
export function thumbnailUrl(location: Location | null, entry: Entry): string | null {
  if (!location || location.fs !== "local" || entry.kind !== "file") return null;
  if (!THUMBNAILABLE.has(extension(entry.name))) return null;
  if (entry.size > 50_000_000) return null;
  return convertFileSrc(entry.path);
}

/** Convert a wildcard pattern list ("*.jpg; *.png") into a name matcher. */
export function patternMatcher(patterns: string): (name: string) => boolean {
  const regexes = patterns
    .split(/[;,]/)
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const body = p.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".");
      return new RegExp(`^${body}$`, "i");
    });
  return (name) => regexes.some((r) => r.test(name));
}
