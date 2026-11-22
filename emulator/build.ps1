# Builds shadps4.exe for Windows x64 with clang-cl and Ninja.
#
# Requirements:
#   - Visual Studio 2022 or later (Build Tools are enough) with the "Desktop development with C++"
#     workload: MSVC, the Windows SDK, and the CMake and Ninja that ship with it.
#   - LLVM 19 or later for Windows (clang-cl, lld-link, llvm-rc). Pass -LlvmDir, set LLVM_DIR, or
#     put clang-cl on PATH.
#   - Submodules checked out: git submodule update --init --recursive
#   - No Vulkan SDK: Vulkan headers, loader and glslang are built from externals/.
#   - Prefer a source path without spaces; a directory junction works (mklink /J).
#
# Usage: powershell -ExecutionPolicy Bypass -File build.ps1 [-BuildDir <dir>] [-Configure] [-Jobs N] [-KeepGoing]
param(
    [string]$BuildDir = (Join-Path $PSScriptRoot 'build\win'),
    [string]$LlvmDir = $env:LLVM_DIR,
    [string]$VsDir,
    [string]$BuildType = 'Release',
    [int]$Jobs = [Environment]::ProcessorCount,
    [switch]$Configure,
    [switch]$KeepGoing
)
$ErrorActionPreference = 'Stop'
$src = $PSScriptRoot

if (-not $VsDir) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { throw 'vswhere.exe not found; pass -VsDir.' }
    $VsDir = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $VsDir) { throw 'No Visual Studio installation with the C++ tools was found; pass -VsDir.' }
}
$vcvars = Join-Path $VsDir 'VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) { throw "vcvars64.bat not found under $VsDir" }

foreach ($line in (cmd /c "`"$vcvars`" >nul && set")) {
    if ($line -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}

if ($LlvmDir) {
    $llvmBin = if (Test-Path (Join-Path $LlvmDir 'bin\clang-cl.exe')) { Join-Path $LlvmDir 'bin' } else { $LlvmDir }
} else {
    $clang = Get-Command clang-cl.exe -ErrorAction SilentlyContinue
    if (-not $clang) { throw 'clang-cl not found; pass -LlvmDir or set LLVM_DIR.' }
    $llvmBin = Split-Path $clang.Source
}
if (-not (Test-Path (Join-Path $llvmBin 'clang-cl.exe'))) { throw "clang-cl.exe not found in $llvmBin" }
$env:PATH = "$llvmBin;$env:PATH"

$cmake = (Get-Command cmake.exe -ErrorAction SilentlyContinue).Source
if (-not $cmake) { throw 'cmake not found on PATH or in Visual Studio.' }
$ninja = (Get-Command ninja.exe -ErrorAction SilentlyContinue).Source
if (-not $ninja) { throw 'ninja not found on PATH or in Visual Studio.' }

if (-not (Test-Path (Join-Path $src 'externals\sdl3\CMakeLists.txt'))) {
    throw 'Submodules are missing; run: git submodule update --init --recursive'
}

$llvm = $llvmBin.Replace('\', '/')
if ($Configure -or -not (Test-Path (Join-Path $BuildDir 'build.ninja'))) {
    & $cmake -S $src -B $BuildDir -G Ninja `
        "-DCMAKE_MAKE_PROGRAM=$($ninja.Replace('\', '/'))" `
        "-DCMAKE_C_COMPILER=$llvm/clang-cl.exe" `
        "-DCMAKE_CXX_COMPILER=$llvm/clang-cl.exe" `
        "-DCMAKE_RC_COMPILER=$llvm/llvm-rc.exe" `
        "-DCMAKE_LINKER=$llvm/lld-link.exe" `
        "-DCMAKE_BUILD_TYPE=$BuildType" `
        '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL' `
        '-DENABLE_DISCORD_RPC=OFF' '-DENABLE_UPDATER=OFF' '-DENABLE_OPENXR=ON' `
        '-DALSOFT_UPDATE_BUILD_VERSION=OFF'
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$extra = if ($KeepGoing) { @('--', '-k', '0') } else { @() }
& $cmake --build $BuildDir --target shadps4 --parallel $Jobs @extra
exit $LASTEXITCODE
