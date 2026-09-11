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
    // Genuinely never READ: retained as parse diagnostics captured at
    // construction for future surfacing; nothing consumes it today.
    #[allow(dead_code)]
    parse_error: Option<String>,
}

impl LayoutRenderer {
    /// Create a new layout renderer from configuration.
    ///
    /// `#[allow(dead_code)]`: `src/display.rs` no longer calls this — it computes
    /// the effective template itself and calls [`Self::with_format`], so the
    /// preset is resolved ONCE per render and the pricing gate can query the very
    /// renderer that will produce the output (CR-01). This remains the
    /// config-driven constructor for library consumers and is exercised by
    /// `layout::tests::test_from_config_preset` /
    /// `test_from_config_custom_format`; the two constructions MUST stay
    /// equivalent (empty `format` => named preset, else the format string).
    #[allow(dead_code)]
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
    pub fn default_template(separator: &str) -> Self {
        let template = load_user_template().unwrap_or_else(|| DEFAULT_TEMPLATE.to_string());
        Self::new_with_ast(template, separator.to_string())
    }

    /// Create a renderer with a specific format string
    pub fn with_format(format: &str, separator: &str) -> Self {
        Self::new_with_ast(format.to_string(), separator.to_string())
    }

    /// Internal constructor that parses the template into an AST.
    ///
    /// `{sep}` is parsed as a regular variable; it is bound to the safe
    /// separator at render time in BOTH [`Self::render_template`] and
    /// [`Self::render`]. This avoids pre-parse text substitution, which would
    /// let a user-configured separator containing template syntax (e.g.
    /// `"{else}"`, `"}{if git}"`) inject AST structure into the parsed template.
    ///
    /// Round-4 CR-01 was that same B5 defect surviving on the legacy path:
    /// `render` still pre-expanded `{sep}` into its output buffer before
    /// substituting over it, so a price placeholder sitting inside a user's
    /// `[layout].separator` was filled in from a table the price-source gate in
    /// `src/display.rs` had never authorized. `render` is now a single
    /// left-to-right pass that resolves `{sep}` inline, closing it there too.
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

    /// Render the template with the provided variables (legacy, non-AST path).
    ///
    /// Variables are provided as a HashMap where:
    /// - Key: variable name without braces (e.g., "directory")
    /// - Value: the rendered component string (with colors)
    ///
    /// # One left-to-right pass
    ///
    /// This is ONE scan of `self.template`. Literal text is copied to the output
    /// buffer and each `{name}` is resolved from the variable map exactly once,
    /// and the scan never re-examines text it has already written. Two
    /// consequences are load-bearing:
    ///
    /// * A substituted VALUE containing `{name}` is inert DATA, not a
    ///   placeholder (round-4 CR-02). Values reach this method from untrusted
    ///   external input — `workspace.current_dir`, git branch names, model ids —
    ///   and [`crate::utils::sanitize_for_terminal`] deliberately does not strip
    ///   braces, so re-scanning emitted output let that input inject a
    ///   statusline variable. Nondeterministically, too: the old implementation
    ///   looped over the map in randomized `HashMap` order, so the same payload
    ///   rendered two different lines.
    /// * `{sep}` is resolved INLINE as a variable rather than pre-expanded into
    ///   the buffer, so the SEPARATOR'S TEXT is not part of the substitution
    ///   surface (round-4 CR-01). This matches [`Self::render_template`] and
    ///   extends [`Self::new_with_ast`]'s B5 rationale to this path. The
    ///   price-source gate in `src/display.rs` scans `self.template` only;
    ///   keeping the separator out of the substitution surface is what makes
    ///   that a STRUCTURAL superset rather than a claim re-proved against each
    ///   new reproducer.
    ///
    /// # Brace handling (the legacy unreplaced-placeholder sweep, folded in)
    ///
    /// * An unknown `{...}` span is DROPPED whole, from the first `{` through
    ///   the `}`, so an unresolved variable vanishes along with its braces.
    /// * A `{` with no following `}` is KEPT VERBATIM, together with everything
    ///   after it.
    /// * The variable NAME is taken from the LAST `{` before the closing `}`.
    ///   That reproduces the old `str::replace("{name}", value)` on escaped
    ///   forms such as `"{{api_equiv_cost}"`, where substitution began at byte
    ///   one, and it is what keeps [`Self::uses_variable`] — i.e.
    ///   `template.contains("{name}")` — a superset of what this scan resolves.
    ///
    /// Offsets are BYTE offsets from `str::find`/`str::rfind` over the ASCII
    /// needles `{` and `}`; templates, values and separators are routinely
    /// multi-byte UTF-8 (the default separator is `" \u{2022} "`).
    ///
    /// # Intentional divergence from the pre-11-11 implementation
    ///
    /// | Input | Before (HEAD `c84482c`) | After | Why |
    /// |-------|-------------------------|-------|-----|
    /// | a separator containing `{api_equiv_cost}` | the bundled figure was substituted into it | the separator text is emitted verbatim | closes CR-01; matches `render_template` |
    /// | a variable VALUE containing `{other_var}` | nondeterministically substituted or swallowed | emitted verbatim | closes CR-02; matches `evaluate` |
    /// | `{{name}}` where `name` RESOLVES | `""` (the substituted value was then eaten by the legacy unreplaced-placeholder sweep, which this scan now subsumes) | `{VALUE}` | a substituted value is never re-scanned. The UNRESOLVED case is unchanged and still yields `}` |
    ///
    /// Everything else is byte-identical. For conditional template support, use
    /// [`Self::render_template`].
    pub fn render(&self, variables: &HashMap<String, String>) -> String {
        // Sanitize separator (user-provided, could contain control characters)
        // but preserve valid ANSI colors in template output.
        let safe_separator = sanitize_for_terminal(&self.separator);

        let mut out = String::with_capacity(self.template.len());
        let mut rest = self.template.as_str();

        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);

