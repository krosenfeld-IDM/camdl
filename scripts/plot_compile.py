#!/usr/bin/env python3
"""Render compiler-scaling figures from scripts/bench_compile.py output.

Compile-side analogue of plot_scaling.py. Reads one or more compile TSVs and
draws four panels: compile wall-time and peak RSS as functions of patch count
(the realistic Kano shape, A=21, coupling on, full grad), and both vs emitted
IR size across the whole grid. Pass multiple labelled TSVs to overlay variants
(baseline vs flambda vs an optimization) on the same axes — the before/after
story the FOI study told for runtime, now for the compiler.

    # single baseline
    uv run --with matplotlib --with numpy scripts/plot_compile.py

    # overlay: before/after a change
    uv run --with matplotlib --with numpy scripts/plot_compile.py \
        --tsv baseline=docs/dev/notes/assets/compile/compile_baseline.tsv \
        --tsv flambda=docs/dev/notes/assets/compile/compile_flambda.tsv \
        -o docs/dev/notes/assets/compile/compile_flambda_before_after.png

Plotting deps (matplotlib, numpy) are kept out of the measurement driver so a
plotting failure can never lose measured data.
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
DEFAULT = REPO / "docs/dev/notes/assets/compile/compile_baseline.tsv"


def load(tsv: Path) -> list[dict]:
    with tsv.open() as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    for r in rows:
        r["ir_bytes"] = int(r["ir_bytes"])
        for k in ("wall_min_s", "wall_median_s", "peak_rss_mb"):
            r[k] = float(r[k])
        for k in ("P", "A", "n_compartments", "n_transitions"):
            r[k] = int(r[k]) if r[k] not in ("", None) else None
    return rows


def _slope(xs: list[float], ys: list[float]) -> float:
    lx, ly = np.log(xs), np.log(ys)
    return float(np.polyfit(lx, ly, 1)[0])


def patch_slice(rows: list[dict]) -> list[dict]:
    """The A=21, coupling=on, full-grad patch sweep, sorted by P."""
    return sorted(
        [r for r in rows if r["slice"] == "patch"], key=lambda r: r["P"])


PASSES = ("parse", "expand", "validate", "dimcheck", "autodiff", "serialize")


def load_passes(tsv: Path) -> list[dict]:
    with tsv.open() as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    for r in rows:
        r["ir_bytes"] = int(r["ir_bytes"])
        r["P"] = int(r["P"]) if r["P"] not in ("", None) else None
        for k in (*PASSES, "total_s"):
            r[k] = float(r[k])
    return rows


def plot_passes(passes_tsv: Path, out: Path) -> None:
    """Per-pass breakdown over the patch ladder: where compile time goes."""
    rows = [r for r in load_passes(passes_tsv) if r["slice"] == "patch"]
    rows.sort(key=lambda r: r["P"])
    if not rows:
        return
    P = [r["P"] for r in rows]
    fig, axes = plt.subplots(1, 3, figsize=(17, 5))

    # ① Stacked absolute pass time vs P — serialize ≈ the whole bar.
    ax = axes[0]
    bottom = np.zeros(len(rows))
    cmap = {"parse": "C7", "expand": "C0", "validate": "C5",
            "dimcheck": "C2", "autodiff": "C1", "serialize": "C3"}
    for p in PASSES:
        vals = np.array([r[p] for r in rows])
        ax.bar([str(x) for x in P], vals, bottom=bottom, label=p, color=cmap[p])
        bottom += vals
    ax.set_xlabel("P (patches), A=21, coupling=on, grad=full")
    ax.set_ylabel("processor seconds")
    ax.set_title("① Where compile time goes (stacked)")
    ax.legend(fontsize=8)

    # ② serialize as a fraction of total vs P — flat near 1.0.
    ax = axes[1]
    frac = [100 * r["serialize"] / r["total_s"] for r in rows]
    ax.plot(P, frac, "o-", color="C3")
    ax.set_ylim(0, 100)
    ax.set_xlabel("P (patches)")
    ax.set_ylabel("serialize share of compile (%)")
    ax.set_title("② Serialize dominates at every scale")
    ax.grid(True, alpha=0.3)

    # ③ The non-serialize passes alone (log-y), so their scaling is visible.
    ax = axes[2]
    for p in ("expand", "dimcheck", "autodiff"):
        ax.loglog(P, [max(r[p], 1e-4) for r in rows], "o-",
                  color=cmap[p], label=p)
    ax.set_xlabel("P (patches)")
    ax.set_ylabel("processor seconds")
    ax.set_title("③ Non-serialize passes (each ≈1% of total)")
    ax.grid(True, which="both", alpha=0.3)
    ax.legend(fontsize=8)

    fig.suptitle("camdlc per-pass profile over the patch ladder "
                 "(serialize = pretty-printing the IR JSON)", fontsize=12)
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=130)
    print(f"wrote {out}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tsv", action="append", default=[],
                    help="LABEL=path (repeatable). Bare path → label from stem.")
    ap.add_argument("-o", "--out", type=Path,
                    default=DEFAULT.parent / "compile_curves.png")
    args = ap.parse_args()

    specs = args.tsv or [str(DEFAULT)]
    series: list[tuple[str, list[dict]]] = []
    for spec in specs:
        if "=" in spec:
            label, path = spec.split("=", 1)
        else:
            label, path = Path(spec).stem.replace("compile_", ""), spec
        series.append((label, load(Path(path))))

    colors = ["C0", "C3", "C2", "C1", "C4"]
    markers = ["o", "s", "^", "D", "v"]
    fig, axes = plt.subplots(2, 2, figsize=(13, 10))

    # ── Panel 1: compile wall time vs P (patch slice) ──
    ax = axes[0, 0]
    for i, (label, rows) in enumerate(series):
        pts = patch_slice(rows)
        if not pts:
            continue
        P = [r["P"] for r in pts]
        W = [r["wall_min_s"] for r in pts]
        s = _slope([float(p) for p in P], W) if len(P) > 1 else float("nan")
        ax.loglog(P, W, markers[i % 5] + "-", color=colors[i % 5],
                  label=f"{label} (slope≈{s:.2f})")
    ax.set_xlabel("P (patches), A=21, coupling=on, grad=full")
    ax.set_ylabel("compile wall time (s, min of reps)")
    ax.set_title("① Compile time vs patches")
    ax.grid(True, which="both", alpha=0.3)
    ax.legend()

    # ── Panel 2: peak RSS vs P (patch slice) ──
    ax = axes[0, 1]
    for i, (label, rows) in enumerate(series):
        pts = patch_slice(rows)
        if not pts:
            continue
        P = [r["P"] for r in pts]
        R = [r["peak_rss_mb"] / 1000 for r in pts]
        ax.loglog(P, R, markers[i % 5] + "-", color=colors[i % 5], label=label)
    ax.set_xlabel("P (patches), A=21, coupling=on, grad=full")
    ax.set_ylabel("peak RSS (GB)")
    ax.set_title("② Compiler peak memory vs patches")
    ax.grid(True, which="both", alpha=0.3)
    ax.legend()

    # ── Panel 3: compile wall vs IR size (all points) ──
    ax = axes[1, 0]
    for i, (label, rows) in enumerate(series):
        B = np.array([r["ir_bytes"] / 1e6 for r in rows])
        W = np.array([r["wall_min_s"] for r in rows])
        ax.scatter(B, W, color=colors[i % 5], marker=markers[i % 5],
                   alpha=0.7, s=30, label=label)
        mask = B > 50
        if mask.sum() > 2:
            k = float(np.polyfit(B[mask], W[mask], 1)[0])
            xs = np.array([B[mask].min(), B[mask].max()])
            ax.plot(xs, k * xs + (W[mask] - k * B[mask]).mean(),
                    "--", color=colors[i % 5], alpha=0.5,
                    label=f"{label}: {k*1000:.1f} ms/MB·IR")
    ax.set_xlabel("emitted IR size (MB)")
    ax.set_ylabel("compile wall time (s)")
    ax.set_title("③ Compile time vs IR bytes (slope = serialize/alloc cost)")
    ax.grid(True, alpha=0.3)
    ax.legend(fontsize=8)

    # ── Panel 4: peak RSS vs IR size (all points) ──
    ax = axes[1, 1]
    for i, (label, rows) in enumerate(series):
        B = np.array([r["ir_bytes"] / 1e6 for r in rows])
        R = np.array([r["peak_rss_mb"] for r in rows])
        ax.scatter(B, R, color=colors[i % 5], marker=markers[i % 5],
                   alpha=0.7, s=30, label=label)
        mask = B > 50
        if mask.sum() > 0:
            ratio = float(np.median(R[mask] / B[mask]))
            xs = np.array([B[B > 1].min(), B.max()])
            ax.plot(xs, ratio * xs, "--", color=colors[i % 5], alpha=0.5,
                    label=f"{label}: RSS≈{ratio:.1f}×IR")
    ax.set_xlabel("emitted IR size (MB)")
    ax.set_ylabel("peak RSS (MB)")
    ax.set_title("④ Compiler memory ≈ const × IR bytes")
    ax.loglog()
    ax.grid(True, which="both", alpha=0.3)
    ax.legend(fontsize=8)

    title = ("camdlc compiler scaling (parse→expand→dimcheck→autodiff→serialize)"
             if len(series) == 1 else
             "camdlc compiler scaling — " + " vs ".join(l for l, _ in series))
    fig.suptitle(title, fontsize=12)
    fig.tight_layout(rect=(0, 0, 1, 0.98))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(args.out, dpi=130)
    print(f"wrote {args.out}")

    # If a per-pass companion TSV sits next to the first series, draw the
    # "where compile time goes" breakdown alongside the scaling curves.
    first_path = Path(specs[0].split("=", 1)[-1])
    passes_tsv = first_path.with_name(first_path.stem + "_passes.tsv")
    if passes_tsv.exists():
        plot_passes(passes_tsv, args.out.parent / "compile_passes.png")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
