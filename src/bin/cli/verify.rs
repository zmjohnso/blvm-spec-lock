//! Verification orchestration
//!
//! Discovers functions, extracts contracts, and runs verification

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use syn::{Attribute, File, ImplItem, ImplItemFn, ItemFn};

/// Convert ImplItemFn to ItemFn so Z3 can translate the function body.
/// Enables implementation validation for impl-block methods.
fn impl_item_fn_to_item_fn(impl_fn: &ImplItemFn) -> ItemFn {
    ItemFn {
        attrs: impl_fn.attrs.clone(),
        vis: impl_fn.vis.clone(),
        sig: impl_fn.sig.clone(),
        block: Box::new(impl_fn.block.clone()),
    }
}

/// Simplified contract structure for CLI
#[derive(Debug, Clone)]
pub struct Contract {
    pub contract_type: ContractType,
    pub condition: String,
    pub expr: Option<syn::Expr>, // Parsed expression for static checker
    /// True when this contract was auto-derived from the Orange Paper spec (not manually written).
    /// Spec-derived failures are demoted to Partial (see `demote_if_all_spec_derived`).
    pub is_spec_derived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractType {
    Requires,
    Ensures,
}

/// Extract contracts from a function
fn extract_contracts(func: &ItemFn) -> Vec<Contract> {
    let mut contracts = Vec::new();

    for attr in &func.attrs {
        let path = attr.path();

        // Check for #[requires(...)] or #[ensures(...)]
        let is_requires = path.is_ident("requires")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "requires");

        let is_ensures = path.is_ident("ensures")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "ensures");

        if is_requires || is_ensures {
            // Parse the condition expression from the attribute
            // The attribute format is: #[requires(condition)] or #[ensures(condition)]
            if let Ok(expr) = attr.parse_args::<syn::Expr>() {
                // Convert expression to string for storage
                let condition_str = quote::quote!(#expr).to_string();

                contracts.push(Contract {
                    contract_type: if is_requires {
                        ContractType::Requires
                    } else {
                        ContractType::Ensures
                    },
                    condition: condition_str,
                    expr: Some(expr),
                    is_spec_derived: false,
                });
            } else {
                // If parsing fails, store as string only
                let condition_str = quote::quote!(#attr).to_string();
                contracts.push(Contract {
                    contract_type: if is_requires {
                        ContractType::Requires
                    } else {
                        ContractType::Ensures
                    },
                    condition: condition_str,
                    expr: None,
                    is_spec_derived: false,
                });
            }
        }
    }

    contracts
}

/// A function to verify
#[derive(Debug, Clone)]
pub struct FunctionToVerify {
    pub file_path: PathBuf,
    pub function_name: String,
    pub contracts: Vec<Contract>,
    pub section: Option<String>,
    /// When **`Some`**, **`#[spec_locked]`** named a **`F_*`** id (positional, combined, or `function =`).
    pub formula_anchor: Option<String>,
    /// When **`Some`**, **`#[spec_locked]`** named a **`C_*`** consensus-constant stable id (**`constants_stable_id_map`**).
    pub constant_anchor: Option<String>,
    pub function_sig: Option<syn::ItemFn>, // Store function signature for type inference
}

/// Recursively collect all `.rs` files under `dir`, skipping `target/`, `.git/`, `.cargo/`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let path_str = path.to_string_lossy();
        if path_str.contains("/target/")
            || path_str.contains("/.git/")
            || path_str.contains("/.cargo/")
        {
            continue;
        }
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Discover all functions with #[spec_locked] attributes
pub fn discover_functions(workspace_root: &Path) -> Result<Vec<FunctionToVerify>, String> {
    let mut functions = Vec::new();
    let mut errors = Vec::new();

    let mut rs_files = Vec::new();
    collect_rs_files(workspace_root, &mut rs_files);

    for path in &rs_files {
        // Only process .rs files
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            match parse_file_for_functions(path) {
                Ok(mut file_functions) => {
                    functions.append(&mut file_functions);
                }
                Err(e) => {
                    // Collect errors but continue processing
                    errors.push(format!("{}: {}", path.display(), e));
                }
            }
        }
    }

    // If we have functions, return them even if there were some errors
    // (errors might be from files that don't have spec_locked functions)
    if !functions.is_empty() || errors.is_empty() {
        Ok(functions)
    } else {
        Err(format!(
            "Failed to discover functions:\n{}",
            errors.join("\n")
        ))
    }
}