            let Some(close_rel) = rest[open..].find('}') else {
                // No closing brace: this is not a placeholder. Keep the `{` and
                // everything after it verbatim, then stop scanning.
                out.push_str(&rest[open..]);
                rest = "";
                break;
            };
            let close = open + close_rel;

            // The LAST `{` before the `}` names the variable, so an escaped
            // form such as `{{name}` still resolves `name` (see the rustdoc).
            // `rest[open]` is `'{'`, so the fallback is unreachable; it is a
            // fallback rather than an unwrap because this is the render path
            // and the status line must never panic.
            let name_open = rest[open..close].rfind('{').map_or(open, |r| open + r);
            let name = &rest[name_open + 1..close];

            if name == "sep" {
                // `sep` resolves from the configured separator and NEVER from
                // the variable map. Checked before the map lookup to preserve
                // the precedence the old pre-expansion had.
                out.push_str(&rest[open..name_open]);
                out.push_str(&safe_separator);
            } else if let Some(value) = variables.get(name) {
                // Emitted VERBATIM: `out` is never re-scanned, so braces inside
                // a value are data.
                out.push_str(&rest[open..name_open]);
                out.push_str(value);
            }
            // Unknown name: the whole `{..}` span is dropped, braces included.

            rest = &rest[close + 1..];
        }
        out.push_str(rest);

        // Clean up multiple separators (when components are empty).
        // Uses the same sanitized separator for consistent matching.
        clean_separators(&out, &safe_separator)
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

    /// Does the PARSED template use any variable whose name starts with
    /// `prefix`?
    ///
    /// This is the AST-level counterpart to the string-level
    /// [`Self::uses_variable`] / [`Self::get_used_variables`], which both re-scan
    /// the raw template text. It exists because the price-source gate in
    /// `src/display.rs` must not be tripped by a literal MENTION of
    /// `api_equiv_cost` sitting outside a `{...}` placeholder (review finding
    /// RV-L1): a mention is not a use, and reading `prices.json` for one is
    /// filesystem IO the user never asked for.
    ///
    /// `uses_variable` and `get_used_variables` remain string-level and MUST NOT
    /// be used for that gate.
    ///
    /// When the template failed to parse (`self.ast` is `None`) this returns
    /// `false` — it FAILS CLOSED (WR-03, round 3). This method is NOT the whole
    /// gate: `src/display.rs` ORs it with a RAW half
    /// (`PRICE_VARS.iter().any(|v| renderer.uses_variable(v))`) that tests
    /// `template.contains("{name}")`, i.e. exactly the substrings
    /// [`Self::render`] — the method that actually produces output on that path
    /// — can resolve in its single left-to-right scan of the unparsed template.
    /// The raw half is therefore a STRUCTURAL superset of what an unparsed
    /// template can substitute, so answering `false` here cannot blank a
    /// price. Meanwhile [`Self::render_template`] substitutes
    /// NOTHING without an AST (it returns `"[tmpl err]"`), so failing open here
    /// bought nothing and cost a `prices.json` `File::open` on EVERY render for
    /// any user whose `[layout] format` merely fails to parse (e.g.
    /// `"{directory}{if git}"`). Do not re-introduce fail-open on the assumption
    /// that this method is the whole gate — see the two-half rationale in
    /// `src/display.rs`.
    ///
    /// Conditional branches are searched too, AND
    /// so is the conditional's own CONDITION: a variable used only inside
    /// `{if ..}` still counts as used (review CR-01, second instance — the arm
    /// previously destructured `{ if_branch, else_branch, .. }` and discarded
    /// `condition`, making that promise aspirational rather than true).
    pub fn uses_variable_prefix(&self, prefix: &str) -> bool {
        /// The variable name a condition references. All four `Condition`
        /// variants carry one, so all four must be scanned — an exhaustive
        /// match here means a fifth variant cannot be added without a
        /// compile error pointing at this gate.
        fn cond_name(c: &Condition) -> &str {
            match c {
                Condition::Truthy(n)
                | Condition::Negated(n)
                | Condition::Equals(n, _)
                | Condition::NotEquals(n, _) => n,
            }
        }

        fn scan(nodes: &[TemplateNode], prefix: &str) -> bool {
            nodes.iter().any(|node| match node {
                TemplateNode::Literal(_) => false,
                TemplateNode::Variable(name) => name.starts_with(prefix),
                TemplateNode::Conditional {
                    condition,
                    if_branch,
                    else_branch,
                } => {
                    cond_name(condition).starts_with(prefix)
                        || scan(if_branch, prefix)
                        || scan(else_branch, prefix)
                }
            })
        }

        match &self.ast {
            Some(nodes) => scan(nodes, prefix),
            // Fail CLOSED (WR-03, round 3). A `None` AST means `render_template`
            // returns "[tmpl err]" and substitutes nothing, while the path that
            // actually runs — `format_statusline_with_layout` ->
            // `LayoutRenderer::render` — is fully covered by the RAW half of the
            // gate in src/display.rs, which tests `template.contains("{name}")`
            // and is a structural superset of what `render`'s single
            // left-to-right scan can resolve. Failing open bought nothing and
            // cost a
            // prices.json File::open on EVERY render for any user whose
            // `[layout] format` fails to parse.
            None => false,
        }
    }

    /// Check if the template uses a specific variable.
    ///
    /// STRING-LEVEL by design: this answers exactly the question
    /// [`Self::render`] asks. `render` is ONE left-to-right scan of the UNPARSED
    /// template text that can resolve a name only where a `{` is followed by a
    /// `}`, so `self.template.contains("{name}")` is a STRUCTURAL superset of
    /// that scan's substitution set — a property of how the scan is written, not
    /// a claim re-proved against each new reproducer. It therefore reports
    /// `true` for placeholders the parser never turns into a `Variable` node —
    /// most importantly `"{{api_equiv_cost}"`, where the leading `{{` is
    /// consumed as an escaped literal yet `render` still resolves
    /// `api_equiv_cost`, because the name is taken from the LAST `{` before the
    /// closing `}`.
    ///
    /// The SEPARATOR is NOT part of that substitution surface. Since round-4
    /// CR-01, `render` resolves `{sep}` inline as a variable instead of
    /// pre-expanding it into the output buffer, so a placeholder sitting inside
    /// a user's `[layout].separator` is emitted verbatim and resolves nothing.
    /// That is why the price gate in `src/display.rs` scans only
    /// `self.template` and does not need to scan the separator.
    ///
    /// This is a LIVE input to the price-source gate in `src/display.rs`, ORed
    /// with [`Self::uses_variable_prefix`] so the gate is a provable superset of
    /// what `render` can substitute (review CR-01, round 2). It requires the
    /// BRACES, so a bare literal mention of a variable name still matches
    /// nothing and the RV-L1 / WR-07 zero-read guarantee is unaffected.
    pub fn uses_variable(&self, name: &str) -> bool {
        let placeholder = format!("{{{}}}", name);
        self.template.contains(&placeholder)
    }

    /// Get list of variables used in the template
    // Genuinely uncalled by the BINARY: library/tooling introspection helper,
    // exercised only by the colocated tests.
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
