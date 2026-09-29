#!/usr/bin/env node
/**
 * Build the `latest.json` the launcher's auto-updater polls (see
 * `plugins.updater.endpoints` in src-tauri/tauri.conf.json).
 *
 * Run this after `bun run tauri build` with the signing env vars set (see
 * RUNNING.md's "Setting up the auto-updater" section) — the build produces
 * both the NSIS installer and a `.sig` file next to it. This script just
 * bundles the version, the signature, and the installer's hosted URL into
 * the JSON shape the updater expects.
 *
 *   node tools/make-update-manifest.mjs \
 *     --version 0.2.0 \
 *     --sig "src-tauri/target/release/bundle/nsis/SP Launcher_0.2.0_x64-setup.exe.sig" \
 *     --url "https://files.example.com/sp/updates/SP Launcher_0.2.0_x64-setup.exe" \
 *     --notes "Fixed the connect prompt, added download resume." \
 *     --out latest.json
 *
 * Upload the installer and this latest.json to the same host, then make sure
 * `latest.json` is at the exact URL your `tauri.conf.json` endpoints list.
 */

import { readFile, writeFile } from "node:fs/promises";

function parseArgs(argv) {
  const out = {};
  for (let i = 2; i < argv.length; i += 2) {
    out[argv[i].replace(/^--/, "")] = argv[i + 1];
  }
  return out;
}

const args = parseArgs(process.argv);
const { version, sig, url, out } = args;
const notes = args.notes ?? "";

if (!version || !sig || !url) {
  console.error(
    "usage: make-update-manifest.mjs --version X.Y.Z --sig <path to .sig> --url <installer URL> [--notes \"...\"] [--out latest.json]"
  );
  process.exit(1);
}

const signature = (await readFile(sig, "utf8")).trim();

const manifest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": { signature, url },
  },
};

const outPath = out ?? "latest.json";
await writeFile(outPath, JSON.stringify(manifest, null, 2));
console.log(`wrote ${outPath}`);
console.log(`  version ${version}`);
console.log(`  installer: ${url}`);
