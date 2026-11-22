# FZH:Resurrection — Copy style guide

## Voice

Short, direct, present tense. The launcher talks to one player ("you", "your").
State what happens, not what should happen. No hedging, no filler.

## Banned

| Rule | Example of what NOT to write |
|---|---|
| No server details | IP addresses, server hardware or VPN names, hosting instructions |
| No streaming software | Virtual Desktop, Steam Link, SSW, ALVR |
| No filler words | seamless, simply, just, please note, in order to |
| No exclamation marks | Ready to play! |
| No emoji | ✅ Game ready |
| No em-dash asides | The game — which runs on shadPS4 — starts |
| No tricolon rhythm | Fast, smooth, immersive |
| No obvious explanations | Click the button to press it |
| No promises the app can't keep | Disabled feature described as "coming soon" |

Disabled features get one honest line: "{Feature} is not available yet." Never describe how a disabled feature would work.

## Internal details

Technical internals stay out of player-facing text:
- File paths (e.g. `sce_sys/param.sfo`, `sce_sys/pic1.png`) → say "game info" or "game image"
- Component names (P2P relay, progression server) → only on the About screen or the service status panel; everywhere else say "progress", "connection relay", etc.
- Build identifiers (emulator commit hash, fork name) → only on the About screen
- Internal operations (linking folders, writing mod pak, applying patches) → say "Setting up the game files…"

## The server

The default server is called **the official server** in English, **il server ufficiale** in Italian.
A private server is named with the address the player typed ("private server 203.0.113.5").
Never say where or how any server is hosted.

## Terminology glossary

| EN | IT | Notes |
|---|---|---|
| FZH:Resurrection | FZH:Resurrection | Never translate the app name |
| launcher | launcher | Loanword, lowercase in running text |
| dump | dump | PS4 game dump; loanword in IT |
| PKG file | file PKG | The PS4 package format |
| base game | gioco base | The 1.00 disc release |
| update 1.43 | aggiornamento 1.43 | The required patch |
| game folder | cartella del gioco | Where the dump lives |
| install location | posizione di installazione | Where PKG files are extracted |
| installed copy | copia installata | The extracted result |
| prepare / preparation | preparare / preparazione | Linking + mod pak + patches |
| shader cache | cache degli shader | Loanword "cache" + "shader" |
| warm up (noun) | preriscaldamento | The solo-match compilation pass |
| warm up (verb) | preriscaldare | — |
| shader | shader | Loanword |
| emulator | emulatore | shadPS4 fork |
| headset | visore | VR headset |
| official server | server ufficiale | The default online server |
| private server | server privato | A player-hosted server |
| Online ID | Online ID | shadNet username; never translate |
| recovery code | codice di recupero | Shown once at registration; signs the account in on another PC |
| friend request | richiesta di amicizia | — |
| sign in | accedi | Imperative: accedi / accedere |
| sign out | esci | — |
| account | account | Loanword |
| avatar | avatar | Loanword |
| recovery email | email di recupero | For password reset |
| Aim controller | controller Aim | The PS VR rifle |
| DualShock 4 | DualShock 4 | Never translate product name |
| calibration | calibrazione | Grip offset |
| frame rate | frequenza fotogrammi | — |
| pixel density | densità dei pixel | — |
| sharpness | nitidezza | — |
| field of view | campo visivo | — |
| shadow quality | qualità delle ombre | — |
| anisotropic filtering | filtro anisotropico | — |
| antialiasing | antialiasing | Loanword |
| FPS counter | contatore FPS | — |
| frame diagnostics | diagnostica fotogrammi | — |
| background shader compilation | compilazione shader in sottofondo | — |
| connection relay | relay di connessione | Helps friends connect; "P2P relay" only on About/service panel |
| progress | progressi | Level, credits, unlocks; "progression server" only on About/service panel |
| network | rete | Sign-in, rooms, friends |
| problem report | segnalazione problema | — |
| setup assistant | assistente configurazione | The wizard |
| game language | lingua del gioco | In-game text language |
| launcher language | lingua del launcher | UI language |
| mod pak | mod pak | Internal; never shown standalone |

## Plurals

JSON uses `{"one": "...", "other": "..."}` objects where a count varies.
Italian: `one` for 1, `other` for 0 and 2+.
English: `one` for 1, `other` for 0 and 2+.

## Placeholders

Curly braces: `{name}`, `{count}`, `{size}`, `{version}`, `{address}`, `{mode}`.
Every placeholder in `en.json` must appear in `it.json` for the same key and vice versa.

## Capitalisation

- EN: sentence case everywhere (headings, buttons, labels). Exception: proper nouns (Aim, DualShock, FXAA, TAA, MSAA).
- IT: sentence case everywhere. Exception: proper nouns.

## Decisions

1. **dump** stays as a loanword in Italian. "Copia del gioco" is too generic (conflicts with "installed copy"). The PS4 community already uses "dump".
2. **warm up** in IT is "preriscaldamento" (noun) rather than "riscaldamento" to distinguish from generic heating.
3. **shader** stays English in both languages; the Italian "ombreggiatore" is not used in gaming.
4. **cache** stays English in Italian; "memoria temporanea" is too long and unfamiliar.
5. Setting labels (Microphone, Output, etc.) keep the same word order in IT where natural. No forced mirror of English syntax.
6. The wizard has 5 steps (new order): Launcher language, Game files, Game language, Account, Ready. The two language steps use distinct headings: "Launcher language" vs "Game language".
