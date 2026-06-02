//! Reading the content-addressed (`runid::RunRecord`) CAS store.
//!
//! A generic, Layout-driven walk: the presence of a parseable `RunRecord`
//! `run.json` is the only discovery signal — no hardcoded level depth. Every
//! run kind (sim / fit-stage / profile-point / pfilter / survey) is discovered
//! here; the per-kind projections live in `browse` and `fit::fit_view`.

use std::path::{Path, PathBuf};

use runid::{ArtifactKind, RunRecord};

use crate::cas_index;

// The fit-level provenance sidecar (`fit.meta.json`) lives in
// `run_meta::FitSidecar` (it carries `run_meta` provenance types —
// `ResolvedPriorEntry`, `ParameterProvenance`), with `write_fit_sidecar` /
// `read_fit_sidecar` beside it there.

/// Recursively collect every `(dir, RunRecord)` under `subtree` whose dir holds
/// a parseable `RunRecord` `run.json`. Hidden dirs (`.staging`, `.quarantine`)
/// are skipped. Descends through leaves too, but a leaf's declared child
/// sub-artifacts under `obs/…` carry an `obs.json` (not a `run.json`), so they
/// are not surfaced here as standalone records — they're reached as the
/// trajectory leaf's `children`.
pub fn walk_records(subtree: &Path) -> Vec<(PathBuf, RunRecord)> {
    let mut out = Vec::new();
    if !subtree.exists() {
        return out;
    }
    let mut stack = vec![subtree.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rj = dir.join("run.json");
        if rj.is_file() {
            if let Ok(bytes) = std::fs::read(&rj) {
                if let Ok(rec) = serde_json::from_slice::<RunRecord>(&bytes) {
                    out.push((dir.clone(), rec));
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                let name = e.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue; // .staging / .quarantine
                }
                stack.push(p);
            }
        }
    }
    out
}

/// A new-format leaf record with its directory, plus convenience accessors
/// that read the factored level labels (provenance) for display.
#[derive(Debug, Clone)]
pub struct Leaf {
    pub dir: PathBuf,
    pub record: RunRecord,
}

impl Leaf {
    /// A level's readable label by level name (`"model"`, `"scenario"`, …).
    pub fn level_label(&self, name: &str) -> &str {
        self.record
            .levels
            .iter()
            .find(|l| l.name == name)
            .map(|l| l.label.as_str())
            .unwrap_or("")
    }

    pub fn run_id_hex(&self) -> String {
        self.record.run_id.to_hex()
    }