/// Parse a Rust file for functions with #[spec_locked]
fn parse_file_for_functions(file_path: &std::path::Path) -> Result<Vec<FunctionToVerify>, String> {
    let content = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

    let ast: File = syn::parse_file(&content)
        .map_err(|e| format!("Failed to parse {}: {}", file_path.display(), e))?;

    let mut functions = Vec::new();

    for item in ast.items {
        match item {
            syn::Item::Fn(func) if has_spec_locked(&func.attrs) => {
                functions.push(make_function_to_verify(
                    file_path,
                    &func.sig.ident.to_string(),
                    &func.attrs,
                    &func,
                ));
            }
            syn::Item::Impl(impl_item) => {
                for assoc_item in &impl_item.items {
                    if let ImplItem::Fn(impl_fn) = assoc_item {
                        if has_spec_locked(&impl_fn.attrs) {
                            let contracts = extract_contracts_from_attrs(&impl_fn.attrs);
                            let section = extract_section(&impl_fn.attrs);
                            let item_fn = impl_item_fn_to_item_fn(impl_fn);
                            functions.push(FunctionToVerify {
                                file_path: file_path.to_path_buf(),
                                function_name: impl_fn.sig.ident.to_string(),
                                contracts,
                                section,
                                formula_anchor: extract_formula_anchor(&impl_fn.attrs),
                                constant_anchor: extract_constant_anchor(&impl_fn.attrs),
                                function_sig: Some(item_fn),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(functions)
}

fn make_function_to_verify(
    file_path: &std::path::Path,
    function_name: &str,
    attrs: &[Attribute],
    func: &ItemFn,
) -> FunctionToVerify {
    let contracts = extract_contracts(func);
    let section = extract_section(attrs);
    FunctionToVerify {
        file_path: file_path.to_path_buf(),
        function_name: function_name.to_string(),
        contracts,
        section,
        formula_anchor: extract_formula_anchor(attrs),
        constant_anchor: extract_constant_anchor(attrs),
        function_sig: Some(func.clone()),
    }
}

/// Extract contracts from attributes (used for impl methods which use ImplItemFn)
fn extract_contracts_from_attrs(attrs: &[Attribute]) -> Vec<Contract> {
    let mut contracts = Vec::new();
    for attr in attrs {
        let path = attr.path();
        let is_requires = path.is_ident("requires")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "requires");
        let is_ensures = path.is_ident("ensures")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "ensures");
        if is_requires || is_ensures {
            if let Ok(expr) = attr.parse_args::<syn::Expr>() {
                contracts.push(Contract {
                    contract_type: if is_requires {
                        ContractType::Requires
                    } else {
                        ContractType::Ensures
                    },
                    condition: quote::quote!(#expr).to_string(),
                    expr: Some(expr),
                    is_spec_derived: false,
                });
            } else {
                contracts.push(Contract {
                    contract_type: if is_requires {
                        ContractType::Requires
                    } else {
                        ContractType::Ensures
                    },
                    condition: quote::quote!(#attr).to_string(),
                    expr: None,
                    is_spec_derived: false,
                });
            }
        }
    }
    contracts
}

/// Check if function has #[spec_locked] attribute (including inside #[cfg_attr(...)])
fn has_spec_locked(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        if path.is_ident("spec_locked")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "spec_locked")
        {
            return true;
        }
        // cfg_attr(feature = "...", spec_locked("10.1"))
        if path.is_ident("cfg_attr") {
            let tokens = quote::quote!(#attr).to_string();
            return tokens.contains("spec_locked");
        }
        false
    })
}

/// Second literal anchored to **`F_*`** or **`C_*`** (`spec_locked("§", "F_x"|"C_x")`, `§::Id`, **`function = "…"`**, **`cfg_attr`** nests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecondarySpecAnchor {
    Formula(String),
    Constant(String),
}

fn classify_secondary_anchor(id: &str) -> Option<SecondarySpecAnchor> {
    if id.starts_with("F_")
        && id.len() > 2
        && id[2..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Some(SecondarySpecAnchor::Formula(id.to_string()));
    }
    if id.starts_with("C_")
        && id.len() > 2
        && id[2..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Some(SecondarySpecAnchor::Constant(id.to_string()));
    }
    None
}

/// Extract **`F_*`** / **`C_*`** second anchor from **`#[spec_locked]`** / **`cfg_attr`**.
pub fn extract_secondary_spec_anchor(attrs: &[Attribute]) -> Option<SecondarySpecAnchor> {
    static RE_NAMED: OnceLock<regex::Regex> = OnceLock::new();
    static RE_COMBO: OnceLock<regex::Regex> = OnceLock::new();
    static RE_DUAL: OnceLock<regex::Regex> = OnceLock::new();

    fn re_named() -> &'static regex::Regex {
        RE_NAMED.get_or_init(|| {
            regex::Regex::new(r#"function\s*=\s*"((?:F_|C_)[A-Za-z0-9_]+)""#)
                .expect("secondary anchor named-regex")
        })
    }

    fn re_combo() -> &'static regex::Regex {
        RE_COMBO.get_or_init(|| {
            regex::Regex::new(
                r#"spec_locked\s*\(\s*"((?:\d+)(?:\.\d+)*)::((?:F_|C_)[A-Za-z0-9_]+)"\s*\)"#,
            )
            .expect("secondary anchor combo regex")
        })
    }

    fn re_dual() -> &'static regex::Regex {
        RE_DUAL.get_or_init(|| {
            regex::Regex::new(r#"spec_locked\s*\(\s*"[^"]*"\s*,\s*"((?:F_|C_)[A-Za-z0-9_]+)""#)
                .expect("secondary anchor dual regex")
        })
    }

    for attr in attrs {
        let path = attr.path();
        let tokens = quote::quote!(#attr).to_string();

        let is_direct = path.is_ident("spec_locked")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "spec_locked");

        let is_cfg_nested = path.is_ident("cfg_attr") && tokens.contains("spec_locked");

        if !(is_direct || is_cfg_nested) {
            continue;
        }

        if let Some(c) = re_named().captures(&tokens) {
            if let Some(id) = c.get(1).and_then(|m| classify_secondary_anchor(m.as_str())) {
                return Some(id);
            }
        }
        if let Some(c) = re_combo().captures(&tokens) {
            if let Some(id) = c.get(2).and_then(|m| classify_secondary_anchor(m.as_str())) {
                return Some(id);
            }
        }
        if let Some(c) = re_dual().captures(&tokens) {
            if let Some(id) = c.get(1).and_then(|m| classify_secondary_anchor(m.as_str())) {
                return Some(id);
            }
        }
    }
    None
}

/// Second literal anchored to **`F_*`** (`spec_locked("§", "F_x")`, `spec_locked("§::F_x")`,
/// `function = "F_x"`, **`cfg_attr`** nests included).
pub fn extract_formula_anchor(attrs: &[Attribute]) -> Option<String> {
    match extract_secondary_spec_anchor(attrs) {
        Some(SecondarySpecAnchor::Formula(id)) => Some(id),
        _ => None,
    }
}

/// **`#[spec_locked]`** second literal **`C_*`** (consensus stable id).
pub fn extract_constant_anchor(attrs: &[Attribute]) -> Option<String> {
    match extract_secondary_spec_anchor(attrs) {
        Some(SecondarySpecAnchor::Constant(id)) => Some(id),
        _ => None,
    }
}
fn extract_section(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        let path = attr.path();
        let tokens = quote::quote!(#attr).to_string();
        let is_spec_locked = path.is_ident("spec_locked")
            || (path.segments.len() == 2
                && path.segments[0].ident == "blvm_spec_lock"
                && path.segments[1].ident == "spec_locked")
            || (path.is_ident("cfg_attr") && tokens.contains("spec_locked"));

        if is_spec_locked {
            // Try to parse the section from the attribute tokens
            // Format: #[spec_locked("6.1")] or #[cfg_attr(feature = "x", spec_locked("10.1"))]
            // Look for spec_locked("X.Y") pattern - section numbers after spec_locked
            if let Some(spec_pos) = tokens.find("spec_locked") {
                let after_spec = &tokens[spec_pos..];
                if let Some(start) = after_spec.find('"') {
                    if let Some(end) = after_spec[start + 1..].find('"') {
                        let section_raw = &after_spec[start + 1..start + 1 + end];
                        if section_raw.chars().any(|c| c.is_ascii_digit()) {
                            let section_normalized = section_raw.split_once("::").map_or_else(
                                || section_raw.to_string(),
                                |(lhs, rhs)| {
                                    if rhs.starts_with("F_") || rhs.starts_with("C_") {
                                        lhs.to_string()
                                    } else {
                                        section_raw.to_string()
                                    }
                                },
                            );
                            return Some(section_normalized);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Verify a single function.
/// `timeout_secs`: Z3 solver timeout in seconds (0 = use default 5s).
#[allow(unused_variables)]
pub fn verify_function(function: &FunctionToVerify, timeout_secs: u64) -> VerificationResult {
    if function.contracts.is_empty() {
        return VerificationResult::NoContracts {
            section: function.section.clone().unwrap_or_else(|| "?".to_string()),
        };
    }

    // Verification flow:
    // 1. Try static checks first (fast, no Z3 needed)
    // 2. If static checks can't verify, use Z3 (if available)
    // 3. Return appropriate result

    let mut verified_count = 0;
    // (contract_type_str, reason, is_spec_derived)
    let mut failed_contracts: Vec<(String, String, bool)> = Vec::new();
    let mut requires_z3_count = 0;
    let translation_error = RefCell::new(None::<String>);

    // Separate requires and ensures contracts
    let requires_contracts: Vec<_> = function
        .contracts
        .iter()
        .filter(|c| c.contract_type == ContractType::Requires)
        .collect();
    let ensures_contracts: Vec<_> = function
        .contracts
        .iter()
        .filter(|c| c.contract_type == ContractType::Ensures)
        .collect();

    // Verify requires contracts first
    for contract in &requires_contracts {
        // Basic validation: check if contract condition is non-empty
        if contract.condition.trim().is_empty() {
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Empty contract condition".to_string(),
                contract.is_spec_derived,
            ));
            continue;
        }

        // Try static checking if we have a parsed expression
        if let Some(ref expr) = contract.expr {
            match check_contract_statically(expr, contract.contract_type) {
                StaticCheck::Passed => {
                    verified_count += 1;
                }
                StaticCheck::Failed(reason) => {
                    failed_contracts.push((
                        format!("{:?}", contract.contract_type),
                        reason,
                        contract.is_spec_derived,
                    ));
                }
                StaticCheck::RequiresZ3 => {
                    requires_z3_count += 1;
                    #[cfg(feature = "z3")]
                    {
                        if let Err(e) = verify_with_z3(
                            contract,
                            function.function_sig.as_ref(),
                            &[],
                            timeout_secs,
                        ) {
                            failed_contracts.push((
                                format!("{:?}", contract.contract_type),
                                format!("Z3 verification failed: {e}"),
                                contract.is_spec_derived,
                            ));
                        } else {
                            verified_count += 1;
                        }
                    }
                    #[cfg(not(feature = "z3"))]
                    {
                        failed_contracts.push((
                            format!("{:?}", contract.contract_type),
                            "Z3 required but not built. Build blvm-spec-lock with --features z3."
                                .to_string(),
                            contract.is_spec_derived,
                        ));
                    }
                }
            }
        } else {
            requires_z3_count += 1;
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Cannot verify: contract condition could not be parsed as expression".to_string(),
                contract.is_spec_derived,
            ));
        }
    }

    // Early return if requires contracts failed.
    if let Some(result) =
        demote_if_all_spec_derived(&failed_contracts, verified_count, function.contracts.len())
    {
        return result;
    }
    if !failed_contracts.is_empty() {
        let (contract_type, reason, _) = &failed_contracts[0];
        return failed_verification(contract_type, reason, failed_contracts.len());
    }

    // Now verify ensures contracts with the requires as context
    // We prove: requires && implementation => ensures
    for contract in &ensures_contracts {
        if contract.condition.trim().is_empty() {
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Empty contract condition".to_string(),
                contract.is_spec_derived,
            ));
            continue;
        }

        if crate::parser::condition::is_result_equality(&contract.condition) {
            #[cfg(feature = "z3")]
            {
                if let Some(ref func) = function.function_sig {
                    requires_z3_count += 1;
                    match verify_determinism(func, &requires_contracts, timeout_secs) {
                        Ok(()) => verified_count += 1,
                        Err(e) => {
                            if e.contains("Could not translate body") {
                                requires_z3_count += 1;
                                let mut slot = translation_error.borrow_mut();
                                if slot.is_none() {
                                    *slot = Some(e);
                                }
                            } else {
                                failed_contracts.push((
                                    format!("{:?}", contract.contract_type),
                                    format!("Determinism: {e}"),
                                    contract.is_spec_derived,
                                ));
                            }
                        }
                    }
                } else {
                    requires_z3_count += 1;
                    failed_contracts.push((
                        format!("{:?}", contract.contract_type),
                        "Determinism requires function signature".to_string(),
                        contract.is_spec_derived,
                    ));
                }
            }
            #[cfg(not(feature = "z3"))]
            {
                failed_contracts.push((
                    format!("{:?}", contract.contract_type),
                    "Z3 required for determinism verification. Build blvm-spec-lock with --features z3.".to_string(),
                    contract.is_spec_derived,
                ));
            }
            continue;
        }

        if let Some(ref expr) = contract.expr {
            match check_contract_statically(expr, contract.contract_type) {
                StaticCheck::Passed => {
                    verified_count += 1;
                }
                StaticCheck::Failed(reason) => {
                    failed_contracts.push((
                        format!("{:?}", contract.contract_type),
                        reason,
                        contract.is_spec_derived,
                    ));
                }
                StaticCheck::RequiresZ3 => {
                    requires_z3_count += 1;
                    #[cfg(feature = "z3")]
                    {
                        if let Err(e) = verify_with_z3(
                            contract,
                            function.function_sig.as_ref(),
                            &requires_contracts,
                            timeout_secs,
                        ) {
                            failed_contracts.push((
                                format!("{:?}", contract.contract_type),
                                format!("Z3: {e}"),
                                contract.is_spec_derived,
                            ));
                        } else {
                            verified_count += 1;
                        }
                    }
                    #[cfg(not(feature = "z3"))]
                    {
                        failed_contracts.push((
                            format!("{:?}", contract.contract_type),
                            "Z3 required but not built. Build blvm-spec-lock with --features z3."
                                .to_string(),
                            contract.is_spec_derived,
                        ));
                    }
                }
            }
        } else {
            requires_z3_count += 1;
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Cannot verify: contract condition could not be parsed".to_string(),
                contract.is_spec_derived,
            ));
        }
    }

    // Report results.
    if let Some(result) =
        demote_if_all_spec_derived(&failed_contracts, verified_count, function.contracts.len())
    {
        return result;
    }
    if !failed_contracts.is_empty() {
        let (contract_type, reason, _) = &failed_contracts[0];
        return failed_verification(contract_type, reason, failed_contracts.len());
    }

    if verified_count == function.contracts.len() {
        VerificationResult::Passed
    } else if requires_z3_count > 0 {
        let trans_err = translation_error.into_inner();
        let reason_msg = format!(
            "Z3 verification required but unavailable or incomplete ({} of {} verified). {}",
            verified_count,
            function.contracts.len(),
            trans_err
                .as_deref()
                .unwrap_or("Build with --features z3 for full verification.")
        );
        #[cfg(not(feature = "z3"))]
        let partial_reason = PartialReason::MissingZ3Build;
        #[cfg(feature = "z3")]
        let partial_reason = if trans_err.is_some() {
            PartialReason::UnsupportedTranslation
        } else {
            PartialReason::IncompleteCoverage
        };
        VerificationResult::Partial {
            verified: verified_count,
            total: function.contracts.len(),
            reason: Some(reason_msg),
            partial_reason: Some(partial_reason),
        }
    } else {
        VerificationResult::Passed
    }
}

/// Result of static checking
enum StaticCheck {
    Passed,
    Failed(String),
    RequiresZ3,
}

/// Check a contract statically (simplified version for CLI)
fn check_contract_statically(expr: &syn::Expr, _contract_type: ContractType) -> StaticCheck {
    // Simple pattern matching for common cases
    match expr {
        // Non-negative checks: x >= 0 or 0 <= x
        syn::Expr::Binary(bin) if matches!(bin.op, syn::BinOp::Ge(_)) => {
            if is_zero_literal(&bin.right) || is_zero_literal(&bin.left) {
                // x >= 0 or 0 <= x - this is a valid check pattern
                // Can't verify statically without type info, but syntax is valid
                return StaticCheck::RequiresZ3;
            }
        }
        // Equality checks: x == CONSTANT
        syn::Expr::Binary(bin) if matches!(bin.op, syn::BinOp::Eq(_)) => {
            if is_literal(&bin.left) || is_literal(&bin.right) {
                // Constant equality - requires Z3 for actual verification
                return StaticCheck::RequiresZ3;
            }
        }
        // Comparison checks: x < y, x > y, etc.
        syn::Expr::Binary(bin)
            if matches!(
                bin.op,
                syn::BinOp::Lt(_) | syn::BinOp::Le(_) | syn::BinOp::Gt(_) | syn::BinOp::Ge(_)
            ) =>
        {
            // Comparison - requires Z3
            return StaticCheck::RequiresZ3;
        }
        // Boolean operations: x && y, x || y
        syn::Expr::Binary(bin) if matches!(bin.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) => {
            // Boolean logic - requires Z3
            return StaticCheck::RequiresZ3;
        }
        _ => {
            // Unknown pattern - requires Z3
            return StaticCheck::RequiresZ3;
        }
    }

    StaticCheck::RequiresZ3
}

/// Check if expression is a zero literal
fn is_zero_literal(expr: &syn::Expr) -> bool {
    if let syn::Expr::Lit(lit) = expr {
        if let syn::Lit::Int(int_lit) = &lit.lit {
            return int_lit.base10_digits() == "0";
        }
    }
    false
}

/// Check if expression is a literal
fn is_literal(expr: &syn::Expr) -> bool {
    matches!(expr, syn::Expr::Lit(_))
}

/// Verify determinism (two-run Z3: a==b => f(a)==f(b))
#[cfg(feature = "z3")]
fn verify_determinism(
    func: &syn::ItemFn,
    requires_contracts: &[&Contract],
    timeout_secs: u64,
) -> Result<(), String> {
    use crate::parser::contracts::{
        Contract as LibraryContract, ContractType as LibraryContractType,
    };
    use crate::translator::z3_verifier::{VerificationResult, Z3Verifier};

    let timeout_ms = if timeout_secs > 0 {
        timeout_secs * 1000
    } else {
        10000
    };
    let mut verifier = Z3Verifier::new(timeout_ms);

    let requires_library: Vec<_> = requires_contracts
        .iter()
        .filter_map(|c| {
            c.expr.as_ref().map(|expr| LibraryContract {
                contract_type: LibraryContractType::Requires,
                condition: expr.clone(),
                comment: None,
            })
        })
        .collect();

    match verifier.verify_determinism(func, &requires_library) {
        VerificationResult::Verified => Ok(()),
        VerificationResult::Failed { counterexample } => {
            let msg = if let Some(ce) = counterexample {
                format!("Non-deterministic. Counterexample: {:?}", ce.assignments)
            } else {
                "Non-deterministic (counterexample not extracted)".to_string()
            };
            Err(msg)
        }
        VerificationResult::Unknown { reason } => Err(format!("Z3 unknown: {reason}")),
        VerificationResult::Error { error } => Err(format!("Z3 error: {error}")),
    }
}

/// Verify contract with Z3 (if feature enabled)
#[cfg(feature = "z3")]
fn verify_with_z3(
    contract: &Contract,
    func_sig: Option<&syn::ItemFn>,
    requires_contracts: &[&Contract],
    timeout_secs: u64,
) -> Result<(), String> {
    use crate::parser::contracts::{
        Contract as LibraryContract, ContractType as LibraryContractType,
    };
    use crate::translator::z3_verifier::{VerificationResult, Z3Verifier};

    // Convert CLI Contract to library Contract
    let expr = contract
        .expr
        .as_ref()
        .ok_or_else(|| "Cannot verify: missing parsed expression".to_string())?;

    let library_contract = LibraryContract {
        contract_type: match contract.contract_type {
            ContractType::Requires => LibraryContractType::Requires,
            ContractType::Ensures => LibraryContractType::Ensures,
        },
        condition: expr.clone(),
        comment: None,
    };

    // Use Z3 verifier with function signature and requires contracts for context
    let timeout_ms = if timeout_secs > 0 {
        timeout_secs * 1000
    } else {
        5000
    };
    let mut verifier = Z3Verifier::new(timeout_ms);

    // Convert requires contracts to library format
    let requires_library: Vec<_> = requires_contracts
        .iter()
        .filter_map(|c| {
            c.expr.as_ref().map(|expr| LibraryContract {
                contract_type: LibraryContractType::Requires,
                condition: expr.clone(),
                comment: None,
            })
        })
        .collect();

    match verifier.verify_contract_with_context(&library_contract, func_sig, &requires_library) {
        VerificationResult::Verified => Ok(()),
        VerificationResult::Failed { counterexample } => {
            let msg = if let Some(ce) = counterexample {
                format!("Contract violated. Counterexample: {:?}", ce.assignments)
            } else {
                "Contract violated (no counterexample available)".to_string()
            };
            Err(msg)
        }
        VerificationResult::Unknown { reason } => Err(format!("Z3 verification unknown: {reason}")),
        VerificationResult::Error { error } => Err(format!("Z3 verification error: {error}")),
    }
}

#[cfg(not(feature = "z3"))]
fn verify_with_z3(
    _contract: &Contract,
    _func_sig: Option<&syn::ItemFn>,
    _requires: &[&Contract],
    _timeout_secs: u64,
) -> Result<(), String> {
    Err("Z3 feature not enabled. Build with --features z3 to enable Z3 verification.".to_string())
}

/// High-level classification for failed verification (JSON **`detail.failure_kind`**).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Z3 found a satisfying assignment refuting the obligation (or static counterexample path).
    Counterexample,
    /// Contract text did not parse to a Rust expression / unsupported surface syntax.
    ParseError,
    /// Z3 returned **unknown** (including typical timeout wording from the solver wrapper).
    SolverUnknown,
    /// Z3 or translator returned an explicit error (not unknown/sat).
    SolverError,
    /// Build or feature issue (e.g. Z3 feature disabled).
    Tooling,
    /// Everything else (static messages, mixed failures).
    Other,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::Counterexample => "counterexample",
            FailureKind::ParseError => "parse_error",
            FailureKind::SolverUnknown => "solver_unknown",
            FailureKind::SolverError => "solver_error",
            FailureKind::Tooling => "tooling",
            FailureKind::Other => "other",
        }
    }
}

