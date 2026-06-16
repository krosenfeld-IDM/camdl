#!/usr/bin/env Rscript
# gh#166 Phase B incidence oracle — R deSolve::lsoda reference.
#
# Second independent implementation, in a second language: the same canonical
# RHS augmented with one cumulative-incidence variable per tracked transition,
# integrated with deSolve::lsoda (Hindmarsh/Petzold, adaptive stiff/non-stiff),
# sampled at the model's output grid. Per-interval incidence + prevalence are
# written to ref/<model>__desolve_lsoda.tsv.
#
# Params MUST match the --set values used to compile models/<model>.ir.json.
# CI never runs this — it reads the committed ref/*.tsv fixtures.

suppressMessages(library(deSolve))

# run.sh cd's into gen/ before invoking this, so ../ref is the cache dir.
ref_dir <- normalizePath(file.path("..", "ref"), mustWork = FALSE)

write_ref <- function(name, comps, incs, times, prev, per) {
  path <- file.path(ref_dir, sprintf("%s__desolve_lsoda.tsv", name))
  header <- c("t", comps, paste0("inc_", incs))
  con <- file(path, "w")
  writeLines(paste(header, collapse = "\t"), con)
  for (k in seq_along(times)) {
    row <- c(sprintf("%.1f", times[k]),
             sprintf("%.10e", prev[k, ]),
             sprintf("%.10e", per[k, ]))
    writeLines(paste(row, collapse = "\t"), con)
  }
  close(con)
  cat(sprintf("wrote %s (%d rows)\n", path, length(times)))
}

run_model <- function(name, comps, incs, rhs, y0, p, t_end, step) {
  times <- seq(0, t_end, by = step)
  nc <- length(comps); ni <- length(incs)
  out <- lsoda(y = y0, times = times, func = rhs, parms = p,
               rtol = 1e-11, atol = 1e-11)
  out <- as.matrix(out)
  prev <- out[, 2:(1 + nc), drop = FALSE]
  cum  <- out[, (2 + nc):(1 + nc + ni), drop = FALSE]
  per <- rbind(rep(0, ni), cum[-1, , drop = FALSE] - cum[-nrow(cum), , drop = FALSE])
  write_ref(name, comps, incs, times, prev, per)
}

# ── SIR ──────────────────────────────────────────────────────────────────────
sir_rhs <- function(t, y, p) {
  S <- y[1]; I <- y[2]; R <- y[3]
  N <- S + I + R
  inf <- p$beta * S * I / N; rec <- p$gamma * I
  list(c(-inf, inf - rec, rec, inf, rec))
}
run_model("sir", c("S", "I", "R"), c("infection", "recovery"), sir_rhs,
          y0 = c(100000 - 10, 10, 0, 0, 0),
          p = list(beta = 0.5, gamma = 0.25), t_end = 60, step = 1.0)

# ── SEIR ─────────────────────────────────────────────────────────────────────
seir_rhs <- function(t, y, p) {
  S <- y[1]; E <- y[2]; I <- y[3]; R <- y[4]
  N <- S + E + I + R
  inf <- p$beta * S * I / N; prog <- p$sigma * E; rec <- p$gamma * I
  list(c(-inf, inf - prog, prog - rec, rec, inf, prog, rec))
}
run_model("seir", c("S", "E", "I", "R"), c("infection", "progression", "recovery"), seir_rhs,
          y0 = c(100000 - 10, 0, 10, 0, 0, 0, 0),
          p = list(beta = 0.6, sigma = 0.2, gamma = 0.25), t_end = 80, step = 1.0)

# ── TB (2-stage latency) ─────────────────────────────────────────────────────
tb_rhs <- function(t, y, p) {
  S <- y[1]; Lf <- y[2]; Ls <- y[3]; I <- y[4]; R <- y[5]
  N <- S + Lf + Ls + I + R
  inf <- p$beta * S * I / N
  fp <- p$phi * Lf; st <- p$kappa * Lf; re <- p$omega * Ls; rec <- p$gamma * I
  list(c(-inf, inf - (fp + st), st - re, fp + re - rec, rec, inf, fp, st, re, rec))
}
run_model("tb", c("S", "Lf", "Ls", "I", "R"),
          c("infection", "fast_prog", "stabilize", "reactivate", "recovery"), tb_rhs,
          y0 = c(100000 - 50, 0, 0, 50, 0, 0, 0, 0, 0, 0),
          p = list(beta = 0.3, phi = 0.02, kappa = 0.008, omega = 0.0003, gamma = 0.02),
          t_end = 365, step = 1.0)
