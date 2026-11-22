# Starts Firewall Zero Hour (CUSA11182) in the emulator with the launcher's game files, outside
# the launcher. The launcher must have prepared the game once (game links, mod pak, patches) and
# signed in, so that <data>\run\user holds config.json and users.json.
#
#   powershell -ExecutionPolicy Bypass -File tools\run.ps1 [-VR] [-Aim] [-Fps] [-Microphone | -NoMicrophone]
#              [-Seconds N] [-InputScript file.txt] [-Instance N] [-DataDir dir] [-Emulator shadps4.exe]
#
# -VR            real headset through OpenXR; without it the game shows in a window with a virtual headset
# -Aim           emulate the Aim controller instead of the DualShock 4
# -Seconds N     kill the emulator after N seconds
# -InputScript   timed inputs, see docs\testing.md
# -Instance N    a second (third...) instance in <data>\run<N>, with its own shadNet account and P2P port
# -DataDir       the launcher's data folder (default FZH_DATA_DIR, else %LOCALAPPDATA%\com.fzh-resurrection.launcher)
# -Emulator      default FZH_EMULATOR, else emulator\build\win\shadps4.exe in this repository
# FZH_LIVE_INPUT names an input script the emulator re-reads whenever it changes.
param([switch]$VR, [switch]$Aim, [switch]$Microphone, [switch]$NoMicrophone, [switch]$Fps,
      [int]$Seconds = 0, [string]$InputScript = '', [int]$Instance = 1,
      [string]$DataDir = '', [string]$Emulator = '')
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
if (-not $DataDir) { $DataDir = if ($env:FZH_DATA_DIR) { $env:FZH_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'com.fzh-resurrection.launcher' } }
if (-not $Emulator) { $Emulator = if ($env:FZH_EMULATOR) { $env:FZH_EMULATOR } else { Join-Path $repo 'emulator\build\win\shadps4.exe' } }
$run = if ($Instance -le 1) { Join-Path $DataDir 'run' } else { Join-Path $DataDir "run$Instance" }
$game = Join-Path $DataDir 'game\CUSA11182\eboot.bin'
$config = Join-Path $run 'user\config.json'

if (-not (Test-Path $Emulator)) { throw "Emulator not found: $Emulator (build it, or set -Emulator / FZH_EMULATOR)" }
if (-not (Test-Path $game)) { throw "Game not found: $game (choose the game folder in the launcher first)" }
if (-not (Test-Path $config) -and $Instance -gt 1) {
    $first = Join-Path $DataDir 'run\user'
    New-Item -ItemType Directory -Force (Join-Path $run 'user') | Out-Null
    foreach ($f in 'config.json', 'host_overrides.json') {
        if (Test-Path (Join-Path $first $f)) { Copy-Item (Join-Path $first $f) (Join-Path $run "user\$f") }
    }
}
if (-not (Test-Path $config)) { throw "Instance configuration not found: $config (start the game once from the launcher)" }
if ($Instance -gt 1 -and -not (Test-Path (Join-Path $run 'user\users.json'))) {
    throw "No account for instance ${Instance}: copy $DataDir\run\user\users.json to $run\user\ and set another shadNet account (shadnet_npid, shadnet_password) in it"
}

function Set-ConfigValue($root, [string]$section, [string]$key, $value) {
    if (-not $root.PSObject.Properties[$section]) { $root | Add-Member -NotePropertyName $section -NotePropertyValue ([pscustomobject]@{}) }
    $root.$section | Add-Member -NotePropertyName $key -NotePropertyValue $value -Force
}
$cfg = Get-Content $config -Raw | ConvertFrom-Json
Set-ConfigValue $cfg 'Input' 'use_special_pad' ([bool]$Aim)
Set-ConfigValue $cfg 'Input' 'special_pad_class' 9
# Without a headset the default microphone can be the headset's virtual one, which gives nothing
# and is slow to open; so no microphone unless asked for.
Set-ConfigValue $cfg 'Audio' 'sdl_mic_device' $(if (-not $NoMicrophone -and ($VR -or $Microphone)) { 'Default Device' } else { 'None' })
# Two instances on one PC need different P2P ports.
Set-ConfigValue $cfg 'General' 'p2p_port' (3657 + [Math]::Max($Instance, 1))
$cfg | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $config -Encoding UTF8

$env:SHADPS4_VR = '1'
$env:SHADPS4_OPENXR = if ($VR) { '1' } else { '0' }
$env:SHADPS4_XR_WAIT = '0'; $env:SHADPS4_XR_PAUSE = '0'
$env:SHADPS4_XR_GAMEPAD_LAYOUT = '1'; $env:SHADPS4_XR_PAD_HAND = 'right'
$env:SHADPS4_VR_PACE = '1'
# The game draws both eyes in one instanced call and relies on GPU clip distances to keep them
# apart; without it the right eye shows a wrong view.
$env:SHADPS4_NATIVE_CLIP_DISTANCE = '1'
# The game attaches the weapon to the Aim's glowing sphere at its tip, while the hand holds the
# trigger grip: metres right, up, towards the player (negative = forward).
if ($Aim -and -not $env:SHADPS4_XR_PAD_OFFSET) { $env:SHADPS4_XR_PAD_OFFSET = '0,0.07,-0.30' }
$env:SHADPS4_TITLE_RESOLUTION = 'title'; $env:SHADPS4_TITLE_EYE_WIDTH = ''
$env:SHADPS4_INPUT_SCRIPT = $InputScript
$env:SHADPS4_LIVE_INPUT = if ($env:FZH_LIVE_INPUT) { $env:FZH_LIVE_INPUT } else { '' }

$arguments = @('-g', ('"' + $game + '"'))
if ($Fps) { $arguments += '--show-fps' }
$p = Start-Process -FilePath $Emulator -WorkingDirectory $run -ArgumentList $arguments -PassThru
Write-Host "Started (PID $($p.Id)). Log: $run\user\log\shad_log.txt"
if ($Seconds -gt 0) {
    if (-not $p.WaitForExit($Seconds * 1000)) { Stop-Process -Id $p.Id -Force; Write-Host "Stopped after $Seconds s." }
} else {
    $p.WaitForExit()
}
Write-Host "Exit code: $($p.ExitCode)"
