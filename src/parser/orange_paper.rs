//! Orange Paper parser
//!
//! Parses Orange Paper markdown to extract function specifications, theorems, and properties
//! and links them to Rust implementations.

use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Parse round-trip formula: \text{Outer}(\text{Inner}(x)) = x or (tx, w)
/// Returns (property_type, outer_func, inner_func, constraint)
fn parse_round_trip_formula(
    formula: &str,
) -> (
    StandalonePropertyType,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    // Match \text{FunctionName} - capture names (filter out common non-functions like "inputs", "valid")
    let text_func_re = Regex::new(r"\\text\{([^}]+)\}").ok().unwrap();
    let non_functions = ["inputs", "outputs", "valid", "invalid", "value"];
    let func_names: Vec<String> = text_func_re
        .captures_iter(formula)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .filter(|s| !non_functions.contains(&s.as_str()))
        .collect();

    // Round-trip: Outer(Inner(x)) = x → outer is first \text{} (wraps the rest), inner is second
    if func_names.len() >= 2 {
        let outer = func_names.first().cloned();
        let inner = func_names.get(1).cloned();
        if outer.is_some() && inner.is_some() {
            // Check for constraint: |w| = |tx.inputs| or similar
            let constraint = if formula.contains("|w|") && formula.contains("|tx") {
                Some("|w| = |tx.inputs|".to_string())
            } else if formula.contains("implies") || formula.contains("⟹") {
                formula
                    .split("implies")
                    .next()
                    .or_else(|| formula.split("⟹").next())
                    .map(|s| s.trim().to_string())
            } else {
                None
            };
            return (StandalonePropertyType::RoundTrip, outer, inner, constraint);
        }
    }

    // Idempotent: F(F(x)) = F(x) - same func name appears twice in nesting
    if !func_names.is_empty() {
        let first = &func_names[0];
        if formula.matches(first).count() >= 2 {
            return (
                StandalonePropertyType::Idempotent,
                func_names.first().cloned(),
                func_names.first().cloned(),
                None,
            );
        }
    }

    (StandalonePropertyType::Other, None, None, None)
}

/// A function specification from the Orange Paper
#[derive(Debug, Clone)]
pub struct FunctionSpec {
    /// Function name (e.g., "GetBlockSubsidy")
    pub name: String,
    /// Section ID (e.g., "6.1")
    pub section: String,
    /// Function signature (e.g., "ℕ → ℤ")
    pub signature: Option<String>,
    /// Properties extracted from the Orange Paper
    pub properties: Vec<Property>,
    /// Theorems related to this function
    pub theorems: Vec<Theorem>,
    /// Contracts extracted from properties
    pub contracts: Vec<Contract>,
    /// Raw markdown content for this section
    pub content: String,
    /// Conditions (for backward compatibility with macro_impl)
    pub conditions: Vec<String>,
    /// Mathematical formula
    pub formula: Option<String>,
    /// Description
    pub description: Option<String>,
}

/// A property from the Orange Paper
#[derive(Debug, Clone)]
pub struct Property {
    /// Property name (e.g., "Non-negative")
    pub name: String,
    /// Mathematical statement
    pub statement: String,
    /// Type of property (precondition, postcondition, invariant)
    pub property_type: PropertyType,
}

/// Type of property
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyType {
    Requires,  // Precondition
    Ensures,   // Postcondition
    Invariant, // Invariant
}

/// A theorem from the Orange Paper
#[derive(Debug, Clone)]
pub struct Theorem {
    /// Theorem number (e.g., "6.1.1")
    pub number: String,
    /// Theorem name
    pub name: String,
    /// Mathematical statement
    pub statement: String,
    /// Proof reference (e.g., formal proof name)
    pub proof_reference: Option<String>,
}

/// A contract extracted from Orange Paper
#[derive(Debug, Clone)]
pub struct Contract {
    /// Contract type
    pub contract_type: ContractType,
    /// Condition (mathematical expression, to be translated to Rust)
    pub condition: String,
    /// Comment/description
    pub comment: Option<String>,
}

/// Contract type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractType {
    Requires,
    Ensures,
    Property,
    EdgeCase,
}

/// Orange Paper parser
pub struct SpecParser {
    content: String,
    sections: HashMap<String, SpecSection>,
}

/// Standalone property block (e.g. **Property** (Name): ... $$...$$)
#[derive(Debug, Clone)]
pub struct StandaloneProperty {
    /// Property name (e.g. "SegWit Transaction Serialization Round-Trip")
    pub name: String,
    /// Section ID (e.g., "8.2.2")
    pub section_id: String,
    /// Raw LaTeX formula from $$...$$
    pub formula_raw: String,
    /// Parsed property type
    pub property_type: StandalonePropertyType,
    /// For round-trip: outer function (e.g. DeserializeTransactionWithWitness)
    pub outer_func: Option<String>,
    /// For round-trip: inner function (e.g. SerializeTransactionWithWitness)
    pub inner_func: Option<String>,
    /// Constraint from formula (e.g. |w| = |tx.inputs|)
    pub constraint: Option<String>,
}

/// Type of standalone property
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandalonePropertyType {
    RoundTrip,
    Idempotent,
    Ordering,
    Other,
}

