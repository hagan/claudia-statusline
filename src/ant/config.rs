//! Ant (Anthropic CLI enrichment) module configuration.
//!
//! Defines the `[ant]` TOML section for the opt-in Claude API enrichment
//! feature. Uses `#[serde(default)]` so existing configs without an `[ant]`
//! section silently receive sensible defaults.
//!
//! Per CONTEXT.md decision D-08, `enabled` defaults to **false** (the opposite
//! of `[gsd]`, which defaults to true). The section is intentionally minimal —
//! D-10 forbids a `cache_dir` key (location is controlled by `XDG_CACHE_HOME`)
//! and there are no usage/staleness sub-toggles in this foundation plan.

use crate::config_validation::{FindingKind, Report, SectionContext, Validate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single named `ant` account (Phase 08, ANT-24).
///
/// Each `[ant.accounts.<name>]` TOML table maps an account label (the map key)
/// to the argv used to fetch its org-admin key. Starting minimal per RESEARCH
/// Open Question #2: the only field is the `admin_key_command` argv; the map key
/// IS the account name (no redundant `name` field). There is deliberately **no**
/// key/secret field on disk — the command produces the key out-of-band at sync
/// time and it is never persisted (D-17).
///
/// # Example
///
/// ```toml
/// [ant.accounts.work]
/// admin_key_command = ["security", "find-generic-password", "-s", "x", "-w"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AntAccount {
    /// Argv (program + args) that, when run, prints the org-admin API key on
    /// stdout. Empty by default — an account with no command resolves to no key
    /// and degrades silently (the usage path is opt-in per account).
    pub admin_key_command: Vec<String>,
}

#[allow(clippy::derivable_impls)]
impl Default for AntAccount {
    fn default() -> Self {
        Self {
            admin_key_command: Vec::new(),
        }
    }
}

/// Configuration for the `ant` (opt-in Claude API enrichment) module.
///
/// Added to `statusline.toml` as an `[ant]` section. All fields have defaults
/// via `#[serde(default)]`, so configs without this section work unchanged and
/// — critically — render byte-identically to v3.1.0 when `[ant]` is absent or
/// `enabled = false`.
///
/// # Example
///
/// ```toml
/// [ant]
/// enabled = true
/// profile = "work"   # optional `ant` CLI profile name; default ""
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AntConfig {
    /// Enable the `ant` enrichment module (default: **false**, opt-in per D-08).
    pub enabled: bool,
    /// Optional `ant` CLI profile name to use for out-of-band fetches.
    /// When empty, the default profile / key-precedence applies.
    pub profile: String,
    /// Named accounts for the org-admin usage/cost path (Phase 08, ANT-24).
    ///
    /// The map key is the account label (used as the per-account usage cache
    /// file name after sanitization). Absent by default so configs without an
    /// `[ant.accounts.*]` table parse to an empty map (byte-identical default
    /// behavior — `enabled`/`profile` stay account-agnostic per D-08/A4).
    #[serde(default)]
    pub accounts: HashMap<String, AntAccount>,
    /// Mark the usage-age template var stale once the usage cache exceeds this
    /// (default `"30m"`, D-10). Same single-unit grammar as `--max-age`
    /// (parsed with [`crate::ant::duration::parse_max_age`] on use).
    #[serde(default = "default_usage_stale_after")]
    pub usage_stale_after: String,
    /// Mark the models-age template var stale once the models cache exceeds this
    /// (default `"48h"`, D-10). Same single-unit grammar as `--max-age`.
    #[serde(default = "default_models_stale_after")]
    pub models_stale_after: String,
}

/// Hard cap on how much of a configured PROGRAM NAME may appear in a finding.
///
/// `admin_key_command[0]` is the ONE config value this section is allowed to
/// echo (naming the missing program is the whole point of the PATH warning), so
/// it is bounded and stripped of control characters here rather than trusted.
/// Every LATER element of the argv is where credentials live and is never
/// echoed at all — see the [`Validate`] impl below.
const MAX_PROGRAM_LABEL_CHARS: usize = 64;

/// Bound and de-control a configured program name for inclusion in a finding.
fn program_label(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    match cleaned.char_indices().nth(MAX_PROGRAM_LABEL_CHARS) {
        Some((idx, _)) => format!("{}…", &cleaned[..idx]),
        None => cleaned,
    }
}

