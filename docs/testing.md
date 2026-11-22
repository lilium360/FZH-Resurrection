# Testing

## Unit checks

See [building.md](building.md): `npm run typecheck`, `npm run check:i18n`, `npm test` and
`cargo test --lib` in `launcher/`. The pak tests compare the Rust output byte for byte with golden
files written by `tools/modpak.py` (`launcher/src-tauri/tests/fixtures/`); regenerate them with:

```
python tools/modpak.py pack launcher/src-tauri/tests/fixtures/pak-src launcher/src-tauri/tests/fixtures/reference.pak
python tools/modpak.py mod --ini-dir launcher/src-tauri/tests/fixtures/pak-src --out launcher/src-tauri/tests/fixtures/mod-reference.pak <lines of mod-inputs.txt>
```

## A local server

On the same PC, with shadNet built from `server/shadnet/` (or a Linux machine with `server/bare/`):

```
python server/backend/backend.py --data run/backend-data.json --log run/backend-requests.log
python server/relay/relay.py --host 127.0.0.1
FZH_REGISTRATION_KEY=<key> python server/account/create_account.py 127.0.0.1 31313 Player1 player1@example.invalid
```

The launcher uses `127.0.0.1` when built without `FZH_DEFAULT_SERVER`, or as a private server.

## Runs without the launcher UI

The tools in `tools/` use the launcher's data folder (`FZH_DATA_DIR`, default
`%LOCALAPPDATA%\com.fzh-resurrection.launcher`): choose the game folder and sign in once in the
launcher, start the game once from it, then:

```
powershell -ExecutionPolicy Bypass -File tools\run.ps1 [-VR] [-Aim] [-Seconds 120] [-InputScript tools\input\login.txt]
powershell -ExecutionPolicy Bypass -File tools\test-run.ps1 -Name login1 -Seconds 110 -InputScript tools\input\login.txt
powershell -ExecutionPolicy Bypass -File tools\warmup-cache.ps1
```

`test-run.ps1` keeps `shad_log.txt` and the screenshots in `test-runs\<name>\` and prints the exit,
the first crash line and, with `-BackendLog`, the backend requests of the run.

The emulator is `emulator\build\win\shadps4.exe` unless `-Emulator` or `FZH_EMULATOR` says otherwise.

### Input scripts

One step per line: `<start seconds> <hold seconds> <input>...`, times from emulator start; `#`
starts a comment. Inputs: button names (`cross`, `circle`, `square`, `triangle`, `up`, `down`,
`left`, `right`, `l1`, `r1`, `l2`, `r2`, `l3`, `r3`, `options`, `touchpad`), sticks (`lx=`, `ly=`,
`rx=`, `ry=`, 0..255, 128 centred), `worn=1` (headset on), `padrot=yaw,pitch,roll` (controller
direction in degrees, kept until changed). The emulator's `src/input/scripted_input.h` lists the
rest. With `FZH_LIVE_INPUT=<file>`, `run.ps1` passes a script the emulator re-reads whenever it
changes, to drive a running game.

| Script | What it does |
|---|---|
| `login.txt` | LOGIN on the title screen |
| `solo.txt` | LOGIN, TRAINING > SOLO (no network needed) |
| `public-match-first.txt`, `public-match-second.txt` | a public TRAINING match between two instances; the second picks PUBLIC later and finds the first one's room |
| `private-lobby.txt` | LOGIN, TRAINING > PRIVATE |
| `invite-host.txt`, `invite-host-slow.txt`, `invite-guest.txt` | invitation between two instances (below) |
| `warmup.txt` | the shader cache warm-up, also bundled with the launcher |

The menu positions assume the pad is still while a screen loads: the game takes the pad's pose at
that moment as its reference.

## Two instances on one PC

Each instance needs its own shadNet account and P2P port. `run.ps1 -Instance 2` uses
`<data>\run2\user`: on first use it copies instance 1's `config.json` and `host_overrides.json`;
copy `users.json` there yourself and change `shadnet_npid` / `shadnet_password` to a second
account. `p2p_port` is set to 3657 + instance number. The second instance picks PUBLIC later and
finds the first one's room; when the match starts follows the game's own lobby rules (the mod pak
changes them only for the warm-up):

```
start powershell -File tools\test-run.ps1 -Name pub1 -Instance 1 -Seconds 300 -InputScript tools\input\public-match-first.txt
start powershell -File tools\test-run.ps1 -Name pub2 -Instance 2 -Seconds 300 -InputScript tools\input\public-match-second.txt
```

Invitations: the two accounts must be friends. Set `SHADPS4_DBG_INVITE_AUTO=<seconds>` for both
runs: the invitation dialog then picks the first friend and sends, and the invitee's prompt is
accepted, that many seconds after they appear. Run `invite-host-slow.txt` for the host when both
instances start together (the guest must be in the main menu first) and `invite-guest.txt` for
the guest.
