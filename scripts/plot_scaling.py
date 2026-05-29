#!/usr/bin/env python3
"""Render scaling-curve figures from scripts/bench_scaling.py output.

Reads scaling.tsv (stdlib csv) and writes scaling_curves.png. Plotting deps
(matplotlib, numpy) are kept out of the measurement driver so a plotting
failure can never lose measured data. Run under uv:

    uv run --with matplotlib --with numpy scripts/plot_scaling.py
"""
from __future__ import annotations

import argparse
import csv
import math
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
DEFAULT = REPO / "docs/dev/notes/assets/scaling/scaling.tsv"


def load(tsv: Path) -> list[dict]:
    with tsv.open() as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    for r in rows:
        for k in ("P", "A", "n_compartments", "n_transitions", "ir_bytes"):
            r[k] = int(r[k])
        for k in ("compile_s", "sim_s", "peak_rss_mb"):
            r[k] = float(r[k])
    return rows


def _slope(xs: list[float], ys: list[float]) -> float:
    lx, ly = np.log(xs), np.log(ys)
    return float(np.polyfit(lx, ly, 1)[0])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-i", "--tsv", type=Path, default=DEFAULT)
    ap.add_argument("-o", "--out", type=Path, default=DEFAULT.parent / "scaling_curves.png")
    args = ap.parse_args()
    rows = load(args.tsv)

    fig, axes = plt.subplots(2, 2, figsize=(13, 10))

    # ── Panel 1: IR size vs P — O(P^2) coupling-on vs O(P) off (A=1, minimal) ──
    ax = axes[0, 0]
    for coup, color in (("on", "C3"), ("off", "C0")):
        pts = sorted([r for r in rows if r["slice"] == "slope" and r["coupling"] == coup],
                     key=lambda r: r["P"])
        P = [r["P"] for r in pts]
        B = [r["ir_bytes"] / 1e6 for r in pts]
        s = _slope([float(p) for p in P], B)
        ax.loglog(P, B, "o-", color=color, label=f"coupling={coup} (slope≈{s:.2f})")
    ax.set_xlabel("P (patches), A=1, grad=minimal")
    ax.set_ylabel("IR size (MB)")
    ax.set_title("① IR size: O(P²) spatial sum vs O(P) control")
    ax.grid(True, which="both", alpha=0.3)
    ax.legend()

    # ── Panel 2: realism A=21 — grad multiplier + super-linear growth ──
    ax = axes[0, 1]
    for grad, color in (("minimal", "C2"), ("full", "C1")):
        pts = sorted([r for r in rows if r["slice"] == "realism" and r["grad"] == grad],
                     key=lambda r: r["P"])
        P = [r["P"] for r in pts]
        B = [r["ir_bytes"] / 1e6 for r in pts]
        ax.loglog(P, B, "o-", color=color, label=f"grad={grad}")
    ax.set_xlabel("P (patches), A=21, coupling=on")
    ax.set_ylabel("IR size (MB)")
    ax.set_title("② rate_grad ≈ 5× multiplier (A=21 realism slice)")
    ax.axhline(2060, ls=":", color="k", alpha=0.5)
    ax.annotate("real-model regime\n(P=44,A=21,full): 2.06 GB",
                xy=(44, 2060), xytext=(6, 2300), fontsize=8,
                arrowprops=dict(arrowstyle="->", alpha=0.6))
    ax.grid(True, which="both", alpha=0.3)
    ax.legend()

    # ── Panel 3: forward-sim wall time is PARSE-bound (sim_s vs IR bytes) ──
    ax = axes[1, 0]
    B = [r["ir_bytes"] / 1e6 for r in rows]
    S = [r["sim_s"] for r in rows]
    ax.scatter(B, S, c=["C1" if r["grad"] == "full" else "C2" for r in rows],
               alpha=0.7, s=30)
    # fit a line through the non-trivial points
    big = [(r["ir_bytes"] / 1e6, r["sim_s"]) for r in rows if r["sim_s"] > 0.05]
    if len(big) > 2:
        bx, by = zip(*big)
        k = np.polyfit(bx, by, 1)[0]
        xs = np.array([min(bx), max(bx)])
        ax.plot(xs, k * xs, "k--", alpha=0.5, label=f"sim_s ≈ {k*1000:.1f} ms/MB · IR")
    ax.set_xlabel("IR size (MB)")
    ax.set_ylabel("forward simulate wall time (s, 365 steps)")
    ax.set_title("③ Forward-sim time ∝ IR bytes → PARSE-bound, not compute")
    ax.scatter([], [], c="C2", label="grad=minimal")
    ax.scatter([], [], c="C1", label="grad=full")
    ax.grid(True, alpha=0.3)
    ax.legend()

    # ── Panel 4: peak RSS ≈ const × IR bytes (H4) ──
    ax = axes[1, 1]
    B = np.array([r["ir_bytes"] / 1e6 for r in rows])
    R = np.array([r["peak_rss_mb"] for r in rows])
    ax.scatter(B, R, alpha=0.7, s=30, color="C4")
    # Fit the ratio on the GB-scale points where the ~10 MB binary baseline is
    # negligible (small-IR points are dominated by that fixed baseline).
    mask = B > 100
    ratio = float(np.median(R[mask] / B[mask]))
    xs = np.array([B[B > 1].min(), B.max()])
    ax.plot(xs, ratio * xs, "k--", alpha=0.5, label=f"RSS ≈ {ratio:.1f}× IR (IR>100 MB)")
    ax.set_xlabel("IR size (MB)")
    ax.set_ylabel("peak RSS (MB)")
    ax.set_title(f"④ Memory ≈ {ratio:.1f}× IR bytes (boxed Expr trees)")
    ax.loglog()
    ax.grid(True, which="both", alpha=0.3)
    ax.legend()

    fig.suptitle("camdl FOI scaling: the flat-inlined spatial sum drives an O(P²) "
                 "IR blowup (toy SEIR, chain_binomial)", fontsize=12)
    fig.tight_layout(rect=(0, 0, 1, 0.98))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(args.out, dpi=130)
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
