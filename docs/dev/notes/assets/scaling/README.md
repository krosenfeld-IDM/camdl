# FOI scaling study — figures & data

Artifacts for [`../../2026-05-29-foi-scaling-bench.md`](../../2026-05-29-foi-scaling-bench.md)
(the profiling investigation + optimization staging). All are regenerable from
the committed harness — see the `make` targets noted per item.

## Figures

| file | what it shows | regenerate |
| --- | --- | --- |
| `scaling_curves.png` | 4-panel scaling characterization: ① IR size O(P²) coupling-on vs O(P) off; ② the ~5× `rate_grad` multiplier; ③ forward-sim time ∝ IR bytes (parse-bound); ④ RSS ≈ 2.7× IR bytes | `make bench-scaling` |
| `deser_load_before_after.png` | **Fix E** before/after: model load time (`ir::from_str` + `CompiledModel::new`) with derived `#[serde(untagged)]` vs the hand-written single-pass `Deserialize`, per model size (2.5×–5.8×, grows with tree depth) | `make bench-micro` + `scripts/plot_deser_before_after.py` |
| `flamegraph_real.svg` | symbolicated flamegraph of `simulate` on the ~2 GB anchor (P=44,A=21): ~65% in `ir::from_str`, ~50% in serde `Content` buffering. **gitignored** (~4 MB, regenerable) | `make flamegraph-real` |

## Data (raw, committed)

| file | columns |
| --- | --- |
| `scaling.tsv` | `slice P A coupling grad n_compartments n_transitions ir_bytes compile_s sim_s peak_rss_mb` (macro sweep) |
| `deser_load_before_after.tsv` | `model ir_mb before_us after_us` (criterion `load_parse_compile`; before = untagged, after = single-pass) |

## Live viewing

While a server is running (`python3 -m http.server 8910 --directory <this dir>`),
the figures are at `http://thuja:8910/<file>` over Tailscale.
