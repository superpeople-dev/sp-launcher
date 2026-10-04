# Shipping updates through the auto-updater

This is the reference for the update system that's already wired into the
launcher (Tauri's official updater plugin). It covers what's already set up,
and the steps to run every time you want to push a new version to players.

## How it actually works

Every time the launcher starts, it fetches a small JSON file — `latest.json`
— from a URL baked into the app at build time (see "One-time setup" below).
That file says what the newest version is, where to download the installer,
and carries a cryptographic signature of that installer.

The launcher compares `latest.json`'s version against its own. If it's
newer, it shows a toast on the Play screen: "Update available: vX.Y.Z" with
an **Install & Restart** button. Settings also has a **Check for updates**
button for a manual check, plus the current version number.

When someone clicks Install & Restart, the launcher downloads the new
installer and verifies its signature against the public key that was baked
into the app when *that copy* of the launcher was built. If the signature
doesn't match — file got corrupted, `latest.json` got tampered with, wrong
key — it refuses and shows an error instead of running something unverified.
This is why the server hosting `latest.json` doesn't need to be trusted with
HTTPS or anything special: the signature is what actually protects players,
not the transport.

Once verified, it launches the new NSIS installer, which reinstalls over the
current install and restarts the app.

## One-time setup (already done)

This part is finished — it's here so you know what exists and where, not
because you need to redo it.

- **Signing keypair**: `%USERPROFILE%\.tauri\sp-launcher-2.key` (private —
  never shared, never committed) and `sp-launcher-2.key.pub` (public, on the
  same machine, not sensitive but not needed after step below).

  > **Rotated 20.09.2026.** The original `sp-launcher.key` had a password
  > that was lost, so it could no longer sign anything. The old pair is still
  > in `.tauri` in case the password ever resurfaces, but it is dead for
  > practical purposes. The replacement has NO password: the file itself is
  > the secret, and an unprotected key you still have beats a protected one
  > you cannot open. **Back it up** — a password manager entry, not the repo
  > and not the VPS.
  >
  > Consequence: every launcher built before 0.2.7 carries the OLD public key
  > and rejects anything signed with the new one. Those installs had to be
  > updated by hand once. From 0.2.7 onward the updater works normally again.
- **Public key** is baked into `src-tauri/tauri.conf.json` under
  `plugins.updater.pubkey`. It's what every future build ships with, so it
  only needs to be set once — until you deliberately rotate keys (see
  "Losing the private key" below).
- **Update endpoints**: also in `tauri.conf.json`, under
  `plugins.updater.endpoints`, tried in order until one answers:
  1. `https://github.com/superpeople-dev/sp-launcher/releases/latest/download/latest.json`,
     the `latest.json` of the newest GitHub release (from 0.3.4 on; the
     release workflow publishes it, see "Automatic releases" below);
  2. `http://64.226.112.204/launcher/updates/latest.json`, the VPS, which
     is the only one launchers up to 0.3.3 know.
- **Server folder**: `/var/www/html/launcher/updates/` on the Ubuntu box,
  served by nginx at `http://64.226.112.204/launcher/updates/`.

If any of these three ever need to change (new server, new domain, rotated
key), the app has to be rebuilt and redistributed — they're compiled into
the binary, not read from a config file at runtime, since letting the
update source be user-editable would defeat the point of signing.

## Automatic releases

[`.github/workflows/release.yml`](.github/workflows/release.yml) releases
every push to `main` that changes the launcher (not docs-only pushes, and not
a commit whose message contains `[skip release]`). It makes the same build as
the manual steps below: the game binaries of the latest
[sp-native](https://github.com/superpeople-dev/sp-native) build (both DLLs and
the client fixes PAK/`.sig`) and `7za.exe` bundled, the installer signed,
`latest.json` written. It publishes them as a GitHub release, `vX.Y.Z`, which
is also what the website's Download button points at. The release notes name
the sp-native build.

- **A new sp-native build without a launcher change**: Actions → Release →
  *Run workflow* on `main`. A run started by hand releases even a commit that
  is released already, with the patch number + 1.

- **Version**: the one in `src-tauri/tauri.conf.json` when it is newer than
  the last release, otherwise the last release with its patch number + 1. So
  nothing needs bumping for a patch; for 0.4.0, set 0.4.0 in the three files
  in the pull request.
- **Where launchers find it**: `latest.json` on the newest GitHub release,
  the first update endpoint from 0.3.4 on. Its installer URL is the GitHub
  release asset, so nothing has to be uploaded to the VPS.

### One-time setup

Both secrets live in the repository's **release** environment, which only
`main` can use: other branches and pull requests never see them.

1. **The environment**: Settings → Environments → New environment `release`
   → Deployment branches and tags: *Selected branches*, add `main`. No
   required reviewers, or every release waits for a click.
2. **`TAURI_SIGNING_PRIVATE_KEY`**: the contents of
   `%USERPROFILE%\.tauri\sp-launcher-2.key`, added by whoever holds it, on
   their own machine:

   ```powershell
   Get-Content "$env:USERPROFILE\.tauri\sp-launcher-2.key" -Raw | gh secret set TAURI_SIGNING_PRIVATE_KEY --env release -R superpeople-dev/sp-launcher
   ```

   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is only needed if the key ever gets a
   password again.
3. **`SP_NATIVE_TOKEN`**: a token that can read the private `sp-native`
   repository's releases. An organization owner creates a fine-grained
   personal access token (github.com → Settings → Developer settings →
   Fine-grained tokens): resource owner `superpeople-dev`, only the
   `sp-native` repository, permission *Contents: Read-only*. Then:

   ```powershell
   gh secret set SP_NATIVE_TOKEN --env release -R superpeople-dev/sp-launcher
   gh secret set SP_NATIVE_TOKEN -R superpeople-dev/sp-launcher
   ```

   (paste the token when asked). The second one, a repository secret, lets the
   unsigned build of pull requests bundle the binaries too. When the token
   expires, the release fails at its first step until it is replaced.
   The old `SP_LISTEN_PATCH_DEPLOY_KEY` secret is not used any more.

Without either secret the workflow fails at its first step and says which is
missing; it never publishes an unsigned build.

### Code signing

Windows 11's Smart App Control and SmartScreen trust files with an
Authenticode signature from a trusted provider. Ours comes from Azure Artifact
Signing (account `superpeoplesign`, certificate profile in the repository
variable `ARTIFACT_SIGNING_PROFILE`). This signature is separate from the
updater's `.sig` above, which only launchers check.

- **What is signed**: the launcher, its uninstaller and the installer, by
  Tauri through `bundle.windows.signCommand` → `tools/sign.ps1`, before the
  updater's `.sig` is made. The game DLLs it bundles are signed by
  sp-native's build.
- **When**: only once `ARTIFACT_SIGNING_PROFILE` is set. Until then releases
  are built as before, without it.
- **Sign-in**: the release job signs in to Azure with GitHub's OIDC token for
  the `release` environment, as the app `sp-code-signing`. Its federated
  credential trusts that environment only, so there is no secret. The other
  repository variables say where: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`,
  `ARTIFACT_SIGNING_ENDPOINT`, `ARTIFACT_SIGNING_ACCOUNT`.
- **Checking it**: Actions → Check code signing → *Run workflow* signs in
  and, with a profile, signs a copy of the latest installer. On a PC:
  `Get-AuthenticodeSignature .\SP.Launcher_X.Y.Z_x64-setup.exe` (Status
  `Valid`).
- A build made by hand (`tools/publish-update.ps1`) is not code-signed.

### Launchers up to 0.3.3

They only know the VPS endpoint. After the first automatic release (0.3.4),
put its `latest.json` there once; its installer URL already points at GitHub:

```powershell
gh release download v0.3.4 -p latest.json -R superpeople-dev/sp-launcher
scp latest.json root@64.226.112.204:/var/www/html/launcher/updates/latest.json
```

Those launchers then update to 0.3.4, and from there follow GitHub. The VPS
file can stay as it is after that.

## Releasing a new version by hand

Only needed when the workflow can't run. A version released by hand must also
be a GitHub release with its `latest.json` (see the workflow's last two steps),
or launchers from 0.3.4 on, which read GitHub first, won't see it.

> `powershell -ExecutionPolicy Bypass -File tools\publish-update.ps1` runs
> steps 1b to 3 for you, asking for the key password up front (Tauri's own
> mid-build prompt fails under a script) and refusing to build if the game
> binaries or the key are missing. The manual steps below remain the reference.

**1. Bump the version number.** Keep these three in step (they don't have
to match by a hard requirement, but it avoids confusion):
- `src-tauri/tauri.conf.json` → `"version"`
- `src-tauri/Cargo.toml` → `[package] version`
- `package.json` → `"version"`

**1b. Get the game binaries.** `XAPOFX1_5.dll`, `SPClientFixes.dll` and the
client fixes PAK/`.sig` come from the latest sp-native release (`gh` must be
signed in with access to `superpeople-dev/sp-native`):

```powershell
powershell -ExecutionPolicy Bypass -File tools\fetch-binaries.ps1
```

It checks every file against the release's `manifest.json`. Without
`XAPOFX1_5.dll` the launcher ships without the no-Steam fix — and since it
passes `-ServicePlatform=`, every player who does not already have that DLL
by hand gets a game that never leaves the loading screen. `build.rs` warns
when it is missing and the launcher logs `this launcher has no DLL bundled`
at launch; `tools\publish-update.ps1` refuses to build at all.

**2. Build with signing enabled.** The private key has to be available as an
environment variable during the build — this is what makes `tauri build` sign
the installer automatically instead of just building it. The current key
(`sp-launcher-2.key`, since 20.09.2026) has no password:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$env:USERPROFILE\.tauri\sp-launcher-2.key" -Raw
bun run tauri build
```

This produces, in `src-tauri\target\release\bundle\nsis\`:
- `SP Launcher_X.Y.Z_x64-setup.exe` — the installer
- `SP Launcher_X.Y.Z_x64-setup.exe.sig` — its signature (plain text, base64)

If you forget to set the env vars, the build still succeeds but produces no
`.sig` file — that's the tell that signing didn't happen.

**3. Generate `latest.json`:**

```powershell
node tools/make-update-manifest.mjs `
  --version X.Y.Z `
  --sig "src-tauri\target\release\bundle\nsis\SP Launcher_X.Y.Z_x64-setup.exe.sig" `
  --url "http://64.226.112.204/launcher/updates/SP Launcher_X.Y.Z_x64-setup.exe" `
  --notes "One or two lines describing what changed" `
  --out latest.json
```

`--notes` isn't shown anywhere in the current UI yet — it's just carried in
the JSON for future use — but fill it in anyway so you have a record.

**4. Upload both files to the server**, so they land at exactly the URLs the
launcher expects:

```powershell
scp "src-tauri\target\release\bundle\nsis\SP Launcher_X.Y.Z_x64-setup.exe" youruser@64.226.112.204:/var/www/html/launcher/updates/
scp latest.json youruser@64.226.112.204:/var/www/html/launcher/updates/latest.json
```

**5. Verify it's actually live** before telling anyone to update:

```powershell
curl http://64.226.112.204/launcher/updates/latest.json
```

You should see the JSON you just generated, with the new version number.

That's it — every running launcher will pick this up on its next startup
(or immediately if someone clicks Check for updates) and offer to install
it.

## A version you upload replaces the previous one for everybody

There's only ever one `latest.json` at that URL. Uploading a new version
overwrites what every player's launcher will see next time it checks — you
can't have some players offered v1.2 and others v1.3 from the same
endpoint. If you need to roll back a bad release, re-upload the previous
version's installer and a `latest.json` pointing at it (with the old, lower
version number) — though note the updater compares versions and normally
won't offer a downgrade; you'd need `allowDowngrades` for that, which isn't
currently enabled anywhere in this app.

