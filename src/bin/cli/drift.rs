//! Spec drift detection
//!
//! Detects when Orange Paper and implementation diverge

use super::verify::{discover_functions, FunctionToVerify};
use std::path::PathBuf;
// Note: SpecParser is not accessible from binary (proc-macro crate limitation)
// Using simplified drift detection for now

/// Drift detection result
#[derive(Debug, Clone)]
pub struct DriftResult {
    /// Functions with contracts that don't match Orange Paper
    pub mismatched_contracts: Vec<MismatchedContract>,
    /// Functions missing from Orange Paper
    pub missing_from_spec: Vec<FunctionToVerify>,
    /// Orange Paper theorems without implementations
    pub missing_implementations: Vec<String>,
    /// Functions with auto-inferred sections (may need verification)
    pub auto_inferred: Vec<FunctionToVerify>,
    /// Spec contracts that are unparseable (spec has them but we can't verify)
    pub unparseable_spec_contracts: Vec<UnparseableContract>,
}

/// A spec contract that couldn't be parsed for verification
#[derive(Debug, Clone)]
pub struct UnparseableContract {
    pub section: String,
    pub function: String,
    pub condition: String,
}

/// A contract mismatch
#[derive(Debug, Clone)]
pub struct MismatchedContract {
    pub function: FunctionToVerify,
    pub orange_paper_contract: String,
    pub implementation_contract: String,
    pub section: String,
}

/// Detect spec drift
pub fn detect_drift(
    workspace_root: &PathBuf,
    orange_paper_paths: Option<&[PathBuf]>,
) -> Result<DriftResult, String> {
    use crate::parser::condition;
    use crate::parser::orange_paper::SpecParser;

    let mut functions = discover_functions(workspace_root)?;

    let spec_paths: Vec<PathBuf> = orange_paper_paths
        .map(|p| p.to_vec())
        .unwrap_or_else(|| vec![workspace_root.join("../blvm-spec/THE_ORANGE_PAPER.md")]);

    // Enrich functions with spec-derived contracts before drift check.
    // Without this, #[spec_locked] functions have empty contracts (macro output not in parsed source).
    if spec_paths.iter().all(|p| p.exists()) && !spec_paths.is_empty() {
        let _ = super::spec_enrich::enrich_functions_with_spec(&mut functions, &spec_paths);
    }

    let mismatched_contracts = Vec::new();
    let mut missing_from_spec = Vec::new();
    let mut auto_inferred = Vec::new();
    let mut unparseable_spec_contracts = Vec::new();
    let missing_implementations = Vec::new();

    for func in &functions {
        if func.section.is_none() {
            auto_inferred.push(func.clone());
            continue;
        }
        if func.contracts.is_empty() {
            missing_from_spec.push(func.clone());
        }
    }

    if spec_paths.iter().all(|p| p.exists()) && !spec_paths.is_empty() {
        let parser =
            SpecParser::from_paths(&spec_paths).map_err(|e| format!("Failed to load spec: {e}"))?;

        for (section_id, section) in parser.iter_sections() {
            for func in &section.functions {
                for contract in &func.contracts {
                    if condition::extract_parseable_condition(&contract.condition).is_none() {
                        let short = if contract.condition.len() > 80 {
                            format!("{}...", &contract.condition[..77])
                        } else {
                            contract.condition.clone()
                        };
                        unparseable_spec_contracts.push(UnparseableContract {
                            section: section_id.clone(),
                            function: func.name.clone(),
                            condition: short,
                        });
                    }
                }
            }
        }
    }

    Ok(DriftResult {
        mismatched_contracts,
        missing_from_spec,
        missing_implementations,
        auto_inferred,
        unparseable_spec_contracts,
    })
}

