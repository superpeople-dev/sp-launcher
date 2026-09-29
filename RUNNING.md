# Running & building SP Launcher

Quick reference for working on the launcher day-to-day, and for producing an
installer to hand to players. All commands run in PowerShell from the
project folder:

```powershell
cd path\to\sp-launcher
```

## First-time setup

You need Rust and the C++ build tools once per machine:

1. Install Rust via [rustup](https://rustup.rs).
2. Install Visual Studio Build Tools with the "Desktop development with
   C++" workload (Tauri links against it on Windows).
3. Install [Bun](https://bun.sh) (`powershell -c "irm bun.sh/install.ps1 | iex"`)
   and Node.js, then the dependencies:

   ```powershell
   bun install
   ```

## Running it while developing

```powershell
bun run tauri dev
```

The first run compiles the whole Rust dependency tree, which takes a few
minutes. After that it's fast, and the React frontend hot-reloads on save —
Rust changes still need a recompile, which `tauri dev` does automatically
when you save a `.rs` file.

## Building the installer

```powershell
bun run tauri build
```

This produces a release build of both the Rust backend and the frontend,
then packages them. The NSIS installer lands in:

```
src-tauri\target\release\bundle\nsis\
```

That `.exe` is the one thing you hand to players — it installs the
launcher for the current user (no admin needed to install; the launcher
itself will ask for elevation later, but only if you turn on the hosts
redirect and it's not writable).

A release build takes noticeably longer than `tauri dev` (optimized Rust
codegen). Only run it when you actually need an installer to distribute or
test the packaged app — use `tauri dev` for everyday iteration.

## Changing the app icon

If you swap `src-tauri/icons/icon.png` (the 512×512 source) for a new
design, regenerate every derived size from it — don't hand-edit
`icon.ico` or the individual PNGs, they're generated. See
`src-tauri/icons/` for what's currently there; ask for the icon to be
rebuilt from a new source if you change the artwork.

## Getting the game files to players

There is no in-launcher downloader. Players are expected to already have a
game install and point the launcher at it from Settings → Game Files
(Install location / Browse / Open folder).

Drop `--limit`/`--max-size` and swap in the real `--base-url` when you're
ready to go live with the actual 28 GB.

## Troubleshooting

- **Taskbar/title bar icon still looks low-res after a rebuild**: two
  separate things have to happen, in order:

  1. **The app actually has to be rebuilt.** Editing the icon files on
     disk does nothing to an `.exe` that was already built — it has the
     old icon baked into it until you rebuild. Cargo also doesn't always
     notice that only an icon *asset* changed (no `.rs` file changed), so
     the safest way to force it to re-embed the icon is a clean rebuild:

     ```powershell
     cd src-tauri
     cargo clean
     cd ..
     bun run tauri build
     ```

     (Slower than an incremental build, but guarantees the new icon is
     actually linked into the new `.exe`, not just present in `icons/`.)

  2. **Pinned taskbar shortcuts cache their own icon bitmap**, separate
     from the icon cache and separate from the `.exe` resource — logging
     out/in does not touch this. Unpin the launcher from the taskbar,
     install/run the freshly rebuilt version, and pin it again.

  If it's still stale after both of those, force the system icon cache
  itself to rebuild:

  ```powershell
  taskkill /f /im explorer.exe
  Remove-Item "$env:LOCALAPPDATA\IconCache.db" -Force -ErrorAction SilentlyContinue
  Remove-Item "$env:LOCALAPPDATA\Microsoft\Windows\Explorer\iconcache*" -Force -ErrorAction SilentlyContinue
  Start-Process explorer.exe
  ```

- **`cargo` not found**: the Rust install didn't add itself to PATH for the
  current shell — open a new PowerShell window (or restart) after
  installing rustup.
- **A `link.exe` / MSVC error during build**: the C++ Build Tools
  workload above is missing or incomplete.
