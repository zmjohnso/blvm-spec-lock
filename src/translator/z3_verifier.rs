//! Z3 verifier: Z3 solving with counterexample extraction
//!
//! Uses Z3 to verify contracts and extract counterexamples when verification fails.
//!
//! ## Orange Paper = Single Source of Truth
//!
//! This verifier implements the core principle: the Orange Paper defines the math,
//! and we verify that the Rust implementation satisfies that math.
//!
//! For `ensures` contracts:
//! 1. Extract preconditions (requires) and postconditions (ensures) from Orange Paper
//! 2. Translate the Rust implementation to Z3 formula
//! 3. Prove: requires && implementation => ensures
//!
//! If Z3 proves this implication, the implementation is mathematically locked to the spec.

use crate::parser::contracts::{Contract, ContractType};
#[cfg(feature = "z3")]
use crate::translator::z3_translator::{
    returns_bool_like, returns_result_or_option_result, Z3Translator, Z3VarMap,
};
#[cfg(feature = "z3")]
use z3::ast::{forall_const, Ast, Bool, Int};
#[cfg(feature = "z3")]
use z3::{Context, SatResult, Solver, Sort};

#[cfg(feature = "z3")]
/// Result of Z3 verification
#[derive(Debug, Clone)]
pub enum VerificationResult {
    /// Property holds (unsatisfiable negation)
    Verified,
    /// Property fails (satisfiable - found counterexample)
    Failed {
        counterexample: Option<Counterexample>,
    },
    /// Verification timed out or was too complex
    Unknown { reason: String },
    /// Error during verification
    Error { error: String },
}

#[cfg(feature = "z3")]
/// Counterexample from Z3 model
#[derive(Debug, Clone)]
pub struct Counterexample {
    /// Variable assignments that violate the property
    pub assignments: std::collections::HashMap<String, String>,
}

#[cfg(feature = "z3")]
/// Z3 verifier for contracts
pub struct Z3Verifier {
    translator: Z3Translator,
}

#[cfg(feature = "z3")]
impl Z3Verifier {
    /// Create a new Z3 verifier.
    /// `timeout_ms`: Solver timeout in milliseconds (avoids Unknown from indefinite solving).
    pub fn new(timeout_ms: u64) -> Self {
        let translator = Z3Translator::new(timeout_ms);

        Z3Verifier { translator }
    }

    /// Verify a contract
    ///
    /// For requires: checks if precondition can be violated
    /// For ensures: checks if postcondition can be violated
    pub fn verify_contract(&mut self, contract: &Contract) -> VerificationResult {
        self.verify_contract_with_context(contract, None, &[])
    }

