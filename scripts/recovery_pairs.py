# /// script
# requires-python = ">=3.11"
# dependencies = ["pandas", "matplotlib", "seaborn"]
# ///
"""Pair (corner) plot for parameter-recovery diagnostics.

Reads a TSV whose rows are parameter estimates — one per chain (across-chain
view) or one per synthetic dataset (multi-seed recovery view) or posterior
draws — and renders a seaborn pair plot, optionally overlaying the planted
truth as reference lines / a marker. A one-off diagnostic helper: not wired
into the build, run it with uv.

    uv run scripts/recovery_pairs.py estimates.tsv \
        --cols beta gamma --truth beta=0.4 gamma=0.15 -o pairs.png

The input is whitespace- or tab-delimited with a header row containing the
column names referenced by --cols / --truth. Extra columns (seed, ll, …) are
ignored unless selected.
"""
import argparse
import sys

import pandas as pd
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import seaborn as sns


def parse_truth(items: list[str]) -> dict[str, float]:
    out: dict[str, float] = {}
    for it in items or []:
        if "=" not in it:
            sys.exit(f"--truth entry must be NAME=VALUE, got {it!r}")
        name, val = it.split("=", 1)
        out[name.strip()] = float(val)
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("tsv", help="estimates table (TSV/whitespace, with header)")
    ap.add_argument("--cols", nargs="+", help="parameter columns to plot (default: all numeric)")
    ap.add_argument("--truth", nargs="*", default=[], help="NAME=VALUE planted-truth markers")
    ap.add_argument("--hue", default=None, help="optional column to colour points by")
    ap.add_argument("-o", "--out", default="pairs.png", help="output PNG")
    ap.add_argument("--title", default=None)
    args = ap.parse_args()

    df = pd.read_csv(args.tsv, sep=r"\s+", engine="python")
    truth = parse_truth(args.truth)
    cols = args.cols or [c for c in df.columns if pd.api.types.is_numeric_dtype(df[c]) and c != args.hue]
    missing = [c for c in cols if c not in df.columns]
    if missing:
        sys.exit(f"columns not in {args.tsv}: {missing}; have {list(df.columns)}")

    n = len(df)
    g = sns.PairGrid(df, vars=cols, hue=args.hue, corner=False, diag_sharey=False)
    g.map_diag(sns.histplot, kde=(n >= 8))
    g.map_offdiag(sns.scatterplot, s=60, alpha=0.8)
    if args.hue:
        g.add_legend()

    # Overlay planted truth: a line on each marginal, a star on each scatter.
    for i, yc in enumerate(cols):
        for j, xc in enumerate(cols):
            ax = g.axes[i][j]
            if i == j:
                if xc in truth:
                    ax.axvline(truth[xc], color="crimson", ls="--", lw=1.5)
            else:
                if xc in truth:
                    ax.axvline(truth[xc], color="crimson", ls="--", lw=1.0, alpha=0.7)
                if yc in truth:
                    ax.axhline(truth[yc], color="crimson", ls="--", lw=1.0, alpha=0.7)
                if xc in truth and yc in truth:
                    ax.plot(truth[xc], truth[yc], marker="*", ms=18, color="crimson",
                            mec="black", mew=0.5, zorder=5)

    # Annotate mean estimate vs truth on each marginal.
    for i, c in enumerate(cols):
        ax = g.axes[i][i]
        m = df[c].mean()
        sub = f"mean={m:.4g}" + (f"  truth={truth[c]:.4g}  ({(m/truth[c]-1)*100:+.0f}%)" if c in truth else "")
        ax.set_title(sub, fontsize=9)

    if args.title:
        g.figure.suptitle(args.title, y=1.02, fontsize=12)
    g.figure.tight_layout()
    g.figure.savefig(args.out, dpi=130, bbox_inches="tight")
    print(f"wrote {args.out}  ({n} rows, cols={cols}, truth-marked={'★' if truth else 'none'})")


if __name__ == "__main__":
    main()
