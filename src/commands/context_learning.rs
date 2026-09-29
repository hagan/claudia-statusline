//! `context-learning` subcommand handler: status, details, reset, rebuild.

use crate::error::Result;

/// Handle context learning command
pub(crate) fn handle_context_learning_command(
    status: bool,
    reset: Option<String>,
    details: Option<String>,
    reset_all: bool,
    rebuild: bool,
) -> Result<()> {
    use crate::common::get_data_dir;
    use crate::context_learning::ContextLearner;
    use crate::database::SqliteDatabase;
    use crate::display::Colors;

    // Create context learner
    let db_path = get_data_dir().join("stats.db");
    let db = SqliteDatabase::new(&db_path)?;
    let learner = ContextLearner::new(db);

    // Handle reset before rebuild (allows --reset-all --rebuild combination)
    if reset_all {
        cli_println!();
        cli_println!(
            "{}Resetting all learned context data...{}",
            Colors::yellow(),
            Colors::reset()
        );
        learner.reset_all()?;
        cli_println!(
            "{}✓ All learning data cleared{}",
            Colors::green(),
            Colors::reset()
        );
        cli_println!();

        // Don't return if rebuild is also requested
        if !rebuild {
            return Ok(());
        }
    }

    // Handle rebuild from session history (can be combined with --reset-all)
    if rebuild {
        cli_println!(
            "{}Rebuilding learned context windows from session history...{}",
            Colors::cyan(),
            Colors::reset()
        );
        cli_println!();

        learner.rebuild_from_sessions()?;

        cli_println!();
        cli_println!("{}✓ Rebuild complete{}", Colors::green(), Colors::reset());
        cli_println!();
        cli_println!(
            "{}Use --status to see the results{}",
            Colors::cyan(),
            Colors::reset()
        );
        cli_println!();
        return Ok(());
    }

    // Handle reset for specific model
    if let Some(model_name) = reset {
        // Sanitize model name for terminal output
        let sanitized_model = crate::utils::sanitize_for_terminal(&model_name);

        cli_println!(
            "{}Resetting learned context data for: {}{}",
            Colors::yellow(),
            sanitized_model,
            Colors::reset()
        );
        learner.reset_model(&model_name)?;
        cli_println!(
            "{}✓ Learning data cleared for {}{}",
            Colors::green(),
            sanitized_model,
            Colors::reset()
        );
        return Ok(());
    }

    // Handle details for specific model
    if let Some(model_name) = details {
        // Sanitize model name once up front for both success and error paths
        let sanitized_model = crate::utils::sanitize_for_terminal(&model_name);

        if let Some(record) = learner.get_learned_window_details(&model_name)? {
            cli_println!();
            cli_println!(
                "{}Learned Context Window Details for {}{}",
                Colors::cyan(),
                sanitized_model,
                Colors::reset()
            );
            cli_println!("{}", "=".repeat(60));
            cli_println!();
            cli_println!(
                "  Observed Max Tokens:     {}{}{}",
                Colors::green(),
                record.observed_max_tokens,
                Colors::reset()
            );
            cli_println!(
                "  Confidence Score:        {}{:.1}%{}",
                Colors::green(),
                record.confidence_score * 100.0,
                Colors::reset()
            );
            cli_println!("  Ceiling Observations:    {}", record.ceiling_observations);
            cli_println!("  Compaction Count:        {}", record.compaction_count);
            cli_println!("  First Seen:              {}", record.first_seen);
            cli_println!("  Last Updated:            {}", record.last_updated);
            cli_println!();
            cli_println!("{}Audit Trail:{}", Colors::cyan(), Colors::reset());
            cli_println!(
                "  Workspace:               {}",
                crate::utils::sanitize_for_terminal(
                    record.workspace_dir.as_deref().unwrap_or("<unknown>")
                )
            );
            cli_println!(
                "  Device ID:               {}",
                crate::utils::sanitize_for_terminal(
                    record.device_id.as_deref().unwrap_or("<unknown>")
                )
            );
            cli_println!();

            let config = crate::config::get_config();
            if record.confidence_score >= config.context.learning_confidence_threshold {
                cli_println!(
                    "  {}✓ Confidence threshold met - using learned value{}",
                    Colors::green(),
                    Colors::reset()
                );
            } else {
                cli_println!(
                    "  {}⚠ Confidence too low - using default value{}",
                    Colors::yellow(),
                    Colors::reset()
                );
                cli_println!(
                    "    Threshold: {:.1}%",
                    config.context.learning_confidence_threshold * 100.0
                );
            }
            cli_println!();
        } else {
            cli_println!(
                "{}No learning data found for: {}{}",
                Colors::yellow(),
                sanitized_model,
                Colors::reset()
            );
        }
        return Ok(());
    }

    // Handle status (show all)
    if status {
        let all_records = learner.get_all_learned_windows()?;

        if all_records.is_empty() {
            cli_println!();
            cli_println!(
                "{}No learned context windows yet{}",
                Colors::yellow(),
                Colors::reset()
            );
            cli_println!();
            cli_println!(
                "{}To enable adaptive learning:{}",
                Colors::cyan(),
                Colors::reset()
            );
            cli_println!("  1. Edit config: statusline generate-config");
            cli_println!("  2. Set [context] adaptive_learning = true");
            cli_println!("  3. Use Claude normally - learning happens automatically");
            cli_println!();
            return Ok(());
        }

        cli_println!();
        cli_println!(
            "{}Learned Context Windows{}",
            Colors::cyan(),
            Colors::reset()
        );
        cli_println!("{}", "=".repeat(80));
        cli_println!();
        cli_println!(
            "{:<25} {:>12} {:>10} {:>10} {:>12}",
            "Model",
            "Max Tokens",
            "Confidence",
            "Compactions",
            "Observations"
        );
        cli_println!("{}", "-".repeat(80));

        for record in all_records {
            let confidence_color = if record.confidence_score >= 0.7 {
                Colors::green()
            } else if record.confidence_score >= 0.4 {
                Colors::yellow()
            } else {
                Colors::red()
            };

            cli_println!(
                "{:<25} {:>12} {}{:>9.1}%{} {:>10} {:>12}",
                crate::utils::sanitize_for_terminal(&record.model_name),
                record.observed_max_tokens,
                confidence_color,
                record.confidence_score * 100.0,
                Colors::reset(),
                record.compaction_count,
                record.ceiling_observations
            );
        }
        cli_println!();

        let config = crate::config::get_config();
        if config.context.adaptive_learning {
            cli_println!(
                "{}✓ Adaptive learning is enabled{}",
                Colors::green(),
                Colors::reset()
            );
        } else {
            cli_println!(
                "{}⚠ Adaptive learning is disabled in config{}",
                Colors::yellow(),
                Colors::reset()
            );
        }
        cli_println!(
            "  Confidence threshold: {:.1}%",
            config.context.learning_confidence_threshold * 100.0
        );
        cli_println!();
        cli_println!(
            "{}Use --details <model> to see audit trail (workspace/device){}",
            Colors::cyan(),
            Colors::reset()
        );
        cli_println!();

        return Ok(());
    }

    // No flags specified - show help
    cli_println!();
    cli_println!(
        "{}Adaptive Context Learning Commands{}",
        Colors::cyan(),
        Colors::reset()
    );
    cli_println!();
    cli_println!("  statusline context-learning --status");
    cli_println!("    Show all learned context windows");
    cli_println!();
    cli_println!("  statusline context-learning --details <model>");
    cli_println!("    Show detailed observations for a specific model");
    cli_println!();
    cli_println!("  statusline context-learning --reset <model>");
    cli_println!("    Reset learning data for a specific model");
    cli_println!();
    cli_println!("  statusline context-learning --reset-all");
    cli_println!("    Reset all learning data");
    cli_println!();

    Ok(())
}
