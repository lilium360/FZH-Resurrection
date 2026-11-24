import json, os, socket, time, unittest

import backend
from tests.support import Backend, FakeRelay

PATH = "/status/v1/relays"


class Relays(unittest.TestCase):
    def setUp(self):
        saved = backend.RELAY_ROUND, backend.RELAY_TIMEOUT
        backend.RELAY_ROUND, backend.RELAY_TIMEOUT = 0.1, 0.2
        self.addCleanup(lambda: setattr(backend, "RELAY_ROUND", saved[0]) or setattr(backend, "RELAY_TIMEOUT", saved[1]))
        self.relay = FakeRelay()
        self.addCleanup(self.relay.close)
        # A port that was free a moment ago: nothing answers there.
        gone = FakeRelay()
        self.dead = gone.address
        gone.close()

    def start(self, entries, raw=None):
        self.backend = Backend()
        self.addCleanup(self.backend.close)
        self.addCleanup(lambda: backend.relay_watch and backend.relay_watch.stop.set())
        self.list = os.path.join(self.backend.dir, "relays.json")
        self.write(entries, raw)
        self.backend.argv += ["--relay-list", self.list]
        self.backend.restart()

    def write(self, entries, raw=None):
        with open(self.list + ".tmp", "w", encoding="utf-8") as f:
            f.write(json.dumps(entries) if raw is None else raw)
        os.replace(self.list + ".tmp", self.list)

    def relays(self, wanted, seconds=10):
        """The published list once it equals `wanted`; fails with the last one seen otherwise."""
        end = time.monotonic() + seconds
        while True:
            status, body, _ = self.backend.call("GET", PATH)
            self.assertEqual(status, 200)
            if body["relays"] == wanted or time.monotonic() > end:
                self.assertEqual(body["relays"], wanted)
                return body
            time.sleep(0.05)

    def entry(self, address, region="Europe", location="Stockholm, Sweden", operator="someone"):
        return {"address": address, "region": region, "location": location, "operator": operator}

    def shown(self, online, region="Europe", location="Stockholm, Sweden", operator="someone"):
        return {"region": region, "location": location, "operator": operator, "online": online}

    def test_a_relay_that_answers_is_online_and_one_that_does_not_is_offline(self):
        self.start([self.entry(self.relay.address), self.entry(self.dead, "Asia", "Singapore", "other"),
                    self.entry("no-such-host.invalid", "Africa", "Nowhere", "nobody")])
        body = self.relays([self.shown(True), self.shown(False, "Asia", "Singapore", "other"),
                            self.shown(False, "Africa", "Nowhere", "nobody")])
        self.assertRegex(body["checkedAt"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.000Z$")
        self.assertNotIn(str(self.relay.port), json.dumps(body))
        self.assertEqual(self.backend.call("GET", PATH)[2]["Cache-Control"], "max-age=30")

    def test_a_relay_that_stops_goes_offline_at_the_next_round(self):
        self.start([self.entry(self.relay.address)])
        self.relays([self.shown(True)])
        self.relay.close()
        self.relays([self.shown(False)])

    def test_the_list_is_read_again_when_the_file_changes(self):
        self.start([self.entry(self.dead)])
        self.relays([self.shown(False)])
        self.write([self.entry(self.dead), self.entry(self.relay.address, "Oceania", "Sydney, Australia", "new")])
        self.relays([self.shown(False), self.shown(True, "Oceania", "Sydney, Australia", "new")])
        self.write([], raw="[{")
        time.sleep(0.5)
        self.relays([self.shown(False), self.shown(True, "Oceania", "Sydney, Australia", "new")])
        self.assertIn("relay list: cannot read", self.backend.log())

    def test_before_the_first_round_the_status_is_unknown(self):
        silent = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.addCleanup(silent.close)
        silent.bind(("127.0.0.1", 0))
        backend.RELAY_TIMEOUT = 2
        self.start([self.entry("127.0.0.1:%d" % silent.getsockname()[1])])
        status, body, _ = self.backend.call("GET", PATH)
        self.assertEqual((status, body), (200, {"checkedAt": None, "relays": [self.shown(None)]}))

    def test_without_the_setting_there_is_no_such_endpoint(self):
        b = Backend()
        self.addCleanup(b.close)
        self.assertIsNone(backend.relay_watch)
        status, body, _ = b.call("GET", PATH)
        self.assertEqual((status, body["error"]), (404, "not_found"))

    def test_other_paths_and_methods(self):
        self.start([self.entry(self.relay.address)])
        self.assertEqual(self.backend.call("GET", "/status/v1/other")[0], 404)
        status, body, headers = self.backend.call("POST", PATH, {})
        self.assertEqual((status, body["error"], headers["Allow"]), (405, "method_not_allowed", "GET"))

    def test_a_backend_on_a_public_address_serves_it(self):
        self.start([self.entry(self.relay.address)])
        backend.Handler.remote = True
        self.addCleanup(setattr, backend.Handler, "remote", False)
        self.relays([self.shown(True)])
        self.assertEqual(self.backend.call("GET", "/status")[0], 404)

    def test_requests_are_limited_per_address(self):
        self.start([self.entry(self.relay.address)])
        limit = backend.STATUS_LIMIT[0]
        self.assertEqual({self.backend.call("GET", PATH, ip="198.51.100.7")[0] for _ in range(limit)}, {200})
        status, body, headers = self.backend.call("GET", PATH, ip="198.51.100.7")
        self.assertEqual((status, body["error"]), (429, "rate_limited"))
        self.assertGreater(int(headers["Retry-After"]), 0)
        self.assertEqual(self.backend.call("GET", PATH, ip="198.51.100.8")[0], 200)

    def test_a_malformed_list_is_tolerated(self):
        entries = [self.entry(self.relay.address), "text", {"region": "Europe"}, {"address": 5}, {"address": ""},
                   {"address": self.dead, "region": 7, "location": "x" * 200},
                   {"address": "127.0.0.1:99999999"}, {"address": ":"}]
        self.start(entries)
        blank = {"region": "", "location": "", "operator": "", "online": False}
        self.relays([self.shown(True), dict(blank, location="x" * 64), blank, blank])
        for raw in ("{", '{"address": "127.0.0.1"}', "null", ""):
            self.start([], raw=raw)
            self.assertEqual(self.relays([])["relays"], [])
        self.start([])
        os.remove(self.list)
        self.backend.restart()
        self.relays([])


if __name__ == "__main__":
    unittest.main()