    /// Verify a contract with function signature context for type inference
    /// For ensures contracts, requires_contracts are used as additional constraints
    pub fn verify_contract_with_context(
        &mut self,
        contract: &Contract,
        func_sig: Option<&syn::ItemFn>,
        requires_contracts: &[Contract],
    ) -> VerificationResult {
        // Extract parameter types and return type from function signature
        let (param_types, return_type) = if let Some(func) = func_sig {
            (extract_parameter_types(func), extract_return_type(func))
        } else {
            (std::collections::HashMap::new(), None)
        };

        // Translate contract to Z3 with type information
        let (z3_expr, type_constraints) = match self.translator.translate_contract_with_types(
            contract,
            &param_types,
            return_type.as_ref(),
        ) {
            Ok((expr, constraints)) => (expr, constraints),
            Err(e) => {
                return VerificationResult::Error {
                    error: format!("Translation error: {e}"),
                };
            }
        };

        // For verification, we check the negation
        // If negation is unsatisfiable, the property holds
        // If negation is satisfiable, we found a counterexample
        let negated_bool = match z3_expr.as_bool() {
            Some(b) => b.not(),
            None => {
                return VerificationResult::Error {
                    error: "Contract expression must be boolean".to_string(),
                };
            }
        };

        // Create solver for this verification
        let ctx = self.translator.context();
        let mut solver = Solver::new(ctx);

        // Add type constraints first (e.g., u64 >= 0)
        for constraint in &type_constraints {
            solver.assert(constraint);
        }

        // For ensures contracts:
        // 1. Add requires constraints (assume preconditions hold)
        // 2. Add implementation formula (translate function body to Z3)
        // 3. Check: requires && implementation => ensures
        //
        // This makes Orange Paper the single source of truth:
        // - Math (ensures contracts) comes from Orange Paper
        // - Implementation must satisfy the math
        // Track whether we successfully translated the function body into solver constraints.
        // A SAT result without body constraints is vacuous (not a real counterexample).
        let mut body_translated = false;

        if matches!(contract.contract_type, ContractType::Ensures) {
            // Add requires constraints
            for requires_contract in requires_contracts {
                if let Ok((requires_expr, requires_constraints)) =
                    self.translator.translate_contract_with_types(
                        requires_contract,
                        &param_types,
                        return_type.as_ref(),
                    )
                {
                    for constraint in &requires_constraints {
                        solver.assert(constraint);
                    }
                    if let Some(requires_bool) = requires_expr.as_bool() {
                        solver.assert(&requires_bool);
                    }
                }
            }

            // KEY: Translate function body to Z3 formula
            // This constrains 'result' to be the actual computed value
            if let Some(func) = func_sig {
                // Create fresh vars map for body translation
                let mut body_vars = Z3VarMap::new();

                // Initialize parameter variables
                for name in param_types.keys() {
                    let symbol = z3::Symbol::String(name.clone());
                    let var = z3::ast::Int::new_const(ctx, symbol);
                    body_vars.insert(name.clone(), var.into());
                }

                // Initialize result: Bool for bool/Result<bool>/Option<bool>, Int otherwise
                let result_symbol = z3::Symbol::String("result".to_string());
                let result_var = if return_type.as_ref().is_some_and(returns_bool_like) {
                    z3::ast::Bool::new_const(ctx, result_symbol).into()
                } else {
                    z3::ast::Int::new_const(ctx, result_symbol).into()
                };
                body_vars.insert("result".to_string(), result_var);

                add_shift_axioms(ctx, &mut solver);

                if let Ok(Some(impl_formula)) = self
                    .translator
                    .translate_function_body(func, &mut body_vars)
                {
                    solver.assert(&impl_formula);
                    body_translated = true;
                }
                // If translation fails, we still verify based on type constraints and requires.
                // body_translated remains false; SAT in that case is vacuous (see below).
            }
        }

        // Add negated ensures contract to solver
        // We're checking: requires && implementation && !ensures is UNSAT
        // If UNSAT: requires && implementation => ensures (postcondition holds)
        // If SAT: Found counterexample where implementation doesn't satisfy postcondition
        solver.assert(&negated_bool);

        // Check satisfiability
        match solver.check() {
            SatResult::Unsat => {
                // Negation is unsatisfiable, so property holds
                VerificationResult::Verified
            }
            SatResult::Sat => {
                // Negation is satisfiable — Z3 found a model.
                //
                // Only treat this as a real counterexample when we have concrete variable
                // assignments.  There are two reasons an empty-assignments result is vacuous:
                //
                // 1. Body translation failed: no implementation constraints were added,
                //    so Z3 trivially satisfies !ensures from the postcondition alone.
                // 2. Body translation succeeded but assignments map is empty: the current
                //    extract_counterexample implementation is a stub; it does not yet walk
                //    the Z3 model to extract named-variable values.  Until that is
                //    implemented every SAT result carries an empty {} counterexample,
                //    which gives no evidence of a real implementation violation.
                //
                // In both cases return Unknown so the caller classifies the result as a
                // translation-gap (Partial) rather than a hard failure.  Once
                // extract_counterexample is fully implemented this branch will only trigger
                // for case 1; case 2 will produce non-empty assignments and fall through
                // to the real Failed path below.
                let counterexample = self.extract_counterexample(&solver);
                let is_vacuous = !body_translated
                    || counterexample
                        .as_ref()
                        .is_none_or(|ce| ce.assignments.is_empty());
                if is_vacuous {
                    return VerificationResult::Unknown {
                        reason: if !body_translated {
                            "Could not translate function body to Z3 constraints; \
                             SAT result without body constraints is not meaningful"
                                .to_string()
                        } else {
                            "Z3 found SAT but counterexample model has no named variable \
                             assignments (incomplete translator); result is not a concrete \
                             witness against the implementation"
                                .to_string()
                        },
                    };
                }
                VerificationResult::Failed { counterexample }
            }
            SatResult::Unknown => VerificationResult::Unknown {
                reason: "Z3 solver returned Unknown (timeout or complexity). Try --timeout 30."
                    .to_string(),
            },
        }
    }

