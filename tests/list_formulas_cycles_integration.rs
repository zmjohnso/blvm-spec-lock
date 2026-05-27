//! **`list-formulas`**stderr cycle diagnostics (**P1 richer `Depends on`** — cyclic **F_* only** edges).

use std::path::PathBuf;
use std::process::Command;

#[test]
fn list_formulas_writes_cycle_warning_to_stderr() {
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
            "list-formulas",
            "--spec-path",
            md.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("cargo run list-formulas");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "list-formulas exits 0 (stderr is informational only).\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("list-formulas: warning:") && stderr.contains("cyclic"),
        "expected cycle warning\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("F_CycleA") && stderr.contains("F_CycleB"),
        "expected cycle vertices in stderr:\n{stderr}",
    );
}