/// Why a function is **partial** (JSON **`detail.partial_reason`** when set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialReason {
    /// Wrapper reports Z3 unknown (may include timeout phrasing).
    Z3Unknown,
    /// Z3 **unknown** with timeout-style wording (**`failure_kind`** is still **`solver_unknown`**).
    Z3Timeout,
    /// Body or contract could not be translated to the solver (e.g. unimplemented MIR path).
    UnsupportedTranslation,
    /// Built without `z3` feature but obligations need the solver.
    MissingZ3Build,
    /// Solver available but not all contracts completed (budget / incomplete path).
    IncompleteCoverage,
    /// Z3 counterexample or solver error on an auto-enriched (spec-derived) contract.
    /// Does not block CI exit 0 — LaTeX→Z3 translation is best-effort; manual contracts
    /// with counterexamples still produce **Failed**.
    SpecDerivedCounterexample,
    Other,
}

impl PartialReason {
    pub fn as_str(self) -> &'static str {
        match self {
            PartialReason::Z3Unknown => "z3_unknown",
            PartialReason::Z3Timeout => "z3_timeout",
            PartialReason::UnsupportedTranslation => "unsupported_translation",
            PartialReason::MissingZ3Build => "missing_z3_build",
            PartialReason::IncompleteCoverage => "incomplete_coverage",
            PartialReason::SpecDerivedCounterexample => "spec_derived_counterexample",
            PartialReason::Other => "other",
        }
    }
}

