//! Fixture: #[spec_locked] with no explicit section leaves `section` unset in the verifier AST.
//! Verification must treat this as missing contracts linkage (NoContracts gate under normal exit rules).

use blvm_spec_lock::spec_locked;

#[spec_locked]
pub fn bare_missing_section_attr() -> i32 {
    42
}
