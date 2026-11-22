# Building

## Emulator

`emulator/` is the modified shadPS4 tree. Its build instructions are in
[emulator/README.md](../emulator/README.md), the requirements at the top of `emulator/build.ps1`:

```
git submodule update --init --recursive
powershell -ExecutionPolicy Bypass -File emulator\build.ps1 -LlvmDir C:\path\to\LLVM
```

The launcher bundle expects `shadps4.exe` in `emulator/build/win/`; set `FZH_EMULATOR_BUILD` to use
another folder.

## Launcher

Windows 10 or 11 x64 with:

- Node.js 20 or newer
- Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools
- WebView2 (included in Windows 11)

```
cd launcher
npm ci
npm run dev            # the UI in a browser, with the simulated backend (src/backend.mock.ts)
npm run tauri dev      # the real app; uses the emulator and patches from the development overrides
```

Checks:

```
npm run typecheck
npm run check:i18n     # keys used in src/ and src-tauri/src/ exist in copy/en.json and copy/it.json
npm test               # translator, bundle parity, key-art crop
cd src-tauri && cargo test --lib
```

`cargo test --test live_pkg` and `cargo test --test live_shadnet` are ignored by default: they need
real PKG files or a throwaway shadNet server (see the header of each file).

### Release build

```
set FZH_DEFAULT_SERVER=203.0.113.10
npm run tauri build
```

A release build also needs the updater signing key (`TAURI_SIGNING_PRIVATE_KEY`,
[releasing.md](releasing.md)). Without it the installer is written and the build then fails on the
missing signature; for a local build add
`-- --config "{\"bundle\":{\"createUpdaterArtifacts\":false}}"`.

`npm run tauri build` runs `npm run build`, then `scripts/bundle-emulator.mjs`, which fills
`src-tauri/bundle/` with the emulator, the Visual C++ runtime DLLs it imports, `commit.txt` and the
licence texts. The NSIS installer ends up in `src-tauri/target/release/bundle/nsis/`.

| Build-time variable | Default | Effect |
|---|---|---|
| `FZH_DEFAULT_SERVER` | `127.0.0.1` | Server address used when "Use a private server" is off (format below). Never shown in the UI. An address that does not parse falls back to `127.0.0.1`. |
| `FZH_EMULATOR_BUILD` | `emulator/build/win` | Folder holding `shadps4.exe`. |
| `FZH_EMULATOR_SOURCE` | `emulator` | Source tree for the commit and the licence files. |
| `FZH_VC_CRT` | newest local `Microsoft.VC14x.CRT` | Folder with the x64 Visual C++ runtime DLLs. |

`FZH_DEFAULT_SERVER` is the only place the public server's address lives: it is compiled in and is
in no file of the repo. It takes the same address a player types for a private server:

- `host`: every service on its standard port (shadNet TCP 31313 and UDP 31314, WebAPI TCP 31315,
  backend TCP 1338, relay UDP 31316);
- `host:game:matching:webapi:backend:relay`: a host that hands out its own ports;
- either form followed by `/relayhost` or `/relayhost:port`: the connection relay runs on another
  machine (port 31316 when omitted; the relay port of the first part is then unused);
- either form followed by `/` and up to 8 relays separated by `,` (relays in several regions,
  [self-hosting.md](self-hosting.md#hosting-only-a-relay)).

The link behind About > Source code and in `licences/README.txt` is fixed to
https://github.com/lilium360/FZH-Resurrection (`src-tauri/src/commands.rs`,
`scripts/bundle-emulator.mjs`).

For example the current public test server, which will change:
`sg-sgp08.altare.global:25885:25923:25923:25231:25885`.

The bundled resources also include `patches/*.xml` and `tools/input/warmup.txt`
(`src-tauri/tauri.conf.json`).

### Development overrides

At run time, environment variables or `<data>\dev.json` (`"emulator"`, `"patches"`) replace the
bundled parts: `FZH_DATA_DIR`, `FZH_EMULATOR`, `FZH_PATCHES`, `FZH_WARMUP_SCRIPT`.
`<data>` is `%LOCALAPPDATA%\com.fzh-resurrection.launcher`. For `npm run tauri dev`:

```
set FZH_EMULATOR=%CD%\..\emulator\build\win\shadps4.exe
set FZH_PATCHES=%CD%\..\patches
set FZH_WARMUP_SCRIPT=%CD%\..\tools\input\warmup.txt
```

The front end and the Rust side talk through the commands and events in
[launcher-contract.md](launcher-contract.md).
