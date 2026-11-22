import time, unittest

from backend import plain_png
from tests.support import Backend, FakeShadnet, FakeWebApi, derive_password, new_recovery_code

HOME_A, HOME_B, ELSEWHERE = "198.51.100.10", "198.51.100.20", "203.0.113.99"


class GameAuth(unittest.TestCase):
    def setUp(self):
        self.webapi = FakeWebApi()
        self.backend = Backend("--game-login", "grant", webapi=self.webapi)

    def tearDown(self):
        self.backend.close()
        self.webapi.close()

    def play(self, user_id, online_id, ip):
        return self.backend.call("POST", "/account/v1/play", {"code": self.webapi.issue(user_id, online_id)}, ip=ip)

    def game_login(self, user_id, ip, nickname="whatever"):
        return self.backend.call("POST", "/parse/functions/login",
                                 {"uniqueNetId": str(user_id), "nickname": nickname, "platform": "ps4",
                                  "accessToken": ""}, ip=ip)

    def signed_in(self, user_id, online_id, ip):
        self.assertEqual(self.play(user_id, online_id, ip)[0], 200)
        status, body, _ = self.game_login(user_id, ip)
        self.assertEqual(status, 200)
        return body["result"]["playerStatsId"]

    def test_play_grant(self):
        status, body, _ = self.play(7, "Raven", HOME_A)
        self.assertEqual((status, body), (200, {"accountId": "7", "onlineId": "Raven", "address": HOME_A,
                                                "expiresIn": 12 * 3600}))

    def test_play_refuses_bad_codes(self):
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {"code": "nope42"})[1]["error"], "invalid_code")
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {"code": "a/b"})[1]["error"], "invalid_request")
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {})[1]["error"], "invalid_request")
        other_client = self.webapi.issue(7, "Raven", client_id="some-game")
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {"code": other_client})[0], 401)
        used = self.webapi.issue(7, "Raven")
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {"code": used})[0], 200)
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {"code": used})[0], 401)

    def test_play_without_webapi(self):
        self.webapi.close()
        self.assertEqual(self.backend.call("POST", "/account/v1/play", {"code": "abc123"})[:2],
                         (502, {"error": "shadnet_unavailable", "message": "The account server did not answer."}))

    def test_game_login_needs_a_grant_from_the_same_address(self):
        self.assertEqual(self.game_login(7, HOME_A)[0], 403)
        self.play(7, "Raven", HOME_A)
        self.assertEqual(self.game_login(7, ELSEWHERE)[0], 403)
        self.assertEqual(self.game_login(8, HOME_A)[0], 403)
        status, body, _ = self.game_login(7, HOME_A, nickname="Impostor")
        self.assertEqual(status, 200)
        pid = body["result"]["playerStatsId"]
        self.assertRegex(pid, r"^[A-Za-z0-9]{10}$")
        status, body, _ = self.backend.call("GET", "/parse/classes/PlayerStats/" + pid, ip=HOME_A)
        self.assertEqual(body["results"][0]["nickname"], "Raven")

    def test_own_data_only(self):
        a = self.signed_in(1, "Alpha", HOME_A)
        b = self.signed_in(2, "Bravo", HOME_B)
        call = self.backend.call
        self.assertEqual(call("POST", "/parse/functions/setUserPrefs", {"playerStatsId": a, "data": "QQ=="}, ip=HOME_A)[0], 200)
        self.assertEqual(call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": a}, ip=HOME_A)[1], {"result": "QQ=="})

        for method, path, body in (
                ("POST", "/parse/functions/getUserPrefs", {"playerStatsId": a}),
                ("POST", "/parse/functions/setUserPrefs", {"playerStatsId": a, "data": "Qg=="}),
                ("GET", "/parse/classes/PlayerStats/" + a, None),
                ("GET", '/parse/classes/PlayerStats?where={"objectId":"%s"}&limit=1' % a, None),
                ("PUT", "/parse/classes/PlayerStats/" + a, {"Rank": {"__op": "Increment", "amount": 5}}),
                ("GET", "/parse/classes/PlayerStats", None),
                ("POST", "/parse/functions/getUserPrefs", {"playerStatsId": "0123456789"})):
            for ip in (HOME_B, ELSEWHERE):
                self.assertEqual(call(method, path, body, ip=ip)[:2], (403, {"code": 209, "error": "invalid session"}),
                                 (method, path, ip))
        self.assertEqual(call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": a}, ip=HOME_A)[1], {"result": "QQ=="})
        self.assertEqual(call("GET", "/parse/classes/PlayerStats/" + a, ip=HOME_A)[1]["results"][0].get("Rank"), None)
        self.assertEqual(call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": b}, ip=HOME_B)[0], 200)

    def test_protected_fields_and_limits(self):
        a = self.signed_in(1, "Alpha", HOME_A)
        call = self.backend.call
        call("PUT", "/parse/classes/PlayerStats/" + a,
             {"Rank": {"__op": "Increment", "amount": 2}, "objectId": "x", "uniqueNetId": "2", "nickname": "Zed",
              "userPrefs": "y", "Big": "z" * 5000, "Title": {"__op": "Set", "amount": "Major"}}, ip=HOME_A)
        p = call("GET", "/parse/classes/PlayerStats/" + a, ip=HOME_A)[1]["results"][0]
        self.assertEqual((p["objectId"], p["uniqueNetId"], p["nickname"], p["Rank"], p["Title"]), (a, "1", "Alpha", 2, "Major"))
        self.assertNotIn("Big", p)
        self.assertNotIn("userPrefs", p)
        self.assertEqual(call("POST", "/parse/functions/setUserPrefs",
                              {"playerStatsId": a, "data": "A" * (256 * 1024 + 1)}, ip=HOME_A)[0], 400)

    def test_match_results_from_the_host(self):
        host = self.signed_in(1, "Alpha", HOME_A)
        guest = self.signed_in(2, "Bravo", HOME_B)
        call = self.backend.call
        result =[{host: {"Coins": {"__op": "Increment", "amount": 500000}},
                   guest: {"Coins": {"__op": "Increment", "amount": 50}, "XP": {"__op": "Increment", "amount": 10 ** 9},
                           "Debt": {"__op": "Increment", "amount": -5}, "Title": {"__op": "Set", "amount": "x"}},
                   "notaplayer": {"Coins": {"__op": "Increment", "amount": 1}}}]
        self.assertEqual(call("POST", "/parse/classes/GameStats", result, ip=ELSEWHERE.replace("99", "98"))[0], 403)
        self.assertEqual(call("POST", "/parse/classes/GameStats", result, ip=HOME_A)[0], 201)
        p_host = call("GET", "/parse/classes/PlayerStats/" + host, ip=HOME_A)[1]["results"][0]
        p_guest = call("GET", "/parse/classes/PlayerStats/" + guest, ip=HOME_B)[1]["results"][0]
        self.assertEqual(p_host["Coins"], 500000)
        self.assertEqual(p_guest["Coins"], 50)
        for field in ("XP", "Debt", "Title"):
            self.assertNotIn(field, p_guest)
        self.assertEqual(call("GET", "/parse/classes/GameStats", ip=HOME_A)[1], {"results": [], "count": 0})

    def test_open_mode_still_binds_sessions_to_the_address(self):
        self.backend.close()
        self.backend = Backend("--game-login", "open", webapi=self.webapi)
        status, body, _ = self.game_login(5, HOME_A)
        self.assertEqual(status, 200)
        pid = body["result"]["playerStatsId"]
        self.assertEqual(self.backend.call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": pid}, ip=HOME_B)[0], 403)
        self.assertEqual(self.backend.call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": pid}, ip=HOME_A)[0], 200)

    def test_default_mode_on_loopback_is_open(self):
        self.backend.close()
        self.backend = Backend(webapi=self.webapi)
        self.assertEqual(self.game_login(5, HOME_A)[0], 200)

    def test_sessions_and_grants_expire(self):
        self.backend.close()
        self.backend = Backend("--game-login", "grant", "--session-hours", "0.0005", webapi=self.webapi)
        pid = self.signed_in(1, "Alpha", HOME_A)
        time.sleep(2.2)
        self.assertEqual(self.backend.call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": pid}, ip=HOME_A)[0], 403)
        self.assertEqual(self.game_login(1, HOME_A)[0], 403)

    def test_sessions_survive_a_restart(self):
        pid = self.signed_in(1, "Alpha", HOME_A)
        self.backend.restart()
        self.assertEqual(self.backend.call("POST", "/parse/functions/getUserPrefs", {"playerStatsId": pid}, ip=HOME_A)[0], 200)
        self.assertEqual(self.game_login(1, HOME_A)[0], 200)

    def test_game_login_limit(self):
        self.backend.close()
        self.backend = Backend("--game-login", "open", webapi=self.webapi)
        statuses = [self.game_login(i, ELSEWHERE)[0] for i in range(21)]
        self.assertEqual(statuses, [200] * 20 + [429])

    def test_proxy_header_only_from_trusted_peers(self):
        self.backend.close()
        self.backend = Backend("--game-login", "grant", "--trusted-proxies", "192.0.2.1", webapi=self.webapi)
        status, body, _ = self.play(7, "Raven", HOME_A)
        self.assertEqual(body["address"], "127.0.0.1")

    def test_avatar_upload_keeps_its_token_check(self):
        self.webapi.tokens["tok-alpha"] = "Alpha"
        png = plain_png("x")
        call = self.backend.call
        self.assertEqual(call("PUT", "/a/Alpha.png", png, headers={"Authorization": "Bearer tok-alpha"})[0], 200)
        self.assertEqual(call("PUT", "/a/Bravo.png", png, headers={"Authorization": "Bearer tok-alpha"})[0], 403)
        self.assertEqual(call("PUT", "/a/Alpha.png", png, headers={"Authorization": "Bearer wrong"})[0], 403)
        self.assertEqual(call("PUT", "/a/Alpha.png", png)[0], 403)
        status, body, headers = call("GET", "/a/alpha.png", ip=ELSEWHERE)
        self.assertEqual((status, body, headers.get("X-Avatar-Placeholder")), (200, png, None))


class LogRedaction(unittest.TestCase):
    def test_no_secret_reaches_the_log(self):
        webapi, shadnet = FakeWebApi(), FakeShadnet(Backend.KEY)
        b = Backend("--game-login", "grant", shadnet=shadnet, webapi=webapi)
        try:
            password = derive_password(new_recovery_code(), "Secretive")
            self.assertEqual(b.call("POST", "/account/v1/register", {"onlineId": "Secretive", "password": password})[0], 201)
            code = webapi.issue(9, "Secretive")
            self.assertEqual(b.call("POST", "/account/v1/play", {"code": code}, ip=HOME_A)[0], 200)
            access_tokens = list(webapi.access)
            status, login, _ = b.call("POST", "/parse/functions/login", {"uniqueNetId": "9", "accessToken": "game-token-1"}, ip=HOME_A)
            session_token = login["result"]["sessionToken"]
            zygo = b.call("POST", "/?provider=psn&code=zygo-code-1", ip=HOME_A)[1]
            webapi.tokens["bearer-token-1"] = "Secretive"
            b.call("PUT", "/a/Secretive.png", b"x", headers={"Authorization": "Bearer bearer-token-1",
                                                               "X-Parse-REST-API-Key": "rest-key-1",
                                                               "X-Parse-Application-Id": "app-id-1"})
            b.call("POST", "/account/v1/register", b'{"onlineId": "Broken", "password": "' + password.encode())
            log = b.log()
            secrets_seen = [password, code, Backend.KEY, session_token, "game-token-1", "zygo-code-1",
                            "bearer-token-1", "rest-key-1", "app-id-1", zygo["session"]["token"],
                            zygo["session"]["accessToken"], zygo["session"]["sessionToken"]] + access_tokens
            for secret in secrets_seen:
                self.assertNotIn(secret, log)
            self.assertIn("<redacted>", log)
            self.assertIn("POST 127.0.0.1:%d/account/v1/register" % b.port, log)
            self.assertIn('"playerStatsId"', log)
        finally:
            b.close()
            webapi.close()
            shadnet.close()


if __name__ == "__main__":
    unittest.main()
