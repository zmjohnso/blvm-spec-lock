# Spec Wording for Parseability

Guidance for writing Orange Paper conditions that blvm-spec-lock can parse and verify. The condition parser (`src/parser/condition.rs`) and lexer (`src/parser/lexer.rs`) extract parseable Rust expressions from spec-derived contracts.

## Principles

1. **Mathematical over descriptive** – Prefer formal notation over prose.
2. **Standard patterns** – Use patterns the condition parser recognizes.
3. **Avoid noise** – Keep activation heights, URLs, and references in separate fields.
4. **Bitwise vs logical** – Use `\land` for bitwise AND (e.g. `seq \land 0x00400000`); the parser maps it to Rust `&`.

## Supported Patterns

### Result types

| Spec pattern | Parseable form | Meaning |
|--------------|----------------|---------|
| `result(...) \in {valid, invalid}` | `(result == true \|\| result == false)` | Bool |
| `result(...) \in {true, false}` | Same | Bool |
| `result(...) \in {accepted, rejected}` | Same | Bool |
| `result(...) \in {success, failure}` | Same | Bool |
| `result(...) \in \mathbb{N}` | `result >= 0` | Non-negative int |
| `result(...) \in \mathbb{Z}` | `true` | Any int |
| `result(...) \in {Some}(\mathcal{UC}), None}` | `true` | Option |

### Result equality (determinism)

| Spec pattern | Meaning |
|--------------|---------|
| `result(a) == result(b)` | Same inputs → same outputs |
| `result(a) \iff result(b)` | Same |

### Domain-specific

| Spec pattern | Parseable form |
|--------------|----------------|
| `result(seq) extracts lower N bits` | `result >= 0 && result < 2^N` |
| `result(seq) extracts bit N` | `(result == true \|\| result == false)` (bool-returning) |
| `requires t \in [0, 1]` | `t >= 0 && t <= 1` |

### Implication

| Spec pattern | Parseable form |
|--------------|----------------|
| `A => B` | Conclusion `B` |
| `result(...) == true \iff (condition)` | Condition |

## Avoid

- **Extra braces** – `{valid}, invalid}}` instead of `{valid, invalid}` (parser normalizes, but spec should be clean).
- **Mixed prose** – Prefer `result \in \mathbb{N}` over "result is a natural number."
- **Activation heights in conditions** – Put e.g. `227,836; testnet: 211,111` in a separate field, not in the contract body.
- **URLs in conditions** – Put references in comments or metadata.

## Descriptive text → math

When prose is needed, use phrases the parser recognizes:

- "Same inputs yield same hash" → determinism
- "validates X", "enforces Y", "uses Z for domain separation" → invariants
- "extracts lower N bits" → `result >= 0 && result < 2^N`
- "extracts bit N" → `(result == true \|\| result == false)` for bool-returning functions

## LaTeX in Orange Paper parser

The Orange Paper parser (`src/parser/orange_paper.rs`) translates properties via `translate_property_to_rust`:

- `\land` → `&` (bitwise AND)
- `≥` → `>=`, `≤` → `<=`, `≠` → `!=`
- `\text{FunctionName}(args)` → `result`
- `\iff` → implication; conclusion is used for ensures

## Implemented

- **Determinism verification** – Two-run Z3 for `result(a) == result(b)`.
- **Option/Result** – `is_some`, `is_none`, `is_ok`, `is_err`, `unwrap`, `match` in Z3.
- **Complex enums** – `\in {AllLegacy, All, None, Single}` → `(result == 0 || result == 1 || ...)`.

## Related

- `SPEC_AS_SOURCE_OF_TRUTH.md` – How spec-derived contracts flow to verification
- `SPEC_LOCK_COVERAGE.md` – Verification status and parseable %
- `cargo spec-lock coverage --spec-path ...` – Theorems → contracts → parseable %
- `cargo spec-lock check-drift --spec-path ...` – Unparseable contracts, missing impls
