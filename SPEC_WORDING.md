# Spec Wording for Parseability

Guidance for writing Orange Paper conditions that blvm-spec-lock can parse and verify. The condition parser (`src/parser/condition.rs`) and lexer (`src/parser/lexer.rs`) extract parseable Rust expressions from spec-derived contracts.

## Principles

1. **Mathematical over descriptive** – Prefer formal notation over prose.
2. **Standard patterns** – Use patterns the condition parser recognizes.
3. **Avoid noise** – Keep activation heights, URLs, and references in separate fields.
4. **Bitwise vs logical** – Use `\land` for bitwise AND (e.g. `seq \land 0x00400000`); the parser maps it to Rust `&`.

## Named formulas (F_* ids) — v1 authoring

Orange Paper **named formulas** register stable **`F_*`** ids; **`cargo spec-lock verify`**, **`spec_enrich`**, and **`#[spec_locked]`** (proc-macro) resolve anchors that match **`^F_[A-Za-z0-9_]+$`** to **`SpecParser::formulas()`** before **`Function`** name lookup ([**`LOCKING_MECHANISM.md`**](docs/LOCKING_MECHANISM.md)).

- **Header:** ``**Formula** (**F_YourId**):`` on one line — **`YourId`** must match **`[A-Za-z0-9_]+`** (full id **`F_…`**).
- **Body:** the first **`$$ … $$`** block after the header becomes **`FormulaSpec.latex_body`** in **`SpecParser`** (trimmed inner text).
- **Depends on:** optional prose after **`$$`**; machine-readable **`Depends on`** lines list bold **`F_*`** / **`C_*`** anchors into **`FormulaSpec::depends_on`** (non-normative; **`cargo spec-lock list-formulas`** echoes **`depends_on`** plus **`missing_f_refs`** / **`missing_c_refs`** — **`F_*`** / **`C_*`** absent from merged formula / §**4** constant sets). **`SpecParser::unresolved_formula_dependencies()`** and **`SpecParser::unresolved_constant_dependencies()`** pair **`formula_id`** with missing deps; **`merged_consensus_constant_ids()`** exposes stable **`C_*`** keys from **`$CONST = …$`** in **`4.*`** sections. **`verify`**, **`check-formulas`**, **`check-drift`**, **`coverage`**, **`summary`**, **`extract-constants`**, **`extract-formulas`**, and **`extract-property-tests`** emit stderr **`formula_id -> dep`** lines when **`F_*`** or **`C_*`** Depends on refs are unresolved.
- **`#[spec_locked]`** — second literal or **`function = "F_..."`**, or **`"§::F_id"`**: lock § must **subsume** the formula heading § (**`13.3`** binds a formula authored under **`13.3.6`**, etc.). See **`blvm-consensus`** **`src/spec_lock_formula_witness.rs`** + **`PROTOCOL.md`** §**13.3.6** (**`F_SpecLockWitness`**) for a minimal end-to-end witness.

**Rollout:** set **`SPEC_LOCK_FORMULAS=0`** (**`false`** / **`no`** / **`off`**) to skip formula ingestion (**`SpecParser::formulas()`** empty). Unset ⇒ formulas **on** (default). See **[`VERIFY_JSON.md`](docs/VERIFY_JSON.md)** (environment notes).

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
- `\cdot`, `\cdotp`, `\times`, `\ast`, `\div` (and middot/multiplication-sign Unicode variants) normalize to arithmetic `*`, `/` for **`formula` / contract** lexer paths (**`formula_latex_parseable_for_verify`** gate)
- Unicode pasted from PDFs: **`≠`** (`U+2260`), **`−`** minus (`U+2212`), **`∧` / `∨`** (`U+2227` / `U+2228`), **`→` / `⇒`** (strip to implication conclusion, same as ASCII **`=>`** heuristic), **`⇔` / `⟺`** → **`==`** for the lexer gate
- `\mathrm{…}`, `\mathbf{…}`, `\mathit{…}`, `\mathsf{…}` → identifier (same as `\text`)
- `\left(`, `\right)`, `\left.`, `\right.` sizing dropped to plain parentheses / stripped
- `≥` → `>=`, `≤` → `<=`, `≠` → `!=`
- `\text{FunctionName}(args)` → `result`
- `\iff` → implication; conclusion is used for ensures

### Function blocks (`**Name**:`) vs metadata

The markdown scanner ends a spec function’s block at the next `**SingleWord**:` line. The following bold headers are **not** function signatures: they are skipped during scanning so they do not cut off **`**Properties**:`** for the real function above them:

- **`**Properties**:`** (and **`**Properties** (Updated):`**) — contracts list
- **`**Inputs**:`** — narrative inputs (e.g. §11.1.5)
- **`**Definition**:`** / **`**Definition** (…):`** — algorithm prose (the parser treats real specs as `(Qualifier):` lines whose payload starts with `$` / `\`)
- **`**Activation**:`**, **`**Deactivation**:`**, **`**References**:`**, **`**Mainnet**:`**, **`**Regtest**:`** — deployment / reference notes

Put math signatures immediately after **`**Func**:`** or after **`(Updated):`**, and keep metadata on these recognizable headers (or in bullet lists) so locking stays predictable.

## Implemented

- **Determinism verification** – Two-run Z3 for `result(a) == result(b)`.
- **Option/Result** – `is_some`, `is_none`, `is_ok`, `is_err`, `unwrap`, `match` in Z3.
- **Complex enums** – `\in {AllLegacy, All, None, Single}` → `(result == 0 || result == 1 || ...)`.

## Related

- `SPEC_AS_SOURCE_OF_TRUTH.md` – How spec-derived contracts flow to verification
- `SPEC_LOCK_COVERAGE.md` – Verification status and parseable %
- `cargo spec-lock coverage --spec-path ...` – Theorems → contracts → parseable %
- `cargo spec-lock check-drift --spec-path …` — Unparseable **Function** contracts; unparseable **`F_*`** **`$$`** bodies (**`--scoped-formulas`** when supported — same enrich/verify parse gate); **`--scoped-unparseables`** for §-scoped **Properties**
