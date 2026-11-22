import json, unittest

from tests.support import Backend, FakeShadnet, canonical_code, derive_password, new_recovery_code


class Registration(unittest.TestCase):
    def setUp(self):
        self.shadnet = FakeShadnet(Backend.KEY)
        self.backend = Backend("--register-cooldown", "0", shadnet=self.shadnet)

    def tearDown(self):
        self.backend.close()
        self.shadnet.close()

    def register(self, online_id, ip="198.51.100.1", code=None):
        password = derive_password(code or new_recovery_code(), online_id)
        return self.backend.call("POST", "/account/v1/register", {"onlineId": online_id, "password": password}, ip=ip)

    def test_success_creates_the_shadnet_account(self):
        code = new_recovery_code()
        status, body, headers = self.register("Night_Owl-7", code=code)
        self.assertEqual((status, body), (201, {"onlineId": "Night_Owl-7"}))
        self.assertEqual(headers["Cache-Control"], "no-store")
        account = self.shadnet.accounts["night_owl-7"]
        self.assertEqual(account["password"], derive_password(code, "Night_Owl-7"))
        self.assertEqual(account["email"], "night_owl-7@users.invalid")
        self.assertEqual(account["avatar"], "http://fzh.avatar/a/Night_Owl-7.png")

    def test_reserved_names(self):
        for name in ("admin", "Admin_2", "MODERATOR", "sys-tem", "FZH-team", "shadNet1", "root", "Fire_Wall"):
            self.assertEqual(self.register(name)[:2],
                             (403, {"error": "reserved_name", "message": "That Online ID is reserved."}), name)
        self.assertEqual(self.shadnet.accounts, {})
        self.assertEqual(self.register("rooster")[0], 201)

    def test_extra_reserved_names(self):
        self.backend.close()
        self.backend = Backend("--reserved-names", "Bad_Name, other", shadnet=self.shadnet)
        self.assertEqual(self.register("badname")[0], 403)
        self.assertEqual(self.register("Other")[0], 403)

    def test_duplicate_any_casing(self):
        self.assertEqual(self.register("Falcon")[0], 201)
        status, body, _ = self.register("fALCON", ip="198.51.100.2")
        self.assertEqual((status, body["error"]), (409, "name_taken"))
        self.shadnet.accounts["madeelsewhere"] = {"npid": "MadeElsewhere", "email": "x@y.z"}
        self.assertEqual(self.register("madeElsewhere", ip="198.51.100.3")[1]["error"], "name_taken")

    def test_bad_input(self):
        call = self.backend.call
        cases = [
            (b"not json", 400, "invalid_request"),
            ([1, 2], 400, "invalid_request"),
            ({"onlineId": "Abc"}, 400, "invalid_request"),
            ({"onlineId": 5, "password": "x" * 43}, 400, "invalid_request"),
            ({"onlineId": "ab", "password": "x" * 43}, 400, "invalid_name"),
            ({"onlineId": "a" * 17, "password": "x" * 43}, 400, "invalid_name"),
            ({"onlineId": "bad name", "password": "x" * 43}, 400, "invalid_name"),
            ({"onlineId": "café", "password": "x" * 43}, 400, "invalid_name"),
            ({"onlineId": "Abcd", "password": "short"}, 400, "invalid_request"),
            ({"onlineId": "Abcd", "password": "+" * 43}, 400, "invalid_request"),
            ({"onlineId": "Abcd", "password": "x" * 43, "pad": "y" * 5000}, 413, "too_large"),
        ]
        for body, status, error in cases:
            got_status, got, _ = call("POST", "/account/v1/register", body)
            self.assertEqual((got_status, got["error"]), (status, error), body if len(str(body)) < 80 else "big")
        self.assertEqual(self.shadnet.accounts, {})
        self.assertEqual(call("GET", "/account/v1/register")[0], 405)
        self.assertEqual(call("POST", "/account/v1/nothing", {})[0], 404)

    def test_per_address_day_limit_and_cooldown(self):
        self.backend.close()
        self.backend = Backend("--register-per-day", "2", "--register-cooldown", "0", shadnet=self.shadnet)
        self.assertEqual(self.register("First1")[0], 201)
        self.assertEqual(self.register("Second2")[0], 201)
        status, body, headers = self.register("Third3")
        self.assertEqual((status, body["error"]), (429, "rate_limited"))
        self.assertGreater(int(headers["Retry-After"]), 80000)
        self.assertEqual(body["retryAfter"], int(headers["Retry-After"]))
        self.assertEqual(self.register("Third3", ip="198.51.100.9")[0], 201)

        self.backend.close()
        self.backend = Backend("--register-cooldown", "60", shadnet=self.shadnet)
        self.assertEqual(self.register("Fourth4")[0], 201)
        status, body, headers = self.register("Fifth5")
        self.assertEqual(status, 429)
        self.assertTrue(0 < int(headers["Retry-After"]) <= 61)

    def test_failed_attempts_do_not_count_as_registrations(self):
        self.backend.close()
        self.backend = Backend("--register-per-day", "1", "--register-cooldown", "60", shadnet=self.shadnet)
        self.assertEqual(self.register("admin")[0], 403)
        self.assertEqual(self.register("ab")[0], 400)
        self.assertEqual(self.register("Taken1", ip="198.51.100.5")[0], 201)
        self.assertEqual(self.register("taken1")[0], 409)
        self.assertEqual(self.register("Mine1")[0], 201)

    def test_global_limit(self):
        self.backend.close()
        self.backend = Backend("--register-global-per-hour", "2", "--register-cooldown", "0", shadnet=self.shadnet)
        self.assertEqual(self.register("One111", ip="203.0.113.1")[0], 201)
        self.assertEqual(self.register("Two222", ip="203.0.113.2")[0], 201)
        self.assertEqual(self.register("Three3", ip="203.0.113.3")[0], 429)

    def test_request_limit_per_address(self):
        statuses = [self.backend.call("GET", "/account/v1/names/Nobody1")[0] for _ in range(31)]
        self.assertEqual(statuses[:30], [404] * 30)
        self.assertEqual(statuses[30], 429)
        self.assertEqual(self.backend.call("GET", "/account/v1/names/Nobody1", ip="198.51.100.77")[0], 404)

    def test_closed_without_shadnet(self):
        self.backend.close()
        self.backend = Backend()
        self.assertEqual(self.register("Someone")[1]["error"], "registration_closed")

    def test_wrong_key_closes_registration_and_stays_out_of_the_log(self):
        self.shadnet.key = "another-key"
        status, body, _ = self.register("Someone")
        self.assertEqual((status, body["error"]), (503, "registration_closed"))
        self.assertIn("shadNet refused the registration key", self.backend.log())
        self.assertNotIn(Backend.KEY, self.backend.log())

    def test_shadnet_down(self):
        self.shadnet.close()
        status, body, _ = self.register("Someone")
        self.assertEqual((status, body["error"]), (502, "shadnet_unavailable"))