/// Semantic rules for `[ant]` (plan 12-06, QUAL-02).
///
/// # The never-execute contract
///
/// `[ant.accounts.*].admin_key_command` is an ARGV the USER put in a config
/// file. Running it from a diagnostic would turn `config validate` into an
/// arbitrary-code-execution surface (T-12-20), so this impl validates its SHAPE
/// and probes `PATH` — and nothing else. There is no `std::process` call of any
/// kind anywhere in this file, in production code or in its tests; plan 12-12
/// proves the absence at RUNTIME with a marker file, and an acceptance grep
/// pins it structurally.
///
/// The PATH probe is [`crate::ant::fetch::tool_on_path`] — the same pure
/// `env::split_paths` + `is_file()` walk `ant doctor` uses
/// (`src/commands/ant.rs`). It was ALREADY `pub(crate)`, so this module reaches
/// it with no visibility change, and `src/ant/fetch.rs` is byte-unchanged.
///
/// # What may be echoed
///
/// Only `admin_key_command[0]`, the program name, and only through
/// [`program_label`]. Never a later argument: verified live, a
/// `[ant.accounts.work]` carrying
/// `admin_key_command = ["security", "find-generic-password", "-w", "sk-ant-…"]`
/// puts the secret at index 3 (T-12-21). `validate_ant_never_echoes_command_arguments`
/// pins it with an `sk-ant-SENTINEL` argument.
///
/// # Severities
///
/// ERROR only where the value provably cannot work: a threshold that
/// [`crate::ant::duration::parse_max_age`] rejects, and an account key
/// [`crate::ant::cache::sanitize_account_name`] rejects (that key becomes the
/// per-account cache FILENAME, so an illegal one means the cache can never be
/// addressed at all). Everything else — an empty argv, a program that is not on
/// `PATH` right now, enrichment switched off with accounts configured — is a
/// WARNING, because each of those is a legitimate intermediate state.
impl Validate for AntConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        // --- QUAL-02's "staleness thresholds" ------------------------------
        for (field, raw) in [
            ("usage_stale_after", &self.usage_stale_after),
            ("models_stale_after", &self.models_stale_after),
        ] {
            if let Err(e) = crate::ant::duration::parse_max_age(raw) {
                // The parser quotes the offending input back (it was written for
                // a CLI flag), so it crosses the redaction boundary first; the
                // grammar sentence is appended AFTER, where an unterminated quote
                // in the user's value cannot swallow it.
                let detail = crate::config_validation::redact_value_text(&e.to_string());
                report.error(
                    FindingKind::InvalidValue,
                    cx.key(field),
                    format!(
                        "`{field}` is not a valid duration ({detail}); the grammar is a \
                         non-negative integer followed by exactly one unit of s/m/h/d"
                    ),
                );
            }
        }

        // --- Accounts -------------------------------------------------------
        // Deterministic order: `accounts` is a `HashMap` and the report is
        // serialized by plan 12-09.
        let mut names: Vec<&String> = self.accounts.keys().collect();
        names.sort();

        for name in names {
            let account_key =
                crate::config_validation::redact_key_path(&cx.key(&format!("accounts.{name}")));

            if let Err(e) = crate::ant::cache::sanitize_account_name(name) {
                let detail = crate::config_validation::redact_value_text(&e.to_string());
                report.error(
                    FindingKind::InvalidValue,
                    account_key.clone(),
                    format!(
                        "this account name is not usable as a cache filename ({detail}), so its \
                         usage cache can never be addressed; allowed characters are A-Za-z0-9._-"
                    ),
                );
            }

            let argv = &self.accounts[name].admin_key_command;
            let argv_key = crate::config_validation::redact_key_path(&format!(
                "{account_key}.admin_key_command"
            ));

            match argv.first() {
                None => {
                    report.warn(
                        FindingKind::InvalidValue,
                        argv_key,
                        "`admin_key_command` is empty, so this account resolves to no org-admin \
                         key and its usage data is never fetched",
                    );
                }
                Some(program) => {
                    // A pure PATH walk. No spawn, no shell, no argument is read.
                    if !crate::ant::fetch::tool_on_path(program) {
                        report.warn(
                            FindingKind::InvalidValue,
                            argv_key,
                            format!(
                                "`admin_key_command` starts with `{}`, which is not on PATH right \
                                 now, so fetching this account's org-admin key would fail",
                                program_label(program)
                            ),
                        );
                    }
                }
            }
        }

        // --- Cross-field: configured but switched off -----------------------
        // The symptom a user reports is "my ant vars are empty", which is exactly
        // the silent failure this command exists to end (research Open Question 3,
        // adopted). Lowest severity: `false` with accounts staged is a perfectly
        // legitimate state.
        if !self.enabled && !self.accounts.is_empty() {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("enabled"),
                format!(
                    "`enabled` is false while {} `[ant.accounts.*]` table(s) are configured, so \
                     none of the ant enrichment variables are populated and the segment renders \
                     nothing",
                    self.accounts.len()
                ),
            );
        }
    }
}

