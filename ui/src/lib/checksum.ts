// Helpers for the Checksums dialogs. The file formats mirror crates/dualpal-core/src/checksum.rs.
import type { ChecksumAlgorithm, ChecksumEntry, VerifyEntry } from "./api";

export const CHECKSUM_ALGORITHMS: { id: ChecksumAlgorithm; label: string }[] = [
  { id: "md5", label: "MD5" },
  { id: "sha1", label: "SHA-1" },
  { id: "sha256", label: "SHA-256" },
  { id: "xxh128", label: "XXH128" },
];

/** The algorithm named by a checksum file's extension (`SUMS.md5`), or null. */
export function checksumAlgorithmOf(name: string): ChecksumAlgorithm | null {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  const ext = name.slice(dot + 1).toLowerCase();
  return CHECKSUM_ALGORITHMS.find((a) => a.id === ext)?.id ?? null;
}

/** `name` with its checksum extension, of any algorithm, replaced by the one for `algorithm`. */
export function withChecksumExtension(name: string, algorithm: ChecksumAlgorithm): string {
  const stem = checksumAlgorithmOf(name) ? name.slice(0, name.lastIndexOf(".")) : name.trim();
  return `${stem}.${algorithm}`;
}

/** The name suggested for a checksum file of `names` in a folder called `folderName`. */
export function defaultChecksumName(names: string[], folderName: string, algorithm: ChecksumAlgorithm): string {
  const base = names.length === 1 ? names[0] : folderName || "Checksums";
  return withChecksumExtension(base, algorithm);
}

/** The name of the last part of a folder path, for naming a checksum file. */
export function folderNameOf(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "";
}

/** Checksum lines for the entries that have a hash, in the GNU coreutils layout. */
export function checksumText(entries: ChecksumEntry[]): string {
  return entries
    .filter((e) => e.hash)
    .map((e) => `${e.hash}  ${e.path}\n`)
    .join("");
}

const RANK = { failed: 0, missing: 1, ok: 2 } as const;

/** Failures first, then missing files, then matches; by path within each. */
export function sortVerifyEntries(entries: VerifyEntry[]): VerifyEntry[] {
  return [...entries].sort((a, b) => RANK[a.status] - RANK[b.status] || a.path.localeCompare(b.path));
}
