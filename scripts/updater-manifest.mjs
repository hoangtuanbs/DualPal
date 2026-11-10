#!/usr/bin/env node
// Release helper for the in-app updater. No dependencies.
//
//   node scripts/updater-manifest.mjs config
//       Prints the `tauri build --config` override that turns on updater
//       artifacts and sets the public key (TAURI_UPDATER_PUBKEY).
//
//   node scripts/updater-manifest.mjs manifest --dir installers --tag v0.2.0 \
//       --repo owner/name [--out latest.json] [--notes "text"]
//       Writes latest.json, the static file the app reads from
//       https://github.com/<repo>/releases/latest/download/latest.json.
//       Every updater artifact (<file> plus <file>.sig) found under --dir
//       becomes one platform entry.

import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/** Tauri's platform keys for the files each build produces. */
export function platformKeysFor(fileName) {
  // The macOS build is universal: one archive serves both Macs.
  if (fileName.endsWith(".app.tar.gz")) return ["darwin-aarch64", "darwin-x86_64"];
  if (fileName.endsWith(".AppImage")) return ["linux-x86_64"];
  // The NSIS installer is the updater's Windows file. MSI packages are not used.
  if (fileName.toLowerCase().endsWith(".exe")) return ["windows-x86_64"];
  return [];
}

/** The name GitHub gives an uploaded asset: spaces become dots. */
export function assetName(fileName) {
  return fileName.replace(/\s+/g, ".");
}

/**
 * Builds the Tauri static updater manifest.
 * `entries` are { fileName, signature }: the updater file's name and the
 * text of its .sig file.
 */
export function buildManifest({ version, notes = "", pubDate, releaseUrl, entries }) {
  const platforms = {};
  for (const { fileName, signature } of entries) {
    for (const key of platformKeysFor(fileName)) {
      if (platforms[key]) {
        throw new Error(`two updater files for ${key}: ${platforms[key].url} and ${fileName}`);
      }
      platforms[key] = {
        signature: signature.trim(),
        url: `${releaseUrl}/${assetName(fileName)}`,
      };
    }
  }
  if (Object.keys(platforms).length === 0) {
    throw new Error("no updater artifacts found (expected .app.tar.gz, .AppImage or .exe with a .sig)");
  }
  return { version, notes, pub_date: pubDate, platforms };
}

/** Pairs each updater file in `names` with its `.sig` file. Returns the file names that have one. */
export function pairSignatures(names) {
  const set = new Set(names);
  return names.filter((name) => !name.endsWith(".sig") && set.has(`${name}.sig`));
}

/** Every file under `dir`, as paths relative to it. */
async function listFiles(dir, prefix = "") {
  const out = [];
  for (const entry of await readdir(join(dir, prefix), { withFileTypes: true })) {
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...(await listFiles(dir, rel)));
    else out.push(rel);
  }
  return out;
}

/** Reads the updater files and their signatures from a downloaded-artifacts folder. */
export async function readEntries(dir) {
  const paths = new Map(); // file name -> path under dir
  for (const rel of await listFiles(dir)) {
    const name = rel.split("/").pop();
    if (paths.has(name)) throw new Error(`the same file name is in two folders: ${paths.get(name)} and ${rel}`);
    paths.set(name, rel);
  }
  const entries = [];
  for (const name of pairSignatures([...paths.keys()])) {
    const signature = await readFile(join(dir, paths.get(`${name}.sig`)), "utf8");
    entries.push({ fileName: name, signature });
  }
  return entries;
}

/** The --config override for `tauri build`: updater artifacts on, with the real public key. */
export function updaterBuildConfig(pubkey) {
  if (!pubkey || !pubkey.trim()) {
    throw new Error("TAURI_UPDATER_PUBKEY is empty: set the repository variable to the public key from `tauri signer generate`");
  }
  return { bundle: { createUpdaterArtifacts: true }, plugins: { updater: { pubkey: pubkey.trim() } } };
}

function parseArgs(argv) {
  const args = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const value = argv[i + 1];
      if (value === undefined || value.startsWith("--")) throw new Error(`missing value for ${a}`);
      args[a.slice(2)] = value;
      i++;
    } else {
      args._.push(a);
    }
  }
  return args;
}

async function main(argv) {
  const args = parseArgs(argv);
  const command = args._[0];
  if (command === "config") {
    process.stdout.write(JSON.stringify(updaterBuildConfig(process.env.TAURI_UPDATER_PUBKEY)));
    return;
  }
  if (command === "manifest") {
    for (const key of ["dir", "tag", "repo"]) {
      if (!args[key]) throw new Error(`--${key} is required`);
    }
    const version = args.tag.replace(/^v/, "");
    const manifest = buildManifest({
      version,
      notes: args.notes ?? "",
      pubDate: new Date().toISOString(),
      releaseUrl: `https://github.com/${args.repo}/releases/download/${args.tag}`,
      entries: await readEntries(args.dir),
    });
    await writeFile(args.out ?? "latest.json", `${JSON.stringify(manifest, null, 2)}\n`);
    console.log(`wrote ${args.out ?? "latest.json"} with ${Object.keys(manifest.platforms).join(", ")}`);
    return;
  }
  throw new Error("usage: updater-manifest.mjs config | manifest --dir <dir> --tag <tag> --repo <owner/name> [--out <file>] [--notes <text>]");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((e) => {
    console.error(`::error::${e.message}`);
    process.exitCode = 1;
  });
}