## Losing the private key

If `%USERPROFILE%\.tauri\sp-launcher.key` (or wherever you end up backing it
up to) is lost, you cannot sign any future update that existing installs
will trust — every copy of the launcher already out there has today's
public key baked in and will reject anything signed by a different one.
Recovering from that means generating a new keypair, putting the new public
key in `tauri.conf.json`, and shipping that build to everyone through some
channel *other* than the auto-updater (since the old installs can't verify
it) — e.g. posting a fresh installer link in Discord. Back the private key
up somewhere durable (a password manager, an encrypted drive) — losing it
isn't catastrophic, but it does mean burning the auto-updater for everyone
currently on an older version.

## Troubleshooting

- **"Check for updates" never finds anything, even after uploading a newer
  version.** Check the endpoint URL is reachable and returns the file you
  expect: `curl http://64.226.112.204/launcher/updates/latest.json`. A 404
  means it's not at that exact path; nginx returning the wrong folder means
  the `launcher/updates/` structure under `/var/www/html/` doesn't match.
- **Update found, but install fails with a signature error.** Almost always
  means the `.sig` file used in `make-update-manifest.mjs --sig` doesn't
  match the `.exe` you actually uploaded — rebuild and regenerate both
  together, don't mix an old `.sig` with a new `.exe` or vice versa.
- **Build succeeds but no `.sig` file appears.** `TAURI_SIGNING_PRIVATE_KEY`
  wasn't set in that shell session — environment variables set with `$env:`
  in PowerShell only last for that window; set them again if you open a new
  one before building.
