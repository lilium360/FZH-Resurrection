# Self-hosting a server

Online play needs three services on one machine that every player can reach:

| Service | Ports | Code |
|---|---|---|
| shadNet, the PSN replacement (accounts, friends, lobbies) | TCP 31313, UDP 31314, TCP 31315 | upstream shadNet, built by `server/shadnet/` |
| Progression backend (player progress, avatars, account registration) | TCP 1338 | `server/backend/backend.py` |
| P2P relay, for players who cannot connect directly | UDP 31316 | `server/relay/relay.py` |

Matches run between the players' PCs; the server only introduces them, plus the relay traffic of
players behind strict NATs. One CPU and 1 GB of memory are enough for a group of friends (building
shadNet once needs about 4 GB).

Two set-ups do the same thing: Docker (`server/docker/`) or plain processes (`server/bare/`).
Copy the whole `server/` folder to the machine (`git clone`, or `scp -r server`).

## Docker

Ubuntu 22.04/24.04, Debian 13 or Oracle Linux 8/9, x86-64 or ARM64.

```
cd server/docker
sudo bash install.sh
```

It installs Docker if missing, creates `/srv/fzh` with a new registration key and `shadnet.cfg`,
opens the five ports in the machine's firewall (ufw, firewalld or iptables), builds the images
(the first shadNet build takes 5-15 minutes) and starts everything. Running it again is safe.
The services restart by themselves after a reboot.

| Command | What it does |
|---|---|
| `sudo bash status.sh` | services, listening ports, number of accounts, last log lines |
| `sudo bash stop.sh` | stops the services; data stays (start again with `install.sh`) |
| `sudo bash update.sh` | rebuilds and restarts after copying a new `server/` folder |
| `sudo bash create-account.sh <OnlineID> <email>` | creates an account as admin (password asked without echo) |

Settings: `server/docker/.env.example` (copy to `.env`); the backend's are explained under
"Backend settings" below.

## Without Docker

For a machine or sandbox where Docker or systemd is not available. Needs `python3` (3.9+), `git`,
`cmake`, `ninja`, `g++` and `python3-venv` to build shadNet once.

```
cd server/bare
cp config.env.example config.env     # optional: install folder, ports, backend settings
bash install.sh                      # builds shadNet with Qt 6.8 from aqtinstall, writes shadnet.cfg
bash start.sh
bash status.sh
bash stop.sh
bash create-account.sh <OnlineID> <email>
```

Processes are tracked by pid files in `$FZH_ROOT/run`; output goes to `$FZH_ROOT/logs`. Nothing
restarts them after a reboot: run `start.sh` again (or from cron `@reboot`).

When the hosting provider assigns port numbers instead of letting you choose them, put the
assigned numbers in `config.env`; players then enter the server as
`host:game:matching:webapi:backend:relay` (for example `203.0.113.10:40001:40002:40003:40004:40005`).

## Hosting only a relay

Players who cannot reach each other directly talk through a relay, so a relay far from them adds
lag. Volunteers in other regions can run just a relay: one small Python process (`python3`
3.9+, standard library only, a few MB of memory), one inbound UDP port, nothing stored on disk.
Copy `server/relay/relay.py` to the machine and start it:

```
python3 relay.py --host 0.0.0.0 --port 31316
```

It prints, every 10 seconds when they change, how many players are registered and how many
packets it forwarded per pair. Keep it running with whatever the machine offers (systemd, cron
`@reboot`, a `screen` session). Open the UDP port inbound in the provider's firewall.

The server operator then adds the relays to the address players type, after a `/`, separated by
commas without spaces, with `:port` when it is not 31316:

```
play.example.org/play.example.org,relay-eu.example.org,relay-us.example.org:40000
```

The list replaces the relay of the main server, so name the main server too if its relay should
stay in use. Up to 8 relays. Give every player the same list (the same address): the emulator
registers with all of them and two players agree on one relay both registered with, preferring
the shorter round trip. A relay that stops answering is dropped within a few seconds and new
connections use another, but a match that was running through it has been lost in tests.

### Adding your relay to the list

The relays of the community server are listed in [`server/relay/relays.json`](../server/relay/relays.json).
To add yours:

