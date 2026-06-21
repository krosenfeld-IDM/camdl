//! Per-run liveness/progress heartbeat (gh#278).
//!
//! A long fit run (a national-scale PGAS sweep can take days, with minutes
//! between trace writes) leaves no reliable on-disk liveness signal: the
//! `.lock` PID is same-host-only and reuse-prone, and `trace.tsv` mtime is not
//! a heartbeat (one sweep is a full particle-filter pass). This module emits a
//! small `progress.json`, refreshed on a **fixed wall-clock timer independent
//! of step cadence**, so any consumer (a remote dashboard, CI) reads one
//! contract for liveness + progress instead of reverse-engineering it — and can
//! spot a fit that is broken early and stop it.
//!
//! The progress model is **algorithm-agnostic** so EVERY fitting method maps
//! onto it: a generic `step`/`total` counter plus a [`Phase`] (MCMC
//! burn-in/sampling, optimizer search, profile grid). The [`Heartbeat`]
//! constructor picks how the phase is derived ([`Heartbeat::mcmc`] /
//! [`Heartbeat::optimizing`] / [`Heartbeat::profiling`]); the loop just
//! [`bump`](Heartbeat::bump)s a step counter.
//!
//! The honest model: a SIGKILLed run *cannot* write "I died". So the artifact
//! records the run's last self-report ([`RunState`]); **deadness is a consumer
//! inference from staleness**, never a self-claim. [`liveness`] folds the
//! artifact + the clock into that judgement once ([`RunLiveness`]).
//!
//! The heartbeat is a **pure observer**: the step loop only stores into a shared
//! atomic ([`Heartbeat::bump`]); a background thread does the file I/O. It reads
//! nothing the inference writes and consumes no RNG — it cannot change a fit
//! number.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The artifact filename written into a run's seed/stage directory.
pub const PROGRESS_FILE: &str = "progress.json";

/// What kind of work a run is doing — algorithm-agnostic, so every fitting
/// method maps onto the one progress type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// MCMC warmup — no trace rows yet (gh#278 motivation 3). PGAS / PMMH / `mh` on ode.
    BurnIn,
    /// MCMC sampling — trace rows accrue. PGAS / PMMH / `mh` on ode.
    Sampling,
    /// Searching for the MLE — IF2's cooling iterations or an NLopt eval loop.
    Optimizing,
    /// Stepping a profile-likelihood grid.
    Profiling,
}

/// How a run derives its [`Phase`] from the step counter. A [`Heartbeat`]'s
/// constructor picks the rule — MCMC splits on `burn_in`; every other algorithm
/// has a single fixed phase. This is what lets one progress type serve all of
/// them without a phase that some algorithm can't fill in.
#[derive(Debug, Clone, Copy)]
enum PhaseRule {
    Mcmc { burn_in: u64 },
    Fixed(Phase),
}

impl PhaseRule {
    fn at(&self, step: u64) -> Phase {
        match self {
            PhaseRule::Mcmc { burn_in } => {
                if step < *burn_in { Phase::BurnIn } else { Phase::Sampling }
            }
            PhaseRule::Fixed(p) => *p,
        }
    }
}

/// The run's last self-report. An ADT, so incoherent combinations (a failure
/// with no reason, `running` with no counter) are unrepresentable. Serializes
/// externally-tagged: `{"running": {…}}` / `"done"` / `{"failed": {…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Live: `step` of `total` units done. The unit is the algorithm's
    /// (sweeps / IF2 iterations / NLopt evals / profile grid points); `phase`
    /// gives the context. Coherent only while running.
    Running { phase: Phase, step: u64, total: u64 },
    /// Clean completion.
    Done,
    /// Clean, caught failure — carries the reason. An *un*caught death
    /// (SIGKILL/panic) instead leaves the last `Running` on disk going stale;
    /// see [`RunLiveness`].
    Failed { reason: String },
}

/// The on-disk artifact: an always-present envelope around the state ADT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Unix epoch SECONDS of the last write. Freshness of this — not the
    /// `state` field — is the liveness signal (a killed run can't update it).
    pub updated_at: u64,
    /// The writing process's PID. Informational only; liveness must NOT depend
    /// on it (cross-host + PID-reuse fragile — the whole point of this artifact).
    pub pid: u32,
    /// The run's last self-report.
    pub state: RunState,
}

impl Progress {
    fn now(state: RunState) -> Progress {
        Progress {
            updated_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            pid: std::process::id(),
            state,
        }
    }
}