/// Default usage-cache staleness threshold (D-10).
fn default_usage_stale_after() -> String {
    "30m".into()
}

/// Default models-cache staleness threshold (D-10).
fn default_models_stale_after() -> String {
    "48h".into()
}

// The manual impl is intentional (and mirrors `GsdConfig`): it makes the
// security-relevant D-08 default — `enabled = false` (opt-in) — explicit at the
// definition site rather than implied by the field type's derived default.
#[allow(clippy::derivable_impls)]
impl Default for AntConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: String::new(),
            accounts: HashMap::new(),
            usage_stale_after: default_usage_stale_after(),
            models_stale_after: default_models_stale_after(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_with_empty_profile() {
        let cfg = AntConfig::default();
        assert!(!cfg.enabled, "ant must default to disabled (D-08)");
        assert!(cfg.profile.is_empty(), "profile must default to empty");
    }

    #[test]
    fn toml_without_ant_table_yields_default() {
        // Deserializing an empty TOML document with #[serde(default)] yields the
        // default AntConfig (mirrors how a statusline.toml with no [ant] section
        // is handled when the field carries #[serde(default)]).
        let cfg: AntConfig = toml::from_str("").expect("empty TOML should parse to default");
        assert!(!cfg.enabled);
        assert!(cfg.profile.is_empty());
    }

    #[test]
    fn toml_enables_and_sets_profile() {
        let cfg: AntConfig =
            toml::from_str("enabled = true\nprofile = \"work\"\n").expect("valid TOML");
        assert!(cfg.enabled);
        assert_eq!(cfg.profile, "work");
    }

    #[test]
    fn default_accounts_is_empty() {
        // The new per-account map must default to empty so configs without an
        // [ant.accounts.*] table preserve byte-identical default behavior.
        let cfg = AntConfig::default();
        assert!(cfg.accounts.is_empty(), "accounts must default to empty");
        assert!(!cfg.enabled, "enabled is still false (D-08)");
    }

    #[test]
    fn toml_without_accounts_table_yields_empty_map() {
        // A config that sets [ant] flags but no accounts table parses to an empty
        // accounts map (the field rides on #[serde(default)]).
        let cfg: AntConfig =
            toml::from_str("enabled = true\nprofile = \"\"\n").expect("valid TOML");
        assert!(
            cfg.accounts.is_empty(),
            "no [ant.accounts.*] table => empty accounts"
        );
    }

    #[test]
    fn toml_parses_account_admin_key_command() {
        // [ant.accounts.work] with an admin_key_command argv deserializes into the
        // accounts map keyed by the table name.
        let toml = "enabled = true\n\n[accounts.work]\nadmin_key_command = [\"security\", \"find-generic-password\", \"-s\", \"x\", \"-w\"]\n";
        let cfg: AntConfig = toml::from_str(toml).expect("valid TOML with accounts");
        let work = cfg.accounts.get("work").expect("work account present");
        assert_eq!(
            work.admin_key_command,
            vec![
                "security".to_string(),
                "find-generic-password".to_string(),
                "-s".to_string(),
                "x".to_string(),
                "-w".to_string(),
            ]
        );
    }

    #[test]
    fn account_default_admin_key_command_is_empty() {
        assert!(AntAccount::default().admin_key_command.is_empty());
    }

    #[test]
    fn default_staleness_thresholds() {
        // D-10: usage 30m / models 48h are the per-cache staleness defaults.
        let cfg = AntConfig::default();
        assert_eq!(cfg.usage_stale_after, "30m");
        assert_eq!(cfg.models_stale_after, "48h");
    }

    #[test]
    fn toml_without_thresholds_yields_default_thresholds() {
        // An [ant] config that omits the threshold keys must still parse to the
        // 30m/48h defaults (rides on #[serde(default = ...)]), so existing
        // configs keep working unchanged.
        let cfg: AntConfig =
            toml::from_str("enabled = true\n").expect("valid TOML without thresholds");
        assert_eq!(cfg.usage_stale_after, "30m");
        assert_eq!(cfg.models_stale_after, "48h");
    }

    // -----------------------------------------------------------------------
    // `AntConfig`'s `Validate` impl (plan 12-06, QUAL-02). The banner
    // deliberately does NOT spell the `impl … for …` line, so the acceptance
    // grep for the DEFINITION counts exactly one site.
    // -----------------------------------------------------------------------

    use crate::config_validation::{present_keys_for_section, Report, Severity};
    use std::collections::BTreeSet;

    /// Validate an `[ant]` section the way the engine does: deserialize the
    /// `ant` SUBTREE directly, then run the rules against a context built from
    /// the same document.
    fn validate_ant_doc(document: &str) -> Report {
        let doc: toml::Value = toml::from_str(document).expect("fixture document parses");
        let section: AntConfig = match doc.get("ant") {
            Some(sub) => sub.clone().try_into().expect("fixture [ant] deserializes"),
            None => AntConfig::default(),
        };
        let keys: BTreeSet<String> = present_keys_for_section(&doc, "ant");
        let cx = SectionContext::new("ant", &keys, &doc);
        let mut report = Report::new();
        section.validate(&cx, &mut report);
        report
    }

    fn keys_of(report: &Report) -> Vec<&str> {
        report.findings.iter().map(|f| f.key.as_str()).collect()
    }

    /// Replace `PATH` with a single EMPTY directory for the duration of `f`.
    ///
    /// Two independent jobs: it makes "this program is not on PATH" a FACT
    /// rather than an assumption about the host, and it makes the on-PATH arm's
    /// success attributable to the seeded file rather than to a coincidence.
    /// Every caller is `#[serial]` because `PATH` is process-global.
    fn with_path<R>(dir: &std::path::Path, f: impl FnOnce() -> R) -> R {
        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", dir);
        let out = f();
        match saved {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        out
    }

    #[test]
    fn validate_ant_default_section_is_clean() {
        assert!(
            validate_ant_doc("[ant]\n").findings.is_empty(),
            "the shipped default must validate cleanly"
        );
    }

    #[test]
    fn validate_ant_bad_usage_stale_after_is_error() {
        let report = validate_ant_doc("[ant]\nusage_stale_after = \"soon\"\n");
        assert_eq!(keys_of(&report), vec!["ant.usage_stale_after"]);
        assert_eq!(report.findings[0].severity, Severity::Error);
        assert_eq!(report.findings[0].kind, FindingKind::InvalidValue);
        assert!(
            report.findings[0].message.contains("s/m/h/d"),
            "the message must name the grammar: {}",
            report.findings[0].message
        );
        assert!(
            !report.findings[0].message.contains("soon"),
            "the message must not echo the config value: {}",
            report.findings[0].message
        );
    }

    #[test]
    fn validate_ant_bad_models_stale_after_is_error() {
        let report = validate_ant_doc("[ant]\nmodels_stale_after = \"48\"\n");
        assert_eq!(keys_of(&report), vec!["ant.models_stale_after"]);
        assert_eq!(report.findings[0].severity, Severity::Error);
        // ...and a THRESHOLD THAT PARSES is silent, so the rule is not
        // unconditional.
        assert!(validate_ant_doc("[ant]\nmodels_stale_after = \"48h\"\n")
            .findings
            .is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn validate_ant_illegal_account_key_is_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = with_path(dir.path(), || {
            validate_ant_doc(
                "[ant]\nenabled = true\n\n[ant.accounts.\"../etc\"]\nadmin_key_command = [\"security\"]\n",
            )
        });

        let errors: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "an illegal account key is exactly one ERROR: {:?}",
            report.findings
        );
        assert_eq!(errors[0].key, "ant.accounts.../etc");
        assert!(
            errors[0].message.contains("A-Za-z0-9._-"),
            "the message must name the legal alphabet: {}",
            errors[0].message
        );
        // Non-vacuity of the rule's ORACLE: the key really is rejected by the
        // consumer that turns it into a cache filename.
        assert!(crate::ant::cache::sanitize_account_name("../etc").is_err());
        // ...and a LEGAL key produces no error at all.
        let legal = with_path(dir.path(), || {
            validate_ant_doc(
                "[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = [\"security\"]\n",
            )
        });
        assert!(
            !legal.has_errors(),
            "a legal account key must not error: {:?}",
            legal.findings
        );
    }

    #[test]
    fn validate_ant_empty_admin_key_command_warns() {
        let report = validate_ant_doc(
            "[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = []\n",
        );
        assert_eq!(
            keys_of(&report),
            vec!["ant.accounts.work.admin_key_command"]
        );
        assert_eq!(report.findings[0].severity, Severity::Warning);
    }

    #[test]
    fn validate_ant_disabled_with_configured_account_warns_once() {
        let report = validate_ant_doc(
            "[ant]\nenabled = false\n\n[ant.accounts.work]\nadmin_key_command = []\n",
        );
        let at_enabled: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.key == "ant.enabled")
            .collect();
        assert_eq!(
            at_enabled.len(),
            1,
            "exactly one cross-field warning: {:?}",
            report.findings
        );
        assert_eq!(at_enabled[0].severity, Severity::Warning);
        assert!(
            !report.has_errors(),
            "a switched-off section is never an ERROR: {:?}",
            report.findings
        );
        // The mirror arm: with enrichment ON the rule is silent.
        let on = validate_ant_doc(
            "[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = []\n",
        );
        assert!(on.findings.iter().all(|f| f.key != "ant.enabled"));
    }

    #[test]
    #[serial_test::serial]
    fn validate_ant_valid_config_with_on_path_program_is_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        let program = "statusline-12-06-fixture-prog";
        std::fs::write(dir.path().join(program), b"#!/bin/sh\n").expect("seed a PATH entry");

        let report = with_path(dir.path(), || {
            validate_ant_doc(&format!(
                "[ant]\nenabled = true\nusage_stale_after = \"30m\"\nmodels_stale_after = \"48h\"\n\n\
                 [ant.accounts.work]\nadmin_key_command = [\"{program}\", \"--quiet\"]\n"
            ))
        });
        assert!(
            report.findings.is_empty(),
            "a fully valid [ant] with an on-PATH program must be clean: {:?}",
            report.findings
        );

        // POSITIVE CONTROL for the instrument itself: the SAME document with the
        // seeded file removed DOES warn. Without this arm, "clean" above could
        // equally mean "the PATH rule never fires at all".
        std::fs::remove_file(dir.path().join(program)).expect("unseed");
        let without = with_path(dir.path(), || {
            validate_ant_doc(&format!(
                "[ant]\nenabled = true\n\n[ant.accounts.work]\nadmin_key_command = [\"{program}\", \"--quiet\"]\n"
            ))
        });
        assert_eq!(
            keys_of(&without),
            vec!["ant.accounts.work.admin_key_command"],
            "the PATH rule must be able to fire: {:?}",
            without.findings
        );
    }

    #[test]
    #[serial_test::serial]
    fn validate_ant_never_echoes_command_arguments() {
        // An EMPTY PATH guarantees the not-on-PATH warning fires, so the
        // "no SENTINEL anywhere" assertion below is never vacuously true.
        let dir = tempfile::tempdir().expect("tempdir");
        let report = with_path(dir.path(), || {
            validate_ant_doc(
                "[ant]\nenabled = true\n\n[ant.accounts.work]\n\
                 admin_key_command = [\"some-prog\", \"--password\", \"sk-ant-SENTINEL-FFF\"]\n",
            )
        });

        assert!(
            !report.findings.is_empty(),
            "NON-VACUITY: this test only means something if a finding was produced"
        );
        for f in &report.findings {
            assert!(
                !f.key.contains("SENTINEL") && !f.message.contains("SENTINEL"),
                "a later argv element is where the credential lives and must never be echoed: {f:?}"
            );
        }
        // The PROGRAM name MAY appear — that is the actionable part.
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.message.contains("some-prog")),
            "the program name is permitted and is what makes the warning useful: {:?}",
            report.findings
        );
    }

    #[test]
    fn program_label_bounds_and_de_controls() {
        assert_eq!(program_label("security"), "security");
        assert_eq!(program_label("se\u{1b}[31mcurity"), "se[31mcurity");
        let long = "a".repeat(MAX_PROGRAM_LABEL_CHARS + 10);
        let label = program_label(&long);
        assert!(label.chars().count() <= MAX_PROGRAM_LABEL_CHARS + 1);
        assert!(label.ends_with('…'));
    }
}
