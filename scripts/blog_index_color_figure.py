#!/usr/bin/env python3
"""Generate the index-color figure for the camdl blog post.

Side-by-side rendering of the age-stratified force-of-infection
transition in two notations:

  Math (top, LaTeX-rendered): infection_a: S_a -> E_a @ ... sum_b ...
  camdl (bottom, monospace):  infection[a in age] : S[a] --> E[a] @ ...

The TARGET-AGE index `a` is colored RED in both rows.
The SOURCE-AGE index `b` is colored BLUE in both rows.
Everything else is black/dark gray.

This visually demonstrates the camdl tagline that the DSL syntax
mirrors the math you'd write on a whiteboard: every red `a` in the
math has a red `a` in the same role in the camdl.

Run with:
    uv run --with matplotlib scripts/blog_index_color_figure.py

Requires a system LaTeX install (pdflatex) for usetex coloring of
math indices via \\textcolor. Falls back to mathtext (no per-symbol
coloring on the math line) if LaTeX is unavailable.
"""
from __future__ import annotations

import shutil
from pathlib import Path

import matplotlib

A_COLOR = "#d62728"  # vermilion red — target age (`a`)
B_COLOR = "#1f77b4"  # steel blue   — source age (`b`)
SET_COLOR = "#2ca02c"  # forest green — the index set (`age`)
INK = "#222222"

USETEX = shutil.which("pdflatex") is not None

if USETEX:
    # PGF backend (vs the default Agg + dvipng path) is required for inline
    # \textcolor in math mode — Agg's dvi-to-png converter drops color info.
    matplotlib.use("pgf")
    matplotlib.rcParams.update({
        "pgf.texsystem": "pdflatex",
        "pgf.rcfonts": False,
        "text.usetex": True,
        "pgf.preamble": r"\usepackage{xcolor}\usepackage{amsmath}",
    })

import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.offsetbox import (  # noqa: E402
    AnchoredOffsetbox, HPacker, TextArea, VPacker,
)

A = rf"\textcolor[HTML]{{{A_COLOR[1:]}}}{{a}}"
B = rf"\textcolor[HTML]{{{B_COLOR[1:]}}}{{b}}"
AGE = rf"\textcolor[HTML]{{{SET_COLOR[1:]}}}{{\mathrm{{age}}}}"

# Math: standard split-FOI form (Anderson & May / Keeling & Rohani).
# Headline ODE first, then the FOI definition.
MATH_ODE = (
    rf"$\dfrac{{dS_{{{A}}}}}{{dt}} \;=\; "
    rf"-\lambda_{{{A}}}\, S_{{{A}}}$"
)
MATH_FOI = (
    rf"$\lambda_{{{A}}} \;=\; \beta \sum_{{{B}\, \in\, {AGE}}} "
    rf"C_{{{A}{B}}}\, \dfrac{{I_{{{B}}}}}{{N_{{{B}}}}}$"
)

# camdl as it actually appears in the post: one rule, broken at the `:`
# with the rate continuation indented under it.
CAMDL_LINES = [
    [
        ("infection[", INK),
        ("a", A_COLOR),
        (" in ", INK),
        ("age", SET_COLOR),
        ("] : S[", INK),
        ("a", A_COLOR),
        ("] --> E[", INK),
        ("a", A_COLOR),
        ("]", INK),
    ],
    [
        ("    @ beta * S[", INK),
        ("a", A_COLOR),
        ("] * sum(", INK),
        ("b", B_COLOR),
        (" in ", INK),
        ("age", SET_COLOR),
        (", C_age[", INK),
        ("a", A_COLOR),
        (", ", INK),
        ("b", B_COLOR),
        ("] * I[", INK),
        ("b", B_COLOR),
        ("] / N_local[", INK),
        ("b", B_COLOR),
        ("])", INK),
    ],
]


def _line_box(segments, fontsize):
    return HPacker(
        children=[
            TextArea(
                text,
                textprops=dict(
                    color=color, fontsize=fontsize,
                    family="monospace", usetex=False,
                ),
            )
            for text, color in segments
        ],
        align="baseline", pad=0, sep=0,
    )


def render_camdl_block(ax, y, lines, fontsize=14):
    """Render a multi-line camdl block, left-aligned, centered around y.
    usetex is OFF for these so symbols like `-->` and `_` render literally.
    """
    block = VPacker(
        children=[_line_box(line, fontsize) for line in lines],
        align="left", pad=0, sep=4,
    )
    anchored = AnchoredOffsetbox(
        loc="center", child=block, pad=0, frameon=False,
        bbox_to_anchor=(0.5, y), bbox_transform=ax.transAxes,
    )
    ax.add_artist(anchored)


def render_legend(ax, y, fontsize=11):
    """Three small swatches explaining the color-to-role mapping."""
    items = [
        ("a",   A_COLOR,   "target age (compartment receiving infection)"),
        ("b",   B_COLOR,   "source age (summed over)"),
        ("age", SET_COLOR, "the age dimension (the set both indices live in)"),
    ]
    row_step = 0.07
    for i, (sym, color, desc) in enumerate(items):
        yi = y - i * row_step
        ax.text(0.30, yi, sym, color=color, fontsize=fontsize + 2,
                family="monospace", weight="bold", usetex=False,
                transform=ax.transAxes, va="center", ha="right")
        ax.text(0.32, yi, f" = {desc}", color=INK, fontsize=fontsize,
                usetex=False,
                transform=ax.transAxes, va="center", ha="left")


def main(out_path: Path) -> Path:
    fig, ax = plt.subplots(figsize=(11, 4.8), dpi=200)
    ax.set_axis_off()
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)

    # Math rows: ODE first (the headline), then the FOI definition.
    ax.text(0.5, 0.88, MATH_ODE, fontsize=18, color=INK,
            transform=ax.transAxes, ha="center", va="center")
    ax.text(0.5, 0.68, MATH_FOI, fontsize=18, color=INK,
            transform=ax.transAxes, ha="center", va="center")

    # camdl block (2 lines, left-aligned within the centered block)
    render_camdl_block(ax, 0.40, CAMDL_LINES, fontsize=14)

    # Color legend (3 rows now, tighter spacing)
    render_legend(ax, 0.18)

    fig.savefig(out_path, bbox_inches="tight", facecolor="white", dpi=200)
    plt.close(fig)
    return out_path


if __name__ == "__main__":
    repo_root = Path(__file__).resolve().parent.parent
    out = repo_root / "docs" / "assets" / "math-vs-dsl.png"
    out.parent.mkdir(parents=True, exist_ok=True)
    written = main(out)
    print(f"wrote {written}  (usetex={USETEX})")
