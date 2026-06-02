//! `camdl list`, `camdl show`, `camdl cat` — browse the content-addressable
//! store written by `camdl simulate --cas` and `camdl batch run`.
//!
//! All three walk `./results/sims/` by default. For alpha, walk is
//! unindexed — fast enough for thousands of runs. A persistent index
//! can be added later if needed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use owo_colors::OwoColorize;

use crate::cas_read;
use crate::run_meta::{Run, RunKind};
use crate::util::fmt_relative_time;

// ── Entry types ──────────────────────────────────────────────────────────────

/// A new-format (`runid::RunRecord`) simulate leaf, prepared for the `list`
/// display. The kind-Sim filter happens in [`cas_read::walk_sim_leaves`].
struct SimRow {
    leaf: cas_read::Leaf,
    /// Path relative to the current working directory, copy-paste ready.
    rel_path: String,
    /// When the run was written (from `provenance.created_at`; falls back to
    /// filesystem mtime).
    created: SystemTime,
}

/// Discover the new-format sim leaves under `root/sims/` for `list`.
fn discover_sim_rows(root: &str) -> Vec<SimRow> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cas_read::walk_sim_leaves(Path::new(root))
        .into_iter()
        .map(|leaf| {
            let created = leaf
                .record
                .provenance
                .created_at
                .as_deref()
                .and_then(parse_iso8601)
                .unwrap_or_else(|| {
                    std::fs::metadata(&leaf.dir)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH)
                });
            let rel_path = pathdiff_str(&leaf.dir, &cwd);
            SimRow { leaf, rel_path, created }
        })
        .collect()
}

/// A new-format pfilter-eval leaf, prepared for the `list` display.
struct PfilterRow {
    leaf: cas_read::Leaf,
    rel_path: String,
    created: SystemTime,
}

/// Discover the new-format pfilter leaves under `root/pfilters/` for `list`.
fn discover_pfilter_rows(root: &str) -> Vec<PfilterRow> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cas_read::walk_pfilter_leaves(Path::new(root))
        .into_iter()
        .map(|leaf| {
            let created = leaf.record.provenance.created_at.as_deref()
                .and_then(parse_iso8601)
                .unwrap_or_else(|| {
                    std::fs::metadata(&leaf.dir).and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH)
                });
            let rel_path = pathdiff_str(&leaf.dir, &cwd);
            PfilterRow { leaf, rel_path, created }
        })
        .collect()
}

/// Shared preamble for the **legacy** `run_meta::Run` kinds (fit/profile/
/// survey): read `run.json` and derive the display time + cwd-relative path.
/// Returns `None` when the directory isn't a (legacy) run.
///
/// M3-DELETION-BOUND (gh#147): the transitional reader dispatches new-format
/// `sims/` through [`cas_read`] and the legacy kinds through this path. When
/// M3 migrates the fit/profile/survey *writers* to `RunRecord`, delete this
/// helper and all `discover_fits`/`discover_profiles`/`discover_surveys` /
/// `ResolvedRun` machinery in the same change — the generic walker subsumes
/// them. The dual path is debt with a due date, not a keeper.
fn load_run_common(dir: &Path, cwd: &Path) -> Option<(Run, SystemTime, String)> {
    let run = Run::read(dir).ok()?;
    let created = parse_iso8601(&run.created_at)
        .unwrap_or_else(|| std::fs::metadata(dir)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH));
    let rel_path = pathdiff_str(dir, cwd);
    Some((run, created, rel_path))
}

// ── cmd_list ─────────────────────────────────────────────────────────────────

/// `--kind` filter: which of sims / fits / profiles / surveys to
/// surface. `All` is the default and includes all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindFilter { Sim, Fit, Profile, Pfilter, Survey, All }

impl KindFilter {
    fn includes_sims(self)     -> bool { matches!(self, Self::Sim     | Self::All) }
    fn includes_fits(self)     -> bool { matches!(self, Self::Fit     | Self::All) }
    fn includes_profiles(self) -> bool { matches!(self, Self::Profile | Self::All) }
    fn includes_pfilters(self) -> bool { matches!(self, Self::Pfilter | Self::All) }
    fn includes_surveys(self)  -> bool { matches!(self, Self::Survey  | Self::All) }
}