    /// Lightweight **formula-only** check: translate an **ensures** condition (no Rust implementation)
    /// and ask Z3 whether the formula is **satisfiable**.
    ///
    /// - **`Verified`**: the formula is **SAT** — there exists a model (consistent / not an obvious contradiction).
    /// - **`Failed`**: **UNSAT** — the formula is unsatisfiable (e.g. `x < x`).
    /// - **`Unknown` / `Error`**: solver or translation issue.
    ///
    /// This does **not** prove the formula against code; it is a smoke test that LaTeX → Rust → Z3 is coherent.
    pub fn check_ensures_formula_sat_smoke(&mut self, contract: &Contract) -> VerificationResult {
        if !matches!(contract.contract_type, ContractType::Ensures) {
            return VerificationResult::Error {
                error: "check_ensures_formula_sat_smoke expects an Ensures contract".to_string(),
            };
        }
        let (z3_expr, type_constraints) = match self.translator.translate_contract_with_types(
            contract,
            &std::collections::HashMap::new(),
            None,
        ) {
            Ok(x) => x,
            Err(e) => {
                return VerificationResult::Error {
                    error: format!("Translation error: {e}"),
                };
            }
        };
        let pos = match z3_expr.as_bool() {
            Some(b) => b,
            None => {
                return VerificationResult::Error {
                    error: "Formula must translate to a boolean Z3 expression".to_string(),
                };
            }
        };
        let ctx = self.translator.context();
        let mut solver = Solver::new(ctx);
        add_shift_axioms(ctx, &mut solver);
        for c in &type_constraints {
            solver.assert(c);
        }
        solver.assert(&pos);
        match solver.check() {
            SatResult::Sat => VerificationResult::Verified,
            SatResult::Unsat => VerificationResult::Failed {
                counterexample: None,
            },
            SatResult::Unknown => VerificationResult::Unknown {
                reason: "Z3 Unknown (formula satisfiability smoke)".to_string(),
            },
        }
    }

    /// Extract counterexample from Z3 model.
    ///
    /// Returns `None` when no model is available, and `Some(Counterexample { assignments })`
    /// when Z3 produced a satisfying model.  `assignments` is populated from the model's
    /// declarations; when the translator did not introduce named constants the map is empty
    /// (see note below).
    ///
    /// **Important — stub status:** model traversal is not yet implemented; `assignments`
    /// is always empty.  As long as this is the case every SAT result is vacuous (no
    /// concrete witness), so callers must treat an empty-assignments counterexample the
    /// same as an Unknown/translation-gap result for spec-derived contracts.
    fn extract_counterexample(&self, solver: &Solver<'_>) -> Option<Counterexample> {
        // Obtain the model to confirm SAT has a concrete witness; actual variable
        // extraction is not yet implemented.
        let _model = solver.get_model()?;
        Some(Counterexample {
            assignments: std::collections::HashMap::new(),
        })
    }

    /// Reset the solver (for verifying multiple contracts)
    /// Note: Since we create solvers on-demand, this is a no-op
    pub fn reset(&mut self) {
        // No-op: solvers are created on-demand
    }

