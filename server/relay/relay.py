#!/usr/bin/env python3
"""Fallback UDP relay for the player-to-player (P2P) traffic of Firewall Zero Hour.

Used when two emulators cannot reach each other directly (symmetric NAT, no UPnP): both reach the
relay outbound and the relay forwards one's packets to the other. It runs next to shadNet and is
the only service that needs a UDP port of its own (31316).

  python relay.py [--host 0.0.0.0] [--port 31316] [--status-interval 10]

Each emulator registers from its P2P socket, at intervals, with its "virtual address": the
ip:port shadNet knows it by, which is what the other players are told. The relay maps
virtual address -> real address the registration came from.

Packet format (network byte order, IPv4 only; address = 4-byte ip + 2-byte port):
  all          "FZR1" + type (1 byte)
  1 REGISTER   n (1 byte) + n virtual addresses              emulator -> relay
  2 CONFIRM    n accepted (1 byte) + real address seen       relay -> emulator
  3 SEND       destination + sender + data                   emulator -> relay
  4 RECEIVE    sender + data                                 relay -> emulator

Rules: only registered senders reach registered destinations (never an address that asked for
nothing: not an open reflector); registrations expire after 30 s; oversized packets are dropped;
a virtual address must have the IP the registration comes from (--no-ip-check lifts this).
The log only holds counts, never payloads.
"""
import argparse, socket, struct, sys, time

MAGIC = b"FZR1"
REGISTER, CONFIRM, SEND, RECEIVE = 1, 2, 3, 4
EXPIRY = 30.0
MAX_ALIASES = 4
MAX_REGISTERED = 256


def text(address):
    return "%s:%d" % address


def read_address(data, at):
    return (socket.inet_ntoa(data[at:at + 4]), struct.unpack_from("!H", data, at + 4)[0])


def write_address(address):
    return socket.inet_aton(address[0]) + struct.pack("!H", address[1])


class Relay:
    def __init__(self, ip_check=True, max_packet=2048, write=print):
        self.ip_check = ip_check
        self.max_packet = max_packet
        self.write = write
        self.virtual = {}     # virtual address -> (real address, expiry)
        self.real = {}        # real address -> [virtual addresses], the first is the main one
        self.names = {}       # (sender's real, destination's real) -> virtual address used
        self.forwarded = {}   # (sender virtual, destination virtual) -> [packets, bytes]
        self.dropped = {}     # reason -> packets
        self.registrations = 0

    def drop(self, reason):
        self.dropped[reason] = self.dropped.get(reason, 0) + 1

    def expire(self, now):
        for virtual in [v for v, (_, until) in self.virtual.items() if until < now]:
            real, _ = self.virtual.pop(virtual)
            aliases = self.real.get(real, [])
            if virtual in aliases:
                aliases.remove(virtual)
            if not aliases:
                self.real.pop(real, None)
                for key in [k for k in self.names if real in k]:
                    del self.names[key]
            self.write("expired %s (was %s)" % (text(virtual), text(real)))

    def register(self, data, real, now):
        """Returns the reply for the registering emulator."""
        if len(data) < 6:
            self.drop("short registration")
            return None
        n = min(data[5], MAX_ALIASES)
        if len(data) < 6 + 6 * n:
            self.drop("short registration")
            return None
        accepted = []
        for i in range(n):
            virtual = read_address(data, 6 + 6 * i)
            if virtual[1] == 0 or virtual in accepted:
                continue
            if self.ip_check and virtual[0] != real[0]:
                self.drop("registration from another IP")
                continue
            current = self.virtual.get(virtual)
            if current is None and len(self.virtual) >= MAX_REGISTERED:
                self.drop("table full")
                continue
            if current is None or current[0] != real:
                self.write("registered %s -> %s" % (text(virtual), text(real)))
                if current is not None and virtual in self.real.get(current[0], []):
                    self.real[current[0]].remove(virtual)
            self.virtual[virtual] = (real, now + EXPIRY)
            accepted.append(virtual)
        if accepted:
            # Order matters: the first address is the main one.
            self.real[real] = accepted
            self.registrations += 1
        return MAGIC + bytes([CONFIRM, len(accepted)]) + write_address(real)

    def forward(self, data, real, now):
        """Returns (packet, destination's real address) or None."""
        if len(data) < 17:
            self.drop("short packet")
            return None
        own = self.real.get(real)
        if not own:
            self.drop("sender not registered")
            return None
        destination = read_address(data, 5)
        entry = self.virtual.get(destination)
        if entry is None or entry[1] < now:
            self.drop("unknown destination")
            return None
        destination_real = entry[0]
        # The destination must see the sender under the address it knows it by: the one it
        # writes to itself, if it already did; else the declared one, if it really is the sender's.
        self.names[(real, destination_real)] = destination
        sender = self.names.get((destination_real, real))
        if sender not in own:
            sender = read_address(data, 11)
            if sender not in own:
                sender = own[0]
        count = self.forwarded.setdefault((sender, destination), [0, 0])
        count[0] += 1
        count[1] += len(data) - 17
        return MAGIC + bytes([RECEIVE]) + write_address(sender) + data[17:], destination_real

    def receive(self, data, real, now):
        """Handles one incoming packet; returns (reply, recipient) or None."""
        if len(data) > self.max_packet:
            self.drop("too large")
            return None
        if len(data) < 5 or data[:4] != MAGIC:
            self.drop("not a relay packet")
            return None
        if data[4] == REGISTER:
            reply = self.register(data, real, now)
            return (reply, real) if reply else None
        if data[4] == SEND:
            return self.forward(data, real, now)
        self.drop("unknown type")
        return None

    def status(self):
        lines = ["status: %d registered, %d registrations received" %
                 (len(self.virtual), self.registrations)]
        for (sender, destination), (packets, size) in sorted(self.forwarded.items()):
            lines.append("  %s -> %s: %d packets, %d bytes" %
                         (text(sender), text(destination), packets, size))
        if self.dropped:
            lines.append("  dropped: " + ", ".join("%s %d" % (r, n) for r, n in sorted(self.dropped.items())))
        return lines


