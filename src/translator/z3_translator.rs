//! Z3 translator: Rust AST → Z3 AST translation
//!
//! Translates Rust expressions AND function bodies to Z3 expressions for verification.
//! Focused on Bitcoin-specific patterns (u64, i64, Vec, arithmetic, comparisons).
//!
//! ## Key Insight: Implementation IS the Formula
//!
//! For ensures contracts, we don't just check "can the postcondition be violated?"
//! We translate the ACTUAL IMPLEMENTATION to Z3 and verify:
//!   requires + implementation_formula => ensures
//!
//! This makes the Orange Paper the single source of truth:
//! - Orange Paper defines the math (contracts)
//! - Implementation must satisfy the math
//! - Z3 proves implementation => postcondition

use crate::parser::contracts::Contract;
use syn::{Block, Expr, ItemFn, Stmt};
use z3::ast::{Ast, Bool, Int};
#[cfg(feature = "z3")]
use z3::{Config, Context, Sort};

#[cfg(feature = "z3")]
/// Variable map for Z3 translation: holds Int or Bool (for result when return type is bool)
pub type Z3VarMap<'a> = std::collections::HashMap<String, z3::ast::Dynamic<'a>>;

#[cfg(feature = "z3")]
/// Z3 translator for Rust expressions
pub struct Z3Translator {
    ctx: Context,
}

#[cfg(feature = "z3")]
impl Z3Translator {
    /// Create a new Z3 translator.
    /// `timeout_ms`: Solver timeout in milliseconds for deterministic verification.
    pub fn new(timeout_ms: u64) -> Self {
        let mut cfg = Config::new();
        cfg.set_proof_generation(true);
        cfg.set_model_generation(true);
        if timeout_ms > 0 {
            cfg.set_param_value("timeout", &timeout_ms.to_string());
        }
        let ctx = Context::new(&cfg);

        Z3Translator { ctx }
    }

