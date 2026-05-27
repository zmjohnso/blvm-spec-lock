//! End-to-end smoke for **`[experimental]` extract commands** (**`extract-formulas`**, **`extract-property-tests`**).
//!
//! Locks in a minimal **deterministic exit 0 path** + stable substrings in generated output (**P1 contract seeds** —
//! not **`verify`** assurance; see crate **`README`** and **`docs/LOCKING_MECHANISM.md`** (**experimental codegen**)).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SUFFIX: AtomicU64 = AtomicU64::new(0);

fn unique_extract_out(name: &str) -> PathBuf {
    let n = TMP_SUFFIX.fetch_add(1, Ordering::SeqCst);
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("extract_cmd_integration_{name}_{n}.rs"))
}

#[test]
fn extract_formulas_exits_zero_and_emits_subsidy_helper_skeleton() {
    let spec =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extract_formulas_smoke.md");
    assert!(spec.exists(), "fixture missing: {}", spec.display());

    let out = unique_extract_out("formulas");
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
            "extract-formulas",
            "--spec-path",
            spec.to_str().unwrap(),
            "--output",
            out.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run extract-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "extract-formulas expected exit 0.\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("Generated property test helpers"),
        "expected stderr success line\ngot:\n{stderr}",
    );

    let rust =
        std::fs::read_to_string(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    let _ = std::fs::remove_file(&out);

    assert!(
        rust.contains("Property test helpers generated from Orange Paper formulas"),
        "missing module doc banner\n---\n{rust}",
    );
    assert!(
        rust.contains("pub fn expected_getblocksubsidy_from_orange_paper(height: u64) -> i64"),
        "missing expected_getblocksubsidy helper signature",
    );
    assert!(
        rust.contains("initial_subsidy = 50 * C"),
        "missing halving scaffold from translate_formula_to_rust",
    );
}

#[test]
fn extract_property_tests_exits_zero_round_trip_with_bindings() {
    let spec =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/standalone_property.md");
    let bindings =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extract_bindings.toml");

    assert!(spec.exists(), "fixture missing: {}", spec.display());
    assert!(bindings.exists(), "fixture missing: {}", bindings.display());

    let out = unique_extract_out("property_tests");
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
            "extract-property-tests",
            "--spec-path",
            spec.to_str().unwrap(),
            "--bindings-path",
            bindings.to_str().unwrap(),
            "--output",
            out.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run extract-property-tests");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "extract-property-tests expected exit 0.\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("Generated 1 round-trip property test"),
        "expected stderr success line for one round-trip row\ngot:\n{stderr}",
    );

    let rust =
        std::fs::read_to_string(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    let _ = std::fs::remove_file(&out);

    assert!(
        rust.contains("AUTO-GENERATED from Orange Paper"),
        "missing banner comment",
    );
    assert!(
        rust.contains("// TODO: Golden round-trip - add strategy"),
        "expected placeholder when property name misses built-in codegen strategies",
    );
}
