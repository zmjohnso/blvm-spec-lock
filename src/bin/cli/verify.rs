//! Verification orchestration
//!
//! Discovers functions, extracts contracts, and runs verification

use std::path::PathBuf;
use syn::{Attribute, File, ImplItem, ImplItemFn, ItemFn};
use walkdir::WalkDir;

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
                    expr: Some(expr), // Store parsed expression for static checker
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
    pub function_sig: Option<syn::ItemFn>, // Store function signature for type inference
}

/// Discover all functions with #[spec_locked] attributes
pub fn discover_functions(workspace_root: &PathBuf) -> Result<Vec<FunctionToVerify>, String> {
    let mut functions = Vec::new();
    let mut errors = Vec::new();

    // Walk through Rust source files
    for entry in WalkDir::new(workspace_root)
        .into_iter()
        .filter_entry(|e| {
            let path = e.path();
            // Skip target directory and other build artifacts
            !path.to_string_lossy().contains("/target/")
                && !path.to_string_lossy().contains("/.git/")
                && !path.to_string_lossy().contains("/.cargo/")
        })
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

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
            syn::Item::Fn(func) => {
                if has_spec_locked(&func.attrs) {
                    functions.push(make_function_to_verify(
                        file_path,
                        &func.sig.ident.to_string(),
                        &func.attrs,
                        &func,
                    ));
                }
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

/// Extract Orange Paper section from #[spec_locked] attribute (including inside #[cfg_attr(...)])
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
                        let section = &after_spec[start + 1..start + 1 + end];
                        if section.chars().any(|c| c.is_ascii_digit()) {
                            return Some(section.to_string());
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
    let mut failed_contracts = Vec::new();
    let mut requires_z3_count = 0;
    let mut translation_error: Option<String> = None;

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
                    failed_contracts.push((format!("{:?}", contract.contract_type), reason));
                }
                StaticCheck::RequiresZ3 => {
                    requires_z3_count += 1;
                    // Try Z3 if available
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
                                format!("Z3 verification failed: {}", e),
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
                        ));
                    }
                }
            }
        } else {
            // No parsed expression - can't do static check
            // Mark as requiring Z3 or manual verification
            requires_z3_count += 1;
            // Don't count as verified - we can't verify without a parsed expression
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Cannot verify: contract condition could not be parsed as expression".to_string(),
            ));
        }
    }

    // Early return if requires contracts failed
    if !failed_contracts.is_empty() {
        let (contract_type, reason) = &failed_contracts[0];
        return VerificationResult::Failed {
            contract: contract_type.clone(),
            reason: format!("{} ({} total failures)", reason, failed_contracts.len()),
        };
    }

    // Now verify ensures contracts with the requires as context
    // This is the KEY to Orange Paper verification:
    // We prove: requires && implementation => ensures
    for contract in &ensures_contracts {
        if contract.condition.trim().is_empty() {
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Empty contract condition".to_string(),
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
                                if translation_error.is_none() {
                                    translation_error = Some(e);
                                }
                            } else {
                                failed_contracts.push((
                                    format!("{:?}", contract.contract_type),
                                    format!("Determinism: {}", e),
                                ));
                            }
                        }
                    }
                } else {
                    requires_z3_count += 1;
                    failed_contracts.push((
                        format!("{:?}", contract.contract_type),
                        "Determinism requires function signature".to_string(),
                    ));
                }
            }
            #[cfg(not(feature = "z3"))]
            {
                failed_contracts.push((
                    format!("{:?}", contract.contract_type),
                    "Z3 required for determinism verification. Build blvm-spec-lock with --features z3.".to_string(),
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
                    failed_contracts.push((format!("{:?}", contract.contract_type), reason));
                }
                StaticCheck::RequiresZ3 => {
                    requires_z3_count += 1;
                    #[cfg(feature = "z3")]
                    {
                        // For ensures, pass the requires contracts as context
                        // This allows verifier to prove: requires && impl => ensures
                        if let Err(e) = verify_with_z3(
                            contract,
                            function.function_sig.as_ref(),
                            &requires_contracts,
                            timeout_secs,
                        ) {
                            failed_contracts.push((
                                format!("{:?}", contract.contract_type),
                                format!("Z3: {}", e),
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
                        ));
                    }
                }
            }
        } else {
            requires_z3_count += 1;
            failed_contracts.push((
                format!("{:?}", contract.contract_type),
                "Cannot verify: contract condition could not be parsed".to_string(),
            ));
        }
    }

    // Report results
    if !failed_contracts.is_empty() {
        let (contract_type, reason) = &failed_contracts[0];
        return VerificationResult::Failed {
            contract: contract_type.clone(),
            reason: format!("{} ({} total failures)", reason, failed_contracts.len()),
        };
    }

    if verified_count == function.contracts.len() {
        VerificationResult::Passed
    } else if requires_z3_count > 0 {
        VerificationResult::Failed {
            contract: "Z3".to_string(),
            reason: format!(
                "Z3 verification required but unavailable or incomplete ({} of {} verified). {}",
                verified_count,
                function.contracts.len(),
                translation_error
                    .as_deref()
                    .unwrap_or("Build with --features z3 for full verification.")
            ),
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
        VerificationResult::Unknown { reason } => Err(format!("Z3 unknown: {}", reason)),
        VerificationResult::Error { error } => Err(format!("Z3 error: {}", error)),
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
        VerificationResult::Unknown { reason } => {
            Err(format!("Z3 verification unknown: {}", reason))
        }
        VerificationResult::Error { error } => Err(format!("Z3 verification error: {}", error)),
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

/// Result of function verification
#[derive(Debug, Clone)]
pub enum VerificationResult {
    Passed,
    Failed {
        contract: String,
        reason: String,
    },
    Partial {
        verified: usize,
        total: usize,
        reason: Option<String>,
    },
    /// No contracts from spec or code - cannot verify (add to Orange Paper or #[requires]/#[ensures])
    NoContracts {
        section: String,
    },
    NotImplemented,
}
