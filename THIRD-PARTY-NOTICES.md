# Third-party notices

This project is licensed under the GNU General Public License, version 2 or (at your option) any
later version (GPL-2.0-or-later); the full text is in `LICENSE`. It contains no files from
Firewall Zero Hour or any other game.

## Emulator (`emulator/`)

| Component | Source | Licence |
|---|---|---|
| shadPS4 | https://github.com/shadps4-emu/shadPS4 | GPL-2.0-or-later |
| Farpoint PC VR fork of shadPS4, the base of this tree | https://github.com/evertec82/Farpoint | GPL-2.0-or-later |

The libraries under `emulator/externals/` keep their own licences: see `emulator/LICENSES/` and the
licence file of each component. The launcher installer copies these texts into `licences/emulator/`.

## Server (`server/`)

| Component | Source | Licence |
|---|---|---|
| shadNet, built unmodified from commit `b38cf175` by `server/shadnet/` | https://github.com/shadps4-emu/shadNet | GPL-2.0-or-later |
| Qt 6, downloaded by the build recipes, not part of this repository | https://www.qt.io | the licences of the Qt modules used (LGPL-3.0 / GPL) |

## Launcher (`launcher/`)

| Component | Licence |
|---|---|
| Tauri (`tauri`, `tauri-build`, `tauri-plugin-dialog`, `tauri-plugin-opener`, `@tauri-apps/api`, `@tauri-apps/cli`) | MIT OR Apache-2.0 |
| Barlow font, Copyright 2017 The Barlow Project Authors, via `@fontsource/barlow` | SIL Open Font License 1.1 |
| Lekton font, Copyright 2008-2010 Accademia di Belle Arti di Urbino, via `@fontsource/lekton` | SIL Open Font License 1.1 |
| Visual C++ runtime DLLs placed next to the emulator in the installer | Microsoft Visual C++ redistributable licence |

Other Rust crates (`launcher/src-tauri/Cargo.lock`) are under MIT, Apache-2.0, MIT OR Apache-2.0,
BSD-3-Clause, Zlib, Unlicense OR MIT, Unicode-3.0 and CC0/MIT-0 terms; five crates
pulled in by Tauri (`cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext`, `selectors`) are
MPL-2.0, with their source on crates.io. The build-only npm packages (TypeScript, Vite and their
dependencies) are under MIT, Apache-2.0, ISC and BSD-3-Clause and are not shipped. `cargo metadata`
and `package-lock.json` give the exact licence of every package.

The launcher's icon and interface are original to this project.
