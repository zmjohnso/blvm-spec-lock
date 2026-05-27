//! **P1**: golden-ish snapshot for **`extract-formulas`** output on **`extract_formulas_smoke.md`** (deterministic scaffold).
//!
//! Updates: `INSTA_UPDATE=always cargo test -p blvm-spec-lock --test extract_formulas_snap --features z3`

use std::path::PathBuf;
use std::process::Command;

#[test]
fn extract_formulas_smoke_matches_snapshot() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let spec = manifest.join("tests/fixtures/extract_formulas_smoke.md");
    assert!(spec.is_file());

    let out = manifest.join("target/extract_formulas_golden_snap.rs");
    let _ = std::fs::remove_file(&out);

    let status = Command::new("cargo")
        .current_dir(&manifest)
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
        .status()
        .expect("cargo run extract-formulas");

    assert!(
        status.success(),
        "extract-formulas must exit 0 for snapshot regeneration"
    );

    let generated = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    let _ = std::fs::remove_file(&out);

    insta::assert_snapshot!("extract_formulas_smoke_generated", generated);
}