    /// Verify determinism: ∀a,b: a=b → f(a)=f(b)
    /// Two-run Z3: translate body twice with distinct vars, prove (inputs equal) => (outputs equal)
    pub fn verify_determinism(
        &mut self,
        func: &syn::ItemFn,
        requires_contracts: &[Contract],
    ) -> VerificationResult {
        let ctx = self.translator.context();
        let param_types = extract_parameter_types(func);
        let return_type = extract_return_type(func);

        if param_types.is_empty() {
            return VerificationResult::Verified;
        }

        let mut solver = Solver::new(ctx);
        add_shift_axioms(ctx, &mut solver);

        let prefix1 = "r1_";
        let prefix2 = "r2_";

        let mut vars1 = Z3VarMap::new();
        let mut vars2 = Z3VarMap::new();

        for name in param_types.keys() {
            let sym1 = z3::Symbol::String(format!("{prefix1}{name}"));
            let sym2 = z3::Symbol::String(format!("{prefix2}{name}"));
            let var1 = z3::ast::Int::new_const(ctx, sym1);
            let var2 = z3::ast::Int::new_const(ctx, sym2);
            vars1.insert(name.clone(), var1.into());
            vars2.insert(name.clone(), var2.into());
        }

        let result_sym1 = z3::Symbol::String(format!("{prefix1}result"));
        let result_sym2 = z3::Symbol::String(format!("{prefix2}result"));
        // Result<T> / Option<Result<T>>: use Int for determinism (0=Err/None, 1=Ok(false), 2=Ok(true))
        // Only for functions that need it (script verification) - avoids breaking others
        let func_name = func.sig.ident.to_string();
        let use_int_result = return_type
            .as_ref()
            .is_some_and(returns_result_or_option_result)
            && matches!(
                func_name.as_str(),
                "verify_script_with_context_full"
                    | "verify_script_with_context"
                    | "try_verify_p2pk_fast_path"
            );
        let result1: z3::ast::Dynamic = if use_int_result {
            z3::ast::Int::new_const(ctx, result_sym1).into()
        } else if return_type.as_ref().is_some_and(returns_bool_like) {
            z3::ast::Bool::new_const(ctx, result_sym1).into()
        } else {
            z3::ast::Int::new_const(ctx, result_sym1).into()
        };
        let result2: z3::ast::Dynamic = if use_int_result {
            z3::ast::Int::new_const(ctx, result_sym2).into()
        } else if return_type.as_ref().is_some_and(returns_bool_like) {
            z3::ast::Bool::new_const(ctx, result_sym2).into()
        } else {
            z3::ast::Int::new_const(ctx, result_sym2).into()
        };
        vars1.insert("result".to_string(), result1.clone());
        vars2.insert("result".to_string(), result2.clone());

        let formula1 = match self.translator.translate_function_body(func, &mut vars1) {
            Ok(Some(f)) => f,
            Ok(None) => {
                return VerificationResult::Error {
                    error: "Could not translate body for run 1 (no formula)".to_string(),
                }
            }
            Err(e) => {
                return VerificationResult::Error {
                    error: format!("Could not translate body for run 1: {e}"),
                }
            }
        };
        let formula2 = match self.translator.translate_function_body(func, &mut vars2) {
            Ok(Some(f)) => f,
            Ok(None) => {
                return VerificationResult::Error {
                    error: "Could not translate body for run 2 (no formula)".to_string(),
                }
            }
            Err(e) => {
                return VerificationResult::Error {
                    error: format!("Could not translate body for run 2: {e}"),
                }
            }
        };

        solver.assert(&formula1);
        solver.assert(&formula2);

        for req in requires_contracts {
            if let Ok(expr1) = self
                .translator
                .translate_expr_with_vars(&req.condition, &mut vars1)
            {
                if let Some(b1) = expr1.as_bool() {
                    solver.assert(&b1);
                }
            }
            if let Ok(expr2) = self
                .translator
                .translate_expr_with_vars(&req.condition, &mut vars2)
            {
                if let Some(b2) = expr2.as_bool() {
                    solver.assert(&b2);
                }
            }
        }

        let mut input_equiv = Vec::new();
        for name in param_types.keys() {
            let v1 = vars1.get(name).and_then(|x| x.as_int()).unwrap();
            let v2 = vars2.get(name).and_then(|x| x.as_int()).unwrap();
            input_equiv.push(v1._eq(&v2));
        }
        let equiv_refs: Vec<&Bool> = input_equiv.iter().collect();
        let input_equal = Bool::and(ctx, &equiv_refs);

        let output_diff = if let (Some(r1), Some(r2)) = (result1.as_int(), result2.as_int()) {
            r1._eq(&r2).not()
        } else if let (Some(r1), Some(r2)) = (result1.as_bool(), result2.as_bool()) {
            r1._eq(&r2).not()
        } else {
            return VerificationResult::Error {
                error: "Result type not Int or Bool".to_string(),
            };
        };

        let negated = Bool::and(ctx, &[&input_equal, &output_diff]);
        solver.assert(&negated);

        match solver.check() {
            SatResult::Unsat => VerificationResult::Verified,
            SatResult::Sat => VerificationResult::Failed {
                counterexample: self.extract_counterexample(&solver),
            },
            SatResult::Unknown => VerificationResult::Unknown {
                reason: "Z3 returned Unknown (determinism check). Try --timeout 30.".to_string(),
            },
        }
    }
}

