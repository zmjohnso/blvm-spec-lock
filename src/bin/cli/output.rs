//! Output formatting for verification results
//!
//! Formats results as human-readable, JSON, JUnit XML, or Markdown

use crate::cli::verify::{FunctionToVerify, VerificationResult};

/// Format verification results
pub fn format_results(results: &[(FunctionToVerify, VerificationResult)], format: &str) -> String {
    match format {
        "human" => format_human(results),
        "json" => format_json(results),
        "junit" => format_junit(results),
        "markdown" => format_markdown(results),
        _ => format_human(results),
    }
}

/// Format as human-readable text
fn format_human(results: &[(FunctionToVerify, VerificationResult)]) -> String {
    let mut output = String::new();
    output.push_str("Running BLVM Spec Lock verification...\n\n");

    for (func, result) in results {
        output.push_str(&format!(
            "{}::{}\n",
            func.file_path.display(),
            func.function_name
        ));

        match result {
            VerificationResult::Passed => {
                output.push_str("  ✅ Status: PASSED\n");
            }
            VerificationResult::Failed { contract, reason } => {
                output.push_str("  ❌ Status: FAILED\n");
                output.push_str(&format!("    Contract: {}\n", contract));
                output.push_str(&format!("    Reason: {}\n", reason));
            }
            VerificationResult::Partial {
                verified,
                total,
                reason,
            } => {
                output.push_str(&format!(
                    "  ⚠️  Status: PARTIAL ({} of {} verified)\n",
                    verified, total
                ));
                if let Some(r) = reason {
                    output.push_str(&format!("    Reason: {}\n", r));
                }
            }
            VerificationResult::NoContracts { section } => {
                output.push_str(&format!(
                    "  ❌ Status: FAILED (no contracts - section {}; add to Orange Paper or #[requires]/#[ensures])\n",
                    section
                ));
            }
            VerificationResult::NotImplemented => {
                output.push_str("  ⏳ Status: NOT IMPLEMENTED\n");
            }
        }
        output.push('\n');
    }

    // Summary
    let passed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Passed))
        .count();
    let failed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Failed { .. }))
        .count();
    let no_contracts = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::NoContracts { .. }))
        .count();
    let partial = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Partial { .. }))
        .count();

    output.push_str(&format!(
        "test result: {}. {} passed; {} failed; {} partial; 0 skipped\n",
        if failed > 0 || no_contracts > 0 {
            "FAILED"
        } else {
            "ok"
        },
        passed,
        failed + no_contracts,
        partial
    ));

    // Add duration and summary stats
    output.push_str(&format!("  Functions verified: {}\n", results.len()));

    output
}

/// Format as JSON
fn format_json(results: &[(FunctionToVerify, VerificationResult)]) -> String {
    use serde_json::json;

    let passed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Passed))
        .count();
    let failed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Failed { .. }))
        .count();
    let partial = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Partial { .. }))
        .count();

    let mut json_results = Vec::new();
    for (func, result) in results {
        let mut result_obj = json!({
            "file": func.file_path.to_string_lossy(),
            "function": func.function_name,
        });

        if let Some(ref section) = func.section {
            result_obj["section"] = json!(section);
        }

        match result {
            VerificationResult::Passed => {
                result_obj["status"] = json!("passed");
            }
            VerificationResult::Failed { contract, reason } => {
                result_obj["status"] = json!("failed");
                result_obj["contract"] = json!(contract);
                result_obj["reason"] = json!(reason);
            }
            VerificationResult::Partial {
                verified,
                total,
                reason,
            } => {
                result_obj["status"] = json!("partial");
                result_obj["verified"] = json!(*verified);
                result_obj["total"] = json!(*total);
                if let Some(r) = reason {
                    result_obj["reason"] = json!(r);
                }
            }
            VerificationResult::NoContracts { section } => {
                result_obj["status"] = json!("failed");
                result_obj["reason"] = json!(format!("no contracts (section {})", section));
            }
            VerificationResult::NotImplemented => {
                result_obj["status"] = json!("not_implemented");
            }
        }

        json_results.push(result_obj);
    }

    let no_contracts = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::NoContracts { .. }))
        .count();

    let output = json!({
        "summary": {
            "total": results.len(),
            "passed": passed,
            "failed": failed,
            "partial": partial,
            "no_contracts": no_contracts,
        },
        "results": json_results,
    });

    serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
}

