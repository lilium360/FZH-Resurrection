#!/usr/bin/env python3
"""Progression backend for Firewall Zero Hour: stands in for the game's Parse server and its
studio login service ("Zygo"), serves avatars, and registers shadNet accounts for the launcher.

The game reaches it through the emulator's host_overrides.json (every HTTP host is sent here) and
through the mod pak (FCBackend ApiBaseURL/GatewayURL). The launcher uses /account/v1/ (docs/
launcher-contract.md, "Backend account API"). Every request is appended to the request log, with
passwords, tokens, codes and keys removed.

  python backend.py [--host 127.0.0.1] [--port 1338] [--data FILE] [--log FILE] ...

`python backend.py --help` lists the settings; each has an FZH_* environment variable.
On any address other than loopback only the game's API (/parse/, the login service, avatars) and
/account/v1/ are served. Request bodies are capped at 1 MB.
"""
import argparse, base64, collections, datetime, json, os, re, secrets, struct, threading, time
import urllib.error, urllib.parse, urllib.request, uuid, zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, parse_qsl, urlencode, urlparse

import shadnet_client

AVATAR_PATH = re.compile(r"/a/([A-Za-z0-9_-]{3,16})\.png")
NAME_PATH = re.compile(r"/account/v1/names/([^/]*)")
AVATAR_SIDE = 256
AVATAR_MAX_BYTES = 256 * 1024
MAX_BODY = 1 << 20
ACCOUNT_MAX_BODY = 4096
LOOPBACK = ("127.0.0.1", "localhost", "::1")

ONLINE_ID = re.compile(r"[A-Za-z0-9_-]{3,16}")
# 32 bytes in unpadded base64url: the password the launcher derives from the recovery code.
PASSWORD = re.compile(r"[A-Za-z0-9_-]{43}")
AUTH_CODE = re.compile(r"[A-Za-z0-9]{1,64}")
# Compared with the name in lower case without "_" and "-".
RESERVED_PREFIXES = ("admin", "moderator", "system", "support", "staff", "official", "shadnet",
                     "shadps4", "fzh", "firewall", "zerohour", "resurrection", "firstcontact",
                     "playstation")
RESERVED_NAMES = {"root", "server", "sony", "psn", "mod", "mods", "null", "undefined", "guest",
                  "player", "deleteduser", "everyone", "anonymous", "unknown"}

# shadNet issues an authorization code for any client id the requester names and checks only
# that the redeemer names the same one; this is the id the launcher must request.
OAUTH_CLIENT = "fzh-backend"
OAUTH_REDIRECT = "orbis://games"

PROTECTED_FIELDS = {"objectId", "uniqueNetId", "nickname", "createdAt", "updatedAt", "userPrefs"}
MAX_FIELDS = 256
MAX_FIELD_NAME = 64
MAX_VALUE = 4096
# The largest userPrefs seen in play is about 23 KB of base64.
MAX_PREFS = 256 * 1024
# Highest increment one player's match result may add to another player's stat; a match's
# largest observed value is a few hundred.
MAX_INCREMENT = 100000
MAX_GAME_STATS = 1000

API_LIMIT = (30, 600)
GAME_LOGIN_LIMIT = (20, 600)

SECRET_NAME = re.compile(r"(?i)pass|secret|token|key|auth|^code$")
SECRET_HEADERS = {"cookie", "x-parse-application-id", "x-parse-rest-api-key", "x-parse-session-token"}
INVALID_SESSION = 403, {"code": 209, "error": "invalid session"}

config = argparse.Namespace(data="backend-data.json", log="backend-requests.log", avatars="avatars",
                            auth="backend-auth.json", shadnet_webapi="http://127.0.0.1:31315",
                            shadnet_game=None, registration_key_file=None, game_login="open",
                            session_seconds=12 * 3600, register_per_day=3, register_cooldown=60,
                            register_global_per_hour=30, reserved=set(), proxy_header="",
                            trusted_proxies=set())
lock = threading.Lock()
register_lock = threading.Lock()


def now_iso():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.") + "000Z"


def write_json(path, value):
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        json.dump(value, f, indent=2)
    os.replace(path + ".tmp", path)


