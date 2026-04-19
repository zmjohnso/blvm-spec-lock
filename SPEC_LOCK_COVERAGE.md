# blvm-spec-lock Coverage

This document describes blvm-spec-lock verification status across blvm-consensus and blvm-node.

**Scope**: The Orange Paper focuses on **consensus only**. RPC, rate limiters, and node-internal design (beyond wire format) are out of scope. See [CONSENSUS_SPEC_FOCUS_PLAN.md](../docs/CONSENSUS_SPEC_FOCUS_PLAN.md).

## Actual spec_locked Count (as of plan completion)

| Crate | Functions with `#[spec_locked]` |
|-------|---------------------------------|
| **blvm-consensus** | **166** |
| **blvm-node** | **8** |
| **blvm-protocol** | **6** (11.4 UTXO commitments + 13.4 peer consensus) |
| **Total** | **180** |

All point to valid Orange Paper sections. With `--spec-path`, all 140 discovered functions receive spec-derived contracts (0 no-contracts). No sections reference removed content (8.2.2 serialization, 13.3.2 serialization theorems).

## Locked Sections by Orange Paper

All 148 consensus functions map to these spec sections (each section may have multiple functions):

| Section | Modules |
|---------|---------|
| 5.1, 5.1.1 | transaction_hash (incl. compute_legacy_sighash_*, batch_*), block, transaction, mempool, script/mod, lib |
| 5.2, 5.2.1, 5.2.2, 5.2.5 | script/mod, script/signature, sigop, block (calculate_base_script_flags_for_block, calculate_script_flags_for_block, add_per_tx_script_flags), lib |
| 5.3, 5.3.1 | block, lib |
| 5.4.1–5.4.8 | bip_validation, bip119, bip348, locktime, script/mod |
| 5.5 | block, locktime, sequence_locks, bip113 |
| 6.1, 6.2, 6.3, 6.5 | economic, lib |
| 7.1, 7.2 | pow, mining (expand_target, calculate_block_hash), lib |
| 8.4.1 | mining (incl. merkle_tree_from_hashes), block |
| 9.1, 9.2, 9.3 | mempool, lib |
| 10.6 | blvm-node dandelion |
| 11.1.1–11.1.9 | witness, segwit (weight, vsize, block weight, witness structure, extraction, merkle root, commitment, is_segwit, validate_block), transaction_hash, lib |
| 11.2.1–11.2.8 | witness, taproot (script validation, key ops, script path, witness structure, transaction validation, sig hashes), sigop, lib |
| 11.3, 11.3.1 | reorganization (incl. find_common_ancestor, disconnect_block, calculate_chain_work), lib |
| 12.1, 12.2, 12.3, 12.4 | mining (incl. create_coinbase_transaction), lib |

## Maximum Consensus Coverage

**Current status (verified)**: All `#[spec_locked]` functions in blvm-consensus are fully covered:

| Metric | Value |
|--------|-------|
| **Z3 verified** | 162 passed, 0 failed |
| **Parseable** | 100% (412/412 contracts) |
| **Missing from spec** | 0 |
| **Unparseable** | 0 |
| **Drift** | 0 |

**Remaining gap**: ~29 Orange Paper functions not yet annotated (serialization, network protocol, internal helpers). SegWit/Taproot weight/merkle/witness helpers, P2SH redeem extraction, witness sigops, coinbase creation, block hash, difficulty expansion, disconnect/chain-work are now locked. For *currently* annotated functions, we have maximum coverage.

## Summary

| Area | Orange Paper | blvm-spec-lock Status |
|------|--------------|------------------------|
| **Protocol (10.1)** | 10.1 | **Covered** – 2 functions (parse_message, calculate_checksum) |
| **Handshake (10.2)** | 10.2 | **Covered** – 1 function (handle_version_received) |
| **Dandelion** | 10.6 | **Covered** – 5 functions in blvm-node |
| **chainstate** | 5.3 | **GAP** – Node ChainState; consensus 5.3 covered in blvm-consensus |
| **cryptographic** | 2.1, 2.2 | **Implied** – Hash primitives (SHA256) implied by spec; SafeAdd/SafeSub (2.2.2) for overflow safety |
| **utxostore** | 5.3.1 | **GAP** – Node UtxoStore; consensus 5.3.1 in blvm-consensus |
| **mempool** | 9.1–9.3 | **Partial** – Consensus mempool in blvm-consensus; node MempoolManager not |
| **rpc** | — | **Out of scope** – RPC is node API, not consensus. Do not add. |
| **protocol** | 10.1 | **Covered** – 2 functions (parse_message, calculate_checksum) |
| **rate_limiter** | — | **Out of scope** – DoS mitigation, not consensus. Do not add. |
| **state_machine** | 10.2 | **Covered** – 1 function (handle_version_received) |

## Dandelion (10.6) – Covered

