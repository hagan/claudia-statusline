// Check Turso database schema version
// Run with: cargo run --example check_turso_version --features turso-sync

use statusline::config::get_config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = get_config();

    if !config.sync.enabled {
        eprintln!("❌ Error: Sync is not enabled in config");
        std::process::exit(1);
    }

    let database_url = &config.sync.turso.database_url;
    let auth_token = &config.sync.turso.auth_token;

    // Resolve auth token
    let resolved_token = if auth_token.starts_with("${") && auth_token.ends_with('}') {
        let var_name = &auth_token[2..auth_token.len() - 1];
        std::env::var(var_name)?
    } else if let Some(var_name) = auth_token.strip_prefix('$') {
        std::env::var(var_name)?
    } else {
        auth_token.clone()
    };

    // Connect to Turso
    use libsql::Builder;
    let db = Builder::new_remote(database_url.clone(), resolved_token)
        .build()
        .await?;

    let conn = db.connect()?;

    println!("📊 Turso Database Schema Information\n");
    println!("Database: {}\n", database_url);

    // Get current version
    let mut rows = conn
        .query("SELECT MAX(version) FROM schema_migrations", ())
        .await?;

    let current_version: u32 = if let Some(row) = rows.next().await? {
        row.get::<i64>(0).unwrap_or(0) as u32
    } else {
        0
    };

    println!("Current schema version: v{}\n", current_version);

    // Show all applied migrations
    println!("Migration history:");
    println!("─────────────────────────────────────────────────────────────");

    let mut rows = conn
        .query(
            "SELECT version, applied_at, description, execution_time_ms
             FROM schema_migrations
             ORDER BY version",
            (),
        )
        .await?;

    while let Some(row) = rows.next().await? {
        let version: i64 = row.get(0)?;
        let applied_at: String = row.get(1)?;
        let description: Option<String> = row.get(2).ok();
        let exec_time: Option<i64> = row.get(3).ok();

        print!("v{} - {}", version, applied_at);
        if let Some(desc) = description {
            print!(" - {}", desc);
        }
        if let Some(ms) = exec_time {
            print!(" ({}ms)", ms);
        }
        println!();
    }

    println!("\n✅ Schema version tracking is active!");

    Ok(())
}
