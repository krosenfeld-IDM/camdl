# MRE-bundle fixture

A self-contained fit that exercises `camdl mre fit` — in particular the part
that's hard: capturing the covariate file the model `read()`s at **compile**
time, which appears nowhere in `fit.toml`.

```
mre/
  fit.toml                  # estimates beta, gamma; data = cases.tsv
  cases.tsv                 # observed weekly cases (2 streams)
  model/
    sir_patches.camdl       # 2-patch SIR; reads pop.tsv at compile time
    pop.tsv                 # the read() population table — the covariate
```

## Run it

From the repo root, after `make build`:

```sh
CAMDLC=ocaml/_build/default/bin/camdlc.exe \
  rust/target/release/camdl mre fit tests/fixtures/mre/fit.toml -b /tmp/demo.mre.tar.gz
```

(Or `make install` once, then just `camdl mre fit tests/fixtures/mre/fit.toml`.)

You'll see the consent banner for `cases.tsv` and a `demo.mre.tar.gz`. Inspect
it:

```sh
tar tzf /tmp/demo.mre.tar.gz
#   demo.mre/fit.toml
#   demo.mre/cases.tsv
#   demo.mre/model/sir_patches.camdl
#   demo.mre/model/pop.tsv        <-- the read() covariate, captured
#   demo.mre/manifest.toml
#   demo.mre/README.md
```

The bundle is self-contained: unpack it anywhere and `camdl fit run fit.toml`
reproduces (every path is bundle-relative).

Try `--no-data` for a structure-only bundle (omits `cases.tsv`).
