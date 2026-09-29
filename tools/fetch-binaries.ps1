# ---------------------------------------------------------------------------
#  fetch-binaries.ps1 -- puts the game binaries the launcher bundles into
#  src-tauri\resources, from a release of superpeople-dev/sp-native (the one
#  repo that builds them):
#
#      XAPOFX1_5.dll                     -> resources\XAPOFX1_5.dll
#      SPClientFixes.dll                 -> resources\SPClientFixes.dll
#      BravoHotelGame-ClientFixes_P.pak  -> resources\client-fixes\
#      BravoHotelGame-ClientFixes_P.sig  -> resources\client-fixes\
#
#  and that release's manifest.json as resources\sp-native.json. Every file is
#  checked against manifest.json (SHA-256) before anything is replaced.
#
#      powershell -ExecutionPolicy Bypass -File tools\fetch-binaries.ps1           # latest build
#      powershell -ExecutionPolicy Bypass -File tools\fetch-binaries.ps1 build-3   # one build
#
#  sp-native is private: gh needs a login or a GH_TOKEN that can read it.
# ---------------------------------------------------------------------------
param([string]$Build = '')
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$repo = 'superpeople-dev/sp-native'
$files = [ordered]@{
    'XAPOFX1_5.dll'                    = 'src-tauri\resources\XAPOFX1_5.dll'
    'SPClientFixes.dll'                = 'src-tauri\resources\SPClientFixes.dll'
    'BravoHotelGame-ClientFixes_P.pak' = 'src-tauri\resources\client-fixes\BravoHotelGame-ClientFixes_P.pak'
    'BravoHotelGame-ClientFixes_P.sig' = 'src-tauri\resources\client-fixes\BravoHotelGame-ClientFixes_P.sig'
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('sp-native-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $ghArgs = @('release', 'download')
    if ($Build) { $ghArgs += $Build }
    $ghArgs += @('--repo', $repo, '--dir', $tmp, '--pattern', 'manifest.json')
    foreach ($name in $files.Keys) { $ghArgs += @('--pattern', $name) }
    & gh @ghArgs
    if ($LASTEXITCODE -ne 0) { throw "Could not download from $repo. Is gh signed in with access to it (gh auth login, or GH_TOKEN)?" }

    $manifest = Get-Content -LiteralPath (Join-Path $tmp 'manifest.json') -Raw | ConvertFrom-Json
    foreach ($name in $files.Keys) {
        $entry = $manifest.files | Where-Object { $_.name -eq $name }
        if (-not $entry) { throw "$name is not in the manifest of $($manifest.build)" }
        $hash = (Get-FileHash -LiteralPath (Join-Path $tmp $name) -Algorithm SHA256).Hash.ToLower()
        if ($hash -ne $entry.sha256) { throw "$name of $($manifest.build) does not match its manifest.json entry" }
    }

    foreach ($name in $files.Keys) {
        $dest = Join-Path $root $files[$name]
        New-Item -ItemType Directory -Path (Split-Path -Parent $dest) -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $tmp $name) -Destination $dest -Force
    }
    Copy-Item -LiteralPath (Join-Path $tmp 'manifest.json') -Destination (Join-Path $root 'src-tauri\resources\sp-native.json') -Force

    Write-Host "sp-native $($manifest.build) (commit $($manifest.commit.Substring(0, 7))), checked against its manifest:"
    foreach ($name in $files.Keys) {
        $entry = $manifest.files | Where-Object { $_.name -eq $name }
        $version = if ($entry.version) { " $($entry.version)" } else { '' }
        Write-Host "  $name$version  $($entry.sha256.Substring(0, 12))"
    }
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
