#!/usr/bin/env python3
"""Asks every relay of server/relay/relays.json (or the addresses given) whether it answers, and
how long the round trip takes from this machine.

  python tools/check-relays.py [host[:port] ...]

It sends one REGISTER of the relay's protocol (server/relay/relay.py) for an address that is not
this machine's, which a relay answers with a CONFIRM and does not keep. Exit code 1 when a relay
does not answer.
"""
import json, os, socket, struct, sys, time

DEFAULT_PORT = 31316
LIST = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "server", "relay", "relays.json")


def split(address):
    host, _, port = address.rpartition(":")
    return (host, int(port)) if host and port.isdigit() else (address, DEFAULT_PORT)


def probe(address, tries=3, timeout=3.0):
    """Best round trip in milliseconds, or None."""
    packet = b"FZR1" + bytes([1, 1]) + socket.inet_aton("203.0.113.1") + struct.pack("!H", 3658)
    best = None
    for _ in range(tries):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.settimeout(timeout)
            try:
                start = time.monotonic()
                s.sendto(packet, split(address))
                reply, _ = s.recvfrom(2048)
            except OSError:
                continue
            if reply[:5] == b"FZR1\x02":
                took = (time.monotonic() - start) * 1000
                best = took if best is None else min(best, took)
    return best


def main(argv):
    if argv:
        relays = [{"address": a} for a in argv]
    else:
        with open(LIST, encoding="utf-8") as f:
            relays = json.load(f)
    failed = 0
    for relay in relays:
        took = probe(relay["address"])
        where = " (%s, %s)" % (relay["location"], relay["operator"]) if "location" in relay else ""
        if took is None:
            failed += 1
            print("no answer  %s%s" % (relay["address"], where))
        else:
            print("%6.0f ms  %s%s" % (took, relay["address"], where))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
