//! **P1**: golden snapshot for **`extract-property-tests`** on **`standalone_property.md`** + **`extract_bindings.toml`**.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn extract_property_tests_smoke_matches_snapshot() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let spec = manifest.join("tests/golden/standalone_property.md");
    let bindings = manifest.join("tests/fixtures/extract_bindings.toml");
    assert!(spec.is_file());
    assert!(bindings.is_file());

    let out = manifest.join("target/extract_property_tests_golden_snap.rs");
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
            "extract-property-tests",
            "--spec-path",
            spec.to_str().unwrap(),
            "--bindings-path",
            bindings.to_str().unwrap(),
            "--output",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("cargo run extract-property-tests");

    assert!(status.success(), "extract-property-tests must exit 0");

    let generated =
        std::fs::read_to_string(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    let _ = std::fs::remove_file(&out);

    insta::assert_snapshot!("extract_property_tests_smoke_generated", generated);
}
