//! Config validation core (QUAL-01 / QUAL-02).
//!
//! This module is the machinery behind the `config validate` command. It is
//! **never** reached from the render path: `Config::load()` keeps its existing
//! leniency unchanged (D-01), and nothing here runs during a render except the
//! one shared redaction helper ([`redact_toml_error`]), which is called from
//! `Config::load_from_file`'s `map_err` purely to keep credential material out
//! of an error string that already existed.
//!
//! Three properties are load-bearing and each is pinned by a test:
//!
//! 1. **Per-section deserialization (D-02).** A whole-`Config` `serde_ignored`
//!    pass is *silently blind* inside `[pricing]`, because `src/config.rs`
//!    carries `deserialize_with = "crate::pricing::deserialize_lenient"` on that
//!    field and that function opens with `toml::Value::deserialize(...)`, which
//!    consumes the entire subtree before `serde_ignored` can see it. The
//!    per-section loop deserializes `PricingConfig` directly and is therefore
//!    the only shape that reports `pricing.aliasess` at all.
//! 2. **Redaction (T-12-14 / T-12-53).** `toml`'s own diagnostics echo the
//!    offending source line AND quote the offending value inline, so a
//!    `admin_key_command = "sk-ant-…"` typo leaks an API key into a log line.
//!    `crate::utils::sanitize_for_terminal` is NOT a mitigation — it strips ANSI
//!    and control characters while preserving every printable byte. Every route
//!    from a `toml` error to a user-visible message goes through
//!    [`redact_toml_error`].
//! 3. **Context-carrying rules (D-03).** A section's `validate()` receives a
//!    [`SectionContext`] holding the *target file's* parsed document and the set
//!    of keys the user explicitly SET, so a rule can resolve a sibling section
//!    (`display.theme` for a positional `PATH`, never the process config) and
//!    can tell an explicitly-supplied default from an omitted field.

// This module is compiled into BOTH crates: `pub mod config_validation;` in
// src/lib.rs and `mod config_validation;` in src/main.rs, because the two crate
// roots carry INDEPENDENT module graphs and the binary-only
// `src/commands/config.rs` becomes a caller in plan 12-09. Until then the binary
// crate has no caller for most of this surface. Same situation, same remedy as
// `src/ant/duration.rs:18`.
#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Finding vocabulary
// ---------------------------------------------------------------------------

/// Severity of a [`Finding`].
///
/// `Error` orders BEFORE `Warning` on purpose: [`Report::sort_findings`] sorts
/// on this, so errors lead the report and the `--json` array (plan 12-09).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// A real problem. Affects the exit code (D-11).
    Error,
    /// Advisory. Never affects the exit code unless `--strict` is passed (D-11).
    Warning,
}

impl Severity {
    /// Stable machine token, used as a `--json` field value.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// Closed vocabulary of finding categories.
///
/// This is the `--json` consumer's filter key, so it is deliberately small and
/// stable. `ConfigIo` covers every way the *target file itself* can be
/// unusable — absent, unreadable, a directory, over the size cap — so plan
/// 12-09 never has to borrow `SyntaxError` for a filesystem problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindingKind {
    /// The document is not parseable TOML at all.
    SyntaxError,
    /// A key the deserializer ignored — almost always a typo (D-09: an ERROR).
    UnknownKey,
    /// A value of the wrong TOML type for its field.
    TypeError,
    /// Right type, unacceptable value (out of range, unknown enum, unparseable
    /// duration, unresolvable theme/preset/alias target).
    InvalidValue,
    /// A cache the config governs exists but is older than its threshold.
    StaleCache,
    /// A cache the config governs exists but cannot be read or parsed.
    UnreadableCache,
    /// A config file sits somewhere the resolver never consults (D-10).
    MisplacedConfig,
    /// The target file could not be read at all.
    ConfigIo,
}

impl FindingKind {
    /// Stable snake_case machine token, used as a `--json` field value.
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::SyntaxError => "syntax_error",
            FindingKind::UnknownKey => "unknown_key",
            FindingKind::TypeError => "type_error",
            FindingKind::InvalidValue => "invalid_value",
            FindingKind::StaleCache => "stale_cache",
            FindingKind::UnreadableCache => "unreadable_cache",
            FindingKind::MisplacedConfig => "misplaced_config",
            FindingKind::ConfigIo => "config_io",
        }
    }
}

/// One validation result.
///
/// `key` is a dotted config path (`display.theme`, `pricing.aliasess`) — the
/// locator this command exists to provide, and the ONLY thing derived from the
/// user's file that may appear in output. Config VALUES never do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Error or warning.
    pub severity: Severity,
    /// Category — the `--json` filter key.
    pub kind: FindingKind,
    /// Dotted key path, already passed through [`redact_key_path`].
    pub key: String,
    /// Human sentence. Must contain no config VALUE text.
    pub message: String,
    /// Reserved seam for the deferred `--suggest` idea; `None` today.
    pub hint: Option<String>,
}

/// An advisory that is neither an error nor a warning.
///
/// Notices exist so D-06's "the positional PATH is not the active config" and
/// D-10's "no config found, using defaults" advice have a home inside the
/// report. Printing them as a stray stdout line would break plan 12-09's
/// single-JSON-document contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Stable machine token (e.g. `"no_config_found"`).
    pub code: &'static str,
    /// Human sentence.
    pub message: String,
}

/// Collected findings plus notices for one validation run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Every finding, in insertion order until [`Report::sort_findings`] runs.
    pub findings: Vec<Finding>,
    /// Advisories that carry no severity.
    pub notices: Vec<Notice>,
}

impl Report {
    /// An empty report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an error-severity finding.
    pub fn error(&mut self, kind: FindingKind, key: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Error, kind, key, message, None);
    }

    /// Record a warning-severity finding.
    pub fn warn(&mut self, kind: FindingKind, key: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Warning, kind, key, message, None);
    }

    /// Record an error-severity finding carrying a remediation hint.
    pub fn error_with_hint(
        &mut self,
        kind: FindingKind,
        key: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.push(Severity::Error, kind, key, message, Some(hint.into()));
    }

    /// Record a warning-severity finding carrying a remediation hint.
    pub fn warn_with_hint(
        &mut self,
        kind: FindingKind,
        key: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.push(Severity::Warning, kind, key, message, Some(hint.into()));
    }

    /// Record a severity-free advisory.
    pub fn notice(&mut self, code: &'static str, message: impl Into<String>) {
        self.notices.push(Notice {
            code,
            message: message.into(),
        });
    }

    fn push(
        &mut self,
        severity: Severity,
        kind: FindingKind,
        key: impl Into<String>,
        message: impl Into<String>,
        hint: Option<String>,
    ) {
        self.findings.push(Finding {
            severity,
            kind,
            key: key.into(),
            message: message.into(),
            hint,
        });
    }

    /// Any error-severity finding present?
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// Any warning-severity finding present?
    pub fn has_warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == Severity::Warning)
    }

    /// Number of error-severity findings.
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    /// Number of warning-severity findings.
    pub fn warning_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// Sort findings into a TOTAL, process-independent order.
    ///
    /// The key is `(severity, key, kind, message)`. Totality is the point: a
    /// report assembled from `HashMap` iteration (free-form `[pricing.aliases]`
    /// / `[ant.accounts]` tables) or from `read_dir` enumeration (the user
    /// preset directory) must serialize identically across processes.
    pub fn sort_findings(&mut self) {
        self.findings.sort_by(|a, b| {
            (a.severity, &a.key, a.kind.as_str(), &a.message).cmp(&(
                b.severity,
                &b.key,
                b.kind.as_str(),
                &b.message,
            ))
        });
    }
}

// ---------------------------------------------------------------------------
// SectionContext
// ---------------------------------------------------------------------------

/// What a section's [`Validate`] impl is handed.
///
/// It carries three things a bare `&self` cannot supply:
///
/// * the section's dotted **prefix**, so nested structs compose in one line;
/// * the **target file's** whole parsed document, so a rule can resolve a
///   sibling section — `LayoutConfig`'s colour rules need `display.theme` of the
///   file under validation, NEVER `get_config()`'s, which would validate the
///   running process's theme for a positional `PATH`;
/// * the set of keys the user **explicitly set**, so a defaulted field such as
///   `PricingConfig::max_age` (a `String` that is `"30d"` whether written or
///   omitted) can still be told apart after deserialization.
///
/// `prefix` and `present_keys` are [`Cow`] rather than plain borrows because
/// [`SectionContext::child`] must synthesize an extended prefix and a narrowed
/// key set that outlive the `&self` it is called on.
#[derive(Debug, Clone)]
pub struct SectionContext<'a> {
    prefix: Cow<'a, str>,
    present_keys: Cow<'a, BTreeSet<String>>,
    document: &'a toml::Value,
}

impl<'a> SectionContext<'a> {
    /// Build a context for a top-level section.
    ///
    /// `present_keys` holds dotted paths RELATIVE to the section.
    pub fn new(
        prefix: &'a str,
        present_keys: &'a BTreeSet<String>,
        document: &'a toml::Value,
    ) -> Self {
        Self {
            prefix: Cow::Borrowed(prefix),
            present_keys: Cow::Borrowed(present_keys),
            document,
        }
    }

    /// This section's dotted prefix (`"display"`, `"layout.components.git"`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Full dotted key for a field relative to this section.
    pub fn key(&self, relative: &str) -> String {
        format!("{}.{}", self.prefix, relative)
    }

    /// Did the user EXPLICITLY write this field in the target file?
    ///
    /// `relative` is dotted and relative to this section.
    pub fn is_set(&self, relative: &str) -> bool {
        self.present_keys.contains(relative)
    }

    /// A context for a nested struct, one line at the call site.
    pub fn child(&self, relative: &str) -> SectionContext<'a> {
        let needle = format!("{}.", relative);
        let narrowed: BTreeSet<String> = self
            .present_keys
            .iter()
            .filter_map(|k| k.strip_prefix(&needle).map(|s| s.to_string()))
            .collect();
        SectionContext {
            prefix: Cow::Owned(format!("{}.{}", self.prefix, relative)),
            present_keys: Cow::Owned(narrowed),
            document: self.document,
        }
    }

    /// The WHOLE parsed document of the TARGET file.
    pub fn document(&self) -> &toml::Value {
        self.document
    }

    /// Read a dotted path out of the target document as a string.
    ///
    /// Use this — never `crate::config::get_config()` — for cross-section
    /// lookups, so a positional `PATH` is validated against its own contents.
    pub fn target_string(&self, dotted: &str, default: &str) -> String {
        let mut cur = self.document;
        for segment in dotted.split('.') {
            match cur.get(segment) {
                Some(next) => cur = next,
                None => return default.to_string(),
            }
        }
        cur.as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| default.to_string())
    }
}

// ---------------------------------------------------------------------------
// The Validate trait
// ---------------------------------------------------------------------------

/// Semantic validation for one config section, colocated with the struct (D-03).
///
/// The default body is empty so a section that has no rules yet — or none at
/// all — needs only `impl Validate for X {}`. That is what lets plans 12-05 and
/// 12-06 fill in different sections in parallel without either of them editing
/// this file.
///
/// # Contract for implementors
///
/// An implementation MUST be cheap and side-effect free:
///
/// * **No network.** Ever. `config validate` is an offline command.
/// * **No process spawning.** Never `std::process::Command`, never a shell.
///   `[ant.accounts.*].admin_key_command` is argv the user configured; this
///   command validates its SHAPE and must never execute it.
/// * **No IO beyond the cheap, side-effect-free lookups D-04 permits** — a
///   `metadata()`/`exists()` probe on a cache path or the user preset directory,
///   and reads of already-embedded data (`Theme::embedded_themes()`, the bundled
///   price table). No writes, no directory creation, no cache refresh.
///
/// Findings go into `report`; the method never returns a `Result` and can never
/// short-circuit, so one bad field cannot hide the next.
pub trait Validate {
    /// Append findings for this section to `report`.
    fn validate(&self, _cx: &SectionContext, _report: &mut Report) {}
}

// ---------------------------------------------------------------------------
// Redaction boundary
// ---------------------------------------------------------------------------

/// Hard cap on any message or key path this module emits.
const MAX_MESSAGE_CHARS: usize = 200;

/// Hard cap on one dotted segment of a key path.
const MAX_KEY_SEGMENT_CHARS: usize = 64;

/// Hard cap on how many legal enum variants [`legal_variants`] will recover.
const MAX_LEGAL_VARIANTS: usize = 32;

/// Hard cap on the length of ONE recovered legal enum variant.
const MAX_VARIANT_CHARS: usize = 40;

/// The marker serde puts in front of an enum's legal-variant list.
const EXPECTED_ONE_OF: &str = "expected one of ";

