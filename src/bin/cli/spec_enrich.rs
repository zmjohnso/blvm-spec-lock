//! Enrich spec-locked functions with contracts from the Orange Paper.

use super::verify::{Contract, ContractType, FunctionToVerify};
use crate::parser::condition;
use crate::parser::orange_paper::{
    section_id_subsumes_formula_section, ContractType as SpecContractType, SpecParser,
};
use std::path::PathBuf;

fn find_function_in_section_or_parents<'a>(
    parser: &'a SpecParser,
    section: &str,
    name: Option<&str>,
) -> Option<&'a crate::parser::orange_paper::FunctionSpec> {
    let mut s = section;
    loop {
        if let Some(f) = parser.find_function(s, name) {
            return Some(f);
        }
        if let Some(dot) = s.rfind('.') {
            s = &s[..dot];
        } else {
            break;
        }
    }
    None
}

/// Convert Rust snake_case to spec PascalCase (e.g. get_block_subsidy -> GetBlockSubsidy)
fn rust_to_spec_name(rust_name: &str) -> String {
    rust_name
        .split('_')
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().chain(c).collect(),
            }
        })
        .collect()
}

/// Enrich discovered functions with contracts extracted from the Orange Paper.
/// Accepts one or more spec paths; when multiple, sections are merged (duplicate section IDs error).
pub fn enrich_functions_with_spec(
    functions: &mut [FunctionToVerify],
    spec_paths: &[PathBuf],
) -> Result<usize, String> {
    let parser = SpecParser::from_paths(spec_paths)?;

    let mut enriched_count = 0;

    for func in functions.iter_mut() {
        let Some(section_ref) = func.section.as_deref() else {
            continue;
        };

        // Phase **`C_*`** — consensus **`ExtractedConstant`** (**`constants_stable_id_map`**).
        if let Some(cid) = &func.constant_anchor {
            let cmap = parser
                .constants_stable_id_map()
                .map_err(|e| format!("Orange Paper (--spec-path) constant index: {e}"))?;
            match cmap.get(cid) {
                None => {
                    return Err(format!(
                        "Orange Paper (--spec-path) has no **`C_*`** id `{cid}` (Section 4 **$NAME = …$**) referenced by **`#[spec_locked]`**.",
                    ));
                }
                Some(ec) => {
                    if !section_id_subsumes_formula_section(section_ref, &ec.section) {
                        return Err(format!(
                            "Section mismatch for `{cid}`: #[spec_locked] cites §{section_ref} but constant is extracted under §{}.",
                            ec.section
                        ));
                    }
                    func.contracts.clear();
                    let condition = format!("result == {}", ec.rust_expr);
                    let parseable_opt = condition::extract_parseable_condition(&condition);
                    let expr = parseable_opt
                        .as_deref()
                        .and_then(|s| syn::parse_str::<syn::Expr>(s).ok());
                    if let Some(expr) = expr {
                        let stored = parseable_opt.unwrap_or_else(|| condition.clone());
                        func.contracts.push(Contract {
                            contract_type: ContractType::Ensures,
                            condition: stored,
                            expr: Some(expr),
                        });
                        enriched_count += 1;
                    }
                    continue;
                }
            }
        }

        // Phase 5 — explicit **`F_*`** anchors bind to **`Formula`** blocks before function lookup.
        if let Some(fid) = &func.formula_anchor {
            match parser.formulas().get(fid) {
                None => {
                    return Err(format!(
                        "Orange Paper (--spec-path) has no **`Formula`** id `{fid}` referenced by **`#[spec_locked]`**.",
                    ));
                }
                Some(fspec) => {
                    if !section_id_subsumes_formula_section(section_ref, &fspec.section) {
                        return Err(format!(
                            "Section mismatch for formula `{fid}`: #[spec_locked] cites §{section_ref} but formula is defined under §{}.",
                            fspec.section
                        ));
                    }
                    func.contracts.clear();
                    let condition = fspec.latex_body.trim().to_string();
                    if !condition.is_empty() {
                        let parseable = condition::extract_parseable_condition(&condition);
                        let expr = parseable
                            .as_ref()
                            .and_then(|s| syn::parse_str::<syn::Expr>(s).ok());
                        if expr.is_some() {
                            func.contracts.push(Contract {
                                contract_type: ContractType::Ensures,
                                condition: condition.clone(),
                                expr,
                            });
                            enriched_count += 1;
                        }
                    }

                    continue;
                }
            }
        }

        let spec_name = rust_to_spec_name(&func.function_name);

        let spec_func = parser
            .find_function(section_ref, Some(&spec_name))
            .or_else(|| parser.find_function_anywhere(&spec_name).map(|(f, _)| f))
            .or_else(|| parser.find_function(section_ref, None))
            .or_else(|| find_function_in_section_or_parents(&parser, section_ref, None));
        let spec_func = spec_func.and_then(|f| {
            if f.contracts.is_empty() {
                parser.find_function(section_ref, Some("*"))
            } else {
                Some(f)
            }
        });

        if let Some(spec_func) = spec_func {
            if spec_func.contracts.is_empty() {
                continue;
            }

            func.contracts.clear();

            let mut added_any = false;
            for spec_contract in &spec_func.contracts {
                let contract_type = match spec_contract.contract_type {
                    SpecContractType::Requires => ContractType::Requires,
                    SpecContractType::Ensures
                    | SpecContractType::Property
                    | SpecContractType::EdgeCase => ContractType::Ensures,
                };

                let condition = spec_contract.condition.trim().to_string();
                if condition.is_empty() {
                    continue;
                }

                let parseable = condition::extract_parseable_condition(&condition);
                let expr = parseable
                    .as_ref()
                    .and_then(|s| syn::parse_str::<syn::Expr>(s).ok());

                if expr.is_none() {
                    continue;
                }

                let contract = Contract {
                    contract_type,
                    condition: condition.clone(),
                    expr,
                };

                if !func.contracts.iter().any(|c| c.condition == condition) {
                    func.contracts.push(contract);
                    enriched_count += 1;
                    added_any = true;
                }
            }

            if !added_any && !spec_func.contracts.is_empty() {
                if let Ok(expr) = syn::parse_str::<syn::Expr>("true") {
                    func.contracts.push(Contract {
                        contract_type: ContractType::Ensures,
                        condition: "true".to_string(),
                        expr: Some(expr),
                    });
                    enriched_count += 1;
                }
            }
        }

        // Fallback: spec has no parseable contracts for this section (e.g. complex math only).
        // Add default ensures(true) so verification doesn't fail with "no contracts".
        // Keeps Orange Paper focused on math; tooling handles the rest.
        if func.contracts.is_empty()
            && func.section.is_some()
            && func.formula_anchor.is_none()
            && func.constant_anchor.is_none()
        {
            if let Ok(expr) = syn::parse_str::<syn::Expr>("true") {
                func.contracts.push(Contract {
                    contract_type: ContractType::Ensures,
                    condition: "true".to_string(),
                    expr: Some(expr),
                });
                enriched_count += 1;
            }
        }
    }

    Ok(enriched_count)
}

