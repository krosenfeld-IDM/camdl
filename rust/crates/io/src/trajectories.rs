//! The shared posterior-trajectory writer + manifest.
//!
//! Implements §4b of
//! `docs/dev/proposals/2026-06-09-latent-trajectory-output-consolidation.md`:
//! the tidy/long `trajectories.tsv` (all chains × all draws stacked, leading
//! `chain  draw  time [date]` id columns) plus a small `trajectories.json`
//! manifest so tooling can interpret a run without scraping the header.

use std::io::Write;
use std::path::Path;

use sim::{Flows, Trajectory};

/// Time resolution of a posterior path. PGAS paths are substep resolution;
/// PF/PMMH paths (a later consolidation step) are observation-step resolution.
/// Carried into the file header + manifest so a downstream union of mixed
/// outputs cannot silently blend substep PGAS paths with obs-resolution
/// PF/PMMH paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Substep,
    Observation,
}

impl Granularity {
    pub fn as_str(self) -> &'static str {
        match self {
            Granularity::Substep => "substep",
            Granularity::Observation => "observation",
        }
    }
}

/// One posterior draw: the latent path it implies, identified by `(chain,
/// draw)`. The path *is* a [`Trajectory`] (the `simulate` output type); the
/// model-predicted per-stream incidence rides as a sidecar
/// (`incidence[snapshot][stream]`, parallel to `path.snapshots`) so the writer
/// can emit `inc_<stream>` columns without finite-differencing counts.
pub struct PosteriorDraw {
    pub chain: usize,
    pub draw: usize,
    pub path: Trajectory,
    /// `incidence[s][k]` — the k-th incidence stream's model-predicted value at
    /// snapshot `s`. Either empty (no incidence streams) or
    /// `incidence.len() == path.snapshots.len()`, each inner row of length
    /// `stream_names.len()` (see [`write_trajectories_tsv`]). The producer (the
    /// PGAS adapter) computes this from the observation model's `FlowSum`
    /// projection — never from `−ΔS` / `diff(flow)`.
    pub incidence: Vec<Vec<f64>>,
}

/// Column-layout description shared by the writer and the manifest. Built once
/// from the model so the header and the `trajectories.json` `columns` list
/// cannot drift.
pub struct TrajColumnSpec {
    /// Integer compartment names, in model order (index into
    /// `Snapshot::int_state.counts`).
    pub int_comps: Vec<String>,
    /// Real compartment names, in model order (index into
    /// `Snapshot::real_state.values`).
    pub real_comps: Vec<String>,
    /// `flow_<transition>` names, in model order (index into the snapshot flow
    /// vector).
    pub flows: Vec<String>,
    /// `inc_<stream>` names, in incidence-stream order (index into a
    /// [`PosteriorDraw::incidence`] row).
    pub incidence: Vec<String>,
}

impl TrajColumnSpec {
    /// Build from a model + the incidence-stream names. Integer/real
    /// compartments split by `CompartmentKind`, matching the `simulate`
    /// trajectory writer's column order.
    pub fn from_model(model: &ir::Model, incidence_stream_names: &[String]) -> Self {
        let mut int_comps = Vec::new();
        let mut real_comps = Vec::new();
        for c in &model.compartments {
            match c.kind {
                ir::model::CompartmentKind::Integer => int_comps.push(c.name.clone()),
                ir::model::CompartmentKind::Real => real_comps.push(c.name.clone()),
            }
        }
        let flows = model
            .transitions
            .iter()
            .map(|t| format!("flow_{}", t.name))
            .collect();
        let incidence = incidence_stream_names
            .iter()
            .map(|n| format!("inc_{}", n))
            .collect();
        TrajColumnSpec {
            int_comps,
            real_comps,
            flows,
            incidence,
        }
    }

    /// Every data-column name (excluding the leading `chain/draw/time[/date]`
    /// id columns), in emit order. Used for the manifest `columns` list.
    pub fn data_column_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.int_comps.iter().cloned());
        out.extend(self.real_comps.iter().cloned());
        out.extend(self.flows.iter().cloned());
        out.extend(self.incidence.iter().cloned());
        out
    }
}