    /// The base seed parsed from the `seed_{n}` label (0 if absent/unparsed).
    pub fn seed(&self) -> u64 {
        self.level_label("seed")
            .strip_prefix("seed_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// `traj.tsv` size in bytes (0 if absent).
    pub fn traj_bytes(&self) -> u64 {
        std::fs::metadata(self.dir.join("traj.tsv")).map(|m| m.len()).unwrap_or(0)
    }
}

/// Resolve leaves of `kind` whose `run_id` hex starts with `prefix`, using the
/// derived index as an accelerator with `run.json` as the source of truth.
///
/// The fast path consults [`cas_index::resolve_prefix`], every hit of which is
/// verified against the live `run.json` (a stale/repointed entry is dropped —
/// never resolved to a dead path). On an index miss (no index, no matching
/// entry, or every candidate stale) it falls back to the full per-kind walk —
/// which finds out-of-band-added leaves the index lacks — and then repairs the
/// index from a fresh full-tree walk (best-effort; a cache, never a gate).
fn resolve_prefix_indexed(
    root: &Path,
    kind: ArtifactKind,
    prefix: &str,
    walk: impl Fn(&Path) -> Vec<Leaf>,
) -> Vec<Leaf> {
    if let Some(hits) = cas_index::resolve_prefix(root, kind, prefix) {
        return hits;
    }
    // Index miss → authoritative full walk (out-of-band leaves are found
    // here), then repair the index so the next lookup is fast.
    let hits: Vec<Leaf> =
        walk(root).into_iter().filter(|s| s.run_id_hex().starts_with(prefix)).collect();
    let _ = cas_index::rebuild(root);
    hits
}

/// All `sims/` leaves of kind `Sim` (new-format trajectory runs).
pub fn walk_sim_leaves(root: &Path) -> Vec<Leaf> {
    walk_records(&root.join("sims"))
        .into_iter()
        .filter(|(_, r)| r.kind == ArtifactKind::Sim)
        .map(|(dir, record)| Leaf { dir, record })
        .collect()
}

/// New-format sims whose `run_id` hex matches `prefix` (for `show`/`cat`
/// prefix resolution; combined with the legacy `run.hash` matches in
/// `browse::resolve_any` so a user can address any run during M2→M3).
pub fn resolve_sim_prefix(root: &Path, prefix: &str) -> Vec<Leaf> {
    resolve_prefix_indexed(root, ArtifactKind::Sim, prefix, walk_sim_leaves)
}

/// All `fits/` leaves of kind `FitStage` (new-format fit-stage runs, M3.2).
pub fn walk_fit_leaves(root: &Path) -> Vec<Leaf> {
    walk_records(&root.join("fits"))
        .into_iter()
        .filter(|(_, r)| r.kind == ArtifactKind::FitStage)
        .map(|(dir, record)| Leaf { dir, record })
        .collect()
}

/// New-format fit stages whose `run_id` hex matches `prefix` (for `show`/`cat`
/// prefix resolution alongside `resolve_sim_prefix`).
pub fn resolve_fit_prefix(root: &Path, prefix: &str) -> Vec<Leaf> {
    resolve_prefix_indexed(root, ArtifactKind::FitStage, prefix, walk_fit_leaves)
}

/// All `profiles/` leaves of kind `ProfilePoint` (new-format profile-point
/// mini-fits, M3.3). Each is one `(grid point × seed × start)` cell under the
/// factored `profile/point/stage/seed/start` tree.
pub fn walk_profile_leaves(root: &Path) -> Vec<Leaf> {
    walk_records(&root.join("profiles"))
        .into_iter()
        .filter(|(_, r)| r.kind == ArtifactKind::ProfilePoint)
        .map(|(dir, record)| Leaf { dir, record })
        .collect()
}

/// New-format profile points whose `run_id` hex matches `prefix` (for
/// `show`/`cat` prefix resolution alongside `resolve_sim_prefix`).
pub fn resolve_profile_prefix(root: &Path, prefix: &str) -> Vec<Leaf> {
    resolve_prefix_indexed(root, ArtifactKind::ProfilePoint, prefix, walk_profile_leaves)
}

/// All `pfilters/` leaves of kind `Pfilter` (new-format particle-filter evals,
/// M3.3). Each is one `(model × config × params × seed)` standalone eval —
/// a single leaf, no grid.
pub fn walk_pfilter_leaves(root: &Path) -> Vec<Leaf> {
    walk_records(&root.join("pfilters"))
        .into_iter()
        .filter(|(_, r)| r.kind == ArtifactKind::Pfilter)
        .map(|(dir, record)| Leaf { dir, record })
        .collect()
}

/// New-format pfilter evals whose `run_id` hex matches `prefix` (for
/// `show`/`cat` prefix resolution alongside `resolve_sim_prefix`).
pub fn resolve_pfilter_prefix(root: &Path, prefix: &str) -> Vec<Leaf> {
    resolve_prefix_indexed(root, ArtifactKind::Pfilter, prefix, walk_pfilter_leaves)
}

/// All `surveys/` leaves of kind `Survey` (new-format likelihood-landscape
/// surveys, M3.3). Each is one `(model × config × box × seed)` LHS landscape —
/// a single leaf, the N points are within it (not an axis).
pub fn walk_survey_leaves(root: &Path) -> Vec<Leaf> {
    walk_records(&root.join("surveys"))
        .into_iter()
        .filter(|(_, r)| r.kind == ArtifactKind::Survey)
        .map(|(dir, record)| Leaf { dir, record })
        .collect()
}

/// New-format surveys whose `run_id` hex matches `prefix` (for `show`/`cat`
/// prefix resolution alongside `resolve_sim_prefix`).
pub fn resolve_survey_prefix(root: &Path, prefix: &str) -> Vec<Leaf> {
    resolve_prefix_indexed(root, ArtifactKind::Survey, prefix, walk_survey_leaves)
}