fn solver_unknown_partial_reason(reason: &str) -> PartialReason {
    let r = reason.to_lowercase();
    if r.contains("timeout") || r.contains("timed out") || r.contains("time out") {
        PartialReason::Z3Timeout
    } else {
        PartialReason::Z3Unknown
    }
}

/// When every failure is on a spec-derived (auto-enriched) contract **and** the failure
/// is a LaTeX→Z3 translation gap (could not be parsed), demote to **Partial** so CI is
/// not blocked by approximation limits in the spec parser.
///
/// Z3 counterexamples on spec-derived contracts are **not** demoted — they indicate that
/// the implementation contradicts the enriched contract, which is a real signal regardless
/// of whether the contract was auto-generated. Those remain **Failed** (returns `None`).
fn demote_if_all_spec_derived(
    failed_contracts: &[(String, String, bool)],
    verified_count: usize,
    total: usize,
) -> Option<VerificationResult> {
    if failed_contracts.is_empty() {
        return None;
    }
    let all_spec_derived = failed_contracts.iter().all(|(_, _, sd)| *sd);
    if !all_spec_derived {
        return None;
    }
    // Only demote when every failure is a parse/translation gap.  A Z3 counterexample
    // means Z3 successfully evaluated the contract and found a violation — that is a
    // real finding even on an auto-enriched contract.
    let all_translation_gaps = failed_contracts
        .iter()
        .all(|(_, reason, _)| reason.contains("could not be parsed"));
    if !all_translation_gaps {
        return None;
    }
    Some(VerificationResult::Partial {
        verified: verified_count,
        total,
        reason: Some(failed_contracts[0].1.clone()),
        partial_reason: Some(PartialReason::UnsupportedTranslation),
    })
}

