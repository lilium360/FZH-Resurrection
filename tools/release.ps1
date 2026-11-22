# Builds the signed installer of the version in launcher\src-tauri\tauri.conf.json, writes
# latest.json next to it and, with -Publish, tags the commit and creates the GitHub Release
# (docs\releasing.md). Needs the update key committed as plugins.updater.pubkey, a clean tree
# and the GitHub CLI (gh) signed in for -Publish.
#
#   $env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\fzh-resurrection.key"
#   powershell -ExecutionPolicy Bypass -File tools\release.ps1 -Server <public server address>
#              -Notes "First public release" [-Publish]
#
# -Server   compiled in as FZH_DEFAULT_SERVER (docs\building.md); it is written to no file
# -Notes    one change per line; shown by the launcher under About > What's new
# -Publish  tag v<version>, push it and create the release with the installer, its .sig and latest.json
# The key's password is asked for unless TAURI_SIGNING_PRIVATE_KEY_PASSWORD is set.
param([Parameter(Mandatory = $true)][string]$Server, [Parameter(Mandatory = $true)][string]$Notes,
      [switch]$Publish)
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$launcher = Join-Path $repo 'launcher'
$conf = Get-Content (Join-Path $launcher 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"

if ($conf.plugins.updater.pubkey -eq 'UPDATER-PUBLIC-KEY-NOT-SET') { throw 'No update key: see "Signing key" in docs\releasing.md' }
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) { throw 'Set TAURI_SIGNING_PRIVATE_KEY to the path of the private key' }
foreach ($file in 'package.json', 'src-tauri\Cargo.toml') {
    if (-not (Select-String -Path (Join-Path $launcher $file) -Pattern "^\s*`"?version`"?\s*[:=]\s*`"$([regex]::Escape($version))`"" -Quiet)) {
        throw "launcher\$file is not at version $version"
    }
}
if (git -C $repo status --porcelain) { throw 'The tree has uncommitted changes: a release is built from a commit' }
if (git -C $repo tag --list $tag) { throw "Tag $tag exists: raise the version first" }

$typed = $false
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    $secure = Read-Host 'Password of the update key' -AsSecureString
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
        [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
    $typed = $true
}
$env:FZH_DEFAULT_SERVER = $Server
try {
    Push-Location $launcher
    npm ci
    if ($LASTEXITCODE) { throw 'npm ci failed' }
    npm run tauri build
    if ($LASTEXITCODE) { throw 'The build failed' }
} finally {
    Pop-Location
    Remove-Item Env:FZH_DEFAULT_SERVER
    if ($typed) { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD }
}

$commit = Get-Content (Join-Path $launcher 'src-tauri\bundle\commit.txt') -Raw
if ($commit -match 'dirty') { throw "The bundled emulator comes from a modified tree: $($commit.Trim())" }

$out = Join-Path $launcher 'src-tauri\target\release\bundle\nsis'
$installer = Join-Path $out "FZH Resurrection_${version}_x64-setup.exe"
$signature = "$installer.sig"
if (-not (Test-Path $signature)) { throw "No signature next to the installer: $signature" }

# GitHub replaces the spaces of an uploaded file name with dots.
$url = "https://github.com/lilium360/FZH-Resurrection/releases/download/$tag/FZH.Resurrection_${version}_x64-setup.exe"
$latest = Join-Path $out 'latest.json'
$json = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{ 'windows-x86_64' = [ordered]@{ signature = (Get-Content $signature -Raw).Trim(); url = $url } }
} | ConvertTo-Json -Depth 5
[IO.File]::WriteAllText($latest, $json)
Write-Host "Built $installer"

if (-not $Publish) {
    Write-Host 'Not published: run again with -Publish, or follow "Publishing" in docs\releasing.md.'
    exit 0
}
git -C $repo tag $tag
git -C $repo push origin $tag
if ($LASTEXITCODE) { throw 'Pushing the tag failed' }
gh release create $tag $installer $signature $latest --repo lilium360/FZH-Resurrection --title "FZH Resurrection $version" --notes $Notes
if ($LASTEXITCODE) { throw 'Creating the release failed' }
Write-Host "Published $tag. Check https://github.com/lilium360/FZH-Resurrection/releases/latest/download/latest.json"
