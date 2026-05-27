//! Integration smoke for **`cargo-spec-lock verify-formulas`**, **`check-formulas`**, and merged **`verify`**
//! on tiny **`F_*`** fixtures (**`tests/fixtures/formula_verify_smoke.md`**, **`formula_verify_bad_static.md`**).
//!
//! Uses `cargo run` (same pattern as **`negative_verification`**) so the binary **clap wiring** stays covered end-to-end.

use std::path::PathBuf;
use std::process::Command;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formula_verify_smoke.md")
}

fn bad_static_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formula_verify_bad_static.md")
}

fn no_section_fixture_crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no_section_attribute")
}

#[test]
fn verify_formulas_skip_z3_reports_formula_in_json_stdout() {
    let md = fixture_path();
    assert!(md.exists(), "fixture missing: {}", md.display());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "verify-formulas",
            "--skip-z3",
            "--format",
            "json",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run verify-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "verify-formulas --skip-z3 expected exit 0.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );

    assert!(
        stdout.contains("\"command\": \"verify-formulas\""),
        "expected verify-formulas JSON payload\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"formula_id\": \"F_VerifySmoke\""),
        "expected F_VerifySmoke row\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"static_fail\": 0"),
        "expected zero static failures\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"z3_sat_requested\": false"),
        "skip-z3 must suppress Z3 SAT\nstdout:\n{stdout}",
    );
}

#[test]
fn verify_formulas_default_requests_z3_sat_smoke_when_z3_linked() {
    let md = fixture_path();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Default `[features] default = ["z3"]` links Z3 — SAT smoke should pass for $$true$$.
    let output = Command::new("cargo")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "verify-formulas",
            "--format",
            "json",
            "--timeout",
            "30",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run verify-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "verify-formulas with Z3 expected exit 0 for trivial $$true$$.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );

    assert!(
        stdout.contains("\"z3_sat_requested\": true"),
        "default run should request Z3 SAT smoke\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"z3_sat_effective\": true"),
        "Z3-linked build must run SAT phase\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"status\": \"sat\""),
        "expected SAT row for tautology\nstdout:\n{stdout}",
    );
}

/// **P2** stretch: **`F_*`** body fails the same static LaTeX→`syn` gate as **`check-formulas`** / merged **`verify`** registry.
#[test]
fn verify_formulas_skip_z3_exits_nonzero_when_static_gate_fails() {
    let md = bad_static_fixture_path();
    assert!(md.exists(), "fixture missing: {}", md.display());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "verify-formulas",
            "--skip-z3",
            "--format",
            "json",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run verify-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "verify-formulas should fail when static gate fails.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stdout.contains("\"formula_id\": \"F_StaticFail\""),
        "expected F_StaticFail in JSON output\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"static_fail\": 1"),
        "expected one static failure in summary\nstdout:\n{stdout}",
    );
}

/// Mirrors merged **`verify`** static gate failures: **`check-formulas`** must exit non‑zero too.
#[test]
fn check_formulas_exits_nonzero_when_static_gate_fails() {
    let md = bad_static_fixture_path();
    assert!(md.exists(), "fixture missing: {}", md.display());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "check-formulas",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run check-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "check-formulas expected exit≠0 when **`F_StaticFail`** static gate fails.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("F_StaticFail") || stdout.contains("F_StaticFail"),
        "expected F_StaticFail in output\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

/// Merged **`cargo spec-lock verify`** runs the **`F_*`** registry gate before Rust function proofs; blocking static failures must fail fast.
#[test]
fn verify_exits_nonzero_when_merged_formula_registry_static_gate_fails() {
    let md = bad_static_fixture_path();
    let crate_dir = no_section_fixture_crate_dir();
    assert!(md.exists(), "fixture missing: {}", md.display());
    assert!(crate_dir.exists(), "fixture crate missing: {}", crate_dir.display());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .env("SPEC_LOCK_VERIFY_FORMULAS_SKIP_Z3", "1")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "verify",
            "--crate-path",
            crate_dir.to_str().unwrap(),
            "--spec-path",
            md.to_str().unwrap(),
            "--jobs",
            "1",
            "--timeout",
            "10",
            "--strict",
            "--format",
            "json",
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run verify");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "merged verify must fail fast on bad **`F_*`** registry (skip Z3 for formula phase).\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("F_StaticFail")
            || stdout.contains("F_StaticFail")
            || stdout.contains("\"formula_id\": \"F_StaticFail\""),
        "expected F_StaticFail surfaced in merged verify output\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

fn unsat_z3_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formula_verify_unsat_z3.md")
}

/// **P2**: static gate passes but Z3 SAT smoke finds **UNSAT** (e.g. **`x < x`**).
#[test]
fn verify_formulas_z3_sat_exits_nonzero_on_unsat_contradiction() {
    let md = unsat_z3_fixture_path();
    assert!(md.exists(), "fixture missing: {}", md.display());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "verify-formulas",
            "--format",
            "json",
            "--timeout",
            "30",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run verify-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "verify-formulas must fail when Z3 SAT smoke is UNSAT.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stdout.contains("\"formula_id\": \"F_UnsatContradiction\""),
        "expected F_UnsatContradiction row\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"status\": \"unsat\""),
        "expected z3_sat unsat status\nstdout:\n{stdout}",
    );
    assert!(
        stdout.contains("\"z3_sat_unsat\": 1"),
        "expected summary z3_sat_unsat=1\nstdout:\n{stdout}",
    );
}

#[test]
fn check_formulas_emits_cycle_warning_on_stderr_for_cyclic_depends_on() {
    let md = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formula_dep_cycle.md");
    assert!(md.exists(), "fixture {}", md.display());

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args([
            "run",
            "-q",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "check-formulas",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run check-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "check-formulas should pass static gate for cycle fixture.\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("check-formulas: warning:") && stderr.contains("cyclic"),
        "expected cycle diagnostic on stderr:\n{stderr}",
    );
    assert!(
        stderr.contains("F_CycleA") && stderr.contains("F_CycleB"),
        "expected cycle vertices:\n{stderr}",
    );
}
