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

/// Turn a `toml` parser diagnostic into a message that leaks nothing.
///
/// # Contract
///
/// **The returned string contains no byte sequence taken from `source` other
/// than a line/column number.** This is pinned by sentinel tests covering BOTH
/// parser paths, because both leak:
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
/// 2. replace every double-quoted, single-quoted and backtick-quoted run with
///    the literal `"<redacted>"` (kills inline values — serde spells an unknown
///    enum variant with backticks, so all three delimiters must go);
/// 3. append ` (line L, column C)` when the error carries a span — positions
///    are not secret and are the locator the user needs;
/// 4. cap the result at [`MAX_MESSAGE_CHARS`].
///
/// Note that `toml::de::Error::to_string()` is deliberately NOT used anywhere:
/// its `Display` impl is precisely what renders the source-line echo.
pub fn redact_toml_error(source: &str, e: &toml::de::Error) -> String {
    // Step 1: first line of the error's own message only.
    let raw = e.message();
    let first_line = raw.split('\n').next().unwrap_or("").trim();

    // Step 2: remove every quoted run, then any remaining control characters.
    let mut redacted = redact_quoted_runs(first_line);
    redacted.retain(|c| c == '\t' || !c.is_control());
    let redacted = redacted.trim().to_string();

    let mut out = if redacted.is_empty() {
        "invalid TOML".to_string()
    } else {
        redacted
    };

    // Step 3: a position, if the error carries one.
    if let Some(span) = e.span() {
        let (line, column) = line_and_column(source, span.start);
        out.push_str(&format!(" (line {}, column {})", line, column));
    }

    // Step 4: bound the result.
    cap_chars(&out, MAX_MESSAGE_CHARS)
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

/// Replace every `"…"`, `'…'` and `` `…` `` run with the literal `"<redacted>"`.
///
/// An unterminated run redacts to end of input — the failure mode must be "too
/// much removed", never "a value survived".
fn redact_quoted_runs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' || c == '`' {
            for d in chars.by_ref() {
                if d == c {
                    break;
                }
            }
            out.push_str("\"<redacted>\"");
        } else {
            out.push(c);
        }
    }
    out
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
}
