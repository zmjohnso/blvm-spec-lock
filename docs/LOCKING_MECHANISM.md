# The Spec Lock Mechanism

This document describes how blvm-spec-lock links Rust code to the Orange Paper specification and verifies that implementations satisfy their contracts.

## What Is Locking?

**Locking** is the process of binding a Rust function to a specification section and verifying that the implementation satisfies the specified properties. A **locked** function has:

1. **Explicit linkage** — `#[spec_locked("section")]` points to an Orange Paper section
2. **Derived contracts** — Pre/postconditions extracted from the spec (when `--spec-path` is used)
3. **Verification** — Static analysis and/or Z3 SMT solving to check contracts hold

Locking prevents specification drift: if the spec changes, verification fails until the implementation is updated (or the spec is corrected).

## Lifecycle

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. DISCOVER                                                                 │
│    Walk crate sources, find #[spec_locked("X.Y")] on functions and impls   │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. ENRICH (when --spec-path provided)                                        │
│    Parse Orange Paper, extract FunctionName + Properties / Invariants        │
│    Match by name (PascalCase ↔ snake_case), fallback to section first fn   │
│    Replace manual #[requires]/#[ensures] with spec-derived contracts       │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. TRANSLATE                                                                 │
│    Convert contracts to Z3 SMT (or static checker)                          │
│    LaTeX → Rust: \land→&, ≥→>=, \text{Fn}(args)→result                       │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. VERIFY                                                                   │
│    Run Z3 (or static checker) on each contract                              │
│    Emit: Passed | Failed | Partial | NoContracts                            │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Attribute Syntax

```rust
use blvm_spec_lock::spec_locked;

// Minimal: section only, function name inferred from Rust
#[spec_locked("6.1")]
pub fn get_block_subsidy(height: u64) -> i64 { ... }

// Explicit function name (when spec uses different naming)
#[spec_locked("6.1", "GetBlockSubsidy")]
pub fn get_block_subsidy(height: u64) -> i64 { ... }

// Combined format
#[spec_locked("6.1::GetBlockSubsidy")]
pub fn get_block_subsidy(height: u64) -> i64 { ... }

// Named parameters
#[spec_locked(section = "6.1", function = "GetBlockSubsidy")]
pub fn get_block_subsidy(height: u64) -> i64 { ... }

// Auto-infer section from function name (searches entire spec)
#[spec_locked]
pub fn get_block_subsidy(height: u64) -> i64 { ... }
```

## Contract Types

| Type | Meaning | Example |
|------|---------|---------|
| **requires** | Precondition — must hold before the function runs | `height >= 0` |
| **ensures** | Postcondition — must hold after the function returns | `result >= 0 && result <= INITIAL_SUBSIDY` |

When `--spec-path` is provided, contracts come **only** from the spec. Manual `#[requires]`/`#[ensures]` are ignored. Without `--spec-path`, only manual attributes are used.

## Verification Modes

| Mode | When | What it does |
|------|------|--------------|
| **Static** | Default (no Z3) | Simple expression checks; most contracts reported as Partial |
| **Z3** | `--features z3` | Full SMT solving; contracts either Passed or Failed |

Use Z3 for full verification. Static mode is a fallback when Z3 is not available.

## Status Semantics

| Status | Meaning |
|--------|---------|
| **Passed** | All contracts verified (Z3 proved) |
| **Failed** | Z3 found a counterexample — implementation violates spec |
| **Partial** | Some contracts verified, others not (e.g. Z3 Unknown, unsupported expr) |
| **NoContracts** | No spec-derived contracts; add Properties to Orange Paper or manual attributes |

**Strict mode** (`--strict`): Fails the run on any Partial or NoContracts. Use in CI to enforce full lock coverage.

## Section Matching

The spec parser matches functions using this order:

1. **Exact name** in section (e.g. `VerifyScript` ↔ `verify_script`)
2. **Name anywhere** in spec
3. **First function** in section (one spec function applies to all impls in that section)
4. **Parent sections** (5.4.1 → 5.4 → 5) when granularity differs
5. **Implementation Invariants** (`*`) when section has invariants but no named function

See [SPEC_AS_SOURCE_OF_TRUTH.md](../SPEC_AS_SOURCE_OF_TRUTH.md) for spec format details.

## Spec Formats

### FunctionName + Properties

```
**GetBlockSubsidy**: $\mathbb{N} \rightarrow \mathbb{Z}$

**Properties**:
- Non-negative: $\text{GetBlockSubsidy}(h) \geq 0$
- Bounded: $\text{GetBlockSubsidy}(h) \leq \text{INITIAL\_SUBSIDY}$
```

### Implementation Invariants

For algorithm sections (e.g. Dandelion 10.6):

```
**Implementation Invariants (BLVM Specification Lock Verified)**:
1. **No Premature Broadcast**: $\text{phase} = \text{Stem} \implies \text{broadcast\_count}(tx) = 0$
2. **Bounded Stem Length**: $\text{stem\_hops}(tx) \leq \text{max\_stem\_hops}$
```

Any `#[spec_locked("section")]` in that section receives these contracts.

## Parseable Patterns

Orange Paper conditions must use patterns the condition parser recognizes. See [SPEC_WORDING.md](../SPEC_WORDING.md) for:

- `result \in \mathbb{N}` → `result >= 0`
- `\land` → `&` (bitwise AND)
- `extracts lower N bits` → `result >= 0 && result < 2^N`
- What to avoid (activation heights in conditions, mixed prose)

## Lock Summary

Use `cargo spec-lock summary` to see lock status without running full verification:

```bash
# Quick overview: functions, sections, enrichment
cargo spec-lock summary --crate-path ../blvm-consensus

# With spec: also shows how many received spec-derived contracts
cargo spec-lock summary --crate-path ../blvm-consensus --spec-path ../blvm-spec/THE_ORANGE_PAPER.md

# Badge format: one-line markdown for README
cargo spec-lock summary --crate-path . --format badge
# Output: [![spec-lock](https://img.shields.io/badge/spec--lock-138%20locked-brightgreen)](#)
```

## CI Integration

```bash
# Basic verification (fail on any Failed)
cargo spec-lock verify --spec-path ../blvm-spec/THE_ORANGE_PAPER.md --crate-path .

# Strict: fail on Partial or NoContracts
cargo spec-lock verify --spec-path ... --crate-path . --strict

# JUnit for CI reporting
cargo spec-lock verify --spec-path ... --crate-path . --format junit > spec-lock.xml
```

## Lock Coverage

| Crate | Locked | Notes |
|-------|--------|-------|
| blvm-consensus | 148 | 100% Z3 verified |
| blvm-node | 5 | Dandelion 10.6 |
| blvm-protocol | 3 | UTXO commitments 11.4 |

See [SPEC_LOCK_COVERAGE.md](../SPEC_LOCK_COVERAGE.md) for full status.

## Related

- [SPEC_AS_SOURCE_OF_TRUTH.md](../SPEC_AS_SOURCE_OF_TRUTH.md) — Spec-derived contract flow
- [SPEC_WORDING.md](../SPEC_WORDING.md) — Parseable condition patterns
- [ANNOTATION_GUIDE.md](ANNOTATION_GUIDE.md) — How to add `#[spec_locked]`
- [SPEC_LOCK_COVERAGE.md](../SPEC_LOCK_COVERAGE.md) — Verification status
