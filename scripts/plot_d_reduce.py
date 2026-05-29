#!/usr/bin/env python3
"""Plot Fix D (sum → n-ary Reduce node) before/after: IR size + the parse cliff.

Reads d_reduce_ir_cliff.tsv (A=1, coupling=on, grad=minimal; before = deep
left-nested BinOp(Add) chain, after = flat Reduce node) and renders a log-log
IR-size curve. The before-curve stops at P=44 — past ~50 patches the Add-chain
nests deep enough to trip serde_json's recursion limit (128) at IR-parse time,
so the model is *unparseable*. The Reduce node is depth-1, so the after-curve
continues. Run: uv run --with matplotlib --with numpy scripts/plot_d_reduce.py
"""
from __future__ import annotations

import argparse
import csv
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
DEFAULT = REPO / "docs/dev/notes/assets/scaling/d_reduce_ir_cliff.tsv"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-i", "--tsv", type=Path, default=DEFAULT)
    ap.add_argument("-o", "--out", type=Path, default=DEFAULT.parent / "d_reduce_ir_cliff.png")
    args = ap.parse_args()

    rows = list(csv.DictReader(args.tsv.open(), delimiter="\t"))
    P = np.array([int(r["P"]) for r in rows])
    after = np.array([float(r["after_bytes"]) / 1e6 for r in rows])
    before = np.array([float(r["before_bytes"]) / 1e6 if r["before_bytes"] else np.nan for r in rows])

    fig, ax = plt.subplots(figsize=(9, 6))
    bmask = ~np.isnan(before)
    ax.loglog(P[bmask], before[bmask], "o-", color="C3",
              label="before — deep BinOp(Add) chain")
    ax.loglog(P, after, "o-", color="C2", label="after — flat Reduce node")
    # ratio annotations on the overlap
    for p, b, a in zip(P[bmask], before[bmask], after[bmask]):
        ax.annotate(f"{b/a:.1f}×", xy=(p, a), xytext=(0, -15), textcoords="offset points",
                    ha="center", fontsize=8, color="C2")
    # cliff band
    ax.axvspan(50, P.max() * 1.1, color="red", alpha=0.06)
    ax.axvline(50, ls=":", color="C3", alpha=0.7)
    ax.annotate("parse cliff →\nAdd-chain hits serde\nrecursion limit (128)\n→ unparseable",
                xy=(56, before[bmask].max()), fontsize=8, color="C3", va="top")
    # mark the points that only exist after
    amask = np.isnan(before)
    ax.scatter(P[amask], after[amask], facecolors="none", edgecolors="C2", s=140,
               linewidths=1.5, zorder=5, label="now parses (was a hard failure)")

    ax.set_xlabel("P (patches), A=1, coupling=on")
    ax.set_ylabel("IR size (MB)")
    ax.set_title("Fix D — sum → flat Reduce node\nconstant-factor IR shrink (→2.3× at P=44) "
                 "+ parse cliff removed (P>50 now parses)")
    ax.grid(True, which="both", alpha=0.3)
    ax.legend(loc="upper left")
    fig.tight_layout()
    fig.savefig(args.out, dpi=130)
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
