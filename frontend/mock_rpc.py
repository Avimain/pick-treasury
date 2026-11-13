#!/usr/bin/env python3
"""Local mock of the NEAR testnet RPC (call_function only) for smoke-testing
frontend/index.html without hitting — or being rate-limited by — the real
endpoint. Wire format matches rpc.testnet.near.org exactly: JSON-RPC 2.0 with
`result.result` as an array of byte numbers.

    python3 frontend/mock_rpc.py          # listens on :8743
    # then in the page's settings: rpc = http://localhost:8743

Not used in production; purely a dev/QA tool.
"""
import json
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

NOW = int(time.time())

COIN = "pick-coin.testnet"
PASSPORT = "pick-passport.testnet"

EVENTS = [
    {
        "id": 0,
        "question": "Will the $PICK demo ship before 14 Nov 2026?",
        "side_a": "YES",
        "side_b": "NO",
        "close_ts": NOW + 86_400,
        "resolve_deadline": NOW + 172_800,
        "status": {"Open": None},
        "stake_a": "420000000000",
        "stake_b": "180000000000",
        "winner_pool": "0",
        "winner_total": "0",
        "paid_out": "0",
        "swept": False,
    },
    {
        "id": 1,
        "question": "BTC above $100k this week?",
        "side_a": "YES",
        "side_b": "NO",
        "close_ts": NOW - 172_800,
        "resolve_deadline": NOW - 86_400,
        "status": {"Resolved": {"winner": 0, "resolve_ts": NOW - 86_400}},
        "stake_a": "1000000000000",
        "stake_b": "500000000000",
        "winner_pool": "1410000000000",
        "winner_total": "1000000000000",
        "paid_out": "0",
        "swept": False,
    },
    {
        "id": 2,
        "question": "Will anyone stake event #2? (one-sided test)",
        "side_a": "A",
        "side_b": "B",
        "close_ts": NOW - 259_200,
        "resolve_deadline": NOW - 172_800,
        "status": {"Void": {"void_ts": NOW - 172_800}},
        "stake_a": "700000000000",
        "stake_b": "0",
        "winner_pool": "0",
        "winner_total": "0",
        "paid_out": "0",
        "swept": False,
    },
]

LEADERBOARD = [
    {"account": "alice.testnet", "streak": 7, "best": 9, "wins": 12, "losses": 4, "tier": "ORACLE"},
    {"account": "bob.testnet", "streak": 3, "best": 5, "wins": 6, "losses": 6, "tier": "TRADER"},
    {"account": "carol.testnet", "streak": 1, "best": 2, "wins": 2, "losses": 1, "tier": "ROOKIE"},
]

HISTORY = [
    {"ts": NOW - 30, "kind": "stake", "account": "alice.testnet", "event_id": 0, "amount": "420000000000"},
    {"ts": NOW - 60, "kind": "tax", "account": "bob.testnet", "event_id": 18446744073709551615, "amount": "5000000000"},
    {"ts": NOW - 90, "kind": "resolve", "account": "resolver.testnet", "event_id": 1, "amount": "1410000000000"},
    {"ts": NOW - 120, "kind": "claim", "account": "alice.testnet", "event_id": 1, "amount": "1410000000000"},
    {"ts": NOW - 150, "kind": "void", "account": "anyone.testnet", "event_id": 2, "amount": "0"},
]

VIEWS = {
    (COIN, "ft_total_supply"): "1000000000000000000",
    (COIN, "free_treasury"): "750000000000",
    (COIN, "ft_metadata"): {
        "spec": "ft-1.0.0", "name": "PICK", "symbol": "PICK",
        "icon": None, "decimals": 9, "reference": None, "reference_hash": None,
    },
    (COIN, "get_events"): EVENTS,
    (COIN, "get_leaderboard"): LEADERBOARD,
    (COIN, "get_history"): HISTORY,
    (COIN, "get_params"): {
        "resolver": "resolver.testnet", "passport": PASSPORT,
        "transfer_tax_bps": 100, "tax_burn_bps": 5000, "event_bonus_bps": 200,
        "loser_burn_bps": 2000, "trader_bonus_bps": 100, "oracle_bonus_bps": 300,
        "trader_wins": 5, "oracle_wins": 10, "claim_window_secs": 7776000,
    },
    (PASSPORT, "is_minted"): False,
    (PASSPORT, "get_summary"): None,
}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):  # quiet
        pass

    def _send(self, payload, code=200):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.end_headers()

    def do_POST(self):
        try:
            req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            params = req.get("params", {})
            key = (params.get("account_id"), params.get("method_name"))
            if key not in VIEWS:
                self._send({
                    "jsonrpc": "2.0", "id": req.get("id"),
                    "error": {"code": -32601, "name": "REQUEST_VALIDATION_ERROR",
                              "message": f"method not found: {key[1]}"},
                })
                return
            data = json.dumps(VIEWS[key])
            result = [b for b in data.encode()]  # byte numbers, like the real RPC
            self._send({"jsonrpc": "2.0", "id": req.get("id"),
                        "result": {"request_type": "call_function",
                                   "block_height": 1, "result": result}})
        except Exception as e:  # noqa: BLE001
            self._send({"jsonrpc": "2.0", "id": None,
                        "error": {"code": -32700, "message": str(e)}}, 400)


if __name__ == "__main__":
    print("mock NEAR rpc on http://localhost:8743  (coin=passport=" + COIN + ")")
    HTTPServer(("127.0.0.1", 8743), Handler).serve_forever()
