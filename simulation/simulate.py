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

# display order + colors for the chart (matches README table order)
CHART_STYLE = {
    "viral":   ("#f85149", "viral  (40M peak volume)"),
    "steady":  ("#3fb950", "steady (9M volume)"),
    "dormant": ("#58a6ff", "dormant (100k volume)"),
}


def write_svg_chart(all_series, start_supply, width=760, height=420):
    """Render the 60-day supply chart as SVG using only the stdlib.

    Returns the output path. Kept dependency-free so `python3
    simulation/simulate.py` always produces a chart judges can open.
    """
    import xml.etree.ElementTree as ET

    pad_l, pad_r, pad_t, pad_b = 78, 18, 46, 44
    plot_w, plot_h = width - pad_l - pad_r, height - pad_t - pad_b

    # y-range across every scenario, with the initial supply included
    y_min = min(min(r[1] for r in rows) for rows in all_series.values())
    y_max = max(start_supply, max(max(r[1] for r in rows) for rows in all_series.values()))
    span = (y_max - y_min) or 1.0
    y_min -= span * 0.06
    y_max += span * 0.06
    span = y_max - y_min

    days = len(next(iter(all_series.values())))
    x = lambda d: pad_l + plot_w * (d / (days - 1))
    y = lambda v: pad_t + plot_h * (1 - (v - y_min) / span)

    svg = ET.Element("svg", {
        "xmlns": "http://www.w3.org/2000/svg",
        "width": str(width), "height": str(height),
        "viewBox": f"0 0 {width} {height}",
        "font-family": "ui-monospace, SFMono-Regular, Menlo, monospace",
    })
    ET.SubElement(svg, "rect", {"width": str(width), "height": str(height),
                                "fill": "#0d1117"})

    def label(px, py, text, size="11", fill="#8b949e", anchor="start", weight="normal"):
        t = ET.SubElement(svg, "text", {"x": f"{px:.1f}", "y": f"{py:.1f}",
                                        "font-size": size, "fill": fill,
                                        "text-anchor": anchor, "font-weight": weight})
        t.text = text

    # title + legend
    label(pad_l, 22, "$PICK supply over 60 days — three scenarios", "13", "#c9d1d9", "start", "bold")
    lx = pad_l
    for name, (color, desc) in CHART_STYLE.items():
        if name in all_series:
            ET.SubElement(svg, "rect", {"x": f"{lx}", "y": "30", "width": "14",
                                        "height": "4", "fill": color})
            label(lx + 19, 37, desc, "10")
            lx += 19 + 7.0 * len(desc) + 18

    # gridlines + y-axis ticks (5)
    for i in range(5):
        v = y_min + span * i / 4
        yy = y(v)
        ET.SubElement(svg, "line", {
            "x1": str(pad_l), "x2": str(width - pad_r),
            "y1": f"{yy:.1f}", "y2": f"{yy:.1f}",
            "stroke": "#21262d", "stroke-width": "1",
        })
        label(pad_l - 8, yy + 4, f"{v/1e6:.0f}M" if v >= 1e6 else f"{v/1e3:.0f}k",
              "10", anchor="end")

    # x-axis ticks every 10 days
    for d in range(0, days, 10):
        xx = x(d)
        ET.SubElement(svg, "line", {
            "x1": f"{xx:.1f}", "x2": f"{xx:.1f}",
            "y1": str(pad_t), "y2": str(height - pad_b),
            "stroke": "#161b22", "stroke-width": "1",
        })
        label(xx, height - pad_b + 18, f"d{d}", "10", anchor="middle")
    label(width - pad_r, height - 10, "day →", "10", anchor="end")

    # initial-supply reference line
    y0 = y(start_supply)
    ET.SubElement(svg, "line", {
        "x1": str(pad_l), "x2": str(width - pad_r),
        "y1": f"{y0:.1f}", "y2": f"{y0:.1f}",
        "stroke": "#8b949e", "stroke-width": "1", "stroke-dasharray": "5 4",
    })

    # scenario polylines
    for name, rows in all_series.items():
        color = CHART_STYLE.get(name, ("#d29922", name))[0]
        pts = " ".join(f"{x(r[0]):.1f},{y(r[1]):.1f}" for r in rows)
        ET.SubElement(svg, "polyline", {
            "points": pts, "fill": "none", "stroke": color,
            "stroke-width": "2", "stroke-linejoin": "round",
        })
        # end-of-series label with day-60 value
        end = rows[-1]
        label(x(end[0]) - 6, y(end[1]) - 7, f"{end[1]/1e6:.1f}M", "10", color, "end", "bold")

    out = os.path.join(os.path.dirname(__file__), "out", "supply.svg")
    ET.ElementTree(svg).write(out, encoding="unicode", xml_declaration=True)
    return out


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

    # --- SVG chart (stdlib only, always runs) -----------------------------
    svg_path = write_svg_chart(all_series, START_SUPPLY)
    print(f"chart written to {svg_path}")

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
        print("(matplotlib not installed — using the svg chart above)")


if __name__ == "__main__":
    main()
