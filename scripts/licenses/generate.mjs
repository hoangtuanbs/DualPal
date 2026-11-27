#!/usr/bin/env node
// Writes THIRD_PARTY_NOTICES.txt: the license of every third-party Rust crate
// and npm package that ends up inside Dual Pal. The installers ship the file
// and the About window shows it.
//
//   node scripts/licenses/generate.mjs          write the file
//   node scripts/licenses/generate.mjs --check  fail if the file is out of date
//
// Needs cargo-about (cargo install cargo-about --locked --features cli) and
// `npm ci`. Rust crates come from cargo-about with scripts/licenses/about.toml
// (all desktop targets, no dev or build dependencies). npm packages are the
// ones Vite actually bundles into the UI, found by building it once.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const outFile = join(root, "THIRD_PARTY_NOTICES.txt");
const check = process.argv.includes("--check");

// Code that crates compile in from other projects, with its own license that
// the crate's license file does not cover. Path is relative to the crate.
const EMBEDDED = {
  unrar_sys: { name: "UnRAR", license: "UnRAR license (freeware)", file: "vendor/unrar/license.txt" },
};

const OWN = new Set(["dualpal", "dualpal-core", "dualpal-testkit"]);

/** Groups of packages that share one license text. */
const groups = new Map();
function add(license, text, pkg) {
  const body = text.replace(/\r\n?/g, "\n").replace(/[ \t]+$/gm, "").replace(/^\n+/, "").trimEnd();
  const key = `${license}\n${body}`;
  if (!groups.has(key)) groups.set(key, { license, text: body, users: new Set() });
  groups.get(key).users.add(pkg);
}

// --- Rust ---------------------------------------------------------------
function rustCrates() {
  const tmp = mkdtempSync(join(tmpdir(), "dualpal-about-"));
  const json = join(tmp, "about.json");
  try {
    execFileSync(
      "cargo",
      [
        "about", "generate",
        "--config", join(root, "scripts", "licenses", "about.toml"),
        "--manifest-path", join(root, "src-tauri", "Cargo.toml"),
        "--format", "json", "--locked", "--fail",
        "--output-file", json,
      ],
      { cwd: root, stdio: ["ignore", "inherit", "inherit"] },
    );
    return JSON.parse(readFileSync(json, "utf8"));
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

const about = rustCrates();
for (const lic of about.licenses) {
  for (const { crate } of lic.used_by) {
    if (OWN.has(crate.name)) continue;
    add(lic.id, lic.text, `${crate.name} ${crate.version} (Rust crate)`);
  }
}
for (const { package: p } of about.crates) {
  const extra = EMBEDDED[p.name];
  if (!extra) continue;
  const file = join(dirname(p.manifest_path), extra.file);
  if (!existsSync(file)) throw new Error(`${p.name} ${p.version}: ${extra.file} not found`);
  add(extra.license, readFileSync(file, "latin1"), `${extra.name}, compiled in by ${p.name} ${p.version}`);
}

// --- npm (what Vite bundles into the UI) ---------------------------------
async function bundledPackages() {
  const { build } = await import("vite");
  const dirs = new Set();
  const outDir = mkdtempSync(join(tmpdir(), "dualpal-ui-"));
  try {
    await build({
      configFile: join(root, "vite.config.ts"),
      logLevel: "warn",
      build: { outDir, emptyOutDir: true, write: false },
      plugins: [
        {
          name: "dualpal-list-packages",
          generateBundle(_options, bundle) {
            for (const chunk of Object.values(bundle)) {
              for (const id of Object.keys(chunk.modules ?? {})) {
                const path = id.replace(/^\0/, "").split("?")[0].split("/").join(sep);
                const marker = `${sep}node_modules${sep}`;
                const at = path.lastIndexOf(marker);
                if (at < 0) continue;
                const rest = path.slice(at + marker.length).split(sep);
                const name = rest[0].startsWith("@") ? `${rest[0]}${sep}${rest[1]}` : rest[0];
                dirs.add(path.slice(0, at + marker.length) + name);
              }
            }
          },
        },
      ],
    });
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
  return [...dirs];
}

const LICENSE_FILE = /^(licen[cs]e|copying|notice)(\.|-|$)/i;
for (const dir of await bundledPackages()) {
  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  if (OWN.has(pkg.name)) continue;
  const license = typeof pkg.license === "string" ? pkg.license : pkg.license?.type ?? "UNKNOWN";
  const files = readdirSync(dir).filter((f) => LICENSE_FILE.test(f)).sort();
  if (files.length === 0) throw new Error(`${pkg.name} ${pkg.version}: no license file to include`);
  const text = files.map((f) => readFileSync(join(dir, f), "utf8").trim()).join("\n\n");
  add(license, text, `${pkg.name} ${pkg.version} (npm package)`);
}

// --- Write ---------------------------------------------------------------
const rule = "-".repeat(80);
const sorted = [...groups.values()]
  .map((g) => ({ ...g, users: [...g.users].sort((a, b) => a.localeCompare(b, "en")) }))
  .sort((a, b) => a.license.localeCompare(b.license, "en") || a.users[0].localeCompare(b.users[0], "en"));

const count = new Set(sorted.flatMap((g) => g.users)).size;
let out =
  `Dual Pal third-party notices\n\n` +
  `Dual Pal includes the ${count} third-party components listed below, each\n` +
  `under its own license. Their source code is available from crates.io and\n` +
  `npmjs.com under the names and versions shown.\n\n` +
  `This file is generated by scripts/licenses/generate.mjs; do not edit it.\n`;
for (const g of sorted) {
  out += `\n${rule}\n${g.license}\n\nUsed by:\n${g.users.map((u) => `  ${u}`).join("\n")}\n\n${g.text}\n`;
}

// The check compares the component lists only: license texts found by
// cargo-about's online lookup can change wording without any dependency change.
const components = (text) => text.split("\n").filter((l) => /^  \S/.test(l)).sort();
if (check) {
  const current = existsSync(outFile) ? readFileSync(outFile, "utf8") : "";
  const want = components(out);
  const have = components(current);
  const missing = want.filter((c) => !have.includes(c));
  const extra = have.filter((c) => !want.includes(c));
  if (missing.length || extra.length) {
    for (const c of missing) console.error(`missing:${c}`);
    for (const c of extra) console.error(`no longer used:${c}`);
    console.error("THIRD_PARTY_NOTICES.txt is out of date. Run: node scripts/licenses/generate.mjs");
    process.exit(1);
  }
  console.log(`THIRD_PARTY_NOTICES.txt is up to date (${count} components).`);
} else {
  writeFileSync(outFile, out);
  console.log(`Wrote THIRD_PARTY_NOTICES.txt (${count} components, ${sorted.length} license texts).`);
}
