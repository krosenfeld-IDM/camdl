#!/usr/bin/env python3
"""Issue-triage dashboard generator.

Reads the *live* GitHub backlog via `gh` and renders a single self-contained
`index.html` (no external assets, no JS deps) that visualizes the label
taxonomy from ../issue-labels.md: the kind x area heat map, the blocker
critical path, the upstream-audit cohort, the needs-RFC list, and the
s-class/effort batch queue. Every issue is a link.

Stdlib only. Requires `gh` on PATH, authenticated.

    python3 build.py                 # write index.html
    python3 build.py --serve         # build, then serve (regenerates per load)
    python3 build.py --serve --port 8799

To refresh a static build, just re-run it; under --serve every page load
re-fetches from gh, so the dashboard is always current.
"""

from __future__ import annotations

import argparse
import functools
import html
import json
import subprocess
import sys
import urllib.parse
from datetime import datetime, timedelta, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "index.html"
SNAP = HERE / "snapshots.ndjson"  # forward-accruing metric log (gitignored)

# Trend windows in days. 7d = week-over-week is the "are we winning" signal;
# 1d catches the bursty triage days; 3d smooths. Swap 7 -> 5 for a work-week.
WINDOWS = [1, 3, 7]

# Selectable momentum-chart windows: (days back, pill label). The longest sets
# how far back we fetch; DEFAULT_DAYS is the range shown on first load.
RANGES = [(7, "1w"), (14, "2w"), (30, "1m")]
DEFAULT_DAYS = 14
MAX_DAYS = max(d for d, _ in RANGES)

# Canonical display order (axes from ../issue-labels.md).
KIND_ORDER = [
    "kind/bug", "kind/feature", "kind/refactor",
    "kind/design", "kind/docs", "kind/question",
]
AREA_ORDER = [
    "area/compiler", "area/engine", "area/inference", "area/cli",
    "area/obs-model", "area/ir-schema", "area/testing",
]

# The main dev. Issues authored by anyone else get an "@handle" external
# badge — a dashboard-only marker (no GitHub label involved).
MAINTAINER = "vsbuffalo"


def sh(args: list[str]) -> str:
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"`{' '.join(args)}` failed:\n{r.stderr}")
    return r.stdout


def repo_slug() -> str:
    return sh(["gh", "repo", "view", "--json", "nameWithOwner",
               "-q", ".nameWithOwner"]).strip()


def fetch_issues() -> list[dict]:
    raw = sh(["gh", "issue", "list", "--state", "open", "--limit", "500",
              "--json", "number,title,labels,url,author"])
    out = []
    for it in json.loads(raw):
        names = [lbl["name"] for lbl in it["labels"]]
        author = (it.get("author") or {}).get("login") or "ghost"
        out.append({
            "n": it["number"],
            "t": it["title"],
            "url": it["url"],
            "author": author,
            "external": author != MAINTAINER,
            "kind": next((n for n in names if n.startswith("kind/")), None),
            "areas": [n for n in names if n.startswith("area/")],
            "effort": next((n for n in names if n.startswith("effort/")), None),
            "status": next((n for n in names if n.startswith("status/")), None),
            "blocker": "blocker" in names,
            "audit": "upstream-audit" in names,
        })
    return sorted(out, key=lambda x: x["n"])


def parse_ts(s: str | None):
    return datetime.fromisoformat(s.replace("Z", "+00:00")) if s else None


