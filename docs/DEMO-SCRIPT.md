# $PICK — demo video script (≈50s) + voiceover

Target: one continuous screen recording, no cuts needed, < 60s.
Voiceover: ~125 words ≈ 48 seconds at conversational pace.

---

## Voiceover (read this)

**[0:00–0:06] — hook**
> Here is a memecoin that actually does something.

**[0:06–0:18] — the mechanic nobody else has**
> PICK is a NEP-141 token, but one percent of every transfer funds prediction
> pools *inside the token contract*. Holders stake on real events, winners
> split the pot plus two percent of the treasury, and twenty percent of
> losing stakes burn — so supply reacts to the community instead of just
> trading.

**[0:18–0:30] — the NFT that changes the maths**
> This soulbound Prediction Passport is not a JPEG. It records every pick
> on chain, and its tier changes how much the contract actually pays you
> on claim.

**[0:30–0:40] — live proof**
> And this is live on testnet, not a mock-up: mint, stake, resolve, claim —
> two hundred and sixty thousand PICK paid out, exactly matching the
> published formula.

**[0:40–0:50] — honesty + close**
> Parameters frozen at deploy with no admin key, a reproducible sixty-day
> supply simulation, and the sybil limits stated plainly. Built at NEAR
> Hackathon.

---

## Unique features to hit on screen (in this order)

| # | Feature | Where to show it |
|---|---|---|
| 1 | **Transfer tax → prediction pools inside the token** (memecoin + prediction market in one NEP-141) | Make a `ft_transfer`, then `get_history` shows the tax row and `FREE TREASURY` tick up |
| 2 | **Reactive supply**: half the tax burns + 20% of losing stakes burn | `TOTAL SUPPLY` decreases on the transfer; the 60-day chart |
| 3 | **Parimutuel payout with treasury bonus** | Event 4: 200,000 vs 50,000 PICK → resolved → `winner_pool` includes the 2% bonus |
| 4 | **Soulbound NEP-171 Passport that changes payout maths** | "My Passport": `get_records` + `get_summary`, tier `ROOKIE → TRADER → ORACLE` |
| 5 | **Streaks, leaderboard, public mint/burn log** | Frontend: "Streak leaderboard" + "Onchain log" sections |
| 6 | **No admin key — params frozen** | `get_params` view call returning the frozen table |
| 7 | **No backend** | Two contracts only; mirrors are fire-and-forget with `repair()` |
| 8 | **Honest sybil limits + 60-day sim** | `simulation/out/supply.svg` chart, then the README "Trust model" section |

## Suggested shot order (~50s)

1. **5s** — hosted frontend loads, `DEMO · read-only` badge visible
2. **8s** — `ft_transfer` between two wallets → tax row appears, supply ticks down
3. **8s** — an open event, odds bar moving as stakes land on both sides
4. **8s** — `resolve` → loser burn + treasury bonus shown in the winner pool
5. **8s** — `claim` → payout, then **My Passport** appends the `WIN +payout` row, streak increments
6. **8s** — `get_params` (frozen, no admin) and the 60-day supply chart
7. **5s** — README title + repo, "Built at NEAR Hackathon"

## Repo evidence to flash at the end

- `15/15` tests green
- live addresses `pick-coin.testnet` / `pick-passport.testnet`
- settled event 4 → `260,009.98 PICK` payout, reproducible with two `view` calls

## X post requirements

- must say **built at NEAR HACKATHON**
- tag **@NEARProtocol** and **#NEARHackathon**
- attach video and/or `docs/demo-top.png` · `docs/demo-passport.png`
- link the repo https://github.com/Avimain/pick-treasury
- its URL becomes `demoUrl` in the payload
