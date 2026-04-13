//! Negative verification test: deliberately wrong implementation must fail.
//!
//! Validates that the spec-lock verifier correctly rejects implementations
//! that violate their contracts.

use std::path::PathBuf;
use std::process::Command;

fn negative_crate_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/negative_crate")
}

#[test]
fn wrong_implementation_fails_verification() {
    let crate_path = negative_crate_path();
    assert!(
        crate_path.exists(),
        "Negative fixture crate not found at {}",
        crate_path.display()
    );

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cargo")
        .args([
            "run",
            "--features",
            "z3",
            "--bin",
            "cargo-spec-lock",
            "--",
            "verify",
            "--crate-path",
            crate_path.to_str().unwrap(),
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("Failed to run cargo spec-lock verify");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "Verification should FAIL for wrong implementation, but it passed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(
        stdout.contains("FAILED") || stderr.contains("FAILED"),
        "Output should mention failure.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(
        stdout.contains("get_block_subsidy") || stderr.contains("get_block_subsidy"),
        "Output should mention get_block_subsidy.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