/// Recover the SCHEMA-derived legal-variant list out of a serde unknown-variant
/// message, splitting it away from the user's own value.
///
/// # Why this exists
///
/// serde formats an unknown variant as
/// ``unknown variant `bunlded`, expected one of `auto`, `bundled`, `synced` ``.
/// The first backtick run is the USER's value; everything after
/// [`EXPECTED_ONE_OF`] comes from the enum's compile-time `VARIANTS` constant —
/// the SCHEMA, never the document. [`redact_after_first_delimiter`] cannot
/// tell the two apart and removes both, which left `config validate` answering a
/// `source = "bunlded"` typo with
/// `unknown variant "<redacted>", expected one of "<redacted>", "<redacted>",
/// "<redacted>"` — a diagnostic that names neither what is wrong nor what is
/// legal, in the one command whose entire purpose is to say so. A section whose
/// deserialization fails never runs its semantic pass, so for `[pricing].source`
/// this message is the ONLY thing the user is ever told.
///
/// # Why it cannot leak
///
/// Three independent bounds, in the order they bite:
///
/// 1. the split is at the **LAST** occurrence of the marker, so a value that
///    contains the marker text itself lands wholly in the discarded head;
/// 2. the tail must parse as NOTHING BUT a comma-separated list of backtick
///    runs — any stray character and the whole recovery is abandoned;
/// 3. each run must match a conservative identifier allowlist (ASCII lowercase,
///    digits, `_`, `-`), be non-empty and at most [`MAX_VARIANT_CHARS`] long,
///    with at most [`MAX_LEGAL_VARIANTS`] of them.
///
/// The head — which is where the user's value lives — is returned UNREDACTED
/// and is redacted by the caller exactly as before.
///
/// Returns `(head, variants)`, or `None` when this is not an unknown-variant
/// message or the tail does not match the expected shape.
fn legal_variants(first_line: &str) -> Option<(&str, Vec<String>)> {
    let idx = first_line.rfind(EXPECTED_ONE_OF)?;
    let head = &first_line[..idx];
    let mut rest = first_line[idx + EXPECTED_ONE_OF.len()..].trim();

    let mut variants: Vec<String> = Vec::new();
    while !rest.is_empty() {
        let after_open = rest.strip_prefix('`')?;
        let close = after_open.find('`')?;
        let token = &after_open[..close];
        if token.is_empty()
            || token.chars().count() > MAX_VARIANT_CHARS
            || !token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return None;
        }
        variants.push(token.to_string());
        if variants.len() > MAX_LEGAL_VARIANTS {
            return None;
        }
        rest = after_open[close + 1..].trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
        rest = rest.strip_prefix("or ").unwrap_or(rest).trim_start();
    }

    if variants.is_empty() {
        return None;
    }
    Some((head, variants))
}

/// The marker serde puts in front of the free-prose `expected …` clause it
/// appends to a type / value / unknown-field error.
const EXPECTED_CLAUSE: &str = ", expected ";

/// Hard cap on the length of a recovered free-prose `expected …` clause.
const MAX_EXPECTED_PROSE_CHARS: usize = 64;

/// The opening of the closed `duplicate key` shape.
const DUPLICATE_KEY_OPEN: &str = "duplicate key `";

/// The separator between the duplicated key and its table in that shape.
const IN_TABLE: &str = "` in table `";

/// Hard cap on how many grammar tokens [`expected_grammar_tokens`] recovers.
const MAX_GRAMMAR_TOKENS: usize = 8;

/// The literal that stands in for everything [`redact_after_first_delimiter`]
/// removes.
const REDACTED: &str = "\"<redacted>\"";

/// The closed set of grammar tokens `toml`'s parser names in an `expected …`
/// syntax diagnostic.
///
/// This is a compile-time table and [`expected_grammar_tokens`] emits an
/// ELEMENT OF THE TABLE, never the matched input. That is what makes that
/// recovery leak-proof by construction rather than by allowlist: every byte it
/// returns is a `&'static str`, so no byte of it can have come from the
/// document. An unrecognized token aborts the whole recovery.
const TOML_GRAMMAR_TOKENS: &[&str] = &[
    ".",
    "=",
    ",",
    "#",
    "[",
    "[[",
    "]",
    "]]",
    "{",
    "}",
    "\"",
    "'",
    "newline",
    "whitespace",
    "key",
    "value",
    "string",
    "basic string",
    "literal string",
    "boolean",
    "integer",
    "float",
    "array",
    "inline table",
    "table header",
];

/// Recover the SCHEMA-derived free-prose `expected …` clause serde appends to a
/// type, value or unknown-field error, splitting it away from the user's value.
///
/// # Why this exists
///
/// serde's formats END with the expectation — `invalid type: {unexp}, expected
/// {exp}` — where `{exp}` is the `Visitor::expecting` prose ("a sequence", "a
/// string"), compile-time text and the only actionable half of the message.
/// [`redact_after_first_delimiter`] necessarily discards it, because the user's
/// value comes first; recovering it here is what keeps `invalid type: string
/// "<redacted>", expected a sequence` from collapsing to `invalid type: string
/// "<redacted>"`.
///
/// # Why it cannot leak
///
/// Four independent bounds, in the order they bite:
///
/// 1. the split is at the **LAST** occurrence of [`EXPECTED_CLAUSE`], and
///    serde's own clause is by construction the last thing in the message, so a
///    value that contains the marker text lands wholly in the discarded head;
/// 2. the tail must contain **no delimiter at all** and nothing outside a
///    conservative prose allowlist (ASCII alphanumerics, space, `,`, `.`, `-`,
///    `_`), so it can neither re-open a quoted run nor smuggle a control byte;
/// 3. the tail is at most [`MAX_EXPECTED_PROSE_CHARS`] characters;
/// 4. **decisive:** the tail is rejected outright if it occurs ANYWHERE in the
///    config source. Whatever survives that check provably is not a byte
///    sequence taken from the user's document — which is exactly the contract
///    [`redact_toml_error`] states. The cost of a false positive is a less
///    helpful message, never a leak.
fn expected_prose<'a>(first_line: &'a str, source: &str) -> Option<(&'a str, &'a str)> {
    let idx = first_line.rfind(EXPECTED_CLAUSE)?;
    let head = &first_line[..idx];
    let tail = first_line[idx + EXPECTED_CLAUSE.len()..].trim();
    if tail.is_empty()
        || tail.chars().count() > MAX_EXPECTED_PROSE_CHARS
        || !tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | ',' | '.' | '-' | '_'))
        || source.contains(tail)
    {
        return None;
    }
    Some((head, tail))
}

/// Recover the TOML GRAMMAR token list out of a syntax diagnostic whose WHOLE
/// first line is of the shape ``expected `.`, `=` ``.
///
/// # Why this exists
///
/// `toml`'s parser names what it wanted in backticks, and those names come from
/// the GRAMMAR, never the document. Redacting them left
/// `expected "<redacted>", "<redacted>"`, a diagnostic that says nothing at all.
///
/// # Why it cannot leak
///
/// 1. the shape is anchored at the START of the line, so no user value can
///    precede the marker and ride out on a mis-split;
/// 2. the remainder must parse as NOTHING BUT a comma/`or`-separated list of
///    backtick runs — any stray character abandons the recovery;
/// 3. every token must be an element of [`TOML_GRAMMAR_TOKENS`], and the
///    `&'static str` FROM THE TABLE is what is emitted. The matched input is
///    used only as a lookup key and is then dropped, so the returned bytes are
///    compile-time constants by construction.
fn expected_grammar_tokens(first_line: &str) -> Option<Vec<&'static str>> {
    let mut rest = first_line.strip_prefix("expected ")?.trim();
    let mut tokens: Vec<&'static str> = Vec::new();
    while !rest.is_empty() {
        if tokens.len() == MAX_GRAMMAR_TOKENS {
            return None;
        }
        let after_open = rest.strip_prefix('`')?;
        let close = after_open.find('`')?;
        let known = TOML_GRAMMAR_TOKENS
            .iter()
            .find(|candidate| **candidate == &after_open[..close])?;
        tokens.push(known);
        rest = after_open[close + 1..].trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
        rest = rest.strip_prefix("or ").unwrap_or(rest).trim_start();
    }
    if tokens.is_empty() {
        return None;
    }
    Some(tokens)
}

/// A bare, unquoted TOML key token: non-empty, bounded, and built only from the
/// characters a bare key may contain plus the `.` of a dotted path.
fn is_plain_key_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().count() <= MAX_KEY_SEGMENT_CHARS
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Rebuild the closed `duplicate key` shape with its two locators intact.
///
/// # Why this exists
///
/// A duplicate key is reported with the pseudo-key `<file>`, so the MESSAGE is
/// the only locator the user gets, and blanket redaction reduced it to
/// `duplicate key "<redacted>" in table "<redacted>"` — "some key is duplicated
/// somewhere", from the one command whose purpose is to say which.
///
/// # Why it cannot leak a VALUE
///
/// 1. the shape is anchored at BOTH ends — the line must START with
///    [`DUPLICATE_KEY_OPEN`] and END with a backtick — so no free text rides
///    along;
/// 2. the key/table split is at the LAST [`IN_TABLE`] marker and BOTH halves
///    must then satisfy [`is_plain_key_token`]; a half containing a delimiter,
///    a space or any other punctuation abandons the whole recovery, which is
///    what catches the mis-split a hostile QUOTED key would otherwise cause;
/// 3. both halves are emitted through [`redact_key_path`] — the module's
///    sanctioned channel for key text — so they are control-stripped and capped
///    exactly as every `unknown key` finding's key already is;
/// 4. a TOML *key* is not a *value*. `toml` names only the duplicated KEY and
///    its TABLE in this clause; the duplicated value never appears in it. Key
///    paths are permitted output by this module's doctrine (see
///    [`redact_key_path`] and [`Finding::key`]), so this opens no channel that
///    `unknown key` findings do not already open by design.
fn duplicate_key_clause(first_line: &str) -> Option<String> {
    let inner = first_line
        .strip_prefix(DUPLICATE_KEY_OPEN)?
        .strip_suffix('`')?;
    match inner.rfind(IN_TABLE) {
        Some(idx) => {
            let key = &inner[..idx];
            let table = &inner[idx + IN_TABLE.len()..];
            if !is_plain_key_token(key) || !is_plain_key_token(table) {
                return None;
            }
            Some(format!(
                "duplicate key `{}` in table `{}`",
                redact_key_path(key),
                redact_key_path(table)
            ))
        }
        None => {
            if !is_plain_key_token(inner) {
                return None;
            }
            Some(format!("duplicate key `{}`", redact_key_path(inner)))
        }
    }
}

/// Turn a `toml` parser diagnostic into a message that leaks nothing.
///
/// # Contract
///
/// **The returned string contains no byte sequence taken from `source` other
/// than a line/column number and — for the one closed `duplicate key` shape —
/// a config KEY PATH, which this module's doctrine explicitly permits in output
/// (see [`redact_key_path`] and [`Finding::key`]). A config VALUE never
/// appears.** This is pinned by sentinel tests covering BOTH parser paths,
/// because both leak:
///
/// * the **syntax** path echoes the offending source line verbatim in a gutter
///   block (`3 | admin_key_command = ["security", "-w", "sk-ant-…"`);
/// * the **deserialization** path quotes the offending VALUE inline
///   (``invalid type: string "sk-ant-…", expected a sequence``), so truncating
///   the gutter alone is insufficient.
///
/// The construction is therefore:
///
/// 1. take the error's own message text and cut it at the first newline (kills
///    the gutter block);
/// 2. if the line is a fully-recognized CLOSED shape — [`duplicate_key_clause`]
///    or [`expected_grammar_tokens`] — rebuild it whole from shape-checked
///    pieces and skip redaction entirely, because nothing of the user's VALUE
///    can appear in those shapes;
/// 3. otherwise split a SCHEMA-derived tail off the head, if one is present and
///    passes its reader's bounds — [`legal_variants`] for an enum's legal set,
///    [`expected_prose`] for serde's `expecting` text — so that the tail
///    survives step 4;
/// 4. redact the head from its FIRST quoting delimiter to its end
///    ([`redact_after_first_delimiter`]), then drop any remaining control
///    characters;
/// 5. re-attach the schema text recovered in step 3, spelled out;
/// 6. append ` (line L, column C)` when the error carries a span — positions
///    are not secret and are the locator the user needs;
/// 7. cap the result at [`MAX_MESSAGE_CHARS`].
///
/// Every un-redactor runs in step 2 or 3 — that is, BEFORE redaction and on the
/// raw line — and each is a narrow shape reader with explicit bounds. Nothing
/// is ever added back after step 4.
///
/// Note that `toml::de::Error::to_string()` is deliberately NOT used anywhere:
/// its `Display` impl is precisely what renders the source-line echo.
pub fn redact_toml_error(source: &str, e: &toml::de::Error) -> String {
    // Steps 1-5.
    let mut out = redact_message_body(e.message(), source);

    // Step 6: a position, if the error carries one.
    if let Some(span) = e.span() {
        let (line, column) = line_and_column(source, span.start);
        out.push_str(&format!(" (line {}, column {})", line, column));
    }

    // Step 7: bound the result.
    cap_chars(&out, MAX_MESSAGE_CHARS)
}

