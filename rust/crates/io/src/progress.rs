//! Per-run liveness/progress heartbeat (gh#278).
//!
//! A long fit run (a national-scale PGAS sweep can take days, with minutes
//! between trace writes) leaves no reliable on-disk liveness signal: the
//! `.lock` PID is same-host-only and reuse-prone, and `trace.tsv` mtime is not
//! a heartbeat (one sweep is a full particle-filter pass). This module emits a
//! small `progress.json`, refreshed on a **fixed wall-clock timer independent
//! of sweep cadence**, so any consumer (a remote dashboard, CI) reads one
//! contract for liveness + progress instead of reverse-engineering it.
//!
//! The honest model: a SIGKILLed run *cannot* write "I died". So the artifact
//! records the run's last self-report ([`RunState`]); **deadness is a consumer
//! inference from staleness**, never a self-claim. [`liveness`] folds the
//! artifact + the clock into that judgement once ([`RunLiveness`]), so a
//! consumer never touches a PID or an mtime.
//!
//! The heartbeat is a **pure observer**: the sweep loop only stores into shared
//! atomics ([`Heartbeat::set`]); a background thread does the file I/O. It reads
//! nothing the inference writes and consumes no RNG — it cannot change a single
//! fit number.

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

/// Where in a sampler run the chains currently are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Warming up — no trace rows are written yet, so this phase is otherwise
    /// invisible on disk (gh#278 motivation 3).
    BurnIn,
    /// Post-burn-in sampling — trace rows accrue.
    Sampling,
}

impl Phase {
    /// Derive the phase from how far the run has swept. The single source of
    /// truth is the sweep counter — no separate phase field to drift or race.
    fn at(sweep: u64, burn_in: u64) -> Phase {
        if sweep < burn_in { Phase::BurnIn } else { Phase::Sampling }
    }
}

/// The run's last self-report. An ADT, so incoherent combinations
/// (`done` + `burn_in`, a failure with no reason, `sampling` with no sweep
/// counter) are unrepresentable. Serializes externally-tagged:
/// `{"running": {…}}` / `"done"` / `{"failed": {"reason": …}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Live: a sweep counter + phase. Coherent only while running.
    Running { phase: Phase, sweep: u64, total_sweeps: u64 },
    /// Clean completion — carries no sweep counter (it ran to `total_sweeps`).
    Done,
    /// Clean, caught failure — carries the reason (the flat-JSON proposal
    /// dropped this). An *un*caught death (SIGKILL/panic) leaves the last
    /// `Running` on disk going stale instead; see [`RunLiveness`].
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
/// `now_unix` and `max_stale` are seconds. A clean terminal state short-circuits
/// the freshness check; only a stale `Running` becomes `PresumedDead`.
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
    // Unique temp name (pid) so two writers in the same dir never collide.
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
    burn_in: u64,
    total_sweeps: u64,
    sweep: AtomicU64, // monotonic (fetch_max) — furthest any chain has reached
    stop: AtomicBool,
}