1. Run the relay as above, on a machine that stays on, with an address that does not change (a host
   name, or a fixed IP address) and the UDP port open inbound.
2. Check that it answers from outside its own network:

   ```
   python tools/check-relays.py relay.example.org:31316
   ```

   It prints the round trip in milliseconds, or `no answer`.
3. Fork the repository, add one entry at the end of `relays.json` and open a pull request:

   ```json
   {"address": "relay.example.org:31316", "region": "North America", "location": "Ashburn, United States", "operator": "your-github-name"}
   ```

   `address` is `host` or `host:port` (31316 when left out); `region` is one of Europe, North
   America, South America, Asia, Oceania, Africa; `operator` is the GitHub account to contact when
   the relay stops answering.

What to know before you offer one:

- A relay sees the IP addresses of the players it serves and carries their game traffic. Run
  `relay.py` as it is in this repository: it logs counts only, never payloads or addresses beyond
  the registrations it prints, and stores nothing.
- Traffic is small: about 20 MB an hour for each pair of players that goes through the relay.
- The list is compiled into the launcher when a release is built (`tools/release.ps1`), so a new
  relay reaches players with the next launcher release, and a removed one stays in older launchers
  until they update. A launcher uses up to 8 relays: the maintainers take them from the list in
  order, aiming for one or two in each region, and may leave out or remove a relay that does not
  answer (`python tools/check-relays.py` checks them all).
- To stop, open a pull request that removes your entry, or an issue; then keep the relay running
  until the next launcher release if you can.