/// A consumer's judgement, folding the artifact + the clock into one ADT so the
/// staleness/terminal logic lives in a single parse, not re-derived per reader.
#[derive(Debug, Clone, PartialEq)]
pub enum RunLiveness {
    /// `Running` and the heartbeat is fresh.
    Alive(RunState),
    /// `Running` but the heartbeat is stale → presumed dead or hung. This is
    /// the SIGKILL case, named honestly: an inference, not a self-claim.
    PresumedDead(RunState),
    /// `Done`/`Failed` — a clean terminal write; freshness is irrelevant.
    Terminal(RunState),
}

/// Fold a [`Progress`] read + the current time into a [`RunLiveness`].
/// `now_unix` and `max_stale_secs` are seconds. A clean terminal state
/// short-circuits the freshness check; only a stale `Running` is `PresumedDead`.
pub fn liveness(p: &Progress, now_unix: u64, max_stale_secs: u64) -> RunLiveness {
    match &p.state {
        RunState::Done | RunState::Failed { .. } => RunLiveness::Terminal(p.state.clone()),
        RunState::Running { .. } => {
            let age = now_unix.saturating_sub(p.updated_at);
            if age <= max_stale_secs {
                RunLiveness::Alive(p.state.clone())
            } else {
                RunLiveness::PresumedDead(p.state.clone())
            }
        }
    }
}

/// Atomically write `progress.json` into `dir` (temp file + rename), so a
/// concurrent reader never observes a half-written file.
pub fn write_progress(dir: &Path, p: &Progress) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(p).map_err(io::Error::other)?;
    let final_path = dir.join(PROGRESS_FILE);
    let tmp = dir.join(format!("{}.{}.tmp", PROGRESS_FILE, std::process::id()));
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &final_path)?;
    Ok(())
}

