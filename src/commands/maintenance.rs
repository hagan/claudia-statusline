//! `db-maintain` subcommand handler: WAL checkpoint, optimize, vacuum, prune.

use crate::error::Result;

/// Perform database maintenance operations
pub(crate) fn perform_database_maintenance(
    force_vacuum: bool,
    no_prune: bool,
    quiet: bool,
) -> Result<()> {
    if !quiet {
        cli_println!("🔧 Starting database maintenance...\n");
    }

    // Get database path
    let db_path = crate::stats::StatsData::get_sqlite_path()?;
    if !db_path.exists() {
        if !quiet {
            cli_println!("❌ Database not found at: {}", db_path.display());
        }
        return Err(crate::error::StatuslineError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Database file not found",
        )));
    }

    // Get initial size
    let initial_size = std::fs::metadata(&db_path)?.len() as f64 / (1024.0 * 1024.0);
    if !quiet {
        cli_println!("📊 Initial database size: {:.2} MB", initial_size);
    }

    // Perform maintenance operations
    let maintenance_result = crate::database::perform_maintenance(force_vacuum, no_prune, quiet)?;

    // Get final size
    let final_size = std::fs::metadata(&db_path)?.len() as f64 / (1024.0 * 1024.0);

    if !quiet {
        cli_println!("\n📊 Final database size: {:.2} MB", final_size);

        if final_size < initial_size {
            let saved = initial_size - final_size;
            let percent = (saved / initial_size) * 100.0;
            cli_println!("💾 Space saved: {:.2} MB ({:.1}%)", saved, percent);
        }

        cli_println!("\n📋 Maintenance summary:");
        cli_println!(
            "  ✅ WAL checkpoint: {}",
            if maintenance_result.checkpoint_done {
                "completed"
            } else {
                "not needed"
            }
        );
        cli_println!(
            "  ✅ Optimization: {}",
            if maintenance_result.optimize_done {
                "completed"
            } else {
                "not needed"
            }
        );
        cli_println!(
            "  ✅ Vacuum: {}",
            if maintenance_result.vacuum_done {
                "completed"
            } else {
                "not needed"
            }
        );
        cli_println!(
            "  ✅ Pruning: {}",
            if maintenance_result.prune_done {
                format!("removed {} old records", maintenance_result.records_pruned)
            } else if no_prune {
                "skipped".to_string()
            } else {
                "not needed".to_string()
            }
        );
        cli_println!(
            "  ✅ Integrity check: {}",
            if maintenance_result.integrity_ok {
                "passed"
            } else {
                "FAILED"
            }
        );

        if maintenance_result.integrity_ok {
            cli_println!("\n✅ Database maintenance completed successfully!");
        } else {
            cli_println!(
                "\n❌ Database integrity check failed! Consider rebuilding from JSON backup."
            );
        }
    }

    // Exit with non-zero if integrity check failed
    if !maintenance_result.integrity_ok {
        std::process::exit(1);
    }

    Ok(())
}