/// The `trajectories.json` manifest. Records how to interpret the sibling
/// `trajectories.tsv` without scraping its header, and surfaces the
/// conditioned-vs-forward distinction (this file's `inc_<stream>` is the
/// *conditioned* smoother `p(x|y)`; a `simulate --obs` file is the *forward*
/// posterior-predictive `p(y|θ)`).
pub struct TrajManifest {
    pub method: String,
    pub granularity: Granularity,
    pub n_chains: usize,
    pub n_draws: usize,
    /// Every TSV column name in emit order: the id columns
    /// (`chain`, `draw`, `time`, optional `date`) then the data columns.
    pub columns: Vec<String>,
    pub model_hash: String,
    /// `true` for a smoother path conditioned on the data (PGAS `X|θ,y`); the
    /// `inc_<stream>` columns are conditioned incidence, NOT the free-forward
    /// posterior-predictive a `simulate --obs` run produces.
    pub conditioned: bool,
    /// `true` if the paths come from an ancestral filter-smoother prone to
    /// early-time degeneracy (PF / PMMH). PGAS ancestor sampling mitigates this,
    /// so PGAS paths set `false`.
    pub degeneracy_caveat: bool,
    /// The requested number of saved trajectories (`n_trajectories` /
    /// `--save-paths N`) — the source count this file was produced from.
    pub n_trajectories: usize,
    /// Best-effort pointer to the run's free-forward posterior-predictive
    /// observation file (from `simulate --obs`), so a researcher can compare
    /// the conditioned smoother incidence here against the forward predictive.
    /// `None` when no such file is discoverable for this run.
    pub predictive_obs_file: Option<String>,
}

impl TrajManifest {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "format": "camdl-trajectories",
            "version": 1,
            "method": self.method,
            "granularity": self.granularity.as_str(),
            "n_chains": self.n_chains,
            "n_draws": self.n_draws,
            "columns": self.columns,
            "model_hash": self.model_hash,
            // Conditioned (smoother p(x|y)) vs forward predictive (p(y|θ)). The
            // inc_<stream> columns here are conditioned; a simulate --obs file is
            // forward. Surfaced so a smoother is never mistaken for the
            // predictive.
            "conditioned": self.conditioned,
            "degeneracy_caveat": self.degeneracy_caveat,
            "n_trajectories": self.n_trajectories,
            "predictive_obs_file": self.predictive_obs_file,
        })
    }

    /// Write the manifest as pretty JSON to `path`.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.to_json())
            .map_err(|e| format!("trajectories manifest: json error: {e}"))?;
        std::fs::write(path, json)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

/// The `# camdl-trajectories v1 ...` header line for a `trajectories.tsv`.
fn header_comment(model_hash: &str, method: &str, granularity: Granularity) -> String {
    format!(
        "# camdl-trajectories v1\tmodel={}\tmethod={}\tgranularity={}",
        model_hash,
        method,
        granularity.as_str()
    )
}

/// Optional calendar-date origin for a `date` column.
///
/// `(origin, time_unit)` — the model's declared `origin =
/// date("...")` and `time_unit`. When `Some`, the writer emits a `date` column
/// rendered via [`ir::caltime::internal_to_date_hires`] (the same path
/// `simulate --dates` uses); when `None`, no date column.
pub type DateOrigin<'a> = Option<(&'a str, &'a str)>;

