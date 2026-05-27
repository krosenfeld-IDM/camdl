//! gh#110. Shared particle-filter degeneracy watchdog.
//!
//! `bootstrap_filter` and IF2's inner per-iteration PF loop both
//! advance particles, reweight, and resample at each observation
//! window. Both have the same implicit contract: *return a finite
//! log-likelihood in bounded time for any θ within the parameter
//! bounds.* That contract silently breaks at bound-box extremes
//! (σ at the upper bound, R₀ ≈ 50, …) where ESS collapses to ~1
//! and the filter runs effectively forever producing no usable
//! information.
//!
//! This module collects the three detectable failure modes into one
//! helper so the two filter loops agree on what counts as "degenerate"
//! and what the bail looks like. Call from each loop after the
//! per-window weight-normalisation step:
//!
//! ```ignore
//! ess_history.push(swarm.ess());
//! if let Some(kind) = check_pf_degeneracy(
//!     &ess_history, t0.elapsed(), obs_idx, dead_count, n_particles,
//! ) {
//!     return Err(SimError::PFDegenerate {
//!         kind, obs_window: obs_idx, elapsed_s: t0.elapsed().as_secs_f64(),
//!     });
//! }
//! ```
//!
//! Thresholds are hardcoded (no CLI flag). The acceptance criterion
//! on gh#110 is explicit: no `--allow-pf-degeneracy` opt-out until
//! a user demands an override. `WALLCLOCK_TIMEOUT_S = 120` per-call
//! is generous on healthy production models (seconds to tens of
//! seconds) and tight enough to bail before a multi-chain run loses
//! tens of minutes to one bad chain. `ESS_FLOOR = 2.0` over
//! `ESS_COLLAPSE_WINDOWS = 3` consecutive obs windows on a 200-obs
//! SEIR fit means we bail only if the filter is producing one usable
//! particle across 1.5% of the observation series — generous
//! against legitimate peak-dynamics dips, tight against sustained
//! collapse.

use std::time::Duration;

use crate::error::PFDegenerateKind;

/// gh#110. Effective sample size below this value at an observation
/// window counts as "collapsed" for that window. Floor of 1.0 is
/// "literally one particle dominates"; 2.0 gives a small margin and
/// matches conventional resampling triggers.
pub const ESS_FLOOR: f64 = 2.0;

/// gh#110. Number of *consecutive* obs windows below `ESS_FLOOR`
/// required to bail. Single-window dips are normal at epidemic
/// peaks; sustained collapse is the pathology this watchdog catches.
pub const ESS_COLLAPSE_WINDOWS: usize = 3;

/// gh#110. Per-call wall-clock timeout. A healthy PF eval on
/// production models is seconds-to-tens-of-seconds; 2 minutes is
/// generous enough to never false-positive on legitimate slow
/// models and tight enough that a 6-chain run can't lose 40+
/// minutes to one bad chain.
pub const WALLCLOCK_TIMEOUT_S: u64 = 120;

