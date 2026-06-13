//! `--list-vars` handler: run all providers and print variables grouped by source.

use std::env;
use std::io::{self, Read};

use crate::error::Result;
use crate::models::StatuslineInput;
use crate::Cli;

/// Handle `--list-vars` CLI flag: run all providers and print variables grouped by source.
pub(crate) fn handle_list_vars(cli: &Cli) -> Result<()> {
    use crate::provider::ProviderOrchestrator;
    use std::collections::BTreeMap;

    // Read JSON from stdin (needed for session/cost/model data)
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let input: StatuslineInput = serde_json::from_str(&buffer).unwrap_or_default();

    let current_dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_ref())
        .cloned()
        .unwrap_or_else(|| {
            env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "~".to_string())
        });

    let full_config = crate::config::get_config();

    // --- Run providers via orchestrator ---
    let mut orchestrator = ProviderOrchestrator::new();

    // Register GitProvider
    orchestrator.register(Box::new(crate::git_provider::GitProvider::new(
        &current_dir,
    )));

    // Register StatsProvider
    let total_cost = input
        .cost
        .as_ref()
        .and_then(|c| c.total_cost_usd)
        .unwrap_or(0.0);
    let lines_added = input
        .cost
        .as_ref()
        .and_then(|c| c.total_lines_added)
        .unwrap_or(0);
    let lines_removed = input
        .cost
        .as_ref()
        .and_then(|c| c.total_lines_removed)
        .unwrap_or(0);
    let daily_total = {
        let data = crate::stats::get_or_load_stats_data();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0)
    };
    let db_path = crate::stats::StatsData::get_sqlite_path()
        .ok()
        .map(|p| p.display().to_string());
    orchestrator.register(Box::new(crate::stats::StatsProvider::new(
        input.session_id.clone(),
        total_cost,
        daily_total,
        lines_added,
        lines_removed,
        input.transcript.clone(),
        db_path,
    )));

    // Register GsdProvider
    let cwd = std::path::Path::new(&current_dir);
    orchestrator.register(Box::new(crate::gsd::GsdProvider::new(
        &full_config.gsd,
        cwd,
    )));

    // Collect all provider variables
    let provider_vars = orchestrator.collect_all();

    // --- Build core variables (not from providers) ---
    let mut core_vars: BTreeMap<String, String> = BTreeMap::new();

    // Directory
    let short_dir = crate::display::Colors::directory()
        + &crate::utils::sanitize_for_terminal(&crate::utils::shorten_path(&current_dir))
        + &crate::display::Colors::reset();
    core_vars.insert("directory".into(), short_dir);

    let basename = std::path::Path::new(&current_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&current_dir)
        .to_string();
    core_vars.insert("dir_short".into(), basename);

    // Model
    if let Some(name) = input.model.as_ref().and_then(|m| m.display_name.as_deref()) {
        let sanitized = crate::utils::sanitize_for_terminal(name);
        let model_type = crate::models::ModelType::from_name(&sanitized);
        core_vars.insert("model".into(), model_type.abbreviation());
        core_vars.insert("model_full".into(), sanitized);
        core_vars.insert("model_name".into(), model_type.family());
    }

    // Context
    if let Some(transcript) = input.transcript.as_deref() {
        let model_name = input.model.as_ref().and_then(|m| m.display_name.as_deref());
        if let Some(ctx) = crate::utils::calculate_context_usage(
            transcript,
            model_name,
            input.session_id.as_deref(),
            None,
        ) {
            core_vars.insert(
                "context_pct".into(),
                format!("{}", ctx.percentage.round() as u32),
            );
        }
    }

    // Cost (raw values for display)
    if let Some(cost_data) = &input.cost {
        if let Some(tc) = cost_data.total_cost_usd {
            core_vars.insert("cost".into(), format!("${:.2}", tc));
        }
    }
    if daily_total > 0.0 {
        core_vars.insert("daily_total".into(), format!("${:.2}", daily_total));
    }

    // --- Print results grouped by provider ---
    // First, categorise provider vars by prefix
    let mut git_vars: BTreeMap<String, String> = BTreeMap::new();
    let mut stats_vars: BTreeMap<String, String> = BTreeMap::new();
    let mut gsd_vars: BTreeMap<String, String> = BTreeMap::new();
    let mut other_vars: BTreeMap<String, String> = BTreeMap::new();

    for (key, value) in &provider_vars {
        if key.starts_with("git") {
            git_vars.insert(key.clone(), value.clone());
        } else if key.starts_with("stats_") {
            stats_vars.insert(key.clone(), value.clone());
        } else if key.starts_with("gsd_") {
            gsd_vars.insert(key.clone(), value.clone());
        } else {
            other_vars.insert(key.clone(), value.clone());
        }
    }

    // Helper to print a variable group
    let print_group = |name: &str, vars: &BTreeMap<String, String>| {
        if vars.is_empty() {
            return;
        }
        println!("=== {} ===", name);
        for (key, value) in vars {
            if value.is_empty() {
                println!("  {} = (empty)", key);
            } else {
                println!("  {} = {:?}", key, value);
            }
        }
        println!();
    };

    println!("Template Variables");
    println!("==================");
    if cli.no_color {
        println!("(Colors disabled)");
    }
    println!();

    print_group("git", &git_vars);
    print_group("stats", &stats_vars);
    print_group("gsd", &gsd_vars);
    if !other_vars.is_empty() {
        print_group("other", &other_vars);
    }
    print_group("core", &core_vars);

    // Print template info
    println!("=== template ===");
    println!(
        "  Default template: {}",
        include_str!("../templates/default.tmpl").trim()
    );
    let user_tmpl = dirs::config_dir().map(|d| d.join("claudia-statusline").join("template.tmpl"));
    if let Some(ref path) = user_tmpl {
        if path.exists() {
            println!("  User override:    {} (active)", path.display());
        } else {
            println!(
                "  User override:    {} (not found, using default)",
                path.display()
            );
        }
    }
    println!();

    Ok(())
}
