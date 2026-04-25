//! Template parsing and rendering for the layout engine.
//!
//! Supports two rendering paths:
//! - `render()`: Legacy string-replacement approach (backward compatible)
//! - `render_template()`: AST-based conditional template engine with {if}/{else}/{endif}

use std::collections::HashMap;

use super::format::clean_separators;
use super::presets::get_preset_format;
use crate::config::LayoutConfig;
use crate::utils::sanitize_for_terminal;

// ---------------------------------------------------------------------------
// Conditional template AST types
// ---------------------------------------------------------------------------

/// A parsed template node.
#[derive(Debug, Clone)]
enum TemplateNode {
    /// Literal text to output as-is.
    Literal(String),
    /// Variable substitution: {var_name}
    Variable(String),
    /// Conditional block: {if condition}...{else}...{endif}
    Conditional {
        condition: Condition,
        if_branch: Vec<TemplateNode>,
        else_branch: Vec<TemplateNode>,
    },
}

/// A condition expression for conditional template blocks.
#[derive(Debug, Clone)]
enum Condition {
    /// Truthiness: non-empty string = true
    Truthy(String),
    /// Negation: {if !var}
    Negated(String),
    /// Equality: {if var == value}
    Equals(String, String),
    /// Inequality: {if var != value}
    NotEquals(String, String),
}

/// Maximum nesting depth for conditional blocks.
const MAX_NESTING_DEPTH: usize = 10;

/// Terminator found when parsing a branch inside a conditional.
#[derive(Debug, PartialEq)]
enum BranchTerminator {
    /// Hit {else}
    Else,
    /// Hit {endif}
    EndIf,
    /// Reached end of input (only valid at top level)
    EndOfInput,
}

// ---------------------------------------------------------------------------
// Template parsing
// ---------------------------------------------------------------------------

/// Parse a template string into an AST.
///
/// Handles:
/// - `{{` -> literal `{` (brace escaping)
/// - `{if condition}...{else}...{endif}` with nesting
/// - `{var_name}` variable references
/// - Plain literal text
fn parse_template(input: &str) -> Result<Vec<TemplateNode>, String> {
    let mut pos = 0;
    let (nodes, terminator) = parse_until_terminator(input, &mut pos, 0)?;
    match terminator {
        BranchTerminator::EndOfInput => Ok(nodes),
        BranchTerminator::Else => Err("unexpected {else} outside conditional".to_string()),
        BranchTerminator::EndIf => Err("unexpected {endif} outside conditional".to_string()),
    }
}

