import unittest

from tests.support import Backend

HOME, ELSEWHERE = "198.51.100.10", "203.0.113.99"


class Catalogue(unittest.TestCase):
    def setUp(self):
        self.backend = Backend("--game-login", "open")
        status, body, _ = self.call("functions/login", {"uniqueNetId": "7", "nickname": "Raven"})
        self.assertEqual(status, 200)
        self.pid = body["result"]["playerStatsId"]

    def tearDown(self):
        self.backend.close()

    def call(self, resource, body=None, method="POST", ip=HOME):
        return self.backend.call(method, "/parse/" + resource, body, ip=ip)

    def stats(self, **fields):
        self.assertEqual(self.call("classes/PlayerStats/" + self.pid, fields, method="PUT")[0], 200)

    def owned(self):
        status, body, _ = self.call("functions/getReceiptsByCategory", {"category": "", "playerStatsId": self.pid})
        self.assertEqual(status, 200)
        return {r["productId"] for r in body["result"]}

    def buy(self, product):
        return self.call("functions/purchaseProduct", {"playerStatsId": self.pid, "productId": product})[:2]

    def test_catalogue_entries_have_the_fields_the_game_reads(self):
        status, body, _ = self.call("functions/getProductCatalog")
        self.assertEqual(status, 200)
        entries = {e["productId"]: e for e in body["result"]}
        self.assertEqual(entries["Primary002"], {"productId": "Primary002", "category": "Primary_Weapons",
                                                 "name": "KRUGER", "unlockLevel": 7, "cost": 15000, "klass": ""})
        self.assertEqual(entries["Equipment017"]["klass"], "Tactical")
        for e in body["result"]:
            self.assertEqual([type(e[k]) for k in ("productId", "category", "name", "unlockLevel", "cost", "klass")],
                             [str, str, str, int, int, str])

    def test_free_products_are_owned_from_the_start(self):
        owned = self.owned()
        self.assertIn("Perk029", owned)
        self.assertNotIn("Primary002", owned)

    def test_receipts_need_the_player_s_session(self):
        body = {"category": "", "playerStatsId": self.pid}
        self.assertEqual(self.call("functions/getReceiptsByCategory", body, ip=ELSEWHERE)[0], 403)
        self.assertEqual(self.call("functions/purchaseProduct", {"playerStatsId": self.pid, "productId": "Perk001"},
                                   ip=ELSEWHERE)[0], 403)

    def test_purchase_takes_the_coins_and_answers_the_new_balance(self):
        self.stats(Rank=7, Coins=20000)
        self.assertEqual(self.buy("primary002"), (200, {"result": {"coins": 5000, "productId": "primary002"}}))
        self.assertIn("Primary002", self.owned())
        self.assertEqual(self.call("classes/PlayerStats/" + self.pid, method="GET")[1]["results"][0]["Coins"], 5000)
        self.assertEqual(self.buy("Primary002"), (200, {"result": {"coins": 5000, "productId": "Primary002"}}))
        self.backend.restart()
        self.call("functions/login", {"uniqueNetId": "7", "nickname": "Raven"})
        self.assertIn("Primary002", self.owned())

    def test_purchase_refusals(self):
        self.stats(Rank=6, Coins=20000)
        self.assertEqual(self.buy("Primary002")[0], 400)
        self.stats(Rank=7, Coins=14999)
        self.assertEqual(self.buy("Primary002")[0], 400)
        self.assertEqual(self.buy("NoSuchProduct")[0], 400)
        self.assertEqual(self.call("functions/purchaseProduct", {"playerStatsId": self.pid})[0], 400)
        self.assertNotIn("Primary002", self.owned())

    def missions(self):
        status, body, _ = self.call('functions/missions?where={"objectId":"%s"}' % self.pid, method="GET")
        self.assertEqual((status, list(body)), (200, [self.pid]))
        return body[self.pid]

    def claim(self, *products):
        return self.call("functions/purchaseProducts", {"playerStatsId": self.pid, "productIds": list(products)})[:2]

    def test_a_mission_is_complete_from_its_rank_on(self):
        self.stats(Rank=2)
        self.assertEqual(self.missions(), {"daily": {"active": [], "completed": [], "deleted": []},
                                           "seasonal": [], "rewards": []})
        self.stats(Rank=3)
        progress = self.missions()
        self.assertIn({"id": "Free_PlayerKillsARs_01", "value": 8}, progress["seasonal"])
        self.assertEqual(progress["rewards"], [{"id": "Reward_FreeMission_01"}])
        self.assertEqual(self.call("functions/missions/daily", [{"id": "x"}], method="PUT")[:2], (204, b""))

    def test_the_reward_of_a_complete_mission_is_claimed_at_no_cost(self):
        self.stats(Rank=2, Coins=100)
        self.assertEqual(self.claim("TrinketNightfall_0001")[0], 400)
        self.stats(Rank=3)
        self.assertEqual(self.claim("TrinketNightfall_0001"),
                         (200, {"result": {"coins": 100, "productIds": ["TrinketNightfall_0001"]}}))
        self.assertIn("TrinketNightfall_0001", self.owned())
        self.assertEqual(self.claim("Perk001")[0], 400)
        self.assertEqual(self.claim("TrinketNightfall_0001", "Perk001")[0], 400)
        self.assertEqual(self.claim()[0], 400)
        self.assertNotIn("Perk001", self.owned())
        self.assertEqual(self.claim("TrinketNightfall_0001", "Primary025")[0], 400)
        self.stats(Rank=6)
        self.assertEqual(self.claim("TrinketNightfall_0001", "Primary025")[0], 200)
        self.assertLessEqual({"TrinketNightfall_0001", "Primary025"}, self.owned())