The services of the community server other than the relays are the build-time `FZH_DEFAULT_SERVER`
of the launcher ([building.md](building.md#release-build)).

### Relay status

The launcher's Advanced screen lists the relays by region, with who hosts each one and whether
it is online. Launchers never ask the relays: they ask the backend of the server they play on
(`GET /status/v1/relays`), and the backend answers from what it found itself. It does that only
when `FZH_RELAY_LIST` names a list file: every minute it sends each relay of the file one
registration for an address that is not its own, which a relay confirms and does not keep, and
keeps the result in memory. The file is the operator's own list, in the format of `relays.json`
above; it is read again when it changes, so a new list needs no restart. The answer leaves the
addresses out.

The setting is empty after an installation, and the backend then has no such endpoint (`404`):
the launcher says that the server publishes no relay list. To turn it on, copy your list
next to the backend's data and name it in `config.env` or `.env`, then restart the backend:

```
cp relays.json "$FZH_ROOT/data/relays.json"        # without Docker
FZH_RELAY_LIST="$FZH_ROOT/data/relays.json"        # in config.env

sudo cp relays.json /srv/fzh/backend/relays.json   # Docker: the backend sees this folder as /data
FZH_RELAY_LIST=/data/relays.json                   # in .env
```

## Network

Open inbound in the provider's network firewall (security list, security group): TCP 31313,
UDP 31314, TCP 31315, TCP 1338, UDP 31316 (or the ports of `config.env`). Players need no open
ports at home.

## Accounts and players

- Players create their account in the launcher: they choose an Online ID and get a recovery code,
  nothing else. The launcher asks the backend (`POST /account/v1/register`), and the backend
  creates the shadNet account with the registration key. The key (`registration-key.txt` in the
  data folder) never leaves the server; the backend reads it at each registration.
- The recovery code lets a player sign in from another PC. The server never sees it: the
  launcher turns it into the account password, so a lost code cannot be recovered from the server
  and a leaked one cannot be revoked (shadNet has no password change).
  [launcher-contract.md](launcher-contract.md#backend-account-api), "Backend account API",
  describes the scheme.
- Before each game start the launcher gets a one-time code from shadNet and hands it to the
  backend (`POST /account/v1/play`). Only then does the game's login work for that account, and
  only from that player's address; the session lasts 12 hours (`FZH_SESSION_HOURS`).
- `create-account.sh` creates accounts as admin (any password, no recovery code).
  `server/account/create_account.py` also works from another PC, but the password and key then
  travel unencrypted.
- Players turn on "Use a private server" in the launcher's Advanced screen and enter the
  server's address.

### Backend settings

In `.env` (Docker) or `config.env` (without Docker); an empty value keeps the default.

| Setting | Default | Meaning |
|---|---|---|
| `FZH_GAME_LOGIN` | `grant` (`open` when the backend listens on loopback) | `grant`: the game's login works only for an account the launcher vouched for, from the same address. `open`: any login, for launchers that do not call `/account/v1/play` yet. |
| `FZH_SESSION_HOURS` | `12` | Lifetime of a play grant and of a game session. |
| `FZH_REGISTER_PER_DAY` | `3` | Accounts one address may register per 24 hours. |
| `FZH_REGISTER_COOLDOWN` | `60` | Seconds between two registrations from one address. |
| `FZH_REGISTER_GLOBAL_PER_HOUR` | `30` | Accounts registered per hour from all addresses together. |
| `FZH_RESERVED_NAMES` | empty | Online IDs refused in addition to the built-in list (admin, moderator, system, the project's names...), comma-separated. |
| `FZH_TRUSTED_PROXY_HEADER` | empty | Behind a reverse proxy: the header with the client address, e.g. `X-Forwarded-For`. |
| `FZH_TRUSTED_PROXIES` | `127.0.0.1,::1` | Addresses whose `FZH_TRUSTED_PROXY_HEADER` is believed. |
| `FZH_RELAY_LIST` | empty | A list of relays to check and publish with their status ("Relay status" above). Empty: off. |

Limits and sessions are per client address: the TCP peer, or the last entry of the proxy header
when the request comes from a trusted proxy. Leave the header empty without a proxy, or anyone
could choose their address. Players behind one public address (a household, carrier-grade NAT)
share the registration limits. Besides the settings, every address may make 30 account requests,
20 game logins and 60 relay status requests per 10 minutes. The limit counters are kept in memory and
cleared by a restart; grants and sessions are kept in `backend-auth.json`.

## Backups

Everything that matters is in the data folder (`/srv/fzh` with Docker, `$FZH_ROOT/data` without):
`shadnet/` (`shadnet.cfg`, `db/` with accounts and friends), `backend/` (`backend-data.json`,
`backend-auth.json` with the names registered through the backend and the open sessions,
`avatars/`, the request log) and `registration-key.txt`.

```
sudo tar czf fzh-backup.tar.gz -C /srv fzh
```

Restore by extracting into the same place before running `install.sh`. The backup contains the
key and shadNet's password hashes: keep it private.

## Security

- Nothing is encrypted: shadNet and the backend speak plain TCP/HTTP. The account password
  travels in clear once at registration and at each shadNet sign-in; the play code and avatar
  tokens travel in clear too. Someone who can read a player's traffic can take over the account.
- Stored secrets: shadNet keeps salted password hashes and, in clear, each account's WebAPI token
  (`db/`); the backend keeps no password, recovery code or token. The request log leaves out
  passwords, codes, tokens and the game's keys, and every body of `/account/` requests.
- The game sends nothing the backend can verify, so the backend relies on the launcher's play
  grant: with `FZH_GAME_LOGIN=grant`, logging the game in as someone else needs that player's
  shadNet password, or sharing a public address with them while their grant is active. Reading or writing a player's progress needs the
  session of that player's game login, from the same address. Still open: the match host reports
  every player's match result, so any player with a session can add bounded amounts (at most
  100,000 per stat and report) to the stats of a player with an active session whose
  `playerStatsId` they know, which every match partner learns; and anyone may read avatars and
  the game's constant data (catalogue, tuning).
- With `FZH_GAME_LOGIN=open` anyone who knows a player's shadNet account id (a small number) can
  log in as that player and read or change their progress.
- Avatar uploads need a valid shadNet token of the avatar's owner.
- The relay forwards only between registered addresses and checks that a registration comes from
  the address it claims, but anyone can register and use bandwidth.
- shadNet's admin and member APIs are off.
- Docker publishes its ports past `ufw`; the provider's network firewall is the real gate.

To change the registration key, edit `RegistrationSecretKey=` in `shadnet/shadnet.cfg`, write the
same value to `registration-key.txt` and restart the services (Docker: `stop.sh`, then
`install.sh`; without Docker: `stop.sh`, then `start.sh`). `server/shadnet/shadnet.cfg.example`
explains every setting.