pub fn cmd_list(a: &crate::args::ListArgs) {
    // --parent=HASH: enumerate the grid-point × start runs of one
    // specific profile. Takes precedence over the default sim/fit
    // enumeration because it's a more specific request; the other
    // filters (since, limit, format) still apply.
    if let Some(parent_hash) = a.parent.as_ref() {
        list_profile_children(&a.root.to_string_lossy(), parent_hash, a);
        return;
    }

    let root = a.root.to_string_lossy();
    let filter_since: Option<std::time::Duration> = a.since.as_ref().map(|d| d.0);
    let filter_kind = match a.kind.as_str() {
        "sim" | "simulate"      => KindFilter::Sim,
        "fit"                   => KindFilter::Fit,
        "profile" | "profiles"  => KindFilter::Profile,
        "pfilter" | "pfilters"  => KindFilter::Pfilter,
        "survey" | "surveys"    => KindFilter::Survey,
        _                       => KindFilter::All,
    };
    let format_json = a.format.as_deref() == Some("json");

    let runs = if !filter_kind.includes_sims() {
        Vec::new()
    } else {
        discover_sim_rows(&root)
    };
    let fits = if !filter_kind.includes_fits() {
        Vec::new()
    } else {
        discover_fits(&root).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
    };
    let profiles = if !filter_kind.includes_profiles() {
        Vec::new()
    } else {
        discover_profiles(&root).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
    };
    let surveys = if !filter_kind.includes_surveys() {
        Vec::new()
    } else {
        discover_surveys(&root).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
    };
    let pfilters = if !filter_kind.includes_pfilters() {
        Vec::new()
    } else {
        discover_pfilter_rows(&root)
    };

    let now = SystemTime::now();
    let mut filtered_runs: Vec<SimRow> = runs.into_iter()
        .filter(|r| a.model.as_deref().is_none_or(|m| r.leaf.level_label("model").contains(m)))
        .filter(|r| a.scenario.as_deref().is_none_or(|s| r.leaf.level_label("scenario") == s))
        .filter(|r| match filter_since {
            Some(dur) => now.duration_since(r.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_runs.sort_by(|x, y| y.created.cmp(&x.created));

    let mut filtered_fits: Vec<FitEntry> = fits.into_iter()
        .filter(|f| a.model.as_deref().is_none_or(|m| f.meta.model.contains(m)))
        .filter(|_| a.scenario.is_none())
        .filter(|f| match filter_since {
            Some(dur) => now.duration_since(f.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_fits.sort_by(|x, y| y.created.cmp(&x.created));

    let mut filtered_profiles: Vec<ProfileEntry> = profiles.into_iter()
        .filter(|p| a.model.as_deref().is_none_or(|m| p.model.contains(m)))
        .filter(|_| a.scenario.is_none())
        .filter(|p| match filter_since {
            Some(dur) => now.duration_since(p.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_profiles.sort_by(|x, y| y.created.cmp(&x.created));

    let mut filtered_surveys: Vec<SurveyEntry> = surveys.into_iter()
        .filter(|s| a.model.as_deref().is_none_or(|m| s.model.contains(m)))
        .filter(|_| a.scenario.is_none())
        .filter(|s| match filter_since {
            Some(dur) => now.duration_since(s.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_surveys.sort_by(|x, y| y.created.cmp(&x.created));

    let mut filtered_pfilters: Vec<PfilterRow> = pfilters.into_iter()
        .filter(|p| a.model.as_deref().is_none_or(|m| p.leaf.level_label("model").contains(m)))
        .filter(|_| a.scenario.is_none())
        .filter(|p| match filter_since {
            Some(dur) => now.duration_since(p.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_pfilters.sort_by(|x, y| y.created.cmp(&x.created));

    if !a.all {
        filtered_runs.truncate(a.limit);
        filtered_fits.truncate(a.limit);
        filtered_profiles.truncate(a.limit);
        filtered_surveys.truncate(a.limit);
        filtered_pfilters.truncate(a.limit);
    }

    if format_json {
        print_sim_json(&filtered_runs);
        print_fits_json(&filtered_fits);
        print_profiles_json(&filtered_profiles);
        print_surveys_json(&filtered_surveys);
        print_pfilter_json(&filtered_pfilters);
    } else {
        let any_other = !filtered_fits.is_empty()
            || !filtered_profiles.is_empty()
            || !filtered_surveys.is_empty()
            || !filtered_pfilters.is_empty();
        if !filtered_fits.is_empty() {
            eprintln!("{}", "fits".bold());
            print_fits_table(&filtered_fits, now);
            eprintln!();
        }
        if !filtered_profiles.is_empty() {
            eprintln!("{}", "profiles".bold());
            print_profiles_table(&filtered_profiles, now);
            eprintln!();
        }
        if !filtered_surveys.is_empty() {
            eprintln!("{}", "surveys".bold());
            print_surveys_table(&filtered_surveys, now);
            eprintln!();
        }
        if !filtered_pfilters.is_empty() {
            eprintln!("{}", "pfilters".bold());
            print_pfilter_table(&filtered_pfilters, now);
            eprintln!();
        }
        if !filtered_runs.is_empty() || !any_other {
            if any_other { eprintln!("{}", "sims".bold()); }
            print_sim_table(&filtered_runs, now);
        }
    }
}

/// Enumerate the (grid point × seed × start) leaves of one profile, identified
/// by its profile-base hash prefix (the `profile` level). Walks the new-format
/// `ProfilePoint` leaves under `<root>/profiles/` and prints those whose base
/// hash matches, in (point, seed, start) order.
fn list_profile_children(
    root: &str,
    parent_hash_prefix: &str,
    a: &crate::args::ListArgs,
) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut matches: Vec<cas_read::Leaf> = cas_read::walk_profile_leaves(Path::new(root))
        .into_iter()
        .filter(|leaf| leaf.record.levels.first()
            .map(|l| l.hash.to_hex().starts_with(parent_hash_prefix))
            .unwrap_or(false))
        .collect();

    if matches.is_empty() {
        eprintln!("no profile-point runs found with profile-base hash prefix '{}'",
            parent_hash_prefix);
        return;
    }

    // Natural grid-traversal order: by (point, seed, start) level label.
    matches.sort_by(|x, y| {
        (x.level_label("point"), x.level_label("seed"), x.level_label("start"))
            .cmp(&(y.level_label("point"), y.level_label("seed"), y.level_label("start")))
    });

    let limit = if a.all { matches.len() } else { a.limit.min(matches.len()) };

    if a.format.as_deref() == Some("json") {
        let slice: Vec<&runid::RunRecord> = matches.iter().take(limit).map(|l| &l.record).collect();
        match serde_json::to_string_pretty(&slice) {
            Ok(s)  => println!("{}", s),
            Err(e) => eprintln!("json error: {}", e),
        }
        return;
    }

    eprintln!("{}", "profile-point leaves".bold());
    eprintln!("  {:<16} {:<8} {:<8} {:>14}  {}",
        "point", "seed", "start", "best_loglik", "path");
    for leaf in matches.iter().take(limit) {
        let ll = leaf.record.inputs.as_object()
            .and_then(|o| o.get("best_loglik"))
            .and_then(|v| v.as_f64())
            .map(|x| format!("{:.2}", x))
            .unwrap_or_else(|| "—".into());
        let rel = pathdiff_str(&leaf.dir, &cwd);
        eprintln!("  {:<16} {:<8} {:<8} {:>14}  {}",
            leaf.level_label("point"), leaf.level_label("seed"),
            leaf.level_label("start"), ll, rel.dimmed());
    }
    if matches.len() > limit {
        eprintln!("  ... {} more (use --all to show)", matches.len() - limit);
    }
}

// ── cmd_show ─────────────────────────────────────────────────────────────────

pub fn cmd_show(a: &crate::args::ShowArgs) {
    let root = a.root.to_string_lossy();
    match resolve_any(&root, &a.target) {
        Ok(Resolved::Sim { leaf, rel_path, created }) => show_sim_record(&leaf, &rel_path, created),
        Ok(Resolved::Fit { leaf, rel_path, created }) => show_fit_record(&leaf, &rel_path, created),
        Ok(Resolved::Profile { leaf, rel_path, created }) => show_profile_record(&leaf, &rel_path, created),
        Ok(Resolved::Pfilter { leaf, rel_path, created }) => show_pfilter_record(&leaf, &rel_path, created),
        Ok(Resolved::Legacy(r)) => show(&r),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Render a new-format (`RunRecord`) sim: the factored levels, the run_id
/// address, and provenance. Mirrors the legacy `show_simulate` layout.
fn show_sim_record(leaf: &cas_read::Leaf, rel_path: &str, created: SystemTime) {
    let rec = &leaf.record;
    println!("{}", "path".bright_black()); println!("  {}", rel_path.cyan());
    println!("{}", "kind".bright_black()); println!("  sim");
    if let Some(ref l) = rec.provenance.label {
        println!("{}", "label".bright_black()); println!("  {}", l);
    }
    println!("{}", "model".bright_black()); println!("  {}", leaf.level_label("model"));
    println!("{}", "scenario".bright_black()); println!("  {}", leaf.level_label("scenario"));
    println!("{}", "seed".bright_black()); println!("  {}", leaf.seed());
    println!("{}", "config".bright_black()); println!("  {}", leaf.level_label("config"));
    println!("{}", "run_id".bright_black()); println!("  {}", rec.run_id.to_hex().dimmed());
    println!("{}", "levels".bright_black());
    for lvl in &rec.levels {
        println!("  {:<9} {}-{}", lvl.name, lvl.label, lvl.hash.short8().dimmed());
    }
    println!("{}", "trajectory".bright_black());
    println!("  {} bytes", leaf.traj_bytes());
    println!("{}", "created".bright_black());
    println!("  {}  ({})",
        rec.provenance.created_at.as_deref().unwrap_or("?"),
        fmt_relative_time(created, SystemTime::now()));
    println!("{}", "engine".bright_black()); println!("  {}", rec.engine_version);
    println!("{}", "argv".bright_black());
    println!("  {}", rec.provenance.argv.join(" "));
}

/// Render a new-format (`RunRecord`) fit-stage leaf: the factored levels, the
/// run_id address, the `deps` (lineage), and the recorded FitStageMeta
/// `inputs` (display-only). Mirrors `show_sim_record` for the CAS fit path.
fn show_fit_record(leaf: &cas_read::Leaf, rel_path: &str, created: SystemTime) {
    let rec = &leaf.record;
    println!("{}", "path".bright_black()); println!("  {}", rel_path.cyan());
    println!("{}", "kind".bright_black()); println!("  fit_stage");
    println!("{}", "fit".bright_black()); println!("  {}", leaf.level_label("fit"));
    println!("{}", "stage".bright_black()); println!("  {}", leaf.level_label("stage"));
    println!("{}", "seed".bright_black()); println!("  {}", leaf.level_label("seed"));
    println!("{}", "run_id".bright_black()); println!("  {}", rec.run_id.to_hex().dimmed());
    println!("{}", "levels".bright_black());
    for lvl in &rec.levels {
        println!("  {:<9} {}-{}", lvl.name, lvl.label, lvl.hash.short8().dimmed());
    }
    if !rec.deps.is_empty() {
        println!("{}", "deps".bright_black());
        for d in &rec.deps {
            println!("  {} {} ({})", d.run_id.short8().dimmed(), d.artifact, d.digest.short8().dimmed());
        }
    }
    // FitStageMeta-equivalent recorded in `inputs` (display-only, never hashed).
    if let Some(obj) = rec.inputs.as_object() {
        for key in ["method", "backend", "n_chains", "best_chain", "best_loglik"] {
            if let Some(v) = obj.get(key) {
                println!("{}", key.bright_black()); println!("  {}", v);
            }
        }
    }
    println!("{}", "created".bright_black());
    println!("  {}  ({})",
        rec.provenance.created_at.as_deref().unwrap_or("?"),
        fmt_relative_time(created, SystemTime::now()));
    println!("{}", "engine".bright_black()); println!("  {}", rec.engine_version);
}

/// Render a new-format (`RunRecord`) profile-point leaf: the five factored
/// levels (`profile`/`point`/`stage`/`seed`/`start`), the run_id address, the
/// `--label` (read from the profile-base `fit.meta.json` sidecar — its single
/// authoritative home, NOT copied per leaf), and the recorded `inputs`
/// including the per-leaf provenance (gh#83/85 parameter resolution, per-chain
/// init, suppressed-warnings waiver). Mirrors `show_fit_record`.
fn show_profile_record(leaf: &cas_read::Leaf, rel_path: &str, created: SystemTime) {
    let rec = &leaf.record;
    println!("{}", "path".bright_black()); println!("  {}", rel_path.cyan());
    println!("{}", "kind".bright_black()); println!("  profile_point");
    // The label lives once on the profile-base sidecar; walk up the leaf's
    // ancestors to the first segment carrying a `fit.meta.json`.
    if let Some(sc) = leaf.dir.ancestors().find_map(crate::run_meta::read_fit_sidecar) {
        if let Some(ref l) = sc.label {
            println!("{}", "label".bright_black()); println!("  {}", l);
        }
    }
    for lvl_name in ["profile", "point", "stage", "seed", "start"] {
        println!("{}", lvl_name.bright_black());
        println!("  {}", leaf.level_label(lvl_name));
    }
    println!("{}", "run_id".bright_black()); println!("  {}", rec.run_id.to_hex().dimmed());
    println!("{}", "levels".bright_black());
    for lvl in &rec.levels {
        println!("  {:<9} {}-{}", lvl.name, lvl.label, lvl.hash.short8().dimmed());
    }
    if !rec.deps.is_empty() {
        println!("{}", "deps".bright_black());
        for d in &rec.deps {
            println!("  {} {} ({})", d.run_id.short8().dimmed(), d.artifact, d.digest.short8().dimmed());
        }
    }
    if let Some(obj) = rec.inputs.as_object() {
        for key in ["method", "grid_point", "start", "best_loglik", "wall_time_seconds"] {
            if let Some(v) = obj.get(key) {
                println!("{}", key.bright_black()); println!("  {}", v);
            }
        }
        // Per-leaf provenance (display-only, never hashed). Surface the audit
        // trail the old per-run ProfileMeta carried so write→read→visible holds.
        if let Some(prov) = obj.get("provenance").and_then(|v| v.as_object()) {
            if let Some(pp) = prov.get("parameters_provenance").and_then(|v| v.as_object()) {
                if !pp.is_empty() {
                    println!("{}", "parameter provenance".bright_black());
                    let mut names: Vec<&String> = pp.keys().collect();
                    names.sort();
                    for n in names {
                        let src = pp[n].get("source").and_then(|v| v.as_str()).unwrap_or("?");
                        let role = pp[n].get("role").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("  {:<16} {} ({})", n, src, role);
                    }
                }
            }
            if let Some(ip) = prov.get("init_provenance") {
                if let Some(method) = ip.get("method").and_then(|v| v.as_str()) {
                    let n_chains = ip.get("chains").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                    println!("{}", "init provenance".bright_black());
                    println!("  method {} ({} chain{})", method, n_chains,
                        if n_chains == 1 { "" } else { "s" });
                }
            }
            if let Some(sw) = prov.get("suppressed_warnings").and_then(|v| v.as_array()) {
                if !sw.is_empty() {
                    let items: Vec<&str> = sw.iter().filter_map(|v| v.as_str()).collect();
                    println!("{}", "suppressed warnings".bright_black());
                    println!("  {}", items.join(", "));
                }
            }
        }
    }
    println!("{}", "created".bright_black());
    println!("  {}  ({})",
        rec.provenance.created_at.as_deref().unwrap_or("?"),
        fmt_relative_time(created, SystemTime::now()));
    println!("{}", "engine".bright_black()); println!("  {}", rec.engine_version);
}

/// Render a new-format (`RunRecord`) pfilter-eval leaf: the four factored
/// levels (`model`/`config`/`params`/`seed`), the run_id address, and the
/// recorded loglik result + scored point from `inputs`. A pfilter eval is a
/// single leaf (no grid); the `--label`, if any, is on the leaf provenance.
fn show_pfilter_record(leaf: &cas_read::Leaf, rel_path: &str, created: SystemTime) {
    let rec = &leaf.record;
    println!("{}", "path".bright_black()); println!("  {}", rel_path.cyan());
    println!("{}", "kind".bright_black()); println!("  pfilter");
    if let Some(ref l) = rec.provenance.label {
        println!("{}", "label".bright_black()); println!("  {}", l);
    }
    for lvl_name in ["model", "config", "params", "seed"] {
        println!("{}", lvl_name.bright_black());
        println!("  {}", leaf.level_label(lvl_name));
    }
    println!("{}", "run_id".bright_black()); println!("  {}", rec.run_id.to_hex().dimmed());
    println!("{}", "levels".bright_black());
    for lvl in &rec.levels {
        println!("  {:<9} {}-{}", lvl.name, lvl.label, lvl.hash.short8().dimmed());
    }
    if let Some(obj) = rec.inputs.as_object() {
        for key in ["loglik", "loglik_sd", "n_replicates", "n_particles"] {
            if let Some(v) = obj.get(key) {
                if v.is_null() { continue; }
                println!("{}", key.bright_black()); println!("  {}", v);
            }
        }
        if let Some(params) = obj.get("params").and_then(|v| v.as_array()) {
            if !params.is_empty() {
                println!("{}", "scored point".bright_black());
                for pair in params {
                    if let Some(p) = pair.as_array() {
                        if p.len() == 2 {
                            println!("  {:<16} {}",
                                p[0].as_str().unwrap_or("?"), p[1]);
                        }
                    }
                }
            }
        }
    }
    println!("{}", "created".bright_black());
    println!("  {}  ({})",
        rec.provenance.created_at.as_deref().unwrap_or("?"),
        fmt_relative_time(created, SystemTime::now()));
    println!("{}", "engine".bright_black()); println!("  {}", rec.engine_version);
}

/// Kind-agnostic show entry point. One match on `run.kind`; per-kind
/// renderers below. Adding a new `RunKind` variant gets a compiler
/// error here until a renderer is wired in.
fn show(r: &ResolvedRun) {
    match &r.run.kind {
        RunKind::Simulate(_)     => show_simulate(r),
        RunKind::Fit(_)          => show_fit(r),
        RunKind::FitStage(_)     => show_fit_stage(r),
        RunKind::Profile(_)      => show_profile_leaf(r),
        RunKind::Survey(_)       => show_survey(r),
    }
}

/// Header shared by every kind: path, kind label, optional label,
/// timing/version/argv. Keeps the per-kind renderers focused on
/// kind-specific fields.
fn show_header(r: &ResolvedRun) {
    println!("{}", "path".bright_black()); println!("  {}", r.rel_path.cyan());
    println!("{}", "kind".bright_black()); println!("  {}", kind_label(&r.run.kind));
    if let Some(ref l) = r.run.label {
        println!("{}", "label".bright_black()); println!("  {}", l);
    }
}

fn show_footer(r: &ResolvedRun) {
    println!("{}", "created".bright_black());
    println!("  {}  ({})", r.run.created_at,
        fmt_relative_time(r.created, SystemTime::now()));
    println!("{}", "version".bright_black()); println!("  {}", r.run.version);
    println!("{}", "wall time".bright_black());
    match r.run.status.wall_time_seconds() {
        Some(t) => println!("  {:.1}s", t),
        None    => println!("  (running)"),
    }
    println!("{}", "argv".bright_black());
    println!("  {}", r.run.argv.join(" "));
}

fn show_simulate(r: &ResolvedRun) {
    let RunKind::Simulate(m) = &r.run.kind else { unreachable!() };
    show_header(r);
    println!("{}", "model".bright_black()); println!("  {}", m.model);
    println!("{}", "scenario".bright_black()); println!("  {}", m.scenario);
    println!("{}", "seed".bright_black()); println!("  {}", m.seed);
    println!("{}", "backend".bright_black());
    println!("  {} (dt = {})", m.backend, m.dt);
    println!("{}", "hashes".bright_black());
    println!("  sim   {}", m.sim_hash.dimmed());
    println!("  scen  {}", m.scen_hash.dimmed());
    println!("  model {}", m.model_hash.dimmed());
    if let Some(fh) = &m.from_fit_hash {
        println!("  from-fit {}", fh.dimmed());
    }
    let traj_bytes = std::fs::metadata(r.abs_path.join("traj.tsv"))
        .map(|m| m.len()).unwrap_or(0);
    println!("{}", "trajectory".bright_black());
    println!("  {} bytes", traj_bytes);
    show_footer(r);
}

fn show_fit(r: &ResolvedRun) {
    let RunKind::Fit(m) = &r.run.kind else { unreachable!() };
    show_header(r);
    println!("{}", "model".bright_black()); println!("  {}", m.model);
    println!("{}", "fit.toml".bright_black()); println!("  {}", m.fit_toml_path);
    println!("{}", "estimate".bright_black()); println!("  {}", m.estimated.join(", "));
    if !m.fixed.is_empty() {
        let mut fx: Vec<_> = m.fixed.iter().collect();
        fx.sort_by_key(|(k, _)| k.to_string());
        let items: Vec<String> = fx.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        println!("{}", "fixed".bright_black()); println!("  {}", items.join(", "));
    }
    println!("{}", "stages".bright_black());
    println!("  {}", m.stages_declared.join(", "));
    println!("{}", "hashes".bright_black());
    println!("  fit      {}", r.run.hash.dimmed());
    println!("  model    {}", m.model_hash.dimmed());
    println!("  fit.toml {}", m.fit_toml_hash.dimmed());
    show_footer(r);
}

fn show_fit_stage(r: &ResolvedRun) {
    let RunKind::FitStage(m) = &r.run.kind else { unreachable!() };
    show_header(r);
    println!("{}", "stage".bright_black());
    println!("  {} (method: {})", m.stage, m.method);
    println!("{}", "seed".bright_black()); println!("  {}", m.seed);
    println!("{}", "chains".bright_black()); println!("  {}", m.n_chains);
    if let Some(ll) = m.best_loglik {
        let chain = m.best_chain.map(|c| format!(" (chain {})", c + 1)).unwrap_or_default();
        println!("{}", "best loglik".bright_black());
        println!("  {:.2}{}", ll, chain);
    }
    if !m.algorithm.is_null() {
        println!("{}", "algorithm".bright_black());
        let pretty = serde_json::to_string_pretty(&m.algorithm).unwrap_or_default();
        for line in pretty.lines() { println!("  {}", line.dimmed()); }
    }
    if let Some(sf) = &m.starts_from {
        let h = sf.stage_hash.as_deref().unwrap_or("?");
        let short = &h[..h.len().min(16)];
        println!("{}", "starts from".bright_black());
        println!("  {} ({})", sf.stage, short.dimmed());
    }
    if let Some(ref hash) = m.parent_profile_hash {
        let short = &hash[..hash.len().min(16)];
        println!("{}", "parent profile".bright_black());
        println!("  {}", short.dimmed());
        if let (Some(pi), Some(si)) = (m.profile_point_idx, m.profile_start_idx) {
            println!("  point {} / start {}", pi, si);
        }
    }
    if let Some(ref df) = m.derived_from {
        println!("{}", "derived from".bright_black());
        println!("  {}", df);
    }
    println!("{}", "hashes".bright_black());
    println!("  stage {}", r.run.hash.dimmed());
    println!("  fit   {}", m.fit_hash.dimmed());
    show_footer(r);
}

fn show_profile_leaf(r: &ResolvedRun) {
    let RunKind::Profile(m) = &r.run.kind else { unreachable!() };
    show_header(r);
    println!("{}", "model".bright_black()); println!("  {}", m.model);
    println!("{}", "focal params".bright_black());
    println!("  {}", m.focal_params.join(", "));
    println!("{}", "grid".bright_black());
    for axis in &m.grid {
        let n = axis.values.len();
        let preview = if n <= 6 {
            axis.values.iter().map(|v| format!("{}", v)).collect::<Vec<_>>().join(", ")
        } else {
            let head: Vec<String> = axis.values.iter().take(3).map(|v| format!("{}", v)).collect();
            let tail: Vec<String> = axis.values.iter().rev().take(2).rev().map(|v| format!("{}", v)).collect();
            format!("{}, …, {}", head.join(", "), tail.join(", "))
        };
        println!("  {}: {} values [{}]", axis.param, n, preview);
    }
    println!("{}", "starts".bright_black()); println!("  {} per grid point", m.n_starts);
    println!("{}", "total jobs".bright_black()); println!("  {}", m.total_jobs);
    println!("{}", "seed".bright_black()); println!("  {}", m.seed_base);
    let profile_tsv = r.abs_path.join("profile.tsv");
    if profile_tsv.exists() {
        let bytes = std::fs::metadata(&profile_tsv).map(|m| m.len()).unwrap_or(0);
        println!("{}", "rollup".bright_black());
        println!("  profile.tsv ({} bytes)", bytes);
    }
    println!("{}", "hashes".bright_black());
    println!("  profile        {}", r.run.hash.dimmed());
    println!("  model          {}", m.model_hash.dimmed());
    println!("  if2 config     {}", m.if2_config_hash.dimmed());
    println!("  base params    {}", m.base_params_hash.dimmed());
    show_footer(r);
}

fn show_survey(r: &ResolvedRun) {
    let RunKind::Survey(m) = &r.run.kind else { unreachable!() };
    show_header(r);
    println!("{}", "model".bright_black()); println!("  {}", m.model);
    println!("{}", "estimated".bright_black());
    println!("  {}", m.estimated.join(", "));
    println!("{}", "bounds".bright_black());
    let mut bounds: Vec<(&String, &(f64, f64))> = m.bounds.iter().collect();
    bounds.sort_by(|a, b| a.0.cmp(b.0));
    for (name, (lo, hi)) in &bounds {
        println!("  {}: [{}, {}]", name, lo, hi);
    }
    if !m.fixed.is_empty() {
        let mut fx: Vec<(&String, &f64)> = m.fixed.iter().collect();
        fx.sort_by(|a, b| a.0.cmp(b.0));
        let items: Vec<String> = fx.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        println!("{}", "fixed".bright_black()); println!("  {}", items.join(", "));
    }
    if let Some(ref s) = m.scenario {
        println!("{}", "scenario".bright_black()); println!("  {}", s);
    }
    println!("{}", "n_points".bright_black()); println!("  {}", m.n_points);
    println!("{}", "eval".bright_black());
    match m.eval_method {
        crate::run_meta::SurveyEvalMethod::Pfilter =>
            println!("  pfilter ({} particles × {} replicates)",
                m.eval_particles, m.eval_replicates),
        crate::run_meta::SurveyEvalMethod::Simulate =>
            println!("  simulate (single trajectory per point)"),
        // SurveyMeta only stores resolved methods — `Auto` is
        // resolved in `cmd_survey` before persistence.
        crate::run_meta::SurveyEvalMethod::Auto =>
            println!("  auto (unresolved — bug; SurveyMeta should never store Auto)"),
    }
    println!("{}", "seed".bright_black()); println!("  {}", m.seed);
    let landscape = r.abs_path.join("landscape.tsv");
    if landscape.exists() {
        let bytes = std::fs::metadata(&landscape).map(|m| m.len()).unwrap_or(0);
        println!("{}", "landscape".bright_black());
        println!("  landscape.tsv ({} bytes)", bytes);
    }
    let summary = r.abs_path.join("summary.json");
    if summary.exists() {
        // Inline the top-loglik / SE-quartile fields if available.
        if let Ok(s) = std::fs::read_to_string(&summary) {
            if let Ok(j) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(top) = j.get("top_loglik").and_then(|v| v.as_f64()) {
                    println!("{}", "top loglik".bright_black());
                    println!("  {:.2}", top);
                }
                if let Some(se_q) = j.get("loglik_se_quartiles") {
                    println!("{}", "loglik_se quartiles".bright_black());
                    println!("  {}", se_q);
                }
            }
        }
    }
    let html = r.abs_path.join("landscape.html");
    if html.exists() {
        let bytes = std::fs::metadata(&html).map(|m| m.len()).unwrap_or(0);
        println!("{}", "rendered".bright_black());
        println!("  landscape.html ({} bytes)", bytes);
    }
    println!("{}", "hashes".bright_black());
    println!("  survey {}", r.run.hash.dimmed());
    println!("  model  {}", m.model_hash.dimmed());
    show_footer(r);
}

// ── cmd_cat ──────────────────────────────────────────────────────────────────

pub fn cmd_cat(a: &crate::args::CatArgs) {
    let root = a.root.to_string_lossy();
    let resolved = resolve_any(&root, &a.target).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });

    use std::io::Write as _;

    // New-format sim: emit traj.tsv (or an obs stream) from the leaf dir.
    let resolved = match resolved {
        Resolved::Sim { leaf, rel_path, .. } => {
            let bytes = if let Some(ref stream) = a.stream {
                let path = find_obs_stream(&leaf.dir, stream).unwrap_or_else(|| {
                    eprintln!("error: no observation stream '{}' in {}", stream, rel_path);
                    std::process::exit(1);
                });
                std::fs::read(&path).unwrap_or_else(|e| {
                    eprintln!("error reading {}: {}", path.display(), e); std::process::exit(1);
                })
            } else {
                std::fs::read(leaf.dir.join("traj.tsv")).unwrap_or_else(|e| {
                    eprintln!("error reading traj.tsv: {}", e); std::process::exit(1);
                })
            };
            let _ = std::io::stdout().write_all(&bytes);
            return;
        }
        // New-format fit stage: default to the θ̂ summary; `--stream NAME`
        // cats a named file from the leaf (e.g. `draws.tsv`,
        // `chain_1/trace.tsv`).
        Resolved::Fit { leaf, rel_path, .. } => {
            let name = a.stream.as_deref().unwrap_or("fit_state.toml");
            let path = leaf.dir.join(name);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                eprintln!("error reading {} in {}: {}", name, rel_path, e);
                std::process::exit(1);
            });
            let _ = std::io::stdout().write_all(&bytes);
            return;
        }
        // New-format profile point: default to the per-(point, seed, start)
        // `mle.toml`; `--stream NAME` cats a named file from the leaf.
        Resolved::Profile { leaf, rel_path, .. } => {
            let name = a.stream.as_deref().unwrap_or("mle.toml");
            let path = leaf.dir.join(name);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                eprintln!("error reading {} in {}: {}", name, rel_path, e);
                std::process::exit(1);
            });
            let _ = std::io::stdout().write_all(&bytes);
            return;
        }
        // New-format pfilter eval: default to the `loglik.toml` summary;
        // `--stream NAME` cats a named saved artifact from the leaf.
        Resolved::Pfilter { leaf, rel_path, .. } => {
            let name = a.stream.as_deref().unwrap_or("loglik.toml");
            let path = leaf.dir.join(name);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                eprintln!("error reading {} in {}: {}", name, rel_path, e);
                std::process::exit(1);
            });
            let _ = std::io::stdout().write_all(&bytes);
            return;
        }
        Resolved::Legacy(r) => r,
    };

    match &resolved.run.kind {
        // Legacy sims no longer exist (sims are RunRecord), but the match
        // stays exhaustive; a path-form cat of an old sim run.json reads here.
        RunKind::Simulate(_) => {
            let bytes = if let Some(ref stream) = a.stream {
                let path = find_obs_stream(&resolved.abs_path, stream).unwrap_or_else(|| {
                    eprintln!("error: no observation stream '{}' in {}", stream, resolved.rel_path);
                    std::process::exit(1);
                });
                std::fs::read(&path).unwrap_or_else(|e| {
                    eprintln!("error reading {}: {}", path.display(), e); std::process::exit(1);
                })
            } else {
                std::fs::read(resolved.abs_path.join("traj.tsv")).unwrap_or_else(|e| {
                    eprintln!("error reading traj.tsv: {}", e); std::process::exit(1);
                })
            };
            let _ = std::io::stdout().write_all(&bytes);
        }
        RunKind::Profile(_) => {
            let profile_tsv = resolved.abs_path.join("profile.tsv");
            if !profile_tsv.exists() {
                eprintln!("error: 'camdl cat' on a profile leaf expects \
                    profile.tsv, which has not been written yet for {}.",
                    resolved.rel_path);
                std::process::exit(1);
            }
            let bytes = std::fs::read(&profile_tsv).unwrap_or_else(|e| {
                eprintln!("error reading {}: {}", profile_tsv.display(), e);
                std::process::exit(1);
            });
            let _ = std::io::stdout().write_all(&bytes);
        }
        RunKind::Fit(_) => {
            eprintln!("error: 'camdl cat' on a fit has no single-file target.\n  \
                       {} is a fit directory. For stage output, pass the stage\n  \
                       path directly, e.g. `camdl cat {}/real/fit_<seed>/<stage>/mle_params.toml`.",
                      resolved.rel_path, resolved.rel_path);
            std::process::exit(1);
        }
        RunKind::FitStage(_) => {
            eprintln!("error: 'camdl cat' on a fit-stage has no canonical \
                       single-file target. {} is a stage directory; pass a \
                       specific file path (mle_params.toml, draws.tsv, …) \
                       directly.",
                      resolved.rel_path);
            std::process::exit(1);
        }
        RunKind::Survey(_) => {
            let landscape = resolved.abs_path.join("landscape.tsv");
            if !landscape.exists() {
                eprintln!("error: 'camdl cat' on a survey expects \
                    landscape.tsv, which has not been written yet for {}.",
                    resolved.rel_path);
                std::process::exit(1);
            }
            let bytes = std::fs::read(&landscape).unwrap_or_else(|e| {
                eprintln!("error reading {}: {}", landscape.display(), e);
                std::process::exit(1);
            });
            let _ = std::io::stdout().write_all(&bytes);
        }
    }
}

/// Locate `<sim_dir>/obs/<obs_subdir>/<stream>.tsv`, taking the first
/// match across `obs_subdir/`. Returns `None` if no stream by that
/// name exists.
fn find_obs_stream(sim_dir: &Path, stream: &str) -> Option<PathBuf> {
    let obs_root = sim_dir.join("obs");
    if !obs_root.exists() { return None; }
    let entries = std::fs::read_dir(&obs_root).ok()?;
    for entry in entries.flatten() {
        let file = entry.path().join(format!("{}.tsv", stream));
        if file.exists() { return Some(file); }
    }
    None
}

// ── Internals: discovery + resolution ────────────────────────────────────────
//
// New-format `sims/` are discovered generically via [`discover_sim_rows`]
// (data-driven depth through [`cas_read`]). The legacy fit/profile/survey
// discovery below is M3-DELETION-BOUND (gh#147) — see [`load_run_common`].

/// A discovered cached fit.
#[derive(Debug, Clone)]
struct FitEntry {
    run: Run,
    meta: crate::run_meta::FitMeta,
    rel_path: String,
    created: SystemTime,
}

// ── Profile listings ─────────────────────────────────────────────────────────

/// A discovered profile, summarized from its `ProfilePoint` leaves under
/// `<root>/profiles/<base>/.../`. One entry per profile (grouped by the
/// `profile`-level base hash); carries the display fields `camdl list` needs.
#[derive(Debug, Clone)]
struct ProfileEntry {
    /// Profile-base hash (the `profile` level / `levels[0]`) — the
    /// `camdl show <hash>`/`list --parent <hash>` address for this profile.
    hash: String,
    /// User `--label` from the profile-base `fit.meta.json` sidecar (its
    /// single authoritative home; `None` when unset).
    label: Option<String>,
    /// The profile-base segment, cwd-relative.
    rel_path: String,
    /// Latest leaf's creation time.
    created: SystemTime,
    /// Display-only model path (from the profile-base sidecar).
    model: String,
    /// Comma-separated focal param names (e.g. "beta,gamma"), reconstructed
    /// from the distinct `point`-level labels.
    focal: String,
    /// Grid shape (e.g. "11×9 starts=4"), reconstructed from the distinct
    /// `point` labels (per-axis value counts) and distinct `start` levels.
    shape: String,
    /// Number of seed replicates (distinct `seed` levels).
    n_seeds: usize,
}

/// Discover profiles for `camdl list`: walk the new-format `ProfilePoint`
/// leaves under `<root>/profiles/` and fold them into one `ProfileEntry` per
/// profile, keyed by the `profile`-level base hash. The grid shape and focal
/// names are reconstructed from the leaves' `point` labels; the `--label` and
/// model come from the profile-base `fit.meta.json` sidecar.
fn discover_profiles(root: &str) -> Result<Vec<ProfileEntry>, String> {
    use std::collections::{HashMap, HashSet};
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // One `ProfileEntry` per profile: group new-format `ProfilePoint` leaves
    // by their `profile`-level (base) hash. Each leaf is one (point × seed ×
    // start) cell; the entry summarizes the cells.
    let mut groups: HashMap<String, Vec<cas_read::Leaf>> = HashMap::new();
    for leaf in cas_read::walk_profile_leaves(Path::new(root)) {
        let base = leaf.record.levels.first().map(|l| l.hash.to_hex()).unwrap_or_default();
        groups.entry(base).or_default().push(leaf);
    }

    let mut out = Vec::new();
    for (base_hash, leaves) in groups {
        let mut point_labels: Vec<String> = Vec::new();
        let mut seed_hashes: HashSet<String> = HashSet::new();
        let mut start_hashes: HashSet<String> = HashSet::new();
        let mut created = SystemTime::UNIX_EPOCH;
        let mut base_seg: Option<PathBuf> = None;
        let level_hash = |leaf: &cas_read::Leaf, name: &str| -> Option<String> {
            leaf.record.levels.iter().find(|l| l.name == name).map(|l| l.hash.to_hex())
        };
        for leaf in &leaves {
            point_labels.push(leaf.level_label("point").to_string());
            if let Some(h) = level_hash(leaf, "seed") { seed_hashes.insert(h); }
            if let Some(h) = level_hash(leaf, "start") { start_hashes.insert(h); }
            let c = leaf_created(leaf);
            if c > created { created = c; }
            // The profile-base segment is four levels up (start/seed/stage/
            // point → base); used for the sidecar (label/model) and rel path.
            if base_seg.is_none() {
                base_seg = leaf.dir.ancestors().nth(4).map(|p| p.to_path_buf());
            }
        }
        let sidecar = base_seg.as_deref().and_then(crate::run_meta::read_fit_sidecar);
        let label = sidecar.as_ref().and_then(|s| s.label.clone());
        let model = sidecar.as_ref()
            .map(|s| s.model_path.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "?".to_string());
        let (focal, shape) = summarize_grid(&point_labels, start_hashes.len());
        let rel_path = base_seg.as_deref()
            .map(|p| pathdiff_str(p, &cwd))
            .unwrap_or_default();
        out.push(ProfileEntry {
            hash: base_hash, label, rel_path, created, model, focal, shape,
            n_seeds: seed_hashes.len(),
        });
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(out)
}

/// Reconstruct a profile's focal-param list + grid shape from the distinct
/// `point`-level labels. Each label is `name=val[__name2=val2]` (written by
/// `profile_cas::resolve_profile_point`); the per-axis distinct-value counts
/// give the grid dims. E.g. an 11×9 grid with 4 starts → `("beta,gamma",
/// "11×9 starts=4")`.
fn summarize_grid(point_labels: &[String], n_starts: usize) -> (String, String) {
    use std::collections::{HashMap, HashSet};
    let mut names: Vec<String> = Vec::new();
    let mut values: HashMap<String, HashSet<String>> = HashMap::new();
    let mut distinct_points: HashSet<&str> = HashSet::new();
    for lbl in point_labels {
        distinct_points.insert(lbl.as_str());
        for pair in lbl.split("__") {
            if let Some((name, val)) = pair.split_once('=') {
                if !names.iter().any(|n| n == name) { names.push(name.to_string()); }
                values.entry(name.to_string()).or_default().insert(val.to_string());
            }
        }
    }
    let focal = names.join(",");
    let shape = if names.is_empty() {
        format!("{} pts starts={}", distinct_points.len(), n_starts)
    } else {
        let dims: Vec<String> = names.iter()
            .map(|n| values.get(n).map(|s| s.len()).unwrap_or(0).to_string())
            .collect();
        format!("{} starts={}", dims.join("×"), n_starts)
    };
    (focal, shape)
}

// ── Survey listings ──────────────────────────────────────────────────────────

/// One discovered survey run. Surveys live at
/// `<root>/surveys/<stem>-<hash[:8]>/` with a `run.json` of kind
/// `Survey(SurveyMeta)`. Display-only fields surfaced in `camdl list`.
#[derive(Debug, Clone)]
struct SurveyEntry {
    run: Run,
    rel_path: String,
    created: SystemTime,
    /// Display model path (from `SurveyMeta.model`).
    model: String,
    /// Comma-separated estimated parameter names.
    estimated: String,
    /// "pfilter Px×Rk" or "simulate".
    eval: String,
    /// Number of LHS points.
    n_points: usize,
    /// Best loglik in `landscape.tsv`. `None` when the artifact is
    /// missing (interrupted run).
    top_loglik: Option<f64>,
}

/// Walk `<root>/surveys/` one level deep. Each child dir is a
/// survey-run directory.
fn discover_surveys(root: &str) -> Result<Vec<SurveyEntry>, String> {
    let surveys_root = Path::new(root).join("surveys");
    if !surveys_root.exists() { return Ok(Vec::new()); }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let entries = std::fs::read_dir(&surveys_root)
        .map_err(|e| format!("cannot read {}: {}", surveys_root.display(), e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() { continue; }
        let Some((run, created, rel_path)) = load_run_common(&dir, &cwd) else { continue; };
        let RunKind::Survey(m) = &run.kind else { continue };
        let eval = match m.eval_method {
            crate::run_meta::SurveyEvalMethod::Pfilter =>
                format!("pfilter {}p×{}r", m.eval_particles, m.eval_replicates),
            crate::run_meta::SurveyEvalMethod::Simulate => "simulate".to_string(),
            crate::run_meta::SurveyEvalMethod::Auto => "auto".to_string(),
        };
        // Read top loglik from summary.json when present.
        let top_loglik = std::fs::read_to_string(dir.join("summary.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|j| j.get("top_loglik").and_then(|v| v.as_f64()));
        out.push(SurveyEntry {
            model: m.model.clone(),
            estimated: m.estimated.join(","),
            eval,
            n_points: m.n_points,
            top_loglik,
            run, rel_path, created,
        });
    }
    Ok(out)
}

fn print_surveys_table(surveys: &[SurveyEntry], now: SystemTime) {
    let mut t = comfy_table::Table::new();
    t.set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    t.set_header(vec!["model", "estimate", "n_points", "eval", "top_loglik", "age", "path"]);
    for s in surveys {
        let age = fmt_relative_time(s.created, now);
        let ll = s.top_loglik
            .map(|x| format!("{:.2}", x))
            .unwrap_or_else(|| "—".into());
        t.add_row(vec![
            s.model.clone(),
            s.estimated.clone(),
            s.n_points.to_string(),
            s.eval.clone(),
            ll,
            age,
            s.rel_path.clone(),
        ]);
    }
    println!("{t}");
}

fn print_surveys_json(surveys: &[SurveyEntry]) {
    let runs: Vec<&Run> = surveys.iter().map(|s| &s.run).collect();
    match serde_json::to_string_pretty(&runs) {
        Ok(s) => println!("{}", s),
        Err(e) => eprintln!("json error: {}", e),
    }
}

/// Walk `root/fits/` one level deep — each immediate child is a fit
/// directory (`<stem>-<hash[:8]>/`). Stage-level run.json records live
/// deeper and are not surfaced by `camdl list`.
///
/// Implementation: delegates to `fit_tree::walk_fits_root` for
/// canonical fit-dir discovery, then layers on the per-entry display
/// metadata (`rel_path`, `created` mtime) browse needs that the
/// canonical walker doesn't carry.
fn discover_fits(root: &str) -> Result<Vec<FitEntry>, String> {
    let fits_dir = Path::new(root).join("fits");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let entries = crate::fit::fit_tree::walk_fits_root(&fits_dir)
        .map_err(|e| format!("cannot read {}: {}", fits_dir.display(), e))?;
    Ok(entries
        .into_iter()
        .map(|e| {
            // `walk_fits_root` already parsed run.json; reuse its
            // `run` rather than re-reading the file. `created` and
            // `rel_path` are display-only and computed from the
            // already-parsed `run.created_at` plus the dir path.
            let created = parse_iso8601(&e.run.created_at)
                .unwrap_or_else(|| std::fs::metadata(&e.fit_dir)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH));
            let rel_path = pathdiff_str(&e.fit_dir, &cwd);
            FitEntry { run: e.run, meta: e.fit_meta, rel_path, created }
        })
        .collect())
}

/// One resolved run, kind-agnostic. Kind-specific data lives inside
/// `run.kind` (a `RunKind` tagged union); renderers dispatch on the
/// variant rather than carrying a parallel enum here. This single
/// shape applies to every `RunKind` — sim, fit, fit-stage, profile,
/// replicate-set — so `camdl show` and `camdl cat` can route
/// uniformly.
#[derive(Debug, Clone)]
struct ResolvedRun {
    run: Run,
    abs_path: PathBuf,
    rel_path: String,
    created: SystemTime,
}

/// A resolved run: a new-format sim (`RunRecord`) or a legacy kind (`Run`).
/// The transitional reader resolves across both during M2→M3.
#[derive(Debug)]
enum Resolved {
    Sim { leaf: cas_read::Leaf, rel_path: String, created: SystemTime },
    /// New-format (`RunRecord`) fit-stage leaf under `fits/` (M3.2).
    Fit { leaf: cas_read::Leaf, rel_path: String, created: SystemTime },
    /// New-format (`RunRecord`) profile-point leaf under `profiles/` (M3.3).
    Profile { leaf: cas_read::Leaf, rel_path: String, created: SystemTime },
    /// New-format (`RunRecord`) pfilter-eval leaf under `pfilters/` (M3.3).
    Pfilter { leaf: cas_read::Leaf, rel_path: String, created: SystemTime },
    Legacy(ResolvedRun),
}

/// Resolve a user-supplied key to a single run, spanning both the new-format
/// `sims/` (matched on `run_id` hex prefix) and the legacy fit/profile/survey
/// subtrees (matched on `Run.hash` prefix). Accepts either a path to a
/// `run.json`-containing directory (new or legacy format), or a hash prefix
/// where `{prefix}/{scenario}[/{seed_N}]` narrows sims further. An ambiguous
/// prefix errors, listing all candidates with their kinds.
fn resolve_any(root: &str, key: &str) -> Result<Resolved, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Path form: read run.json directly — try the new RunRecord first, then
    // fall back to a legacy Run.
    let as_path = Path::new(key);
    if as_path.is_dir() && as_path.join("run.json").exists() {
        if let Ok(bytes) = std::fs::read(as_path.join("run.json")) {
            if let Ok(rec) = serde_json::from_slice::<runid::RunRecord>(&bytes) {
                let kind = rec.kind;
                let leaf = cas_read::Leaf { dir: as_path.to_path_buf(), record: rec };
                let created = leaf_created(&leaf);
                let rel_path = pathdiff_str(as_path, &cwd);
                return Ok(match kind {
                    runid::ArtifactKind::FitStage => Resolved::Fit { leaf, rel_path, created },
                    runid::ArtifactKind::ProfilePoint => Resolved::Profile { leaf, rel_path, created },
                    runid::ArtifactKind::Pfilter => Resolved::Pfilter { leaf, rel_path, created },
                    _ => Resolved::Sim { leaf, rel_path, created },
                });
            }
        }
        let (run, created, rel_path) = load_run_common(as_path, &cwd)
            .ok_or_else(|| format!("could not read run.json at {}", as_path.display()))?;
        return Ok(Resolved::Legacy(ResolvedRun {
            run, rel_path, created, abs_path: as_path.to_path_buf(),
        }));
    }

    // Hash-prefix form.
    let parts: Vec<&str> = key.split('/').collect();
    let hash_prefix = parts[0];
    let scen_filter = parts.get(1).copied();
    let seed_filter: Option<u64> = parts.get(2)
        .and_then(|s| s.strip_prefix("seed_"))
        .or_else(|| parts.get(2).copied())
        .and_then(|s| s.parse().ok());

    // New-format sims: match the run_id hex prefix, narrow by scenario/seed.
    let mut sim_matches: Vec<(cas_read::Leaf, String, SystemTime)> = Vec::new();
    for leaf in cas_read::resolve_sim_prefix(Path::new(root), hash_prefix) {
        if scen_filter.is_some_and(|s| s != leaf.level_label("scenario")) { continue; }
        if seed_filter.is_some_and(|s| s != leaf.seed()) { continue; }
        let created = leaf_created(&leaf);
        let rel = pathdiff_str(&leaf.dir, &cwd);
        sim_matches.push((leaf, rel, created));
    }

    // New-format fit stages (M3.2): match the run_id hex prefix under fits/.
    let mut fit_matches: Vec<(cas_read::Leaf, String, SystemTime)> = Vec::new();
    for leaf in cas_read::resolve_fit_prefix(Path::new(root), hash_prefix) {
        let created = leaf_created(&leaf);
        let rel = pathdiff_str(&leaf.dir, &cwd);
        fit_matches.push((leaf, rel, created));
    }

    // New-format profile points (M3.3): match the run_id hex prefix under
    // profiles/. A profile-base prefix is the umbrella view — use `list
    // --parent <base>` for that; here a single leaf is addressed.
    let mut profile_matches: Vec<(cas_read::Leaf, String, SystemTime)> = Vec::new();
    for leaf in cas_read::resolve_profile_prefix(Path::new(root), hash_prefix) {
        let created = leaf_created(&leaf);
        let rel = pathdiff_str(&leaf.dir, &cwd);
        profile_matches.push((leaf, rel, created));
    }

    // New-format pfilter evals (M3.3): match the run_id hex prefix under
    // pfilters/.
    let mut pfilter_matches: Vec<(cas_read::Leaf, String, SystemTime)> = Vec::new();
    for leaf in cas_read::resolve_pfilter_prefix(Path::new(root), hash_prefix) {
        let created = leaf_created(&leaf);
        let rel = pathdiff_str(&leaf.dir, &cwd);
        pfilter_matches.push((leaf, rel, created));
    }

    // Legacy kinds: match Run.hash prefix under surveys. Sims/fits/profiles/
    // pfilters are content-addressed now (matched above); survey migrates
    // later in M3.3.
    let mut legacy_matches: Vec<ResolvedRun> = Vec::new();
    for top in ["surveys"] {
        let subroot = Path::new(root).join(top);
        if !subroot.exists() { continue; }
        for dir in walkdir_all(&subroot) {
            if !dir.join("run.json").exists() { continue; }
            let Some((run, created, rel_path)) = load_run_common(&dir, &cwd) else { continue; };
            if !run.hash.starts_with(hash_prefix) { continue; }
            legacy_matches.push(ResolvedRun { run, rel_path, created, abs_path: dir });
        }
    }

    match sim_matches.len() + fit_matches.len() + profile_matches.len()
        + pfilter_matches.len() + legacy_matches.len() {
        0 => Err(format!("no run matches '{}' in {}", key, root)),
        1 => {
            if let Some((leaf, rel_path, created)) = sim_matches.into_iter().next() {
                Ok(Resolved::Sim { leaf, rel_path, created })
            } else if let Some((leaf, rel_path, created)) = fit_matches.into_iter().next() {
                Ok(Resolved::Fit { leaf, rel_path, created })
            } else if let Some((leaf, rel_path, created)) = profile_matches.into_iter().next() {
                Ok(Resolved::Profile { leaf, rel_path, created })
            } else if let Some((leaf, rel_path, created)) = pfilter_matches.into_iter().next() {
                Ok(Resolved::Pfilter { leaf, rel_path, created })
            } else {
                Ok(Resolved::Legacy(legacy_matches.into_iter().next().unwrap()))
            }
        }
        n => {
            let mut msg = format!("'{}' is ambiguous, matches {} entries:\n", key, n);
            for (_, rel, _) in &sim_matches {
                msg.push_str(&format!("  {:<14} {}\n", "sim", rel));
            }
            for (_, rel, _) in &fit_matches {
                msg.push_str(&format!("  {:<14} {}\n", "fit_stage", rel));
            }
            for (_, rel, _) in &profile_matches {
                msg.push_str(&format!("  {:<14} {}\n", "profile_point", rel));
            }
            for (_, rel, _) in &pfilter_matches {
                msg.push_str(&format!("  {:<14} {}\n", "pfilter", rel));
            }
            for r in &legacy_matches {
                msg.push_str(&format!("  {:<14} {}\n", kind_label(&r.run.kind), r.rel_path));
            }
            msg.push_str("refine by appending /<scenario> and/or /<seed_N>, \
                         or pass a longer hash prefix");
            Err(msg)
        }
    }
}

/// Created-time for a new-format leaf (provenance timestamp, else dir mtime).
fn leaf_created(leaf: &cas_read::Leaf) -> SystemTime {
    leaf.record
        .provenance
        .created_at
        .as_deref()
        .and_then(parse_iso8601)
        .unwrap_or_else(|| {
            std::fs::metadata(&leaf.dir)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
}

/// Short tag for the disambiguation listing (`camdl show <ambiguous>`)
/// — same vocabulary as the `kind` discriminator in run.json.
fn kind_label(kind: &RunKind) -> &'static str {
    match kind {
        RunKind::Simulate(_)     => "sim",
        RunKind::Fit(_)          => "fit",
        RunKind::FitStage(_)     => "fit-stage",
        RunKind::Profile(_)      => "profile",
        RunKind::Survey(_)       => "survey",
    }
}

/// Find the fit-stage directory whose `run.json` has `Run.hash`
/// starting with `hash_prefix`. Walks every
/// `<root>/fits/**/run.json` file — stage-level (FitStage kind)
/// only; the top-level `Run::Fit` at the fit root is skipped.
///
/// Returns `Ok(path)` for exactly one match, `Err` on zero or
/// multiple matches (with the candidates enumerated in the
/// multiple-match error). Used by `--starts-from <hash>` to let
/// users reference a stage by git-style short hash without
/// knowing the directory layout.
pub fn resolve_stage_by_hash(root: &str, hash_prefix: &str)
    -> Result<std::path::PathBuf, String>
{
    let fits = std::path::Path::new(root).join("fits");
    if !fits.exists() {
        return Err(format!("no fits/ tree under {}", root));
    }
    let mut matches = Vec::new();
    for entry in walkdir_all(&fits) {
        let run_json = entry.join("run.json");
        if !run_json.is_file() { continue; }
        let Ok(run) = Run::read(&entry) else { continue; };
        // We only want FitStage runs, not the top-level Fit run.
        if !matches!(run.kind, RunKind::FitStage(_)) { continue; }
        if run.hash.starts_with(hash_prefix) {
            matches.push(entry.clone());
        }
    }
    match matches.len() {
        0 => Err(format!("no fit stage matching hash prefix '{}' under {}",
            hash_prefix, root)),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => {
            let mut msg = format!(
                "hash prefix '{}' is ambiguous, matches {} stages:\n",
                hash_prefix, n);
            for p in &matches {
                msg.push_str(&format!("  {}\n", p.display()));
            }
            msg.push_str("refine by passing a longer hash prefix");
            Err(msg)
        }
    }
}

/// Walk a directory tree returning every directory encountered. Depth-
/// unbounded; used by `resolve_stage_by_hash`. Dedicated because the
/// walkdir crate isn't a direct dep of this module and we only need
/// the simplest possible recursion.
fn walkdir_all(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.push(p.clone());
                    stack.push(p);
                }
            }
        }
    }
    out
}

// ── Output formatting ────────────────────────────────────────────────────────

fn print_sim_table(rows: &[SimRow], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};

    if rows.is_empty() {
        eprintln!("{}", "(no cached runs)".dimmed());
        return;
    }

    // NOTHING preset: plain aligned columns, no borders. Reads like `ls -l`
    // and scans cleanly for 20+ rows without box-art visual fatigue.
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("RUN_ID").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("LABEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SCENARIO").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SEED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PARAMS").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SIZE").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);

    for r in rows {
        let rel_time   = fmt_relative_time(r.created, now);
        let model      = model_display_name(r.leaf.level_label("model"));
        let size       = format_size(r.leaf.traj_bytes());
        // The address is the run_id (the path is keyed by the factored level
        // hashes; the run_id is what `show`/`cat` resolve).
        let hash_short = short_hash_cell(&r.leaf.run_id_hex());
        let label_cell = label_cell(&r.leaf.record.provenance.label);
        // The params level label carries the sweep point (`beta=0.2`) or
        // `base` for an unswept run.
        let params     = r.leaf.level_label("params").to_string();
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            hash_short,
            label_cell,
            Cell::new(model),
            Cell::new(r.leaf.level_label("scenario")).fg(comfy_table::Color::Green),
            Cell::new(r.leaf.seed()),
            Cell::new(params).add_attribute(comfy_table::Attribute::Dim),
            Cell::new(size),
            Cell::new(&r.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }

    println!("{table}");
}

/// Compact model identifier for the list's MODEL column. Full absolute
/// paths (`/Users/vsb/projects/work/camdl/ocaml/golden/sir_basic.ir.json`)
/// are unreadable at table width. Strip the directory and the standard
/// extensions — a reader recognizes the model by its basename.
fn model_display_name(path: &str) -> String {
    // Take the last path component after either separator.
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    // Strip `.ir.json` first (longer suffix), then fall back to `.camdl`.
    if let Some(stem) = base.strip_suffix(".ir.json") { return stem.to_string(); }
    if let Some(stem) = base.strip_suffix(".camdl")   { return stem.to_string(); }
    base.to_string()
}

fn print_sim_json(rows: &[SimRow]) {
    for r in rows {
        let json = serde_json::to_string(&r.leaf.record).unwrap_or_default();
        println!("{}", json);
    }
}

fn print_pfilter_json(rows: &[PfilterRow]) {
    for r in rows {
        let json = serde_json::to_string(&r.leaf.record).unwrap_or_default();
        println!("{}", json);
    }
}

fn print_pfilter_table(rows: &[PfilterRow], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("RUN_ID").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("LABEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("CONFIG").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SEED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("LOGLIK").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);
    for r in rows {
        let rel_time = fmt_relative_time(r.created, now);
        let model    = model_display_name(r.leaf.level_label("model"));
        let loglik   = r.leaf.record.inputs.get("loglik")
            .and_then(|v| v.as_f64())
            .map(|x| format!("{:.2}", x))
            .unwrap_or_else(|| "—".into());
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            short_hash_cell(&r.leaf.run_id_hex()),
            label_cell(&r.leaf.record.provenance.label),
            Cell::new(model),
            Cell::new(r.leaf.level_label("config")).add_attribute(comfy_table::Attribute::Dim),
            Cell::new(r.leaf.seed()),
            Cell::new(loglik).fg(comfy_table::Color::Magenta),
            Cell::new(&r.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }
    println!("{table}");
}

fn print_fits_json(fits: &[FitEntry]) {
    for f in fits {
        let json = serde_json::to_string(&f.run).unwrap_or_default();
        println!("{}", json);
    }
}

fn print_fits_table(fits: &[FitEntry], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("HASH").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("LABEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("ESTIMATE").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("STAGES").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);
    let mut unlabelled = 0usize;
    for f in fits {
        let rel_time = fmt_relative_time(f.created, now);
        let model    = model_display_name(&f.meta.model);
        let estimate = {
            let joined = f.meta.estimated.join(",");
            if joined.chars().count() > 30 {
                let mut s: String = joined.chars().take(29).collect(); s.push('…'); s
            } else { joined }
        };
        let stages = f.meta.stages_declared.join(",");
        if f.run.label.is_none() { unlabelled += 1; }
        let hash_short = short_hash_cell(&f.run.hash);
        let label_cell = label_cell(&f.run.label);
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            hash_short,
            label_cell,
            Cell::new(model),
            Cell::new(estimate).add_attribute(comfy_table::Attribute::Dim),
            Cell::new(stages).fg(comfy_table::Color::Green),
            Cell::new(&f.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }
    println!("{table}");
    crate::fit::fit_table::emit_unlabelled_warning(unlabelled);
}

fn print_profiles_json(profiles: &[ProfileEntry]) {
    for p in profiles {
        let json = serde_json::json!({
            "hash":    p.hash,
            "label":   p.label,
            "model":   p.model,
            "focal":   p.focal,
            "shape":   p.shape,
            "n_seeds": p.n_seeds,
            "path":    p.rel_path,
        });
        println!("{}", serde_json::to_string(&json).unwrap_or_default());
    }
}

fn print_profiles_table(profiles: &[ProfileEntry], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("HASH").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("LABEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("FOCAL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SHAPE").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SEEDS").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);
    for p in profiles {
        let rel_time = fmt_relative_time(p.created, now);
        let model    = model_display_name(&p.model);
        let seeds_cell = if p.n_seeds == 1 {
            Cell::new("1")
        } else {
            // Multi-seed profile: highlight so the sensitivity-spread
            // surface is easy to spot in long listings.
            Cell::new(p.n_seeds.to_string())
                .fg(comfy_table::Color::Green)
                .add_attribute(comfy_table::Attribute::Bold)
        };
        let hash_short = short_hash_cell(&p.hash);
        let label_cell = label_cell(&p.label);
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            hash_short,
            label_cell,
            Cell::new(model),
            Cell::new(&p.focal).fg(comfy_table::Color::Magenta),
            Cell::new(&p.shape).add_attribute(comfy_table::Attribute::Dim),
            seeds_cell,
            Cell::new(&p.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }
    println!("{table}");
}

/// 8-char hash prefix cell — what `camdl show <hash>` and
/// `camdl label <hash>` accept.
fn short_hash_cell(hash: &str) -> comfy_table::Cell {
    let n = hash.len().min(8);
    comfy_table::Cell::new(&hash[..n]).add_attribute(comfy_table::Attribute::Dim)
}

/// Render the LABEL cell uniformly across kinds: the trimmed label or
/// a dim "<unlabelled>" placeholder.
fn label_cell(label: &Option<String>) -> comfy_table::Cell {
    match label {
        Some(l) => comfy_table::Cell::new(l),
        None => comfy_table::Cell::new("<unlabelled>")
            .add_attribute(comfy_table::Attribute::Dim),
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 { format!("{}B", bytes) }
    else if bytes < 1024 * 1024 { format!("{}K", bytes / 1024) }
    else if bytes < 1024 * 1024 * 1024 { format!("{}M", bytes / 1024 / 1024) }
    else { format!("{}G", bytes / 1024 / 1024 / 1024) }
}

// ── Parsers (stdlib only) ────────────────────────────────────────────────────

/// Parse a duration like "1h", "30m", "2d", "1w". Returns Err on unknown
/// suffix or parse failure.
#[cfg(test)]
fn parse_duration(s: &str) -> Result<std::time::Duration, String> {
    let s = s.trim();
    if s.is_empty() { return Err("empty duration".into()); }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let n: u64 = num_str.parse()
        .map_err(|_| format!("bad duration '{}', expected <number><unit> (e.g. 1h, 2d)", s))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        "w" => n * 86400 * 7,
        other => return Err(format!("unknown duration unit '{}', expected s/m/h/d/w", other)),
    };
    Ok(std::time::Duration::from_secs(secs))
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` back to SystemTime.
fn parse_iso8601(s: &str) -> Option<SystemTime> {
    // Format: 2026-04-16T14:23:11Z
    if s.len() != 20 || !s.ends_with('Z') { return None; }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let minute: u32 = s[14..16].parse().ok()?;
    let second: u32 = s[17..19].parse().ok()?;
    let secs = days_from_civil(year, month, day) * 86400
        + (hour * 3600 + minute * 60 + second) as i64;
    if secs < 0 { return None; }
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
}

/// Howard Hinnant's days_from_civil (inverse of the one in cas.rs).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe/4 - yoe/100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// Produce a path relative to `base` (usually CWD), falling back to the
/// absolute string if the strip fails.
fn pathdiff_str(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_)  => path.to_string_lossy().into_owned(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_ok() {
        use std::time::Duration;
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("1w").unwrap(), Duration::from_secs(86400 * 7));
    }

    #[test]
    fn parse_duration_bad() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5y").is_err()); // y not supported; use weeks for alpha
        assert!(parse_duration("1.5h").is_err());
    }

    #[test]
    fn parse_iso8601_roundtrip() {
        use crate::cas::iso8601_utc;
        let times = [
            std::time::UNIX_EPOCH,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(946684800), // 2000-01-01
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1776297600), // 2026-04-16
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1709210096), // 2024-02-29T12:34:56Z
        ];
        for t in times {
            let s = iso8601_utc(t);
            let parsed = parse_iso8601(&s).expect("should parse");
            assert_eq!(parsed, t, "round-trip failed for {}", s);
        }
    }

    #[test]
    fn format_size_buckets() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(2048), "2K");
        assert_eq!(format_size(5 * 1024 * 1024), "5M");
    }

    #[test]
    fn model_display_name_strips_dir_and_extension() {
        // Absolute path + .ir.json → basename without extension
        assert_eq!(
            model_display_name("/Users/vsb/projects/work/camdl/ocaml/golden/sir_basic.ir.json"),
            "sir_basic"
        );
        // .camdl extension also stripped
        assert_eq!(model_display_name("../models/seir.camdl"), "seir");
        // No extension → bare basename
        assert_eq!(model_display_name("/tmp/custom"), "custom");
        // Bare basename unchanged (still strips known extension)
        assert_eq!(model_display_name("sir.ir.json"), "sir");
    }

    // ── Transitional reader: new-format (RunRecord) sim resolution ──────────

    /// Write a new-format sim leaf (RunRecord run.json + traj.tsv) at its
    /// factored `store_path`. `salt` varies the seed-level hash so two records
    /// land at distinct paths; `run_id` is set directly to exercise prefix
    /// resolution.
    fn write_sim_record(
        root: &Path,
        run_id: runid::ContentHash,
        seed: u64,
        salt: u8,
    ) -> PathBuf {
        let h = |b: u8| runid::ContentHash::from_bytes([b; 32]);
        let lvl = |name: &str, label: String, b: u8| runid::LevelId {
            name: name.into(), label, hash: h(b), schema_version: 1,
        };
        let levels = vec![
            lvl("model", "sir".into(), 1),
            lvl("config", "chain_binomial-dt1".into(), 2),
            lvl("params", "base".into(), 3),
            lvl("scenario", "baseline".into(), 4),
            lvl("seed", format!("seed_{seed}"), salt),
        ];
        let dir = runid::store_path(root, runid::ArtifactKind::Sim, &levels);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("traj.tsv"), "t\tS\n0\t100\n").unwrap();
        let rec = runid::RunRecord {
            format_version: runid::FORMAT_VERSION,
            kind: runid::ArtifactKind::Sim,
            run_id,
            hash_version: runid::HASH_VERSION,
            ir_version: "0.7".into(),
            engine_version: "test".into(),
            levels,
            deps: vec![],
            status: runid::RunStatus::Completed,
            artifacts: Default::default(),
            children: Default::default(),
            inputs: serde_json::Value::Null,
            provenance: runid::Provenance::default(),
        };
        std::fs::write(dir.join("run.json"), serde_json::to_string(&rec).unwrap()).unwrap();
        dir
    }

    #[test]
    fn resolve_sim_by_run_id_prefix_and_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_sim_record(
            tmp.path(),
            runid::ContentHash::from_bytes([0xab; 32]),
            42,
            10,
        );
        let root = tmp.path().to_str().unwrap();

        // run_id hex prefix.
        match resolve_any(root, "abab").unwrap() {
            Resolved::Sim { leaf, .. } => assert_eq!(leaf.seed(), 42),
            _ => panic!("expected new-format Sim"),
        }
        // /scenario narrowing.
        match resolve_any(root, "abab/baseline").unwrap() {
            Resolved::Sim { leaf, .. } => assert_eq!(leaf.seed(), 42),
            _ => panic!("expected Sim"),
        }
        // Path form.
        match resolve_any(root, dir.to_str().unwrap()).unwrap() {
            Resolved::Sim { .. } => {}
            _ => panic!("expected Sim from path"),
        }
        // No match.
        assert!(resolve_any(root, "ffff").is_err());
    }

    #[test]
    fn resolve_sim_ambiguous_prefix_lists_candidates() {
        // Two sims whose run_ids share the prefix "ab" but diverge after.
        let tmp = tempfile::tempdir().unwrap();
        write_sim_record(tmp.path(), runid::ContentHash::from_bytes([0xab; 32]), 1, 10);
        let mut b = [0xab; 32];
        b[1] = 0xcd; // hex "abcd…"
        write_sim_record(tmp.path(), runid::ContentHash::from_bytes(b), 2, 20);
        let root = tmp.path().to_str().unwrap();

        // "ab" matches both → ambiguous, with the sim kind label listed.
        let err = resolve_any(root, "ab").expect_err("ambiguous prefix must reject");
        assert!(err.contains("ambiguous"), "got: {}", err);
        assert!(err.contains("matches 2"), "got: {}", err);
        assert!(err.contains("sim"), "expected kind label: got {}", err);

        // "abab" uniquely resolves the first.
        match resolve_any(root, "abab").unwrap() {
            Resolved::Sim { leaf, .. } => assert_eq!(leaf.seed(), 1),
            _ => panic!("expected Sim"),
        }
    }
}
