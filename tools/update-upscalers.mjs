// Regenerates src-tauri/resources/upscalers.json, the DLSS / DLSS Frame Generation / XeSS builds
// the launcher recognises as a player's own swap (src-tauri/src/upscalers.rs), from DLSS
// Swapper's catalogue. Run it when new builds come out, then commit the file:
//
//   node tools/update-upscalers.mjs
//
// Only builds with a valid vendor signature are taken. Being in the list says what a file is,
// not that this game build works with it: the player's tool chooses, installs and restores.
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const SOURCE = 'https://raw.githubusercontent.com/beeradmoore/dlss-swapper-manifest-builder/refs/heads/main/manifest.json';
const KINDS = ['dlss', 'dlss_g', 'xess'];
const out = fileURLToPath(new URL('../src-tauri/resources/upscalers.json', import.meta.url));

const res = await fetch(SOURCE);
if (!res.ok) throw new Error(`${SOURCE}: HTTP ${res.status}`);
const manifest = await res.json();
const lines = [`{"source":${JSON.stringify(SOURCE)},"generated":${JSON.stringify(new Date().toISOString().slice(0, 10))},`];
KINDS.forEach((kind, i) => {
  const builds = (manifest[kind] || [])
    .filter((b) => b.is_signature_valid && /^[0-9A-F]{32}$/i.test(b.md5_hash) && Number(b.file_size) > 0)
    .map((b) => [String(b.version), b.md5_hash.toUpperCase(), Number(b.file_size)]);
  if (!builds.length) throw new Error(`no ${kind} builds in the catalogue`);
  lines.push(`${JSON.stringify(kind)}:[`);
  builds.forEach((b, j) => lines.push(JSON.stringify(b) + (j < builds.length - 1 ? ',' : '')));
  lines.push(i < KINDS.length - 1 ? '],' : ']}');
  console.log(`${kind}: ${builds.length} builds`);
});
writeFileSync(out, lines.join('\n') + '\n');
console.log(`wrote ${out}`);