/// A section from the Orange Paper
#[derive(Debug, Clone)]
pub struct SpecSection {
    /// Section ID (e.g., "6.1")
    pub id: String,
    /// Section title
    pub title: String,
    /// Functions in this section
    pub functions: Vec<FunctionSpec>,
    /// Theorems in this section
    pub theorems: Vec<Theorem>,
    /// Constants in this section
    pub constants: Vec<ExtractedConstant>,
    /// Standalone properties (e.g. **Property** (Name): ... $$...$$)
    pub standalone_properties: Vec<StandaloneProperty>,
    /// Raw content
    pub content: String,
}

/// Extracted constant from Orange Paper
#[derive(Debug, Clone)]
pub struct ExtractedConstant {
    /// Constant name (e.g., "H", "C", "MAX_MONEY")
    pub name: String,
    /// Section ID (e.g., "4.1")
    pub section: String,
    /// Mathematical value (e.g., "210000", "10^8")
    pub value: String,
    /// Rust type (e.g., "u64", "i64")
    pub rust_type: String,
    /// Rust expression (e.g., "210_000", "100_000_000")
    pub rust_expr: String,
    /// Description from Orange Paper
    pub description: String,
}

impl SpecParser {
    /// Create a new parser from Orange Paper content
    pub fn new(content: String) -> Self {
        SpecParser {
            content,
            sections: HashMap::new(),
        }
    }

    /// Merge sections from another parser. Errors if any section ID already exists.
    pub fn merge(&mut self, other: SpecParser) -> Result<(), String> {
        for (id, section) in other.sections {
            if self.sections.contains_key(&id) {
                return Err(format!("Duplicate section ID: {id}"));
            }
            self.sections.insert(id, section);
        }
        Ok(())
    }

    /// Create a parser from one or more spec files. Reads each file, parses, and merges sections.
    /// Errors on duplicate section IDs across files.
    pub fn from_paths<P: AsRef<Path>>(paths: &[P]) -> Result<SpecParser, String> {
        if paths.is_empty() {
            return Err("At least one spec path required".to_string());
        }
        // Resolve paths relative to cwd (supports both absolute and relative paths)
        let resolve = |p: &Path| -> Result<PathBuf, String> {
            if p.is_absolute() {
                Ok(p.to_path_buf())
            } else {
                std::env::current_dir()
                    .map_err(|e| format!("Failed to get cwd: {e}"))
                    .map(|cwd| cwd.join(p))
            }
        };
        let p0 = resolve(paths[0].as_ref())?;
        let content = std::fs::read_to_string(&p0)
            .map_err(|e| format!("Failed to read spec {}: {}", p0.display(), e))?;
        let mut parser = SpecParser::new(content);
        parser
            .parse()
            .map_err(|e| format!("Failed to parse spec: {e}"))?;

        for path in paths.iter().skip(1) {
            let p = resolve(path.as_ref())?;
            let content = std::fs::read_to_string(&p)
                .map_err(|e| format!("Failed to read spec {}: {}", p.display(), e))?;
            let mut other = SpecParser::new(content);
            other
                .parse()
                .map_err(|e| format!("Failed to parse spec {}: {}", p.display(), e))?;
            parser.merge(other)?;
        }
        Ok(parser)
    }

    /// Parse the entire Orange Paper (must be called before using other methods)
    pub fn parse(&mut self) -> Result<(), String> {
        // Initialize sections map if not already done
        if self.sections.is_empty() {
            // Parse will populate sections
        }
        // Split into sections by headers (both ### and ##)
        // Match sections like "6.1", "5.2.1", etc.
        let section_re = Regex::new(r"^##+?\s+(\d+(?:\.\d+)*)\s+(.+)$")
            .map_err(|e| format!("Regex error: {e}"))?;

        // Clone content to avoid borrow checker issues
        let content = self.content.clone();
        let lines: Vec<&str> = content.lines().collect();
        let mut current_section: Option<String> = None;
        let mut current_content = Vec::new();

        for line in &lines {
            if let Some(caps) = section_re.captures(line) {
                // Save previous section
                if let Some(ref section_id) = current_section {
                    self.parse_section(section_id, &current_content.join("\n"))?;
                }

                // Start new section
                let section_id = caps.get(1).unwrap().as_str().to_string();
                let title = caps.get(2).unwrap().as_str().to_string();
                current_section = Some(section_id.clone());
                current_content = vec![line.to_string()];

                // Initialize section
                self.sections.insert(
                    section_id.clone(),
                    SpecSection {
                        id: section_id,
                        title,
                        functions: Vec::new(),
                        theorems: Vec::new(),
                        constants: Vec::new(),
                        standalone_properties: Vec::new(),
                        content: String::new(),
                    },
                );
            } else if current_section.is_some() {
                current_content.push(line.to_string());
            }
        }

        // Parse last section
        if let Some(ref section_id) = current_section {
            self.parse_section(section_id, &current_content.join("\n"))?;
        }

        Ok(())
    }