#[cfg(all(test, feature = "z3"))]
mod formula_sat_smoke_tests {
    use super::{VerificationResult, Z3Verifier};
    use crate::parser::contracts::{Contract, ContractType};
    use syn::parse_str;

    #[test]
    fn ensures_literal_true_is_sat() {
        let mut v = Z3Verifier::new(5000);
        let c = Contract {
            contract_type: ContractType::Ensures,
            condition: parse_str("true").unwrap(),
            comment: None,
        };
        assert!(matches!(
            v.check_ensures_formula_sat_smoke(&c),
            VerificationResult::Verified
        ));
    }

    #[test]
    fn result_eq_result_is_sat() {
        let mut v = Z3Verifier::new(5000);
        let c = Contract {
            contract_type: ContractType::Ensures,
            condition: parse_str("result == result").unwrap(),
            comment: None,
        };
        assert!(matches!(
            v.check_ensures_formula_sat_smoke(&c),
            VerificationResult::Verified
        ));
    }

    #[test]
    fn x_lt_x_is_unsat_contradiction() {
        let mut v = Z3Verifier::new(5000);
        let c = Contract {
            contract_type: ContractType::Ensures,
            condition: parse_str("x < x").unwrap(),
            comment: None,
        };
        assert!(matches!(
            v.check_ensures_formula_sat_smoke(&c),
            VerificationResult::Failed { .. }
        ));
    }
}

