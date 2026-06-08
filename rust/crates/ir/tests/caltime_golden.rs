//! Cross-language calendar-time golden (gh#98).
//!
//! Reads `ir/golden/caltime.tsv` — the single source of truth shared with
//! `ocaml/test/test_caltime_golden.ml` — and asserts, for every row, that the
//! Rust date machinery (`ir::caltime::parse_iso_date` / `rata_die` /
//! `date_to_internal`) agrees with the committed `delta_days` / `t` (accept
//! rows) or rejects the string (reject rows). The OCaml test reads the same
//! file, so the two independent parsers are pinned to the same acceptance set
//! and the same conversion — closing the "two un-pinned date parsers" hole the
//! typed-time proposal §5.6 flagged.

use ir::caltime::{date_to_internal, parse_iso_date, rata_die};
use std::path::PathBuf;

/// The TSV lives at `ir/golden/`, outside any crate's manifest dir, so locate
/// the repo root by walking up from this crate (`rust/crates/ir`).
fn golden_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // .../rust/crates/ir
    for _ in 0..3 {
        p = p.parent().expect("walking to repo root").to_path_buf();
    }
    p.join("ir/golden/caltime.tsv")
}

struct Row {
    origin: String,
    date: String,
    time_unit: String,
    expect: String,
    delta_days: String,
    t: String,
}

fn rows() -> Vec<Row> {
    let path = golden_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("golden table missing: {} ({e})", path.display()));
    text.lines()
        .filter(|l| {
            let lt = l.trim();
            !lt.is_empty() && !lt.starts_with('#')
        })
        // Split on tabs WITHOUT trimming cells: a date cell deliberately carries
        // surrounding whitespace (the trim-acceptance row), and parse_iso_date
        // must receive it raw.
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            assert!(c.len() == 6, "malformed golden row ({} cells): {l:?}", c.len());
            if c[0] == "origin" {
                return None; // header
            }
            Some(Row {
                origin: c[0].to_string(),
                date: c[1].to_string(),
                time_unit: c[2].to_string(),
                expect: c[3].to_string(),
                delta_days: c[4].to_string(),
                t: c[5].to_string(),
            })
        })
        .collect()
}

#[test]
fn caltime_golden_matches_rust_parser() {
    let rows = rows();
    assert!(rows.len() >= 15, "golden table unexpectedly small: {} rows", rows.len());
    let mut accepts = 0;
    let mut rejects = 0;
    for r in &rows {
        match r.expect.as_str() {
            "accept" => {
                accepts += 1;
                let (oy, om, od) = parse_iso_date(&r.origin)
                    .unwrap_or_else(|e| panic!("origin {:?} must parse: {e:?}", r.origin));
                let (ty, tm, td) = parse_iso_date(&r.date)
                    .unwrap_or_else(|e| panic!("date {:?} must parse: {e:?}", r.date));
                let delta = rata_die(ty, tm, td) - rata_die(oy, om, od);
                let want_delta: i64 = r.delta_days.parse()
                    .unwrap_or_else(|_| panic!("bad delta_days {:?}", r.delta_days));
                assert_eq!(
                    delta, want_delta,
                    "delta_days mismatch for origin={:?} date={:?}", r.origin, r.date
                );
                let got_t = date_to_internal(&r.origin, &r.date, &r.time_unit)
                    .unwrap_or_else(|e| panic!("date_to_internal {:?}->{:?} {}: {e:?}", r.origin, r.date, r.time_unit));
                let want_t: f64 = r.t.parse()
                    .unwrap_or_else(|_| panic!("bad t {:?}", r.t));
                assert!(
                    (got_t - want_t).abs() < 1e-9,
                    "t mismatch for origin={:?} date={:?} unit={}: got {got_t}, want {want_t}",
                    r.origin, r.date, r.time_unit
                );
            }
            "reject" => {
                rejects += 1;
                // The `date` cell is the string under test; it must be rejected.
                assert!(
                    parse_iso_date(&r.date).is_err(),
                    "date {:?} must be REJECTED by the Rust parser but it accepted",
                    r.date
                );
            }
            other => panic!("unknown expect value {other:?}"),
        }
    }
    assert!(accepts > 0 && rejects > 0, "golden must cover both accept and reject rows (got {accepts}/{rejects})");
}
