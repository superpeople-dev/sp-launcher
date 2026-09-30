# src-tauri/resources

## The game binaries — required before shipping a release

`XAPOFX1_5.dll`, `SPClientFixes.dll` and `client-fixes/BravoHotelGame-ClientFixes_P.pak`
+ `.sig` are built, tested and signed in
[superpeople-dev/sp-native](https://github.com/superpeople-dev/sp-native). This
repo keeps no copy of their source. Put the latest release here with:

    powershell -ExecutionPolicy Bypass -File tools\fetch-binaries.ps1

It checks every file against the release's `manifest.json` and keeps that
manifest as `sp-native.json` (which build is bundled). `gh` must be signed in
with access to sp-native; the release workflow uses the `SP_NATIVE_TOKEN` secret.

- **XAPOFX1_5.dll** is the no-Steam proxy. The launcher embeds it with
  `include_bytes!` and writes it into the player's
  `BravoHotelGame\Binaries\Win64\` before every launch, so the auto-updater
  delivers it along with the launcher itself. It is not optional: the launcher
  passes `-ServicePlatform=`, and without this DLL intercepting the game's
  request for the Steam online subsystem, the client waits forever for a Steam
  that is not running — the loading screen that never ends.
- **SPClientFixes.dll** and the **PAK/.sig pair** are loaded only when
  Settings → Client fixes is on. All three are embedded together; an
  incomplete set disables the bundle.

`build.rs` checks for these files. If they are absent the launcher still
compiles — a fresh clone should not fail to build — but it prints a warning,
ships without them, and logs `this launcher has no DLL bundled` at launch.
`tools\publish-update.ps1` refuses to build a release without them.

They are deliberately not in version control: they are build artifacts of
sp-native, and a stale copy committed by accident is worse than no copy.

