#!/usr/bin/env python3
"""The mod pak: the game's own DefaultEngine.ini and PS4Engine.ini with a few keys changed, packed
as an Unreal Engine 4 pak (version 4, uncompressed) that overrides the game's pak.

This is the reference implementation of the launcher's src-tauri/src/modpak.rs and pak.rs: their
tests compare against tests/fixtures/reference.pak and mod-reference.pak, written by this tool.

  python modpak.py mod [--game-pak FILE | --ini-dir DIR] [--out FILE] [INPUT ...]
  python modpak.py pack SRC_DIR OUT_FILE
  python modpak.py list PAK_FILE

INPUT, in the order the launcher stores them (modPakInputs):
  [players [countdown]] [entry ...] lobby size and countdown of TRAINING lobbies, only for the
                                    shader cache warm-up (the game's own rules otherwise); an
                                    entry is name=value (PS4Engine.ini [SystemSettings]) or
                                    [Section]name=value (DefaultEngine.ini)
  --server=ADDRESS                  anywhere; the machine of the backend (default 127.0.0.1),
                                    host or host:game:matching:webapi:backend:relay

Defaults follow the launcher's data folder (FZH_DATA_DIR, else
%LOCALAPPDATA%\\com.fzh-resurrection.launcher): the pak of the update, else of the base game, in
game\\CUSA11182[-UPDATE]\\firewall\\content\\paks\\firewall-ps4.pak, and the output
game\\CUSA11182-mods\\firewall\\content\\paks\\zz-fzh-mod_P.pak.
"""
import hashlib, os, re, struct, sys, zlib

MAGIC = 0x5A6F12E1
VERSION = 4
DEFAULT_ENGINE = "firewall/Config/DefaultEngine.ini"
PS4_ENGINE = "firewall/Config/PS4/PS4Engine.ini"
BACKEND_PORT = 1338
TITLE_ID = "CUSA11182"
PAKS = os.path.join("firewall", "content", "paks")


def data_dir():
    return os.environ.get("FZH_DATA_DIR") or os.path.join(
        os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "com.fzh-resurrection.launcher")


# ---- pak reading (v1-v4, unencrypted index) ----

def read_index(f):
    f.seek(0, 2)
    size = f.tell()
    f.seek(size - 45)
    tail = f.read(45)
    magic, _, idx_off, idx_size = struct.unpack_from("<IIQQ", tail, 1)
    if magic != MAGIC or tail[0]:
        raise ValueError("not a pak with an unencrypted index")
    f.seek(idx_off)
    idx = f.read(idx_size)
    n = struct.unpack_from("<i", idx, 0)[0]
    o = 4 + n
    count = struct.unpack_from("<i", idx, o)[0]
    o += 4
    entries = []
    for _ in range(count):
        n = struct.unpack_from("<i", idx, o)[0]
        o += 4
        if n < 0:
            name = idx[o:o - 2 * n].decode("utf-16le").rstrip("\0")
            o += -2 * n
        else:
            name = idx[o:o + n].rstrip(b"\0").decode("utf-8", "replace")
            o += n
        off, _, usize, method = struct.unpack_from("<qqqi", idx, o)
        o += 28 + 20
        blocks = []
        if method != 0:
            nb = struct.unpack_from("<i", idx, o)[0]
            o += 4
            blocks = [struct.unpack_from("<qq", idx, o + 16 * i) for i in range(nb)]
            o += nb * 16
        encrypted = idx[o]
        o += 5
        entries.append({"name": name, "off": off, "usize": usize, "method": method,
                        "encrypted": encrypted, "blocks": blocks})
    return entries


def entry_data(f, e):
    if e["encrypted"]:
        raise ValueError("encrypted entry: " + e["name"])
    if e["method"] == 0:
        # In-line header before the data: 8+8+8+4+20 bytes, then 1+4.
        f.seek(e["off"] + 53)
        return f.read(e["usize"])
    out = bytearray()
    for start, end in e["blocks"]:
        f.seek(start)
        out += zlib.decompress(f.read(end - start))
    return bytes(out)


def read_files(pak, names):
    with open(pak, "rb") as f:
        index = {e["name"]: e for e in read_index(f)}
        return [entry_data(f, index[n]) for n in names]


# ---- pak writing ----

def fstring(s):
    b = s.encode("utf-8") + b"\0"
    return struct.pack("<i", len(b)) + b


def entry_record(offset, data, sha1):
    # Offset, Size, UncompressedSize, CompressionMethod, Hash[20], bEncrypted, CompressionBlockSize
    return struct.pack("<qqqi", offset, len(data), len(data), 0) + sha1 + struct.pack("<BI", 0, 0)


def write_pak(files):
    """files: [(path inside the pak, bytes)]; mount point ../../../ as in the game's pak."""
    body = bytearray()
    index_entries = []
    for rel, data in sorted(files):
        sha1 = hashlib.sha1(data).digest()
        offset = len(body)
        body += entry_record(0, data, sha1)
        body += data
        index_entries.append(fstring(rel) + entry_record(offset, data, sha1))
    index = fstring("../../../") + struct.pack("<i", len(files)) + b"".join(index_entries)
    footer = struct.pack("<BIiqq", 0, MAGIC, VERSION, len(body), len(index)) + hashlib.sha1(index).digest()
    return bytes(body) + index + footer


# ---- ini changes ----