def fetch_flow(now: datetime) -> tuple[dict[int, dict], list[tuple[str, int]]]:
    """Exact open-count momentum from issue timestamps (backfilled, real).

    Two bounded calls (created / closed within the widest span we chart + a day
    of slack), then bucketed locally with full-precision timestamps:
      - windowed net per WINDOWS: net = opened - closed = change in open count.
      - per-day net backlog reduction (closed - opened) for the last MAX_DAYS,
        oldest -> newest, for the momentum charts (>0 progress, <0 grew).
    """
    span = max(max(WINDOWS), MAX_DAYS)
    since = (now - timedelta(days=span + 1)).strftime("%Y-%m-%d")
    opened = json.loads(sh(["gh", "issue", "list", "--state", "all",
                            "--search", f"created:>={since}", "--limit", "800",
                            "--json", "createdAt"]))
    closed = json.loads(sh(["gh", "issue", "list", "--state", "all",
                            "--search", f"closed:>={since}", "--limit", "800",
                            "--json", "closedAt"]))
    oc = [parse_ts(i["createdAt"]) for i in opened]
    cc = [parse_ts(i["closedAt"]) for i in closed if i.get("closedAt")]
    flow = {}
    for w in WINDOWS:
        cut = now - timedelta(days=w)
        op = sum(1 for t in oc if t and t >= cut)
        cl = sum(1 for t in cc if t and t >= cut)
        flow[w] = {"opened": op, "closed": cl, "net": op - cl}
    daily = []
    for d in range(MAX_DAYS - 1, -1, -1):
        day = (now - timedelta(days=d)).date()
        op = sum(1 for t in oc if t and t.date() == day)
        cl = sum(1 for t in cc if t and t.date() == day)
        daily.append((day.isoformat(), cl - op))
    return flow, daily


def load_snaps() -> list[dict]:
    if not SNAP.exists():
        return []
    rows = [json.loads(ln) for ln in SNAP.read_text().splitlines() if ln.strip()]
    return sorted(rows, key=lambda r: r["ts"])


def record_snap(row: dict) -> None:
    """Append today's snapshot, one row per UTC date (last write wins)."""
    rows = [r for r in load_snaps() if r.get("date") != row["date"]]
    rows.append(row)
    rows.sort(key=lambda r: r["ts"])
    SNAP.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")


def snap_trend(snaps: list[dict], key: str, w: int, now: datetime, current: int):
    """current - (most recent snapshot at or before now-w days). None if no
    snapshot is old enough yet (history hasn't accrued)."""
    target = now - timedelta(days=w)
    base = [s for s in snaps if parse_ts(s["ts"]) <= target and key in s]
    return None if not base else current - base[-1][key]


def delta_cell(delta, lower_better: bool = True) -> str:
    if delta is None:
        return ('<td class="nd" title="accrues from the first --snapshot">'
                '&middot;</td>')
    if delta == 0:
        return '<td class="flat">&ndash;</td>'
    good = (delta < 0) if lower_better else (delta > 0)
    arrow = "&#9660;" if delta < 0 else "&#9650;"  # ▼ / ▲
    return f'<td class="{"good" if good else "bad"}">{arrow}{abs(delta)}</td>'


def fmt_date(iso: str) -> str:
    """'2026-06-16' -> 'Jun 16' for a compact, intuitive axis tick."""
    months = ("", "Jan", "Feb", "Mar", "Apr", "May", "Jun",
              "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")
    _, m, d = iso.split("-")
    return f"{months[int(m)]} {int(d)}"


def bar_chart_svg(daily: list[tuple[str, int]], w: int = 340, h: int = 150) -> str:
    """Per-day net backlog change (closed - opened) as diverging bars: up &
    green = net progress, down & red = backlog grew (red when < 0). Bars share a
    central zero baseline, scaled to the largest-magnitude day. Inline SVG,
    no JS, no external assets."""
    if not daily:
        return '<p class="empty">&mdash; no issue flow in the window &mdash;</p>'
    m = max(1, max(abs(v) for _, v in daily))
    n = len(daily)
    pad_l = pad_r = 6
    pad_t, pad_b = 12, 18
    plot_h = h - pad_t - pad_b
    mid = pad_t + plot_h / 2
    bw = (w - pad_l - pad_r) / n
    gap = min(4.0, bw * 0.3)
    parts = [f'<line x1="{pad_l}" y1="{mid:.1f}" x2="{w - pad_r}" y2="{mid:.1f}" '
             f'stroke="#d0d7de" stroke-width="1"/>']
    for i, (d, v) in enumerate(daily):
        x = pad_l + i * bw + gap / 2
        bwi = max(1.0, bw - gap)
        bh = max(2.0, (abs(v) / m) * (plot_h / 2))
        if v > 0:
            y, color = mid - bh, "#1a7f37"
        elif v < 0:
            y, color = mid, "#cf222e"
        else:
            y, color, bh = mid - 1, "#afb8c1", 2.0
        tip = f"{d}: {'+' if v > 0 else ''}{v} " + (
            "net (progress)" if v > 0 else
            "net (backlog grew)" if v < 0 else "flat")
        parts.append(f'<rect x="{x:.1f}" y="{y:.1f}" width="{bwi:.1f}" '
                     f'height="{bh:.1f}" rx="1" fill="{color}">'
                     f'<title>{tip}</title></rect>')
    for i, anchor in ((0, "start"), (n - 1, "end")):
        x = pad_l + i * bw + bw / 2
        parts.append(f'<text x="{x:.1f}" y="{h - 5}" font-size="9" '
                     f'fill="#8b949e" text-anchor="{anchor}">{fmt_date(daily[i][0])}</text>')
    return (f'<svg viewBox="0 0 {w} {h}" style="width:100%;height:auto" '
            f'role="img" aria-label="net backlog change per day">'
            + "".join(parts) + "</svg>")


