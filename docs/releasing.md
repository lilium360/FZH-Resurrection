# Releasing

Releases are GitHub Releases of https://github.com/lilium360/FZH-Resurrection. Installed launchers
check `https://github.com/lilium360/FZH-Resurrection/releases/latest/download/latest.json` at start
and from About > Check for updates (Tauri updater plugin, `plugins.updater` in
`launcher/src-tauri/tauri.conf.json`). An update is installed only if its signature matches the
public key built into the launcher, and only for the version the signature was made for
(`requireSignedVersion`).

## Signing key (once)

```
cd launcher
npm run tauri signer generate -- -w %USERPROFILE%\.tauri\fzh-resurrection.key
```

- The private key (`fzh-resurrection.key`) and its password stay with the owner, outside the repo
  (a password manager, plus an offline backup). Losing either means installed launchers can never
  update again: players would have to reinstall by hand.
- The public key (`fzh-resurrection.key.pub`) goes, as its text content, into
  `launcher/src-tauri/tauri.conf.json` > `plugins.updater.pubkey`, in place of
  `UPDATER-PUBLIC-KEY-NOT-SET`. Commit that change. While the placeholder is there the launcher
  builds and runs, but updates are off: About shows no update controls and each start logs
  "Updates are off: this build has no update key."

## The release script

`tools\release.ps1` does the steps of "Building a release" from 3 on, "latest.json" and, with
`-Publish`, "Publishing". It refuses to run without the update key in `tauri.conf.json`, with
uncommitted changes, with different versions in the three files, or when the tag exists.

```
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\fzh-resurrection.key"
powershell -ExecutionPolicy Bypass -File tools\release.ps1 -Server <public server address> -Notes "First public release" -Publish
```

The key is `TAURI_SIGNING_PRIVATE_KEY`, else `%USERPROFILE%\.tauri\fzh-resurrection.key`. The
password is `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, else the one kept in Windows Credential Manager,
else the script asks for it, which keeps it out of the command history. `-StorePassword` asks
for the password once and keeps it in Credential Manager as `FZH Resurrection/update-key`,
readable only by this Windows user; `-ForgetPassword` removes it. Whoever can run programs as
that user can then sign an update: store it only on a PC you trust.

Without `-Publish` the script stops after writing the installer, its `.sig` and `latest.json` in
`launcher/src-tauri/target/release/bundle/nsis/`.

## Building a release

1. Build the emulator (`emulator/`) in Release; `shadps4.exe` must come from a clean tree, so the
   bundled `commit.txt` has no `-dirty` suffix.
2. Set the same version (SemVer, e.g. `0.2.0`) in `launcher/package.json`,
   `launcher/src-tauri/Cargo.toml` and `launcher/src-tauri/tauri.conf.json`; run `npm install` and
   `cargo check` once so `package-lock.json` and `Cargo.lock` follow. The updater only offers a
   version higher than the installed one.
3. Build, from `launcher/`, in a shell that holds the key (never in a file of the repo; `.env` files
   are not read for these variables):

   ```
   set TAURI_SIGNING_PRIVATE_KEY=%USERPROFILE%\.tauri\fzh-resurrection.key
   set TAURI_SIGNING_PRIVATE_KEY_PASSWORD=<password>
   set FZH_DEFAULT_SERVER=<public server address>
   npm run tauri build
   ```

   `TAURI_SIGNING_PRIVATE_KEY` takes the path of the key file or its content. See
   [building.md](building.md) for `FZH_DEFAULT_SERVER`. The installer's source link points at the
   repository; the GPL requires the corresponding source to be offered with the binaries, which the
   tag `v<version>` of the release commit does (Publishing, below), so build from that commit.
4. In `launcher/src-tauri/target/release/bundle/nsis/`:
   - `FZH Resurrection_<version>_x64-setup.exe`: the installer, for new players and for the updater;
   - `FZH Resurrection_<version>_x64-setup.exe.sig`: its signature (one line of base64 text).

   Neither holds game files, accounts or keys. Without `TAURI_SIGNING_PRIVATE_KEY` the build writes
   the installer, then stops with "A public key has been found, but no private key" (exit code 1)
   and no `.sig`. For a local test build without the key, turn the updater artifacts off:
   `npm run tauri build -- --config "{\"bundle\":{\"createUpdaterArtifacts\":false}}"`.

## latest.json

Write it by hand next to the installer:

```json
{
  "version": "0.2.0",
  "notes": "- First change\n- Second change",
  "pub_date": "2026-12-01T12:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<the whole content of the .sig file>",
      "url": "https://github.com/lilium360/FZH-Resurrection/releases/download/v0.2.0/FZH.Resurrection_0.2.0_x64-setup.exe"
    }
  }
}
```

- `version` must equal the version built in; `pub_date` is RFC 3339.
- `notes`: one change per line; About shows each line as a list item (a leading `-` or `*` is
  dropped).
- `url` is the download link of the uploaded installer. GitHub replaces the spaces of an uploaded
  file name with dots: copy the link from the published release rather than typing it.
- Every platform listed must be valid; list only `windows-x86_64`.

## Publishing

1. Tag the release commit `v<version>` and push the tag.
2. Create a GitHub Release on that tag (not a draft, not a pre-release: `releases/latest` skips
   both) and upload the installer, its `.sig` and `latest.json`. The launcher does not read the
   `.sig` file (the signature travels inside `latest.json`); it is there for anyone who wants to
   check the installer.
3. Check that `https://github.com/lilium360/FZH-Resurrection/releases/latest/download/latest.json`
   returns the new file.

## Checking an update

On a PC with the previous version installed (from its own installer):

1. Start the launcher. The log shows "Launcher <version> available" and a banner offers the update.
2. About > What's new shows the notes; Download shows the progress, then "Version <version> ready".
3. Restart and update asks for confirmation; the launcher closes, the installer runs without
   questions (`installMode: passive`) and the new version opens. About shows the new version.
4. With the game or the shader warm-up running, Download and Restart and update are refused. A
   download whose signature does not match ends with "Update download failed" and installs nothing.
