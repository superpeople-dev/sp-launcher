# SP Launcher

The desktop launcher for a private **SUPER PEOPLE** server (client build
1.3.0.473797). Players sign in with a personal key, download or pick the
game, and press Play. The launcher points the game at the private backend,
installs the no-Steam fix and keeps itself up to date.

Built with [Tauri 2](https://tauri.app), React 19 and TypeScript on top of a
Rust backend. Windows only.

## Features

- **Key sign-in.** Players get a personal launcher key from the Discord bot
  (`/authkey` or `!authkey`) and enter it once. The key is stored
  DPAPI-encrypted on the PC and never logged. For every launch the launcher
  mints a short-lived login ticket and hands it to the game through the
  environment (`SP_AUTH_TICKET`), never on the command line.
- **One game folder.** The Settings tab and the Download tab share a single
  "Game folder": pick a folder that already has the game, or download into it.
- **Download tab.** Downloads the game from the archive.org item
  `SPShippingDev` (one 27.7 GB 7z). It shows speed and time left, can pause
  and continue (also across restarts), retries automatically, checks free
  space and verifies the MD5 before unpacking with 7-Zip. When it finishes,
  the game folder is set and the archive is deleted (`src-tauri/src/download.rs`).
- **Hosts redirect without running as admin.** The launcher runs as a normal
  user. On the first Play it writes one marked block into the Windows hosts
  file that points the game's `bravohotel.io` hostnames at the backend. This
  takes a single UAC prompt through a small elevated helper
  (`--sp-hosts apply`). The block then stays, so later starts need no prompt.
  Settings can show, set up or remove it (`src-tauri/src/hosts.rs`).
- **No-Steam fix.** Before each launch the embedded `XAPOFX1_5.dll` proxy is
  written into the game's `Win64` folder, so the client does not wait for Steam
  (`src-tauri/src/shim.rs`, `src-tauri/resources/README.md`).
- **Engine.ini patch.** Before each launch, `n.VerifyPeer=False` and related
  settings are applied (`src-tauri/src/engine_ini.rs`).
- **Starts the real game exe.** The launcher starts
  `BravoHotelGame\Binaries\Win64\BravoHotelClient-Win64-Shipping.exe`
  directly, because the root `BravoHotelClient.exe` bootstrapper demands admin
  rights.
- **Auto-updater.** Updates are signed (minisign) and served from the backend
  VPS. See [UPDATING.md](UPDATING.md) and `tools/publish-update.ps1`.
- **Status pill** in the title bar showing players online or "Offline" (the
  backend's public `/launcher/api/status`).
- **Discord Rich Presence**, a **news carousel**, editable **launch arguments**
  and **close/minimize to tray**.

## Client fixes

**Apply client fixes** is on by default and can be switched off in Settings.
The bundle targets development build `1.3.0.473797`: the launcher checks the
game executable before deploying the DLL and the matched signed PAK/`.sig` pair.
The optional debug window is off by default. The no-Steam proxy is separate.

### DLL (`SPClientFixes.dll`)

- Allows class selection in standalone local games by adjusting the local
  player's level when needed.
- Corrects the White and Gold Super Capsule buff IDs.
- Limits the First Blood audio cue to once per local match, including later bot
  kills, without altering kill counts or playing a replacement cue.
- Adjusts standalone bot-match startup requests to 50 AI players. Bot matches
  choose randomly among the twenty 40-50 player blue-zone layouts. 
- Accepts the bundled PAK's signature only at the expected ClientFixes path and
  makes PAK lookups respect mount order, so its cooked asset overrides load
  together. Other PAK signature checks remain in place.

### Signed PAK (`BravoHotelGame-ClientFixes_P.pak`)

- Adds the lower-right **BOT GAME** button to the lobby with the caption
  "50 Bots - Random Blue Zone". Its click calls the game's standalone match
  entry point; the DLL supplies the bot-match settings above.
- Adds localized `Game.locres` catalogs and overrides across 20
  cultures, including English cheat-command descriptions and other menu text.
- Patches cooked UI assets for HUD health, stance, ammo and weapon layout, 
  bringing back the original CBT UI;
- Updated Management and Black Market presentation; and removed the inventory's 
  obsolete Black Market entry. 


### Deployment

Enabled launches deploy all three files for the game session and remove them
after exit. Disabled launches recover recognized leftovers and inject none of
them. The launcher records deployed hashes outside the game folder and holds
an exclusive session lock to recover after interrupted runs. Unknown or
modified files are preserved and block launch rather than being overwritten.
The no-Steam fix remains independent. A saved
`-ExecCmds="PakFile.SearchRecentlyFoundPaks 0"` from older tests is dropped
from launch arguments because the DLL handles ordered lookup; other
`-ExecCmds` values are passed through unchanged.

## Building

Prerequisites: [Rust](https://rustup.rs), Visual Studio Build Tools with the
"Desktop development with C++" workload, and [Node.js](https://nodejs.org).

```powershell
git clone <this-repo-url>
cd sp-launcher
npm install
npm run tauri dev      # development, hot reload
npm run tauri build    # NSIS installer in src-tauri/target/release/bundle/nsis/
```

Three binaries can be embedded at build time and are **not** in the repo. See
[src-tauri/resources/README.md](src-tauri/resources/README.md):

- `src-tauri/resources/XAPOFX1_5.dll` is the no-Steam proxy, built from
  `sp-listen-patch/sp_proxy.cpp`. It is required for a release.
- `src-tauri/resources/SPClientFixes.dll` is built from `client-fixes/` and
  loaded only when the Client fixes toggle is on. It is required for a release.
- `src-tauri/resources/7za.exe` comes from the 7-Zip Extra package. It is
  recommended.

A fresh clone still compiles without them (`build.rs` only warns).

For an unsigned Windows build without an installer, run the
[Unsigned Windows build](.github/workflows/unsigned-build.yml) GitHub Action.
It compiles `SPClientFixes.dll`, embeds it in the Tauri executable, and uploads
both binaries as a workflow artifact. The no-Steam proxy is not built by this
repository, so this artifact does not include it; testing game launch still
requires `XAPOFX1_5.dll` in the game folder.

## Releasing an update

Merging to `main` releases it: the [Release](.github/workflows/release.yml)
workflow builds, signs and publishes a GitHub release with the installer and
the `latest.json` the launcher's updater reads, bumping the patch version by
itself. Its one-time setup (the signing key and a deploy key, as secrets of
the `release` environment) is in [UPDATING.md](UPDATING.md), "Automatic
releases".

By hand, `tools/publish-update.ps1` builds, signs and writes `latest.json`,
then prints the two `scp` lines for the VPS. The signing key lives in
`%USERPROFILE%\.tauri\` and must never be committed; `.gitignore` blocks
`*.key`. The full procedure, including key rotation, is in
[UPDATING.md](UPDATING.md).

## Project layout

```
src/                         React frontend
  App.tsx                    tabs, sign-in gate, launch/stop, updater
  components/                TitleBar (status pill), PlayPanel, DownloadPanel,
                             SettingsPanel, SignIn, News, LaunchArgs
  lib/                       folder picker, formatting, updater glue
  types.ts                   mirrors the Rust structs
  styles.css                 the whole design, one file
src-tauri/src/
  lib.rs                     Tauri commands, app state, tray
  main.rs                    entry; also the elevated "--sp-hosts" helper mode
  auth.rs                    key redeem, DPAPI storage, login tickets
  config.rs                  settings (config.v1.json), atomic writes
  game.rs                    install detection, launching the game
  hosts.rs                   hosts block, one-time elevation
  shim.rs                    no-Steam DLL install
  engine_ini.rs              Engine.ini patch
  download.rs                Download tab: archive.org, MD5, 7-Zip, resume
  discord.rs                 Discord Rich Presence
  news.rs                    news feed
  gateway.rs                 legacy, unused by the UI
tools/
  publish-update.ps1         build + sign + latest.json
  make-update-manifest.mjs   used by the script above
  news.example.json          news feed format
```

## License

No license has been chosen yet. All rights reserved unless a `LICENSE` file
says otherwise. The bundled fonts (Refrigerator Deluxe, `src/assets/fonts/`)
are commercial fonts: check their license before making this repository
public.
