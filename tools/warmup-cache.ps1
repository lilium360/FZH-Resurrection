# Fills the shader cache outside the launcher (the launcher's "Warm up" does the same): plays solo
# TRAINING > PUBLIC matches (a random map each time) with tools\input\warmup.txt and reports how
# many new pipelines each round compiled. It stops after two rounds in a row with a match and
# fewer than 5 new pipelines. shadNet and the backend must be reachable; leave the game window alone.
#
#   powershell -ExecutionPolicy Bypass -File tools\warmup-cache.ps1 [-Rounds 8] [-Instance 1]
#              [-RoundSeconds 345] [-Server 127.0.0.1] [-DataDir dir]
#
# During the warm-up the mod pak has a 1-player lobby with an 8 s countdown. At the end the pak is
# deleted: the launcher builds its own again at the next launch (without the launcher, run
# python tools\modpak.py mod --server=<address>).
param([int]$Rounds = 8, [int]$Instance = 1, [int]$RoundSeconds = 345, [string]$Server = '127.0.0.1',
      [string]$DataDir = '')
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
if (-not $DataDir) { $DataDir = if ($env:FZH_DATA_DIR) { $env:FZH_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'com.fzh-resurrection.launcher' } }
$env:FZH_DATA_DIR = $DataDir
$run = if ($Instance -le 1) { Join-Path $DataDir 'run' } else { Join-Path $DataDir "run$Instance" }
$modpak = Join-Path $PSScriptRoot 'modpak.py'
$out = Join-Path $repo 'test-runs'

python $modpak mod 1 8 "--server=$Server" | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'modpak.py failed' }
$quiet = 0
try {
    for ($r = 1; $r -le $Rounds; $r++) {
        $name = "warmup-$r"
        powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'test-run.ps1') -Name $name `
            -Seconds $RoundSeconds -Instance $Instance -InputScript (Join-Path $PSScriptRoot 'input\warmup.txt') `
            -Shots '60,150' -OutDir $out -DataDir $DataDir | Out-Null
        $log = Join-Path $out "$name\shad_log.txt"
        $new = (Select-String -Path $log -Pattern 'Compiling graphics pipeline').Count
        $preloaded = Select-String -Path $log -Pattern 'Preloaded (\d+) pipelines' | Select-Object -First 1
        $pre = if ($preloaded) { $preloaded.Matches[0].Groups[1].Value } else { '0' }
        $crashes = (Select-String -Path $log -Pattern 'Assertion Failed|Unhandled Exception code 0xc').Count
        $match = (Select-String -Path $log -Pattern 'sceNpMatching2SearchRoom: called').Count -gt 0
        'round {0}: preloaded {1}, new {2}, match {3}, crashes {4}' -f $r, $pre, $new, $(if ($match) { 'yes' } else { 'NO' }), $crashes
        if ($match -and $new -lt 5) { $quiet++ } else { $quiet = 0 }
        if ($quiet -ge 2) { 'Two rounds in a row with almost no new pipelines: the cache is ready.'; break }
    }
} finally {
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $DataDir 'game\CUSA11182-mods\firewall\content\paks\zz-fzh-mod_P.pak')
    'Warm-up mod pak removed; the launcher rebuilds its own at the next launch.'
    "Cache: $run\user\cache\CUSA11182"
}