    /// Parse a specific section
    fn parse_section(&mut self, section_id: &str, content: &str) -> Result<(), String> {
        // Get section content first
        let section_content = content.to_string();

        // Extract functions
        let function_re = Regex::new(r"\*\*(\w+)\*\*:\s*\$?([^\$]+)\$?")
            .map_err(|e| format!("Regex error: {e}"))?;

        let mut functions = Vec::new();

        for cap in function_re.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            // Skip section headers that look like functions
            if name == "Properties" || name == "Theorem" {
                continue;
            }
            let signature = cap.get(2).map(|m| m.as_str().to_string());

            let mut func_spec = FunctionSpec {
                name: name.clone(),
                section: section_id.to_string(),
                signature: signature.clone(),
                properties: Vec::new(),
                theorems: Vec::new(),
                contracts: Vec::new(),
                content: String::new(),
                conditions: Vec::new(),
                formula: None,
                description: None,
            };

            // Compute this function's block (from after signature to next function)
            let func_marker = format!("**{name}**");
            let block_content: &str = if let Some(func_pos) = content.find(&func_marker) {
                let start = func_pos + func_marker.len();
                let remaining = &content[start..];
                let block_end = Self::find_next_function_boundary(remaining);
                &remaining[..block_end]
            } else {
                ""
            };

            // Extract properties and theorems from this function's block only
            self.extract_properties_from_block(&mut func_spec, block_content, &name)?;
            self.extract_theorems(&mut func_spec, block_content)?;

            // Extract formula
            self.extract_formula(&mut func_spec, content, &name)?;

            // Generate contracts from properties
            self.generate_contracts(&mut func_spec)?;

            // NEW: Generate contracts from theorems
            self.generate_contracts_from_theorems(&mut func_spec)?;

            // Populate conditions from contracts
            func_spec.conditions = func_spec
                .contracts
                .iter()
                .map(|c| c.condition.clone())
                .collect();

            functions.push(func_spec);
        }

        // Extract constants for Section 4 (Consensus Constants)
        let mut constants = Vec::new();
        if section_id.starts_with("4.") {
            constants = self.extract_constants_from_section(section_id, content)?;
        }

        // Extract standalone **Property** (Name): ... $$...$$ blocks
        let standalone_properties = self.extract_standalone_properties(section_id, content)?;

        // Sections with Implementation Invariants: add synthetic "*" function so any
        // spec_locked function in this section gets those contracts (e.g. 10.6 Dandelion)
        if let Some(invariant_func) = self.extract_implementation_invariants(section_id, content)? {
            functions.push(invariant_func);
        }

        // Update section with parsed functions, constants, and standalone properties
        if let Some(section) = self.sections.get_mut(section_id) {
            section.content = section_content;
            section.functions = functions;
            section.constants = constants;
            section.standalone_properties = standalone_properties;
        }