Orange Paper Section 10.6 defines **Implementation Invariants (BLVM Specification Lock Verified)**:

1. **No Premature Broadcast**: `phase = Stem ⟹ broadcast_count(tx) = 0`
2. **Bounded Stem Length**: `stem_hops(tx) ≤ max_stem_hops`
3. **Timeout Enforcement**: `elapsed_time(tx) > stem_timeout ⟹ phase(tx) = Fluff`
4. **Single Stem State**: `|stem_states(tx)| ≤ 1`
5. **Eventual Fluff**: `∃ t: phase_at_time(tx, t) = Fluff`

blvm-node `dandelion.rs` has `#[spec_locked("10.6")]` on the key functions.

## Gaps and Recommendations

### Can Add spec_locked (Orange Paper exists)

- **Protocol (10.1)**: ✅ Done – 10.1.1 Message Header Parsing theorems added; parse_message, calculate_checksum annotated.
- **State machine (10.2)**: ✅ Done – 10.2.1 Handshake Invariants added; handle_version_received annotated.

### Out of Scope (Consensus-Only Spec)

- **RPC**: Node API, not consensus. Document in node docs if needed.
- **Rate limiter**: DoS mitigation, not consensus. Document in node docs if needed.

### Node vs Consensus

- **Chainstate, UtxoStore, MempoolManager**: These are node implementations. Consensus equivalents (5.3, 5.3.1, 9.x) are verified in blvm-consensus. Node-layer proofs would require new Orange Paper sections that specify node-specific invariants.

## Verification Commands

**Full Z3 verification is the default.** Run from `blvm-spec-lock` (no root workspace):

```bash
cd blvm-spec-lock
# blvm-consensus (166 functions)
cargo run --bin cargo-spec-lock -- verify --strict \
  --spec-path ../blvm-spec/PROTOCOL.md --spec-path ../blvm-spec/ARCHITECTURE.md \
  --crate-path ../blvm-consensus

# blvm-node Dandelion
cargo run --bin cargo-spec-lock -- verify --strict \
  --spec-path ../blvm-spec/PROTOCOL.md --spec-path ../blvm-spec/ARCHITECTURE.md \
  --crate-path ../blvm-node

# blvm-protocol UTXO commitments (11.4) and peer consensus (13.4)
cargo run --bin cargo-spec-lock -- verify --strict \
  --spec-path ../blvm-spec/PROTOCOL.md --spec-path ../blvm-spec/ARCHITECTURE.md \
  --crate-path ../blvm-protocol --section 11.4 --section 13.4
```

`--strict` fails on any partial; Z3 is default. Use `--no-default-features` only if you cannot build with libclang.

See `SPEC_AS_SOURCE_OF_TRUTH.md` for architecture details. For parseable condition patterns, see [SPEC_WORDING.md](SPEC_WORDING.md).

## Z3 Verification (default)

| Status | Count | Notes |
|--------|-------|-------|
| Passed | 162 | Fully verified |
| Failed | 0 | All gaps filled |
| No-contracts | 0 | All functions have spec-derived contracts |
| Parseable | **100%** | 412/412 contracts |
| Missing from spec | 0 | spec_enrich runs before drift check |

## Plan to Complete All Spec Locks

**Goal**: Every `#[spec_locked]` function is either verified by Z3 or has a documented reason why verification is not yet possible.

### Root Causes of Failures

1. **Body translation fails** (calls, complex control flow, unsupported types) → Z3 returns Unknown
2. **Contract translation fails** (complex expressions, unsupported types) → Z3 returns Error
3. **Z3 finds counterexample** (uninterpreted shift ops, constant resolution, spec/impl mismatch) → Z3 returns Failed

### Work to Complete

| Category | Action |
|----------|--------|
| **Z3 translator** | ✅ Option/Result: `is_some`, `is_none`, `is_ok`, `is_err`, `unwrap`, `match` on Some/None and Ok/Err; `Expr::Block` |
| **Shift modeling** | Replace uninterpreted `shr`/`shl` with concrete semantics or stronger axioms for Bitcoin formulas |
| **Contract alignment** | Review contracts that fail: ensure spec-derived conditions match implementation semantics |
| **Spec content** | Add parseable Properties for sections that need simpler contracts (e.g. `result >= 0` for u64 returns). Use [SPEC_WORDING.md](SPEC_WORDING.md) patterns. |

## Validation Improvements

- **Negative tests**: `cargo test wrong_implementation_fails` — wrong impl (e.g. `get_block_subsidy` returning -1) must fail verification.
- **Spec coverage**: `cargo spec-lock coverage --spec-path ...` — theorems → contracts → parseable % (100%, 412/412).
- **Drift detection**: `cargo spec-lock check-drift --spec-path ...` — unparseable spec contracts, missing-from-spec, auto-inferred.
- **Lexer**: `∀h ∈ ℕ: P(h)` strips quantifier → `P(h)` for parsing.