def main(argv):
    p = argparse.ArgumentParser(description="Fallback UDP relay for Firewall Zero Hour P2P traffic")
    p.add_argument("--host", default="0.0.0.0", help="address to listen on (default 0.0.0.0)")
    p.add_argument("--port", type=int, default=31316, help="UDP port (default 31316)")
    p.add_argument("--status-interval", type=float, default=10.0,
                   help="seconds between count reports, written only when they change (0 = never)")
    p.add_argument("--max-packet", type=int, default=2048, help="largest packet accepted, in bytes")
    p.add_argument("--no-ip-check", action="store_true",
                   help="accept virtual addresses whose IP differs from the registering one")
    a = p.parse_args(argv)

    def write(line):
        print("%s %s" % (time.strftime("%H:%M:%S"), line), flush=True)

    relay = Relay(ip_check=not a.no_ip_check, max_packet=a.max_packet, write=write)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind((a.host, a.port))
    s.settimeout(1.0)
    write("P2P relay listening on %s:%d (UDP)" % (a.host, a.port))
    last_status, last_expiry, last_lines = time.monotonic(), time.monotonic(), None
    try:
        while True:
            try:
                data, real = s.recvfrom(65535)
            except socket.timeout:
                data = None
            except OSError:
                # On Windows a vanished recipient makes the next recvfrom fail.
                continue
            now = time.monotonic()
            if data is not None:
                result = relay.receive(data, real[:2], now)
                if result:
                    try:
                        s.sendto(result[0], result[1])
                    except OSError:
                        relay.drop("send failed")
            if now - last_expiry >= 1.0:
                last_expiry = now
                relay.expire(now)
            if a.status_interval > 0 and now - last_status >= a.status_interval:
                last_status = now
                lines = relay.status()
                if lines[1:] != last_lines:  # the first line changes with every registration
                    last_lines = lines[1:]
                    for line in lines:
                        write(line)
    except KeyboardInterrupt:
        pass
    finally:
        for line in relay.status():
            write(line)
        s.close()


if __name__ == "__main__":
    main(sys.argv[1:])