        Ok(())
    }

    /// Extract Implementation Invariants block (e.g. 10.6 Dandelion)
    fn extract_implementation_invariants(
        &self,
        section_id: &str,
        content: &str,
    ) -> Result<Option<FunctionSpec>, String> {
        let marker = "**Implementation Invariants**";
        let inv_pos = content
            .find(marker)
            .or_else(|| {
                content.find("**Implementation Invariants (BLVM Specification Lock Verified)**")
            })
            .or_else(|| content.find("**Invariants**"));
        let Some(inv_pos) = inv_pos else {
            return Ok(None);
        };
        let block = &content[inv_pos..];
        let inv_re = Regex::new(r"(?m)^\d+\.\s*\*\*([^*]+)\*\*:\s*\$([^$]+)\$")
            .map_err(|e| format!("Regex error: {e}"))?;
        let mut properties = Vec::new();
        for cap in inv_re.captures_iter(block) {
            properties.push(Property {
                name: cap.get(1).unwrap().as_str().trim().to_string(),
                statement: cap.get(2).unwrap().as_str().trim().to_string(),
                property_type: PropertyType::Invariant,
            });
        }
        if properties.is_empty() {
            return Ok(None);
        }
        let mut func_spec = FunctionSpec {
            name: "*".to_string(),
            section: section_id.to_string(),
            signature: None,
            properties: properties.clone(),
            theorems: Vec::new(),
            contracts: Vec::new(),
            content: String::new(),
            conditions: Vec::new(),
            formula: None,
            description: None,
        };
        self.generate_contracts(&mut func_spec)?;
        Ok(Some(func_spec))
    }

    /// Extract constants from Section 4 (Consensus Constants)
    fn extract_constants_from_section(
        &self,
        section_id: &str,
        content: &str,
    ) -> Result<Vec<ExtractedConstant>, String> {
        let mut constants = Vec::new();

        // Pattern: $CONSTANT_NAME = value$ (description)
        // Examples:
        // $C = 10^8$ (satoshis per BTC, ...)
        // $H = 210,000$ (halving interval, ...)
        // $M_{max} = 21 \times 10^6 \times C$ (maximum money supply, ...)
        let constant_re =
            Regex::new(r"\$([A-Za-z_]+(?:\{[^}]+\})?)\s*=\s*([^$]+)\$\s*(?:\(([^)]+)\))?")
                .map_err(|e| format!("Regex error: {e}"))?;

        // Also match lines that might have constants without parentheses
        let _constant_re_alt = Regex::new(r"\$([A-Za-z_]+(?:\{[^}]+\})?)\s*=\s*([^$]+)\$")
            .map_err(|e| format!("Regex error: {e}"))?;

        for cap in constant_re.captures_iter(content) {
            let name_raw = cap.get(1).unwrap().as_str();
            let value_raw = cap.get(2).unwrap().as_str().trim();
            // Extract description - handle cases where it might be cut off
            let description = cap
                .get(3)
                .map(|m| {
                    let desc = m.as_str().to_string();
                    // Fix common issues: add closing paren if missing, clean up
                    if !desc.ends_with(')') && !desc.contains(')') {
                        format!("{desc})")
                    } else {
                        desc
                    }
                })
                .unwrap_or_else(|| {
                    // Try to find description in next line
                    String::new()
                });

            // Clean up constant name (remove LaTeX formatting)
            // Handle subscripts like M_{max} -> M_MAX
            let name = if name_raw.contains('{') {
                // Extract base and subscript
                let parts: Vec<&str> = name_raw.split('{').collect();
                if parts.len() == 2 {
                    let base = parts[0];
                    let subscript = parts[1].trim_end_matches('}');
                    format!("{}_{}", base.to_uppercase(), subscript.to_uppercase())
                } else {
                    name_raw.to_uppercase()
                }
            } else {
                name_raw.to_uppercase()
            };

            // Remove double underscores
            let name = name.replace("__", "_");

            // Parse value and convert to Rust expression
            let (rust_type, rust_expr) = self.parse_constant_value(value_raw)?;

            constants.push(ExtractedConstant {
                name,
                section: section_id.to_string(),
                value: value_raw.to_string(),
                rust_type,
                rust_expr,
                description,
            });
        }

        Ok(constants)
    }

    /// Extract standalone **Property** (Name): ... $$...$$ blocks
    fn extract_standalone_properties(
        &self,
        section_id: &str,
        content: &str,
    ) -> Result<Vec<StandaloneProperty>, String> {
        let mut properties = Vec::new();
        // Match **Property** (Name): - capture property name
        let property_re = Regex::new(r"\*\*Property\*\*\s*\(([^)]+)\):")
            .map_err(|e| format!("Regex error: {e}"))?;
        let formula_re = Regex::new(r"\$\$([^$]+)\$\$").map_err(|e| format!("Regex error: {e}"))?;

        for cap in property_re.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().trim().to_string();
            // Find position of this property in content
            let match_start = cap.get(0).unwrap().start();
            // Look for $$...$$ after this property (formula typically on next line)
            let after_prop = &content[match_start..];
            let formula_raw = formula_re
                .captures(after_prop)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            if formula_raw.is_empty() {
                continue;
            }

            // Parse round-trip: \text{Outer}(\text{Inner}(x)) = x or (tx, w)
            let (property_type, outer_func, inner_func, constraint) =
                parse_round_trip_formula(&formula_raw);

            properties.push(StandaloneProperty {
                name,
                section_id: section_id.to_string(),
                formula_raw: formula_raw.clone(),
                property_type,
                outer_func,
                inner_func,
                constraint,
            });
        }

        Ok(properties)
    }

    /// Parse constant value from mathematical notation to Rust expression
    fn parse_constant_value(&self, value: &str) -> Result<(String, String), String> {
        let value = value.trim();

        // Handle different formats:
        // - "10^8" -> 100_000_000 (u64)
        // - "210,000" -> 210_000 (u64)
        // - "21 \times 10^6 \times C" -> 21_000_000 * C (i64, needs C constant)
        // - "4 \times 10^6" -> 4_000_000 (usize)

        // Remove LaTeX formatting and normalize (keep commas for now, remove later)
        let cleaned = value
            .replace("\\times", "*")
            .replace("×", "*")
            .replace(" ", "")
            .trim()
            .to_string();

        // IMPORTANT: Check most specific patterns FIRST (with anchors ^ and $)
        // This prevents partial matches (e.g., "10^6" matching inside "21*10^6*C")

        // 1. Handle N * 10^M * C format (e.g., "21*10^6*C") - MOST SPECIFIC
        let mult_exp_c_re =
            Regex::new(r"^(\d+)\*10\^(\d+)\*C$").map_err(|e| format!("Regex error: {e}"))?;
        if let Some(captures) = mult_exp_c_re.captures(&cleaned) {
            let base: u64 = captures
                .get(1)
                .unwrap()
                .as_str()
                .parse()
                .map_err(|_| format!("Invalid base: {value}"))?;
            let exponent: u32 = captures
                .get(2)
                .unwrap()
                .as_str()
                .parse()
                .map_err(|_| format!("Invalid exponent: {value}"))?;
            let multiplier = base * 10u64.pow(exponent);
            let formatted = self.format_number_with_underscores(multiplier);
            // C is u64, so we need to cast for i64 result
            return Ok(("i64".to_string(), format!("({formatted} * C) as i64")));
        }

        // 2. Handle N * 10^M format (without C) - SECOND MOST SPECIFIC
        let mult_exp_re =
            Regex::new(r"^(\d+)\*10\^(\d+)$").map_err(|e| format!("Regex error: {e}"))?;
        if let Some(captures) = mult_exp_re.captures(&cleaned) {
            let base: u64 = captures
                .get(1)
                .unwrap()
                .as_str()
                .parse()
                .map_err(|_| format!("Invalid base: {value}"))?;
            let exponent: u32 = captures
                .get(2)
                .unwrap()
                .as_str()
                .parse()
                .map_err(|_| format!("Invalid exponent: {value}"))?;
            let result = base * 10u64.pow(exponent);
            let formatted = self.format_number_with_underscores(result);
            return Ok(("u64".to_string(), formatted));
        }

        // 3. Handle 10^N format - THIRD
        let exp_re = Regex::new(r"^10\^(\d+)$").map_err(|e| format!("Regex error: {e}"))?;
        if let Some(captures) = exp_re.captures(&cleaned) {
            let exponent: u32 = captures
                .get(1)
                .unwrap()
                .as_str()
                .parse()
                .map_err(|_| format!("Invalid exponent: {value}"))?;
            let result = 10u64.pow(exponent);
            let formatted = self.format_number_with_underscores(result);
            return Ok(("u64".to_string(), formatted));
        }

        // 4. Try to parse as number (handle commas) - LEAST SPECIFIC
        let num_str = cleaned.replace(",", "").replace("_", "");
        if let Ok(num) = num_str.parse::<u64>() {
            // Format with underscores for readability
            let formatted = self.format_number_with_underscores(num);
            return Ok(("u64".to_string(), formatted));
        }

        Err(format!("Could not parse constant value: {value}"))
    }

    /// Format number with underscores for readability (e.g., 210000 -> 210_000)
    fn format_number_with_underscores(&self, num: u64) -> String {
        let s = num.to_string();
        let mut result = String::new();
        let chars: Vec<char> = s.chars().rev().collect();

        for (i, ch) in chars.iter().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push('_');
            }
            result.push(*ch);
        }

        result.chars().rev().collect()
    }

    /// Find the position where the next function block starts (not **Properties** or **Theorem**)
    fn find_next_function_boundary(content: &str) -> usize {
        let mut search_start = 0;
        while let Some(pos) = content[search_start..].find("**") {
            let abs_pos = search_start + pos;
            let after = &content[abs_pos + 2..];
            // Skip **Properties**: and **Theorem X**: (skip the entire header, not just first **)
            if after.starts_with("Properties**") && !after.starts_with("Properties** (") {
                // Skip past "Properties**:" to avoid returning at the closing **
                if let Some(colon) = after.find(':') {
                    search_start = abs_pos + 2 + colon + 1;
                } else {
                    search_start = abs_pos + 2;
                }
                continue;
            }
            if after.starts_with("Theorem") && !after.starts_with("Theorem** (") {
                // Skip past "Theorem X.Y**" - find the closing **
                if let Some(close) = after.find("**") {
                    search_start = abs_pos + 2 + close + 2;
                } else {
                    search_start = abs_pos + 2;
                }
                continue;
            }
            return abs_pos;
        }
        content.len()
    }

    /// Extract properties for a function from its block content (already scoped to this function)
    fn extract_properties_from_block(
        &self,
        func: &mut FunctionSpec,
        block_content: &str,
        _func_name: &str,
    ) -> Result<(), String> {
        // Look for properties list - match "- PropertyName:" or "- **PropertyName**:"
        let property_re = Regex::new(r"(?m)^\s*-\s*(?:\*\*)?([^:*]+)(?:\*\*)?:\s*(.+)$")
            .map_err(|e| format!("Regex error: {e}"))?;

        // Look for "**Properties**:" header (exact - not **Properties** (Updated):)
        // Also try "**Properties**:" with optional space before colon (spec format variation)
        let props_start = block_content
            .find("**Properties**:")
            .or_else(|| block_content.find("**Properties** :"));
        if let Some(props_start) = props_start {
            let props_section = &block_content[props_start..];
            for cap in property_re.captures_iter(props_section) {
                let name = cap.get(1).unwrap().as_str().trim().to_string();
                let statement = cap.get(2).unwrap().as_str().trim().to_string();

                // Determine property type
                let property_type = if statement.contains("≥")
                    || statement.contains(">=")
                    || statement.contains("≤")
                    || statement.contains("<=")
                    || statement.contains("=")
                {
                    // Usually a postcondition or invariant
                    if statement.contains("result") || statement.contains("return") {
                        PropertyType::Ensures
                    } else {
                        PropertyType::Invariant
                    }
                } else if statement.contains("implies") || statement.contains("⟹") {
                    // A => B: conclusion B is postcondition (ensures when precondition A holds)
                    PropertyType::Ensures
                } else {
                    PropertyType::Ensures
                };

                func.properties.push(Property {
                    name,
                    statement,
                    property_type,
                });
            }
        }

        Ok(())
    }

    /// Extract theorems
    fn extract_theorems(&self, func: &mut FunctionSpec, content: &str) -> Result<(), String> {
        // Match: **Theorem X.Y.Z** (Name)
        // Use simple, reliable regex
        let theorem_re = Regex::new(r"\*\*Theorem\s+([\d.]+)\*\*[^(]*\(([^)]+)\)")
            .map_err(|e| format!("Regex error: {e}"))?;
        let latex_block_re =
            Regex::new(r"\$\$([^$]+)\$\$").map_err(|e| format!("Regex error: {e}"))?;
        let inline_math_re = Regex::new(r"\$([^$]+)\$").map_err(|e| format!("Regex error: {e}"))?;

        for cap in theorem_re.captures_iter(content) {
            let number = cap.get(1).unwrap().as_str().to_string();
            let name = cap.get(2).unwrap().as_str().to_string();

            // Extract statement - look for LaTeX math blocks or inline math after theorem
            let mut statement = String::new();

            // Find position of this theorem in content
            let theorem_pos = cap.get(0).unwrap().end();
            let content_after = &content[theorem_pos..];

            // Look for LaTeX math blocks ($$...$$)
            if let Some(latex_cap) = latex_block_re.captures(content_after) {
                if let Some(latex_content) = latex_cap.get(1) {
                    statement = latex_content.as_str().to_string();
                }
            }

            // If no LaTeX block, look for inline math
            if statement.is_empty() {
                if let Some(math_cap) = inline_math_re.captures(content_after) {
                    if let Some(math_content) = math_cap.get(1) {
                        statement = math_content.as_str().to_string();
                    }
                }
            }

            // If still empty, try to extract from next few lines
            if statement.is_empty() {
                let lines: Vec<&str> = content_after.lines().take(5).collect();
                let potential_statement: String = lines.join(" ").trim().to_string();
                // Only use if it looks like a mathematical statement
                if potential_statement.contains("∀")
                    || potential_statement.contains("∈")
                    || potential_statement.contains("≥")
                    || potential_statement.contains("≤")
                    || potential_statement.contains("=")
                    || potential_statement.contains(&func.name)
                {
                    statement = potential_statement;
                }
            }

            // Fallback if still empty
            if statement.is_empty() {
                statement = format!("See Orange Paper Theorem {number} for full statement");
            }

            // Look for proof reference
            let proof_ref =
                if content_after.contains("proof") || content_after.contains("verification") {
                    Some("Formal verification".to_string())
                } else {
                    None
                };

            func.theorems.push(Theorem {
                number,
                name,
                statement,
                proof_reference: proof_ref,
            });
        }

        Ok(())
    }

    /// Extract mathematical formula
    fn extract_formula(
        &self,
        func: &mut FunctionSpec,
        content: &str,
        func_name: &str,
    ) -> Result<(), String> {
        // Look for LaTeX formula: $$\text{FunctionName}(...) = ...$$
        // Use string search instead of regex to avoid escape issues
        let latex_func = format!(r"\text{{{func_name}}}");
        let latex_func_alt = func_name.to_string(); // Also match without \text{}

        // Find formula blocks (between $$)
        let mut in_formula = false;
        let mut formula_content = String::new();
        let mut lines_in_formula = 0;
        const MAX_FORMULA_LINES: usize = 10; // Limit formula extraction to prevent grabbing too much

        for line in content.lines() {
            if line.contains("$$") {
                if !in_formula {
                    // Start of formula
                    in_formula = true;
                    formula_content = line.to_string();
                    lines_in_formula = 1;
                } else {
                    // End of formula
                    formula_content.push('\n');
                    formula_content.push_str(line);

                    // Check if this formula matches our function
                    if formula_content.contains(&latex_func)
                        || formula_content.contains(&latex_func_alt)
                        || (formula_content.contains("GetBlockSubsidy")
                            && func_name.contains("Subsidy"))
                        || (formula_content.contains("TotalSupply") && func_name.contains("Supply"))
                    {
                        // Clean up formula - remove extra content after closing $$
                        let cleaned = self.clean_formula(&formula_content);
                        func.formula = Some(cleaned);
                        break;
                    }
                    in_formula = false;
                    formula_content.clear();
                    lines_in_formula = 0;
                }
            } else if in_formula {
                if lines_in_formula < MAX_FORMULA_LINES {
                    formula_content.push('\n');
                    formula_content.push_str(line);
                    lines_in_formula += 1;
                } else {
                    // Formula too long, stop extracting
                    in_formula = false;
                    formula_content.clear();
                    lines_in_formula = 0;
                }
            }
        }

        Ok(())
    }

    /// Clean up extracted formula to remove extra content
    fn clean_formula(&self, formula: &str) -> String {
        // Extract just the formula between $$ markers
        let parts: Vec<&str> = formula.split("$$").collect();
        if parts.len() >= 2 {
            // Take content between first and last $$
            let mut cleaned = String::new();
            for (i, part) in parts.iter().enumerate() {
                if i % 2 == 1 {
                    // Odd indices are between $$ markers
                    if !cleaned.is_empty() {
                        cleaned.push('\n');
                    }
                    cleaned.push_str(part);
                }
            }
            cleaned.trim().to_string()
        } else {
            formula.trim().to_string()
        }
    }

    /// Generate contracts from properties
    fn generate_contracts(&self, func: &mut FunctionSpec) -> Result<(), String> {
        for property in &func.properties {
            // Translate mathematical notation to Rust-like expression
            let condition = self.translate_property_to_rust(&property.statement, &func.name)?;

            let contract_type = match property.property_type {
                PropertyType::Requires => ContractType::Requires,
                PropertyType::Ensures | PropertyType::Invariant => ContractType::Ensures,
            };

            func.contracts.push(Contract {
                contract_type,
                condition,
                comment: Some(property.name.clone()),
            });
        }

        Ok(())
    }

    /// Generate contracts from theorems
    /// Extracts properties directly from theorem statements
    fn generate_contracts_from_theorems(&self, func: &mut FunctionSpec) -> Result<(), String> {
        for theorem in &func.theorems {
            // Parse theorem statement to extract properties
            // Theorem format: "∀h ∈ ℕ: get_block_subsidy(h) ≥ 0 ∧ get_block_subsidy(h) ≤ INITIAL_SUBSIDY"
            let statement = &theorem.statement;

            // Split by logical operators (∧, ∨, ⟹, etc.)
            // For now, handle simple conjunctions (∧)
            let parts: Vec<&str> = statement.split('∧').collect();

            for part in parts {
                let part = part.trim();

                // Check if this part mentions the function
                let func_name_lower = func.name.to_lowercase();
                if part.to_lowercase().contains(&func_name_lower)
                    || part.contains(&format!(r"\text{{{}}}", func.name))
                {
                    // Translate to Rust contract
                    let condition = self.translate_theorem_statement_to_rust(part, &func.name)?;

                    // Determine contract type based on statement (theorems → ensures)
                    let contract_type = ContractType::Ensures;

                    func.contracts.push(Contract {
                        contract_type,
                        condition,
                        comment: Some(format!("From Theorem {}", theorem.number)),
                    });
                }
            }
        }

        Ok(())
    }

    /// Translate theorem statement to Rust-like expression
    fn translate_theorem_statement_to_rust(
        &self,
        statement: &str,
        func_name: &str,
    ) -> Result<String, String> {
        // Use the same translation logic as properties
        self.translate_property_to_rust(statement, func_name)
    }

    /// Translate mathematical property to Rust-like expression
    fn translate_property_to_rust(
        &self,
        statement: &str,
        func_name: &str,
    ) -> Result<String, String> {
        // Simple translation - replace common patterns
        let mut rust_expr = statement.to_string();

        // Replace LaTeX function calls - handle escaped backslashes properly
        // First replace LaTeX \text{FunctionName} format (literal backslash + text)
        let latex_pattern = format!(r"\text{{{func_name}}}");
        rust_expr = rust_expr.replace(&latex_pattern, "result");
        // Also handle escaped version
        let latex_pattern_escaped = format!(r"\\text{{{func_name}}}");
        rust_expr = rust_expr.replace(&latex_pattern_escaped, "result");
        // Then replace plain function name (but not if it's part of a larger word)
        rust_expr = rust_expr.replace(func_name, "result");

        // Replace mathematical operators
        rust_expr = rust_expr.replace("≥", ">=");
        rust_expr = rust_expr.replace("≤", "<=");
        rust_expr = rust_expr.replace("≠", "!=");
        rust_expr = rust_expr.replace("⟹", "=>");
        // LaTeX \land in bitwise context (e.g. seq \land 0x00400000) → Rust &
        rust_expr = rust_expr.replace(r"\land", " & ");

        // For "A => B" (implication), use conclusion B for ensures
        if let Some(arrow) = rust_expr.find("=>") {
            let after = rust_expr[arrow + 2..].trim();
            if !after.is_empty() && !after.contains("=>") {
                rust_expr = after.to_string();
            }
        }

        // Math "=" in comparisons → Rust "==" (equality)
        // Be careful: only in comparison context, not assignment. Replace standalone = between identifiers/numbers
        rust_expr = rust_expr.replace(" = ", " == ");
        if rust_expr.starts_with("= ") {
            rust_expr = rust_expr.replacen("= ", "== ", 1);
        }
        if rust_expr.ends_with(" =") {
            let len = rust_expr.len();
            rust_expr = rust_expr[..len - 2].to_string() + " ==";
        }

        // |x| (cardinality) → x.len()
        let cardinality_re = Regex::new(r"\|([a-zA-Z_][a-zA-Z0-9_]*)\|").ok();
        if let Some(re) = cardinality_re {
            rust_expr = re.replace_all(&rust_expr, "${1}.len()").to_string();
        }

        // Replace common variables (be careful not to replace in the middle of words)
        rust_expr = rust_expr.replace(" h ", " height ");
        rust_expr = rust_expr.replace("(h)", "(height)");
        rust_expr = rust_expr.replace(" h,", " height,");
        rust_expr = rust_expr.replace(" h)", " height)");
        rust_expr = rust_expr.replace(" H ", " HALVING_INTERVAL ");
        rust_expr = rust_expr.replace("(H)", "(HALVING_INTERVAL)");
        rust_expr = rust_expr.replace(" C ", " SATOSHIS_PER_BTC ");
        rust_expr = rust_expr.replace("(C)", "(SATOSHIS_PER_BTC)");

        // Remove LaTeX formatting (do this last)
        rust_expr = rust_expr.replace("$", "");
        rust_expr = rust_expr.replace(r"\text{", "");
        rust_expr = rust_expr.replace(r"\{", "{");
        rust_expr = rust_expr.replace(r"\}", "}");

        Ok(rust_expr)
    }

    /// Iterate over all sections (for coverage reports)
    pub fn iter_sections(&self) -> impl Iterator<Item = (&String, &SpecSection)> {
        self.sections.iter()
    }

    /// Find a section by ID
    pub fn find_section(&self, section_id: &str) -> Option<&SpecSection> {
        self.sections.get(section_id)
    }

    /// Find a function specification by section and name
    pub fn find_function(&self, section: &str, name: Option<&str>) -> Option<&FunctionSpec> {
        if let Some(spec_section) = self.sections.get(section) {
            if let Some(func_name) = name {
                spec_section
                    .functions
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case(func_name))
                    .or_else(|| {
                        // Fallback: section with Implementation Invariants ("*") applies to any function
                        spec_section.functions.iter().find(|f| f.name == "*")
                    })
            } else {
                spec_section.functions.first()
            }
        } else {
            None
        }
    }

    /// Get all functions in a section
    pub fn get_section_functions(&self, section: &str) -> Vec<&FunctionSpec> {
        self.sections
            .get(section)
            .map(|s| s.functions.iter().collect())
            .unwrap_or_default()
    }

    /// Parse function signature
    pub fn parse_signature(sig: &str) -> Option<(Vec<String>, String)> {
        // Simple signature parser: "ℕ → ℤ" or "Natural → Integer"
        if sig.contains("→") || sig.contains("->") {
            let parts: Vec<&str> = sig.split("→").collect();
            if parts.len() == 2 {
                let input = parts[0].trim().to_string();
                let output = parts[1].trim().to_string();
                return Some((vec![input], output));
            }
        }
        None
    }

    /// Find a function specification by name across all sections
    /// Returns the function spec and its section ID
    pub fn find_function_anywhere(&self, func_name: &str) -> Option<(&FunctionSpec, &str)> {
        for (section_id, section) in &self.sections {
            if let Some(func_spec) = section
                .functions
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(func_name))
            {
                return Some((func_spec, section_id));
            }
        }
        None
    }

    /// Find a theorem by function name across all sections
    /// Searches theorem statements for function name mentions
    pub fn find_theorem_by_function_name(&self, func_name: &str) -> Option<(&Theorem, &str, &str)> {
        let func_name_lower = func_name.to_lowercase();
        let func_name_variations = [
            func_name_lower.clone(),
            func_name.to_string(),
            format!("\\text{{{func_name}}}"),
            format!("\\text{{{func_name_lower}}}"),
        ];

        for (section_id, section) in &self.sections {
            for theorem in &section.theorems {
                // Check if theorem statement contains function name
                let theorem_lower = theorem.statement.to_lowercase();
                if func_name_variations.iter().any(|variant| {
                    theorem_lower.contains(variant) || theorem.statement.contains(variant)
                }) {
                    // Find the function in this section
                    if let Some(func_spec) = section
                        .functions
                        .iter()
                        .find(|f| f.name.eq_ignore_ascii_case(func_name))
                    {
                        return Some((theorem, section_id, &func_spec.name));
                    }
                }
            }
        }
        None
    }

    /// Find subsection by granular ID (e.g., "5.1.1")
    /// Returns the section and subsection ID
    pub fn find_subsection(&self, granular_id: &str) -> Option<(&SpecSection, String)> {
        // Parse granular ID: "5.1.1" -> section "5.1", subsection "5.1.1"
        let parts: Vec<&str> = granular_id.split('.').collect();
        if parts.len() >= 2 {
            let section_id = parts[0..parts.len() - 1].join(".");
            if let Some(section) = self.sections.get(&section_id) {
                // Check if granular_id matches a subsection pattern
                // For now, check if it's mentioned in section content or theorems
                if section.content.contains(granular_id)
                    || section.theorems.iter().any(|t| t.number == granular_id)
                {
                    return Some((section, granular_id.to_string()));
                }
            }
        }
        None
    }

    /// Get all theorems in a section
    pub fn get_section_theorems(&self, section_id: &str) -> Vec<&Theorem> {
        self.sections
            .get(section_id)
            .map(|s| s.theorems.iter().collect())
            .unwrap_or_default()
    }

    /// Extract all constants from Section 4 (Consensus Constants)
    pub fn extract_constants(&self) -> Vec<&ExtractedConstant> {
        let mut constants = Vec::new();

        // Extract from Section 4.1, 4.2, 4.3, 4.4
        for section_id in &["4.1", "4.2", "4.3", "4.4"] {
            if let Some(section) = self.sections.get(*section_id) {
                constants.extend(section.constants.iter());
            }
        }

        constants
    }

    /// Get constants from a specific section
    pub fn get_section_constants(&self, section_id: &str) -> Vec<&ExtractedConstant> {
        self.sections
            .get(section_id)
            .map(|s| s.constants.iter().collect())
            .unwrap_or_default()
    }

    /// Get all standalone properties (round-trip, etc.) from all sections
    pub fn get_all_standalone_properties(&self) -> Vec<&StandaloneProperty> {
        let mut props = Vec::new();
        for section in self.sections.values() {
            props.extend(section.standalone_properties.iter());
        }
        props
    }

    /// Extract all functions with formulas from Orange Paper
    pub fn extract_functions_with_formulas(&self) -> Vec<&FunctionSpec> {
        let mut functions = Vec::new();

        for section in self.sections.values() {
            for func in &section.functions {
                if func.formula.is_some() {
                    functions.push(func);
                }
            }
        }

        functions
    }

    /// Get function by name
    pub fn get_function(&self, name: &str) -> Option<&FunctionSpec> {
        for section in self.sections.values() {
            for func in &section.functions {
                if func.name == name {
                    return Some(func);
                }
            }
        }
        None
    }
}
