# Changelog — `blvm-spec-lock`

All notable user-visible changes go here before release tagging. Crate versions are bumped by **`main`** CI (see `.github/workflows/ci.yml` **publish** job) unless you cut a manual patch.

## [Unreleased]

### Added

- **`list-formulas`** — cyclic **`F_*` → `F_*`** **`Depends on`** detection among **defined** formulas; warns on **stderr** (exit **0**). Fixture + test: **`tests/fixtures/formula_dep_cycle.md`**, **`cargo test --test list_formulas_cycles_integration`**.
- **`warn_formula_dep_diagnostics`** — same **missing `Depends on`** + **cycle** stderr on **`verify`**, **`verify-formulas`**, **`check-formulas`**, **`check-drift`**, **`coverage`**, **`summary`**, **`extract-*`**, and **`list-formulas`** (informational; does not change exit codes except existing gates).
- **`docs/VERIFY_JSON.md`** — merged **`verify`** subtree **`formula_registry`** + **`jq`** examples (**`.static`** / **`.z3_sat`** shapes).
- **`extract-formulas`** golden — **`cargo test --test extract_formulas_snap`**.
- **`extract-property-tests`** golden — **`cargo test --test extract_property_tests_snap`**.

### Tests

- **`verify_formulas_integration`**: Z3 **UNSAT** smoke on **`tests/fixtures/formula_verify_unsat_z3.md`** (**`F_UnsatContradiction`**: **`x < x`**); **`check-formulas`** cycle stderr on **`formula_dep_cycle.md`**.

### Notes

- **crates.io**: publishing uses org **`CARGO_REGISTRY_TOKEN`** (and optional **`REPO_ACCESS_TOKEN`** for the auto version-bump push). **`./scripts/publish-crates-io.sh`** remains the manual **`cargo publish -p`** order (**core** first).

## 0.1.12 and earlier

Historical releases — see crates.io **`versions`** lists and repo tags (**`v*`**).