/// A background heartbeat for a run directory. `start` spawns a timer thread
/// that writes `progress.json` every `interval`; the sweep loop calls
/// [`Heartbeat::set`] (cheap atomic stores, no I/O); [`Heartbeat::finish`]
/// writes the clean terminal state and joins the thread. If dropped without
/// `finish` (panic/early return), the thread stops and the last `Running` state
/// is left on disk to go stale — a consumer then reads `PresumedDead`.
pub struct Heartbeat {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl Heartbeat {
    /// Start the heartbeat. Writes an initial `Running{BurnIn, 0}` immediately,
    /// then refreshes every `interval`. `interval` should be a fixed wall-clock
    /// period (5–10 s) — NOT tied to sweep cadence. `burn_in` is the sweep
    /// boundary below which the phase reads `BurnIn`.
    pub fn start(dir: PathBuf, burn_in: u64, total_sweeps: u64, interval: Duration) -> Heartbeat {
        let shared = Arc::new(Shared {
            dir,
            burn_in,
            total_sweeps,
            sweep: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let s = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("camdl-heartbeat".into())
            .spawn(move || {
                // Sleep in short ticks so `stop` is responsive without a condvar.
                let tick = Duration::from_millis(250).min(interval);
                loop {
                    let sweep = s.sweep.load(Ordering::Relaxed);
                    let p = Progress::now(RunState::Running {
                        phase: Phase::at(sweep, s.burn_in),
                        sweep,
                        total_sweeps: s.total_sweeps,
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

    /// Report the furthest sweep a chain has reached. Cheap (one relaxed
    /// `fetch_max`) — safe from the hot sweep loop and from multiple parallel
    /// chains; monotonic, so progress never jitters backward. Does no I/O (the
    /// background thread writes the file).
    pub fn bump(&self, sweep: u64) {
        self.shared.sweep.fetch_max(sweep, Ordering::Relaxed);
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
        // (panic/early return). Stop the thread and leave the last Running
        // state on disk — the consumer infers PresumedDead from its staleness.
        self.stop_thread();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_serializes_as_tagged_adt() {
        let r = RunState::Running { phase: Phase::BurnIn, sweep: 3, total_sweeps: 10 };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains("\"running\"") && j.contains("\"burn_in\"") && j.contains("\"sweep\":3"));
        assert_eq!(serde_json::to_string(&RunState::Done).unwrap(), "\"done\"");
        let f = serde_json::to_string(&RunState::Failed { reason: "boom".into() }).unwrap();
        assert!(f.contains("\"failed\"") && f.contains("boom"));
        // round-trip
        let back: RunState = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn liveness_distinguishes_alive_stale_terminal() {
        let running = |t: u64| Progress {
            updated_at: t, pid: 1,
            state: RunState::Running { phase: Phase::Sampling, sweep: 5, total_sweeps: 10 },
        };
        // fresh Running → Alive
        assert!(matches!(liveness(&running(100), 105, 30), RunLiveness::Alive(_)));
        // stale Running → PresumedDead (the SIGKILL case)
        assert!(matches!(liveness(&running(100), 200, 30), RunLiveness::PresumedDead(_)));
        // Done/Failed → Terminal regardless of freshness
        let done = Progress { updated_at: 1, pid: 1, state: RunState::Done };
        assert!(matches!(liveness(&done, 9_999_999, 30), RunLiveness::Terminal(_)));
    }

    #[test]
    fn write_then_read_round_trips_and_is_atomic_named() {
        let dir = tempfile::tempdir().unwrap();
        let p = Progress::now(RunState::Running { phase: Phase::BurnIn, sweep: 7, total_sweeps: 20 });
        write_progress(dir.path(), &p).unwrap();
        // no leftover temp file
        let tmp = dir.path().join(format!("{}.{}.tmp", PROGRESS_FILE, std::process::id()));
        assert!(!tmp.exists(), "temp file should be renamed away");
        let back = read_progress(dir.path()).unwrap();
        assert_eq!(back.state, p.state);
    }

    #[test]
    fn heartbeat_writes_then_finish_marks_terminal() {
        let dir = tempfile::tempdir().unwrap();
        // burn_in=5, total=30: sweep 12 ⇒ Sampling phase (derived).
        let hb = Heartbeat::start(dir.path().to_path_buf(), 5, 30, Duration::from_millis(20));
        // initial write happens immediately
        std::thread::sleep(Duration::from_millis(40));
        hb.bump(12);
        std::thread::sleep(Duration::from_millis(40));
        let mid = read_progress(dir.path()).unwrap();
        assert!(matches!(mid.state, RunState::Running { sweep: 12, phase: Phase::Sampling, .. }));
        hb.finish(RunState::Done);
        assert_eq!(read_progress(dir.path()).unwrap().state, RunState::Done);
    }
}
