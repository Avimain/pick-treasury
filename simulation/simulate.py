#!/usr/bin/env python3
"""60-day supply simulation for $PICK (mirrors the contract formulas).

Run before locking parameters, as the hackathon track brief asks:
    python3 simulation/simulate.py

Only the two formulas below change supply:
    tax_burn   = volume * transfer_tax_bps/1e4 * tax_burn_bps/1e4
    loser_burn = losing_side * loser_burn_bps/1e4
The treasury bonus is a transfer (free treasury -> winner pool), so it moves
tokens but never changes supply. Writes simulation/out/<scenario>.csv and
prints a summary table. If matplotlib is installed, also writes supply.png.
"""

import csv
import os
import random

# --- frozen params (must match the contract) ---------------------------------
TRANSFER_TAX_BPS = 100        # 1%
TAX_BURN_BPS = 5_000          # half the tax burns
EVENT_BONUS_BPS = 200         # 2% of free treasury funds each pool
LOSER_BURN_BPS = 2_000        # 20% of losing stakes burn

# --- simulation setup --------------------------------------------------------
DAYS = 60
START_SUPPLY = 1_000_000_000  # tokens
SEED = 42

# scenario: (base daily transfer volume, growth to day 60, stake per event,
#            events per week)
SCENARIOS = {
    "viral":   dict(volume=5_000_000,  volume_end=40_000_000, stake=2_000_000, events_pw=7),
    "steady":  dict(volume=8_000_000,  volume_end=9_000_000,  stake=800_000,   events_pw=7),
    "dormant": dict(volume=500_000,    volume_end=100_000,    stake=200_000,   events_pw=2),
}


def run(name, cfg, rng):
    supply = float(START_SUPPLY)
    treasury = 0.0
    tax_burn_total = 0.0
    loser_burn_total = 0.0
    bonus_paid_total = 0.0
    rows = []

    for day in range(DAYS):
        progress = day / (DAYS - 1)
        drift = 1.0 + rng.uniform(-0.25, 0.25)

        # 1) transfer tax
        volume = (cfg["volume"] + (cfg["volume_end"] - cfg["volume"]) * progress) * drift
        tax = volume * TRANSFER_TAX_BPS / 10_000
        tax_burn = tax * TAX_BURN_BPS / 10_000
        treasury += tax - tax_burn
        supply -= tax_burn
        tax_burn_total += tax_burn

        # 2) prediction events (1 per configured cadence)
        if day % max(1, round(7 / cfg["events_pw"])) == 0:
            staked = cfg["stake"] * (0.7 + 0.6 * progress) * drift
            loser_share = rng.uniform(0.3, 0.7)          # balance of the two sides
            loser_stakes = staked * loser_share
            loser_burn = loser_stakes * LOSER_BURN_BPS / 10_000
            supply -= loser_burn
            loser_burn_total += loser_burn

            bonus = treasury * EVENT_BONUS_BPS / 10_000   # moved out, not burned
            treasury -= bonus
            bonus_paid_total += bonus

        rows.append((day + 1, round(supply), round(treasury),
                     round(tax_burn_total), round(loser_burn_total)))

    return rows


def main():
    os.makedirs(os.path.join(os.path.dirname(__file__), "out"), exist_ok=True)
    all_series = {}
    print(f"{'scenario':<9} {'start':>14} {'day60 supply':>14} {'change':>8} "
          f"{'tax burns':>12} {'loser burns':>12} {'treasury':>12}")
    print("-" * 84)
    for name, cfg in SCENARIOS.items():
        rng = random.Random(SEED)
        rows = run(name, cfg, rng)
        all_series[name] = rows
        path = os.path.join(os.path.dirname(__file__), "out", f"{name}.csv")
        with open(path, "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["day", "supply", "treasury", "tax_burn_cum", "loser_burn_cum"])
            w.writerows(rows)
        end = rows[-1]
        change = (end[1] - START_SUPPLY) / START_SUPPLY * 100
        print(f"{name:<9} {START_SUPPLY:>14,} {end[1]:>14,} {change:>7.2f}% "
              f"{end[3]:>12,} {end[4]:>12,} {end[2]:>12,}")

    print(f"\ncsv files written to {os.path.join(os.path.dirname(__file__), 'out')}/")

    try:
        import matplotlib.pyplot as plt
        for name, rows in all_series.items():
            plt.plot([r[0] for r in rows], [r[1] for r in rows], label=name)
        plt.axhline(START_SUPPLY, color="grey", ls="--", lw=0.8, label="initial supply")
        plt.xlabel("day")
        plt.ylabel("supply (PICK)")
        plt.title("$PICK supply over 60 days — three scenarios")
        plt.legend()
        out = os.path.join(os.path.dirname(__file__), "out", "supply.png")
        plt.savefig(out, dpi=140)
        print(f"chart written to {out}")
    except ImportError:
        print("(matplotlib not installed — chart skipped, csv data is complete)")


if __name__ == "__main__":
    main()