def backend_url(server):
    parts = server.split(":")
    host, port = server, BACKEND_PORT
    if len(parts) == 6 and all(p.isdigit() and int(p) > 0 for p in parts[1:]):
        host, port = parts[0], int(parts[4])
    if ":" in host and not host.startswith("["):
        host = "[%s]" % host
    return "http://%s:%d/parse/" % (host, port)


def changes(inputs):
    servers = [a[len("--server="):] for a in inputs if a.startswith("--server=")]
    server = servers[-1] if servers and servers[-1] else "127.0.0.1"
    quoted = '"%s"' % backend_url(server)
    default_engine = [
        # Only the studio's own login ("Zygo") goes off; PSN login through shadNet stays.
        ("WWSZygo", "bEnabled", "false"),
        ("FCBackend", "ApiBaseURL", quoted),
        ("FCBackend", "GatewayURL", quoted),
    ]
    ps4_engine = [
        # Loading a map under emulation can exceed the game's 60 s and drop the client between
        # lobby and match.
        ("/Script/OnlineSubsystemUtils.IpNetDriver", "ConnectionTimeout", "300.0"),
        ("/Script/OnlineSubsystemUtils.IpNetDriver", "InitialConnectTimeout", "300.0"),
    ]
    args = [a for a in inputs if not a.startswith("--server=")]
    lobby = [a for a in args if "=" not in a]
    if lobby:
        default_engine.append(("SystemSettings", "FCDebug.Session.PVEPlayers", lobby[0]))
    if len(lobby) > 1:
        default_engine.append(("SystemSettings", "FCDebug.Session.LobbyLifeTime", lobby[1]))
    for entry in (a for a in args if "=" in a):
        if entry.startswith("["):
            section, _, rest = entry[1:].partition("]")
            name, _, value = rest.partition("=")
            default_engine.append((section, name, value))
        else:
            # PS4Engine.ini is read after DefaultEngine.ini and wins (it already sets r.ShadowQuality=3).
            name, _, value = entry.partition("=")
            ps4_engine.append(("SystemSettings", name, value))
    return default_engine, ps4_engine


def set_ini(text, section, key, value):
    """key=value in [section] (created at the end if missing); commented lines never match."""
    lines = text.split("\n")
    start = next((i for i, l in enumerate(lines) if l.strip() == "[%s]" % section), None)
    if start is None:
        return text.rstrip("\r\n") + "\r\n\r\n[%s]\r\n%s=%s\r\n" % (section, key, value)
    end = next((i for i in range(start + 1, len(lines)) if lines[i].strip().startswith("[")), len(lines))
    cr = "\r" if lines[start].endswith("\r") else ""
    for i in range(start + 1, end):
        if re.match(r"\s*%s\s*=" % re.escape(key), lines[i], re.I):
            lines[i] = "%s=%s%s" % (key, value, cr)
            return "\n".join(lines)
    lines.insert(start + 1, "%s=%s%s" % (key, value, cr))
    return "\n".join(lines)


def build_mod(default_engine, ps4_engine, inputs):
    out = []
    for path, raw, edits in zip((DEFAULT_ENGINE, PS4_ENGINE), (default_engine, ps4_engine), changes(inputs)):
        text = raw.decode("utf-8-sig")
        for section, key, value in edits:
            text = set_ini(text, section, key, value)
        out.append((path, text.encode("utf-8")))
    return write_pak(out)


def default_game_pak():
    game = os.path.join(data_dir(), "game")
    for folder in (TITLE_ID + "-UPDATE", TITLE_ID):
        pak = os.path.join(game, folder, PAKS, "firewall-ps4.pak")
        if os.path.isfile(pak):
            return pak
    return os.path.join(game, TITLE_ID, PAKS, "firewall-ps4.pak")


def main(argv):
    if not argv or argv[0] not in ("mod", "pack", "list"):
        print(__doc__)
        return 2
    command, rest = argv[0], argv[1:]
    if command == "pack":
        src, dst = rest
        files = []
        for root, _, names in os.walk(src):
            for n in names:
                full = os.path.join(root, n)
                with open(full, "rb") as f:
                    files.append((os.path.relpath(full, src).replace("\\", "/"), f.read()))
        with open(dst, "wb") as f:
            f.write(write_pak(files))
        print("wrote %s (%d files)" % (dst, len(files)))
        return 0
    if command == "list":
        with open(rest[0], "rb") as f:
            for e in read_index(f):
                print("%12d  %s" % (e["usize"], e["name"]))
        return 0
    game_pak = ini_dir = None
    out = os.path.join(data_dir(), "game", TITLE_ID + "-mods", PAKS, "zz-fzh-mod_P.pak")
    inputs = []
    i = 0
    while i < len(rest):
        a = rest[i]
        if a in ("--game-pak", "--ini-dir", "--out"):
            value = rest[i + 1]
            i += 2
            if a == "--game-pak":
                game_pak = value
            elif a == "--ini-dir":
                ini_dir = value
            else:
                out = value
            continue
        inputs.append(a)
        i += 1
    if ini_dir:
        sources = []
        for path in (DEFAULT_ENGINE, PS4_ENGINE):
            with open(os.path.join(ini_dir, *path.split("/")), "rb") as f:
                sources.append(f.read())
    else:
        sources = read_files(game_pak or default_game_pak(), [DEFAULT_ENGINE, PS4_ENGINE])
    pak = build_mod(sources[0], sources[1], inputs)
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "wb") as f:
        f.write(pak)
    print("wrote %s (%d bytes) for: %s" % (out, len(pak), " ".join(inputs) or "(no inputs)"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
