# Publish an existing successful cloud Build's pak for an unsigned launcher test.
# The launcher has Contents-read access to sp-native, not Actions-read access.
# No new production release is created; the existing inputs release holds the
# content-addressed files and run/commit provenance. Requires gh authentication
# with Actions-read and Contents-write access on sp-native. Never reads a key.
[CmdletBinding()]
param([Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long]$RunId)
$ErrorActionPreference = 'Stop'
$repo = 'superpeople-dev/sp-native'
$runJson = gh api "repos/$repo/actions/runs/$RunId"
if ($LASTEXITCODE -ne 0) { throw 'Could not read the native Build run.' }
$run = $runJson | ConvertFrom-Json
if ($run.path -ne '.github/workflows/build.yml' -or $run.conclusion -ne 'success' -or $run.head_sha -notmatch '^[a-f0-9]{40}$') {
    throw 'The native Build must have succeeded before publishing test inputs.'
}
$taskOutput = Join-Path ([IO.Path]::GetTempPath()) "sp-native-test-inputs-$RunId-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $taskOutput | Out-Null
gh run download $RunId --repo $repo --name bin-pak --dir $taskOutput
if ($LASTEXITCODE -ne 0) { throw 'Could not download bin-pak (native artifacts are kept for one day).' }
$files = foreach ($name in @('BravoHotelGame-ClientFixes_P.pak', 'BravoHotelGame-ClientFixes_P.sig')) {
    $path = Join-Path $taskOutput $name
    $digest = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLower()
    $asset = "ClientFixes-$digest$([IO.Path]::GetExtension($name))"
    Copy-Item -LiteralPath $path -Destination (Join-Path $taskOutput $asset)
    @{ name = $name; asset = $asset; sha256 = $digest; bytes = (Get-Item -LiteralPath $path).Length }
}
$manifestName = "NativeBuild-$RunId.json"
@{ commit = $run.head_sha; run_id = $RunId; workflow = $run.path; conclusion = $run.conclusion; files = @($files) } |
    ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $taskOutput $manifestName)
# Upload the two payloads first. Publish the manifest only once both are present.
foreach ($file in $files) {
    gh release upload inputs (Join-Path $taskOutput $file.asset) --repo $repo --clobber
    if ($LASTEXITCODE -ne 0) { throw "Could not publish $($file.asset)." }
}
gh release upload inputs (Join-Path $taskOutput $manifestName) --repo $repo --clobber
if ($LASTEXITCODE -ne 0) { throw 'Could not publish the native test manifest.' }
"Published native test inputs: run $RunId, commit $($run.head_sha)"