/// Parse template nodes until a terminator is found.
///
/// Returns (nodes, terminator). At the top level, expects EndOfInput.
/// Inside a conditional, expects Else or EndIf.
fn parse_until_terminator(
    input: &str,
    pos: &mut usize,
    depth: usize,
) -> Result<(Vec<TemplateNode>, BranchTerminator), String> {
    let mut nodes = Vec::new();
    let bytes = input.as_bytes();
    let len = input.len();

    while *pos < len {
        if bytes[*pos] == b'{' {
            // Check for escaped brace: {{
            if *pos + 1 < len && bytes[*pos + 1] == b'{' {
                nodes.push(TemplateNode::Literal("{".to_string()));
                *pos += 2;
                continue;
            }

            // Try to find the closing }
            if let Some(close_pos) = find_closing_brace(input, *pos) {
                let inner = &input[*pos + 1..close_pos];

                // Check for {else} -- terminate this branch
                if inner == "else" {
                    *pos = close_pos + 1;
                    return Ok((nodes, BranchTerminator::Else));
                }

                // Check for {endif} -- terminate this branch
                if inner == "endif" {
                    *pos = close_pos + 1;
                    return Ok((nodes, BranchTerminator::EndIf));
                }

                // Check for {if ...}
                if inner.starts_with("if ") || inner.starts_with("if!") {
                    let condition_str = if inner.starts_with("if!") {
                        &inner[2..] // includes the ! prefix
                    } else {
                        &inner[3..] // skip "if "
                    };

                    if depth >= MAX_NESTING_DEPTH {
                        return Err(format!(
                            "nesting depth exceeds maximum of {}",
                            MAX_NESTING_DEPTH
                        ));
                    }

                    let condition = parse_condition(condition_str.trim())?;
                    *pos = close_pos + 1;

                    // Parse the if-branch (stops at {else} or {endif})
                    let (if_branch, terminator) = parse_until_terminator(input, pos, depth + 1)?;

                    let else_branch = match terminator {
                        BranchTerminator::Else => {
                            // Parse the else-branch (stops at {endif})
                            let (else_nodes, end_terminator) =
                                parse_until_terminator(input, pos, depth + 1)?;
                            match end_terminator {
                                BranchTerminator::EndIf => else_nodes,
                                BranchTerminator::Else => {
                                    return Err("multiple {else} in single conditional".to_string());
                                }
                                BranchTerminator::EndOfInput => {
                                    return Err("unclosed {if} block (missing {endif})".to_string());
                                }
                            }
                        }
                        BranchTerminator::EndIf => Vec::new(),
                        BranchTerminator::EndOfInput => {
                            return Err("unclosed {if} block (missing {endif})".to_string());
                        }
                    };

                    nodes.push(TemplateNode::Conditional {
                        condition,
                        if_branch,
                        else_branch,
                    });
                    continue;
                }

                // It's a variable reference: {var_name}
                nodes.push(TemplateNode::Variable(inner.to_string()));
                *pos = close_pos + 1;
            } else {
                // No closing brace found -- treat { as literal text
                nodes.push(TemplateNode::Literal("{".to_string()));
                *pos += 1;
            }
        } else if bytes[*pos] == b'}' && *pos + 1 < len && bytes[*pos + 1] == b'}' {
            // Escaped closing brace: }} -> literal }
            nodes.push(TemplateNode::Literal("}".to_string()));
            *pos += 2;
        } else {
            // Accumulate literal text until next { or }} escape or end of input
            let start = *pos;
            while *pos < len && bytes[*pos] != b'{' {
                // Check for }} escape within literal text
                if bytes[*pos] == b'}' && *pos + 1 < len && bytes[*pos + 1] == b'}' {
                    break;
                }
                *pos += 1;
            }
            if *pos > start {
                nodes.push(TemplateNode::Literal(input[start..*pos].to_string()));
            }
        }
    }

    Ok((nodes, BranchTerminator::EndOfInput))
}

/// Find the position of the closing `}` for a brace starting at `start`.
fn find_closing_brace(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a condition string from inside `{if ...}`.
///
/// Supports:
/// - `var` -> Truthy(var)
/// - `!var` -> Negated(var)
/// - `var == value` -> Equals(var, value)
/// - `var != value` -> NotEquals(var, value)
fn parse_condition(s: &str) -> Result<Condition, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty condition in {if}".to_string());
    }

    // Check for != (must check before == since both contain =)
    if let Some(idx) = s.find("!=") {
        let var = s[..idx].trim().to_string();
        let val = s[idx + 2..].trim().to_string();
        if var.is_empty() {
            return Err("empty variable name in condition".to_string());
        }
        return Ok(Condition::NotEquals(var, val));
    }

    // Check for ==
    if let Some(idx) = s.find("==") {
        let var = s[..idx].trim().to_string();
        let val = s[idx + 2..].trim().to_string();
        if var.is_empty() {
            return Err("empty variable name in condition".to_string());
        }
        return Ok(Condition::Equals(var, val));
    }

    // Check for negation: !var
    if let Some(rest) = s.strip_prefix('!') {
        let var = rest.trim().to_string();
        if var.is_empty() {
            return Err("empty variable name after ! in condition".to_string());
        }
        return Ok(Condition::Negated(var));
    }

    // Simple truthiness: var
    Ok(Condition::Truthy(s.to_string()))
}

// ---------------------------------------------------------------------------
// Template evaluation
// ---------------------------------------------------------------------------

/// Evaluate an AST against a variable map.
///
/// - `show_unknown`: if true, unknown variables render as `{var_name}`.
///   If false, unknown variables render as empty string.
fn evaluate(nodes: &[TemplateNode], vars: &HashMap<String, String>, show_unknown: bool) -> String {
    let mut result = String::new();
    for node in nodes {
        match node {
            TemplateNode::Literal(text) => result.push_str(text),
            TemplateNode::Variable(name) => {
                if let Some(value) = vars.get(name.as_str()) {
                    result.push_str(value);
                } else if show_unknown {
                    result.push('{');
                    result.push_str(name);
                    result.push('}');
                }
                // else: unknown variable with show_unknown=false -> append nothing
            }
            TemplateNode::Conditional {
                condition,
                if_branch,
                else_branch,
            } => {
                if eval_condition(condition, vars) {
                    result.push_str(&evaluate(if_branch, vars, show_unknown));
                } else {
                    result.push_str(&evaluate(else_branch, vars, show_unknown));
                }
            }
        }
    }
    result
}