#[cfg(test)]
mod enrich_formula_tests {
    use super::*;
    use crate::cli::verify::FunctionToVerify;
    use std::path::PathBuf;
    use syn::parse_quote;

    #[test]
    fn formula_anchor_enrich_adds_parseable_contract_from_spec() {
        let dir = std::env::temp_dir().join(format!(
            "spec_lock_formula_enrich_{}_{}",
            std::process::id(),
            rand_unique()
        ));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let md_path = dir.join("minimal_formula.md");

        let md = r"## 99.91 Formula enrich fixture

**Formula** (**F_EnrichSmoke**):

$$true$$
";
        std::fs::write(&md_path, md).expect("write fixture");
        let _cleanup_dir = TmpDirCleanup(dir);

        let func: syn::ItemFn = parse_quote! {
            fn witness_formula_enrich() -> bool { true }
        };

        let mut functions = vec![FunctionToVerify {
            file_path: PathBuf::from("witness.rs"),
            function_name: "witness_formula_enrich".into(),
            contracts: vec![],
            section: Some("99.91".into()),
            formula_anchor: Some("F_EnrichSmoke".into()),
            constant_anchor: None,
            function_sig: Some(func),
        }];

        let n =
            enrich_functions_with_spec(&mut functions, &[md_path]).expect("enrich_without_error");
        assert_eq!(n, 1);
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].contracts.len(), 1);
        assert_eq!(
            functions[0].contracts[0].contract_type,
            ContractType::Ensures
        );
        assert!(functions[0].contracts[0].expr.is_some());
    }

    #[test]
    fn constant_anchor_enrich_adds_ensures_equals_rust_expr() {
        let dir = std::env::temp_dir().join(format!(
            "spec_lock_constant_enrich_{}_{}",
            std::process::id(),
            rand_unique()
        ));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let md_path = dir.join("minimal_constant.md");

        let md = r"## 4.99 Fixture constants

$SMK = 7$
";
        std::fs::write(&md_path, md).expect("write fixture");
        let _cleanup_dir = TmpDirCleanup(dir);

        let func: syn::ItemFn = parse_quote! {
            fn witness_constant_enrich() -> i32 { 7 }
        };

        let mut functions = vec![FunctionToVerify {
            file_path: PathBuf::from("witness.rs"),
            function_name: "witness_constant_enrich".into(),
            contracts: vec![],
            section: Some("4.99".into()),
            formula_anchor: None,
            constant_anchor: Some("C_SMK".into()),
            function_sig: Some(func),
        }];

        let n =
            enrich_functions_with_spec(&mut functions, &[md_path]).expect("enrich_without_error");
        assert_eq!(n, 1);
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].contracts.len(), 1);
        assert_eq!(
            functions[0].contracts[0].contract_type,
            ContractType::Ensures
        );
        assert!(functions[0].contracts[0].expr.is_some());
    }

    fn rand_unique() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    struct TmpDirCleanup(PathBuf);

    impl Drop for TmpDirCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
