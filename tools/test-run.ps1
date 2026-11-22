# One automated run: starts an instance with an input script, takes screenshots (-Shots 'n,from':
# every n seconds from 'from' seconds on, SHADPS4_SHOT_SECONDS),
# kills the emulator after N seconds, then keeps the log and the screenshots in <OutDir>\<Name>
# and prints a summary (exit, first crash line, backend requests of this run, screenshots).
#
#   powershell -ExecutionPolicy Bypass -File tools\test-run.ps1 -Name login1 [-Seconds 110]
#              [-InputScript tools\input\login.txt] [-Shots 10,25] [-Instance 1]
#              [-BackendLog path\backend-requests.log] [-OutDir test-runs] [-DataDir dir]
#
# -BackendLog: the request log of a backend you run locally; without it no requests are listed.
param([string]$Name = 'run', [int]$Seconds = 110, [int]$Instance = 1, [string]$InputScript = '',
      [string]$Shots = '10,25', [string]$BackendLog = '', [string]$OutDir = '', [string]$DataDir = '')
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
if (-not $InputScript) { $InputScript = Join-Path $PSScriptRoot 'input\login.txt' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'test-runs' }
if (-not $DataDir) { $DataDir = if ($env:FZH_DATA_DIR) { $env:FZH_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'com.fzh-resurrection.launcher' } }
$run = if ($Instance -le 1) { Join-Path $DataDir 'run' } else { Join-Path $DataDir "run$Instance" }
$out = Join-Path $OutDir $Name
$shotsDir = Join-Path $run 'user\screenshots'

$marker = "=== $(Get-Date -Format HH:mm:ss) test run $Name"
if ($BackendLog) { Add-Content $BackendLog $marker }
New-Item -ItemType Directory -Force $out | Out-Null
Get-ChildItem $shotsDir -Filter *.png -ErrorAction SilentlyContinue | Remove-Item -Force

$env:SHADPS4_SHOT_SECONDS = $Shots
try {
    powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'run.ps1') -Seconds $Seconds `
        -InputScript (Resolve-Path $InputScript).Path -Instance $Instance -DataDir $DataDir | Select-Object -Last 1
} finally {
    Remove-Item Env:SHADPS4_SHOT_SECONDS -ErrorAction SilentlyContinue
}

$log = Join-Path $out 'shad_log.txt'
Copy-Item (Join-Path $run 'user\log\shad_log.txt') $log
Get-ChildItem $shotsDir -Filter *.png -ErrorAction SilentlyContinue | Move-Item -Destination $out -Force

$crash = Select-String -Path $log -Pattern 'Unhandled Exception|Assertion Failed' | Select-Object -First 1
if ($crash) { 'CRASH: ' + $crash.Line.Substring(0, [Math]::Min(200, $crash.Line.Length)) } else { 'no crash' }
if ($BackendLog) {
    $lines = Get-Content $BackendLog
    $i = [Array]::LastIndexOf($lines, $marker)
    $lines[($i + 1)..($lines.Count - 1)] | Where-Object { $_ -match '^\d\d:\d\d:\d\d |^  -> ' } |
        ForEach-Object { $_.Substring(0, [Math]::Min(170, $_.Length)) }
}
Get-ChildItem $out -Filter *.png | Sort-Object Name | ForEach-Object { 'screenshot {0} {1}' -f $_.Name, $_.Length }
"Kept in $out"
