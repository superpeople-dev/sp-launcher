#!/usr/bin/env node
/**
 * Set the launcher's version everywhere it is written, before a build:
 *
 *   node tools/set-version.mjs 0.3.4
 *
 * The release workflow (.github/workflows/release.yml) runs it with the
 * version it is about to publish, so the installer, the updater manifest and
 * the version shown in Settings all agree with the GitHub release's tag.
 */

import { readFile, writeFile } from "node:fs/promises";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("usage: set-version.mjs X.Y.Z");
  process.exit(1);
}

async function setJson(path, set) {
  const data = JSON.parse(await readFile(path, "utf8"));
  set(data);
  await writeFile(path, JSON.stringify(data, null, 2) + "\n");
}

await setJson("src-tauri/tauri.conf.json", (conf) => {
  conf.version = version;
});
await setJson("package.json", (pkg) => {
  pkg.version = version;
});
await setJson("package-lock.json", (lock) => {
  lock.version = version;
  if (lock.packages?.[""]) lock.packages[""].version = version;
});

// Only the [package] table's version, not a dependency's.
const cargoPath = "src-tauri/Cargo.toml";
const cargo = await readFile(cargoPath, "utf8");
const pattern = /(\[package\][^[]*?\bversion\s*=\s*")[^"]*(")/;
if (!pattern.test(cargo)) {
  console.error(`no [package] version in ${cargoPath}`);
  process.exit(1);
}
await writeFile(cargoPath, cargo.replace(pattern, `$1${version}$2`));

console.log(`version ${version}`);