/// Evaluate a condition against a variable map.
fn eval_condition(condition: &Condition, vars: &HashMap<String, String>) -> bool {
    match condition {
        Condition::Truthy(var) => vars.get(var.as_str()).is_some_and(|v| !v.is_empty()),
        Condition::Negated(var) => vars.get(var.as_str()).is_none_or(|v| v.is_empty()),
        Condition::Equals(var, value) => vars.get(var.as_str()) == Some(value),
        Condition::NotEquals(var, value) => vars.get(var.as_str()) != Some(value),
    }
}

// ---------------------------------------------------------------------------
// Default template (embedded at compile time)
// ---------------------------------------------------------------------------

/// The default conditional template, embedded from src/templates/default.tmpl.
///
/// Uses {if} conditionals so absent segments (no GSD, no git, etc.) are
/// auto-hidden rather than leaving empty separators.
const DEFAULT_TEMPLATE: &str = include_str!("../templates/default.tmpl");

/// Load a user template override from the config directory.
///
/// Checks `~/.config/claudia-statusline/template.tmpl`. If it exists and
/// is readable, returns its contents. Otherwise returns None.
fn load_user_template() -> Option<String> {
    let config_dir = dirs::config_dir()?;
    let path = config_dir.join("claudia-statusline").join("template.tmpl");
    std::fs::read_to_string(&path).ok()
}

// ---------------------------------------------------------------------------
// LayoutRenderer
// ---------------------------------------------------------------------------

/// Layout renderer that handles template substitution.
///
/// Supports two rendering modes:
/// - `render()`: Legacy string-replacement (backward compatible, strips unknown vars)
/// - `render_template()`: AST-based with conditional support ({if}/{else}/{endif})
pub struct LayoutRenderer {
    /// The format template string
    pub(super) template: String,
    /// Separator to use for {sep}
    separator: String,
    /// Pre-parsed AST for template rendering (None if parse failed)
    ast: Option<Vec<TemplateNode>>,
    /// Parse error message, if AST parsing failed
    #[allow(dead_code)]
    parse_error: Option<String>,
}

impl LayoutRenderer {
    /// Create a new layout renderer from configuration
    pub fn from_config(config: &LayoutConfig) -> Self {
        let template = if config.format.is_empty() {
            get_preset_format(&config.preset).to_string()
        } else {
            config.format.clone()
        };

        let separator = config.separator.clone();
        Self::new_with_ast(template, separator)
    }

    /// Create a renderer using the conditional default template.
    ///
    /// Loads user template override from config directory first;
    /// falls back to the compiled-in default template.
    #[allow(dead_code)]
    pub fn default_template(separator: &str) -> Self {
        let template = load_user_template().unwrap_or_else(|| DEFAULT_TEMPLATE.to_string());
        Self::new_with_ast(template, separator.to_string())
    }

    /// Create a renderer with a specific format string
    #[allow(dead_code)]
    pub fn with_format(format: &str, separator: &str) -> Self {
        Self::new_with_ast(format.to_string(), separator.to_string())
    }

    /// Internal constructor that parses the template into an AST.
    ///
    /// `{sep}` is parsed as a regular variable; it is bound to the safe
    /// separator at render time in `render_template`. This avoids
    /// pre-parse text substitution, which would let a user-configured
    /// separator containing template syntax (e.g. `"{else}"`,
    /// `"}{if git}"`) inject AST structure into the parsed template.
    fn new_with_ast(template: String, separator: String) -> Self {
        let (ast, parse_error) = match parse_template(&template) {
            Ok(nodes) => (Some(nodes), None),
            Err(err) => (None, Some(err)),
        };

        Self {
            template,
            separator,
            ast,
            parse_error,
        }
    }