/// Steps 1-5 of [`redact_toml_error`], uncapped so the caller can append a
/// position before the single final [`cap_chars`].
fn redact_message_body(raw: &str, source: &str) -> String {
    // Step 1: first line of the error's own message only.
    let first_line = raw.split('\n').next().unwrap_or("").trim();

    // Step 2: fully-recognized CLOSED shapes are rebuilt whole.
    if let Some(rebuilt) = duplicate_key_clause(first_line) {
        return rebuilt;
    }
    if let Some(tokens) = expected_grammar_tokens(first_line) {
        return format!("expected {}", backtick_list(tokens.iter().copied()));
    }

    // Step 3: split the SCHEMA-derived tail (if any) away from the user's value,
    // so step 4 can redact the value without also destroying the one piece of
    // guidance the message carries.
    let (to_redact, tail) = if let Some((head, variants)) = legal_variants(first_line) {
        let list = backtick_list(variants.iter().map(String::as_str));
        (head, format!(" — the legal values are {list}"))
    } else if let Some((head, prose)) = expected_prose(first_line, source) {
        (head, format!(", expected {prose}"))
    } else {
        (first_line, String::new())
    };

    // Step 4: redact from the first delimiter to the end of the head, then drop
    // any remaining control characters.
    let mut redacted = redact_after_first_delimiter(to_redact);
    redacted.retain(|c| c == '\t' || !c.is_control());
    // Removing a trailing schema clause leaves a dangling separator.
    let redacted = redacted.trim().trim_end_matches(',').trim().to_string();

    let mut out = if redacted.is_empty() {
        "invalid TOML".to_string()
    } else {
        redacted
    };

    // Step 5: re-attach the schema text. These names are the schema's, not the
    // document's.
    out.push_str(&tail);
    out
}

/// Render a token list as `` `a`, `b`, `c` ``.
fn backtick_list<'a>(tokens: impl Iterator<Item = &'a str>) -> String {
    tokens
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Sanitize a key path for inclusion in a finding.
///
/// Key PATHS are permitted in output — they are the locator this command exists
/// to provide. Key VALUES are not. Control characters are stripped, each dotted
/// segment is capped at [`MAX_KEY_SEGMENT_CHARS`] and the whole path at
/// [`MAX_MESSAGE_CHARS`], so a hostile free-form map key (`[ant.accounts.…]`)
/// cannot smuggle terminal control bytes or unbounded text into the report.
pub fn redact_key_path(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let joined = cleaned
        .split('.')
        .map(|segment| cap_chars(segment, MAX_KEY_SEGMENT_CHARS))
        .collect::<Vec<_>>()
        .join(".");
    cap_chars(&joined, MAX_MESSAGE_CHARS)
}

/// Sanitize a message produced by ANOTHER module's validating parser.
///
/// The project's validating parsers (`crate::ant::duration::parse_max_age`,
/// `crate::ant::cache::sanitize_account_name`) are written for CLI flags and
/// deliberately quote the offending input back at the user
/// (``invalid --max-age '30x' (need a unit: s/m/h/d)``). That is exactly the
/// value echo [`Finding`] forbids, so a rule that wants to reuse one of those
/// messages — and they are worth reusing, they name the grammar — must pass it
/// through here first.
///
/// Same boundary as [`redact_toml_error`]'s step 4 and deliberately sharing its
/// [`redact_after_first_delimiter`] helper: everything from the first quoting
/// delimiter onwards is replaced by the literal `"<redacted>"`, control
/// characters are dropped, and the result is capped at [`MAX_MESSAGE_CHARS`].
/// Redaction ALWAYS runs to end of input, so the failure mode is "too much
/// removed", never "a value survived" — which is why callers append any legal-
/// set or grammar text of their own AFTER this call rather than before it.
pub fn redact_value_text(raw: &str) -> String {
    let mut redacted = redact_after_first_delimiter(raw);
    redacted.retain(|c| c == '\t' || !c.is_control());
    cap_chars(redacted.trim(), MAX_MESSAGE_CHARS)
}

/// Replace everything from the FIRST `"`, `'` or `` ` `` to the end of `input`
/// with the literal `"<redacted>"`.
///
/// # Why this shape (CR-01)
///
/// The previous implementation PAIRED delimiters: on seeing `"`, `'` or `` ` ``
/// it consumed to the next occurrence of that same character and emitted
/// `"<redacted>"`, then resumed copying. It had no notion of escaping and no
/// notion of "this whole span is one value", so any value that itself contained
/// the delimiter the diagnostic rendered it with was split into alternating
/// redacted and **un-redacted** runs — and the un-redacted runs were printed
/// verbatim, on `config validate`'s stdout and on the render path's default-
/// level `warn!`:
///
/// ```text
/// admin_key_command = "security -w sk-ant-adm\"01-REALKEYTAIL"
///   →  invalid type: string "<redacted>"01-REALKEYTAIL"<redacted>"
/// ```
///
/// # Why it cannot be split-leaked
///
/// There is no pairing and no state machine here, so there is nothing for an
/// embedded delimiter to desynchronize. The output is exactly
/// `input[..i] ++ REDACTED`, where `i` is the index of the FIRST delimiter in
/// `input` — or the whole of `input` when it contains none. Two facts follow
/// immediately:
///
/// * `input[..i]` contains no `"`, `'` or `` ` ``, by the definition of `i`;
/// * `toml` and `serde` render EVERY echoed document value delimiter-wrapped —
///   `{:?}` for strings, `` ` `` for enum variants and keys, `'…'` for this
///   project's own validating parsers — so an echoed value begins at some index
///   `≥ i` and therefore lies wholly inside the discarded suffix.
///
/// Whatever the value contains — one delimiter, a hundred of them, an escaped
/// one, or a mismatched pair — the suffix goes in a single piece. Escape
/// handling is irrelevant because no CLOSING delimiter is ever sought: the
/// function never has to decide where a run ends, only where the first one
/// begins. Exactly one `"<redacted>"` is emitted per call.
///
/// The cost is that genuinely SCHEMA-derived text positioned after the value is
/// discarded too. That is recovered BEFORE this function runs, by the
/// shape-checked readers above ([`legal_variants`], [`expected_prose`],
/// [`expected_grammar_tokens`], [`duplicate_key_clause`]) — never after.
fn redact_after_first_delimiter(input: &str) -> String {
    match input.find(['"', '\'', '`']) {
        Some(i) => {
            let mut out = String::with_capacity(i + REDACTED.len());
            out.push_str(&input[..i]);
            out.push_str(REDACTED);
            out
        }
        None => input.to_string(),
    }
}

/// 1-based line and column for a byte offset into `source`.
fn line_and_column(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for (i, byte) in source.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// Cap a string at `max` characters, appending `…` when it is shortened.
fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// The per-section engine
// ---------------------------------------------------------------------------

/// Every top-level section `Config` recognizes — all fourteen, INCLUDING
/// `"sync"` unconditionally.
///
/// `sync` is RECOGNIZED in every build and only DESERIALIZED and validated
/// under `#[cfg(feature = "turso-sync")]`. In a default-features build a
/// `[sync]` table is recognized and skipped SILENTLY: not an error, because
/// D-09 would otherwise fail a legitimate turso user's config, and not a
/// warning, because that is noise on every such config. A fourteen-element
/// array that omits `sync` is the documented failure mode — do not write one.
pub const KNOWN_SECTIONS: &[&str] = &[
    "display",
    "context",
    "cost",
    "database",
    "retry",
    "transcript",
    "git",
    "sync",
    "burn_rate",
    "layout",
    "token_rate",
    "gsd",
    "ant",
    "pricing",
];

/// Hard cap on how deep [`present_keys_for_section`] will walk.
///
/// Deeply nested tables are bounded by `toml`'s own parser, but the walk is
/// recursive and must not be the thing that overflows the stack (T-12-12).
const MAX_PRESENT_KEY_DEPTH: usize = 32;

/// Dotted paths PRESENT under `document[section]`, relative to that section.
///
/// Intermediate tables are included as well as leaves, so
/// `layout.components.git.enabled` contributes `components`,
/// `components.git` and `components.git.enabled`. This is what backs
/// [`SectionContext::is_set`] — the only way to tell an explicitly-written
/// default from an omitted field once serde has applied `#[serde(default)]`.
pub fn present_keys_for_section(document: &toml::Value, section: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(value) = document.get(section) {
        collect_present_keys(value, "", 0, &mut out);
    }
    out
}

fn collect_present_keys(
    value: &toml::Value,
    prefix: &str,
    depth: usize,
    out: &mut BTreeSet<String>,
) {
    if depth >= MAX_PRESENT_KEY_DEPTH {
        return;
    }
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, child) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };
        collect_present_keys(child, &path, depth + 1, out);
        out.insert(path);
    }
}

/// Validate one config document's TEXT, appending findings to `report`.
///
/// The pipeline (D-02):
///
/// 1. parse the whole document as a `toml::Value` — a failure is ONE
///    `SyntaxError` and a return, because nothing else is knowable;
/// 2. reject a non-table document;
/// 3. report every unrecognized top-level key as an `UnknownKey` ERROR (D-09);
/// 4. deserialize each KNOWN section's SUBTREE directly into its concrete type
///    through `serde_ignored`;
/// 5. on success report the ignored paths (PREFIXED with the section name —
///    per-section paths come back relative) and run the section's semantic
///    rules; on failure record EXACTLY ONE `TypeError` for that section, skip
///    its semantic pass, and CONTINUE to the next section.
///
/// Step 4 is load-bearing, not stylistic: `[pricing]` MUST go through
/// `PricingConfig::deserialize` directly and never through
/// `crate::pricing::deserialize_lenient`, which opens with
/// `toml::Value::deserialize(deserializer)?` and consumes the whole subtree —
/// making a whole-`Config` `serde_ignored` pass silently blind in exactly the
/// section QUAL-02 names.
pub fn validate_config_text(text: &str, report: &mut Report) {
    let document: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(e) => {
            report.error(
                FindingKind::SyntaxError,
                "<file>",
                redact_toml_error(text, &e),
            );
            return;
        }
    };

    let Some(table) = document.as_table() else {
        report.error(
            FindingKind::SyntaxError,
            "<file>",
            "config file must be a TOML table of sections",
        );
        return;
    };

    macro_rules! check_section {
        ($name:expr, $ty:ty, $sub:expr) => {{
            let mut ignored: Vec<String> = Vec::new();
            match serde_ignored::deserialize::<_, _, $ty>($sub.clone(), |path| {
                ignored.push(path.to_string())
            }) {
                Ok(section) => {
                    for relative in ignored {
                        // Per-section paths are RELATIVE (`aliasess`, not
                        // `pricing.aliasess`) — the engine must prefix.
                        report.error(
                            FindingKind::UnknownKey,
                            redact_key_path(&format!("{}.{}", $name, relative)),
                            "unknown key (not a recognized setting)",
                        );
                    }
                    let present = present_keys_for_section(&document, $name);
                    let cx = SectionContext::new($name, &present, &document);
                    section.validate(&cx, report);
                }
                Err(e) => {
                    // EXACTLY ONE finding for a failed section, keyed at the
                    // section name: the semantic pass is skipped (its input
                    // never existed), and every other section still reports.
                    report.error(FindingKind::TypeError, $name, redact_toml_error(text, &e));
                }
            }
        }};
    }

    for (name, sub) in table {
        if !KNOWN_SECTIONS.contains(&name.as_str()) {
            report.error(
                FindingKind::UnknownKey,
                redact_key_path(name),
                "unknown top-level section",
            );
            continue;
        }

        match name.as_str() {
            "display" => check_section!("display", crate::config::DisplayConfig, sub),
            "context" => check_section!("context", crate::config::ContextConfig, sub),
            "cost" => check_section!("cost", crate::config::CostConfig, sub),
            "database" => check_section!("database", crate::config::DatabaseConfig, sub),
            "retry" => check_section!("retry", crate::config::RetryConfig, sub),
            "transcript" => check_section!("transcript", crate::config::TranscriptConfig, sub),
            "git" => check_section!("git", crate::config::GitConfig, sub),
            "burn_rate" => check_section!("burn_rate", crate::config::BurnRateConfig, sub),
            "layout" => check_section!("layout", crate::config::LayoutConfig, sub),
            "token_rate" => check_section!("token_rate", crate::config::TokenRateConfig, sub),
            "gsd" => check_section!("gsd", crate::gsd::config::GsdConfig, sub),
            "ant" => check_section!("ant", crate::ant::config::AntConfig, sub),
            "pricing" => check_section!("pricing", crate::pricing::PricingConfig, sub),
            "sync" => {
                // Recognized in EVERY build; deserialized and validated only
                // where the type exists. Silence is deliberate — see
                // `KNOWN_SECTIONS`.
                #[cfg(feature = "turso-sync")]
                check_section!("sync", crate::config::SyncConfig, sub);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Cache reachability classification (QUAL-02 / SC3)
// ---------------------------------------------------------------------------

/// How reachable and how fresh one of the three config-governed caches is.
///
/// `Unparseable` is deliberately distinct from `Unreadable`: the bytes were
/// obtained, so the problem is the CONTENT (garbage, or a `schema_version` this
/// build does not accept), not access. Both produce the same severity — see
/// [`report_cache_findings`] — but the `--json` consumer can tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CacheState {
    /// Exists, parses, and is inside its configured threshold.
    Fresh,
    /// Exists and parses, but is at or beyond its configured threshold.
    Stale,
    /// Not there. The NORMAL state for a user who has never run a sync, and
    /// therefore SILENT — see [`report_cache_findings`].
    Absent,
    /// Present but not obtainable: a permission error, a non-regular file
    /// (directory / fifo / socket), over the probe cap, or an unresolvable path.
    Unreadable,
    /// Bytes obtained but not usable: not JSON, wrong `schema_version`, or a
    /// missing/unparseable `fetched_at`.
    Unparseable,
}

impl CacheState {
    /// Stable lowercase machine token, used as a `--json` field value.
    pub fn as_str(self) -> &'static str {
        match self {
            CacheState::Fresh => "fresh",
            CacheState::Stale => "stale",
            CacheState::Absent => "absent",
            CacheState::Unreadable => "unreadable",
            CacheState::Unparseable => "unparseable",
        }
    }
}

/// One cache's classification result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStatus {
    /// Stable cache identifier: `"prices"`, `"models"` or `"usage"`.
    pub name: &'static str,
    /// The classification.
    pub state: CacheState,
    /// Humanized age, rendered by the SAME `crate::ant::duration::humanize_age`
    /// `ant doctor` uses, so the two surfaces print the same string. `None`
    /// unless the cache actually yielded a `fetched_at`.
    pub age: Option<String>,
    /// The resolved cache path, or `None` when the path could not be resolved at
    /// all (never a fabricated path).
    pub path: Option<std::path::PathBuf>,
    /// A short, VALUE-FREE reason (`"not a regular file (fifo)"`,
    /// `"permission denied"`, `"schema_version mismatch"`). Never file contents.
    pub detail: Option<String>,
}

