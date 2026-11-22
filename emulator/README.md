# Emulator

A modified [shadPS4](https://github.com/shadps4-emu/shadPS4) that shows the emulated PlayStation VR headset in a PC headset through OpenXR. It is vendored from the `main` branch of [Farpoint PC VR](https://github.com/evertec82/Farpoint), which derives from [AstroQuest](https://github.com/bigmak94/AstroQuest), [shadps4-arm64](https://github.com/zenithblue-oss/shadps4-arm64) and shadPS4. On top of that it adds the shadNet network stack from upstream shadPS4, P2P with a relay fallback, invitations answered in the headset, and GPU-thread and memory work for Firewall Zero Hour.

License: GPL-2.0-or-later (`LICENSE`). Third-party code keeps its own license (`LICENSES/`, `REUSE.toml`, and each submodule under `externals/`).

## Building on Windows

```
git submodule update --init --recursive
powershell -ExecutionPolicy Bypass -File emulator\build.ps1 -LlvmDir C:\path\to\LLVM
```

The requirements are listed at the top of `build.ps1`. The executable is written to `emulator\build\win\shadps4.exe` unless `-BuildDir` names another folder.
