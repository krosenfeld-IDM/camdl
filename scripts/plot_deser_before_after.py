#!/usr/bin/env python3
"""Plot model-load time before vs after Fix E (drop serde untagged buffering).

Reads deser_load_before_after.tsv (criterion `load_parse_compile` = ir::from_str
+ CompiledModel::new; before = derived `#[serde(untagged)]`, after = hand-written
single-pass Deserialize) and renders a log-log before/after curve with the
per-point speedup annotated. Run under uv:

    uv run --with matplotlib --with numpy scripts/plot_deser_before_after.py
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
DEFAULT = REPO / "docs/dev/notes/assets/scaling/deser_load_before_after.tsv"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-i", "--tsv", type=Path, default=DEFAULT)
    ap.add_argument("-o", "--out", type=Path, default=DEFAULT.parent / "deser_load_before_after.png")
    args = ap.parse_args()

    with args.tsv.open() as f:
        rows = sorted(csv.DictReader(f, delimiter="\t"), key=lambda r: float(r["ir_mb"]))
    labels = [f"{r['model'].replace('_on','')}\n({float(r['ir_mb']):.2g} MB)" for r in rows]
    before = np.array([float(r["before_us"]) for r in rows]) / 1000.0  # ms
    after = np.array([float(r["after_us"]) for r in rows]) / 1000.0    # ms
    speedup = before / after

    # Grouped bars: a continuous line would interpolate misleadingly across the
    # mixed (P, A) points — the win tracks tree depth (P), not total IR bytes.
    x = np.arange(len(rows))
    w = 0.4
    fig, ax = plt.subplots(figsize=(11, 6))
    ax.bar(x - w / 2, before, w, color="C3", label="before — #[serde(untagged)] (buffered)")
    ax.bar(x + w / 2, after, w, color="C2", label="after — single-pass Deserialize")
    for xi, b, a, s in zip(x, before, after, speedup):
        ax.annotate(f"{s:.1f}×", xy=(xi + w / 2, a), xytext=(0, 3),
                    textcoords="offset points", ha="center", fontsize=9, color="C2", weight="bold")
    ax.set_yscale("log")
    ax.set_xticks(x)
    ax.set_xticklabels(labels, fontsize=8)
    ax.set_ylabel("model load time: ir::from_str + CompiledModel::new (ms, log)")
    ax.set_title("Fix E — Expr deserialization, before vs after\n"
                 "(criterion load_parse_compile; bigger/deeper trees gain most)")
    ax.grid(True, axis="y", which="both", alpha=0.3)
    ax.legend()
    fig.tight_layout()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(args.out, dpi=130)
    print(f"wrote {args.out}  (speedup range {speedup.min():.1f}×–{speedup.max():.1f}×)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