/// Hard upper bound on the bytes this module will read from a cache file.
///
/// This IS `crate::pricing::cache::MAX_PRICE_CACHE_BYTES`, not a copy of its
/// value. Plan 12-06 mirrored the literal with a comment naming the source and
/// recorded the drift risk as accepted, because the source constant was private;
/// plan 12-09 made it `pub(crate)` and bound the two together, so the probe cap
/// and the reader cap can no longer disagree. The rationale for the size
/// (bounding latency, not merely allocation) lives with the original.
///
/// Over-cap is a REJECTION, not a truncate-and-parse: reading exactly the cap
/// from an oversized file can yield a complete valid document followed by
/// padding, which would let an arbitrarily large file through the supposed bound.
const MAX_CACHE_PROBE_BYTES: u64 = crate::pricing::cache::MAX_PRICE_CACHE_BYTES;

/// The POST-parse row cap a typed cache reader enforces, as
/// `(json object field, maximum entries)`.
///
/// Only the price cache has one. `read_price_cache`
/// (`src/pricing/cache.rs:364`) checks `cache.prices.len()` AFTER
/// `serde_json::from_str` and discards the whole cache above
/// `MAX_PRICE_CACHE_ENTRIES`, so a >4096-row `prices.json` that is under the
/// byte cap parses here perfectly well while the render silently falls back to
/// the bundled table.
///
/// Plan 12-06 flagged that gap and left it open, on the grounds that the
/// alternative was calling the typed readers — which would have cost the
/// single-bounded-read guarantee and re-introduced the fifo hazard step 2 of the
/// ladder removes. It costs neither: [`classify_cache`] already parsed the one
/// buffer it read, so counting the rows in the value it is holding adds no read,
/// no open and no allocation of consequence.
fn typed_reader_row_cap(name: &str) -> Option<(&'static str, usize)> {
    match name {
        "prices" => Some(("prices", crate::pricing::cache::MAX_PRICE_CACHE_ENTRIES)),
        _ => None,
    }
}

/// A human label for a filesystem object kind. Carries no path or content.
///
/// `pub` because `config validate`'s own target-file ladder (plan 12-09,
/// `src/commands/config.rs`) must describe a non-regular target with exactly the
/// same words this classifier uses; two spellings of one fact would eventually
/// disagree.
pub fn file_type_label(ft: &std::fs::FileType) -> &'static str {
    if ft.is_dir() {
        return "directory";
    }
    if ft.is_symlink() {
        return "symlink";
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if ft.is_fifo() {
            return "fifo";
        }
        if ft.is_socket() {
            return "socket";
        }
        if ft.is_block_device() {
            return "block device";
        }
        if ft.is_char_device() {
            return "character device";
        }
    }
    "other"
}

/// A human label for an IO error KIND. Deliberately built from the kind alone:
/// `std::io::Error`'s `Display` includes the OS message but a caller-supplied
/// path never reaches it here, and no config value can.
///
/// `pub` for the same reason as [`file_type_label`]: plan 12-09's target-file
/// ladder reports IO failures with this vocabulary.
pub fn io_kind_label(kind: std::io::ErrorKind) -> String {
    match kind {
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        std::io::ErrorKind::NotFound => "not found".to_string(),
        std::io::ErrorKind::InvalidData => "not valid UTF-8".to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn cache_status(
    name: &'static str,
    state: CacheState,
    path: Option<std::path::PathBuf>,
    detail: Option<String>,
) -> CacheStatus {
    CacheStatus {
        name,
        state,
        age: None,
        path,
        detail,
    }
}

/// Classify one cache from METADATA FIRST, then at most ONE bounded read.
///
/// The order of the ladder is the whole design, and two of its steps exist
/// because the obvious shape is wrong:
///
/// 1. **`path` is `Err`** — `Unreadable` with `path: None`. (All three accessors
///    return `Result<PathBuf>`, not `Option<PathBuf>`.)
/// 2. **`std::fs::symlink_metadata`, never `Path::exists()`.** `Path::exists()`
///    answers `false` on a `PermissionDenied` metadata error — reproduced live
///    against a `chmod 000` parent directory — which would silently suppress the
///    unreadable-cache warning QUAL-02 requires. Only `ErrorKind::NotFound`
///    means `Absent`. A SYMLINK is then resolved with `std::fs::metadata` and
///    classified by what it POINTS AT, because the typed cache readers follow
///    links and a symlinked cache works today; classifying the link itself as
///    "not a regular file" would be a false warning on a working config. Both
///    calls are stats, and a stat never blocks on a fifo.
///    A non-regular target **returns without opening the file**. That is the
///    FIFO guard and it is mandatory: verified live, opening a writer-less fifo
///    blocks indefinitely (`timeout 3` -> exit 124) BEFORE a single byte is
///    read, so a byte cap does not bound it.
/// 3. **One bounded read**, capped at [`MAX_CACHE_PROBE_BYTES`] + 1 byte and
///    REJECTED above the cap.
/// 4. **Classify from that one buffer.** This is a deliberately INDEPENDENT
///    structural probe rather than a call into the typed readers in
///    `crate::pricing::cache` / `crate::ant::cache`: each of those resolves the
///    path and opens the file AGAIN, which would be both a second read and a
///    re-introduction of the fifo hazard step 2 just removed. The schema
///    constants are IMPORTED from the owning modules by the caller
///    ([`classify_all_caches`]) so the version numbers cannot drift.
/// 5. **Freshness through the single shared helper**,
///    `crate::ant::duration::is_stale` (plan 12-02), so `config validate` and
///    `ant doctor` can never disagree about staleness (D-12).
///
/// `name` is the stable identifier the resulting [`CacheStatus`] carries; the
/// plan specified it on the struct but not in this signature, and a classifier
/// that returned a nameless status would force the caller to patch the field.
///
/// Side effects: none. No write, no directory creation, no spawn, no network.
pub fn classify_cache(
    name: &'static str,
    path: crate::error::Result<std::path::PathBuf>,
    expected_schema: u32,
    threshold: &str,
) -> CacheStatus {
    // --- 1. Path resolution ------------------------------------------------
    let p = match path {
        Ok(p) => p,
        Err(e) => {
            return cache_status(
                name,
                CacheState::Unreadable,
                None,
                Some(redact_value_text(&e.to_string())),
            )
        }
    };

    // --- 2. Metadata first --------------------------------------------------
    let link_md = match std::fs::symlink_metadata(&p) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return cache_status(name, CacheState::Absent, Some(p), None)
        }
        Err(e) => {
            return cache_status(
                name,
                CacheState::Unreadable,
                Some(p),
                Some(io_kind_label(e.kind())),
            )
        }
    };

    let md = if link_md.file_type().is_symlink() {
        match std::fs::metadata(&p) {
            Ok(md) => md,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return cache_status(
                    name,
                    CacheState::Absent,
                    Some(p),
                    Some("dangling symlink".to_string()),
                )
            }
            Err(e) => {
                return cache_status(
                    name,
                    CacheState::Unreadable,
                    Some(p),
                    Some(io_kind_label(e.kind())),
                )
            }
        }
    } else {
        link_md
    };

    let ft = md.file_type();
    if !ft.is_file() {
        // RETURN WITHOUT OPENING. See the fifo note on this function.
        return cache_status(
            name,
            CacheState::Unreadable,
            Some(p),
            Some(format!("not a regular file ({})", file_type_label(&ft))),
        );
    }
    if md.len() > MAX_CACHE_PROBE_BYTES {
        return cache_status(
            name,
            CacheState::Unreadable,
            Some(p),
            Some(format!(
                "larger than the {MAX_CACHE_PROBE_BYTES} byte probe cap"
            )),
        );
    }

    // --- 3. Exactly one bounded read ---------------------------------------
    let mut buf = String::new();
    let read = std::fs::File::open(&p).and_then(|f| {
        use std::io::Read;
        f.take(MAX_CACHE_PROBE_BYTES + 1).read_to_string(&mut buf)
    });
    if let Err(e) = read {
        return cache_status(
            name,
            CacheState::Unreadable,
            Some(p),
            Some(io_kind_label(e.kind())),
        );
    }
    if buf.len() as u64 > MAX_CACHE_PROBE_BYTES {
        return cache_status(
            name,
            CacheState::Unreadable,
            Some(p),
            Some(format!(
                "larger than the {MAX_CACHE_PROBE_BYTES} byte probe cap"
            )),
        );
    }

    // --- 4. Classify from THAT ONE BUFFER ----------------------------------
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&buf) else {
        return cache_status(
            name,
            CacheState::Unparseable,
            Some(p),
            Some("not valid JSON".to_string()),
        );
    };
    match value.get("schema_version").and_then(|v| v.as_u64()) {
        Some(found) if found == u64::from(expected_schema) => {}
        _ => {
            return cache_status(
                name,
                CacheState::Unparseable,
                Some(p),
                Some("schema_version mismatch".to_string()),
            )
        }
    }
    // The typed reader's POST-parse row cap, applied to the buffer already in
    // hand. Without this a cache the reader REJECTS classifies `Fresh` — a
    // silent fallback reported as health, inside the command written to end
    // silent fallbacks.
    if let Some((field, cap)) = typed_reader_row_cap(name) {
        let rows = value
            .get(field)
            .and_then(|v| v.as_object())
            .map(|m| m.len())
            .unwrap_or(0);
        if rows > cap {
            return cache_status(
                name,
                CacheState::Unparseable,
                Some(p),
                Some(format!(
                    "more than {cap} rows; the reader discards it after parsing and the render \
                     falls back to the bundled table"
                )),
            );
        }
    }

    let Some(fetched_at) = value
        .get("fetched_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&chrono::Utc))
    else {
        return cache_status(
            name,
            CacheState::Unparseable,
            Some(p),
            Some("fetched_at is missing or not an RFC3339 timestamp".to_string()),
        );
    };

    // --- 5. Freshness via the SINGLE shared helper (D-12) -------------------
    let age = chrono::Utc::now().signed_duration_since(fetched_at);
    let state = if crate::ant::duration::is_stale(age, threshold) {
        CacheState::Stale
    } else {
        CacheState::Fresh
    };
    CacheStatus {
        name,
        state,
        age: Some(crate::ant::duration::humanize_age(age)),
        path: Some(p),
        detail: None,
    }
}

/// Classify all three config-governed caches (D-12).
///
/// Thresholds come from the config being validated: `[pricing].max_age` for
/// `prices`, `[ant].models_stale_after` for `models`, `[ant].usage_stale_after`
/// for `usage`.
///
/// # The active account
///
/// The `usage` cache is PER-ACCOUNT, and the active account is the one input
/// `Config` does not carry. It is resolved here with the same passive
/// `STATUSLINE_ANT_ACCOUNT` read the render path performs (`src/display.rs`) and
/// `ant doctor` performs (`src/commands/ant.rs`), empty-string filter included.
/// There is deliberately no `account` parameter and no `--account` override:
/// that flag belongs to the WRITER (`ant sync-usage`) and has no meaning for a
/// passive diagnostic. An env read is not a spawn and not a probe.
///
/// When the variable is unset or empty the usage cache is NOT CLASSIFIED AT ALL
/// and the returned `Vec` carries no `usage` element, mirroring the render path,
/// which inserts nothing in that case. Plan 12-09's `caches` object therefore
/// carries a row per RETURNED status, so `caches.usage` simply reads as absent.
///
/// The usage path is built ONLY through `crate::ant::cache::usage_cache_path`,
/// which runs `sanitize_account_name` BEFORE the join — the account name is
/// untrusted. Its `Err` flows into ladder step 1 and yields `Unreadable` with
/// `path: None`, never a fabricated path.
///
/// # What is deliberately NOT mirrored
///
/// The render path's account read is copied; its enrichment gate is NOT.
/// `config validate` reports all three caches whether or not `[ant]` enrichment
/// is switched on (D-12 puts all three in its domain), because gating would hide
/// a stale cache from precisely the user who just toggled enrichment off and is
/// trying to work out why. That toggle gets its own cross-field warning in
/// `impl Validate for AntConfig`; it is not a filter here.
pub fn classify_all_caches(cfg: &crate::config::Config) -> Vec<CacheStatus> {
    let mut out = Vec::with_capacity(3);

    out.push(classify_cache(
        "prices",
        crate::pricing::cache::price_cache_path(),
        crate::pricing::cache::PRICE_CACHE_SCHEMA_VERSION,
        &cfg.pricing.max_age,
    ));
    out.push(classify_cache(
        "models",
        crate::ant::cache::models_cache_path(),
        crate::ant::cache::MODELS_CACHE_SCHEMA_VERSION,
        &cfg.ant.models_stale_after,
    ));

    if let Some(account) = std::env::var("STATUSLINE_ANT_ACCOUNT")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let path = crate::ant::cache::usage_cache_path(&account).map_err(|e| {
            crate::error::StatuslineError::Config(format!("STATUSLINE_ANT_ACCOUNT rejected: {e}"))
        });
        out.push(classify_cache(
            "usage",
            path,
            crate::ant::cache::USAGE_CACHE_SCHEMA_VERSION,
            &cfg.ant.usage_stale_after,
        ));
    }

    out
}

