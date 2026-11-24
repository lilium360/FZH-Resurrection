# Builds the signed installer of the version in launcher\src-tauri\tauri.conf.json, writes
# latest.json next to it and, with -Publish, tags the commit and creates the GitHub Release
# (docs\releasing.md). Needs the update key committed as plugins.updater.pubkey, a clean tree
# and the GitHub CLI (gh) signed in for -Publish.
#
#   powershell -ExecutionPolicy Bypass -File tools\release.ps1 -Server <public server address>
#              -Notes "First public release" [-FormerServers <former address>] [-Publish]
#
# The private key is TAURI_SIGNING_PRIVATE_KEY (a path or the key itself), else
# %USERPROFILE%\.tauri\fzh-resurrection.key.
#
# -Server   compiled in as FZH_DEFAULT_SERVER (docs\building.md); it is written to no file. Without
#           a relay part ("/relay,relay") the relays of server\relay\relays.json are added, the
#           first 8 in the order of the list
# -FormerServers  optional, compiled in as FZH_FORMER_SERVERS: the addresses the community server
#           had before it moved, separated by commas, so that players stay signed in
# -Notes    one change per line; shown by the launcher under About > What's new
# -Publish  tag v<version>, push it and create the release with the installer, its .sig and latest.json
#
# The key's password comes from TAURI_SIGNING_PRIVATE_KEY_PASSWORD, else from Windows Credential
# Manager, else it is asked for. To keep it in Credential Manager (readable only by this Windows
# user) or remove it from there:
#
#   powershell -ExecutionPolicy Bypass -File tools\release.ps1 -StorePassword
#   powershell -ExecutionPolicy Bypass -File tools\release.ps1 -ForgetPassword
param([Parameter(Mandatory = $true, ParameterSetName = 'Release')][string]$Server,
      [Parameter(ParameterSetName = 'Release')][string]$FormerServers,
      [Parameter(Mandatory = $true, ParameterSetName = 'Release')][string]$Notes,
      [Parameter(ParameterSetName = 'Release')][switch]$Publish,
      [Parameter(Mandatory = $true, ParameterSetName = 'Store')][switch]$StorePassword,
      [Parameter(Mandatory = $true, ParameterSetName = 'Forget')][switch]$ForgetPassword)
$ErrorActionPreference = 'Stop'

$credential = 'FZH Resurrection/update-key'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class FzhCredential {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct CREDENTIAL {
        public int Flags; public int Type; public string TargetName; public string Comment; public long LastWritten;
        public int CredentialBlobSize; public IntPtr CredentialBlob; public int Persist; public int AttributeCount;
        public IntPtr Attributes; public string TargetAlias; public string UserName;
    }
    const int GENERIC = 1, LOCAL_MACHINE = 2;
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredWriteW(ref CREDENTIAL credential, int flags);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredDeleteW(string target, int type, int flags);
    [DllImport("advapi32.dll")]
    static extern void CredFree(IntPtr credential);

    public static string Read(string target) {
        IntPtr p;
        if (!CredReadW(target, GENERIC, 0, out p)) return null;
        try {
            var c = (CREDENTIAL)Marshal.PtrToStructure(p, typeof(CREDENTIAL));
            var blob = new byte[c.CredentialBlobSize];
            Marshal.Copy(c.CredentialBlob, blob, 0, blob.Length);
            return Encoding.Unicode.GetString(blob);
        } finally { CredFree(p); }
    }
    public static void Write(string target, string secret) {
        var blob = Encoding.Unicode.GetBytes(secret);
        var c = new CREDENTIAL { Type = GENERIC, TargetName = target, UserName = "update key", Persist = LOCAL_MACHINE,
                                 CredentialBlobSize = blob.Length, CredentialBlob = Marshal.AllocHGlobal(blob.Length) };
        try {
            Marshal.Copy(blob, 0, c.CredentialBlob, blob.Length);
            if (!CredWriteW(ref c, 0)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        } finally { Marshal.FreeHGlobal(c.CredentialBlob); }
    }
    public static bool Delete(string target) { return CredDeleteW(target, GENERIC, 0); }
}
'@

function Read-Password {
    $secure = Read-Host 'Password of the update key' -AsSecureString
    $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr) } finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr) }
}

if ($StorePassword) {
    $typed = Read-Password
    if (-not $typed) { throw 'No password typed: nothing stored' }
    [FzhCredential]::Write($credential, $typed)
    Write-Host "Stored in Windows Credential Manager as '$credential'."
    exit 0
}
if ($ForgetPassword) {
    if ([FzhCredential]::Delete($credential)) { Write-Host "Removed '$credential' from Windows Credential Manager." } else { Write-Host "Nothing stored as '$credential'." }
    exit 0
}

$repo = Split-Path -Parent $PSScriptRoot
$launcher = Join-Path $repo 'launcher'
$conf = Get-Content (Join-Path $launcher 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"

if ($conf.plugins.updater.pubkey -eq 'UPDATER-PUBLIC-KEY-NOT-SET') { throw 'No update key: see "Signing key" in docs\releasing.md' }
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    $key = Join-Path $env:USERPROFILE '.tauri\fzh-resurrection.key'
    if (-not (Test-Path $key)) { throw "Set TAURI_SIGNING_PRIVATE_KEY to the path of the private key (not found: $key)" }
    $env:TAURI_SIGNING_PRIVATE_KEY = $key
}
foreach ($file in 'package.json', 'src-tauri\Cargo.toml') {
    if (-not (Select-String -Path (Join-Path $launcher $file) -Pattern "^\s*`"?version`"?\s*[:=]\s*`"$([regex]::Escape($version))`"" -Quiet)) {
        throw "launcher\$file is not at version $version"
    }
}
if (git -C $repo status --porcelain) { throw 'The tree has uncommitted changes: a release is built from a commit' }
if (git -C $repo tag --list $tag) { throw "Tag $tag exists: raise the version first" }

$typed = $false
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    $stored = [FzhCredential]::Read($credential)
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = if ($stored) { $stored } else { Read-Password }
    $typed = $true
}
if ($Server -notmatch '/') {
    $list = Get-Content (Join-Path $repo 'server\relay\relays.json') -Raw | ConvertFrom-Json
    $relays = @(foreach ($relay in $list) { $relay.address })
    if ($relays.Count -gt 8) { Write-Host "Using the first 8 of $($relays.Count) relays of relays.json" }
    if ($relays.Count) { $Server = $Server + '/' + (($relays | Select-Object -First 8) -join ',') }
}
$env:FZH_DEFAULT_SERVER = $Server
$env:FZH_FORMER_SERVERS = $FormerServers
try {
    Push-Location $launcher
    npm ci
    if ($LASTEXITCODE) { throw 'npm ci failed' }
    npm run tauri build
    if ($LASTEXITCODE) { throw 'The build failed' }
} finally {
    Pop-Location
    Remove-Item Env:FZH_DEFAULT_SERVER
    Remove-Item Env:FZH_FORMER_SERVERS -ErrorAction SilentlyContinue
    if ($typed) { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD }
}

$commit = Get-Content (Join-Path $launcher 'src-tauri\bundle\emulator\commit.txt') -Raw
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