/// Build **`VerificationResult::Failed`** with optional **`partial_reason`** for **`solver_unknown`** rows.
pub(crate) fn failed_verification(
    contract_type: &str,
    primary_reason: &str,
    failed_contracts_len: usize,
) -> VerificationResult {
    let reason = format!("{primary_reason} ({failed_contracts_len} total failures)");
    let kind = failure_kind(contract_type, &reason);
    let partial_reason = if kind == FailureKind::SolverUnknown {
        Some(solver_unknown_partial_reason(&reason))
    } else {
        None
    };
    VerificationResult::Failed {
        contract: contract_type.to_string(),
        reason,
        kind,
        partial_reason,
    }
}

fn failure_kind(contract_type: &str, reason: &str) -> FailureKind {
    let r = reason.to_lowercase();
    let determinism_z3_unknown = r.contains("determinism")
        && (r.contains("z3 unknown") || r.contains("z3 verification unknown"));
    if r.contains("counterexample")
        || r.contains("contract violated")
        || r.contains("non-deterministic")
        || ((r.contains("determinism") && !r.contains("could not translate"))
            && !determinism_z3_unknown)
    {
        return FailureKind::Counterexample;
    }
    if r.contains("could not be parsed")
        || r.contains("could not parse")
        || r.contains("cannot verify: contract condition could not be parsed")
        || reason.contains("Cannot verify: contract condition could not be parsed as expression")
    {
        return FailureKind::ParseError;
    }
    if r.contains("z3 unknown") || r.contains("z3 verification unknown") {
        return FailureKind::SolverUnknown;
    }
    if r.contains("z3 error") || r.contains("z3 verification error") {
        return FailureKind::SolverError;
    }
    if r.contains("z3 required but not built")
        || r.contains("build blvm-spec-lock with --features z3")
        || r.contains("z3 feature not enabled")
    {
        return FailureKind::Tooling;
    }
    if contract_type == "Z3" && r.contains("unknown") {
        return FailureKind::SolverUnknown;
    }
    FailureKind::Other
}

