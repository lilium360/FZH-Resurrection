"""Client side of shadNet's binary protocol (shadNet documentation/protocol.md): framing, the
string-only protobuf messages, and account creation.

Used by backend.py (registration) and by account/create_account.py (the admin tool).
"""
import socket, struct

HEADER = struct.Struct("<BHIQ")  # type, command, total size, packet id
REQUEST, REPLY = 0, 1
CREATE = 2

NO_ERROR = 0
INVALID_INPUT = 3
CREATION_EXISTING_USERNAME = 11
CREATION_EXISTING_EMAIL = 13
UNAUTHORIZED = 23


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def string_field(number, value):
    data = value.encode("utf-8")
    return varint((number << 3) | 2) + varint(len(data)) + data if data else b""


def proto_blob(*strings):
    """A u32 LE size, then a protobuf message whose fields 1, 2, ... are these strings."""
    msg = b"".join(string_field(i, s) for i, s in enumerate(strings, 1))
    return struct.pack("<I", len(msg)) + msg


def packet(command, payload, packet_id):
    return HEADER.pack(REQUEST, command, HEADER.size + len(payload), packet_id) + payload


def registration(online_id, password, avatar_url, email, key):
    """RegistrationRequest: npid, password, avatar_url, email, secret_key."""
    return proto_blob(online_id, password, avatar_url, email, key)


class Connection:
    """One TCP connection. Requests are numbered from 1; ServerInfo, notifications and replies
    to other requests are skipped while waiting for a reply."""

    def __init__(self, host, port, timeout=10):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.next_id = 1

    def close(self):
        self.sock.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def _receive(self, n):
        buf = b""
        while len(buf) < n:
            part = self.sock.recv(n - len(buf))
            if not part:
                raise ConnectionError("connection closed by the server")
            buf += part
        return buf

    def request(self, command, payload=b""):
        """(error code, rest of the reply) for one request."""
        packet_id = self.next_id
        self.next_id += 1
        self.sock.sendall(packet(command, payload, packet_id))
        while True:
            kind, _, size, pid = HEADER.unpack(self._receive(HEADER.size))
            data = self._receive(size - HEADER.size)
            if kind == REPLY and pid == packet_id:
                if not data:
                    raise ConnectionError("reply without an error byte")
                return data[0], data[1:]


def create_account(host, port, online_id, password, avatar_url, email, key, timeout=10):
    """shadNet's error code for Create (NO_ERROR = account created)."""
    with Connection(host, port, timeout) as c:
        return c.request(CREATE, registration(online_id, password, avatar_url, email, key))[0]