def read_json(path, default):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def load():
    return read_json(config.data, {"players": {}, "game_stats": []})


def save(d):
    write_json(config.data, d)


def new_object_id():
    alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    return "".join(secrets.choice(alphabet) for _ in range(10))


def plain_png(seed):
    """A one-colour avatar for a player who has not uploaded one; the colour follows the name."""
    h = zlib.crc32(seed.encode("utf-8"))
    colour = bytes((64 + (h & 0x7F), 64 + ((h >> 8) & 0x7F), 64 + ((h >> 16) & 0x7F)))
    rows = (b"\x00" + colour * AVATAR_SIDE) * AVATAR_SIDE

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", AVATAR_SIDE, AVATAR_SIDE, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


def valid_avatar(data):
    return (len(data) <= AVATAR_MAX_BYTES and data[:8] == b"\x89PNG\r\n\x1a\n" and data[12:16] == b"IHDR"
            and struct.unpack(">II", data[16:24]) == (AVATAR_SIDE, AVATAR_SIDE) and data[-8:-4] == b"IEND")


def token_owner(authorization):
    """The Online ID a shadNet token belongs to, or None."""
    if not authorization.startswith("Bearer "):
        return None
    request = urllib.request.Request(config.shadnet_webapi + "/v1/users/me/profile?fields=user",
                                     headers={"Authorization": authorization})
    try:
        with urllib.request.urlopen(request, timeout=5) as reply:
            return json.load(reply)["user"]["onlineId"]
    except (OSError, ValueError, KeyError, TypeError):
        return None


def redeem(code):
    """(account id, Online ID) behind a shadNet authorization code issued to OAUTH_CLIENT, or None
    when shadNet does not know the code. OSError when shadNet cannot be asked."""
    basic = "Basic " + base64.b64encode((OAUTH_CLIENT + ":").encode()).decode()
    form = urlencode({"grant_type": "authorization_code", "code": code, "redirect_uri": OAUTH_REDIRECT})
    exchange = urllib.request.Request(config.shadnet_webapi + "/2.0/oauth/token", data=form.encode(),
                                      headers={"Authorization": basic,
                                               "Content-Type": "application/x-www-form-urlencoded"})
    try:
        with urllib.request.urlopen(exchange, timeout=5) as reply:
            token = json.load(reply)["access_token"]
        check = urllib.request.Request(
            config.shadnet_webapi + "/2.0/oauth/token/" + urllib.parse.quote(token, safe=""),
            headers={"Authorization": basic})
        with urllib.request.urlopen(check, timeout=5) as reply:
            info = json.load(reply)
    except urllib.error.HTTPError as e:
        if e.code in (400, 401):
            return None
        raise
    except (ValueError, KeyError, TypeError) as e:
        raise OSError("unexpected WebAPI reply: %s" % e)
    if info.get("client_id") != OAUTH_CLIENT or not info.get("online_id"):
        return None
    return str(info["user_id"]), info["online_id"]


def reserved(online_id):
    name = online_id.lower().replace("_", "").replace("-", "")
    return name in RESERVED_NAMES or name in config.reserved or name.startswith(RESERVED_PREFIXES)


