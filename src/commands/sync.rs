//! `sync` subcommand handlers (requires the `turso-sync` feature).
//!
//! The entire module is gated behind `#[cfg(feature = "turso-sync")] mod sync;`
//! in `commands.rs`, so it only compiles under the feature. The per-function
//! `#[cfg(feature = "turso-sync")]` attributes that previously guarded these
//! handlers in `main.rs` are therefore redundant here.

use crate::display::Colors;
use crate::error::Result;

/// Handle sync commands (status, push, pull)
pub(crate) fn handle_sync_command(
    status: bool,
    push: bool,
    pull: bool,
    dry_run: bool,
) -> Result<()> {
    use crate::config::Config;

    // Load configuration
    let config = Config::load()?;
    let mut sync_manager = crate::sync::SyncManager::new(config.sync.clone());

    // Determine which action to take
    if status || (!push && !pull) {
        // Show status (default if no flags specified)
        return show_sync_status(&sync_manager);
    } else if push {
        // Push to remote
        return handle_sync_push(&mut sync_manager, dry_run);
    } else if pull {
        // Pull from remote
        return handle_sync_pull(&mut sync_manager, dry_run);
    }

    Ok(())
}

/// Handle push command
fn handle_sync_push(sync_manager: &mut crate::sync::SyncManager, dry_run: bool) -> Result<()> {
    println!("{}Pushing to remote{}", Colors::cyan(), Colors::reset());
    if dry_run {
        println!(
            "{}[DRY RUN MODE - No changes will be made]{}",
            Colors::yellow(),
            Colors::reset()
        );
    }
    println!();

    match sync_manager.push(dry_run) {
        Ok(result) => {
            println!("{}✅ Push completed{}", Colors::green(), Colors::reset());
            println!();
            println!("Summary:");
            println!("  Sessions: {} pushed", result.sessions_pushed);
            println!("  Daily stats: {} pushed", result.daily_stats_pushed);
            println!("  Monthly stats: {} pushed", result.monthly_stats_pushed);

            if result.dry_run {
                println!();
                println!(
                    "{}This was a dry run - no data was actually pushed.{}",
                    Colors::yellow(),
                    Colors::reset()
                );
            }
        }
        Err(e) => {
            eprintln!("{}❌ Push failed: {}{}", Colors::red(), e, Colors::reset());
            return Err(e);
        }
    }

    Ok(())
}

/// Handle pull command
fn handle_sync_pull(sync_manager: &mut crate::sync::SyncManager, dry_run: bool) -> Result<()> {
    println!("{}Pulling from remote{}", Colors::cyan(), Colors::reset());
    if dry_run {
        println!(
            "{}[DRY RUN MODE - No changes will be made]{}",
            Colors::yellow(),
            Colors::reset()
        );
    }
    println!();

    match sync_manager.pull(dry_run) {
        Ok(result) => {
            println!("{}✅ Pull completed{}", Colors::green(), Colors::reset());
            println!();
            println!("Summary:");
            println!("  Sessions: {} pulled", result.sessions_pulled);
            println!("  Daily stats: {} pulled", result.daily_stats_pulled);
            println!("  Monthly stats: {} pulled", result.monthly_stats_pulled);
            println!("  Conflicts resolved: {}", result.conflicts_resolved);

            if result.dry_run {
                println!();
                println!(
                    "{}This was a dry run - no data was actually pulled.{}",
                    Colors::yellow(),
                    Colors::reset()
                );
            }
        }
        Err(e) => {
            eprintln!("{}❌ Pull failed: {}{}", Colors::red(), e, Colors::reset());
            return Err(e);
        }
    }

    Ok(())
}

/// Show sync status and configuration
fn show_sync_status(_sync_manager: &crate::sync::SyncManager) -> Result<()> {
    use crate::config::Config;

    let config = Config::load()?;

    println!("{}Sync Status{}", Colors::cyan(), Colors::reset());
    println!("============");
    println!();

    println!("Configuration:");
    println!(
        "  Sync enabled: {}",
        if config.sync.enabled { "✅" } else { "❌" }
    );
    println!("  Provider: {}", config.sync.provider);
    println!("  Sync interval: {}s", config.sync.sync_interval_seconds);
    println!(
        "  Quota warning threshold: {:.0}%",
        config.sync.soft_quota_fraction * 100.0
    );
    println!();

    if config.sync.enabled {
        println!("Turso Configuration:");
        if !config.sync.turso.database_url.is_empty() {
            println!("  Database URL: {}", config.sync.turso.database_url);
        } else {
            println!(
                "  Database URL: {}(not configured){}",
                Colors::red(),
                Colors::reset()
            );
        }

        if !config.sync.turso.auth_token.is_empty() {
            if config.sync.turso.auth_token.starts_with('$') {
                println!("  Auth token: {} (env var)", config.sync.turso.auth_token);
            } else {
                println!("  Auth token: *** (configured)");
            }
        } else {
            println!(
                "  Auth token: {}(not configured){}",
                Colors::red(),
                Colors::reset()
            );
        }
        println!();

        // Test connection
        println!("Testing connection...");
        // Note: We need a mutable reference for test_connection
        // But we received an immutable reference, so we'll create a temp copy
        let mut temp_manager = crate::sync::SyncManager::new(config.sync.clone());

        match temp_manager.test_connection() {
            Ok(connected) => {
                if connected {
                    println!(
                        "  Connection: {}✅ Connected{}",
                        Colors::green(),
                        Colors::reset()
                    );
                } else {
                    println!(
                        "  Connection: {}❌ Not connected{}",
                        Colors::red(),
                        Colors::reset()
                    );
                    if let Some(err) = temp_manager.status().error_message.as_ref() {
                        println!("  Error: {}", err);
                    }
                }
            }
            Err(e) => {
                println!(
                    "  Connection: {}❌ Error: {}{}",
                    Colors::red(),
                    e,
                    Colors::reset()
                );
            }
        }
    } else {
        println!("Sync is disabled. To enable:");
        println!("  1. Edit your config file with sync settings");
        println!("  2. Set sync.enabled = true");
        println!("  3. Configure Turso database URL and token");
        println!();
        println!("See: statusline generate-config for example configuration");
    }

    Ok(())
}