/// Read and parse a run's `progress.json`.
pub fn read_progress(dir: &Path) -> io::Result<Progress> {
    let bytes = fs::read(dir.join(PROGRESS_FILE))?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

// ── The heartbeat handle ─────────────────────────────────────────────────────

struct Shared {
    dir: PathBuf,
    rule: PhaseRule,
    total: u64,
    step: AtomicU64, // monotonic (fetch_max) — furthest any chain has reached
    stop: AtomicBool,
}

/// A background heartbeat for a run directory. The constructor encodes the
/// algorithm's progress shape ([`mcmc`](Heartbeat::mcmc) /
/// [`optimizing`](Heartbeat::optimizing) / [`profiling`](Heartbeat::profiling));
/// the step loop calls [`bump`](Heartbeat::bump) (a cheap atomic, no I/O); a
/// timer thread writes `progress.json` every `interval`. [`finish`](Heartbeat::finish)
/// writes the clean terminal state and joins. If dropped without `finish`
/// (panic/early return), the thread stops and the last `Running` is left to go
/// stale — a consumer then reads `PresumedDead`.
pub struct Heartbeat {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl Heartbeat {
    fn spawn(dir: PathBuf, rule: PhaseRule, total: u64, interval: Duration) -> Heartbeat {
        let shared = Arc::new(Shared {
            dir,
            rule,
            total,
            step: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let s = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("camdl-heartbeat".into())
            .spawn(move || {
                // Sleep in short ticks so `stop` is responsive without a condvar.
                let tick = Duration::from_millis(250).min(interval);
                loop {
                    let step = s.step.load(Ordering::Relaxed);
                    let p = Progress::now(RunState::Running {
                        phase: s.rule.at(step),
                        step,
                        total: s.total,
                    });
                    let _ = write_progress(&s.dir, &p);
                    let mut waited = Duration::ZERO;
                    while waited < interval {
                        if s.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(tick);
                        waited += tick;
                    }
                }
            })
            .expect("spawn heartbeat thread");
        Heartbeat { shared, handle: Some(handle) }
    }

    /// MCMC heartbeat (PGAS / PMMH / `mh` on ode): the phase reads `BurnIn`
    /// below `burn_in` sweeps and `Sampling` at/after it. `total` is the total
    /// sweeps/steps. `interval` is a fixed wall-clock period (5–10 s).
    pub fn mcmc(dir: PathBuf, burn_in: u64, total: u64, interval: Duration) -> Heartbeat {
        Self::spawn(dir, PhaseRule::Mcmc { burn_in }, total, interval)
    }

    /// Optimizer heartbeat (IF2 cooling iterations / NLopt eval loop): a single
    /// `Optimizing` phase. `total` is the iteration / max-eval budget.
    pub fn optimizing(dir: PathBuf, total: u64, interval: Duration) -> Heartbeat {
        Self::spawn(dir, PhaseRule::Fixed(Phase::Optimizing), total, interval)
    }

    /// Profile heartbeat: a single `Profiling` phase. `total` is the grid size.
    pub fn profiling(dir: PathBuf, total: u64, interval: Duration) -> Heartbeat {
        Self::spawn(dir, PhaseRule::Fixed(Phase::Profiling), total, interval)
    }

    /// Report the furthest step reached. Cheap (one relaxed `fetch_max`) — safe
    /// from the hot loop and from multiple parallel chains; monotonic, so
    /// progress never jitters backward. Does no I/O.
    pub fn bump(&self, step: u64) {
        self.shared.step.fetch_max(step, Ordering::Relaxed);
    }

    /// Stop the timer and write the clean terminal state (`Done` / `Failed`).
    pub fn finish(mut self, state: RunState) {
        self.stop_thread();
        let _ = write_progress(&self.shared.dir, &Progress::now(state));
    }

    fn stop_thread(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        // finish() already joined; this only fires on an un-finished drop
        // (panic/early return). Stop the thread and leave the last Running on
        // disk — the consumer infers PresumedDead from its staleness.
        self.stop_thread();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_serializes_as_tagged_adt() {
        let r = RunState::Running { phase: Phase::Optimizing, step: 3, total: 10 };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains("\"running\"") && j.contains("\"optimizing\"") && j.contains("\"step\":3"));
        assert_eq!(serde_json::to_string(&RunState::Done).unwrap(), "\"done\"");
        let f = serde_json::to_string(&RunState::Failed { reason: "boom".into() }).unwrap();
        assert!(f.contains("\"failed\"") && f.contains("boom"));
        let back: RunState = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn phase_rule_covers_every_algorithm_shape() {
        // MCMC: derived from burn_in.
        let mcmc = PhaseRule::Mcmc { burn_in: 5 };
        assert_eq!(mcmc.at(4), Phase::BurnIn);
        assert_eq!(mcmc.at(5), Phase::Sampling);
        // Optimizer / profile: fixed phase regardless of step.
        assert_eq!(PhaseRule::Fixed(Phase::Optimizing).at(999), Phase::Optimizing);
        assert_eq!(PhaseRule::Fixed(Phase::Profiling).at(0), Phase::Profiling);
    }

    #[test]
    fn liveness_distinguishes_alive_stale_terminal() {
        let running = |t: u64| Progress {
            updated_at: t, pid: 1,
            state: RunState::Running { phase: Phase::Sampling, step: 5, total: 10 },
        };
        assert!(matches!(liveness(&running(100), 105, 30), RunLiveness::Alive(_)));
        assert!(matches!(liveness(&running(100), 200, 30), RunLiveness::PresumedDead(_)));
        let done = Progress { updated_at: 1, pid: 1, state: RunState::Done };
        assert!(matches!(liveness(&done, 9_999_999, 30), RunLiveness::Terminal(_)));
    }

    #[test]
    fn write_then_read_round_trips_and_is_atomic_named() {
        let dir = tempfile::tempdir().unwrap();
        let p = Progress::now(RunState::Running { phase: Phase::BurnIn, step: 7, total: 20 });
        write_progress(dir.path(), &p).unwrap();
        let tmp = dir.path().join(format!("{}.{}.tmp", PROGRESS_FILE, std::process::id()));
        assert!(!tmp.exists(), "temp file should be renamed away");
        assert_eq!(read_progress(dir.path()).unwrap().state, p.state);
    }

    #[test]
    fn heartbeat_writes_then_finish_marks_terminal() {
        let dir = tempfile::tempdir().unwrap();
        // optimizing: fixed phase, step 12 of 30.
        let hb = Heartbeat::optimizing(dir.path().to_path_buf(), 30, Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(40));
        hb.bump(12);
        std::thread::sleep(Duration::from_millis(40));
        let mid = read_progress(dir.path()).unwrap();
        assert!(matches!(mid.state, RunState::Running { step: 12, phase: Phase::Optimizing, .. }));
        hb.finish(RunState::Done);
        assert_eq!(read_progress(dir.path()).unwrap().state, RunState::Done);
    }
}