/// Result of function verification
#[derive(Debug, Clone)]
pub enum VerificationResult {
    Passed,
    Failed {
        contract: String,
        reason: String,
        kind: FailureKind,
        /// When **`kind`** is **`solver_unknown`**: **`z3_unknown`** vs **`z3_timeout`** heuristic from message text (**JSON **`detail.partial_reason`**).
        partial_reason: Option<PartialReason>,
    },
    Partial {
        verified: usize,
        total: usize,
        reason: Option<String>,
        /// When **`Some`**, included in machine JSON under **`detail.partial_reason`**.
        partial_reason: Option<PartialReason>,
    },
    /// No contracts from spec or code - cannot verify (add to Orange Paper or #[requires]/#[ensures])
    NoContracts {
        section: String,
    },
    NotImplemented,
}

#[cfg(test)]
mod spec_locked_anchor_extract_tests {
    use super::{extract_constant_anchor, extract_formula_anchor, extract_section};
    use syn::{parse_quote, ItemFn};

    #[test]
    fn dual_literal_extracts_formula_and_section() {
        let f: ItemFn = parse_quote! {
            #[spec_locked("9.91", "F_X")]
            pub fn dummy() {}
        };
        assert_eq!(extract_formula_anchor(&f.attrs).as_deref(), Some("F_X"));
        assert_eq!(extract_constant_anchor(&f.attrs), None);
        assert_eq!(extract_section(&f.attrs).as_deref(), Some("9.91"));
    }

