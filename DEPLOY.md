# DEPLOY — testnet runbook

You run the wallet-authenticated commands; this file has every exact call.
Prereqs: Rust + `wasm32-unknown-unknown` target, `cargo-near`, `near-cli-rs`.

```bash
rustup target add wasm32-unknown-unknown
cargo install cargo-near near-cli-rs
near --version && cargo near --version   # toolchain sanity check
```

All commands run from the repo root unless noted. Accounts actually used for
the live demo:

| Variable | Value |
|---|---|
| `COIN` | `pick-coin.testnet` |
| `PASSPORT` | `pick-passport.testnet` |
| `RESOLVER` | `pick-resolver.testnet` (creates/resolves events) |
| `TEAM` | `pick-team.testnet` (holds the initial supply) |
| `YOU` | your own wallet account |

## 1. Create the two contract accounts

```bash
near account create-account sponsor-by-faucet-service pick-coin.testnet \
  autogenerate-new-keypair save-to-keychain network-config testnet create
near account create-account sponsor-by-faucet-service pick-passport.testnet \
  autogenerate-new-keypair save-to-keychain network-config testnet create
```

(Any names you like; they must be distinct. The faucet-funded key lands in the
near-cli keychain, which is what `sign-with-keychain` uses below. Fallback if
your CLI version lacks the subcommand: create them from the wallet UI at
https://wallet.testnet.near.org → Create account.)

## 2. Build

```bash
cargo build --target wasm32-unknown-unknown --release
# or the production artifact (NEP-330 build info + wasm-opt: 255 KB vs 513 KB).
# cargo near must run from the contract's crate directory, not the workspace root:
cd coin   && cargo near build non-reproducible-wasm --no-abi && cd ..
cd passport && cargo near build non-reproducible-wasm --no-abi && cd ..
```

Artifacts: `target/wasm32-unknown-unknown/release/pick_coin.wasm` and
`.../pick_passport.wasm` (plain build), or `target/near/pick_coin/pick_coin.wasm`
and `target/near/pick_passport/pick_passport.wasm` (`cargo near`).

`--no-abi` skips Near-ABI generation, which requires `JsonSchema` on every type
appearing in a public signature. The ABI is metadata only — the contract runs
identically without it.

## 3. Deploy both

```bash
near contract deploy pick-coin.testnet     use-file target/wasm32-unknown-unknown/release/pick_coin.wasm     network-config testnet sign-as YOU sign-with-keychain send
near contract deploy pick-passport.testnet use-file target/wasm32-unknown-unknown/release/pick_passport.wasm network-config testnet sign-as YOU sign-with-keychain send
```

## 4. Initialize the coin

`new(params, meta, initial_supply)` — params **freeze here, forever**:

```bash
near contract call-function as-transaction pick-coin.testnet new \
  json-args '{
    "params": {
      "resolver": "pick-resolver.testnet",
      "passport": "pick-passport.testnet",
      "transfer_tax_bps": 100,
      "tax_burn_bps": 5000,
      "event_bonus_bps": 200,
      "loser_burn_bps": 2000,
      "trader_bonus_bps": 100,
      "oracle_bonus_bps": 300,
      "trader_wins": 5,
      "oracle_wins": 10,
      "claim_window_secs": 7776000
    },
    "meta": {
      "spec": "ft-1.0.0",
      "name": "PICK — parimutuel treasury coin",
      "symbol": "PICK",
      "icon": null,
      "decimals": 9,
      "reference": null,
      "reference_hash": null
    },
    "initial_supply": "1000000000000000000"
  }' \
  prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' \
  sign-as YOU network-config testnet sign-with-keychain send
```

`initial_supply` is 1,000,000,000 PICK × 10⁹ (yocto-style, 9 decimals),
minted entirely to the account that calls `new`.

## 5. Initialize the passport

```bash
near contract call-function as-transaction pick-passport.testnet new \
  json-args '{
    "coin": "pick-coin.testnet",
    "trader_wins": 5,
    "oracle_wins": 10,
    "meta": {
      "spec": "nft-1.0.0",
      "name": "Prediction Passport",
      "symbol": "PASS",
      "icon": "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjggMTI4Ij48Y2lyY2xlIGN4PSI2NCIgY3k9IjY0IiByPSI1NiIgZmlsbD0iIzFmNmZlYiIvPjxwYXRoIGQ9Ik00MCA2NCA1OCA4MiA5MCA0NiIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjgiIGZpbGw9Im5vbmUiLz48L3N2Zz4=",
      "base_uri": null,
      "reference": null,
      "reference_hash": null
    }
  }' \
  prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' \
  sign-as YOU network-config testnet sign-with-keychain send
```

**`trader_wins` / `oracle_wins` must equal the coin's values** (5/10) —
the tiers are intentionally duplicated in both contracts; mismatching them
only affects the passport's display tier, never money.

## 6. Smoke test

```bash
# read back frozen params — must match step 4 exactly
near contract call-function as-read-only pick-coin.testnet get_params json-args '{}' \
  network-config testnet now

near contract call-function as-read-only pick-coin.testnet ft_total_supply json-args '{}' \
  network-config testnet now
# -> "1000000000000000000"

near contract call-function as-read-only pick-coin.testnet get_events json-args '{}' \
  network-config testnet now
# -> []

# mint your passport (free, no deposit)
near contract call-function as-transaction pick-passport.testnet mint json-args '{}' \
  prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' \
  sign-as YOU network-config testnet sign-with-keychain send

near contract call-function as-read-only pick-coin.testnet has_passport_of \
  json-args '{"user": "YOU"}' network-config testnet now
# -> true   (registration round-trip passport -> coin worked)
```

If `has_passport_of` is `false`, the registration receipt failed — call
`repair()` on the passport as YOU and re-check.

## 7. First event (as RESOLVER)

```bash
# close in 1 day, resolve deadline in 2 days (unix seconds)
close=$(($(date +%s) + 86400)); deadline=$(($(date +%s) + 172800))
near contract call-function as-transaction pick-coin.testnet create_event \
  json-args "{
    \"question\": \"Will $PICK demo run before 14 Nov 2026?\",
    \"side_a\": \"YES\",
    \"side_b\": \"NO\",
    \"close_ts\": $close,
    \"resolve_deadline\": $deadline
  }" \
  prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' \
  sign-as RESOLVER network-config testnet sign-with-keychain send
```

Then transfer some PICK between two wallets (watch the 1% tax in
`get_history`), stake from both sides, `resolve`, `claim` — the exact demo
script in the README.

## 8. Point the frontend at your contracts

Open `frontend/index.html`, open **settings**, set:

- coin → `pick-coin.testnet`
- passport → `pick-passport.testnet`

Save (the page reloads with your addresses in `localStorage`). Serve with
`python3 -m http.server` from `frontend/` — opening the file directly works
too, since everything is `fetch`-based.

## 9. Go public

1. Create the GitHub repo, `git remote add origin …`, push `main`.
2. Host the frontend (GitHub Pages / Vercel / NEAR Pages) — one HTML file.
3. Record the 40s demo → post on X tagging **@NEARProtocol** and
   **#NEARHackathon**.
4. Submit the form (repo link + demo link + your NEAR wallet address)
   before **14 Nov 2026 · 23:59 UTC**.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `MethodDoesNotExist` on `get_params` | step 4 init never ran |
| `has_passport_of` false after mint | `repair()` on passport, re-check |
| `insufficient balance` on transfer | the holder needs a nonzero $PICK balance first (fund from the initial-supply holder) |
| frontend says "cannot reach coin contract" | addresses in settings wrong, or testnet RPC rate-limited (the page retries automatically) |
| `resolve` panics "deadline passed" | create events with a realistic `resolve_deadline`; refunds work in this state |