/// Format as JUnit XML
fn format_junit(results: &[(FunctionToVerify, VerificationResult)]) -> String {
    use std::fmt::Write;

    let _passed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Passed))
        .count();
    let failed = results
        .iter()
        .filter(|(_, r)| {
            matches!(
                r,
                VerificationResult::Failed { .. } | VerificationResult::NoContracts { .. }
            )
        })
        .count();
    let total = results.len();

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    writeln!(
        &mut xml,
        "<testsuites name=\"blvm-spec-lock\" tests=\"{}\" failures=\"{}\" time=\"0.0\">",
        total, failed
    )
    .unwrap();
    writeln!(
        &mut xml,
        "  <testsuite name=\"verification\" tests=\"{}\" failures=\"{}\" time=\"0.0\">",
        total, failed
    )
    .unwrap();

    for (func, result) in results {
        let classname = func
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let status_attr = match result {
            VerificationResult::Passed => "",
            VerificationResult::Failed { .. } => " status=\"failed\"",
            VerificationResult::Partial { .. } => " status=\"partial\"",
            VerificationResult::NoContracts { .. } => " status=\"failed\"",
            VerificationResult::NotImplemented => " status=\"not_implemented\"",
        };

        writeln!(
            &mut xml,
            "    <testcase name=\"{}\" classname=\"{}\"{}>",
            func.function_name, classname, status_attr
        )
        .unwrap();

        if let Some(ref section) = func.section {
            write!(
                &mut xml,
                "      <properties>\n        <property name=\"section\" value=\"{}\"/>\n      </properties>\n",
                section
            ).unwrap();
        }

        if let VerificationResult::Failed { contract, reason } = result {
            writeln!(
                &mut xml,
                "      <failure message=\"{}\">Contract: {}</failure>",
                reason.replace('"', "&quot;"),
                contract.replace('"', "&quot;")
            )
            .unwrap();
        }

        xml.push_str("    </testcase>\n");
    }

    xml.push_str("  </testsuite>\n");
    xml.push_str("</testsuites>\n");

    xml
}

/// Format as Markdown
fn format_markdown(results: &[(FunctionToVerify, VerificationResult)]) -> String {
    let mut md = String::new();

    md.push_str("# BLVM Spec Lock Verification Report\n\n");
    // Simple timestamp (would use chrono if available)
    md.push_str("**Generated:** Verification Report\n\n");

    // Summary
    let passed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Passed))
        .count();
    let failed = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Failed { .. }))
        .count();
    let partial = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::Partial { .. }))
        .count();
    let no_contracts = results
        .iter()
        .filter(|(_, r)| matches!(r, VerificationResult::NoContracts { .. }))
        .count();

    md.push_str("## Summary\n\n");
    md.push_str(&format!("- **Total Functions:** {}\n", results.len()));
    md.push_str(&format!("- **Passed:** {} ✅\n", passed));
    md.push_str(&format!("- **Failed:** {} ❌\n", failed + no_contracts));
    md.push_str(&format!("- **Partial:** {} ⚠️\n\n", partial));

    // Results table
    md.push_str("## Results\n\n");
    md.push_str("| File | Function | Section | Status |\n");
    md.push_str("|------|----------|---------|--------|\n");

    for (func, result) in results {
        let file_name = func
            .file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let section = func.section.as_deref().unwrap_or("-");

        let status = match result {
            VerificationResult::Passed => "✅ Passed".to_string(),
            VerificationResult::Failed { .. } => "❌ Failed".to_string(),
            VerificationResult::Partial {
                verified,
                total,
                reason,
            } => {
                let mut s = format!("⚠️ Partial ({}/{})", verified, total);
                if let Some(r) = reason {
                    s.push_str(&format!(": {}", r));
                }
                s
            }
            VerificationResult::NoContracts { section } => {
                format!("❌ Failed (no contracts §{})", section)
            }
            VerificationResult::NotImplemented => "⏳ Not Implemented".to_string(),
        };

        md.push_str(&format!(
            "| `{}` | `{}` | {} | {} |\n",
            file_name, func.function_name, section, status
        ));
    }

    // Failed details
    let failed_results: Vec<_> = results
        .iter()
        .filter(|(_, r)| {
            matches!(
                r,
                VerificationResult::Failed { .. } | VerificationResult::NoContracts { .. }
            )
        })
        .collect();

    if !failed_results.is_empty() {
        md.push_str("\n## Failed Verifications\n\n");
        for (func, result) in failed_results {
            md.push_str(&format!(
                "### `{}::{}`\n\n",
                func.file_path.display(),
                func.function_name
            ));
            match result {
                VerificationResult::Failed { contract, reason } => {
                    md.push_str(&format!("- **Contract:** {}\n", contract));
                    md.push_str(&format!("- **Reason:** {}\n\n", reason));
                }
                VerificationResult::NoContracts { section } => {
                    md.push_str(&format!(
                        "- **Reason:** no contracts (section {})\n\n",
                        section
                    ));
                }
                _ => {}
            }
        }
    }

    md
}
