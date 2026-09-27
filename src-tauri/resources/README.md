# src-tauri/resources

## SPClientFixes.dll — optional client fixes

Built from `client-fixes/` in this repository. It is separate from the
no-Steam proxy below and is loaded only when Settings → Client fixes is on.
See [client-fixes/README.md](../../client-fixes/README.md). A fresh clone can
compile without it, but the release script requires it so the toggle works.

## XAPOFX1_5.dll  — required before shipping a release

This is the no-Steam proxy from `sp-listen-patch/sp_proxy.cpp`. The launcher
embeds it with `include_bytes!` and writes it into the player's
`BravoHotelGame\Binaries\Win64\` before every launch, so the auto-updater
delivers it along with the launcher itself.

It is not optional. The launcher passes `-ServicePlatform=` , and without this
DLL intercepting the game's request for the Steam online subsystem, the client
waits forever for a Steam that is not running — the loading screen that never
ends.

To produce it:

    cd ..\sp-listen-patch
    build_sp_proxy.bat
    copy dist\XAPOFX1_5.dll ..\sp-launcher\src-tauri\resources\

`build.rs` checks for this file. If it is absent the launcher still compiles —
a fresh clone should not fail to build — but it prints a warning, ships without
the DLL, and logs `this launcher has no DLL bundled` at launch.
`tools\publish-update.ps1` refuses to build a release at all without it.

It is deliberately not in version control: it is a build artifact, it is
platform-specific, and a stale copy committed by accident is worse than no copy.

## 7za.exe  — recommended before shipping a release

The Download tab (`src/download.rs`) unpacks the game archive with 7-Zip.
`build.rs` embeds `resources/7za.exe` when it is present (same `include_bytes!`
pattern as the DLL above) and the launcher writes it to
`%APPDATA%\com.superpeople.launcher\tools\7za.exe` the first time it
unpacks. Without it the launcher falls back to an installed 7-Zip
(`C:\Program Files\7-Zip\7z.exe`) and tells the player to install 7-Zip
if there is none.

To get it: download the "7-Zip Extra" package (`7z....-extra.7z`) from
https://www.7-zip.org/download.html, open it with 7-Zip, and copy
`x64\7za.exe` here. 7za is LGPL-licensed; shipping it inside the launcher is
fine, but keep a note of the version in the release notes.

Not in version control for the same reason as the DLL.

Client fixes additionally requires the committed PAK and signature in `client-fixes/`. All three resources are embedded together; incomplete resource sets disable the bundle. See `client-fixes/README.md` for hashes and rebuild notes.
