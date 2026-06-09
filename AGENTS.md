# AGENTS.md

This repository has two distinct agent audiences; this file routes you to the
right one so they don't get conflated.

- **Using camdl** — an agent helping someone *write and fit models* with camdl
  (the installed `camdl`/`camdlc` binaries): see **[`docs/agents.md`](docs/agents.md)**,
  surfaced offline and version-matched as **`camdl docs agents`**. You do not
  need this repository to build models.

- **Developing camdl** — an agent working *inside this repository* on the OCaml
  compiler or the Rust runtime: see **[`CLAUDE.md`](CLAUDE.md)**. That is the
  contract for changing the software itself (high-risk surfaces, the
  golden/IR-schema human-loop rule, the commit conventions).

Keep modeler guidance in `docs/agents.md` and developer guidance in `CLAUDE.md`.
Do not move developer rules into `docs/agents.md` (a modeler never edits the
compiler), and do not assume a Claude agent reads this file — Claude Code loads
`CLAUDE.md`, not `AGENTS.md`.