/// gh#110. Return the degeneracy mode if the filter has bailed at
/// this observation window, otherwise `None`.
///
/// Inputs:
/// - `ess_history` — every ESS recorded so far, length = obs_windows
///   processed. Only the last `ESS_COLLAPSE_WINDOWS` are inspected.
/// - `elapsed` — wall-clock since the filter call started.
/// - `_obs_window` — the just-processed window index. Reserved for
///   future diagnostics that want to localise the bail (e.g.
///   logging "ESS dropped at obs 47–50"). The current implementation
///   doesn't read it but plumbing it through the helper keeps the
///   call sites honest about what window they're reporting.
/// - `dead_count` — number of particles currently marked dead. Used
///   only for the `AllParticlesDead` check; pass 0 when the caller
///   doesn't track per-particle death (e.g. IF2's inner loop does
///   not, since its `process.step` errors propagate immediately).
/// - `n_particles` — total particles in the swarm.
///
/// Discrimination order (deterministic on tie cases):
///   1. `AllParticlesDead` — the limit case of ESS collapse,
///      cheap and diagnostically distinct; check first.
///   2. `WallClockExceeded` — independent of swarm state, fires
///      even if ESS looks healthy (e.g. step() is just slow).
///   3. `EssCollapsed` — requires `ESS_COLLAPSE_WINDOWS` history.
pub fn check_pf_degeneracy(
    ess_history: &[f64],
    elapsed: Duration,
    _obs_window: usize,
    dead_count: usize,
    n_particles: usize,
) -> Option<PFDegenerateKind> {
    // AllParticlesDead: every particle hit a per-particle-recoverable
    // error. Resampling on the next step would have zero weight
    // everywhere; bail before the divide-by-zero.
    if n_particles > 0 && dead_count >= n_particles {
        return Some(PFDegenerateKind::AllParticlesDead);
    }

    // WallClockExceeded: independent of swarm state. A filter that
    // runs >120s/call is either grossly under-particled or stuck on
    // an absorbing dynamic; either way the chain runner needs to
    // hear about it before the user loses meaningful wall-clock.
    if elapsed.as_secs() >= WALLCLOCK_TIMEOUT_S {
        return Some(PFDegenerateKind::WallClockExceeded);
    }

    // EssCollapsed: K consecutive obs windows at or below the floor.
    // Single-window dips during epidemic peaks are not pathology;
    // sustained collapse is. We need at least K windows of history
    // before this can fire — if the filter bails sooner it's via
    // WallClockExceeded (or AllParticlesDead).
    if ess_history.len() >= ESS_COLLAPSE_WINDOWS {
        let tail = &ess_history[ess_history.len() - ESS_COLLAPSE_WINDOWS..];
        if tail.iter().all(|&ess| ess <= ESS_FLOOR) {
            return Some(PFDegenerateKind::EssCollapsed {
                last_ess: tail.to_vec(),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Healthy run: ESS comfortably above the floor, fast wall-clock,
    /// no dead particles. Must NOT bail.
    #[test]
    fn healthy_run_returns_none() {
        let ess = vec![800.0, 750.0, 820.0, 790.0, 810.0];
        let elapsed = Duration::from_secs(5);
        assert!(check_pf_degeneracy(&ess, elapsed, 4, 0, 1000).is_none());
    }

    /// A single-window dip below the floor is normal during epidemic
    /// peaks. Must NOT trigger EssCollapsed.
    #[test]
    fn single_window_dip_does_not_trigger() {
        let ess = vec![800.0, 1.5, 750.0]; // mid-series dip
        assert!(check_pf_degeneracy(&ess, Duration::from_secs(1), 2, 0, 1000).is_none());
    }

    /// Two consecutive low windows still under K=3 threshold. Must NOT trigger.
    #[test]
    fn two_consecutive_low_windows_do_not_trigger() {
        let ess = vec![800.0, 1.5, 1.5];
        assert!(check_pf_degeneracy(&ess, Duration::from_secs(1), 2, 0, 1000).is_none());
    }

    /// K=3 consecutive windows at or below the floor → EssCollapsed
    /// with the K-window history attached.
    #[test]
    fn k_consecutive_low_windows_trigger_ess_collapsed() {
        let ess = vec![800.0, 1.8, 1.2, 1.5];
        let kind = check_pf_degeneracy(&ess, Duration::from_secs(1), 3, 0, 1000)
            .expect("should bail with ESS collapse");
        match kind {
            PFDegenerateKind::EssCollapsed { last_ess } => {
                assert_eq!(last_ess, vec![1.8, 1.2, 1.5]);
            }
            other => panic!("expected EssCollapsed, got {:?}", other),
        }
    }

    /// Boundary case: ESS exactly == ESS_FLOOR (= 2.0) counts as
    /// collapsed (the comparison is `<=`). One usable particle plus a
    /// thin margin is exactly the pathology we want to catch.
    #[test]
    fn ess_equal_to_floor_counts_as_collapsed() {
        let ess = vec![ESS_FLOOR, ESS_FLOOR, ESS_FLOOR];
        let kind = check_pf_degeneracy(&ess, Duration::from_secs(0), 2, 0, 1000)
            .expect("should bail with ESS at the floor");
        assert!(matches!(kind, PFDegenerateKind::EssCollapsed { .. }));
    }

    /// Wall-clock at or above WALLCLOCK_TIMEOUT_S → WallClockExceeded,
    /// even with healthy ESS. The filter might be stuck in step().
    #[test]
    fn wall_clock_timeout_triggers() {
        let ess = vec![800.0, 750.0]; // healthy
        let elapsed = Duration::from_secs(WALLCLOCK_TIMEOUT_S);
        let kind = check_pf_degeneracy(&ess, elapsed, 1, 0, 1000)
            .expect("should bail on wall-clock");
        assert!(matches!(kind, PFDegenerateKind::WallClockExceeded));
    }

    /// Wall-clock just under the timeout must NOT trigger.
    #[test]
    fn wall_clock_just_under_does_not_trigger() {
        let ess = vec![800.0];
        let elapsed = Duration::from_secs(WALLCLOCK_TIMEOUT_S - 1);
        assert!(check_pf_degeneracy(&ess, elapsed, 0, 0, 1000).is_none());
    }

    /// All particles dead → AllParticlesDead, even with no ESS
    /// history and zero wall-clock.
    #[test]
    fn all_particles_dead_triggers() {
        let ess: Vec<f64> = vec![];
        let kind = check_pf_degeneracy(&ess, Duration::from_secs(0), 0, 1000, 1000)
            .expect("should bail with AllParticlesDead");
        assert!(matches!(kind, PFDegenerateKind::AllParticlesDead));
    }

    /// AllParticlesDead has priority over ESS collapse: when every
    /// particle is dead, the K-window history is irrelevant — the
    /// more specific diagnostic wins.
    #[test]
    fn all_particles_dead_wins_over_ess_collapse() {
        let ess = vec![0.0, 0.0, 0.0];
        let kind = check_pf_degeneracy(&ess, Duration::from_secs(0), 2, 500, 500)
            .expect("should bail");
        assert!(matches!(kind, PFDegenerateKind::AllParticlesDead),
            "AllParticlesDead must take priority over EssCollapsed");
    }

    /// `dead_count == 0` with `n_particles == 0` must NOT trigger
    /// AllParticlesDead (vacuous case — guards against the trivial
    /// `0 >= 0` true that would always fire on an empty swarm).
    #[test]
    fn empty_swarm_does_not_trigger_all_dead() {
        // No way an empty swarm should hit a watchdog — this guards
        // against an off-by-one that returns AllParticlesDead on
        // every call with n_particles=0.
        assert!(check_pf_degeneracy(&[], Duration::from_secs(0), 0, 0, 0).is_none());
    }

    /// Exactly ESS_COLLAPSE_WINDOWS-1 windows of history with all at
    /// floor: not enough history yet, must NOT trigger.
    #[test]
    fn insufficient_history_does_not_trigger() {
        assert!(ESS_COLLAPSE_WINDOWS >= 2,
            "test assumes K-window threshold >= 2");
        let short: Vec<f64> = (0..ESS_COLLAPSE_WINDOWS - 1).map(|_| 0.5).collect();
        assert!(check_pf_degeneracy(&short, Duration::from_secs(0), 0, 0, 1000).is_none());
    }
}
