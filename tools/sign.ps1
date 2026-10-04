# Signs one file with the team's Authenticode certificate (Azure Artifact
# Signing, account superpeoplesign). Tauri runs it for the launcher, its
# uninstaller and the installer: bundle.windows.signCommand, which release.yml
# sets only when signing is on (UPDATING.md, "Code signing").
#
# Needs Azure CLI signed in (azure/login), the ArtifactSigning PowerShell
# module, and ARTIFACT_SIGNING_ENDPOINT, ARTIFACT_SIGNING_ACCOUNT and
# ARTIFACT_SIGNING_PROFILE in the environment.
param([Parameter(Mandatory)][string]$Path)

$ErrorActionPreference = 'Stop'

foreach ($name in 'ARTIFACT_SIGNING_ENDPOINT', 'ARTIFACT_SIGNING_ACCOUNT', 'ARTIFACT_SIGNING_PROFILE') {
  if (-not [Environment]::GetEnvironmentVariable($name)) { throw "$name is not set" }
}

$file = (Resolve-Path -LiteralPath $Path).Path

# Signed as a copy in a folder without spaces ("SP Launcher_..._x64-setup.exe"
# has one), then put back.
$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $base ('sign-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
  $copy = Join-Path $work ('file' + [IO.Path]::GetExtension($file))
  Copy-Item -LiteralPath $file -Destination $copy

  # Azure CLI's sign-in only; no other credential is tried.
  $params = @{
    Endpoint                           = $env:ARTIFACT_SIGNING_ENDPOINT
    CodeSigningAccountName             = $env:ARTIFACT_SIGNING_ACCOUNT
    CertificateProfileName             = $env:ARTIFACT_SIGNING_PROFILE
    Files                              = $copy
    FileDigest                         = 'SHA256'
    TimestampRfc3161                   = 'http://timestamp.acs.microsoft.com'
    TimestampDigest                    = 'SHA256'
    ExcludeEnvironmentCredential       = $true
    ExcludeWorkloadIdentityCredential  = $true
    ExcludeManagedIdentityCredential   = $true
    ExcludeSharedTokenCacheCredential  = $true
    ExcludeVisualStudioCredential      = $true
    ExcludeVisualStudioCodeCredential  = $true
    ExcludeAzurePowerShellCredential   = $true
    ExcludeAzureDeveloperCliCredential = $true
    ExcludeInteractiveBrowserCredential = $true
  }
  Invoke-ArtifactSigning @params

  $sig = Get-AuthenticodeSignature -LiteralPath $copy
  if ($sig.Status -ne 'Valid') { throw "$(Split-Path $file -Leaf) is not validly signed: $($sig.Status) $($sig.StatusMessage)" }
  Copy-Item -LiteralPath $copy -Destination $file -Force
  Write-Host "Signed $(Split-Path $file -Leaf)"
} finally {
  Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