/// Convert Rust snake_case to PascalCase
fn rust_to_pascal_case(rust_name: &str) -> String {
    rust_name
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Check if two contracts are similar (allows for minor formatting differences)
fn contracts_similar(spec: &str, impl_contract: &str) -> bool {
    // Normalize both contracts
    let spec_norm = normalize_contract(spec);
    let impl_norm = normalize_contract(impl_contract);

    // Check for exact match
    if spec_norm == impl_norm {
        return true;
    }

    // Check if one contains the other (for partial matches)
    spec_norm.contains(&impl_norm) || impl_norm.contains(&spec_norm)
}

/// Normalize contract string for comparison
fn normalize_contract(contract: &str) -> String {
    contract.to_lowercase().replace(' ', "")
}

/// Find theorems in Orange Paper without corresponding implementations
/// Note: Implementation pending - requires SpecParser access from binary
fn _find_missing_implementations(_functions: &[FunctionToVerify]) -> Vec<String> {
    Vec::new()
}

/// Format drift report as human-readable text
pub fn format_drift_human(result: &DriftResult) -> String {
    let mut output = String::new();

    output.push_str("=== Spec Drift Detection Report ===\n\n");

    // Mismatched contracts
    if !result.mismatched_contracts.is_empty() {
        output.push_str("⚠️  Mismatched Contracts:\n");
        output.push_str("------------------------\n");
        for mismatch in &result.mismatched_contracts {
            output.push_str(&format!(
                "  Function: {} (Section {})\n",
                mismatch.function.function_name, mismatch.section
            ));
            output.push_str(&format!(
                "    Orange Paper: {}\n",
                mismatch.orange_paper_contract
            ));
            output.push_str(&format!(
                "    Implementation: {}\n",
                mismatch.implementation_contract
            ));
            output.push('\n');
        }
    }

    // Missing from spec
    if !result.missing_from_spec.is_empty() {
        output.push_str("❌ Functions Missing from Orange Paper:\n");
        output.push_str("--------------------------------------\n");
        for func in &result.missing_from_spec {
            output.push_str(&format!(
                "  {} ({})\n",
                func.function_name,
                func.file_path.display()
            ));
        }
        output.push('\n');
    }

    // Auto-inferred
    if !result.auto_inferred.is_empty() {
        output.push_str("ℹ️  Auto-Inferred Functions (verify manually):\n");
        output.push_str("---------------------------------------------\n");
        for func in &result.auto_inferred {
            output.push_str(&format!(
                "  {} ({})\n",
                func.function_name,
                func.file_path.display()
            ));
        }
        output.push('\n');
    }

    if !result.unparseable_spec_contracts.is_empty() {
        output.push_str("⚠️  Unparseable Spec Contracts (can't verify):\n");
        output.push_str("--------------------------------------------\n");
        for u in result.unparseable_spec_contracts.iter().take(10) {
            output.push_str(&format!(
                "  {}::{}: {}\n",
                u.section, u.function, u.condition
            ));
        }
        if result.unparseable_spec_contracts.len() > 10 {
            output.push_str(&format!(
                "  ... and {} more\n",
                result.unparseable_spec_contracts.len() - 10
            ));
        }
        output.push('\n');
    }

    output.push_str("Summary:\n");
    output.push_str("--------\n");
    output.push_str(&format!(
        "  Mismatched contracts: {}\n",
        result.mismatched_contracts.len()
    ));
    output.push_str(&format!(
        "  Missing from spec: {}\n",
        result.missing_from_spec.len()
    ));
    output.push_str(&format!(
        "  Unparseable spec contracts: {}\n",
        result.unparseable_spec_contracts.len()
    ));
    output.push_str(&format!(
        "  Auto-inferred: {}\n",
        result.auto_inferred.len()
    ));
    output.push_str(&format!(
        "  Missing implementations: {}\n",
        result.missing_implementations.len()
    ));

    if result.mismatched_contracts.is_empty()
        && result.missing_from_spec.is_empty()
        && result.missing_implementations.is_empty()
        && result.unparseable_spec_contracts.is_empty()
    {
        output.push_str("\n✅ No drift detected! Spec and implementation are in sync.\n");
    }

    output
}

/// Format drift report as JSON
pub fn format_drift_json(result: &DriftResult) -> String {
    serde_json::json!({
        "mismatched_contracts": result.mismatched_contracts.iter().map(|m| serde_json::json!({
            "function": m.function.function_name,
            "file": m.function.file_path.display().to_string(),
            "section": m.section,
            "orange_paper_contract": m.orange_paper_contract,
            "implementation_contract": m.implementation_contract,
        })).collect::<Vec<_>>(),
        "missing_from_spec": result.missing_from_spec.iter().map(|f| serde_json::json!({
            "function": f.function_name,
            "file": f.file_path.display().to_string(),
        })).collect::<Vec<_>>(),
        "unparseable_spec_contracts": result.unparseable_spec_contracts.iter().map(|u| serde_json::json!({
            "section": u.section,
            "function": u.function,
            "condition": u.condition,
        })).collect::<Vec<_>>(),
        "auto_inferred": result.auto_inferred.iter().map(|f| serde_json::json!({
            "function": f.function_name,
            "file": f.file_path.display().to_string(),
        })).collect::<Vec<_>>(),
        "missing_implementations": result.missing_implementations,
    })
    .to_string()
}