    #[test]
    fn dual_literal_extracts_constant_and_section() {
        let f: ItemFn = parse_quote! {
            #[spec_locked("4.91", "C_SMK")]
            pub fn dummy() {}
        };
        assert_eq!(extract_formula_anchor(&f.attrs), None);
        assert_eq!(extract_constant_anchor(&f.attrs).as_deref(), Some("C_SMK"));
        assert_eq!(extract_section(&f.attrs).as_deref(), Some("4.91"));
    }

    #[test]
    fn combined_literal_section_and_formula() {
        let f: ItemFn = parse_quote! {
            #[spec_locked("44.44::F_C")]
            pub fn dummy() {}
        };
        assert_eq!(extract_formula_anchor(&f.attrs).as_deref(), Some("F_C"));
        assert_eq!(extract_constant_anchor(&f.attrs), None);
        assert_eq!(extract_section(&f.attrs).as_deref(), Some("44.44"));
    }

    #[test]
    fn combined_literal_section_and_constant() {
        let f: ItemFn = parse_quote! {
            #[spec_locked("44.44::C_K")]
            pub fn dummy() {}
        };
        assert_eq!(extract_formula_anchor(&f.attrs), None);
        assert_eq!(extract_constant_anchor(&f.attrs).as_deref(), Some("C_K"));
        assert_eq!(extract_section(&f.attrs).as_deref(), Some("44.44"));
    }