    /// Render the template with the provided variables (legacy path).
    ///
    /// Variables are provided as a HashMap where:
    /// - Key: variable name without braces (e.g., "directory")
    /// - Value: the rendered component string (with colors)
    ///
    /// Unknown variables are replaced with empty string.
    /// The {sep} variable is replaced with the configured separator.
    ///
    /// This method preserves exact backward compatibility with the pre-conditional
    /// template engine. For conditional template support, use `render_template()`.
    pub fn render(&self, variables: &HashMap<String, String>) -> String {
        let mut result = self.template.clone();

        // Sanitize separator (user-provided, could contain control characters)
        // but preserve valid ANSI colors in template output
        let safe_separator = sanitize_for_terminal(&self.separator);

        // Replace {sep} with sanitized separator
        result = result.replace("{sep}", &safe_separator);

        // Replace all variables
        for (key, value) in variables {
            let placeholder = format!("{{{}}}", key);
            result = result.replace(&placeholder, value);
        }

        // Remove any unreplaced variables (unknown or empty)
        result = remove_unreplaced_variables(&result);

        // Clean up multiple separators (when components are empty)
        // Use same sanitized separator for consistent matching
        result = clean_separators(&result, &safe_separator);

        result
    }

    /// Render using the conditional template engine (AST-based).
    ///
    /// Supports `{if var}...{else}...{endif}` conditionals, nesting,
    /// brace escaping (`{{` -> `{`), comparisons (`==`, `!=`), and negation (`!`).
    ///
    /// - `show_unknown`: if true, unknown variables render as `{var_name}` (debug-friendly).
    ///   If false, unknown variables render as empty string.
    ///
    /// On parse error (malformed template), returns `[tmpl err]`.
    ///
    /// The template is parsed once at construction time and evaluated against
    /// the provided variables at render time.
    ///
    /// # Security
    ///
    /// Variable values from `variables` are sanitized via
    /// [`crate::utils::sanitize_for_terminal`] before substitution, matching
    /// the legacy [`crate::display::VariableBuilder`] security boundary. The
    /// `{sep}` special variable is bound to the (also-sanitized) renderer
    /// separator. Sanitizing at render time is a load-bearing invariant: the
    /// AST evaluator concatenates variable values directly into the output
    /// string, so any ANSI escape sequences or control characters in raw
    /// provider values would otherwise reach the terminal verbatim.
    #[allow(dead_code)]
    pub fn render_template(
        &self,
        variables: &HashMap<String, String>,
        show_unknown: bool,
    ) -> String {
        match &self.ast {
            Some(nodes) => {
                let safe_separator = sanitize_for_terminal(&self.separator);
                // Build a fresh variable map: sanitize each provider value
                // (closes F4), then bind `sep` AFTER the sanitize pass to
                // avoid double-sanitization (separator is already
                // sanitized). Parse-time text replacement of `{sep}` is
                // also unsafe because separators are user-controlled and
                // `sanitize_for_terminal` does not strip braces (closes B5);
                // `{sep}` is therefore parsed as a regular variable and
                // resolved here.
                let mut sanitized: HashMap<String, String> = variables
                    .iter()
                    .map(|(k, v)| (k.clone(), sanitize_for_terminal(v)))
                    .collect();
                sanitized.insert("sep".into(), safe_separator.clone());
                let result = evaluate(nodes, &sanitized, show_unknown);
                clean_separators(&result, &safe_separator)
            }
            None => "[tmpl err]".to_string(),
        }
    }

    /// Check if the template uses a specific variable
    #[allow(dead_code)]
    pub fn uses_variable(&self, name: &str) -> bool {
        let placeholder = format!("{{{}}}", name);
        self.template.contains(&placeholder)
    }

    /// Get list of variables used in the template
    #[allow(dead_code)]
    pub fn get_used_variables(&self) -> Vec<String> {
        let mut variables = Vec::new();
        let mut chars = self.template.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                let mut var_name = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        if !var_name.is_empty() && var_name != "sep" {
                            variables.push(var_name);
                        }
                        break;
                    }
                    var_name.push(c);
                }
            }
        }

        variables
    }
}

/// Remove unreplaced {variable} placeholders from the string
fn remove_unreplaced_variables(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            // Check if this is a variable placeholder
            let mut var_content = String::new();
            let mut found_close = false;

            for c in chars.by_ref() {
                if c == '}' {
                    found_close = true;
                    break;
                }
                var_content.push(c);
            }

            if !found_close {
                // Not a valid placeholder, keep the opening brace
                result.push('{');
                result.push_str(&var_content);
            }
            // If found_close is true, we skip the whole {var} placeholder
        } else {
            result.push(c);
        }
    }

    result
}
