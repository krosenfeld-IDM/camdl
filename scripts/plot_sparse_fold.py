#!/usr/bin/env python3
"""Sparse-coupling constant-fold A/B: dense P-term FOI Reduce (fold off) vs
k-term Reduce (fold on), across patch count P at fixed neighbour degree k.

Panel A — IR size vs P (log-log): fold-off scales ~O(P^2) (dense W), fold-on
~O(P*k) (sparse), so the slope flips. Panel B — pfilter wall vs P: the runtime
inner-loop win (fewer Reduce terms per eval), speedup annotated. Both are
byte-identical (verified per P).

    uv run --with matplotlib --with numpy scripts/plot_sparse_fold.py
"""
from __future__ import annotations
import csv
from pathlib import Path
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
ASSETS = REPO / "docs/dev/notes/assets/sparse-fold"


def slope(xs, ys):
    return float(np.polyfit(np.log(xs), np.log(ys), 1)[0]) if len(xs) > 1 else float("nan")


def main():
    rows = list(csv.DictReader((ASSETS / "sparse_fold.tsv").open(), delimiter="\t"))
    P = [int(r["P"]) for r in rows]
    k = rows[0]["k"]
    ir_off = [int(r["ir_off"]) / 1e6 for r in rows]
    ir_on = [int(r["ir_on"]) / 1e6 for r in rows]
    pf_off = [float(r["pfilter_off"]) for r in rows]
    pf_on = [float(r["pfilter_on"]) for r in rows]
    allbit = all(r["bitexact"] == "1" for r in rows)

    fig, (axA, axB) = plt.subplots(1, 2, figsize=(13.5, 5.0))

    axA.plot(P, ir_off, "o-", color="#c44e52", label=f"fold off (dense, slope {slope(P,ir_off):.2f})")
    axA.plot(P, ir_on, "o-", color="#4c72b0", label=f"fold on  (sparse, slope {slope(P,ir_on):.2f})")
    axA.set_yscale("log")   # linear X (patches), log Y (size spans ~2 decades)
    axA.set_xlabel("patches P  (neighbour degree k=%s)" % k)
    axA.set_ylabel("IR size (MB)")
    axA.set_title("A. IR size: O(P²) dense → O(P·k) sparse")
    axA.legend(fontsize=8); axA.grid(True, which="both", alpha=0.3)
    axA.margins(x=0.13, y=0.18)   # padding so edge points + labels don't clip
    for x, a, b in zip(P, ir_off, ir_on):
        # label in the clear gap above the fold-on (blue) line, centred
        axA.annotate(f"{a/b:.0f}×", (x, b), textcoords="offset points", xytext=(0, 7),
                     ha="center", fontsize=8)

    axB.plot(P, pf_off, "o-", color="#c44e52", label=f"fold off (dense, slope {slope(P,pf_off):.2f})")
    axB.plot(P, pf_on, "o-", color="#4c72b0", label=f"fold on  (sparse, slope {slope(P,pf_on):.2f})")
    axB.set_yscale("log")   # linear X (patches), log Y
    axB.set_xlabel("patches P")
    axB.set_ylabel("pfilter wall (s, median)")
    axB.set_title("B. Inference runtime (pfilter)")
    axB.legend(fontsize=8); axB.grid(True, which="both", alpha=0.3)
    axB.margins(x=0.13, y=0.18)   # padding so edge points + labels don't clip
    for x, a, b in zip(P, pf_off, pf_on):
        # label in the clear gap above the fold-on (blue) line, centred
        axB.annotate(f"{a/b:.1f}×", (x, b), textcoords="offset points", xytext=(0, 7),
                     ha="center", fontsize=8)

    tag = "byte-identical (verified per P)" if allbit else "WARNING: trajectory drift!"
    fig.suptitle(f"Sparse-coupling constant-fold (CAMDL_CONSTANT_FOLD) — {tag}", fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    out = ASSETS / "sparse_fold_before_after.png"
    fig.savefig(out, dpi=140)
    print("wrote", out)
    print("IR shrink:", [f"{a/b:.0f}x" for a, b in zip(ir_off, ir_on)])
    print("pfilter speedup:", [f"{a/b:.1f}x" for a, b in zip(pf_off, pf_on)])


if __name__ == "__main__":
    raise SystemExit(main())
