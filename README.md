# $PICK — Parimutuel Treasury Memecoin × Prediction Passport

**Track fit:** `11 · Memecoins With Mechanics` (primary) + `10 · NFTs That Do Something` (secondary)

A memecoin where the meme has rules: every $PICK transfer taxes into a treasury
that funds weekly parimutuel prediction pools. Holders stake on events, winners
split the pot (their stake + a treasury bonus), a fixed slice of losing stakes
burns, and streaks are tracked onchain. Your **Prediction Passport** — a
soulbound NFT — records every pick you make and unlocks tier bonuses on claims.

> "The joke needs rules." — every parameter below is frozen at deploy, readable
> by anyone via `get_params`, and has **no admin key that can change it**.

## Contracts

| Contract | Purpose |
|---|---|
| `coin` (`pick_coin.wasm`) | NEP-141 subset: transfer tax, treasury, events, staking, resolution, claims, streaks, log |
| `passport` (`pick_passport.wasm`) | Soulbound NEP-171 NFT: pick/result mirror, tier display, registers the perk gate in the coin |

Deployment addresses: `COIN_ADDRESS = <todo>`, `PASSPORT_ADDRESS = <todo>`

## Mechanics (exactly as implemented)

**Transfer tax** (user → user only; contract-operated transfers are untaxed):

```
tax   = amount × transfer_tax_bps / 10_000      (100 = 1%)
burn  = tax   × tax_burn_bps / 10_000           (5000 → half the tax burns)
recipient += amount − tax ; treasury += tax − burn ; supply −= burn
```

**Stake** — one pick per user per event (top-ups on the same side, the opposite
side panics). Escrowed stakes sit in the contract's own balance; the invariant
`balance[contract] ≥ escrow` and `Σ balances == total_supply` are asserted by
unit tests.

**Resolve** (resolver, before `resolve_deadline`; either side empty → Void):

```
lose_burn = losing_side × loser_burn_bps / 10_000        (2000 → 20% burns)
bonus     = free_treasury × event_bonus_bps / 10_000     (200  → 2% of treasury)
winner_pool = winner_side + losing_side − lose_burn + bonus
```

**Claim** (winners; `claim_window_secs` after resolution, then `sweep`):

```
payout      = stake_i × winner_pool / winner_side        (u128×u128 via U256)
tier_bonus  = payout × tier_bps / 10_000                 (capped by free treasury)
tier_bps    = 0 without a passport; TRADER ≥ trader_wins wins; ORACLE ≥ oracle_wins
```

**Refund** — Void events, or still-unresolved events after the deadline (the
resolver is *locked out* of late resolutions, so nobody can lose funds to a
missing resolution).

**Streaks** — settled per pick (win → +1, loss → 0). `settle_for` is
permissionless: records can't be hidden by refusing to claim.

## Trust model & honest limitations (read before judging)

1. **Resolution is centralized by design and documented:** events are created
   and resolved by the `resolver` account frozen in `params.resolver` at
   deploy. It cannot be changed. Its failures are bounded: unresolved events
   auto-refund after `resolve_deadline`. Crypto-price events could be moved to
   a price-feed resolver without touching the staking math (roadmap).
2. **Passport mirrors are best-effort.** The coin fires `record_pick` /
   `record_result` at the passport as fire-and-forget receipts. The coin's own
   state is authoritative; `passport.repair()` re-registers the perk if the
   mint-time receipt failed, and late results for early picks land as
   "imported" records (`side = null`).
3. **Not implemented (out of scope):** NEP-145 storage deposits & allowances,
   `ft_transfer_call` receiver hooks, multi-option events (binary only),
   fee-on-transfer edge for contract-op transfers (intentionally exempt).
4. **Soulbound passport:** `nft_transfer` always panics; one per account.
5. **Sybil limits, plainly:** streaks and pick counts are per-account, so
   splitting funds across accounts splits — but does not multiply — your
   records. The tier bonus is small (1–3%) and capped by actual treasury, so
   self-dealing a bonus costs more in tax than it returns. Pot shares scale
   with stake, which punishes sybil splitting rather than rewarding it.
6. **Storage:** the in-contract history log is capped (2,000 entries, oldest
   drained) — fine at demo scale, swap to an indexer for production.

## Parameters (frozen at deploy)

| Param | Value | Meaning |
|---|---|---|
| `transfer_tax_bps` | 100 | 1% on user→user transfers |
| `tax_burn_bps` | 5000 | half the tax burns |
| `event_bonus_bps` | 200 | 2% of free treasury funds each winner pool |
| `loser_burn_bps` | 2000 | 20% of losing stakes burn |
| `trader_bonus_bps` / `oracle_bonus_bps` | 100 / 300 | claim bonus with passport |
| `trader_wins` / `oracle_wins` | 5 / 10 | tier thresholds |
| `claim_window_secs` | 90 days | after which unclaimed pools sweep to treasury |

## Build & test

```bash
cargo check --target wasm32-unknown-unknown   # error-check (near-sdk 5 pattern)
cargo test                                     # 8 unit tests, invariants included
cargo build --target wasm32-unknown-unknown --release
# artifacts: target/wasm32-unknown-unknown/release/{pick_coin,pick_passport}.wasm
# production builds should use `cargo near build` (deterministic, size-optimized)
```

## 60-day supply simulation

```bash
python3 simulation/simulate.py        # writes simulation/out/*.csv + prints a summary table
```

Three scenarios (viral / steady / dormant) simulate daily transfer volume,
tax burn, event cadence and win rates, printing supply and treasury over 60
days — run *before* locking parameters, as the track brief asks.

## Demo script (40s)

1. `ft_transfer` → tax splits live, supply ticks down, history log appends
2. `stake` both sides of an open event → implied odds move
3. `resolve` → loser burn fires, winner pool shows the treasury bonus
4. `claim` → payout + tier bonus, streak increments
5. Passport page: record + tier (`ROOKIE → TRADER → ORACLE`)
6. README chart: 60-day supply under three scenarios

## Repo requirements for submission

- [ ] Repository is public and this README matches actual behavior
- [ ] Demo posted on X tagging @NEARProtocol and #NEARHackathon
- [ ] Form submitted with both links before **14 Nov 2026 · 23:59 UTC**
- [ ] `HACKATHON.md` tips applied to the demo post

## Roadmap

- Price-feed resolver for crypto events (removes the last trusted party)
- `ft_transfer_call` + NEP-145 storage management
- Merkle batch claims for large pots
- Passport `sync()` that re-pulls missed records from the coin