def redact(value):
    if isinstance(value, dict):
        return {k: "<redacted>" if isinstance(v, str) and SECRET_NAME.search(k) else redact(v)
                for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    return value


def shown_path(path):
    base, _, query = path.partition("?")
    if not query:
        return path
    return base + "?" + urlencode([(k, "<redacted>" if SECRET_NAME.search(k) else v)
                                   for k, v in parse_qsl(query, keep_blank_values=True)], safe="<>{}\":,")


def shown_body(path, raw, limit):
    if not raw:
        return ""
    if path.startswith("/account/"):
        return "<%d bytes, not logged>" % len(raw)
    if raw[:4] == b"\x89PNG":
        return "<png, %d bytes>" % len(raw)
    try:
        return json.dumps(redact(json.loads(raw)))[:limit]
    except ValueError:
        return raw.decode("utf-8", "replace")[:limit]


def write_log(line):
    with lock:
        with open(config.log, "a", encoding="utf-8") as f:
            f.write(line + "\n")


def number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def apply_ops(player, changes, increments_only=False):
    """Parse field operations. A match host also reports other players' results, so those
    (increments_only) may only add bounded amounts."""
    for k, v in changes.items():
        if k in PROTECTED_FIELDS or len(k) > MAX_FIELD_NAME or (k not in player and len(player) >= MAX_FIELDS):
            continue
        if isinstance(v, dict) and v.get("__op") == "Increment":
            amount = v.get("amount", 0)
            if not number(amount) or (increments_only and not 0 <= amount <= MAX_INCREMENT):
                continue
            current = player.get(k, 0)
            player[k] = (current if number(current) else 0) + amount
            continue
        if increments_only:
            continue
        value = v.get("amount") if isinstance(v, dict) and v.get("__op") == "Set" else v
        if len(json.dumps(value)) <= MAX_VALUE:
            player[k] = value


class Limiter:
    """Sliding-window counters per key, in memory: a restart clears them."""

    def __init__(self):
        self.hits = collections.defaultdict(collections.deque)
        self.lock = threading.Lock()

    def retry_after(self, key, limit, window):
        """Seconds until one more event under key fits limit events per window; 0 = now."""
        now = time.monotonic()
        with self.lock:
            q = self.hits[key]
            while q and q[0] <= now - window:
                q.popleft()
            return 0 if len(q) < limit else int(q[0] + window - now) + 1

    def record(self, *keys):
        now = time.monotonic()
        with self.lock:
            for key in keys:
                self.hits[key].append(now)
            if len(self.hits) > 10000:
                for key in [k for k, q in self.hits.items() if not q or q[-1] < now - 86400]:
                    del self.hits[key]

    def take(self, key, limit, window):
        wait = self.retry_after(key, limit, window)
        if not wait:
            self.record(key)
        return wait


class AuthStore:
    """backend-auth.json: accounts registered here, play grants and game sessions. Grants and
    sessions hold the client address and are dropped when they expire."""

    def __init__(self, path):
        self.path = path
        self.lock = threading.Lock()
        d = read_json(path, {})
        self.accounts = d.get("accounts", {})
        self.grants = d.get("grants", {})
        self.sessions = d.get("sessions", {})

    def _save(self):
        now = time.time()
        for table in (self.grants, self.sessions):
            for key in [k for k, v in table.items() if v["expires"] <= now]:
                del table[key]
        write_json(self.path, {"accounts": self.accounts, "grants": self.grants, "sessions": self.sessions})

    def account(self, online_id):
        with self.lock:
            return self.accounts.get(online_id.lower())

    def add_account(self, online_id):
        with self.lock:
            self.accounts[online_id.lower()] = {"onlineId": online_id, "createdAt": now_iso()}
            self._save()

    def grant(self, account_id, online_id, ip):
        with self.lock:
            self.grants["%s|%s" % (account_id, ip)] = {"onlineId": online_id,
                                                        "expires": time.time() + config.session_seconds}
            self._save()

    def granted(self, account_id, ip):
        """The Online ID of an unexpired grant for this account from this address, or None."""
        with self.lock:
            g = self.grants.get("%s|%s" % (account_id, ip))
            return g["onlineId"] if g and g["expires"] > time.time() else None

    def open_session(self, player_stats_id, account_id, ip):
        with self.lock:
            self.sessions[player_stats_id] = {"accountId": account_id, "ip": ip,
                                              "expires": time.time() + config.session_seconds}
            self._save()

    def session(self, player_stats_id, ip=None):
        """Whether the player has an unexpired session, from this address when ip is given."""
        with self.lock:
            s = self.sessions.get(player_stats_id)
            return bool(s and s["expires"] > time.time() and (ip is None or s["ip"] == ip))

    def any_session_from(self, ip):
        now = time.time()
        with self.lock:
            return any(s["ip"] == ip and s["expires"] > now for s in self.sessions.values())


auth = limiter = None


def fail(status, code, message, **extra):
    return status, dict(error=code, message=message, **extra)


def json_object(body):
    try:
        value = json.loads(body)
    except ValueError:
        return None
    return value if isinstance(value, dict) else None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    remote = False

    def log_message(self, fmt, *args):
        pass

    def client_ip(self):
        peer = self.client_address[0]
        if peer.startswith("::ffff:") and "." in peer:
            peer = peer[7:]
        forwarded = self.headers.get(config.proxy_header, "") if config.proxy_header else ""
        if forwarded and peer in config.trusted_proxies:
            # A proxy appends the address it received the request from: the last entry is the
            # only one the proxy vouches for.
            return forwarded.split(",")[-1].strip() or peer
        return peer

    def _reply(self, status, payload):
        image = isinstance(payload, bytes)
        data = payload if image else json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "image/png" if image else "application/json; charset=utf-8")
        if image:
            self.send_header("Cache-Control", "max-age=60")
            if self.placeholder:
                self.send_header("X-Avatar-Placeholder", "1")
        for name, value in self.extra_headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
        return data

    def _handle(self, method):
        host = self.headers.get("Host", "")
        self.ip = self.client_ip()
        self.placeholder = False
        self.extra_headers = []
        note = ""
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = -1
        if not 0 <= n <= MAX_BODY:
            self.close_connection = True
            body = b""
            status, answer = 413, {"code": 1, "error": "request body too large"}
        else:
            body = self.rfile.read(n) if n else b""
            try:
                status, answer = self.route(method, host, body)
            except Exception as e:
                status, answer = 500, {"code": 1, "error": "internal error"}
                note = "\n  error: %s: %s" % (type(e).__name__, e)
        sent = self._reply(status, answer)
        headers = {k: "<redacted>" if k.lower() in SECRET_HEADERS or SECRET_NAME.search(k) else v
                   for k, v in self.headers.items()}
        write_log("%s %s %s%s\n  headers: %s\n  body: %s\n  -> %d %s%s\n" % (
            time.strftime("%H:%M:%S"), method, host, shown_path(self.path), json.dumps(headers),
            shown_body(self.path, body, 4000), status, shown_body("", sent, 2000), note))

    do_GET = lambda self: self._handle("GET")
    do_POST = lambda self: self._handle("POST")
    do_PUT = lambda self: self._handle("PUT")
    do_DELETE = lambda self: self._handle("DELETE")

    def route(self, method, host, body):
        url = urlparse(self.path)
        path = url.path
        query = parse_qs(url.query)

        avatar = AVATAR_PATH.fullmatch(path)
        if avatar and method == "GET":
            return self.avatar(avatar.group(1))
        if avatar and method == "PUT":
            return self.store_avatar(avatar.group(1), body)
        if method == "GET" and path.startswith("/shad_net/assets/avatar/"):
            return 200, plain_png("")
        if path.startswith("/account/"):
            return self.account(method, path, body)
        if self.remote and not (path.startswith("/parse/") or path in ("", "/")):
            return 404, {"code": 141, "error": "not implemented"}
        if "/parse/" in path:
            try:
                data = json.loads(body) if body else {}
            except ValueError:
                data = {}
            return self.parse(method, path.split("/parse/", 1)[1], query, data)
        # The studio login service; the game posts with no body.
        if path in ("", "/") and "provider" in query:
            return self.service_login(query)
        if method == "GET" and path in ("", "/") and not query:
            return self.discovery()
        return 404, {"code": 141, "error": "not implemented: %s %s%s" % (method, host, url.path)}

    def avatar(self, online_id):
        try:
            with open(os.path.join(config.avatars, online_id.lower() + ".png"), "rb") as f:
                return 200, f.read()
        except OSError:
            # The game retries a missing avatar every two seconds, so it always gets a picture;
            # the header lets a client that can draw initials tell it from an uploaded one.
            self.placeholder = True
            return 200, plain_png(online_id.lower())

    def store_avatar(self, online_id, data):
        owner = token_owner(self.headers.get("Authorization", ""))
        if owner is None or owner.lower() != online_id.lower():
            return 403, {"code": 119, "error": "not the owner of this avatar"}
        if not valid_avatar(data):
            return 400, {"code": 142, "error": "expected a %dx%d PNG of at most %d bytes"
                                                % (AVATAR_SIDE, AVATAR_SIDE, AVATAR_MAX_BYTES)}
        os.makedirs(config.avatars, exist_ok=True)
        path = os.path.join(config.avatars, online_id.lower() + ".png")
        with open(path + ".tmp", "wb") as f:
            f.write(data)
        os.replace(path + ".tmp", path)
        return 200, {"result": {"success": True}}

    # Account API for the launcher (docs/launcher-contract.md, "Backend account API").

    def account(self, method, path, body):
        self.extra_headers.append(("Cache-Control", "no-store"))
        wait = limiter.take("api|" + self.ip, *API_LIMIT)
        if wait:
            return self.too_many(wait)
        if len(body) > ACCOUNT_MAX_BODY:
            return fail(413, "too_large", "The request body is larger than %d bytes." % ACCOUNT_MAX_BODY)
        name = NAME_PATH.fullmatch(path)
        routes = {"/account/v1/register": ("POST", lambda: self.register(body)),
                  "/account/v1/play": ("POST", lambda: self.play(body))}
        if name:
            routes[path] = ("GET", lambda: self.lookup(urllib.parse.unquote(name.group(1))))
        if path not in routes:
            return fail(404, "not_found", "No such endpoint.")
        allowed, handler = routes[path]
        if method != allowed:
            self.extra_headers.append(("Allow", allowed))
            return fail(405, "method_not_allowed", "Use %s." % allowed)
        return handler()

    def too_many(self, wait):
        self.extra_headers.append(("Retry-After", str(wait)))
        return fail(429, "rate_limited", "Too many requests from this address. Try again in %d seconds." % wait,
                    retryAfter=wait)

    def registration_wait(self):
        day, cooldown, hour = "reg-day|" + self.ip, "reg-cool|" + self.ip, "reg-all"
        return max(limiter.retry_after(day, config.register_per_day, 86400),
                   limiter.retry_after(cooldown, 1, config.register_cooldown) if config.register_cooldown else 0,
                   limiter.retry_after(hour, config.register_global_per_hour, 3600)), (day, cooldown, hour)

    def register(self, body):
        if not config.shadnet_game:
            return fail(503, "registration_closed", "This server does not register accounts.")
        data = json_object(body)
        online_id = data.get("onlineId") if data else None
        password = data.get("password") if data else None
        if not isinstance(online_id, str) or not isinstance(password, str):
            return fail(400, "invalid_request", "Send a JSON object with onlineId and password.")
        if not ONLINE_ID.fullmatch(online_id):
            return fail(400, "invalid_name", "An Online ID has 3-16 letters, digits, _ or -.")
        if reserved(online_id):
            return fail(403, "reserved_name", "That Online ID is reserved.")
        if not PASSWORD.fullmatch(password):
            return fail(400, "invalid_request", "The password must be 43 characters of A-Z, a-z, 0-9, _ and -.")
        with register_lock:
            wait, keys = self.registration_wait()
            if wait:
                return self.too_many(wait)
            if auth.account(online_id):
                return fail(409, "name_taken", "That Online ID is taken.")
            try:
                key = ""
                if config.registration_key_file:
                    with open(config.registration_key_file, encoding="utf-8") as f:
                        key = f.read().strip()
            except OSError:
                write_log("=== registration: the registration key file cannot be read")
                return fail(503, "registration_closed", "Registration is not available on this server now.")
            host, port = config.shadnet_game
            try:
                code = shadnet_client.create_account(host, port, online_id, password,
                                                     "http://fzh.avatar/a/%s.png" % online_id,
                                                     "%s@users.invalid" % online_id.lower(), key)
            except OSError:
                return fail(502, "shadnet_unavailable", "The account server did not answer.")
            if code in (shadnet_client.CREATION_EXISTING_USERNAME, shadnet_client.CREATION_EXISTING_EMAIL):
                return fail(409, "name_taken", "That Online ID is taken.")
            if code == shadnet_client.INVALID_INPUT:
                return fail(400, "invalid_name", "The account server refused that Online ID.")
            if code == shadnet_client.UNAUTHORIZED:
                write_log("=== registration: shadNet refused the registration key")
                return fail(503, "registration_closed", "Registration is not available on this server now.")
            if code != shadnet_client.NO_ERROR:
                return fail(502, "shadnet_unavailable", "The account server answered with error %d." % code)
            auth.add_account(online_id)
            limiter.record(*keys)
        return 201, {"onlineId": online_id}

    def lookup(self, online_id):
        if not ONLINE_ID.fullmatch(online_id):
            return fail(400, "invalid_name", "An Online ID has 3-16 letters, digits, _ or -.")
        record = auth.account(online_id)
        if record is None:
            return fail(404, "not_found", "No account with that Online ID was registered here.")
        return 200, {"onlineId": record["onlineId"]}

    def play(self, body):
        data = json_object(body)
        code = data.get("code") if data else None
        if not isinstance(code, str) or not AUTH_CODE.fullmatch(code):
            return fail(400, "invalid_request", "Send a JSON object with the authorization code as code.")
        try:
            owner = redeem(code)
        except OSError:
            return fail(502, "shadnet_unavailable", "The account server did not answer.")
        if owner is None:
            return fail(401, "invalid_code", "The authorization code is unknown, used or expired.")
        account_id, online_id = owner
        auth.grant(account_id, online_id, self.ip)
        return 200, {"accountId": account_id, "onlineId": online_id, "address": self.ip,
                     "expiresIn": config.session_seconds}

    # Any host works here: host_overrides.json sends every host to this server.
    AUTH = "https://auth.firewall.casa"
    GATEWAY = "https://gateway.firewall.casa/"

    def discovery(self):
        """The environment list; the game picks the one named in its configuration (EnvAutoLogin)."""
        environment = {"auth": self.AUTH, "psnEnvironment": "np"}
        return 200, {"environments": {name: dict(environment) for name in ("dev", "prod-qa", "prod")}}

    def service_login(self, query):
        """Login with the PSN code. The mod pak turns this login off, so nothing reads what it
        answers and every code is accepted."""
        provider = query.get("provider", [""])[0]
        code = query.get("code", [""])[0]
        # A stable numeric id: the game may parse it as an integer.
        uid = str(100000 + uuid.uuid5(uuid.NAMESPACE_OID, code or provider).int % 900000)
        now = now_iso()
        return 200, {
            "user": {"id": uid, "userId": uid, "name": "player" + uid, "provider": provider},
            "session": {"id": uid, "token": secrets.token_urlsafe(24), "accessToken": secrets.token_urlsafe(24),
                        "sessionToken": secrets.token_urlsafe(24), "expires": "2099-01-01T00:00:00.000Z"},
            "properties": {"firstLogin": now, "lastLogin": now, "Preorder": False},
            "gateway": self.GATEWAY,
            "firstLogin": now, "lastLogin": now, "Preorder": False,
        }

    def parse(self, method, resource, query, body):
        with lock:
            d = load()
            try:
                return self._parse(d, method, resource, query, body)
            finally:
                save(d)

    def own_player(self, d, pid):
        """The player whose session this address holds, or None."""
        if not isinstance(pid, str) or not auth.session(pid, self.ip):
            return None
        return next((v for v in d["players"].values() if v["objectId"] == pid), None)

    def game_login(self, d, body):
        wait = limiter.take("login|" + self.ip, *GAME_LOGIN_LIMIT)
        if wait:
            self.extra_headers.append(("Retry-After", str(wait)))
            return 429, {"code": 155, "error": "too many logins from this address"}
        uid = body.get("uniqueNetId")
        if not isinstance(uid, str) or not 0 < len(uid) <= 64:
            return 400, {"code": 142, "error": "uniqueNetId missing"}
        nickname = body.get("nickname") if isinstance(body.get("nickname"), str) else uid
        if config.game_login == "grant":
            # The game sends nothing verifiable: the launcher vouched for this account from
            # this address with POST /account/v1/play.
            nickname = auth.granted(uid, self.ip)
            if nickname is None:
                return 403, {"code": 209, "error": "no play grant for this account from this address"}
        p = d["players"].get(uid)
        if p is None:
            p = {"objectId": new_object_id(), "uniqueNetId": uid, "nickname": nickname[:64], "createdAt": now_iso()}
            d["players"][uid] = p
        if config.game_login == "grant":
            p["nickname"] = nickname
        p["updatedAt"] = now_iso()
        auth.open_session(p["objectId"], uid, self.ip)
        return 200, {"result": {"playerStatsId": p["objectId"], "objectId": p["objectId"],
                                "sessionToken": "r:" + secrets.token_hex(16), "properties": {}}}

    def _parse(self, d, method, resource, query, body):
        fields = body if isinstance(body, dict) else {}
        if resource.startswith("functions/login"):
            return self.game_login(d, fields)
        if resource.startswith("functions/getUserPrefs") or resource.startswith("functions/setUserPrefs"):
            p = self.own_player(d, fields.get("playerStatsId"))
            if p is None:
                return INVALID_SESSION
            if resource.startswith("functions/setUserPrefs"):
                prefs = fields.get("data", "")
                if not isinstance(prefs, str) or len(prefs) > MAX_PREFS:
                    return 400, {"code": 142, "error": "data must be a string of at most %d bytes" % MAX_PREFS}
                p["userPrefs"] = prefs
                p["updatedAt"] = now_iso()
                return 200, {"result": {"success": True, "updatedAt": p["updatedAt"]}}
            # The game reads "result" as the base64 string itself; an object makes it fall back to defaults.
            return 200, {"result": p.get("userPrefs") or ""}
        if resource.startswith("functions/getTuningParams"):
            return 200, {"result": {"misc": {}}}
        if resource.startswith(("functions/getProductCatalog", "functions/getReceiptsByCategory", "functions/missions")):
            return 200, {"result": []}
        if resource.startswith("classes/PlayerStats"):
            pid = resource.split("/")[2] if resource.count("/") >= 2 else None
            if pid is None:
                try:
                    where = json.loads(query.get("where", ["{}"])[0])
                except ValueError:
                    where = {}
                pid = (where.get("objectId") if isinstance(where, dict) else None) or query.get("objectId", [None])[0]
            p = self.own_player(d, pid)
            if p is None:
                return INVALID_SESSION
            if method == "PUT":
                apply_ops(p, fields)
                p["updatedAt"] = now_iso()
                return 200, {"updatedAt": p["updatedAt"]}
            return 200, {"results": [p], "count": 1}
        if resource.startswith("classes/GameStats"):
            if method != "POST":
                return 200, {"results": [], "count": 0}
            if not auth.any_session_from(self.ip):
                return INVALID_SESSION
            if isinstance(body, list):
                # [{<playerStatsId>: {<stat>: {"__op": "Increment", "amount": n}}}], one entry per
                # player of the match, sent by the match host.
                by_id = {v["objectId"]: v for v in d["players"].values()}
                for entry in body:
                    if not isinstance(entry, dict):
                        continue
                    for pid, stats in entry.items():
                        p = by_id.get(pid)
                        if p is None or not isinstance(stats, dict) or not auth.session(pid):
                            continue
                        apply_ops(p, stats, increments_only=not auth.session(pid, self.ip))
                        p["updatedAt"] = now_iso()
                return 201, {"objectId": new_object_id(), "createdAt": now_iso()}
            if len(json.dumps(fields)) > MAX_VALUE:
                return 400, {"code": 142, "error": "entry larger than %d bytes" % MAX_VALUE}
            oid = new_object_id()
            d["game_stats"] = d["game_stats"][-(MAX_GAME_STATS - 1):] + [dict(fields, objectId=oid, createdAt=now_iso())]
            return 201, {"objectId": oid, "createdAt": now_iso()}
        return 404, {"code": 141, "error": "function not implemented: " + resource}


