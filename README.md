# BLVM Spec Lock

[![crates.io](https://img.shields.io/crates/v/blvm-spec-lock.svg)](https://crates.io/crates/blvm-spec-lock)
[![docs.rs](https://docs.rs/blvm-spec-lock/badge.svg)](https://docs.rs/blvm-spec-lock)
[![CI](https://github.com/BTCDecoded/blvm-spec-lock/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/BTCDecoded/blvm-spec-lock/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

Purpose-built formal verification tool for Bitcoin Commons.

**Locking mechanism**: See [docs/LOCKING_MECHANISM.md](docs/LOCKING_MECHANISM.md) for the full lifecycle (discover → enrich → verify), attribute syntax, and status semantics. **Spec wording**: See [SPEC_WORDING.md](SPEC_WORDING.md) for parseable condition patterns. **How to annotate**: See [docs/ANNOTATION_GUIDE.md](docs/ANNOTATION_GUIDE.md) for adding `#[spec_locked]` to new functions.

## Overview

BLVM Spec Lock provides formal verification for Bitcoin consensus code by:
- Linking Rust functions to Orange Paper specifications via `#[spec_locked]` attributes
- Verifying contracts (`#[requires]` and `#[ensures]`) using static analysis and Z3
- Providing a `cargo test`-like CLI experience

## Installation

```bash
cd blvm-spec-lock
cargo build --release --bin cargo-spec-lock
```

The binary will be at `target/release/cargo-spec-lock`.

To use as a cargo subcommand, create a symlink:
```bash
ln -s target/release/cargo-spec-lock ~/.cargo/bin/cargo-spec-lock
```

## Usage

### Basic Verification

```bash
# Verify all functions with #[spec_locked]
cargo spec-lock verify

# With Orange Paper: derive contracts from spec (required for 0 no-contracts)
cargo spec-lock verify --spec-path path/to/THE_ORANGE_PAPER.md

# Strict mode: fail on partial or no-contracts (for CI gates)
cargo spec-lock verify --strict

# Verify specific file
cargo spec-lock verify src/economic.rs

# Verify by subsystem
cargo spec-lock verify --subsystem economic

# Verify by function name
cargo spec-lock verify --name get_block_subsidy

# Verify by Orange Paper section
cargo spec-lock verify --section 6.1

# Lock status summary (no verification)
cargo spec-lock summary --crate-path .
cargo spec-lock summary --crate-path . --spec-path path/to/THE_ORANGE_PAPER.md
```

### Output Formats

```bash
# Human-readable (default)
cargo spec-lock verify

# JSON
cargo spec-lock verify --format json

# JUnit XML (for CI)
cargo spec-lock verify --format junit
```

## Writing Contracts

```rust
use blvm_spec_lock::spec_locked;

#[spec_locked("6.1")]
#[requires(height >= 0)]
#[ensures(result >= 0)]
#[ensures(result <= MAX_SUBSIDY)]
pub fn get_block_subsidy(height: u64) -> i64 {
    // Implementation...
}
```

### Related Projects (blvm-spec-lock sibling to blvm-consensus, blvm-spec)

From `blvm-spec-lock` directory:

```bash
# Verify blvm-consensus (140 functions)
cargo run --bin cargo-spec-lock -- verify \
  --spec-path ../blvm-spec/THE_ORANGE_PAPER.md \
  --crate-path ../blvm-consensus

# Verify blvm-node (Dandelion 10.6)
cargo run --bin cargo-spec-lock -- verify \
  --spec-path ../blvm-spec/THE_ORANGE_PAPER.md \
  --crate-path ../blvm-node
```

With Z3: add `--features z3` to the `cargo run` command.

## Validation

- **Negative test**: `cargo test wrong_implementation_fails` — wrong impl must fail verification.
- **Spec coverage**: `cargo spec-lock coverage --spec-path ...` — theorems → contracts → parseable %.
- **Drift**: `cargo spec-lock check-drift --spec-path ...` — unparseable contracts, missing impls. **`--scoped-unparseables`** limits unparseable failures to Orange Paper sections under the crate’s **`#[spec_locked("…")]`** prefixes.

## Spec Wording

Orange Paper conditions must be parseable for verification. See **[SPEC_WORDING.md](SPEC_WORDING.md)** for:
- Supported patterns (`\in {true, false}`, `extracts lower N bits`, etc.)
- LaTeX → Rust translation (`\land` → `&`, `≥` → `>=`)
- What to avoid (activation heights in conditions, mixed prose)

To add `#[spec_locked]` to new functions, see **[docs/ANNOTATION_GUIDE.md](docs/ANNOTATION_GUIDE.md)**.

## Features

- **Function Discovery**: Automatically finds all `#[spec_locked]` functions
- **Spec-derived Contracts**: Orange Paper parser + lexer extract parseable conditions (LaTeX, `\text{}`, `\geq`, etc.)
- **Contract Parsing**: Extracts `#[requires]` and `#[ensures]` attributes
- **Static Checking**: Fast Rust-based checks for simple properties
- **Z3 Verification**: Full SMT solving for complex properties (requires `--features z3`)
- **Flexible Filtering**: By file, subsystem, name, or Orange Paper section
- **Multiple Output Formats**: Human-readable, JSON, JUnit XML, Markdown

## Z3 Support

Z3 verification requires the `z3` feature and system dependencies:

### Arch Linux

```bash
# Install Z3, LLVM, LLVM libs, and clang (required for bindgen)
sudo pacman -S z3 llvm llvm-libs clang

# Build with Z3 feature
cargo build --features z3 --bin cargo-spec-lock

# Run verification
cargo run --features z3 --bin cargo-spec-lock -- verify
```

**Important:** You need **both** `llvm` and `llvm-libs` packages. The `llvm` package provides static libraries, while `llvm-libs` provides the shared libraries (`.so` files) that bindgen needs.

**Note:** Ensure LLVM and llvm-libs versions match your clang version. If you have clang 21.x, you need llvm 21.x and llvm-libs 21.x. If versions don't match:
```bash
sudo pacman -Syu llvm llvm-libs clang  # Update all to matching versions
```

### Other Linux Distributions

For Debian/Ubuntu:
```bash
sudo apt-get install libz3-dev libclang-dev
cargo build --features z3 --bin cargo-spec-lock
```

For other distributions, install:
- Z3 development libraries
- LLVM and clang (matching versions)
- libclang development headers

