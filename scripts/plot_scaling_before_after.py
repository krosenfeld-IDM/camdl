#!/usr/bin/env python3
"""Fix-B before/after: shared-binding extraction shrinks the FOI IR.

Overlays the pre-Fix-B macro sweep (`scaling.tsv`, flat-inlined `let N[l]`,
`let I_agg[l]`) against the post-Fix-B sweep (`scaling_after.tsv`, the same
lets hoisted into `model.bindings` + `BindingRef`). Same toy models, same
machine — only the compiler differs.

    uv run --with matplotlib --with numpy scripts/plot_scaling_before_after.py

Reads both TSVs (identical schema, from scripts/bench_scaling.py) and writes
fix_b_before_after.png. Plotting deps kept out of the measurement driver.
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
ASSETS = REPO / "docs/dev/notes/assets/scaling"


def load(tsv: Path) -> list[dict]:
    with tsv.open() as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    for r in rows:
        for k in ("P", "A", "n_compartments", "n_transitions", "ir_bytes"):
            r[k] = int(r[k])
        for k in ("compile_s", "sim_s", "peak_rss_mb"):
            r[k] = float(r[k])
    return rows


def pick(rows, slice_, coupling, grad):
    return sorted(
        [r for r in rows if r["slice"] == slice_ and r["coupling"] == coupling
         and r["grad"] == grad],
        key=lambda r: r["P"])


def _slope(xs, ys):
    if len(xs) < 2:
        return float("nan")
    return float(np.polyfit(np.log(xs), np.log(ys), 1)[0])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", type=Path, default=ASSETS / "scaling.tsv")
    ap.add_argument("--after", type=Path, default=ASSETS / "scaling_after.tsv")
    ap.add_argument("-o", "--out", type=Path, default=ASSETS / "fix_b_before_after.png")
    args = ap.parse_args()
    before, after = load(args.before), load(args.after)

    fig, axes = plt.subplots(1, 3, figsize=(17, 5.2))

    def overlay(ax, slice_, coupling, grad, field, scale, ylabel, title):
        for rows, lbl, color, mk in ((before, "before (inlined)", "C3", "o"),
                                     (after, "after (hoisted)", "C0", "s")):
            pts = pick(rows, slice_, coupling, grad)
            if not pts:
                continue
            P = [r["P"] for r in pts]
            Y = [r[field] / scale for r in pts]
            s = _slope([float(p) for p in P], Y)
            ax.loglog(P, Y, mk + "-", color=color, label=f"{lbl} (slope≈{s:.2f})")
        ax.set_xlabel("P (patches)")
        ax.set_ylabel(ylabel)
        ax.set_title(title)
        ax.grid(True, which="both", alpha=0.3)
        ax.legend(fontsize=8)

    # Panel 1: IR size, realism/full slice (A=21, coupling on) — the headline.
    overlay(axes[0], "realism", "on", "full", "ir_bytes", 1e6,
            "IR size (MB)", "① IR size, A=21, coupling=on, grad=full")
    # Annotate the Kano-scale anchor (P=44) reduction factor.
    b44 = pick(before, "realism", "on", "full")
    a44 = pick(after, "realism", "on", "full")
    b44 = next((r for r in b44 if r["P"] == 44), None)
    a44 = next((r for r in a44 if r["P"] == 44), None)
    if b44 and a44:
        fac = b44["ir_bytes"] / a44["ir_bytes"]
        axes[0].annotate(
            f"P=44 (Kano scale)\n{b44['ir_bytes']/1e6:.0f} → "
            f"{a44['ir_bytes']/1e6:.0f} MB  ({fac:.1f}×)",
            xy=(44, a44["ir_bytes"] / 1e6), xytext=(5, a44["ir_bytes"] / 1e6 * 1.4),
            fontsize=8, arrowprops=dict(arrowstyle="->", alpha=0.6))

    # Panel 2: peak RSS, same slice.
    overlay(axes[1], "realism", "on", "full", "peak_rss_mb", 1000.0,
            "peak RSS (GB)", "② peak RSS, A=21, coupling=on, grad=full")

    # Panel 3: forward-sim wall time, same slice (simulate parses the full IR).
    overlay(axes[2], "realism", "on", "full", "sim_s", 1.0,
            "forward simulate wall (s)", "③ simulate wall, A=21, coupling=on, grad=full")

    fig.suptitle("Fix B: hoisting the FOI aggregates (N[l], I_agg[l]) shrinks the "
                 "spatial-model IR — parse, memory, and forward-sim all track IR size",
                 fontsize=12)
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(args.out, dpi=130)
    print(f"wrote {args.out}")

    # Also print the anchor table to stdout for the lab note.
    print("\nP\tbefore_MB\tafter_MB\tIR×\tbefore_rss_GB\tafter_rss_GB\tbefore_sim_s\tafter_sim_s")
    for P in sorted({r["P"] for r in pick(before, "realism", "on", "full")}):
        b = next((r for r in pick(before, "realism", "on", "full") if r["P"] == P), None)
        a = next((r for r in pick(after, "realism", "on", "full") if r["P"] == P), None)
        if b and a:
            print(f"{P}\t{b['ir_bytes']/1e6:.1f}\t{a['ir_bytes']/1e6:.1f}\t"
                  f"{b['ir_bytes']/a['ir_bytes']:.1f}\t{b['peak_rss_mb']/1000:.2f}\t"
                  f"{a['peak_rss_mb']/1000:.2f}\t{b['sim_s']:.2f}\t{a['sim_s']:.2f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