class Recovery(unittest.TestCase):
    """The launcher of a new PC derives the password from the recovery code alone; the backend
    only answers the registered casing of the name."""

    def setUp(self):
        self.shadnet = FakeShadnet(Backend.KEY)
        self.backend = Backend(shadnet=self.shadnet)

    def tearDown(self):
        self.backend.close()
        self.shadnet.close()

    def test_contract_vector(self):
        self.assertEqual(derive_password("ABCDE-FGHJK-MNPQR-STVWX-YZ012", "Player_One"),
                         "jJXhJrVT4CM79WTqRKwHHabrAZAfDbx8wWs1nCbXaCc")

    def test_code_as_typed(self):
        code = "ABCDE-FGHJK-MNPQR-STVWX-YZ012"
        for typed in ("abcde fghjk mnpqr stvwx yz012", "ABCDEFGHJKMNPQRSTVWXYZ0I2", "ABCDE-FGHJK-MNPQR-STVWX-YZOL2"):
            self.assertEqual(canonical_code(typed), canonical_code(code))
            self.assertEqual(derive_password(typed, "player_one"), derive_password(code, "Player_One"))
        for bad in ("ABCDE-FGHJK", "ABCDE-FGHJK-MNPQR-STVWX-YZ01U", "ABCDE-FGHJK-MNPQR-STVWX-YZ0123"):
            self.assertRaises(ValueError, canonical_code, bad)

    def test_new_pc_signs_in_with_name_and_code(self):
        code = new_recovery_code()
        password = derive_password(code, "Grey_Fox")
        self.assertEqual(self.backend.call("POST", "/account/v1/register",
                                           {"onlineId": "Grey_Fox", "password": password})[0], 201)
        status, body, _ = self.backend.call("GET", "/account/v1/names/grey_FOX", ip="192.0.2.50")
        self.assertEqual((status, body), (200, {"onlineId": "Grey_Fox"}))
        typed = code.lower().replace("-", " ")
        self.assertEqual(derive_password(typed, body["onlineId"]), self.shadnet.accounts["grey_fox"]["password"])
        self.assertNotEqual(derive_password(new_recovery_code(), "Grey_Fox"), password)

    def test_lookup(self):
        self.assertEqual(self.backend.call("GET", "/account/v1/names/Unknown1")[1]["error"], "not_found")
        self.assertEqual(self.backend.call("GET", "/account/v1/names/a%20b")[1]["error"], "invalid_name")
        self.assertEqual(self.backend.call("POST", "/account/v1/names/Unknown1")[0], 405)

    def test_the_backend_keeps_the_name_and_no_secret(self):
        password = derive_password(new_recovery_code(), "Kept_Name")
        self.backend.call("POST", "/account/v1/register", {"onlineId": "Kept_Name", "password": password})
        with open(self.backend.dir + "/backend-auth.json") as f:
            stored = json.load(f)
        self.assertEqual(stored["accounts"], {"kept_name": {"onlineId": "Kept_Name",
                                                            "createdAt": stored["accounts"]["kept_name"]["createdAt"]}})
        self.assertNotIn(password, json.dumps(stored))
        self.assertNotIn(password, self.backend.log())


if __name__ == "__main__":
    unittest.main()
