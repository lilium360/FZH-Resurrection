# FZH Resurrection

FZH Resurrection runs Firewall Zero Hour, the PS4 game for PlayStation VR, on a PC VR headset and
brings back its online play. It is made of a modified shadPS4 emulator that renders to the headset
through OpenXR, a Windows launcher that sets up the game from your own dump, and a server you can
host yourself.

## State

A small group of players uses it to play online together. What is known:

- Only the European release, CUSA11182, with update 1.43 is supported. The launcher refuses the
  USA release (CUSA09831) and other versions of the update.
- Not supported: DLC, PKG files with a real passcode, and game folders on network drives or
  non-NTFS volumes.
- Not verified: an update dump kept as a separate folder, as opposed to a merged one; whether the relay carries a match for a player who refused the Windows Firewall
  prompt; how many players one server can carry.

## What you need

- Your own, legally obtained dump of Firewall Zero Hour: the base game CUSA11182 and update 1.43,
  as PKG files or an extracted folder. This repository contains no game data and never will. When
  installing from PKG files the launcher extracts the game, which takes about 40.1 GB.
- A PC VR headset with an OpenXR runtime. The game is played with the Aim controller (emulated
  on the right VR controller) or as with a DualShock 4.
- 64-bit Windows.

## Getting it

Installers are published as [GitHub Releases](https://github.com/lilium360/FZH-Resurrection/releases)
of this repository. The installer needs no administrator rights. A launcher built with the
project's update key checks the latest release at start and installs an update only if its
signature matches ([docs/releasing.md](docs/releasing.md)).

The launcher's setup assistant asks for the game folder, installs the game from PKG files if
needed, prepares it (links to your dump, the emulator patches and a small mod file, all in the
launcher's data folder; your dump is never modified) and creates your account. An optional shader
warm-up compiles the game's shaders before you play by letting the game play solo training matches
in a window on the monitor, 12 to 46 minutes in all.

On the first launch Windows may ask whether the emulator can use networks: choose Allow, at least
for private networks, so other players can connect to you directly.

## Playing online

- **Accounts.** You choose a user name (Online ID) and nothing else: no email, no password to
  remember. The launcher shows a recovery code once. It is the only way to use the account on
  another PC or after reinstalling. Keep it safe: it cannot be shown again, reset or changed, and the server
  never sees it.
- **Friends.** Friend requests are sent and accepted in the launcher by Online ID.
- **Matches.** The game's own public matchmaking and private lobbies run on the server.
  Invitations to a private lobby are sent from the game to friends and answered inside the
  headset.
- **Connections.** Matches run directly between the players' PCs. When two players cannot reach
  each other, their traffic goes through a relay.

An account belongs to the server it was created on. To play on another server, turn on "Use a
private server" in the launcher's Advanced screen and enter the address its owner gives you.

## Hosting a server

A server is three services: shadNet (the PlayStation Network replacement: accounts, friends,
lobbies), a backend (player progression, avatars, account registration) and the relay. Anyone can
run their own, on Linux with Docker or as plain processes; the services need little (one CPU and
1 GB of memory for a group of friends). See [docs/self-hosting.md](docs/self-hosting.md).

## Help wanted: servers and relays

The public online servers run on a free, low-powered VPS. If you can contribute a VPS, and above
all if you can host just a relay in another region, please get in touch through this repository's
[Issues](https://github.com/lilium360/FZH-Resurrection/issues) or
[Discussions](https://github.com/lilium360/FZH-Resurrection/discussions).

Why relays matter: a relay only carries traffic when two players cannot reach each other directly.
When it does, the relay's distance from the players becomes their latency, so a relay on another
continent makes the match lag.

What a relay takes: one Python script (`server/relay/relay.py`, standard library only), one inbound
UDP port (31316 by default), a few megabytes of memory and nothing stored on disk. A server address
can list up to 8 relays after a `/`, for example `play.example.org/relay-eu.example.org,relay-us.example.org:40000`;
the emulator registers with all of them and two players use the one with the shorter round trip.
For the official server the address is built into the launcher, so a new relay reaches players with
the next release. Details in [docs/self-hosting.md](docs/self-hosting.md#hosting-only-a-relay).

## Building from source

Requirements and details are in [docs/building.md](docs/building.md) and
[emulator/README.md](emulator/README.md): Visual Studio 2022 or later with the C++ tools, LLVM 19 or later,
Node.js 20 or later and Rust.

```
git clone --recurse-submodules https://github.com/lilium360/FZH-Resurrection
cd FZH-Resurrection
powershell -ExecutionPolicy Bypass -File emulator\build.ps1 -LlvmDir C:\path\to\LLVM
cd launcher
npm ci
npm run tauri build -- --config "{\"bundle\":{\"createUpdaterArtifacts\":false}}"
```

The installer is written to `launcher/src-tauri/target/release/bundle/nsis/`. Built this way the
launcher uses a server on `127.0.0.1`; [docs/building.md](docs/building.md) explains how to build
in another default server, and [docs/testing.md](docs/testing.md) how to run a local one.

## Repository layout

| Folder | Contents |
|---|---|
| `emulator/` | The modified shadPS4: OpenXR output, the shadNet network stack, peer-to-peer play with relay fallback, the in-headset invitation banner |
| `launcher/` | The Windows launcher (Tauri 2, TypeScript and Rust): game installation and preparation, settings, accounts, friends, mods, updates |
| `server/` | shadNet build recipe, backend, relay, account tool, Docker and plain set-ups |
| `patches/` | Emulator patches for the game's executable (update 1.43) |
| `tools/` | Test scripts that run the game without the launcher, scripted inputs, the mod file reference implementation |
| `docs/` | [Building](docs/building.md), [self-hosting](docs/self-hosting.md), [testing](docs/testing.md), [releasing](docs/releasing.md), [launcher contract](docs/launcher-contract.md) |

## Credits and licence

The emulator is based on [shadPS4](https://github.com/shadps4-emu/shadPS4), through the
[Farpoint PC VR fork](https://github.com/evertec82/Farpoint) (which itself derives from AstroQuest
and shadps4-arm64). Online play uses [shadNet](https://github.com/shadps4-emu/shadNet), built
unmodified by `server/shadnet/`.

FZH Resurrection is licensed under the GNU General Public License, version 2 or (at your option)
any later version ([LICENSE](LICENSE)). Components and their licences are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

This project is not affiliated with or endorsed by First Contact Entertainment, Sony Interactive
Entertainment or Sony. It distributes no game content: you need your own copy of the game.