    /// Get the Z3 context
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Translate a Rust expression to a Z3 expression
    ///
    /// Uses a variable map to ensure same variable name = same Z3 variable within one expression
    pub fn translate_expr_with_vars<'a>(
        &'a self,
        expr: &Expr,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        match expr {
            Expr::Lit(lit) => self.translate_literal(&lit.lit),
            Expr::Path(path) => {
                let name = path_to_string(&path.path);

                // Check if this is a known constant
                if let Some(constant_value) = resolve_constant(&name) {
                    return Ok(Int::from_i64(&self.ctx, constant_value).into());
                }

                // Get or create variable (new vars default to Int)
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Binary(bin) => {
                let left = self.translate_expr_with_vars(&bin.left, vars)?;
                let right = self.translate_expr_with_vars(&bin.right, vars)?;
                // Concrete shr/shl when right is literal (avoids uninterpreted function)
                if matches!(bin.op, syn::BinOp::Shr(_)) {
                    if let Some(shift_val) = extract_int_literal(&bin.right) {
                        if (0..64).contains(&shift_val) {
                            let left_int = left.as_int().ok_or_else(|| {
                                TranslationError::TypeError("Expected Int".to_string())
                            })?;
                            let divisor = 1i64 << shift_val;
                            return Ok(left_int.div(&Int::from_i64(&self.ctx, divisor)).into());
                        }
                    }
                }
                if matches!(bin.op, syn::BinOp::Shl(_)) {
                    if let Some(shift_val) = extract_int_literal(&bin.right) {
                        if (0..64).contains(&shift_val) {
                            let left_int = left.as_int().ok_or_else(|| {
                                TranslationError::TypeError("Expected Int".to_string())
                            })?;
                            let multiplier = 1i64 << shift_val;
                            return Ok((left_int * Int::from_i64(&self.ctx, multiplier)).into());
                        }
                    }
                }
                // BitAnd (&): model with div/rem for constant masks (Z3 Int has no native bitwise)
                if matches!(bin.op, syn::BinOp::BitAnd(_)) {
                    let left_int = left
                        .as_int()
                        .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                    let right_int = right
                        .as_int()
                        .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                    if let Some(mask) = extract_int_literal(&bin.right) {
                        if mask > 0 {
                            let mask_u = mask as u64;
                            // Power of 2 (single bit): a & 2^k = ((a / 2^k) % 2) * 2^k
                            if mask_u.is_power_of_two() {
                                let divisor = Int::from_i64(&self.ctx, mask);
                                let two = Int::from_i64(&self.ctx, 2);
                                let quotient = left_int.div(&divisor);
                                let bit = quotient.rem(&two);
                                return Ok((bit * divisor).into());
                            }
                            // 2^k - 1 (lower k bits): a & (2^k-1) = a % 2^k
                            let next_power = mask + 1;
                            if (next_power as u64).is_power_of_two() {
                                let modulus = Int::from_i64(&self.ctx, next_power);
                                return Ok(left_int.rem(&modulus).into());
                            }
                        }
                    }
                    // Fallback: uninterpreted function (like Shr/Shl for non-constant)
                    let bitand_fn = z3::FuncDecl::new(
                        &self.ctx,
                        "bitand",
                        &[&Sort::int(&self.ctx), &Sort::int(&self.ctx)],
                        &Sort::int(&self.ctx),
                    );
                    return Ok(bitand_fn.apply(&[&left_int, &right_int]));
                }
                self.translate_binary_op(bin.op, left, right)
            }
            Expr::MethodCall(method) => self.translate_method_call(method, vars),
            Expr::Call(call) => self.translate_call(call, vars),
            Expr::Unary(unary) => {
                let expr = self.translate_expr_with_vars(&unary.expr, vars)?;
                self.translate_unary_op(unary.op, expr)
            }
            Expr::Paren(paren) => self.translate_expr_with_vars(&paren.expr, vars),
            Expr::Cast(cast) => {
                // (expr) as Type - pass through (we model as Int)
                self.translate_expr_with_vars(&cast.expr, vars)
            }
            Expr::Reference(reference) => {
                // &expr - pass through (we don't model pointers)
                self.translate_expr_with_vars(&reference.expr, vars)
            }
            Expr::Tuple(tuple) => {
                // (a, b, c) - for determinism, use first element or fresh var from all
                if let Some(first) = tuple.elems.first() {
                    self.translate_expr_with_vars(first, vars)
                } else {
                    let name = format!("tuple_{}", vars.len());
                    let var = vars.entry(name.clone()).or_insert_with(|| {
                        let symbol = z3::Symbol::String(name);
                        Int::new_const(&self.ctx, symbol).into()
                    });
                    Ok(var.clone())
                }
            }
            Expr::Try(try_expr) => {
                // expr? - pass through (model Ok case; Err => early return handled elsewhere)
                self.translate_expr_with_vars(&try_expr.expr, vars)
            }
            Expr::Range(range) => {
                // 0..n or a..b - model as fresh Int (range bounds)
                let _ = range
                    .start
                    .as_ref()
                    .map(|e| self.translate_expr_with_vars(e, vars));
                let _ = range
                    .end
                    .as_ref()
                    .map(|e| self.translate_expr_with_vars(e, vars));
                let name = format!("range_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Field(field) => self.translate_field_access(field, vars),
            Expr::Index(index) => {
                // slice[i] or vec[i] - model as fresh Int (conservative; no array semantics)
                let _ = self.translate_expr_with_vars(&index.expr, vars)?;
                let _ = self.translate_expr_with_vars(&index.index, vars)?;
                let fresh = format!("index_{}", vars.len());
                let var = vars.entry(fresh.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(fresh);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Block(block) => {
                // Block expression: { stmts; expr } - process lets for bindings, translate last expr
                for stmt in &block.block.stmts {
                    if let Stmt::Local(local) = stmt {
                        if let Some(init) = &local.init {
                            if let syn::Pat::Ident(ident) = &local.pat {
                                let var_name = ident.ident.to_string();
                                if let Ok(z3_expr) = self.translate_expr_with_vars(&init.expr, vars)
                                {
                                    if z3_expr.as_int().is_some() || z3_expr.as_bool().is_some() {
                                        vars.insert(var_name, z3_expr);
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(Stmt::Expr(expr, None)) = block.block.stmts.last() {
                    return self.translate_expr_with_vars(expr, vars);
                }
                Err(TranslationError::UnsupportedExpression(
                    "Block expression: no final expression".to_string(),
                ))
            }
            Expr::Match(match_expr) => self.translate_match(match_expr, vars),
            Expr::ForLoop(fl) => {
                let _ = self.translate_expr_with_vars(&fl.expr, vars);
                let _ = self.translate_block_to_result_formula(&fl.body, vars);
                let name = format!("for_loop_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Closure(closure) => {
                let _ = self.translate_expr_with_vars(&closure.body, vars);
                let name = format!("closure_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Repeat(repeat) => {
                // [expr; len] - e.g. [0u8; 32] for zero hash. Model as constant or fresh Int.
                let _ = self.translate_expr_with_vars(&repeat.len, vars);
                if let Some(val) = extract_int_literal(&repeat.expr) {
                    if val == 0 {
                        return Ok(Int::from_i64(&self.ctx, 0).into());
                    }
                }
                let name = format!("repeat_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Array(array) => {
                // [a, b, c] - model as constant if all same literal, else fresh Int
                if array.elems.len() == 1 {
                    if let Some(val) = extract_int_literal(array.elems.first().unwrap()) {
                        return Ok(Int::from_i64(&self.ctx, val).into());
                    }
                }
                let name = format!("array_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::If(if_expr) => {
                // Standalone if (not in block context) - translate to ite
                self.translate_if_expr_to_dynamic(if_expr, vars)
            }
            Expr::While(while_expr) => {
                let _ = self.translate_expr_with_vars(&while_expr.cond, vars);
                let _ = self.translate_block_to_result_formula(&while_expr.body, vars);
                let name = format!("while_loop_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Loop(loop_expr) => {
                let _ = self.translate_block_to_result_formula(&loop_expr.body, vars);
                let name = format!("loop_{}", vars.len());
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            Expr::Let(let_expr) => {
                // if let Some(x)=opt / Ok(x)=res / Err(e)=res / None=opt - used as condition in if
                // Returns Bool (matches/doesn't match); binds inner var for then-branch when present
                let (inner_var_opt, bool_suffix, use_as_is) =
                    parse_let_option_result_pat(&let_expr.pat).ok_or_else(|| {
                        TranslationError::UnsupportedExpression(format!(
                            "Let: only Some(x), Ok(x), Err(e), Err(_), None patterns supported, got {:?}",
                            let_expr.pat
                        ))
                    })?;
                let _ = self.translate_expr_with_vars(&let_expr.expr, vars)?;
                let base = expr_to_var_hint(&let_expr.expr);
                let bool_name = format!("{base}_{bool_suffix}");
                let raw_bool = vars
                    .entry(bool_name.clone())
                    .or_insert_with(|| {
                        let symbol = z3::Symbol::String(bool_name);
                        Bool::new_const(&self.ctx, symbol).into()
                    })
                    .clone();
                let cond_bool = raw_bool.as_bool().ok_or_else(|| {
                    TranslationError::TypeError(format!("{bool_suffix}: expected Bool"))
                })?;
                let cond_bool = if use_as_is {
                    cond_bool
                } else {
                    cond_bool.not()
                };
                if let Some(inner_var) = inner_var_opt {
                    let inner_val = vars
                        .entry(inner_var.clone())
                        .or_insert_with(|| {
                            let symbol = z3::Symbol::String(inner_var.clone());
                            Int::new_const(&self.ctx, symbol).into()
                        })
                        .clone();
                    vars.insert(inner_var, inner_val);
                }
                Ok(cond_bool.into())
            }
            _ => Err(TranslationError::UnsupportedExpression(format!("{expr:?}"))),
        }
    }

    /// Translate a Rust expression to a Z3 expression (public API)
    pub fn translate_expr(&self, expr: &Expr) -> Result<z3::ast::Dynamic<'_>, TranslationError> {
        let mut vars = Z3VarMap::new();
        self.translate_expr_with_vars(expr, &mut vars)
    }

    /// Translate a literal (integer, boolean, etc.)
    fn translate_literal(&self, lit: &syn::Lit) -> Result<z3::ast::Dynamic<'_>, TranslationError> {
        match lit {
            syn::Lit::Int(int_lit) => {
                let value = int_lit
                    .base10_parse::<i64>()
                    .map_err(|e| TranslationError::ParseError(e.to_string()))?;
                Ok(Int::from_i64(&self.ctx, value).into())
            }
            syn::Lit::Bool(bool_lit) => Ok(Bool::from_bool(&self.ctx, bool_lit.value).into()),
            _ => Err(TranslationError::UnsupportedLiteral(format!("{lit:?}"))),
        }
    }

    /// Translate a binary operation given already-translated operands
    fn translate_binary_op<'a>(
        &'a self,
        op: syn::BinOp,
        left: z3::ast::Dynamic<'a>,
        right: z3::ast::Dynamic<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        match op {
            syn::BinOp::Add(_) => {
                let left_int = left
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                let right_int = right
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                Ok((left_int + right_int).into())
            }
            syn::BinOp::Sub(_) => {
                let left_int = left
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                let right_int = right
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                Ok((left_int - right_int).into())
            }
            syn::BinOp::Mul(_) => {
                let left_int = left
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                let right_int = right
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                Ok((left_int * right_int).into())
            }
            syn::BinOp::Div(_) => {
                // Z3 integer division
                let left_int = left
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                let right_int = right
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                Ok(left_int.div(&right_int).into())
            }
            syn::BinOp::Shr(_) => {
                // Right shift: a >> b is equivalent to a / 2^b for non-negative values
                // For simplicity with Z3, we model this using uninterpreted function
                // or approximate for common cases
                let left_int = left
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                let right_int = right
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;

                // Create an uninterpreted shift function
                // This allows Z3 to reason about shift operations abstractly
                let shift_fn = z3::FuncDecl::new(
                    &self.ctx,
                    "shr",
                    &[&Sort::int(&self.ctx), &Sort::int(&self.ctx)],
                    &Sort::int(&self.ctx),
                );
                Ok(shift_fn.apply(&[&left_int, &right_int]))
            }
            syn::BinOp::Shl(_) => {
                // Left shift: a << b is equivalent to a * 2^b
                let left_int = left
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                let right_int = right
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;

                let shift_fn = z3::FuncDecl::new(
                    &self.ctx,
                    "shl",
                    &[&Sort::int(&self.ctx), &Sort::int(&self.ctx)],
                    &Sort::int(&self.ctx),
                );
                Ok(shift_fn.apply(&[&left_int, &right_int]))
            }
            syn::BinOp::Eq(_) => {
                // Equality: Bool-Bool, Int-Int, and Bool-Int (spec: \in {true,false} for 0/1)
                let eq = match (
                    left.as_bool(),
                    left.as_int(),
                    right.as_bool(),
                    right.as_int(),
                ) {
                    (Some(l), _, Some(r), _) => l._eq(&r),
                    (_, Some(l), _, Some(r)) => l._eq(&r),
                    (Some(l), _, _, Some(r)) => {
                        let one = Int::from_i64(&self.ctx, 1);
                        let zero = Int::from_i64(&self.ctx, 0);
                        let l_int = l.ite(&one, &zero);
                        l_int._eq(&r)
                    }
                    (_, Some(l), Some(r), _) => {
                        let one = Int::from_i64(&self.ctx, 1);
                        let zero = Int::from_i64(&self.ctx, 0);
                        let r_int = r.ite(&one, &zero);
                        l._eq(&r_int)
                    }
                    _ => {
                        return Err(TranslationError::TypeError(
                            "Eq: operands must be Bool or Int (got mixed sorts)".to_string(),
                        ))
                    }
                };
                Ok(eq.into())
            }
            syn::BinOp::Ne(_) => {
                let eq = match (
                    left.as_bool(),
                    left.as_int(),
                    right.as_bool(),
                    right.as_int(),
                ) {
                    (Some(l), _, Some(r), _) => l._eq(&r),
                    (_, Some(l), _, Some(r)) => l._eq(&r),
                    (Some(l), _, _, Some(r)) => {
                        let one = Int::from_i64(&self.ctx, 1);
                        let zero = Int::from_i64(&self.ctx, 0);
                        let l_int = l.ite(&one, &zero);
                        l_int._eq(&r)
                    }
                    (_, Some(l), Some(r), _) => {
                        let one = Int::from_i64(&self.ctx, 1);
                        let zero = Int::from_i64(&self.ctx, 0);
                        let r_int = r.ite(&one, &zero);
                        l._eq(&r_int)
                    }
                    _ => {
                        return Err(TranslationError::TypeError(
                            "Ne: operands must be Bool or Int (got mixed sorts)".to_string(),
                        ))
                    }
                };
                Ok(eq.not().into())
            }
            syn::BinOp::Lt(_) | syn::BinOp::Le(_) | syn::BinOp::Gt(_) | syn::BinOp::Ge(_) => {
                // Coerce Bool to Int (true=1, false=0) for comparisons - handles spec patterns
                let left_int = left
                    .as_int()
                    .or_else(|| {
                        left.as_bool().map(|b| {
                            let one = Int::from_i64(&self.ctx, 1);
                            let zero = Int::from_i64(&self.ctx, 0);
                            b.ite(&one, &zero)
                        })
                    })
                    .ok_or_else(|| {
                        TranslationError::TypeError(
                            "Lt/Le/Gt/Ge: left operand must be Int or Bool".to_string(),
                        )
                    })?;
                let right_int = right
                    .as_int()
                    .or_else(|| {
                        right.as_bool().map(|b| {
                            let one = Int::from_i64(&self.ctx, 1);
                            let zero = Int::from_i64(&self.ctx, 0);
                            b.ite(&one, &zero)
                        })
                    })
                    .ok_or_else(|| {
                        TranslationError::TypeError(
                            "Lt/Le/Gt/Ge: right operand must be Int or Bool".to_string(),
                        )
                    })?;
                let cmp = match op {
                    syn::BinOp::Lt(_) => left_int.lt(&right_int),
                    syn::BinOp::Le(_) => left_int.le(&right_int),
                    syn::BinOp::Gt(_) => left_int.gt(&right_int),
                    syn::BinOp::Ge(_) => left_int.ge(&right_int),
                    _ => unreachable!(),
                };
                Ok(cmp.into())
            }
            syn::BinOp::And(_) => {
                let left_bool = left
                    .as_bool()
                    .ok_or_else(|| TranslationError::TypeError("Expected Bool".to_string()))?;
                let right_bool = right
                    .as_bool()
                    .ok_or_else(|| TranslationError::TypeError("Expected Bool".to_string()))?;
                Ok((left_bool & right_bool).into())
            }
            syn::BinOp::Or(_) => {
                let left_bool = left
                    .as_bool()
                    .ok_or_else(|| TranslationError::TypeError("Expected Bool".to_string()))?;
                let right_bool = right
                    .as_bool()
                    .ok_or_else(|| TranslationError::TypeError("Expected Bool".to_string()))?;
                Ok((left_bool | right_bool).into())
            }
            _ => Err(TranslationError::UnsupportedOperator(format!("{op:?}"))),
        }
    }

    /// Translate a method call (e.g., vec.len(), opt.is_some())
    fn translate_method_call<'a>(
        &'a self,
        method: &syn::ExprMethodCall,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        let method_name = method.method.to_string();

        match method_name.as_str() {
            "checked_add" | "checked_sub" | "checked_mul" => {
                // a.checked_add(b) returns Option; model as a+b (overflow ignored for verification)
                let left = self.translate_expr_with_vars(&method.receiver, vars)?;
                let arg = method.args.first().ok_or_else(|| {
                    TranslationError::UnsupportedExpression("checked_add needs 1 arg".to_string())
                })?;
                let right = self.translate_expr_with_vars(arg, vars)?;
                let left_int = left.as_int().ok_or_else(|| {
                    TranslationError::TypeError("checked_add: expected Int".to_string())
                })?;
                let right_int = right.as_int().ok_or_else(|| {
                    TranslationError::TypeError("checked_add: expected Int".to_string())
                })?;
                let result = match method_name.as_str() {
                    "checked_add" => left_int + right_int,
                    "checked_sub" => left_int - right_int,
                    "checked_mul" => left_int * right_int,
                    _ => unreachable!(),
                };
                Ok(result.into())
            }
            "unwrap_or" | "unwrap_or_else" => {
                // x.unwrap_or(default) or x.unwrap_or_else(|| default) - use receiver (the Option)
                self.translate_expr_with_vars(&method.receiver, vars)
            }
            "min" | "max" => {
                let left = self.translate_expr_with_vars(&method.receiver, vars)?;
                let arg = method.args.first().ok_or_else(|| {
                    TranslationError::UnsupportedExpression("min/max needs 1 arg".to_string())
                })?;
                let right = self.translate_expr_with_vars(arg, vars)?;
                let left_int = left.as_int().ok_or_else(|| {
                    TranslationError::TypeError("min/max: expected Int".to_string())
                })?;
                let right_int = right.as_int().ok_or_else(|| {
                    TranslationError::TypeError("min/max: expected Int".to_string())
                })?;
                let result = match method_name.as_str() {
                    "min" => left_int.lt(&right_int).ite(&left_int, &right_int),
                    "max" => left_int.gt(&right_int).ite(&left_int, &right_int),
                    _ => unreachable!(),
                };
                Ok(result.into())
            }
            "with" => {
                // thread_local.with(|cell| block) - translate closure body
                if let Some(Expr::Closure(closure)) = method.args.first() {
                    if closure.inputs.len() == 1 {
                        if let syn::Pat::Ident(ident) = &closure.inputs[0] {
                            let param = ident.ident.to_string();
                            let fresh = format!("with_{param}");
                            let new_var = Int::new_const(&self.ctx, z3::Symbol::String(fresh));
                            let old = vars.insert(param.clone(), new_var.into());
                            let result = self.translate_expr_with_vars(&closure.body, vars);
                            if let Some(prev) = old {
                                vars.insert(param, prev);
                            } else {
                                vars.remove(&param);
                            }
                            return result;
                        }
                    }
                }
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_with_result");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "clear" | "extend_from_slice" | "push" | "copy_from_slice" => {
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                for arg in &method.args {
                    let _ = self.translate_expr_with_vars(arg, vars);
                }
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_after_{method_name}");
                let var_val = vars
                    .entry(name.clone())
                    .or_insert_with(|| {
                        let symbol = z3::Symbol::String(name);
                        Int::new_const(&self.ctx, symbol).into()
                    })
                    .clone();
                // Update receiver so subsequent uses (e.g. digest(&bytes)) see mutated value
                if let Expr::Path(path) = &*method.receiver {
                    vars.insert(path_to_string(&path.path), var_val.clone());
                }
                Ok(var_val)
            }
            "digest" | "finalize" => {
                // Uninterpreted: digest(data) for determinism. Receiver holds state; use first arg if any.
                if let Some(arg) = method.args.first() {
                    let data = self.translate_expr_with_vars(arg, vars)?;
                    let data_int = data.as_int().ok_or_else(|| {
                        TranslationError::TypeError("digest arg must be Int".to_string())
                    })?;
                    let int_sort = Sort::int(&self.ctx);
                    let fn_decl =
                        z3::FuncDecl::new(&self.ctx, "sha256_digest", &[&int_sort], &int_sort);
                    return Ok(fn_decl.apply(&[&data_int]));
                }
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_digest");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "hash256" | "hash" => {
                // Uninterpreted: hash256/hash(data) for determinism
                let arg = method.args.first().ok_or_else(|| {
                    TranslationError::UnsupportedExpression("hash256/hash needs 1 arg".to_string())
                })?;
                let data = self.translate_expr_with_vars(arg, vars)?;
                let data_int = data.as_int().ok_or_else(|| {
                    TranslationError::TypeError("hash arg must be Int".to_string())
                })?;
                let int_sort = Sort::int(&self.ctx);
                let fn_decl = z3::FuncDecl::new(&self.ctx, "hash256", &[&int_sort], &int_sort);
                Ok(fn_decl.apply(&[&data_int]))
            }
            "into" => self.translate_expr_with_vars(&method.receiver, vars),
            "clone" | "serialize" => {
                // x.clone() / x.serialize() - pass through (same value for determinism)
                self.translate_expr_with_vars(&method.receiver, vars)
            }
            "as_ref" => {
                // option.as_ref() - pass through (Option<&T> from Option<T>)
                self.translate_expr_with_vars(&method.receiver, vars)
            }
            "map" => {
                // option.map(|x| f(x)) - use closure body or pass through
                if let Some(Expr::Closure(closure)) = method.args.first() {
                    if closure.inputs.len() == 1 {
                        if let syn::Pat::Ident(ident) = &closure.inputs[0] {
                            let param = ident.ident.to_string();
                            let _recv = self.translate_expr_with_vars(&method.receiver, vars)?;
                            let fresh = format!("map_{param}");
                            let new_var = Int::new_const(&self.ctx, z3::Symbol::String(fresh));
                            let old = vars.insert(param.clone(), new_var.into());
                            let result = self.translate_expr_with_vars(&closure.body, vars);
                            if let Some(prev) = old {
                                vars.insert(param, prev);
                            } else {
                                vars.remove(&param);
                            }
                            if let Ok(r) = result {
                                return Ok(r);
                            }
                        }
                    }
                }
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_map");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "borrow_mut" | "borrow" => {
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_borrow");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "len" => {
                // vec.len() - return fresh Int (length); receiver type not modeled
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = if base == "x" {
                    "len_result".to_string()
                } else {
                    format!("{base}_len")
                };
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "iter" | "into_iter" | "par_iter" => {
                // Pass through to receiver - iterator over collection
                self.translate_expr_with_vars(&method.receiver, vars)
            }
            "step_by" => {
                // (0..n).step_by(2) - pass through (iterator adapter)
                self.translate_expr_with_vars(&method.receiver, vars)
            }
            "collect" => {
                // iter.collect() - return fresh var for collected value
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_collect");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "enumerate" => {
                // iter.enumerate() - pass through (iterator adapter)
                self.translate_expr_with_vars(&method.receiver, vars)
            }
            "next" => {
                // iter.next() - return fresh var for next element
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_next");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "sum" => {
                // iter.sum() - return fresh var for sum (e.g. witness_data.iter().map(|w| w.len()).sum())
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_sum");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "fold" => {
                // iter.fold(init, |acc, x| body) - return fresh var for folded result
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                if let Some(init) = method.args.first() {
                    let _ = self.translate_expr_with_vars(init, vars);
                }
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_fold");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "reduce" => {
                // iter.reduce(|a, b| body) - return fresh var for reduced result
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_reduce");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Int::new_const(&self.ctx, symbol).into()
                });
                Ok(var.clone())
            }
            "saturating_sub" => {
                // a.saturating_sub(b) - model as a - b (determinism: same inputs => same output)
                let left = self.translate_expr_with_vars(&method.receiver, vars)?;
                let arg = method.args.first().ok_or_else(|| {
                    TranslationError::UnsupportedExpression(
                        "saturating_sub needs 1 arg".to_string(),
                    )
                })?;
                let right = self.translate_expr_with_vars(arg, vars)?;
                let left_int = left.as_int().ok_or_else(|| {
                    TranslationError::TypeError("saturating_sub: expected Int".to_string())
                })?;
                let right_int = right.as_int().ok_or_else(|| {
                    TranslationError::TypeError("saturating_sub: expected Int".to_string())
                })?;
                Ok((left_int - right_int).into())
            }
            "to_le_bytes" => {
                // x.to_le_bytes() - uninterpreted for determinism (bytes from int)
                let recv = self.translate_expr_with_vars(&method.receiver, vars)?;
                let recv_int = recv.as_int().ok_or_else(|| {
                    TranslationError::TypeError("to_le_bytes: expected Int".to_string())
                })?;
                let int_sort = Sort::int(&self.ctx);
                let fn_decl = z3::FuncDecl::new(&self.ctx, "to_le_bytes", &[&int_sort], &int_sort);
                Ok(fn_decl.apply(&[&recv_int]))
            }
            "is_some" | "is_none" => {
                // Option checks: return Bool. Use fresh var for unknown Option; name from receiver.
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_is_some");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Bool::new_const(&self.ctx, symbol).into()
                });
                let bool_val = var.as_bool().ok_or_else(|| {
                    TranslationError::TypeError("is_some: expected Bool".to_string())
                })?;
                Ok(if method_name == "is_none" {
                    bool_val.not().into()
                } else {
                    bool_val.clone().into()
                })
            }
            "is_ok" | "is_err" => {
                // Result checks: same as Option (Ok/Err)
                let _ = self.translate_expr_with_vars(&method.receiver, vars);
                let base = expr_to_var_hint(&method.receiver);
                let name = format!("{base}_is_ok");
                let var = vars.entry(name.clone()).or_insert_with(|| {
                    let symbol = z3::Symbol::String(name);
                    Bool::new_const(&self.ctx, symbol).into()
                });
                let bool_val = var.as_bool().ok_or_else(|| {
                    TranslationError::TypeError("is_ok: expected Bool".to_string())
                })?;
                Ok(if method_name == "is_err" {
                    bool_val.not().into()
                } else {
                    bool_val.clone().into()
                })
            }
            "unwrap" => {
                // opt.unwrap() or res.unwrap(): return inner value when Some/Ok.
                // If receiver is Some(expr) or Ok(expr), use that; else fresh Int.
                let receiver = &method.receiver;
                if method.args.is_empty() {
                    // Try to unwrap from call: Some(x) or Ok(x)
                    if let Expr::Call(call) = &**receiver {
                        if let Ok(name) = call_expr_to_name(&call.func) {
                            let base = name.split("::").last().unwrap_or(&name);
                            if (base == "Some" || base == "Ok") && call.args.len() == 1 {
                                return self.translate_expr_with_vars(&call.args[0], vars);
                            }
                        }
                    }
                    // Fallback: fresh Int for unknown Option/Result value
                    let _ = self.translate_expr_with_vars(receiver, vars);
                    let base = expr_to_var_hint(receiver);
                    let name = format!("{base}_unwrap");
                    let var = vars.entry(name.clone()).or_insert_with(|| {
                        let symbol = z3::Symbol::String(name);
                        Int::new_const(&self.ctx, symbol).into()
                    });
                    return Ok(var.clone());
                }
                Err(TranslationError::UnsupportedExpression(
                    "unwrap with args not supported".to_string(),
                ))
            }
            _ => Err(TranslationError::UnsupportedExpression(format!(
                "Method call: {method_name}"
            ))),
        }
    }

    /// Translate a function call. For known functions returning Int, returns a fresh variable
    /// (allows body translation to proceed; formula is conservative).
    /// For Ok(expr) and Some(expr), returns the inner expr (Result/Option unwrap for ensures).
    fn translate_call<'a>(
        &'a self,
        call: &syn::ExprCall,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        let name = call_expr_to_name(&call.func)?;
        let base = name.split("::").last().unwrap_or(&name);
        // result(args): Spec replaces GetMedianTimePast(headers) etc. with result(headers).
        // Treat as the result variable (return value of the function under verification).
        if base == "result" {
            let var = vars.entry("result".to_string()).or_insert_with(|| {
                let symbol = z3::Symbol::String("result".to_string());
                Int::new_const(&self.ctx, symbol).into()
            });
            return Ok(var.clone());
        }
        // Ok(expr) / Some(expr): result is the inner value (for Result<T,E> / Option<T> returns)
        if (base == "Ok" || base == "Some") && call.args.len() == 1 {
            return self.translate_expr_with_vars(&call.args[0], vars);
        }
        // Constructors (new, default) - return fresh var for determinism
        if base == "new" || base == "default" {
            for arg in &call.args {
                let _ = self.translate_expr_with_vars(arg, vars);
            }
            let name = format!("{base}_result");
            let var = vars.entry(name.clone()).or_insert_with(|| {
                let symbol = z3::Symbol::String(name);
                Int::new_const(&self.ctx, symbol).into()
            });
            return Ok(var.clone());
        }
        // Err(_): for Result<T>, use 0 for determinism (0=Err, 1=Ok(false), 2=Ok(true))
        if base == "Err" && call.args.len() <= 1 {
            return Ok(Int::from_i64(&self.ctx, 0).into());
        }
        // Bool-returning uninterpreted (Result<bool>, Option<bool>)
        if let Some((fn_name, arity_opt)) = known_bool_uninterpreted_function(&name) {
            let mut arg_asts: Vec<z3::ast::Int> = Vec::new();
            for arg in &call.args {
                let z3_arg = self.translate_expr_with_vars(arg, vars)?;
                let int_arg = if let Some(i) = z3_arg.as_int() {
                    i
                } else if let Some(b) = z3_arg.as_bool() {
                    let zero = Int::from_i64(&self.ctx, 0);
                    let one = Int::from_i64(&self.ctx, 1);
                    b.ite(&one, &zero)
                } else {
                    return Err(TranslationError::TypeError(
                        "Bool uninterpreted fn arg must be Int or Bool".to_string(),
                    ));
                };
                arg_asts.push(int_arg);
            }
            let arity = arg_asts.len();
            if let Some(expected) = arity_opt {
                if arity != expected {
                    return Err(TranslationError::UnsupportedExpression(format!(
                        "{base} expects {expected} args, got {arity}"
                    )));
                }
            }
            let int_sort = Sort::int(&self.ctx);
            let bool_sort = Sort::bool(&self.ctx);
            let sorts: Vec<&z3::Sort> = std::iter::repeat_n(&int_sort, arity).collect();
            let z3_fn_name = if arity_opt.is_some() {
                format!("{fn_name}_b")
            } else {
                format!("{fn_name}_{arity}_b")
            };
            let fn_decl = z3::FuncDecl::new(
                &self.ctx,
                z3::Symbol::String(z3_fn_name),
                sorts.as_slice(),
                &bool_sort,
            );
            let arg_refs: Vec<&dyn Ast> = arg_asts.iter().map(|a| a as &dyn Ast).collect();
            let result = fn_decl.apply(&arg_refs);
            return Ok(result);
        }
        // Uninterpreted functions for determinism: f(args) so that same inputs => same output
        if let Some((fn_name, arity_opt)) = known_uninterpreted_function(&name) {
            let mut arg_asts: Vec<z3::ast::Int> = Vec::new();
            for arg in &call.args {
                let z3_arg = self.translate_expr_with_vars(arg, vars)?;
                let int_arg = if let Some(i) = z3_arg.as_int() {
                    i
                } else if let Some(b) = z3_arg.as_bool() {
                    let zero = Int::from_i64(&self.ctx, 0);
                    let one = Int::from_i64(&self.ctx, 1);
                    b.ite(&one, &zero)
                } else {
                    return Err(TranslationError::TypeError(
                        "Uninterpreted fn arg must be Int or Bool".to_string(),
                    ));
                };
                arg_asts.push(int_arg);
            }
            let arity = arg_asts.len();
            if let Some(expected) = arity_opt {
                if arity != expected {
                    return Err(TranslationError::UnsupportedExpression(format!(
                        "{base} expects {expected} args, got {arity}"
                    )));
                }
            }
            let int_sort = Sort::int(&self.ctx);
            let sorts: Vec<&z3::Sort> = std::iter::repeat_n(&int_sort, arity).collect();
            let z3_fn_name = if arity_opt.is_some() {
                fn_name.to_string()
            } else {
                format!("{fn_name}_{arity}")
            };
            let fn_decl = z3::FuncDecl::new(
                &self.ctx,
                z3::Symbol::String(z3_fn_name),
                sorts.as_slice(),
                &int_sort,
            );
            let arg_refs: Vec<&dyn Ast> = arg_asts.iter().map(|a| a as &dyn Ast).collect();
            let result = fn_decl.apply(&arg_refs);
            return Ok(result);
        }
        if let Some(fresh_name) = known_int_returning_function(&name) {
            // Translate args (for variable binding; we don't model the relation yet)
            for arg in &call.args {
                let _ = self.translate_expr_with_vars(arg, vars);
            }
            let var = vars.entry(fresh_name.clone()).or_insert_with(|| {
                let symbol = z3::Symbol::String(fresh_name);
                Int::new_const(&self.ctx, symbol).into()
            });
            return Ok(var.clone());
        }
        if let Some(fresh_name) = known_bool_returning_function(&name) {
            for arg in &call.args {
                let _ = self.translate_expr_with_vars(arg, vars);
            }
            let var = vars.entry(fresh_name.clone()).or_insert_with(|| {
                let symbol = z3::Symbol::String(fresh_name);
                Bool::new_const(&self.ctx, symbol).into()
            });
            return Ok(var.clone());
        }
        // Generic fallback: treat unknown calls as uninterpreted (same inputs => same output)
        let mut arg_asts: Vec<z3::ast::Int> = Vec::new();
        for arg in &call.args {
            let z3_arg = self.translate_expr_with_vars(arg, vars)?;
            let int_arg = if let Some(i) = z3_arg.as_int() {
                i
            } else if let Some(b) = z3_arg.as_bool() {
                // Coerce Bool to Int (0/1) for uninterpreted fn
                let zero = Int::from_i64(&self.ctx, 0);
                let one = Int::from_i64(&self.ctx, 1);
                b.ite(&one, &zero)
            } else {
                return Err(TranslationError::TypeError(
                    "Unknown fn arg must be Int or Bool".to_string(),
                ));
            };
            arg_asts.push(int_arg);
        }
        let arity = arg_asts.len();
        let base = name.split("::").last().unwrap_or(&name);
        let z3_fn_name = format!("call_{}_{}", base.replace("::", "_"), arity);
        let int_sort = Sort::int(&self.ctx);
        let sorts: Vec<&z3::Sort> = std::iter::repeat_n(&int_sort, arity).collect();
        let fn_decl = z3::FuncDecl::new(
            &self.ctx,
            z3::Symbol::String(z3_fn_name),
            sorts.as_slice(),
            &int_sort,
        );
        let arg_refs: Vec<&dyn Ast> = arg_asts.iter().map(|a| a as &dyn Ast).collect();
        Ok(fn_decl.apply(&arg_refs))
    }

    /// Translate match expr { Some(x) => a, None => b } or { Ok(v) => a, Err(_) => b }
    fn translate_match<'a>(
        &'a self,
        match_expr: &syn::ExprMatch,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        let _ = self.translate_expr_with_vars(&match_expr.expr, vars)?;
        let base = expr_to_var_hint(&match_expr.expr);
        if match_expr.arms.len() == 2 && match_expr.arms.iter().all(|a| a.guard.is_none()) {
            let (arm1, arm2) = (&match_expr.arms[0], &match_expr.arms[1]);
            let some_arm = parse_some_ok_arm(arm1).or_else(|| parse_some_ok_arm(arm2));
            let none_arm = parse_none_err_arm(arm1).or_else(|| parse_none_err_arm(arm2));
            if let (Some((inner_var, body)), Some(default_body)) = (some_arm, none_arm) {
                let inner_val = vars
                    .entry(inner_var.clone())
                    .or_insert_with(|| {
                        let symbol = z3::Symbol::String(inner_var.clone());
                        Int::new_const(&self.ctx, symbol).into()
                    })
                    .clone();
                vars.insert(inner_var, inner_val);
                let body_z3 = self.translate_expr_with_vars(body, vars)?;
                let default_z3 = self.translate_expr_with_vars(default_body, vars)?;
                let is_some_bool = vars
                    .entry(format!("{base}_is_some"))
                    .or_insert_with(|| {
                        let symbol = z3::Symbol::String(format!("{base}_is_some"));
                        Bool::new_const(&self.ctx, symbol).into()
                    })
                    .as_bool()
                    .ok_or_else(|| {
                        TranslationError::TypeError("is_some: expected Bool".to_string())
                    })?
                    .clone();
                let body_int = body_z3.as_int().ok_or_else(|| {
                    TranslationError::TypeError("match body: expected Int".to_string())
                })?;
                let default_int = default_z3.as_int().ok_or_else(|| {
                    TranslationError::TypeError("match default: expected Int".to_string())
                })?;
                return Ok(is_some_bool.ite(&body_int, &default_int).into());
            }
        }
        Err(TranslationError::UnsupportedExpression(
            "Match: only Option/Result with 2 arms (Some/Ok, None/Err) supported".to_string(),
        ))
    }

    /// Translate a unary operation given already-translated operand
    fn translate_unary_op<'a>(
        &'a self,
        op: syn::UnOp,
        expr: z3::ast::Dynamic<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        match op {
            syn::UnOp::Not(_) => {
                let bool_expr = expr
                    .as_bool()
                    .ok_or_else(|| TranslationError::TypeError("Expected Bool".to_string()))?;
                Ok(bool_expr.not().into())
            }
            syn::UnOp::Neg(_) => {
                let int_expr = expr
                    .as_int()
                    .ok_or_else(|| TranslationError::TypeError("Expected Int".to_string()))?;
                Ok(int_expr.unary_minus().into())
            }
            syn::UnOp::Deref(_) => {
                // Dereference - for now, just return the expression
                Ok(expr)
            }
            _ => Err(TranslationError::UnsupportedExpression(format!(
                "Unsupported unary op: {op:?}"
            ))),
        }
    }

    /// Translate struct field access (e.g. block.header.bits) to a Z3 variable.
    /// Flattens the path to a var name (block_header_bits) for lookup/creation.
    fn translate_field_access<'a>(
        &'a self,
        field: &syn::ExprField,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        let name = field_expr_to_var_name(field);
        let var = vars.entry(name.clone()).or_insert_with(|| {
            let symbol = z3::Symbol::String(name);
            Int::new_const(&self.ctx, symbol).into()
        });
        Ok(var.clone())
    }

    /// Translate a contract condition to Z3
    pub fn translate_contract(
        &self,
        contract: &Contract,
    ) -> Result<z3::ast::Dynamic<'_>, TranslationError> {
        let (expr, _) =
            self.translate_contract_with_types(contract, &std::collections::HashMap::new(), None)?;
        Ok(expr)
    }

    /// Translate a contract condition to Z3 with type information
    /// Returns the expression and type constraints
    pub fn translate_contract_with_types(
        &self,
        contract: &Contract,
        param_types: &std::collections::HashMap<String, syn::Type>,
        return_type: Option<&syn::Type>,
    ) -> Result<(z3::ast::Dynamic<'_>, Vec<z3::ast::Bool<'_>>), TranslationError> {
        let mut vars = Z3VarMap::new();
        let mut type_constraints = Vec::new();

        // Pre-create variables with type constraints for parameters
        for (name, ty) in param_types {
            let symbol = z3::Symbol::String(name.clone());
            let var = Int::new_const(&self.ctx, symbol);
            vars.insert(name.clone(), var.into());

            // Add type-based constraints
            if is_unsigned_type(ty) {
                if let Some(var_ref) = vars.get(name).and_then(|v| v.as_int()) {
                    type_constraints.push(var_ref.ge(&Int::from_i64(&self.ctx, 0)));
                }
            }
            // For signed types (i8, i16, i32, i64, isize, Integer), no constraint
            // For other types, we'd need more sophisticated handling
        }

        // Pre-create "result": Bool for bool/Result<bool>/Option<bool>, Int otherwise
        if let Some(return_ty) = return_type {
            let symbol = z3::Symbol::String("result".to_string());
            if returns_bool_like(return_ty) {
                let var = Bool::new_const(&self.ctx, symbol);
                vars.insert("result".to_string(), var.into());
            } else {
                let var = Int::new_const(&self.ctx, symbol);
                vars.insert("result".to_string(), var.into());
                if is_unsigned_type(return_ty) {
                    if let Some(var_ref) = vars.get("result").and_then(|v| v.as_int()) {
                        type_constraints.push(var_ref.ge(&Int::from_i64(&self.ctx, 0)));
                    }
                }
            }
        }

        let expr = self.translate_expr_with_vars(&contract.condition, &mut vars)?;
        Ok((expr, type_constraints))
    }

    /// Translate a function body to a Z3 formula that relates inputs to result
    ///
    /// This is the KEY for verifying ensures: we translate the implementation
    /// to a Z3 formula and prove: requires && implementation => ensures
    pub fn translate_function_body<'a>(
        &'a self,
        func: &ItemFn,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<Option<z3::ast::Bool<'a>>, TranslationError> {
        // Extract the function body
        let body = &func.block;

        // For simple functions, translate the body to a formula
        // result == <body_expression>
        self.translate_block_to_result_formula(body, vars)
    }

    /// Translate a block to a formula: result == <final_expression>
    ///
    /// Handles:
    /// - let bindings (variable assignments)
    /// - if expressions with early returns
    /// - debug_assert! macros (skipped)
    /// - final implicit return
    fn translate_block_to_result_formula<'a>(
        &'a self,
        block: &Block,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<Option<z3::ast::Bool<'a>>, TranslationError> {
        let _formulas: Vec<z3::ast::Bool<'a>> = Vec::new();
        let mut early_return_conditions: Vec<(z3::ast::Bool<'a>, z3::ast::Bool<'a>)> = Vec::new();

        // Process statements to build variable bindings and collect return conditions
        for stmt in &block.stmts {
            match stmt {
                Stmt::Local(local) => {
                    // let x = expr;
                    if let Some(init) = &local.init {
                        if let syn::Pat::Ident(ident) = &local.pat {
                            let var_name = ident.ident.to_string();
                            // Translate the init expression
                            if let Ok(z3_expr) = self.translate_expr_with_vars(&init.expr, vars) {
                                if z3_expr.as_int().is_some() || z3_expr.as_bool().is_some() {
                                    vars.insert(var_name, z3_expr);
                                }
                            }
                        }
                    }
                }
                Stmt::Expr(expr, Some(_)) => {
                    if let Expr::Assign(assign) = expr {
                        if let Expr::Path(path) = &*assign.left {
                            let var_name = path_to_string(&path.path);
                            if let Ok(z3_expr) = self.translate_expr_with_vars(&assign.right, vars)
                            {
                                if z3_expr.as_int().is_some() || z3_expr.as_bool().is_some() {
                                    vars.insert(var_name, z3_expr);
                                }
                            }
                        } else if let Expr::Index(index) = &*assign.left {
                            let _ = self.translate_expr_with_vars(&index.expr, vars);
                            let _ = self.translate_expr_with_vars(&index.index, vars);
                            if let Ok(val) = self.translate_expr_with_vars(&assign.right, vars) {
                                let base = expr_to_var_hint(&index.expr);
                                let idx = extract_int_literal(&index.index).unwrap_or(0);
                                let name = format!("{base}_after_{idx}");
                                vars.insert(name.clone(), val.clone());
                                // Update base var so return sees final value (hash[i]=x; hash)
                                if let Expr::Path(path) = &*index.expr {
                                    let base_name = path_to_string(&path.path);
                                    vars.insert(base_name, val);
                                }
                            }
                        }
                    } else if let Expr::Binary(bin) = expr {
                        if matches!(bin.op, syn::BinOp::AddAssign(_) | syn::BinOp::SubAssign(_)) {
                            if let Expr::Path(path) = &*bin.left {
                                let var_name = path_to_string(&path.path);
                                let left = vars.get(&var_name).cloned().unwrap_or_else(|| {
                                    Int::new_const(&self.ctx, z3::Symbol::String(var_name.clone()))
                                        .into()
                                });
                                if let Ok(right) = self.translate_expr_with_vars(&bin.right, vars) {
                                    if let (Some(left_int), Some(right_int)) =
                                        (left.as_int(), right.as_int())
                                    {
                                        let result = match bin.op {
                                            syn::BinOp::AddAssign(_) => {
                                                (left_int + right_int).into()
                                            }
                                            syn::BinOp::SubAssign(_) => {
                                                (left_int - right_int).into()
                                            }
                                            _ => continue,
                                        };
                                        vars.insert(var_name, result);
                                    }
                                }
                            }
                        }
                    } else if let Expr::ForLoop(fl) = expr {
                        let _ = self.translate_expr_with_vars(&fl.expr, vars);
                        let _ = self.translate_block_to_result_formula(&fl.body, vars);
                    } else if let Expr::While(while_expr) = expr {
                        let _ = self.translate_expr_with_vars(&while_expr.cond, vars);
                        let _ = self.translate_block_to_result_formula(&while_expr.body, vars);
                    } else if let Expr::Loop(loop_expr) = expr {
                        let _ = self.translate_block_to_result_formula(&loop_expr.body, vars);
                    } else if let Expr::If(if_expr) = expr {
                        if let Some((cond, result_formula)) =
                            self.translate_if_with_early_return(if_expr, vars)?
                        {
                            early_return_conditions.push((cond, result_formula));
                        }
                    } else if let Expr::Return(ret) = expr {
                        if let Some(return_expr) = &ret.expr {
                            if let Ok(z3_expr) = self.translate_expr_with_vars(return_expr, vars) {
                                let result_var = vars.get("result").ok_or_else(|| {
                                    TranslationError::UnsupportedExpression(
                                        "No result variable".to_string(),
                                    )
                                })?;
                                let eq_formula = if let Some(int_val) = z3_expr.as_int() {
                                    result_var.as_int().map(|r| r._eq(&int_val))
                                } else if let Some(bool_val) = z3_expr.as_bool() {
                                    result_var.as_bool().map(|r| r._eq(&bool_val))
                                } else {
                                    None
                                };
                                if let Some(eq) = eq_formula {
                                    return Ok(Some(eq));
                                }
                            }
                        }
                    }
                }
                Stmt::Expr(expr, None) => {
                    // Final expression (implicit return)
                    if let Ok(z3_expr) = self.translate_expr_with_vars(expr, vars) {
                        let result_var = vars.get("result").ok_or_else(|| {
                            TranslationError::UnsupportedExpression(
                                "No result variable".to_string(),
                            )
                        })?;
                        let eq_formula = if let Some(int_val) = z3_expr.as_int() {
                            result_var.as_int().map(|r| r._eq(&int_val))
                        } else if let Some(bool_val) = z3_expr.as_bool() {
                            result_var.as_bool().map(|r| r._eq(&bool_val))
                        } else {
                            None
                        };
                        if let Some(eq) = eq_formula {
                            if early_return_conditions.is_empty() {
                                return Ok(Some(eq));
                            } else {
                                let mut all_conditions = Vec::new();
                                let mut negated_conds = Vec::new();
                                for (cond, result_formula) in &early_return_conditions {
                                    all_conditions.push(cond.implies(result_formula));
                                    negated_conds.push(cond.not());
                                }
                                if !negated_conds.is_empty() {
                                    let refs: Vec<&z3::ast::Bool> = negated_conds.iter().collect();
                                    let no_early_return = Bool::and(&self.ctx, &refs);
                                    all_conditions.push(no_early_return.implies(&eq));
                                }
                                if !all_conditions.is_empty() {
                                    let refs: Vec<&z3::ast::Bool> = all_conditions.iter().collect();
                                    return Ok(Some(Bool::and(&self.ctx, &refs)));
                                }
                                return Ok(Some(eq));
                            }
                        }
                    }
                }
                Stmt::Item(_) => {
                    // Skip items (nested functions, etc.)
                }
                Stmt::Macro(_) => {
                    // Skip macros (debug_assert!, etc.)
                }
            }
        }

        // Check if the last statement is a return or expression
        if let Some(Stmt::Expr(expr, None)) = block.stmts.last() {
            return self.translate_return_expr(expr, vars);
        }

        // Handle case where there are only early returns (no final expression)
        if !early_return_conditions.is_empty() {
            let mut all_conditions = Vec::new();
            for (cond, result_formula) in &early_return_conditions {
                all_conditions.push(cond.implies(result_formula));
            }
            let refs: Vec<&z3::ast::Bool> = all_conditions.iter().collect();
            return Ok(Some(Bool::and(&self.ctx, &refs)));
        }

        // No clear return expression found
        Ok(None)
    }

    /// Handle if statement with potential early return
    /// Returns (condition, result_formula) if there's an early return
    fn translate_if_with_early_return<'a>(
        &'a self,
        if_expr: &syn::ExprIf,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<Option<(z3::ast::Bool<'a>, z3::ast::Bool<'a>)>, TranslationError> {
        // Translate condition
        let cond_z3 = self.translate_expr_with_vars(&if_expr.cond, vars)?;
        let cond_bool = match cond_z3.as_bool() {
            Some(b) => b,
            None => return Ok(None),
        };

        // Check if the then branch has an early return
        for stmt in &if_expr.then_branch.stmts {
            if let Stmt::Expr(Expr::Return(ret), _) = stmt {
                if let Some(return_expr) = &ret.expr {
                    // Ok(expr) with Int result: 1=Ok(false), 2=Ok(true)
                    if let Expr::Call(call) = &**return_expr {
                        if let Ok(name) = call_expr_to_name(&call.func) {
                            let base = name.split("::").last().unwrap_or(&name);
                            if base == "Ok" && call.args.len() == 1 {
                                let inner = self.translate_expr_with_vars(&call.args[0], vars)?;
                                if let Some(b) = inner.as_bool() {
                                    if let Some(r) = vars.get("result").and_then(|v| v.as_int()) {
                                        let one = Int::from_i64(&self.ctx, 1);
                                        let two = Int::from_i64(&self.ctx, 2);
                                        let encoded = b.ite(&two, &one);
                                        return Ok(Some((cond_bool, r._eq(&encoded))));
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(z3_expr) = self.translate_expr_with_vars(return_expr, vars) {
                        let result_var = vars.get("result").ok_or_else(|| {
                            TranslationError::UnsupportedExpression(
                                "No result variable".to_string(),
                            )
                        })?;
                        let eq = if let Some(int_val) = z3_expr.as_int() {
                            result_var.as_int().map(|r| r._eq(&int_val))
                        } else if let Some(bool_val) = z3_expr.as_bool() {
                            result_var.as_bool().map(|r| r._eq(&bool_val))
                        } else {
                            None
                        };
                        if let Some(eq_bool) = eq {
                            return Ok(Some((cond_bool, eq_bool)));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Translate a return expression to: result == <expr>
    fn translate_return_expr<'a>(
        &'a self,
        expr: &Expr,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<Option<z3::ast::Bool<'a>>, TranslationError> {
        match expr {
            Expr::Return(ret) => {
                if let Some(return_expr) = &ret.expr {
                    // Ok(expr) with Int result: 1=Ok(false), 2=Ok(true)
                    if let Expr::Call(call) = &**return_expr {
                        if let Ok(name) = call_expr_to_name(&call.func) {
                            let base = name.split("::").last().unwrap_or(&name);
                            if base == "Ok" && call.args.len() == 1 {
                                let inner = self.translate_expr_with_vars(&call.args[0], vars)?;
                                if let Some(b) = inner.as_bool() {
                                    if let Some(r) = vars.get("result").and_then(|v| v.as_int()) {
                                        let one = Int::from_i64(&self.ctx, 1);
                                        let two = Int::from_i64(&self.ctx, 2);
                                        let encoded = b.ite(&two, &one);
                                        return Ok(Some(r._eq(&encoded)));
                                    }
                                }
                            }
                        }
                    }
                    let z3_expr = self.translate_expr_with_vars(return_expr, vars)?;
                    let result_var = vars.get("result").ok_or_else(|| {
                        TranslationError::UnsupportedExpression("No result variable".to_string())
                    })?;
                    let eq = if let Some(int_val) = z3_expr.as_int() {
                        result_var.as_int().map(|r| r._eq(&int_val))
                    } else if let Some(bool_val) = z3_expr.as_bool() {
                        result_var.as_bool().map(|r| r._eq(&bool_val))
                    } else {
                        None
                    };
                    if let Some(eq_bool) = eq {
                        return Ok(Some(eq_bool));
                    }
                }
            }
            Expr::If(if_expr) => {
                // if condition { then_expr } else { else_expr }
                // Translates to: (condition => result == then_expr) && (!condition => result == else_expr)
                return self.translate_if_to_formula(if_expr, vars);
            }
            Expr::Call(call) => {
                // Ok(expr) with Int result: 1=Ok(false), 2=Ok(true)
                if let Ok(name) = call_expr_to_name(&call.func) {
                    let base = name.split("::").last().unwrap_or(&name);
                    if base == "Ok" && call.args.len() == 1 {
                        if let Some(result_var) = vars.get("result") {
                            if let Some(r) = result_var.as_int() {
                                let inner = self.translate_expr_with_vars(&call.args[0], vars)?;
                                if let Some(b) = inner.as_bool() {
                                    let one = Int::from_i64(&self.ctx, 1);
                                    let two = Int::from_i64(&self.ctx, 2);
                                    let encoded = b.ite(&two, &one);
                                    return Ok(Some(r._eq(&encoded)));
                                }
                            }
                        }
                    }
                }
                // Direct call (e.g. verify_script_with_context_full) - returns Int
                if let Ok(z3_expr) = self.translate_expr_with_vars(expr, vars) {
                    let result_var = vars.get("result").ok_or_else(|| {
                        TranslationError::UnsupportedExpression("No result variable".to_string())
                    })?;
                    let eq = if let Some(int_val) = z3_expr.as_int() {
                        result_var.as_int().map(|r| r._eq(&int_val))
                    } else if let Some(bool_val) = z3_expr.as_bool() {
                        result_var.as_bool().map(|r| r._eq(&bool_val))
                    } else {
                        None
                    };
                    if let Some(eq_bool) = eq {
                        return Ok(Some(eq_bool));
                    }
                }
            }
            _ => {
                // Direct expression (implicit return)
                if let Ok(z3_expr) = self.translate_expr_with_vars(expr, vars) {
                    let result_var = vars.get("result").ok_or_else(|| {
                        TranslationError::UnsupportedExpression("No result variable".to_string())
                    })?;
                    let eq = if let Some(int_val) = z3_expr.as_int() {
                        result_var.as_int().map(|r| r._eq(&int_val))
                    } else if let Some(bool_val) = z3_expr.as_bool() {
                        result_var.as_bool().map(|r| r._eq(&bool_val))
                    } else {
                        None
                    };
                    if let Some(eq_bool) = eq {
                        return Ok(Some(eq_bool));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Translate an if expression to: (cond => result == then) && (!cond => result == else)
    fn translate_if_to_formula<'a>(
        &'a self,
        if_expr: &syn::ExprIf,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<Option<z3::ast::Bool<'a>>, TranslationError> {
        // Translate condition
        let cond_z3 = self.translate_expr_with_vars(&if_expr.cond, vars)?;
        let cond_bool = cond_z3.as_bool().ok_or_else(|| {
            TranslationError::TypeError("If condition must be boolean".to_string())
        })?;

        // Translate then branch
        let then_formula = self.translate_block_to_result_formula(&if_expr.then_branch, vars)?;

        // Translate else branch (if present)
        let else_formula = if let Some((_, else_branch)) = &if_expr.else_branch {
            match &**else_branch {
                Expr::Block(block) => self.translate_block_to_result_formula(&block.block, vars)?,
                Expr::If(nested_if) => self.translate_if_to_formula(nested_if, vars)?,
                _ => {
                    let z3_expr = self.translate_expr_with_vars(else_branch, vars)?;
                    let result_var = vars.get("result").ok_or_else(|| {
                        TranslationError::UnsupportedExpression("No result variable".to_string())
                    })?;
                    result_var
                        .as_int()
                        .and_then(|r| z3_expr.as_int().map(|v| r._eq(&v)))
                        .or_else(|| {
                            result_var
                                .as_bool()
                                .and_then(|r| z3_expr.as_bool().map(|v| r._eq(&v)))
                        })
                }
            }
        } else {
            None
        };

        // Build the formula: (cond => then) && (!cond => else)
        match (then_formula, else_formula) {
            (Some(then_f), Some(else_f)) => {
                // (cond => then) && (!cond => else)
                let then_impl = cond_bool.implies(&then_f);
                let else_impl = cond_bool.not().implies(&else_f);
                Ok(Some(Bool::and(&self.ctx, &[&then_impl, &else_impl])))
            }
            (Some(then_f), None) => {
                // Only then branch matters (cond => then)
                Ok(Some(cond_bool.implies(&then_f)))
            }
            (None, Some(else_f)) => {
                // Only else branch matters (!cond => else)
                Ok(Some(cond_bool.not().implies(&else_f)))
            }
            (None, None) => Ok(None),
        }
    }

    /// Translate standalone if expression to Dynamic (Int or Bool) via ite
    fn translate_if_expr_to_dynamic<'a>(
        &'a self,
        if_expr: &syn::ExprIf,
        vars: &mut Z3VarMap<'a>,
    ) -> Result<z3::ast::Dynamic<'a>, TranslationError> {
        let cond_z3 = self.translate_expr_with_vars(&if_expr.cond, vars)?;
        let cond_bool = cond_z3.as_bool().ok_or_else(|| {
            TranslationError::TypeError("If condition must be boolean".to_string())
        })?;

        let then_z3 = match if_expr.then_branch.stmts.last() {
            Some(Stmt::Expr(expr, None)) => self.translate_expr_with_vars(expr, vars)?,
            _ => {
                return Err(TranslationError::UnsupportedExpression(
                    "If then branch: expected single expression".to_string(),
                ))
            }
        };

        let else_z3 = if let Some((_, else_branch)) = &if_expr.else_branch {
            match &**else_branch {
                Expr::Block(block) => {
                    if let Some(Stmt::Expr(expr, None)) = block.block.stmts.last() {
                        self.translate_expr_with_vars(expr, vars)?
                    } else {
                        return Err(TranslationError::UnsupportedExpression(
                            "If else block: expected single expression".to_string(),
                        ));
                    }
                }
                Expr::If(nested) => self.translate_if_expr_to_dynamic(nested, vars)?,
                other => self.translate_expr_with_vars(other, vars)?,
            }
        } else {
            return Err(TranslationError::UnsupportedExpression(
                "If without else not supported as expression".to_string(),
            ));
        };

        if let (Some(then_int), Some(else_int)) = (then_z3.as_int(), else_z3.as_int()) {
            return Ok(cond_bool.ite(&then_int, &else_int).into());
        }
        if let (Some(then_b), Some(else_b)) = (then_z3.as_bool(), else_z3.as_bool()) {
            return Ok(cond_bool.ite(&then_b, &else_b).into());
        }
        Err(TranslationError::TypeError(
            "If branches must both be Int or both Bool".to_string(),
        ))
    }
}

/// Resolve common Bitcoin consensus constants
/// Returns the constant value if known, None otherwise
fn resolve_constant(name: &str) -> Option<i64> {
    match name {
        // Economic constants (from blvm-consensus/src/constants.rs)
        "INITIAL_SUBSIDY" => Some(50_0000_0000), // 50 BTC in satoshis
        "MAX_MONEY" => Some(2_100_000_000_000_000), // 21M BTC in satoshis
        "HALVING_INTERVAL" => Some(210_000),
        "SATOSHIS_PER_BTC" => Some(100_000_000),

        // Transaction constants
        "MAX_BLOCK_SIZE" => Some(1_000_000), // 1MB
        "MAX_TX_SIZE" => Some(100_000),      // Conservative limit

        // Script constants
        "MAX_SCRIPT_SIZE" => Some(10_000),
        "MAX_STACK_SIZE" => Some(1000),

        _ => None,
    }
}

/// Check if return type is Result<T> or Option<Result<T>>.
/// For determinism, we use Int for result: 0=Err/None, 1=Ok(false)/Some(Err), 2=Ok(true)/Some(Ok(false)), 3=Some(Ok(true)).
pub fn returns_result_or_option_result(ty: &syn::Type) -> bool {
    if let syn::Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            if seg.ident == "Result" {
                return true;
            }
            if seg.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(t) = arg {
                            if returns_result_or_option_result(t) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Check if a type is bool or wraps bool (Result<bool>, Option<bool>).
/// Used to create result as Bool in Z3 instead of Int.
pub fn returns_bool_like(ty: &syn::Type) -> bool {
    if let syn::Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            if seg.ident == "bool" {
                return true;
            }
            if seg.ident == "Result" || seg.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(t) = arg {
                            if returns_bool_like(t) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Check if a type is unsigned
/// Handles both primitive types (u8, u16, u32, u64, u128, usize) and common type aliases
fn is_unsigned_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let type_name = segment.ident.to_string();

            // Check primitive unsigned types
            if type_name.starts_with('u')
                && (type_name == "u8"
                    || type_name == "u16"
                    || type_name == "u32"
                    || type_name == "u64"
                    || type_name == "u128"
                    || type_name == "usize")
            {
                return true;
            }

            // Check common type aliases used in Bitcoin consensus code
            // Natural = u64, Integer = i64 (from blvm-consensus/src/types.rs)
            if type_name == "Natural" {
                return true; // Natural is u64
            }
        }
    }
    false
}

/// Convert a path to a string representation
fn path_to_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|seg| seg.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Extract a variable name from a field expression (e.g. block.header.bits -> "block_header_bits")
fn field_expr_to_var_name(field: &syn::ExprField) -> String {
    let mut parts = vec![];
    collect_field_path(&field.base, &mut parts);
    parts.push(member_to_string(&field.member));
    parts.join("_")
}

fn collect_field_path(expr: &syn::Expr, parts: &mut Vec<String>) {
    match expr {
        syn::Expr::Field(f) => {
            collect_field_path(&f.base, parts);
            parts.push(member_to_string(&f.member));
        }
        syn::Expr::Path(p) => {
            parts.push(path_to_string(&p.path));
        }
        _ => {
            parts.push("_".to_string());
        }
    }
}

fn member_to_string(member: &syn::Member) -> String {
    match member {
        syn::Member::Named(ident) => ident.to_string(),
        syn::Member::Unnamed(index) => index.index.to_string(),
    }
}

/// Extract a variable name hint from an expression (for unique naming)
fn expr_to_var_hint(expr: &syn::Expr) -> String {
    match expr {
        syn::Expr::Path(p) => path_to_string(&p.path).replace("::", "_"),
        syn::Expr::Field(f) => field_expr_to_var_name(f),
        syn::Expr::MethodCall(m) => {
            let rcv = expr_to_var_hint(&m.receiver);
            if rcv == "x" {
                "m".to_string()
            } else {
                format!("{rcv}_rcv")
            }
        }
        _ => "x".to_string(),
    }
}

/// Result of parsing a let pattern: (inner var to bind, bool suffix, use as-is or negated).
/// - Some: (Some(inner), "is_some", true)
/// - None: (None, "is_some", false)
/// - Ok: (Some(inner), "is_ok", true)
/// - Err(e): (Some(e), "is_ok", false)
/// - Err(_): (None, "is_ok", false)
fn parse_let_option_result_pat(pat: &syn::Pat) -> Option<(Option<String>, &'static str, bool)> {
    match pat {
        syn::Pat::TupleStruct(ts) => {
            let seg = ts.path.segments.last()?;
            let variant = seg.ident.to_string();
            match variant.as_str() {
                "Some" if ts.elems.len() == 1 => {
                    if let syn::Pat::Ident(ident) = &ts.elems[0] {
                        return Some((Some(ident.ident.to_string()), "is_some", true));
                    }
                }
                "Ok" if ts.elems.len() == 1 => {
                    if let syn::Pat::Ident(ident) = &ts.elems[0] {
                        return Some((Some(ident.ident.to_string()), "is_ok", true));
                    }
                }
                "Err" if ts.elems.len() == 1 => {
                    if let syn::Pat::Ident(ident) = &ts.elems[0] {
                        return Some((Some(ident.ident.to_string()), "is_ok", false));
                    }
                    return Some((None, "is_ok", false));
                }
                _ => {}
            }
        }
        syn::Pat::Path(p) => {
            let seg = p.path.segments.last()?;
            if seg.ident == "None" {
                return Some((None, "is_some", false));
            }
        }
        _ => {}
    }
    None
}

/// Parse arm as Some(pat) => body or Ok(pat) => body. Returns (inner_var_name, body).
fn parse_some_ok_arm(arm: &syn::Arm) -> Option<(String, &syn::Expr)> {
    if let syn::Pat::TupleStruct(ts) = &arm.pat {
        let seg = ts.path.segments.last()?;
        let variant = seg.ident.to_string();
        if (variant == "Some" || variant == "Ok") && ts.elems.len() == 1 {
            if let syn::Pat::Ident(ident) = &ts.elems[0] {
                return Some((ident.ident.to_string(), &arm.body));
            }
        }
    }
    None
}

/// Parse arm as None => body or Err(_) => body. Returns body.
fn parse_none_err_arm(arm: &syn::Arm) -> Option<&syn::Expr> {
    match &arm.pat {
        syn::Pat::Path(p) => {
            let seg = p.path.segments.last()?;
            if seg.ident == "None" {
                return Some(&arm.body);
            }
        }
        syn::Pat::TupleStruct(ts) => {
            let seg = ts.path.segments.last()?;
            if seg.ident == "Err" && ts.elems.len() <= 1 {
                return Some(&arm.body);
            }
        }
        _ => {}
    }
    None
}

/// Extract integer literal from expression if it's a literal.
/// Handles decimal, hex (0x), octal (0o), and binary (0b).
fn extract_int_literal(expr: &syn::Expr) -> Option<i64> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Int(int_lit),
        ..
    }) = expr
    {
        if let Ok(v) = int_lit.base10_parse::<i64>() {
            return Some(v);
        }
        let s = int_lit.to_string();
        let (digits, radix) =
            if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                (rest, 16u32)
            } else if let Some(rest) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
                (rest, 8)
            } else if let Some(rest) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
                (rest, 2)
            } else {
                return None;
            };
        i64::from_str_radix(digits, radix).ok()
    } else {
        None
    }
}

/// Extract function name from call expression (Path or Path with segments)
fn call_expr_to_name(expr: &syn::Expr) -> Result<String, TranslationError> {
    match expr {
        syn::Expr::Path(p) => Ok(path_to_string(&p.path)),
        _ => Err(TranslationError::UnsupportedExpression(format!(
            "Call func: {expr:?}"
        ))),
    }
}

/// Bool-returning uninterpreted (Result<bool>, Option<bool>). Check before Int-returning.
fn known_bool_uninterpreted_function(name: &str) -> Option<(&'static str, Option<usize>)> {
    let base = name.split("::").last().unwrap_or(name);
    match base {
        "check_proof_of_work" => Some(("check_pow", Some(1))),
        "verify_tapscript_schnorr_signature" => Some(("verify_schnorr", None)),
        "is_standard_tx" => Some(("is_standard_tx", Some(1))),
        "replacement_checks" => Some(("replacement_checks", Some(4))),
        "eval_script" => Some(("eval_script", Some(4))),
        "eval_script_inner" => Some(("eval_script_inner", Some(4))),
        "eval_script_impl" => Some(("eval_script_impl", Some(4))),
        "verify_schnorr" => Some(("verify_schnorr", Some(3))),
        _ => None,
    }
}

/// Uninterpreted functions for determinism: f(args) so same inputs => same output (congruence).
/// Returns (Z3 function name, arity). Use None for arity to accept any arg count.
fn known_uninterpreted_function(name: &str) -> Option<(&'static str, Option<usize>)> {
    let base = name.split("::").last().unwrap_or(name);
    match base {
        "serialize_transaction" => Some(("serialize", Some(1))),
        "serialize_transaction_into" => Some(("serialize_into", Some(2))),
        "digest" => Some(("sha256_digest", Some(1))),
        "hash256" => Some(("hash256", Some(1))),
        "to_le_bytes" => Some(("to_le_bytes", Some(1))),
        "with_capacity" => Some(("vec_with_capacity", Some(1))),
        "get_locktime_type" => Some(("locktime_type", Some(1))),
        "sha256_hash" => Some(("hash256", Some(1))),
        "double_sha256_hash" => Some(("hash256", Some(1))),
        "calculate_tx_id" => Some(("hash256", Some(1))),
        "serialize_header" => Some(("serialize_header", Some(1))),
        "total_supply" => Some(("total_supply", Some(1))),
        "calculate_transaction_size" => Some(("tx_size", Some(1))),
        "expand_target" => Some(("expand_target", Some(1))),
        "from_bytes" => Some(("from_bytes", Some(1))),
        "apply_transaction" => Some(("apply_tx", Some(3))),
        "apply_transaction_with_id" => Some(("apply_tx_with_id", None)),
        "check_tx_inputs" => Some(("check_tx_inputs", Some(3))),
        "check_proof_of_work" => Some(("check_pow", Some(1))),
        "eval_script" => Some(("eval_script", Some(4))),
        "get_next_work_required" => Some(("get_next_work", Some(2))),
        "get_next_work_required_corrected" => Some(("get_next_work", Some(2))),
        "get_next_work_required_internal" => Some(("get_next_work_internal", None)),
        "calculate_merkle_root" => Some(("merkle_root", Some(1))),
        "calculate_merkle_root_from_tx_ids" => Some(("merkle_root_ids", Some(1))),
        "merkle_tree_from_hashes" => Some(("merkle_tree", Some(1))),
        // Flexible arity (use call's arg count).
        // connect_block is the single public API (takes BlockValidationContext); no separate connect_block_ctx.
        "connect_block" => Some(("connect_block", None)),
        "connect_block_with_context" => Some(("connect_block", None)),
        "connect_block_ibd" => Some(("connect_block_ibd", None)),
        "connect_block_inner" => Some(("connect_block_inner", None)),
        "validate_block" => Some(("validate_block", None)),
        "validate_block_with_time_context" => Some(("validate_block_ctx", None)),
        "validate_transaction" => Some(("validate_tx", None)),
        "validate_block_header" => Some(("validate_header", None)),
        "check_tx_inputs_with_utxos" => Some(("check_tx_inputs", None)),
        "calculate_transaction_sighash" => Some(("sighash", None)),
        "calculate_transaction_sighash_single_input" => Some(("sighash_single", None)),
        "calculate_transaction_sighash_with_script_code" => Some(("sighash_script", None)),
        "build_time_context" => Some(("build_time_context", None)),
        _ => None,
    }
}

/// Known functions that return Int/u64/i64. Returns fresh var name for the call result.
/// Multiple calls to same function share a var (conservative; models "some value").
fn known_int_returning_function(name: &str) -> Option<String> {
    let base = name.split("::").last().unwrap_or(name);
    match base {
        "get_block_subsidy"
        | "get_next_work_required"
        | "expand_target"
        | "compress_target"
        | "difficulty_from_bits" => Some(format!("call_{base}_result")),
        "calculate_merkle_root" | "calculate_block_hash" => Some(format!("call_{base}_result")),
        "extract_sequence_locktime_value" | "ExtractSequenceLocktimeValue" => {
            Some("call_extract_sequence_locktime_value_result".to_string())
        }
        "get_median_time_past" | "GetMedianTimePast" => {
            Some("call_get_median_time_past_result".to_string())
        }
        "u128" | "from_le_bytes" => None, // type conversions, not direct
        _ => None,
    }
}

/// Known functions that return bool. Returns fresh var name for the call result.
fn known_bool_returning_function(name: &str) -> Option<String> {
    let base = name.split("::").last().unwrap_or(name);
    match base {
        "is_zero_hash" => Some("call_is_zero_hash_result".to_string()),
        "locktime_types_match" => Some("call_locktime_types_match_result".to_string()),
        "is_standard_script" => Some("call_is_standard_script_result".to_string()),
        _ => None,
    }
}

/// Translation errors
#[derive(Debug, Clone)]
pub enum TranslationError {
    UnsupportedExpression(String),
    UnsupportedLiteral(String),
    UnsupportedOperator(String),
    TypeError(String),
    ParseError(String),
}

impl std::fmt::Display for TranslationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranslationError::UnsupportedExpression(msg) => {
                write!(f, "Unsupported expression: {msg}")
            }
            TranslationError::UnsupportedLiteral(msg) => write!(f, "Unsupported literal: {msg}"),
            TranslationError::UnsupportedOperator(msg) => {
                write!(f, "Unsupported operator: {msg}")
            }
            TranslationError::TypeError(msg) => write!(f, "Type error: {msg}"),
            TranslationError::ParseError(msg) => write!(f, "Parse error: {msg}"),
        }
    }
}

impl std::error::Error for TranslationError {}

#[cfg(not(feature = "z3"))]
/// Stub implementation when Z3 feature is disabled
pub struct Z3Translator;

#[cfg(not(feature = "z3"))]
impl Z3Translator {
    pub fn new() -> Self {
        Z3Translator
    }

    pub fn translate_contract(&mut self, _contract: &Contract) -> Result<(), String> {
        Err("Z3 feature not enabled".to_string())
    }
}
