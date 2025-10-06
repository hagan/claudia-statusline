// Setup Turso database schema
// Run with: cargo run --example setup_schema --features turso-sync

use statusline::config::get_config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔧 Setting up Turso database schema...\n");

    let config = get_config();

    if !config.sync.enabled {
        eprintln!("❌ Error: Sync is not enabled in config");
        std::process::exit(1);
    }

    let database_url = &config.sync.turso.database_url;
    let auth_token = &config.sync.turso.auth_token;

    println!("📍 Database: {}", database_url);
    println!("🔐 Auth token: {} characters\n", auth_token.len());

    // Resolve auth token (handle ${VAR} syntax)
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

    println!("✅ Connected to Turso\n");
    println!("Creating tables...\n");

    // Create sessions table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS sessions (
            device_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            start_time TEXT,
            last_updated TEXT NOT NULL,
            cost REAL NOT NULL DEFAULT 0.0,
            lines_added INTEGER NOT NULL DEFAULT 0,
            lines_removed INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (device_id, session_id)
        )",
        (),
    )
    .await?;
    println!("  ✅ Created sessions table");

    // Create daily_stats table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS daily_stats (
            device_id TEXT NOT NULL,
            date TEXT NOT NULL,
            total_cost REAL NOT NULL DEFAULT 0.0,
            total_lines_added INTEGER NOT NULL DEFAULT 0,
            total_lines_removed INTEGER NOT NULL DEFAULT 0,
            session_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (device_id, date)
        )",
        (),
    )
    .await?;
    println!("  ✅ Created daily_stats table");

    // Create monthly_stats table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS monthly_stats (
            device_id TEXT NOT NULL,
            month TEXT NOT NULL,
            total_cost REAL NOT NULL DEFAULT 0.0,
            total_lines_added INTEGER NOT NULL DEFAULT 0,
            total_lines_removed INTEGER NOT NULL DEFAULT 0,
            session_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (device_id, month)
        )",
        (),
    )
    .await?;
    println!("  ✅ Created monthly_stats table");

    println!("\nCreating indexes...\n");

    // Create indexes
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_device_updated
         ON sessions(device_id, last_updated DESC)",
        (),
    )
    .await?;
    println!("  ✅ Created idx_sessions_device_updated");

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_daily_device_date
         ON daily_stats(device_id, date DESC)",
        (),
    )
    .await?;
    println!("  ✅ Created idx_daily_device_date");

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_monthly_device_month
         ON monthly_stats(device_id, month DESC)",
        (),
    )
    .await?;
    println!("  ✅ Created idx_monthly_device_month");

    println!("\n🎉 Database schema setup complete!\n");
    println!("Next steps:");
    println!("  1. Test sync: statusline sync --status");
    println!("  2. Push data: statusline sync --push --dry-run");
    println!("  3. Actual push: statusline sync --push");

    Ok(())
}
