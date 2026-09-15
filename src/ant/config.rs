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

use crate::config_validation::Validate;
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

/// Semantic rules for `[ant]` are filled in by plan 12-06.
impl Validate for AntConfig {}

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
}