def line_chart_svg(points: list[tuple[str, int]], w: int = 340, h: int = 150) -> str:
    """Open-issue count trajectory (reconstructed from timestamps), oldest ->
    newest, with a simple y-axis (count gridlines at lo/mid/hi) and an intuitive
    date x-axis (~4 ticks). A falling line = backlog shrinking. The level whose
    daily change is the bar chart. Inline SVG, no JS, no external assets."""
    if len(points) < 2:
        return '<p class="empty">&mdash; not enough history &mdash;</p>'
    vals = [v for _, v in points]
    lo, hi = min(vals), max(vals)
    if hi == lo:
        hi, lo = hi + 1, lo - 1          # avoid a zero span on a flat series
    span = hi - lo
    n = len(points)
    pad_l, pad_r, pad_t, pad_b = 26, 10, 10, 22
    plot_w = w - pad_l - pad_r
    plot_h = h - pad_t - pad_b
    base = pad_t + plot_h

    def fx(i):
        return pad_l + (i / (n - 1)) * plot_w

    def fy(v):
        return pad_t + (hi - v) / span * plot_h

    parts = []
    # y-axis: faint gridlines + count labels at lo / mid / hi
    for yv in sorted({lo, (lo + hi) // 2, hi}):
        y = fy(yv)
        parts.append(f'<line x1="{pad_l}" y1="{y:.1f}" x2="{w - pad_r}" y2="{y:.1f}" '
                     f'stroke="#eef1f4" stroke-width="1"/>')
        parts.append(f'<text x="{pad_l - 5}" y="{y + 3:.1f}" font-size="9" '
                     f'fill="#8b949e" text-anchor="end">{yv}</text>')
    # area fill + trajectory line + current-value marker
    pts = " ".join(f"{fx(i):.1f},{fy(v):.1f}" for i, v in enumerate(vals))
    parts.append(f'<polygon points="{fx(0):.1f},{base:.1f} {pts} '
                 f'{fx(n - 1):.1f},{base:.1f}" fill="rgba(29,118,219,0.10)"/>')
    parts.append(f'<polyline points="{pts}" fill="none" stroke="#1d76db" '
                 f'stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>')
    parts.append(f'<circle cx="{fx(n - 1):.1f}" cy="{fy(vals[-1]):.1f}" r="3" '
                 f'fill="#1d76db"/>')
    # x-axis: baseline + ~4 evenly spaced intuitive date ticks
    parts.append(f'<line x1="{pad_l}" y1="{base:.1f}" x2="{w - pad_r}" y2="{base:.1f}" '
                 f'stroke="#d0d7de" stroke-width="1"/>')
    k = min(4, n)
    for i in sorted({round(j * (n - 1) / (k - 1)) for j in range(k)}):
        x = fx(i)
        anchor = "start" if i == 0 else "end" if i == n - 1 else "middle"
        parts.append(f'<line x1="{x:.1f}" y1="{base:.1f}" x2="{x:.1f}" '
                     f'y2="{base + 3:.1f}" stroke="#d0d7de"/>')
        parts.append(f'<text x="{x:.1f}" y="{h - 6}" font-size="9" fill="#8b949e" '
                     f'text-anchor="{anchor}">{fmt_date(points[i][0])}</text>')
    return (f'<svg viewBox="0 0 {w} {h}" style="width:100%;height:auto" '
            f'role="img" aria-label="open issue count over time">'
            + "".join(parts) + "</svg>")


def esc(s: str) -> str:
    return html.escape(s, quote=True)


def search_url(repo: str, *labels: str, extra: str = "") -> str:
    q = "is:open " + " ".join(f'label:"{l}"' for l in labels)
    if extra:
        q += " " + extra
    return f"https://github.com/{repo}/issues?q=" + urllib.parse.quote(q)


def blob(repo: str, path: str) -> str:
    return f"https://github.com/{repo}/blob/main/{path}"


def heat(count: int, mx: int) -> str:
    if count == 0:
        return "background:#f6f8fa;color:#aab"
    t = count / mx if mx else 0.0
    return f"background:rgba(29,118,219,{0.10 + 0.55 * t:.3f});color:#0b1f33"


def issue_li(it: dict) -> str:
    badges = ""
    if it["external"]:
        badges += f' <span class="badge ext">@{esc(it["author"])}</span>'
    if it["blocker"]:
        badges += ' <span class="badge blk">blocker</span>'
    if it["effort"]:
        badges += f' <span class="badge">{esc(it["effort"])}</span>'
    if it["status"]:
        badges += f' <span class="badge">{esc(it["status"])}</span>'
    if it["audit"]:
        badges += ' <span class="badge aud">audit</span>'
    return (f'<li><a class="iss" href="{esc(it["url"])}">#{it["n"]}</a> '
            f'{esc(it["t"])}{badges}</li>')


def details(summary: str, items: list[dict], open_: bool = False) -> str:
    if not items:
        body = '<p class="empty">— none —</p>'
    else:
        body = "<ul>" + "".join(issue_li(i) for i in items) + "</ul>"
    op = " open" if open_ else ""
    return f"<details{op}><summary>{summary} <b>({len(items)})</b></summary>{body}</details>"


def render(repo: str, issues: list[dict], flow: dict,
           daily: list[tuple[str, int]], snaps: list[dict], now: datetime):
    total = len(issues)
    by_kind = {k: [i for i in issues if i["kind"] == k] for k in KIND_ORDER}
    by_area = {a: [i for i in issues if a in i["areas"]] for a in AREA_ORDER}
    blockers = [i for i in issues if i["blocker"]]
    design = by_kind["kind/design"]
    audit = [i for i in issues if i["audit"]]
    external = [i for i in issues if i["external"]]
    ext_search = (f"https://github.com/{repo}/issues?q="
                  + urllib.parse.quote(f"is:open -author:{MAINTAINER}"))
    sclass = [i for i in issues
              if i["status"] == "status/s-class" or i["effort"] == "effort/S"]
    unclassified = [i for i in issues if i["kind"] is None or not i["areas"]]

    # kind x area cross-tab
    cell = {(k, a): sum(1 for i in by_kind[k] if a in i["areas"])
            for k in KIND_ORDER for a in AREA_ORDER}
    mx = max(cell.values()) if cell else 0
    kinds_present = [k for k in KIND_ORDER
                     if by_kind[k] or any(cell[(k, a)] for a in AREA_ORDER)]

    def short(label):
        return esc(label.split("/")[1])

    def colhead(repo_label):
        return (f'<th><a href="{search_url(repo, repo_label)}">'
                f'{short(repo_label)}</a></th>')

    def xcell(k, a):
        c = cell[(k, a)]
        link = f'<a href="{search_url(repo, k, a)}">{c}</a>' if c else c
        return f'<td style="{heat(c, mx)}">{link}</td>'

    def rowhead(label, count):
        return (f'<th class="rk"><a href="{search_url(repo, label)}">'
                f'{short(label)}</a> <span class="ct">{count}</span></th>')

    # Wide (desktop): kinds as rows, areas as columns.
    head = "".join(colhead(a) for a in AREA_ORDER)
    rows = "".join(
        f'<tr>{rowhead(k, len(by_kind[k]))}'
        + "".join(xcell(k, a) for a in AREA_ORDER) + "</tr>"
        for k in kinds_present)
    # Narrow (mobile): transposed — areas as rows, kinds as columns.
    nhead = "".join(colhead(k) for k in kinds_present)
    nrows = "".join(
        f'<tr>{rowhead(a, len(by_area[a]))}'
        + "".join(xcell(k, a) for k in kinds_present) + "</tr>"
        for a in AREA_ORDER)

    def stat(label, n, href=None, tone=None):
        inner = (f'<a href="{href}">{n}</a>' if href else n)
        cls = f"stat {tone}" if tone else "stat"
        return f'<div class="{cls}"><div class="num">{inner}</div><div class="lab">{label}</div></div>'

    stats = "".join([
        stat("open", total, f"https://github.com/{repo}/issues"),
        stat("blockers", len(blockers), search_url(repo, "blocker"),
             tone="ok" if not blockers else "danger"),
        stat("external", len(external), ext_search,
             tone="ok" if not external else "warn"),
        stat("bugs", len(by_kind["kind/bug"]), search_url(repo, "kind/bug")),
        stat("features", len(by_kind["kind/feature"]), search_url(repo, "kind/feature")),
        stat("needs RFC", len(design), search_url(repo, "kind/design")),
    ])

    kind_blocks = "".join(
        details(esc(k), by_kind[k]) for k in KIND_ORDER if by_kind[k])
    area_blocks = "".join(
        details(esc(a), by_area[a]) for a in AREA_ORDER if by_area[a])

    unclass_note = ""
    if unclassified:
        unclass_note = (
            '<p class="warn">⚠ '
            f'{len(unclassified)} issue(s) missing a kind/ or area/: '
            + ", ".join(f'<a href="{esc(i["url"])}">#{i["n"]}</a>'
                        for i in unclassified) + "</p>")

    sclass_hint = ""
    if not sclass:
        sclass_hint = ('<p class="empty">Empty until the effort pass runs — '
                       '<code>effort/</code> and <code>status/s-class</code> '
                       'are set by reading each issue + a code peek, not from '
                       'titles.</p>')

    # Snapshot row (the series we trend on) + momentum panel.
    row = {
        "date": now.strftime("%Y-%m-%d"),
        "ts": now.isoformat(timespec="seconds"),
        "total": total, "blocker": len(blockers),
        "audit": len(audit), "external": len(external),
        **{k: len(by_kind[k]) for k in KIND_ORDER},
        **{a: len(by_area[a]) for a in AREA_ORDER},
    }
    win_h = "".join(f"<th>&Delta; {w}d</th>" for w in WINDOWS)
    # Open issues: exact + backfilled (delta over window == net flow).
    open_cells = "".join(delta_cell(flow[w]["net"]) for w in WINDOWS)

    def metric_row(label, key, current):
        cells = "".join(delta_cell(snap_trend(snaps, key, w, now, current))
                        for w in WINDOWS)
        return f'<tr><th class="rk">{label}</th><td>{current}</td>{cells}</tr>'

    trend_rows = (
        f'<tr><th class="rk">open issues</th><td>{total}</td>{open_cells}</tr>'
        + metric_row("blockers", "blocker", len(blockers))
        + metric_row("bugs", "kind/bug", len(by_kind["kind/bug"]))
        + metric_row("external", "external", len(external)))
    flow_line = " &middot; ".join(
        f'{w}d <b>+{flow[w]["opened"]}</b> / <b>&minus;{flow[w]["closed"]}</b>'
        for w in WINDOWS)
    has_hist = any(parse_ts(s["ts"]) <= now - timedelta(days=min(WINDOWS))
                   for s in snaps)
    trend_note = ("Open trend is exact (issue timestamps); label trends "
                  + ("" if has_hist else "begin accruing now &mdash; ")
                  + "from the <code>--snapshot</code> log. &#9660; down is good.")
    # Two momentum charts, half-width each (fixes the letterboxed aspect ratio):
    # per-day net change (bars) and the open-count level it integrates to (line),
    # reconstructed by walking the daily net reductions back from today's total.
    line_pts = []
    running = total
    for date, red in reversed(daily):   # newest first: open count at day's end
        line_pts.append((date, running))
        running += red                  # undo the day's reduction -> prior end
    line_pts.reverse()

    # Per-range chart variants + a CSS-only window switch (1w / 2w / 1m). Each
    # range slices the tail of the 30-day series; one variant is shown at a time
    # via radio :checked sibling rules (no JS). Selectors are generated from
    # RANGES so the markup and CSS can't drift apart.
    bar_cvs = "".join(
        f'<div class="cv cv{d}">{bar_chart_svg(daily[-d:])}</div>' for d, _ in RANGES)
    line_cvs = "".join(
        f'<div class="cv cv{d}">{line_chart_svg(line_pts[-d:])}</div>' for d, _ in RANGES)
    radios = "".join(
        f'<input class="mrange" type="radio" name="mrange" id="mr{d}"'
        + (" checked" if d == DEFAULT_DAYS else "") + ">" for d, _ in RANGES)
    pills = "".join(f'<label class="rng" for="mr{d}">{lbl}</label>'
                    for d, lbl in RANGES)
    range_css = "\n  ".join([
        ".momentum-charts > input.mrange { position:absolute; opacity:0; pointer-events:none; }",
        ".bc-toolbar { text-align:right; margin:2px 0 6px; font-size:11px; }",
        ".rng-cap { text-transform:uppercase; letter-spacing:.03em; color:#8b949e; margin-right:4px; }",
        ".rng { display:inline-block; padding:2px 9px; margin-left:4px;"
        " border:1px solid #d0d7de; border-radius:999px; color:#57606a; cursor:pointer; }",
        ".rng:hover { border-color:#1d76db; color:#1d76db; }",
        ".cv { display:none; }",
        ",\n  ".join(f"#mr{d}:checked ~ .bc-row .cv{d}" for d, _ in RANGES)
        + " { display:block; }",
        ",\n  ".join(f'#mr{d}:checked ~ .bc-toolbar .rng[for="mr{d}"]' for d, _ in RANGES)
        + " { background:#1d76db; border-color:#1d76db; color:#fff; }",
    ])

    gen = now.strftime("%Y-%m-%d %H:%M UTC")
    labels_doc = blob(repo, "docs/dev/issue-labels.md")
    tiers_doc = blob(repo, "docs/dev/issue-triage-tiers.md")

    page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>camdl · issue triage</title>
<style>
  :root {{ color-scheme: light; }}
  * {{ box-sizing: border-box; }}
  body {{ font: 15px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
         margin: 0; color: #1b1f24; background: #fbfcfd; }}
  .wrap {{ max-width: 1040px; margin: 0 auto; padding: 28px 22px 80px; }}
  h1 {{ font-size: 22px; margin: 0 0 2px; }}
  h2 {{ font-size: 15px; text-transform: uppercase; letter-spacing: .04em;
        color: #57606a; margin: 34px 0 12px; border-bottom: 1px solid #e7ebef; padding-bottom: 6px; }}
  .meta {{ color: #8b949e; font-size: 13px; margin-bottom: 20px; }}
  a {{ color: #1d76db; text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  .stats {{ display: flex; flex-wrap: wrap; gap: 10px; }}
  .stat {{ flex: 1 1 120px; background: #fff; border: 1px solid #e7ebef; border-radius: 10px;
           padding: 14px 16px; }}
  .num {{ font-size: 26px; font-weight: 650; }}
  .stat.ok {{ background: #e6f4ea; border-color: #aacdb6; }}
  .stat.ok .num {{ color: #1a7f37; }}
  .stat.danger {{ background: #ffecea; border-color: #f3b0aa; }}
  .stat.danger .num {{ color: #b60205; }}
  .stat.warn {{ background: #fff8e1; border-color: #ecd98a; }}
  .stat.warn .num {{ color: #9a6700; }}
  .lab {{ font-size: 12px; color: #6e7781; text-transform: uppercase; letter-spacing: .03em; }}
  table {{ border-collapse: collapse; width: 100%; font-size: 13px; }}
  th, td {{ border: 1px solid #e7ebef; padding: 6px 9px; text-align: center; }}
  th {{ background: #f6f8fa; font-weight: 600; }}
  td a {{ color: inherit; font-weight: 600; }}
  th.rk {{ text-align: left; white-space: nowrap; }}
  .ct {{ color: #8b949e; font-weight: 400; font-size: 11px; }}
  details {{ background: #fff; border: 1px solid #e7ebef; border-radius: 8px;
             margin: 7px 0; padding: 4px 12px; }}
  summary {{ cursor: pointer; padding: 6px 0; font-weight: 550; }}
  details ul {{ margin: 4px 0 10px; padding-left: 20px; }}
  details li {{ margin: 3px 0; }}
  a.iss {{ font-variant-numeric: tabular-nums; font-weight: 600; }}
  .badge {{ font-size: 11px; background: #eef1f4; color: #57606a; border-radius: 5px;
            padding: 1px 6px; margin-left: 2px; white-space: nowrap; }}
  .badge.blk {{ background: #ffd7d5; color: #b60205; font-weight: 600; }}
  .badge.aud {{ background: #efe5ff; color: #5319e7; }}
  .badge.ext {{ background: #fff3c4; color: #7a5d00; font-weight: 600; }}
  .empty, .warn {{ color: #8b949e; font-size: 13px; }}
  .warn {{ color: #b60205; }}
  .trend td {{ font-variant-numeric: tabular-nums; font-weight: 600; }}
  .trend th.rk {{ font-weight: 550; }}
  .good {{ color: #1a7f37; background: #e6f4ea; }}
  .bad  {{ color: #b60205; background: #ffe9e7; }}
  .flat {{ color: #8b949e; font-weight: 400; }}
  .nd   {{ color: #c4cdd6; font-weight: 400; }}
  .flow {{ font-size: 13px; color: #57606a; margin: 8px 0 2px; }}
  .flow b {{ font-variant-numeric: tabular-nums; color: #1b1f24; }}
  .bc-row {{ display: grid; grid-template-columns: 1fr 1fr; gap: 14px;
             margin: 10px 0 2px; }}
  .barchart {{ background: #fff; border: 1px solid #e7ebef; border-radius: 8px;
               padding: 10px 12px 4px; }}
  .bc-head {{ font-size: 12px; color: #57606a; text-transform: uppercase;
              letter-spacing: .03em; margin-bottom: 6px; overflow: hidden; }}
  .bc-legend {{ float: right; text-transform: none; letter-spacing: 0;
                color: #8b949e; font-size: 11px; font-weight: 400; }}
  .bc-legend i {{ display: inline-block; width: 9px; height: 9px;
                  border-radius: 2px; margin-right: 2px; }}
  .bc-legend i.g {{ background: #1a7f37; }}
  .bc-legend i.r {{ background: #cf222e; }}
  {range_css}
  .cols {{ display: grid; grid-template-columns: 1fr 1fr; gap: 14px; }}
  .xtab.narrow {{ display: none; }}
  @media (max-width: 720px) {{
    .cols {{ grid-template-columns: 1fr; }}
    .bc-row {{ grid-template-columns: 1fr; }}
    .xtab.wide {{ display: none; }}
    .xtab.narrow {{ display: table; }}
    .xtab.narrow th, .xtab.narrow td {{ padding: 4px 4px; font-size: 12px; }}
    .xtab.narrow th.rk {{ white-space: nowrap; }}
    .xtab.narrow .ct {{ display: none; }}
  }}
  code {{ background: #eef1f4; border-radius: 4px; padding: 1px 5px; font-size: 12px; }}
  footer {{ margin-top: 40px; color: #8b949e; font-size: 12px; }}
</style></head><body><div class="wrap">

<h1>camdl &middot; issue triage</h1>
<div class="meta">{esc(repo)} &middot; generated {gen} &middot; axes per
  <a href="{labels_doc}">issue-labels.md</a></div>

<div class="stats">{stats}</div>
{unclass_note}

<h2>Momentum</h2>
<table class="trend"><tr><th></th><th>now</th>{win_h}</tr>{trend_rows}</table>
<p class="flow">opened / closed: {flow_line}</p>
<div class="momentum-charts">
  {radios}
  <div class="bc-toolbar"><span class="rng-cap">window</span>{pills}</div>
  <div class="bc-row">
    <div class="barchart">
      <div class="bc-head">net change
        <span class="bc-legend"><i class="g"></i>progress&nbsp;<i class="r"></i>grew</span></div>
      {bar_cvs}
    </div>
    <div class="barchart">
      <div class="bc-head">open issues
        <span class="bc-legend">now <b>{total}</b></span></div>
      {line_cvs}
    </div>
  </div>
</div>
<p class="empty">{trend_note}</p>

<h2>Critical path &mdash; blockers</h2>
{details("silent-wrong on the inference/sim path", blockers, open_=True)}

<h2>kind &times; area</h2>
<table class="xtab wide"><tr><th></th>{head}</tr>{rows}</table>
<table class="xtab narrow"><tr><th></th>{nhead}</tr>{nrows}</table>

<h2>Batch queue &mdash; s-class / small</h2>
{details("effort/S or status/s-class", sclass, open_=True)}
{sclass_hint}

<h2>Cohorts</h2>
{details(f"external reporters (not @{esc(MAINTAINER)})", external, open_=True)}
<div class="cols">
  <div>{details("upstream-audit", audit)}{details("needs an RFC (kind/design)", design)}</div>
  <div>{details("docs", by_kind["kind/docs"])}{details("refactor / tech-debt", by_kind["kind/refactor"])}</div>
</div>

<h2>By kind</h2>
{kind_blocks}

<h2>By area</h2>
{area_blocks}

<footer>Order &amp; discipline: <a href="{tiers_doc}">issue-triage-tiers.md</a> &middot;
  rebuild: <code>python3 docs/dev/dashboard/build.py</code></footer>

</div></body></html>
"""
    return page, row


def build(snapshot: bool = False) -> Path:
    repo = repo_slug()
    issues = fetch_issues()
    now = datetime.now(timezone.utc)
    flow, daily = fetch_flow(now)
    snaps = load_snaps()
    page, row = render(repo, issues, flow, daily, snaps, now)
    OUT.write_text(page, encoding="utf-8")
    if snapshot:
        record_snap(row)
    return OUT


def serve(port: int) -> None:
    import http.server
    import socketserver

    class Handler(http.server.SimpleHTTPRequestHandler):
        def do_GET(self):  # regenerate on each top-level load → always live
            if self.path in ("/", "/index.html"):
                try:
                    build()
                except SystemExit as e:
                    self.send_error(500, str(e))
                    return
            return super().do_GET()

        def log_message(self, *a):  # quiet
            pass

    class Server(socketserver.TCPServer):
        allow_reuse_address = True  # rebind immediately after a restart (TIME_WAIT)

    handler = functools.partial(Handler, directory=str(HERE))
    with Server(("", port), handler) as httpd:
        print(f"serving live dashboard at http://localhost:{port}/  (Ctrl-C to stop)")
        httpd.serve_forever()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--serve", action="store_true",
                    help="serve and regenerate on each page load")
    ap.add_argument("--port", type=int, default=8799)
    ap.add_argument("--snapshot", action="store_true",
                    help="record today's counts into the trend log "
                         "(run daily, e.g. from cron, to accrue label history)")
    args = ap.parse_args()
    path = build(snapshot=args.snapshot)
    print(f"wrote {path}" + (" (+snapshot)" if args.snapshot else ""))
    if args.serve:
        serve(args.port)


if __name__ == "__main__":
    main()