    #[test]
    fn named_function_param_extracts_formula() {
        let f: ItemFn = parse_quote! {
            #[spec_locked(section = "8.82", function = "F_Z")]
            pub fn z() {}
        };
        assert_eq!(extract_formula_anchor(&f.attrs).as_deref(), Some("F_Z"));
        assert_eq!(extract_constant_anchor(&f.attrs), None);
        assert_eq!(extract_section(&f.attrs).as_deref(), Some("8.82"));
    }

    #[test]
    fn named_function_param_extracts_constant() {
        let f: ItemFn = parse_quote! {
            #[spec_locked(section = "4.82", function = "C_Z")]
            pub fn z() {}
        };
        assert_eq!(extract_formula_anchor(&f.attrs), None);
        assert_eq!(extract_constant_anchor(&f.attrs).as_deref(), Some("C_Z"));
        assert_eq!(extract_section(&f.attrs).as_deref(), Some("4.82"));
    }
}

#[cfg(test)]
mod failure_kind_tests {
    use super::{
        demote_if_all_spec_derived, failed_verification, failure_kind, FailureKind, PartialReason,
        VerificationResult,
    };

    #[test]
    fn determinism_z3_unknown_is_solver_unknown_not_counterexample() {
        let msg = "Determinism: Z3 unknown: Try --timeout 30. (1 total failures)";
        assert_eq!(failure_kind("Ensures", msg), FailureKind::SolverUnknown);

        match failed_verification("Ensures", "Determinism: Z3 unknown: Try --timeout 30.", 1) {
            VerificationResult::Failed {
                kind,
                partial_reason,
                ..
            } => {
                assert_eq!(kind, FailureKind::SolverUnknown);
                assert_eq!(partial_reason, Some(PartialReason::Z3Timeout));
            }
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn determinism_non_deterministic_stays_counterexample() {
        let msg = "Determinism: Non-deterministic. Counterexample: [] (1 total failures)";
        assert_eq!(failure_kind("Ensures", msg), FailureKind::Counterexample);
    }

    #[test]
    fn spec_derived_counterexample_stays_failed() {
        // A Z3 counterexample on a spec-derived contract is a real finding — it must NOT
        // be demoted to Partial.  demote_if_all_spec_derived returns None so the caller
        // falls through to failed_verification.
        let failed = vec![(
            "Ensures".to_string(),
            "Z3: Contract violated. Counterexample: {}".to_string(),
            true,
        )];
        assert!(
            demote_if_all_spec_derived(&failed, 0, 1).is_none(),
            "spec-derived Z3 counterexample must not be demoted to Partial"
        );
    }

    #[test]
    fn spec_derived_translation_gap_demotes_to_partial() {
        // A parse/translation gap (not a Z3 result) should still demote to Partial.
        let failed = vec![(
            "Ensures".to_string(),
            "Contract could not be parsed: unsupported LaTeX expression".to_string(),
            true,
        )];
        let result =
            demote_if_all_spec_derived(&failed, 0, 1).expect("translation gap should demote");
        match result {
            VerificationResult::Partial { partial_reason, .. } => {
                assert_eq!(partial_reason, Some(PartialReason::UnsupportedTranslation));
            }
            _ => panic!("expected Partial"),
        }
    }

    #[test]
    fn manual_counterexample_stays_failed() {
        let failed = vec![(
            "Ensures".to_string(),
            "Z3: Contract violated.".to_string(),
            false,
        )];
        assert!(demote_if_all_spec_derived(&failed, 0, 1).is_none());
    }
}
