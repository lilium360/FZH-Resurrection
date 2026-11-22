"""Test doubles for shadNet (binary protocol and WebAPI), the backend under test, and the
launcher's side of the account scheme (recovery code and password derivation)."""
import base64, hashlib, hmac, http.client, json, os, secrets, shutil, socket, socketserver, struct
import tempfile, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import backend
import shadnet_client

CODE_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def new_recovery_code():
    raw = "".join(secrets.choice(CODE_ALPHABET) for _ in range(25))
    return "-".join(raw[i:i + 5] for i in range(0, 25, 5))


def canonical_code(text):
    code = text.upper().replace("-", "").replace(" ", "").replace("O", "0").replace("I", "1").replace("L", "1")
    if len(code) != 25 or any(c not in CODE_ALPHABET for c in code):
        raise ValueError("not a recovery code")
    return code


def derive_password(code, online_id):
    digest = hmac.new(canonical_code(code).encode("ascii"), b"fzh-account-v1\n" + online_id.lower().encode("ascii"),
                      hashlib.sha256).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")


def proto_fields(data):
    """Field number -> bytes (length-delimited) or int (varint) of one protobuf message."""
    out, pos = {}, 0

    def read_varint():
        nonlocal pos
        shift = value = 0
        while True:
            b = data[pos]
            pos += 1
            value |= (b & 0x7F) << shift
            shift += 7
            if not b & 0x80:
                return value

    while pos < len(data):
        key = read_varint()
        if key & 7 == 0:
            out[key >> 3] = read_varint()
        else:
            n = read_varint()
            out[key >> 3] = data[pos:pos + n]
            pos += n
    return out


def blob_fields(blob):
    size = struct.unpack_from("<I", blob)[0]
    return proto_fields(blob[4:4 + size])


class FakeShadnet:
    """shadNet's Create over the binary protocol: case-insensitive names, unique email, the
    registration key."""

    def __init__(self, key):
        self.key = key
        self.accounts = {}
        fake = self

        class Session(socketserver.BaseRequestHandler):
            def handle(self):
                s = self.request
                s.sendall(struct.pack("<BHIQI", 3, 0, 19, 0, 1))
                while True:
                    head = s.recv(shadnet_client.HEADER.size, socket.MSG_WAITALL)
                    if len(head) < shadnet_client.HEADER.size:
                        return
                    _, command, size, pid = shadnet_client.HEADER.unpack(head)
                    payload = s.recv(size - len(head), socket.MSG_WAITALL) if size > len(head) else b""
                    error = fake.create(blob_fields(payload)) if command == shadnet_client.CREATE else 33
                    s.sendall(struct.pack("<BHIQB", 1, command, 16, pid, error))

        self.server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Session)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.port = self.server.server_address[1]

    def create(self, f):
        npid, password, avatar, email, key = (f.get(i, b"").decode() for i in range(1, 6))
        if key != self.key:
            return shadnet_client.UNAUTHORIZED
        if not npid or not password or not email:
            return shadnet_client.INVALID_INPUT
        if npid.lower() in self.accounts:
            return shadnet_client.CREATION_EXISTING_USERNAME
        if any(a["email"].lower() == email.lower() for a in self.accounts.values()):
            return shadnet_client.CREATION_EXISTING_EMAIL
        self.accounts[npid.lower()] = {"npid": npid, "password": password, "avatar": avatar, "email": email,
                                       "userId": len(self.accounts) + 1}
        return shadnet_client.NO_ERROR

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class FakeWebApi:
    """The WebAPI routes the backend calls: the authorization code exchange and token lookup
    (OAuth), and the profile behind a bearer token (avatar upload)."""

    def __init__(self):
        self.codes = {}       # code -> (user id, online id, client id)
        self.tokens = {}      # bearer token -> online id
        self.access = {}
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def send(self, status, body):
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def client(self):
                value = self.headers.get("Authorization", "")
                if not value.startswith("Basic "):
                    return None
                return base64.b64decode(value[6:]).decode().partition(":")[0]

            def do_POST(self):
                form = dict(urllib.parse.parse_qsl(self.rfile.read(int(self.headers["Content-Length"])).decode()))
                grant = fake.codes.pop(form.get("code"), None)
                if (self.path != "/2.0/oauth/token" or form.get("grant_type") != "authorization_code"
                        or form.get("redirect_uri") != "orbis://games" or grant is None
                        or grant[2] != self.client()):
                    return self.send(400, {"error": "invalid_grant"})
                token = secrets.token_hex(16)
                fake.access[token] = grant
                self.send(200, {"access_token": token, "token_type": "bearer", "expires_in": 3599})

            def do_GET(self):
                if self.path.startswith("/2.0/oauth/token/"):
                    grant = fake.access.get(urllib.parse.unquote(self.path.rsplit("/", 1)[1]))
                    if grant is None or grant[2] != self.client():
                        return self.send(400, {"error": "invalid_token"})
                    return self.send(200, {"user_id": str(grant[0]), "online_id": grant[1], "client_id": grant[2]})
                if self.path == "/v1/users/me/profile?fields=user":
                    owner = fake.tokens.get(self.headers.get("Authorization", "")[7:])
                    if owner is None:
                        return self.send(401, {"error": {"code": 1}})
                    return self.send(200, {"user": {"accountId": "1", "onlineId": owner}})
                self.send(404, {})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = "http://127.0.0.1:%d" % self.server.server_address[1]

    def issue(self, user_id, online_id, client_id=backend.OAUTH_CLIENT):
        code = secrets.token_hex(3)
        self.codes[code] = (user_id, online_id, client_id)
        return code

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class Backend:
    """backend.py on a free loopback port in a temporary folder. Requests carry X-Test-IP, which
    the backend believes as the client address (trusted proxy header)."""

    KEY = "test-registration-key-" + "x" * 18

    def __init__(self, *flags, shadnet=None, webapi=None):
        self.dir = tempfile.mkdtemp(prefix="fzh-backend-test-")
        self.key_file = os.path.join(self.dir, "registration-key.txt")
        with open(self.key_file, "w") as f:
            f.write(self.KEY + "\n")
        argv = ["--host", "127.0.0.1", "--port", "0", "--data", os.path.join(self.dir, "backend-data.json"),
                "--log", self.log_path, "--trusted-proxy-header", "X-Test-IP",
                "--registration-key-file", self.key_file]
        if shadnet:
            argv += ["--shadnet-game", "127.0.0.1:%d" % shadnet.port]
        if webapi:
            argv += ["--shadnet-webapi", webapi.url]
        self.argv = argv + list(flags)
        self.start()

    def start(self):
        self.server = backend.build(self.argv)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.port = self.server.server_address[1]

    def restart(self):
        self.server.shutdown()
        self.server.server_close()
        self.start()

    @property
    def log_path(self):
        return os.path.join(self.dir, "backend-requests.log")

    def log(self):
        with open(self.log_path, encoding="utf-8") as f:
            return f.read()

    def call(self, method, path, body=None, ip="198.51.100.1", headers=None):
        """(status, decoded JSON or raw bytes, response headers)."""
        data = body if isinstance(body, (bytes, type(None))) else json.dumps(body).encode()
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=15)
        c.request(method, path, body=data, headers=dict({"X-Test-IP": ip}, **(headers or {})))
        r = c.getresponse()
        raw = r.read()
        c.close()
        try:
            return r.status, json.loads(raw), r.headers
        except ValueError:
            return r.status, raw, r.headers

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        shutil.rmtree(self.dir, ignore_errors=True)