def build(argv=None):
    """Parses the settings and returns the server, not yet serving."""
    global auth, limiter

    def env(name, default):
        return os.environ.get(name) or default

    p = argparse.ArgumentParser(description="Firewall Zero Hour progression backend")
    p.add_argument("--host", default="127.0.0.1", help="address to listen on (default 127.0.0.1)")
    p.add_argument("--port", type=int, default=1338, help="TCP port (default 1338)")
    p.add_argument("--data", default=env("FZH_BACKEND_DATA", "backend-data.json"),
                   help="progression data file; avatars/ and backend-auth.json go next to it")
    p.add_argument("--log", default=env("FZH_BACKEND_LOG", "backend-requests.log"), help="request log")
    p.add_argument("--shadnet-webapi", default=env("FZH_SHADNET_WEBAPI", "http://127.0.0.1:31315"),
                   help="shadNet WebAPI: avatar upload tokens and play authorization codes")
    p.add_argument("--shadnet-game", default=env("FZH_SHADNET_GAME", ""),
                   help="host:port of shadNet's binary protocol; registration is off without it")
    p.add_argument("--registration-key-file", default=env("FZH_REGISTRATION_KEY_FILE", ""),
                   help="file holding shadNet's RegistrationSecretKey, read at each registration")
    p.add_argument("--game-login", choices=("grant", "open"), default=env("FZH_GAME_LOGIN", ""),
                   help="grant: the game logs in only for an account the launcher vouched for from the "
                        "same address; open: any login (default: open on loopback, grant otherwise)")
    p.add_argument("--session-hours", type=float, default=env("FZH_SESSION_HOURS", "12"),
                   help="lifetime of play grants and game sessions (default 12)")
    p.add_argument("--register-per-day", type=int, default=env("FZH_REGISTER_PER_DAY", "3"),
                   help="accounts one address may register per 24 hours (default 3)")
    p.add_argument("--register-cooldown", type=int, default=env("FZH_REGISTER_COOLDOWN", "60"),
                   help="seconds between two registrations from one address (default 60)")
    p.add_argument("--register-global-per-hour", type=int, default=env("FZH_REGISTER_GLOBAL_PER_HOUR", "30"),
                   help="accounts registered per hour from all addresses together (default 30)")
    p.add_argument("--reserved-names", default=env("FZH_RESERVED_NAMES", ""),
                   help="comma-separated Online IDs refused in addition to the built-in list")
    p.add_argument("--trusted-proxy-header", default=env("FZH_TRUSTED_PROXY_HEADER", ""),
                   help="header carrying the client address set by a reverse proxy, e.g. X-Forwarded-For "
                        "(default: none, the TCP peer is the client)")
    p.add_argument("--trusted-proxies", default=env("FZH_TRUSTED_PROXIES", "127.0.0.1,::1"),
                   help="addresses whose --trusted-proxy-header is believed (default 127.0.0.1,::1)")
    a = p.parse_args(argv)
    if a.game_login not in ("", "grant", "open"):
        p.error("FZH_GAME_LOGIN must be grant or open")
    config.data = os.path.abspath(a.data)
    config.log = os.path.abspath(a.log)
    config.avatars = os.path.join(os.path.dirname(config.data), "avatars")
    config.auth = os.path.join(os.path.dirname(config.data), "backend-auth.json")
    config.shadnet_webapi = a.shadnet_webapi.rstrip("/")
    config.shadnet_game = None
    if a.shadnet_game:
        host, _, port = a.shadnet_game.rpartition(":")
        config.shadnet_game = (host.strip("[]"), int(port))
    config.registration_key_file = a.registration_key_file or None
    config.game_login = a.game_login or ("open" if a.host in LOOPBACK else "grant")
    config.session_seconds = int(a.session_hours * 3600)
    config.register_per_day = a.register_per_day
    config.register_cooldown = a.register_cooldown
    config.register_global_per_hour = a.register_global_per_hour
    config.reserved = {n.strip().lower().replace("_", "").replace("-", "") for n in a.reserved_names.split(",") if n.strip()}
    config.proxy_header = a.trusted_proxy_header
    config.trusted_proxies = {x.strip() for x in a.trusted_proxies.split(",") if x.strip()}
    Handler.remote = a.host not in LOOPBACK
    for path in (config.data, config.log):
        os.makedirs(os.path.dirname(path), exist_ok=True)
    auth = AuthStore(config.auth)
    limiter = Limiter()
    started = "=== backend started %s on %s:%d, registration %s, game login %s ===" % (
        time.strftime("%Y-%m-%d %H:%M:%S"), a.host, a.port, "on" if config.shadnet_game else "off",
        config.game_login)
    write_log(started)
    print(started, flush=True)
    return ThreadingHTTPServer((a.host, a.port), Handler)


def main(argv=None):
    build(argv).serve_forever()


if __name__ == "__main__":
    main()
