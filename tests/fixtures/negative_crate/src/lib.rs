//! Negative test fixture: deliberately wrong implementation.
//! Verification should FAIL for get_block_subsidy (returns -1, violates result >= 0).

use blvm_spec_lock::spec_locked;

const INITIAL_SUBSIDY: i64 = 50_0000_0000;
const HALVING_INTERVAL: i64 = 210_000;

/// Intentionally wrong: returns -1 for height 0, violating result >= 0.
#[spec_locked("6.1")]
#[blvm_spec_lock::requires(height >= 0)]
#[blvm_spec_lock::ensures(result >= 0)]
#[blvm_spec_lock::ensures(result <= INITIAL_SUBSIDY)]
pub fn get_block_subsidy(height: i64) -> i64 {
    // WRONG: should return INITIAL_SUBSIDY for height 0
    -1
}