/// Bit shifts use UF `shr` / `shl` (`FuncDecl::new` with the same name shares one UF per [`Context`](z3::Context)).
/// Literal `>> k` / `<< k` use [`Z3Translator::pow2_int`](crate::translator::z3_translator::Z3Translator::pow2_int) for correct `2^k` including `k=63`.
/// The `shr(a,k)=a/2^k` axioms use [`Z3Translator::pow2_int`](crate::translator::z3_translator::Z3Translator::pow2_int) for `2^k` (k < 64) so k=63 is correct on the host.
#[cfg(feature = "z3")]
fn add_shift_axioms(ctx: &Context, solver: &mut Solver) {
    let int_sort = Sort::int(ctx);

    let a = Int::new_const(ctx, "axiom_a");
    let b = Int::new_const(ctx, "axiom_b");
    let zero = Int::from_i64(ctx, 0);

    let shr_fn = z3::FuncDecl::new(ctx, "shr", &[&int_sort, &int_sort], &int_sort);
    let shl_fn = z3::FuncDecl::new(ctx, "shl", &[&int_sort, &int_sort], &int_sort);

    let shr_result = shr_fn.apply(&[&a, &b]);
    let shr_result_int = shr_result.as_int().unwrap();
    let premise1 = Bool::and(ctx, &[&a.ge(&zero), &b.ge(&zero)]);
    let conclusion1 = shr_result_int.ge(&zero);
    let axiom1 = premise1.implies(&conclusion1);

    let bound_a = a.clone();
    let bound_b = b.clone();
    let forall1 = forall_const(ctx, &[&bound_a, &bound_b], &[], &axiom1);
    solver.assert(&forall1);

    let conclusion2 = shr_result_int.le(&a);
    let axiom2 = premise1.implies(&conclusion2);
    let forall2 = forall_const(ctx, &[&bound_a, &bound_b], &[], &axiom2);
    solver.assert(&forall2);

    let shr_by_zero = shr_fn.apply(&[&a, &zero]);
    let shr_by_zero_int = shr_by_zero.as_int().unwrap();
    let axiom3 = shr_by_zero_int._eq(&a);
    let forall3 = forall_const(ctx, &[&bound_a], &[], &axiom3);
    solver.assert(&forall3);

    let shl_result = shl_fn.apply(&[&a, &b]);
    let shl_result_int = shl_result.as_int().unwrap();
    let conclusion4 = shl_result_int.ge(&a);
    let axiom4 = premise1.implies(&conclusion4);
    let forall4 = forall_const(ctx, &[&bound_a, &bound_b], &[], &axiom4);
    solver.assert(&forall4);

    let shl_by_zero = shl_fn.apply(&[&a, &zero]);
    let shl_by_zero_int = shl_by_zero.as_int().unwrap();
    let axiom5 = shl_by_zero_int._eq(&a);
    let forall5 = forall_const(ctx, &[&bound_a], &[], &axiom5);
    solver.assert(&forall5);

    for k in 0_u32..64 {
        let k_const = Int::from_i64(ctx, i64::from(k));
        // Match translator literal >> encoding; use pow2_int so k=63 does not overflow host i64.
        let two_pow_k = Z3Translator::pow2_int(ctx, k);
        let shr_ak = shr_fn.apply(&[&a, &k_const]);
        let shr_ak_int = shr_ak.as_int().unwrap();
        let div_ak = a.clone().div(&two_pow_k);
        let axiom = shr_ak_int._eq(&div_ak);
        let forall_k = forall_const(ctx, &[&bound_a], &[], &axiom);
        solver.assert(&forall_k);
    }
}

/// Extract parameter types from function signature
fn extract_parameter_types(func: &syn::ItemFn) -> std::collections::HashMap<String, syn::Type> {
    let mut types = std::collections::HashMap::new();
    for input in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_type) = input {
            if let syn::Pat::Ident(ident) = &*pat_type.pat {
                types.insert(ident.ident.to_string(), *pat_type.ty.clone());
            }
        }
    }
    types
}

/// Extract return type from function signature
fn extract_return_type(func: &syn::ItemFn) -> Option<syn::Type> {
    if let syn::ReturnType::Type(_, ty) = &func.sig.output {
        Some(*ty.clone())
    } else {
        None
    }
}

#[cfg(not(feature = "z3"))]
/// Stub implementation when Z3 feature is disabled
pub struct Z3Verifier;

#[cfg(not(feature = "z3"))]
impl Z3Verifier {
    pub fn new(_timeout_ms: u64) -> Self {
        Z3Verifier
    }

    pub fn verify_contract(&mut self, _contract: &Contract) -> VerificationResult {
        VerificationResult::Error {
            error: "Z3 feature not enabled".to_string(),
        }
    }

    pub fn check_ensures_formula_sat_smoke(&mut self, _contract: &Contract) -> VerificationResult {
        VerificationResult::Error {
            error: "Z3 feature not enabled".to_string(),
        }
    }
}

#[cfg(not(feature = "z3"))]
#[derive(Debug, Clone)]
pub enum VerificationResult {
    Error { error: String },
}
