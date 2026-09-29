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
- **Client fixes (on by default).** A separate `SPClientFixes.dll` is loaded only when
  enabled in Settings. Its source is in `client-fixes/src/`, and the fixes
  are listed in [`client-fixes/`](client-fixes/README.md). Its debug window is
  off by default and can be enabled separately in Settings. It currently includes:
  - **Local class selection:** allows players to choose a class in local games.
  - **Super Capsules:** allows White and Gold Super Capsules to work when used.
  - **First Blood:** plays the announcement once per local match.
  - **Standalone bot matches:** requests 50 AI players with a 0.5-second spawn
    timer and adjusted startup settings for the signed PAK's Bot Game button.
  - **Cheat Widget:** translates its Korean command labels to English.
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

`tools/publish-update.ps1` builds, signs and writes `latest.json`, then prints
the two `scp` lines for the VPS: upload the installer first, then
`latest.json`. The signing key lives in `%USERPROFILE%\.tauri\` and must never
be committed; `.gitignore` blocks `*.key`. The full procedure, including key
rotation, is in [UPDATING.md](UPDATING.md). Bump the version in
`package.json`, `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`
together.

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

With Client fixes on, the DLL makes PAK lookups follow mount priority (same
effect as `PakFile.SearchRecentlyFoundPaks 0`). A saved
`-ExecCmds="PakFile.SearchRecentlyFoundPaks 0"` from older test builds is dropped
from the launch arguments. Other `-ExecCmds` values are passed through unchanged.

## Client fixes resources and lifecycle

Client fixes bundles the DLL, translated PAK and signature together. Enabled launches deploy all three before starting the supported game build and remove them after exit. Disabled launches recover recognized leftovers and inject none of these resources. Steam integration remains independent.

The launcher records deployed hashes outside the game folder and holds an exclusive session lock. It recovers completed files after an interrupted launcher session. Unknown or modified files are preserved and block launch; a file interrupted halfway through writing requires manual inspection. The exact artifacts from the successful preservation test and the DLL shipped with launcher 0.3.2 are recognized for migration.

Translations live entirely in the PAK. The DLL retains scoped signature verification and the validated ordered-lookup patch; translation functions, cache polling and temporary lookup diagnostics have been removed. Ordered lookup can scan more archives than the game's recent-archive shortcut. GitHub Actions checks the bundled signature and deployment lifecycle before building the executable.
