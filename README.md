# SP Launcher

The desktop launcher for a private **SUPER PEOPLE** server (client build
1.3.0.473797). Players sign in with a personal key, download or pick the
game, and press Play. The launcher points the game at the private backend,
installs the no-Steam fix and keeps itself up to date.

Built with [Tauri 2](https://tauri.app), React 19 and TypeScript on top of a
Rust backend. Windows only.

## Features

- **Discord sign-in.** "Connect with Discord" opens superpeople.dev's
  launcher sign-in in a window of its own; the launcher trades the one-time
  code it ends on (with a PKCE verifier) for the website's session, stored
  DPAPI-encrypted on the PC and never logged. For every launch it asks the
  website for a two-minute game pass, and the backend answers with a
  short-lived login ticket, handed to the game through the environment
  (`SP_AUTH_TICKET`), never on the command line (`src-tauri/src/auth.rs`).
  Launcher keys are retired; a player who had one keeps their account.
- **Ideas, Roadmap, Completed.** The website's boards inside the launcher:
  vote, read and write comments, suggest ideas and report bugs as the
  signed-in Discord account (`src-tauri/src/community.rs`,
  `src/components/community/`).
- **One game folder.** The Settings tab and the Download tab share a single
  "Game folder": pick a folder that already has the game, or download into it.
- **Download tab.** Downloads the game's files (about 455 files, 30.7 GB)
  from the team's Storj bucket, twelve at a time. The list of files, with
  each one's size and SHA-256, comes from superpeople.dev
  (`/api/launcher/game`), so a file changed in the bucket is refused. It
  shows speed and time left, can pause and continue (also across restarts),
  retries automatically, checks free space, and skips the files a folder
  already has. When it finishes, the game folder is set
  (`src-tauri/src/download.rs`). When the storage cannot serve (its limit
  reached, blocked where the player is), it quietly turns to the backup: the
  same game as one archive on archive.org, downloaded in pieces over eight
  connections spread across archive.org's servers and unpacked with the
  bundled 7-Zip (`7zr.exe`), or single files from it for a repair, checked
  against the same list. The website can send launchers there at once. For a
  full download it also races the two first: a few seconds of each, and the
  backup takes over where it would finish clearly sooner.
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

The DLL, the PAK and the no-Steam proxy are built, tested and signed in
[superpeople-dev/sp-native](https://github.com/superpeople-dev/sp-native); the
launcher bundles the files of its latest release.

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
- Replaces the 200 bot nicknames in `TBL-AICharacterSettingData` with the
  tested callsign set for local bot matches.
- Adds localized `Game.locres` catalogs and overrides across 20
  cultures, including English cheat-command descriptions and other menu text.
- Patches cooked UI assets for HUD health, stance, ammo and weapon layout, 
  bringing back the original CBT UI;
- Updated Management and Black Market presentation; and removed the inventory's 
  obsolete Black Market entry. 


### Deployment

Enabled launches deploy all three files for the game session and remove them
after exit. Disabled launches recover recognized leftovers and deploy none of
them. The launcher records deployed hashes outside the game folder and holds
an exclusive session lock to recover after interrupted runs. Unknown or
modified files are preserved and block launch rather than being overwritten.
The no-Steam fix remains independent. A saved
`-ExecCmds="PakFile.SearchRecentlyFoundPaks 0"` from older tests is dropped
from launch arguments because the DLL handles ordered lookup; other
`-ExecCmds` values are passed through unchanged.

The launcher explicitly sets `SP_CLIENT_FIXES_ENABLED` to `1` or `0` for each child. The no-Steam proxy loads the adjacent fixes DLL only for an enabled launch; there is no remote-thread injection. Fresh session events acknowledge successful DLL bootstrap (supported executable, mandatory PAK hooks and worker startup). Missing, blocked, incompatible or failed startup stops the game; a missing acknowledgement times out after 60 seconds. Object-dependent fixes still activate later as the game's objects appear. This requires proxy v15 and fixes DLL v19 or later; an older bundle cannot acknowledge startup.

To test matching native changes before releasing them, manually run the **Unsigned Windows build** workflow with `native_ref` set to the `sp-native` branch, tag or commit. It builds/tests both DLLs, verifies the existing signed PAK against the new DLL, runs the cross-process startup tests, and embeds the pair. Its artifact includes source commit/hash provenance and the unsigned launcher executable. It does not publish a release or require the launcher signing key.

Unsigned builds use the latest published launcher release's version in the frontend package, Rust crate and Tauri metadata, so the existing release does not immediately trigger an update prompt. The workflow fails if that release cannot be read or its tag is not `vX.Y.Z`/`X.Y.Z`. A subsequent newer release can still trigger an update normally.

## Building

Prerequisites: [Rust](https://rustup.rs), Visual Studio Build Tools with the
"Desktop development with C++" workload, [Bun](https://bun.sh) and [Node.js](https://nodejs.org).

```powershell
git clone <this-repo-url>
cd sp-launcher
bun install
bun run tauri dev      # development, hot reload
bun run tauri build    # NSIS installer in src-tauri/target/release/bundle/nsis/
```

Some binaries are embedded at build time and are **not** in the repo. See
[src-tauri/resources/README.md](src-tauri/resources/README.md):

- The game binaries: `XAPOFX1_5.dll` (the no-Steam proxy), `SPClientFixes.dll`
  and the client fixes PAK/`.sig`. They come from the latest
  [sp-native](https://github.com/superpeople-dev/sp-native) release:
  `powershell -ExecutionPolicy Bypass -File tools\fetch-binaries.ps1` downloads
  them into `src-tauri/resources` and checks each against the release's
  `manifest.json`. You need `gh` signed in with access to sp-native. They are
  required for a release.

A fresh clone still compiles without them (`build.rs` only warns).

For an unsigned Windows build without an installer, run the
[Unsigned Windows build](.github/workflows/unsigned-build.yml) GitHub Action.
It embeds the latest sp-native binaries in the Tauri executable and uploads it
as a workflow artifact.

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
  App.tsx                    tabs, welcome screen, launch/stop, updater
  components/                TitleBar (status pill, profile menu), Welcome,
                             PlayPanel, DownloadPanel, SettingsPanel, News,
                             LaunchArgs; community/ for Ideas, Roadmap, Completed
  lib/                       folder picker, formatting, updater glue
  types.ts                   mirrors the Rust structs
  styles.css                 the whole design, one file
src-tauri/src/
  lib.rs                     Tauri commands, app state, tray
  main.rs                    entry; also the elevated "--sp-hosts" helper mode
  auth.rs                    Discord sign-in (PKCE), DPAPI storage, game pass
  community.rs               Ideas/Roadmap/Completed: items, votes, comments
  config.rs                  settings (config.v1.json), atomic writes
  game.rs                    install detection, launching the game
  hosts.rs                   hosts block, one-time elevation
  shim.rs                    no-Steam DLL install
  engine_ini.rs              Engine.ini patch
  download.rs                Download tab: file list, Storj, SHA-256, resume
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