/// Write the tidy/long `trajectories.tsv`: a `# camdl-trajectories v1` header,
/// the column header (`chain  draw  time [date]  <int> <real> flow_* inc_*`),
/// then one row per snapshot per draw — all chains × all draws stacked into one
/// file, disambiguated by the leading id columns.
///
/// `columns` describes the data-column layout (built once from the model);
/// every draw's `path` must agree with it (same int/real/flow vector lengths)
/// and each draw's `incidence` (when non-empty) must be parallel to its
/// `path.snapshots` with rows of length `columns.incidence.len()`. A mismatch
/// is a hard error — the writer never silently truncates or zero-fills.
#[allow(clippy::too_many_arguments)]
pub fn write_trajectories_tsv(
    path: &Path,
    draws: &[PosteriorDraw],
    columns: &TrajColumnSpec,
    date_origin: DateOrigin,
    model_hash: &str,
    method: &str,
    granularity: Granularity,
) -> Result<(), String> {
    let f = std::fs::File::create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    // BufWriter: a substep-resolution national-scale path is thousands of rows ×
    // hundreds of fields; unbuffered, each `write!` is a syscall. The simulate
    // and (old) PGAS writers both buffer for exactly this reason.
    let mut w = std::io::BufWriter::new(f);

    writeln!(w, "{}", header_comment(model_hash, method, granularity))
        .map_err(|e| e.to_string())?;

    // Column header.
    write!(w, "chain\tdraw\ttime").map_err(|e| e.to_string())?;
    if date_origin.is_some() {
        write!(w, "\tdate").map_err(|e| e.to_string())?;
    }
    for n in &columns.int_comps {
        write!(w, "\t{}", n).map_err(|e| e.to_string())?;
    }
    for n in &columns.real_comps {
        write!(w, "\t{}", n).map_err(|e| e.to_string())?;
    }
    for n in &columns.flows {
        write!(w, "\t{}", n).map_err(|e| e.to_string())?;
    }
    for n in &columns.incidence {
        write!(w, "\t{}", n).map_err(|e| e.to_string())?;
    }
    writeln!(w).map_err(|e| e.to_string())?;

    let n_int = columns.int_comps.len();
    let n_real = columns.real_comps.len();
    let n_flow = columns.flows.len();
    let n_inc = columns.incidence.len();

    for d in draws {
        // Validate the incidence sidecar shape once per draw.
        if !d.incidence.is_empty() && d.incidence.len() != d.path.snapshots.len() {
            return Err(format!(
                "trajectories: chain {} draw {}: incidence has {} rows but path \
                 has {} snapshots",
                d.chain, d.draw, d.incidence.len(), d.path.snapshots.len()
            ));
        }
        for (s, snap) in d.path.snapshots.iter().enumerate() {
            if snap.int_state.counts.len() != n_int {
                return Err(format!(
                    "trajectories: chain {} draw {}: snapshot has {} integer \
                     compartments, header declares {}",
                    d.chain, d.draw, snap.int_state.counts.len(), n_int
                ));
            }
            if snap.real_state.values.len() != n_real {
                return Err(format!(
                    "trajectories: chain {} draw {}: snapshot has {} real \
                     compartments, header declares {}",
                    d.chain, d.draw, snap.real_state.values.len(), n_real
                ));
            }
            if snap.flows.len() != n_flow {
                return Err(format!(
                    "trajectories: chain {} draw {}: snapshot has {} flows, \
                     header declares {}",
                    d.chain, d.draw, snap.flows.len(), n_flow
                ));
            }

            write!(w, "{}\t{}\t{}", d.chain, d.draw, snap.t).map_err(|e| e.to_string())?;
            if let Some((origin, time_unit)) = date_origin {
                let date = ir::caltime::internal_to_date_hires(origin, snap.t, time_unit)
                    .map_err(|e| format!("trajectories: error rendering date: {e}"))?;
                write!(w, "\t{}", date).map_err(|e| e.to_string())?;
            }
            for &c in &snap.int_state.counts {
                write!(w, "\t{}", c).map_err(|e| e.to_string())?;
            }
            for &v in &snap.real_state.values {
                write!(w, "\t{:.6}", v).map_err(|e| e.to_string())?;
            }
            match &snap.flows {
                Flows::Int(fs) => {
                    for &fl in fs {
                        write!(w, "\t{}", fl).map_err(|e| e.to_string())?;
                    }
                }
                Flows::Real(fs) => {
                    for &fl in fs {
                        write!(w, "\t{:.6}", fl).map_err(|e| e.to_string())?;
                    }
                }
            }
            if n_inc > 0 {
                let row = &d.incidence[s];
                if row.len() != n_inc {
                    return Err(format!(
                        "trajectories: chain {} draw {}: incidence row {} has {} \
                         entries, header declares {}",
                        d.chain, d.draw, s, row.len(), n_inc
                    ));
                }
                for &inc in row {
                    write!(w, "\t{}", inc).map_err(|e| e.to_string())?;
                }
            }
            writeln!(w).map_err(|e| e.to_string())?;
        }
    }

    // Explicit flush: BufWriter swallows write errors on drop, which would
    // silently truncate the file if the disk filled during the final drain.
    w.flush()
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{IntState, RealState, Snapshot};

    fn snap(t: f64, ints: Vec<i64>, flows: Vec<u64>) -> Snapshot {
        Snapshot {
            t,
            int_state: IntState::from_vec(ints),
            real_state: RealState::from_vec(Vec::new()),
            flows: Flows::Int(flows),
        }
    }

    fn cols() -> TrajColumnSpec {
        TrajColumnSpec {
            int_comps: vec!["S".into(), "I".into(), "R".into()],
            real_comps: vec![],
            flows: vec!["flow_infection".into(), "flow_recovery".into()],
            incidence: vec!["inc_cases".into()],
        }
    }

    #[test]
    fn writes_tidy_long_header_and_stacked_rows() {
        let tmp = std::env::temp_dir().join(format!("camdl_io_traj_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join("trajectories.tsv");

        let mut t0 = Trajectory::new();
        t0.push(snap(0.0, vec![99, 1, 0], vec![0, 0]));
        t0.push(snap(1.0, vec![97, 2, 1], vec![2, 1]));
        let mut t1 = Trajectory::new();
        t1.push(snap(0.0, vec![99, 1, 0], vec![0, 0]));
        t1.push(snap(1.0, vec![98, 2, 0], vec![1, 0]));

        let draws = vec![
            PosteriorDraw {
                chain: 0,
                draw: 5,
                path: t0,
                incidence: vec![vec![0.0], vec![2.0]],
            },
            PosteriorDraw {
                chain: 1,
                draw: 5,
                path: t1,
                incidence: vec![vec![0.0], vec![1.0]],
            },
        ];

        write_trajectories_tsv(&path, &draws, &cols(), None, "abc123", "pgas", Granularity::Substep)
            .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("# camdl-trajectories v1"), "got: {}", lines[0]);
        assert!(lines[0].contains("method=pgas"));
        assert!(lines[0].contains("granularity=substep"));
        assert_eq!(
            lines[1],
            "chain\tdraw\ttime\tS\tI\tR\tflow_infection\tflow_recovery\tinc_cases"
        );
        // 2 draws × 2 snapshots = 4 data rows + header-comment + col-header.
        assert_eq!(lines.len(), 6);
        // First data row.
        assert_eq!(lines[2], "0\t5\t0\t99\t1\t0\t0\t0\t0");
        // The inc_cases column == the FlowSum value the producer computed.
        assert_eq!(lines[3], "0\t5\t1\t97\t2\t1\t2\t1\t2");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_incidence_streams_omits_inc_columns() {
        let c = TrajColumnSpec {
            int_comps: vec!["N".into()],
            real_comps: vec![],
            flows: vec!["flow_death".into()],
            incidence: vec![],
        };
        let mut t = Trajectory::new();
        t.push(snap(1.0, vec![950], vec![50]));
        let draws = vec![PosteriorDraw {
            chain: 0,
            draw: 0,
            path: t,
            incidence: vec![],
        }];
        let tmp = std::env::temp_dir().join(format!("camdl_io_traj_noinc_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join("t.tsv");
        write_trajectories_tsv(&path, &draws, &c, None, "h", "pgas", Granularity::Substep).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let header = text.lines().nth(1).unwrap();
        assert_eq!(header, "chain\tdraw\ttime\tN\tflow_death");
        assert!(!header.contains("inc_"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn shape_mismatch_is_a_hard_error() {
        // A snapshot with the wrong number of int compartments must error, not
        // silently truncate / pad.
        let mut t = Trajectory::new();
        t.push(snap(0.0, vec![1, 2], vec![0, 0])); // 2 ints, header wants 3
        let draws = vec![PosteriorDraw {
            chain: 0,
            draw: 0,
            path: t,
            incidence: vec![vec![0.0]],
        }];
        let tmp = std::env::temp_dir().join(format!("camdl_io_traj_err_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join("t.tsv");
        let err = write_trajectories_tsv(&path, &draws, &cols(), None, "h", "pgas", Granularity::Substep)
            .unwrap_err();
        assert!(err.contains("integer compartments"), "got: {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn manifest_records_conditioned_and_columns() {
        let m = TrajManifest {
            method: "pgas".into(),
            granularity: Granularity::Substep,
            n_chains: 2,
            n_draws: 8,
            columns: vec!["chain".into(), "draw".into(), "time".into(), "S".into(), "inc_cases".into()],
            model_hash: "abc".into(),
            conditioned: true,
            degeneracy_caveat: false,
            n_trajectories: 4,
            predictive_obs_file: None,
        };
        let json = m.to_json();
        assert_eq!(json["conditioned"], serde_json::json!(true));
        assert_eq!(json["granularity"], serde_json::json!("substep"));
        assert_eq!(json["n_trajectories"], serde_json::json!(4));
        assert_eq!(json["columns"][4], serde_json::json!("inc_cases"));
        assert_eq!(json["predictive_obs_file"], serde_json::Value::Null);
    }
}
