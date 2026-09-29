# ---------------------------------------------------------------------------
#  publish-update.ps1 -- the release procedure in UPDATING.md, run for you.
#
#      cd C:\Users\Ado\Desktop\sp-server\sp-launcher
#      powershell -ExecutionPolicy Bypass -File tools\publish-update.ps1
#
#  UPDATING.md remains the reference; this is the same four steps with the two
#  mistakes that silently ship a broken release turned into hard stops:
#
#    * no XAPOFX1_5.dll bundled -> every player who lacks it by hand gets a
#      game that never leaves the loading screen, because the launcher passes
#      -ServicePlatform= and nothing intercepts the Steam subsystem request.
#    * no signing key -> the build SUCCEEDS and produces no .sig, and every
#      launcher then rejects the update as unsigned. UPDATING.md calls this
#      out; it is still easy to do at 2am.
#
#  Manifest generation is delegated to tools/make-update-manifest.mjs rather
#  than reimplemented. Uploading is not automated: it is the irreversible step.
# ---------------------------------------------------------------------------
$ErrorActionPreference = 'Stop'

# Run from the project root regardless of where this was invoked from.
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

$conf     = Get-Content 'src-tauri\tauri.conf.json' -Raw | ConvertFrom-Json
$version  = $conf.version
$endpoint = $conf.plugins.updater.endpoints[0]          # .../launcher/updates/latest.json
$baseUrl  = $endpoint -replace '/latest\.json$', ''      # .../launcher/updates
$remote   = '/var/www/html/launcher/updates/'            # per UPDATING.md

Write-Host "SP Launcher $version" -ForegroundColor Cyan
Write-Host "  endpoint: $endpoint"

# --- 1. the DLL ------------------------------------------------------------
$dll = 'src-tauri\resources\XAPOFX1_5.dll'
if (-not (Test-Path $dll)) {
    Write-Host ''
    Write-Host 'STOP: src-tauri\resources\XAPOFX1_5.dll is missing.' -ForegroundColor Red
    Write-Host 'The launcher would ship with no no-Steam fix. Players who do not already'
    Write-Host 'have that DLL by hand cannot start the game at all.'
    Write-Host ''
    Write-Host '  cd ..\sp-listen-patch'
    Write-Host '  build_sp_proxy.bat'
    Write-Host '  copy dist\XAPOFX1_5.dll ..\sp-launcher\src-tauri\resources\'
    exit 1
}
Write-Host "  DLL bundled: $((Get-Item $dll).Length) bytes" -ForegroundColor Green
foreach ($fixResource in @('src-tauri\resources\client-fixes\BravoHotelGame-ClientFixes_P.pak', 'src-tauri\resources\client-fixes\BravoHotelGame-ClientFixes_P.sig')) {
    if (-not (Test-Path -LiteralPath $fixResource)) { throw "Missing bundled Client fixes resource: $fixResource" }
}
$clientFixesDll = 'src-tauri\resources\SPClientFixes.dll'
if (-not (Test-Path $clientFixesDll)) {
    Write-Host ''
    Write-Host 'STOP: src-tauri\resources\SPClientFixes.dll is missing.' -ForegroundColor Red
    Write-Host 'The Client fixes toggle would be present but could not load its DLL.'
    Write-Host 'Build it using client-fixes/README.md before publishing.'
    exit 1
}
Write-Host "  client fixes DLL bundled: $((Get-Item $clientFixesDll).Length) bytes" -ForegroundColor Green

# --- 2. the signing key ----------------------------------------------------
# Tauri wants the key's CONTENTS, not its path (UPDATING.md step 2).
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    # sp-launcher-2.key, not sp-launcher.key: the original was rotated on
    # 20.09.2026 after its password was lost. The old pair is still in .tauri
    # and would build an installer no launcher will ever accept.
    $keyFile = Join-Path $env:USERPROFILE '.tauri\sp-launcher-2.key'
    if (-not (Test-Path $keyFile)) {
        Write-Host ''
        Write-Host "STOP: no signing key at $keyFile and TAURI_SIGNING_PRIVATE_KEY is not set." -ForegroundColor Red
        Write-Host 'Without it the build succeeds, produces no .sig, and every launcher'
        Write-Host 'refuses the update. See "Losing the private key" in UPDATING.md.'
        exit 1
    }
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyFile -Raw
    Write-Host "  key loaded from $keyFile" -ForegroundColor Green
} else {
    Write-Host '  key taken from the environment' -ForegroundColor Green
}

# The password has to come from the environment too. Left unset, Tauri prompts
# for it itself part-way through the build -- and that prompt fails under a
# script with "stream did not contain valid UTF-8", after the installer has
# already been built, which is a slow way to learn nothing got signed.
#
# Asked here instead, and ALWAYS set (an empty string is a valid answer for a
# key with no password) so Tauri never reaches for its own prompt.
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    $secure = Read-Host 'Key password (blank if the key has none)' -AsSecureString
    $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try {
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
    } finally {
        # Do not leave the password sitting in unmanaged memory.
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    }
}

# --- 3. build --------------------------------------------------------------
Write-Host ''
Write-Host 'Building...' -ForegroundColor Cyan
bun run tauri build
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$bundle = 'src-tauri\target\release\bundle\nsis'
$setup  = Get-ChildItem $bundle -Filter "*_${version}_x64-setup.exe" | Select-Object -First 1
if (-not $setup) { throw "no installer for $version in $bundle" }
$sig = "$($setup.FullName).sig"
if (-not (Test-Path $sig)) {
    Write-Host ''
    Write-Host "The installer built but was NOT signed, so no launcher will accept it." -ForegroundColor Red
    Write-Host 'Almost always a wrong key password. If you cannot recall it, see'
    Write-Host '"Losing the private key" in UPDATING.md -- you would rotate the key and'
    Write-Host 'have existing users install this version by hand once.'
    throw "no .sig beside $($setup.Name)"
}
Write-Host "  built and signed: $($setup.Name)" -ForegroundColor Green

# --- 4. manifest -----------------------------------------------------------
$notes = Read-Host 'Release notes (one line)'
if (-not $notes) { $notes = "Version $version" }

node tools/make-update-manifest.mjs `
  --version $version `
  --sig $sig `
  --url "$baseUrl/$($setup.Name)" `
  --notes $notes `
  --out latest.json
if ($LASTEXITCODE -ne 0) { throw 'manifest generation failed' }

Write-Host ''
Write-Host "Ready. Test the installer on your own machine first:" -ForegroundColor Yellow
Write-Host "  $($setup.FullName)"
Write-Host ''
Write-Host 'Then upload - INSTALLER FIRST, so no launcher ever sees a manifest' -ForegroundColor Yellow
Write-Host 'pointing at a file that is not there yet:' -ForegroundColor Yellow
Write-Host ''
Write-Host "  scp `"$($setup.FullName)`" root@64.226.112.204:$remote"
Write-Host "  scp latest.json root@64.226.112.204:${remote}latest.json"
Write-Host ''
Write-Host '  curl http://64.226.112.204/launcher/updates/latest.json'