/// Turn cache statuses into findings — WARNINGS only.
///
/// # The settled absent-cache policy
///
/// `Absent` and `Fresh` emit **nothing**. A missing cache is the normal state
/// for a user who has never run a sync, and warning about it would make the
/// default experience noisy. This supersedes any contrary wording elsewhere in
/// the phase's artifacts; plans 12-10 and 12-12 and the documentation can cite
/// this one answer.
///
/// # Why never an ERROR
///
/// D-11 makes warnings exit-code neutral by default, and SC3 requires stale or
/// unreachable caches NOT to fail validation. `--strict` is the opt-in that
/// promotes them, which is a decision for the command layer, not for this
/// function.
///
/// Messages use [`CacheStatus::detail`] and the cache NAME. They never include
/// file contents.
pub fn report_cache_findings(statuses: &[CacheStatus], report: &mut Report) {
    for status in statuses {
        let key = format!("caches.{}", status.name);
        match status.state {
            CacheState::Fresh | CacheState::Absent => {}
            CacheState::Stale => {
                let age = status.age.as_deref().unwrap_or("unknown");
                report.warn(
                    FindingKind::StaleCache,
                    key,
                    format!(
                        "the {} cache is {} old, at or beyond its configured staleness threshold",
                        status.name, age
                    ),
                );
            }
            CacheState::Unreadable | CacheState::Unparseable => {
                let detail = status.detail.as_deref().unwrap_or("unknown reason");
                report.warn(
                    FindingKind::UnreadableCache,
                    key,
                    format!(
                        "the {} cache is {} ({})",
                        status.name,
                        status.state.as_str(),
                        detail
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unclosed array whose last element is the sentinel — the SYNTAX path.
    pub(super) const SENTINEL_SYNTAX: &str = concat!(
        "[ant.accounts.work]\n",
        "admin_key_command = [\"security\", \"-w\", \"sk-ant-SENTINEL-AAA\"\n"
    );

    /// A string where a sequence is expected — the DESERIALIZATION path.
    ///
    /// The nesting level matters: at `[ant]` level `admin_key_command` is merely
    /// an unknown key, never reaches the deserializer, and leaks nothing. A
    /// fixture placed there proves nothing.
    pub(super) const SENTINEL_TYPE: &str = concat!(
        "[ant.accounts.work]\n",
        "admin_key_command = \"sk-ant-SENTINEL-BBB\"\n"
    );

    #[test]
    fn redaction_removes_config_values_from_syntax_errors() {
        let err = toml::from_str::<toml::Value>(SENTINEL_SYNTAX)
            .expect_err("unclosed array must fail to parse");

        // Non-vacuity: the RAW diagnostic really does carry the secret.
        let raw = format!("{}", err);
        assert!(
            raw.contains("SENTINEL"),
            "fixture is not exercising the leak; raw error was: {raw}"
        );

        let redacted = redact_toml_error(SENTINEL_SYNTAX, &err);
        assert!(
            !redacted.contains("SENTINEL"),
            "redacted message leaked the sentinel: {redacted}"
        );
        assert!(!redacted.is_empty(), "redacted message must not be empty");
        assert!(
            redacted.contains("line") && redacted.contains("column"),
            "redacted message must keep a position locator: {redacted}"
        );
    }

    #[test]
    fn redaction_removes_config_values_from_type_errors() {
        let err = toml::from_str::<crate::config::Config>(SENTINEL_TYPE)
            .expect_err("a string where a sequence is expected must fail");

        let raw = format!("{}", err);
        assert!(
            raw.contains("SENTINEL"),
            "fixture is not exercising the leak; raw error was: {raw}"
        );

        let redacted = redact_toml_error(SENTINEL_TYPE, &err);
        assert!(
            !redacted.contains("SENTINEL"),
            "redacted message leaked the sentinel: {redacted}"
        );
        assert!(
            redacted.contains("invalid type"),
            "redacted message must still name the failure: {redacted}"
        );
    }

    /// A failed section's ONE finding must still name the legal set.
    ///
    /// `[pricing]` has no semantic rule for `source`, and a section whose
    /// deserialization fails never runs its semantic pass, so this message is
    /// the ONLY thing the user is ever told about a `source` typo. Before
    /// [`legal_variants`] it read `unknown variant "<redacted>", expected one of
    /// "<redacted>", "<redacted>", "<redacted>"` — the redaction boundary could
    /// not distinguish the user's value from the enum's own `VARIANTS`.
    #[test]
    fn unknown_variant_message_names_the_legal_set_without_echoing_the_value() {
        // Deserialized as the SECTION, which is what the engine does.
        // `Config`'s own `pricing` field is `deserialize_lenient` and swallows
        // this error entirely — that blindness is the whole reason the engine
        // walks sections instead of the document.
        const BODY: &str = "source = \"bunlded\"\n";
        let err = toml::from_str::<crate::pricing::PricingConfig>(BODY)
            .expect_err("an unknown enum variant must fail to deserialize");

        // Non-vacuity: the RAW diagnostic carries BOTH the user's value and the
        // legal set, which is exactly why they must be separated.
        let raw = format!("{}", err);
        assert!(
            raw.contains("bunlded") && raw.contains("bundled"),
            "fixture is not exercising the split; raw error was: {raw}"
        );

        let redacted = redact_toml_error(BODY, &err);
        for legal in ["auto", "bundled", "synced"] {
            assert!(
                redacted.contains(&format!("`{legal}`")),
                "the message must name the legal value `{legal}`: {redacted}"
            );
        }
        assert!(
            !redacted.contains("bunlded"),
            "the message must never echo the user's value: {redacted}"
        );
        assert!(
            redacted.contains("unknown variant"),
            "the message must still name the failure: {redacted}"
        );
    }

    /// The recovery is a narrow, shape-checked reader — not a general
    /// un-redactor. Each row here is a way a hostile value could try to ride the
    /// `expected one of` clause out of the redaction boundary.
    #[test]
    fn legal_variant_recovery_rejects_everything_but_a_clean_variant_list() {
        // The real shape serde emits.
        let (head, variants) =
            legal_variants("unknown variant `x`, expected one of `auto`, `bundled`, `synced`")
                .expect("serde's own shape must be recognized");
        assert_eq!(head, "unknown variant `x`, ");
        assert_eq!(variants, vec!["auto", "bundled", "synced"]);

        // A value containing the marker cannot steer the split: `rfind` takes
        // the LAST marker, which is always the schema-generated one.
        let (head, variants) = legal_variants(
            "unknown variant `expected one of `sk-ant-LEAK``, expected one of `auto`, `bundled`",
        )
        .expect("the trailing schema clause is still the one that is taken");
        assert!(
            head.contains("sk-ant-LEAK"),
            "the hostile value must land in the head, where the caller redacts it: {head}"
        );
        assert_eq!(variants, vec!["auto", "bundled"]);

        for hostile in [
            // Not an unknown-variant message at all.
            "invalid type: string \"sk-ant-LEAK\", expected a sequence",
            // Tail is not a pure backtick list.
            "expected one of `auto` and maybe `sk-ant-LEAK`",
            // A token outside the identifier allowlist.
            "expected one of `auto`, `sk-ant-LEAK!`",
            "expected one of `AUTO`",
            // Unterminated run.
            "expected one of `auto",
            // Empty token, and an empty list.
            "expected one of ``",
            "expected one of ",
        ] {
            assert!(
                legal_variants(hostile).is_none(),
                "recovery must be abandoned for {hostile:?}"
            );
        }

        // Over-long and over-many are both refused.
        let long = "a".repeat(MAX_VARIANT_CHARS + 1);
        assert!(legal_variants(&format!("expected one of `{long}`")).is_none());
        let many = (0..=MAX_LEGAL_VARIANTS)
            .map(|i| format!("`v{i}`"))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(legal_variants(&format!("expected one of {many}")).is_none());
    }

    /// CR-01: a value that CARRIES the delimiter its diagnostic renders it with
    /// must not be split into alternating redacted and un-redacted runs.
    ///
    /// # Why the phase's existing sentinels cannot see this
    ///
    /// Every sentinel elsewhere in this phase is of the form
    /// `sk-ant-SENTINEL-XXX`, which contains no `"`, `'` or `` ` ``. The old
    /// delimiter-PAIRING scanner therefore always saw such a value as one
    /// well-formed run and redacted it whole. The fixtures below put the
    /// delimiter INSIDE the value, which is what desynchronized the pairing:
    ///
    /// ```text
    /// admin_key_command = "security -w sk-ant-SEN\"TINEL-JJJ"
    ///   → invalid type: string "<redacted>"TINEL-JJJ"<redacted>"
    /// ```
    ///
    /// # Non-vacuity
    ///
    /// Each arm asserts the RAW diagnostic really does carry the fragment
    /// before asserting the redacted one does not, so a future `toml`/`serde`
    /// release that stops echoing values cannot make this pass hollowly. Run
    /// against the pre-fix pairing scanner, every arm FAILS on its fragment
    /// assertion.
    #[test]
    fn a_delimiter_inside_the_value_cannot_split_the_redaction_boundary() {
        // --- Arm 1: DOUBLE QUOTE, through serde's `{:?}` escaping ------------
        // The TOML value is `security -w sk-ant-SEN"TINEL-JJJ`; serde escapes
        // the interior quote as `\"`, and the old scanner closed its run there.
        const DQ: &str =
            "[ant.accounts.work]\nadmin_key_command = \"security -w sk-ant-SEN\\\"TINEL-JJJ\"\n";
        let err = toml::from_str::<crate::config::Config>(DQ)
            .expect_err("a string where a sequence is expected must fail");
        let raw = format!("{err}");
        assert!(
            raw.contains("TINEL-JJJ"),
            "fixture is not exercising the leak; raw error was: {raw}"
        );
        let redacted = redact_toml_error(DQ, &err);
        assert!(
            !redacted.contains("TINEL-JJJ"),
            "double quote: a FRAGMENT of the value survived redaction: {redacted}"
        );
        assert!(
            !redacted.contains("sk-ant-"),
            "double quote: a secret-shaped prefix survived redaction: {redacted}"
        );
        assert!(
            redacted.contains("invalid type"),
            "double quote: the failure must still be named: {redacted}"
        );

        // --- Arm 2: BACKTICK, through serde's UNESCAPED enum-variant echo ----
        // This arm is also the `legal_variants` non-regression: the enum's own
        // names must still be spelled out in the clear.
        const BT: &str = "source = \"sk-ant-SEN`TINEL-KKK\"\n";
        let err = toml::from_str::<crate::pricing::PricingConfig>(BT)
            .expect_err("an unknown enum variant must fail to deserialize");
        let raw = format!("{err}");
        assert!(
            raw.contains("TINEL-KKK"),
            "fixture is not exercising the leak; raw error was: {raw}"
        );
        let redacted = redact_toml_error(BT, &err);
        assert!(
            !redacted.contains("TINEL-KKK"),
            "backtick: a FRAGMENT of the value survived redaction: {redacted}"
        );
        assert!(
            !redacted.contains("sk-ant-"),
            "backtick: a secret-shaped prefix survived redaction: {redacted}"
        );
        for legal in ["auto", "bundled", "synced"] {
            assert!(
                redacted.contains(&format!("`{legal}`")),
                "backtick: the legal set must survive the CR-01 fix: {redacted}"
            );
        }

        // --- Arm 3: SINGLE QUOTE, through `redact_value_text` ----------------
        // `parse_max_age` quotes its input back with `'…'` because it was
        // written for a CLI flag; the old scanner paired on the interior quotes.
        let err = crate::ant::duration::parse_max_age("a'SENTINEL-LLL'b")
            .expect_err("a non-numeric duration must fail");
        let raw = err.to_string();
        assert!(
            raw.contains("SENTINEL-LLL"),
            "fixture is not exercising the leak; raw error was: {raw}"
        );
        let redacted = redact_value_text(&raw);
        assert!(
            !redacted.contains("SENTINEL-LLL"),
            "single quote: a FRAGMENT of the value survived redaction: {redacted}"
        );
        assert!(
            redacted.contains("--max-age"),
            "single quote: the failure must still be named: {redacted}"
        );
    }

    /// The structural invariant the CR-01 argument rests on: the output is
    /// `input[..first_delimiter]` followed by ONE `"<redacted>"` and nothing
    /// else, for every input.
    ///
    /// This is the property that makes escaping irrelevant — the function never
    /// looks for a CLOSING delimiter, so there is no pairing to desynchronize.
    /// The old scanner fails the "exactly one" arm on every row below.
    #[test]
    fn redaction_emits_one_trailing_marker_and_never_resumes_copying() {
        for input in [
            "invalid type: string \"a\\\"b\", expected a sequence",
            "unknown variant `a`b`, expected one of `auto`",
            "invalid --max-age number in 'a'b'c'",
            "prefix \"one\" middle \"two\" tail",
            "`\"'`\"'",
            "unterminated \"run",
            "no delimiters at all",
            "",
        ] {
            let out = redact_after_first_delimiter(input);
            match input.find(['"', '\'', '`']) {
                None => assert_eq!(out, input, "a delimiter-free input must pass through"),
                Some(i) => {
                    assert_eq!(
                        out.matches(REDACTED).count(),
                        1,
                        "exactly one marker must be emitted for {input:?}: {out}"
                    );
                    assert!(
                        out.ends_with(REDACTED),
                        "the marker must be the TAIL — nothing may follow it — for \
                         {input:?}: {out}"
                    );
                    assert_eq!(
                        &out[..out.len() - REDACTED.len()],
                        &input[..i],
                        "only the delimiter-free head may survive for {input:?}: {out}"
                    );
                }
            }
        }
    }

    /// WR-01: the CR-01 boundary must not turn SCHEMA text into collateral
    /// damage.
    ///
    /// Redacting from the first delimiter to end of line necessarily discards
    /// anything the diagnostic put AFTER the user's value. For three closed
    /// shapes that text is the whole point of the message, so it is recovered
    /// BEFORE redaction, on the raw line, by narrow shape readers. This test
    /// drives all three through the REAL parser and asserts the recovered text
    /// end to end.
    #[test]
    fn schema_text_survives_the_redaction_boundary_end_to_end() {
        // 1. `duplicate key `K` in table `T`` — the worst case, because the
        //    finding's key is the pseudo-key `<file>` and the message is the
        //    only other locator.
        const DUP: &str = "[display]\ntheme = \"a\"\ntheme = \"b\"\n";
        let err = toml::from_str::<toml::Value>(DUP).expect_err("a duplicate key must fail");
        let redacted = redact_toml_error(DUP, &err);
        assert!(
            redacted.contains("duplicate key `theme` in table `display`"),
            "the duplicated KEY and its TABLE must both be named: {redacted}"
        );
        assert!(
            redacted.contains("line 3"),
            "the position locator must survive: {redacted}"
        );

        // 2. `expected `.`, `=`` — TOML GRAMMAR tokens, from the table.
        const BAD: &str = "just a string\n";
        let err = toml::from_str::<toml::Value>(BAD).expect_err("a bare string must fail");
        let redacted = redact_toml_error(BAD, &err);
        assert!(
            redacted.contains("`.`") && redacted.contains("`=`"),
            "the grammar tokens the parser wanted must be named: {redacted}"
        );

        // 3. serde's trailing `, expected <prose>` clause.
        let err = toml::from_str::<crate::config::Config>(SENTINEL_TYPE)
            .expect_err("a string where a sequence is expected must fail");
        let redacted = redact_toml_error(SENTINEL_TYPE, &err);
        assert!(
            redacted.contains(", expected a sequence"),
            "serde's own `expecting` prose must survive: {redacted}"
        );
        assert!(
            !redacted.contains("SENTINEL"),
            "and it must not drag the value out with it: {redacted}"
        );
    }

    /// Each schema recovery is a narrow, shape-checked reader — not a general
    /// un-redactor. Every row is a way a hostile value could try to ride one of
    /// them out of the redaction boundary.
    #[test]
    fn schema_recovery_rejects_everything_but_its_own_closed_shape() {
        // --- `duplicate key` ------------------------------------------------
        assert_eq!(
            duplicate_key_clause("duplicate key `theme` in table `display`").as_deref(),
            Some("duplicate key `theme` in table `display`")
        );
        assert_eq!(
            duplicate_key_clause("duplicate key `theme`").as_deref(),
            Some("duplicate key `theme`")
        );
        for hostile in [
            // A delimiter in either half — this is the mis-split a hostile
            // QUOTED key would cause, and it aborts the whole recovery.
            "duplicate key `sk-ant\"LEAK` in table `display`",
            "duplicate key `theme` in table `sk-ant`LEAK`",
            // Marker injection: `rfind` mis-splits, and the allowlist catches it.
            "duplicate key `k` in table `x` in table `y`",
            // Not anchored at both ends.
            "in some context, duplicate key `theme` in table `display`",
            "duplicate key `theme` in table `display` and also sk-ant-LEAK",
            // Not the shape at all.
            "invalid type: string \"sk-ant-LEAK\", expected a sequence",
            // Empty, and over-long.
            "duplicate key ``",
        ] {
            assert!(
                duplicate_key_clause(hostile).is_none(),
                "recovery must be abandoned for {hostile:?}"
            );
        }
        let long = "a".repeat(MAX_KEY_SEGMENT_CHARS + 1);
        assert!(duplicate_key_clause(&format!("duplicate key `{long}`")).is_none());

        // --- TOML grammar tokens --------------------------------------------
        let tokens = expected_grammar_tokens("expected `.`, `=`")
            .expect("the parser's own shape must be recognized");
        assert_eq!(tokens, vec![".", "="]);
        // The decisive bound is enforced by the TYPE, not by this assertion:
        // `expected_grammar_tokens` returns `Vec<&'static str>`, and a slice
        // borrowed from the (non-`'static`) input line cannot satisfy that
        // lifetime — so the compiler rejects any version of the function that
        // returns the matched input rather than the table element. What is
        // checked here is the weaker, run-time half: every token is a member of
        // the table.
        for token in &tokens {
            assert!(
                TOML_GRAMMAR_TOKENS.contains(token),
                "a recovered token must be a table element: {token:?}"
            );
        }
        for hostile in [
            // Not in the table.
            "expected `sk-ant-LEAK`",
            "expected `.`, `sk-ant-LEAK`",
            // Not anchored at the start of the line.
            "unknown variant `sk-ant-LEAK`, expected `.`",
            // Stray text in the list.
            "expected `.` and `=`",
            "expected `.`, `=` trailing",
            // Unterminated, empty, and no list at all.
            "expected `.",
            "expected ``",
            "expected ",
        ] {
            assert!(
                expected_grammar_tokens(hostile).is_none(),
                "recovery must be abandoned for {hostile:?}"
            );
        }
        let many = (0..=MAX_GRAMMAR_TOKENS)
            .map(|_| "`.`".to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert!(expected_grammar_tokens(&format!("expected {many}")).is_none());

        // --- serde's `, expected <prose>` clause ----------------------------
        let (head, prose) = expected_prose("invalid type: string \"x\", expected a sequence", "")
            .expect("serde's own shape must be recognized");
        assert_eq!(head, "invalid type: string \"x\"");
        assert_eq!(prose, "a sequence");

        // A value containing the marker cannot steer the split: `rfind` takes
        // the LAST marker, and serde's own clause is always last.
        let (head, prose) = expected_prose(
            "invalid type: string \"a, expected sk-ant-LEAK\", expected a sequence",
            "",
        )
        .expect("the trailing schema clause is still the one that is taken");
        assert!(
            head.contains("sk-ant-LEAK"),
            "the hostile value must land in the head, where it is redacted: {head}"
        );
        assert_eq!(prose, "a sequence");

        // The DECISIVE bound: a tail that occurs anywhere in the config source
        // is refused, so the recovered text provably is not document-derived.
        assert!(
            expected_prose(
                "invalid type: string \"x\", expected a sequence",
                "note = \"a sequence\"\n"
            )
            .is_none(),
            "a tail present in the source must abandon the recovery"
        );

        for hostile in [
            // A delimiter in the tail would re-open a quoted run.
            "unknown variant `x`, expected one of `auto`",
            "invalid type, expected \"sk-ant-LEAK\"",
            "invalid type, expected 'sk-ant-LEAK'",
            // Outside the prose allowlist.
            "invalid type, expected sk-ant-LEAK!",
            "invalid type, expected a\u{1b}[31m sequence",
            // No marker, and an empty tail.
            "invalid type: string \"x\"",
            "invalid type: string \"x\", expected ",
        ] {
            assert!(
                expected_prose(hostile, "").is_none(),
                "recovery must be abandoned for {hostile:?}"
            );
        }
        let long = "a".repeat(MAX_EXPECTED_PROSE_CHARS + 1);
        assert!(expected_prose(&format!("invalid type, expected {long}"), "").is_none());
    }

    #[test]
    fn load_from_file_error_does_not_echo_config_values() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        for (name, body) in [
            ("syntax.toml", SENTINEL_SYNTAX),
            ("type.toml", SENTINEL_TYPE),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, body).expect("write fixture");
            let err = crate::config::Config::load_from_file(&path)
                .expect_err("fixture must fail to load");
            let text = err.to_string();
            assert!(
                !text.contains("SENTINEL"),
                "{name}: load_from_file leaked the sentinel: {text}"
            );
            assert!(
                text.contains("Failed to parse config file"),
                "{name}: the failure must still be named: {text}"
            );
        }
    }

    #[test]
    fn report_sort_is_total_and_stable() {
        let scrambled_a: Vec<(Severity, FindingKind, &str, &str)> = vec![
            (Severity::Warning, FindingKind::StaleCache, "pricing", "b"),
            (Severity::Error, FindingKind::UnknownKey, "display", "a"),
            (Severity::Error, FindingKind::TypeError, "display", "a"),
            (Severity::Warning, FindingKind::StaleCache, "ant", "c"),
            (Severity::Error, FindingKind::UnknownKey, "ant", "a"),
            (Severity::Error, FindingKind::UnknownKey, "display", "z"),
        ];
        let mut scrambled_b = scrambled_a.clone();
        scrambled_b.reverse();
        scrambled_b.swap(0, 3);

        let build = |rows: &[(Severity, FindingKind, &str, &str)]| {
            let mut r = Report::new();
            for (sev, kind, key, msg) in rows {
                match sev {
                    Severity::Error => r.error(*kind, *key, *msg),
                    Severity::Warning => r.warn(*kind, *key, *msg),
                }
            }
            r.sort_findings();
            r
        };

        let a = build(&scrambled_a);
        let b = build(&scrambled_b);
        assert_eq!(
            a.findings, b.findings,
            "sort_findings must be a TOTAL order — two insertion orders must converge"
        );
        assert_eq!(a.error_count(), 4);
        assert_eq!(a.warning_count(), 2);
        assert!(a.has_errors() && a.has_warnings());
        // Errors lead.
        assert_eq!(a.findings[0].severity, Severity::Error);
        assert_eq!(a.findings.last().unwrap().severity, Severity::Warning);
    }

    #[test]
    fn redact_key_path_bounds_segments_and_strips_control_characters() {
        let hostile = format!("ant.accounts.{}\u{1b}[31m.nope", "x".repeat(200));
        let out = redact_key_path(&hostile);
        assert!(!out.contains('\u{1b}'), "control bytes must be stripped");
        for segment in out.split('.') {
            assert!(
                segment.chars().count() <= MAX_KEY_SEGMENT_CHARS,
                "segment too long: {segment}"
            );
        }
        assert!(out.chars().count() <= MAX_MESSAGE_CHARS);
    }

    #[test]
    fn section_context_composes_prefixes_and_narrows_present_keys() {
        let doc: toml::Value = toml::from_str("[display]\ntheme = \"light\"\n").expect("parse");
        let mut keys = BTreeSet::new();
        keys.insert("components".to_string());
        keys.insert("components.git".to_string());
        keys.insert("components.git.enabled".to_string());
        let cx = SectionContext::new("layout", &keys, &doc);

        assert_eq!(cx.key("preset"), "layout.preset");
        assert!(cx.is_set("components.git.enabled"));
        assert!(!cx.is_set("preset"));
        assert_eq!(cx.target_string("display.theme", "dark"), "light");
        assert_eq!(cx.target_string("display.missing", "dark"), "dark");

        let child = cx.child("components.git");
        assert_eq!(child.prefix(), "layout.components.git");
        assert_eq!(child.key("enabled"), "layout.components.git.enabled");
        assert!(child.is_set("enabled"));
        assert!(!child.is_set("components.git.enabled"));
    }

    // -----------------------------------------------------------------------
    // The per-section engine (plan 12-04 Task 3)
    // -----------------------------------------------------------------------

    fn run(text: &str) -> Report {
        let mut report = Report::new();
        validate_config_text(text, &mut report);
        report
    }

    fn findings_at<'a>(report: &'a Report, key: &str) -> Vec<&'a Finding> {
        report.findings.iter().filter(|f| f.key == key).collect()
    }

    fn has(report: &Report, kind: FindingKind, key: &str) -> bool {
        report
            .findings
            .iter()
            .any(|f| f.kind == kind && f.key == key)
    }

    #[test]
    fn known_sections_cover_every_config_field_including_sync() {
        assert_eq!(
            KNOWN_SECTIONS.len(),
            14,
            "Config has exactly 14 top-level sections and no top-level scalars"
        );
        assert!(
            KNOWN_SECTIONS.contains(&"sync"),
            "`sync` must be RECOGNIZED in every build, feature or not"
        );
    }

    #[test]
    fn pricing_unknown_key_is_reported() {
        // MUTATION PROOF 1 of the phase: a whole-`Config` `serde_ignored` pass
        // cannot satisfy this assertion, because `src/config.rs` carries
        // `deserialize_with = "crate::pricing::deserialize_lenient"` on the
        // `pricing` field and that function consumes the whole subtree. The
        // throwaway pass was written, run, observed to report `display.show_gti`
        // while MISSING `pricing.aliasess`, and deleted (see 12-04-SUMMARY.md).
        let report = run("[display]\nshow_gti = true\n\n[pricing]\naliasess = 1\n");
        assert!(
            has(&report, FindingKind::UnknownKey, "pricing.aliasess"),
            "the per-section pass must see inside [pricing]: {:?}",
            report.findings
        );
        assert!(
            has(&report, FindingKind::UnknownKey, "display.show_gti"),
            "and must still report other sections: {:?}",
            report.findings
        );
    }

    #[test]
    fn type_error_in_one_section_does_not_suppress_another() {
        let report = run("[display]\nprogress_bar_width = \"wide\"\n\n[pricing]\naliasess = 1\n");
        assert!(
            has(&report, FindingKind::TypeError, "display"),
            "the bad type must be reported: {:?}",
            report.findings
        );
        assert!(
            has(&report, FindingKind::UnknownKey, "pricing.aliasess"),
            "a type error in ONE section must not suppress another (D-02): {:?}",
            report.findings
        );
    }

    #[test]
    fn failed_section_reports_exactly_one_finding() {
        // `source` fails deserialization (strict enum) AND `max_age` is
        // semantically invalid. D-02's deliberate behaviour: the section's
        // semantic pass never runs, so this is ONE finding, not two or three.
        let report = run("[pricing]\nsource = \"nonsense\"\nmax_age = \"not-a-duration\"\n");

        let pricing: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.key == "pricing" || f.key.starts_with("pricing."))
            .collect();
        assert_eq!(
            pricing.len(),
            1,
            "a FAILED section must produce exactly ONE finding: {:?}",
            pricing
        );
        assert_eq!(pricing[0].key, "pricing");
        assert_eq!(pricing[0].kind, FindingKind::TypeError);
        // Assert the ABSENCE explicitly — this is the contract downstream tests
        // must not contradict by demanding a per-field finding as well.
        assert!(
            findings_at(&report, "pricing.max_age").is_empty(),
            "a failed section must NOT also report its semantic rules: {:?}",
            report.findings
        );
        assert!(findings_at(&report, "pricing.source").is_empty());
    }

    #[test]
    fn unknown_top_level_section_is_an_error() {
        let report = run("[nonsense]\nk = 1\n");
        assert!(
            has(&report, FindingKind::UnknownKey, "nonsense"),
            "{:?}",
            report.findings
        );
        assert_eq!(report.findings[0].severity, Severity::Error, "D-09");
    }

    #[test]
    fn sync_section_is_never_an_unknown_key() {
        // Must hold under BOTH `cargo test --lib` and
        // `cargo test --all-features --lib`.
        let report = run("[sync]\n");
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.kind == FindingKind::UnknownKey),
            "[sync] must never be an unknown key in any build: {:?}",
            report.findings
        );
    }

    #[test]
    fn free_form_map_keys_are_not_unknown_keys() {
        let text = concat!(
            "[pricing.aliases]\n",
            "\"my-proxy\" = \"claude-opus-4-8\"\n",
            "\n",
            "[ant.accounts.work]\n",
            "admin_key_command = [\"security\", \"-w\"]\n",
            "nope = 1\n",
        );
        let report = run(text);

        for finding in &report.findings {
            assert!(
                !finding.key.contains("my-proxy"),
                "map KEYS are consumed, never ignored: {:?}",
                finding
            );
        }
        assert!(
            findings_at(&report, "ant.accounts.work").is_empty(),
            "a free-form map VALUE table is not an unknown key: {:?}",
            report.findings
        );
        assert!(
            has(&report, FindingKind::UnknownKey, "ant.accounts.work.nope"),
            "but a typo INSIDE a map value's struct is: {:?}",
            report.findings
        );
        assert_eq!(
            report
                .findings
                .iter()
                .filter(|f| f.kind == FindingKind::UnknownKey)
                .count(),
            1,
            "exactly one unknown key here: {:?}",
            report.findings
        );
    }

    #[test]
    fn syntax_error_reports_one_finding_with_position() {
        let report = run("[display\nprogress_bar_width = 10\n");
        assert_eq!(
            report.findings.len(),
            1,
            "an unparseable document yields exactly one finding: {:?}",
            report.findings
        );
        assert_eq!(report.findings[0].kind, FindingKind::SyntaxError);
        assert!(
            report.findings[0].message.contains("line")
                && report.findings[0].message.contains("column"),
            "the syntax finding must carry a locator: {}",
            report.findings[0].message
        );
    }

    #[test]
    fn engine_never_emits_config_values() {
        for (label, text) in [("syntax", SENTINEL_SYNTAX), ("type", SENTINEL_TYPE)] {
            let report = run(text);
            assert!(
                !report.findings.is_empty(),
                "{label}: the fixture must actually produce findings"
            );
            for finding in &report.findings {
                assert!(
                    !finding.message.contains("SENTINEL"),
                    "{label}: finding message leaked a config value: {:?}",
                    finding
                );
                assert!(
                    !finding.key.contains("SENTINEL"),
                    "{label}: finding key leaked a config value: {:?}",
                    finding
                );
            }
        }
    }

    #[test]
    fn section_context_reports_explicit_field_presence() {
        // The exact helper the engine calls to build each SectionContext.
        let written: toml::Value = toml::from_str("[pricing]\nmax_age = \"30d\"\n").expect("parse");
        let omitted: toml::Value =
            toml::from_str("[pricing]\nsource = \"bundled\"\n").expect("parse");

        let keys_written = present_keys_for_section(&written, "pricing");
        let keys_omitted = present_keys_for_section(&omitted, "pricing");
        let cx_written = SectionContext::new("pricing", &keys_written, &written);
        let cx_omitted = SectionContext::new("pricing", &keys_omitted, &omitted);

        assert!(
            cx_written.is_set("max_age"),
            "an explicitly written field must be visible as SET"
        );
        assert!(
            !cx_omitted.is_set("max_age"),
            "an omitted field must NOT be visible as SET"
        );

        // Why this capability has to exist: after deserialization the two are
        // indistinguishable, because `max_age` defaults to exactly "30d".
        let a: crate::pricing::PricingConfig =
            toml::from_str::<toml::Value>("[pricing]\nmax_age = \"30d\"\n")
                .unwrap()
                .get("pricing")
                .unwrap()
                .clone()
                .try_into()
                .unwrap();
        let b = crate::pricing::PricingConfig::default();
        assert_eq!(a.max_age, b.max_age);
    }

    // =======================================================================
    // Cache reachability classification (plan 12-06, QUAL-02 / SC3)
    // =======================================================================

    use std::path::{Path, PathBuf};

    /// `mkfifo(2)` without adding a dependency.
    ///
    /// `libc` is already linked into every Rust binary through `std`, and it is
    /// only an INDIRECT entry in `Cargo.lock`, so declaring the one symbol here
    /// keeps `Cargo.toml` / `Cargo.lock` byte-unchanged. The alternative the plan
    /// allowed — shelling out from the test — would have put a process-spawn
    /// token into this file and made the architectural acceptance grep useless.
    ///
    /// `mode_t` is `u16` on macOS and `u32` on Linux; declaring the wrong width
    /// would be an ABI mismatch, so it is `cfg`-selected.
    #[cfg(unix)]
    mod cfifo {
        #[cfg(target_os = "macos")]
        pub type ModeT = u16;
        #[cfg(not(target_os = "macos"))]
        pub type ModeT = u32;

        extern "C" {
            pub fn mkfifo(path: *const std::os::raw::c_char, mode: ModeT) -> std::os::raw::c_int;
        }
    }

    /// A well-formed cache document with an injected age.
    fn cache_json(schema_version: u32, minutes_old: i64) -> String {
        let fetched_at = chrono::Utc::now() - chrono::Duration::minutes(minutes_old);
        format!(
            "{{\"schema_version\": {}, \"fetched_at\": \"{}\", \"prices\": {{}}}}",
            schema_version,
            fetched_at.to_rfc3339()
        )
    }

    fn write_cache(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write fixture cache");
        path
    }

    fn running_as_root() -> bool {
        #[cfg(unix)]
        {
            // `id -u` without a spawn: the euid is available through std only on
            // nightly, so fall back to the fact that root can read a chmod-000
            // file. Cheap and dependency-free.
            let dir = tempfile::tempdir().expect("tempdir");
            let probe = dir.path().join("probe");
            std::fs::write(&probe, b"x").expect("write probe");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o000))
                .expect("chmod probe");
            let readable = std::fs::read(&probe).is_ok();
            let _ = std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o600));
            readable
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    #[test]
    fn classify_cache_fresh_and_stale() {
        let dir = tempfile::tempdir().expect("tempdir");

        let fresh = write_cache(dir.path(), "fresh.json", &cache_json(1, 5));
        let status = classify_cache("prices", Ok(fresh.clone()), 1, "48h");
        assert_eq!(status.state, CacheState::Fresh, "{status:?}");
        assert_eq!(status.age.as_deref(), Some("5m"));
        assert_eq!(status.path.as_deref(), Some(fresh.as_path()));
        assert!(status.detail.is_none());

        // The SAME file, one threshold different — so `Fresh` above is a decision,
        // not a constant.
        let stale = classify_cache("prices", Ok(fresh), 1, "1m");
        assert_eq!(stale.state, CacheState::Stale, "{stale:?}");

        // ...and a genuinely old file is stale against a generous threshold too.
        let old = write_cache(dir.path(), "old.json", &cache_json(1, 60 * 24 * 10));
        assert_eq!(
            classify_cache("models", Ok(old), 1, "48h").state,
            CacheState::Stale
        );
    }

    #[test]
    fn classify_cache_absent_when_nothing_is_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = classify_cache("usage", Ok(dir.path().join("nope.json")), 1, "30m");
        assert_eq!(status.state, CacheState::Absent, "{status:?}");
        assert!(status.age.is_none());
        assert!(status.path.is_some(), "the resolved path is still reported");
    }

    #[test]
    fn classify_cache_unparseable_body_and_wrong_schema() {
        let dir = tempfile::tempdir().expect("tempdir");

        let garbage = write_cache(dir.path(), "garbage.json", "{not json");
        let status = classify_cache("prices", Ok(garbage), 1, "30d");
        assert_eq!(status.state, CacheState::Unparseable, "{status:?}");
        assert_eq!(status.detail.as_deref(), Some("not valid JSON"));

        // Valid JSON, WRONG schema_version — a different failure with a different
        // detail, so the two are not collapsed.
        let wrong = write_cache(dir.path(), "wrong.json", &cache_json(99, 1));
        let status = classify_cache("prices", Ok(wrong), 1, "30d");
        assert_eq!(status.state, CacheState::Unparseable, "{status:?}");
        assert_eq!(status.detail.as_deref(), Some("schema_version mismatch"));

        // Valid JSON, right schema, NO fetched_at.
        let no_stamp = write_cache(dir.path(), "nostamp.json", "{\"schema_version\": 1}");
        let status = classify_cache("prices", Ok(no_stamp), 1, "30d");
        assert_eq!(status.state, CacheState::Unparseable, "{status:?}");
        assert!(status
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("fetched_at")));
    }

    #[test]
    fn classify_cache_unresolvable_path_is_unreadable_without_a_path() {
        let status = classify_cache(
            "usage",
            Err(crate::error::StatuslineError::Config(
                "STATUSLINE_ANT_ACCOUNT rejected: bad name".to_string(),
            )),
            1,
            "30m",
        );
        assert_eq!(status.state, CacheState::Unreadable);
        assert!(status.path.is_none(), "never fabricate a path");
        assert!(status
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("STATUSLINE_ANT_ACCOUNT")));
    }

    #[cfg(unix)]
    #[test]
    fn classify_cache_unreadable_on_a_permission_denied_file() {
        if running_as_root() {
            eprintln!("skipped: running as root, the permission bit does not bite");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_cache(dir.path(), "locked.json", &cache_json(1, 1));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");

        let status = classify_cache("prices", Ok(path.clone()), 1, "30d");
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

        assert_eq!(status.state, CacheState::Unreadable, "{status:?}");
        assert_eq!(status.detail.as_deref(), Some("permission denied"));
    }

    /// A file inside a `chmod 000` DIRECTORY must classify `Unreadable`, not
    /// `Absent`.
    ///
    /// This is the reason the ladder branches on the metadata ERROR KIND instead
    /// of asking `Path::exists()`: for exactly this case `Path::exists()` answers
    /// `false`, because it collapses every metadata error into "no". Using it
    /// would silently suppress the unreadable-cache warning QUAL-02 requires. The
    /// test asserts that divergence directly, so it is not taken on faith.
    ///
    /// (`Path::exists` is spelled as an associated call below, and named without
    /// its argument list here, so the file-wide acceptance grep for a METHOD-form
    /// existence probe counts only real ladder usage — of which there is none.)
    #[cfg(unix)]
    #[test]
    fn classify_cache_absent_vs_unreadable_are_distinguished() {
        if running_as_root() {
            eprintln!("skipped: running as root, the permission bit does not bite");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let locked_dir = dir.path().join("locked");
        std::fs::create_dir(&locked_dir).expect("mkdir");
        let path = write_cache(&locked_dir, "prices.json", &cache_json(1, 1));
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o000))
            .expect("chmod dir");

        // NON-VACUITY: the naive oracle really does get this wrong.
        let naive_says_missing = !Path::exists(&path);
        let status = classify_cache("prices", Ok(path.clone()), 1, "30d");

        let _ = std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o700));

        assert!(
            naive_says_missing,
            "fixture is not exercising the divergence: the naive existence oracle \
             found the file, so this test proves nothing"
        );
        assert_eq!(
            status.state,
            CacheState::Unreadable,
            "a permission error must NEVER masquerade as absence: {status:?}"
        );
        assert_ne!(status.state, CacheState::Absent);
    }

    /// Opening a writer-less FIFO blocks forever; this must not.
    ///
    /// The assertion is a 5-second `recv_timeout` on an `mpsc` channel fed from a
    /// worker thread. Relying on the harness timeout instead would not
    /// distinguish "returned Unreadable" from "hung" — it would only fail the
    /// whole binary after minutes, with no attribution.
    #[cfg(unix)]
    #[test]
    fn classify_cache_fifo_does_not_block() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fifo.json");
        let c_path = std::ffi::CString::new(path.to_str().expect("utf-8 path")).expect("cstring");
        let rc = unsafe { cfifo::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed; this test cannot mean anything");

        // NON-VACUITY: the fixture really is a fifo with no writer.
        let md = std::fs::symlink_metadata(&path).expect("stat the fifo");
        {
            use std::os::unix::fs::FileTypeExt;
            assert!(md.file_type().is_fifo(), "fixture is not a fifo");
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let probe = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(classify_cache("prices", Ok(probe), 1, "30d"));
        });

        let status = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("classify_cache BLOCKED on a writer-less fifo (5s timeout)");
        assert_eq!(status.state, CacheState::Unreadable, "{status:?}");
        assert_eq!(status.detail.as_deref(), Some("not a regular file (fifo)"));
    }

    /// A SYMLINK to a working cache is classified by its TARGET.
    ///
    /// Consumer accuracy: the typed cache readers follow links, so a symlinked
    /// cache renders today. Classifying the link itself as "not a regular file"
    /// would be a false unreadable-warning on a config that works.
    #[cfg(unix)]
    #[test]
    fn classify_cache_follows_a_symlink_to_its_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = write_cache(dir.path(), "real.json", &cache_json(1, 2));
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let status = classify_cache("prices", Ok(link.clone()), 1, "48h");
        assert_eq!(
            status.state,
            CacheState::Fresh,
            "a symlink to a fresh cache must classify by its TARGET: {status:?}"
        );

        // ...and a symlink to a FIFO is still caught, without blocking.
        let fifo = dir.path().join("fifo.sock");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { cfifo::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let fifo_link = dir.path().join("fifo-link.json");
        std::os::unix::fs::symlink(&fifo, &fifo_link).expect("symlink");

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(classify_cache("prices", Ok(fifo_link), 1, "30d"));
        });
        let status = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("classify_cache BLOCKED on a symlink to a writer-less fifo");
        assert_eq!(status.state, CacheState::Unreadable, "{status:?}");
        assert_eq!(status.detail.as_deref(), Some("not a regular file (fifo)"));

        // A DANGLING symlink is absence, not a read error.
        let dangling = dir.path().join("dangling.json");
        std::os::unix::fs::symlink(dir.path().join("gone.json"), &dangling).expect("symlink");
        assert_eq!(
            classify_cache("prices", Ok(dangling), 1, "30d").state,
            CacheState::Absent
        );
        let _ = link;
    }

    /// The probe cap IS the reader's byte cap, not a copy of its value.
    ///
    /// Plan 12-06 mirrored the literal and recorded the drift risk as accepted
    /// because the source constant was private. It is now `pub(crate)` and
    /// imported, so drift is impossible by construction — this assertion is a
    /// tripwire against someone re-introducing a literal, not a drift detector.
    #[test]
    fn cache_probe_cap_is_the_price_cache_cap() {
        assert_eq!(
            MAX_CACHE_PROBE_BYTES,
            crate::pricing::cache::MAX_PRICE_CACHE_BYTES,
            "the cache probe must bound reads at exactly the cap `read_price_cache` enforces"
        );
    }

    /// A `prices.json` the typed reader REJECTS must not classify as healthy.
    ///
    /// `read_price_cache` enforces `MAX_PRICE_CACHE_ENTRIES` AFTER parsing, so
    /// a >4096-row file under the byte cap used to probe `Fresh` here while the
    /// render silently fell back to the bundled table. Both arms are asserted:
    /// the NON-VACUITY control at exactly the cap must still be `Fresh`, or the
    /// rejecting arm would prove nothing beyond "big files are rejected".
    #[test]
    fn classify_cache_applies_the_typed_readers_row_cap() {
        let cap = crate::pricing::cache::MAX_PRICE_CACHE_ENTRIES;
        let dir = tempfile::tempdir().expect("tempdir");

        let body = |rows: usize| {
            let mut prices = serde_json::Map::new();
            for i in 0..rows {
                prices.insert(format!("m{i}"), serde_json::json!({}));
            }
            serde_json::json!({
                "schema_version": 1,
                "fetched_at": chrono::Utc::now().to_rfc3339(),
                "prices": prices,
            })
            .to_string()
        };

        // NON-VACUITY CONTROL: exactly at the cap is accepted by the reader, so
        // it must stay Fresh here.
        let at_cap = dir.path().join("at-cap.json");
        let at_cap_body = body(cap);
        assert!(
            (at_cap_body.len() as u64) < MAX_CACHE_PROBE_BYTES,
            "the fixture must be UNDER the byte cap, or the byte guard — not the row guard — \
             is what this test would be exercising ({} bytes)",
            at_cap_body.len()
        );
        std::fs::write(&at_cap, &at_cap_body).expect("write at-cap fixture");
        assert_eq!(
            classify_cache("prices", Ok(at_cap), 1, "30d").state,
            CacheState::Fresh,
            "a cache at exactly the row cap is accepted by the reader and must classify Fresh"
        );

        let over = dir.path().join("over-cap.json");
        let over_body = body(cap + 1);
        assert!(
            (over_body.len() as u64) < MAX_CACHE_PROBE_BYTES,
            "the over-cap fixture must also be UNDER the byte cap ({} bytes)",
            over_body.len()
        );
        std::fs::write(&over, &over_body).expect("write over-cap fixture");
        let status = classify_cache("prices", Ok(over), 1, "30d");
        assert_eq!(
            status.state,
            CacheState::Unparseable,
            "a cache the typed reader discards must NOT be reported as healthy: {status:?}"
        );
        assert!(
            status
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("rows") && d.contains("bundled")),
            "the detail must say what actually happens to the render: {status:?}"
        );

        // The cap is the PRICE cache's alone; nothing else carries a row cap.
        assert!(typed_reader_row_cap("models").is_none());
        assert!(typed_reader_row_cap("usage").is_none());
    }

    #[test]
    fn classify_cache_rejects_an_over_cap_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("huge.json");
        let body = "x".repeat(MAX_CACHE_PROBE_BYTES as usize + 16);
        std::fs::write(&path, body).expect("write oversized fixture");
        let status = classify_cache("prices", Ok(path), 1, "30d");
        assert_eq!(status.state, CacheState::Unreadable, "{status:?}");
        assert!(status
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("probe cap")));
    }

    /// Exactly one bounded read per classification, asserted on the SOURCE.
    ///
    /// A call counter is impractical here (the reads go through `std`), so the
    /// invariant is pinned structurally. Every needle is ASSEMBLED rather than
    /// written literally, so this test's own text does not pollute the file-wide
    /// acceptance greps that count real call sites — a self-matching needle would
    /// make those greps permanently useless.
    #[test]
    fn classify_cache_reads_the_file_at_most_once() {
        let open_token = format!("File{}open", "::");
        let typed_readers = [
            format!("read_price{}", "_cache"),
            format!("read_models{}", "_cache"),
            format!("read_usage{}", "_cache"),
        ];

        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config_validation.rs"),
        )
        .expect("read this module's own source");
        let start = source
            .find("pub fn classify_cache(")
            .expect("classify_cache must be declared");
        let rest = &source[start..];
        let end = rest.find("\n}\n").expect("classify_cache must have a body");
        let body = &rest[..end];

        // NON-VACUITY: an extraction that silently produced nothing would make
        // every assertion below pass hollowly.
        assert!(
            body.len() > 1000 && body.contains("symlink_metadata"),
            "the body extraction is broken ({} bytes)",
            body.len()
        );

        assert_eq!(
            body.matches(open_token.as_str()).count(),
            1,
            "exactly ONE bounded open per classification"
        );
        for reader in &typed_readers {
            assert_eq!(
                body.matches(reader.as_str()).count(),
                0,
                "the typed readers resolve the path and open the file AGAIN, which would be a \
                 second read and would re-introduce the fifo hazard: {reader}"
            );
        }
    }

    #[test]
    fn report_cache_findings_is_warnings_only_and_silent_when_fresh_or_absent() {
        let quiet = vec![
            CacheStatus {
                name: "prices",
                state: CacheState::Fresh,
                age: Some("5m".to_string()),
                path: None,
                detail: None,
            },
            CacheStatus {
                name: "models",
                state: CacheState::Absent,
                age: None,
                path: None,
                detail: None,
            },
        ];
        let mut report = Report::new();
        report_cache_findings(&quiet, &mut report);
        assert!(
            report.findings.is_empty(),
            "FRESH and ABSENT are the settled silent states: {:?}",
            report.findings
        );

        let noisy = vec![
            CacheStatus {
                name: "prices",
                state: CacheState::Stale,
                age: Some("31d".to_string()),
                path: None,
                detail: None,
            },
            CacheStatus {
                name: "models",
                state: CacheState::Unreadable,
                age: None,
                path: None,
                detail: Some("permission denied".to_string()),
            },
            CacheStatus {
                name: "usage",
                state: CacheState::Unparseable,
                age: None,
                path: None,
                detail: Some("schema_version mismatch".to_string()),
            },
        ];
        let mut report = Report::new();
        report_cache_findings(&noisy, &mut report);
        assert_eq!(report.findings.len(), 3, "{:?}", report.findings);
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.severity == Severity::Warning),
            "D-11: a cache finding must never affect the exit code by default: {:?}",
            report.findings
        );
        assert!(!report.has_errors());
        assert_eq!(report.findings[0].kind, FindingKind::StaleCache);
        assert_eq!(report.findings[1].kind, FindingKind::UnreadableCache);
        assert_eq!(report.findings[2].kind, FindingKind::UnreadableCache);
        assert_eq!(report.findings[0].key, "caches.prices");
    }

    // --- classify_all_caches: the active-account axis ------------------------
    // These three mutate a process-global env var, so they are `#[serial]` and
    // each restores the prior value.

    fn with_account<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let saved = std::env::var_os("STATUSLINE_ANT_ACCOUNT");
        match value {
            Some(v) => std::env::set_var("STATUSLINE_ANT_ACCOUNT", v),
            None => std::env::remove_var("STATUSLINE_ANT_ACCOUNT"),
        }
        let out = f();
        match saved {
            Some(v) => std::env::set_var("STATUSLINE_ANT_ACCOUNT", v),
            None => std::env::remove_var("STATUSLINE_ANT_ACCOUNT"),
        }
        out
    }

    fn named<'a>(statuses: &'a [CacheStatus], name: &str) -> Option<&'a CacheStatus> {
        statuses.iter().find(|s| s.name == name)
    }

    #[test]
    #[serial_test::serial]
    fn classify_all_caches_without_active_account_omits_usage() {
        let cfg = crate::config::Config::default();

        for value in [None, Some("")] {
            let statuses = with_account(value, || classify_all_caches(&cfg));
            assert!(
                named(&statuses, "usage").is_none(),
                "with STATUSLINE_ANT_ACCOUNT {value:?} there is no usage cache to classify"
            );
            // ...and the assertion above cannot pass merely because the function
            // returned nothing.
            assert!(named(&statuses, "prices").is_some());
            assert!(named(&statuses, "models").is_some());
            assert_eq!(statuses.len(), 2);
        }
    }

    #[test]
    #[serial_test::serial]
    fn classify_all_caches_uses_env_account_path() {
        let mut cfg = crate::config::Config::default();
        // Explicitly OFF: the enrichment toggle is not a filter here (D-12).
        cfg.ant.enabled = false;

        let statuses = with_account(Some("work"), || classify_all_caches(&cfg));
        let usage = named(&statuses, "usage").expect("usage is classified for a legal account");
        assert_eq!(
            usage.path,
            crate::ant::cache::usage_cache_path("work").ok(),
            "the usage path must come from the SANITIZING accessor"
        );
        assert_eq!(statuses.len(), 3);
    }

    #[test]
    #[serial_test::serial]
    fn classify_all_caches_rejects_illegal_account_name() {
        let cfg = crate::config::Config::default();
        let statuses = with_account(Some("../etc"), || classify_all_caches(&cfg));
        let usage = named(&statuses, "usage").expect("an illegal account still reports a status");
        assert_eq!(usage.state, CacheState::Unreadable, "{usage:?}");
        assert!(
            usage.path.is_none(),
            "never construct a path from a rejected name"
        );
        assert!(usage
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("STATUSLINE_ANT_ACCOUNT")));
        // Non-vacuity of the oracle.
        assert!(crate::ant::cache::usage_cache_path("../etc").is_err());
    }
}
