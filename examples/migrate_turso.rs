// Turso database migration tool
// Run with: cargo run --example migrate_turso --features turso-sync

use statusline::config::get_config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔧 Turso Database Migration Tool\n");

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

    println!("✅ Connected to Turso\n");

    // Create schema_migrations table
    println!("Creating schema_migrations table...");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL,
            description TEXT,
            execution_time_ms INTEGER
        )",
        (),
    )
    .await?;
    println!("  ✅ schema_migrations table ready\n");

    // Check current version
    println!("Checking current schema version...");
    let current_version = get_current_version(&conn).await?;
    println!("  Current version: v{}\n", current_version);

    // Apply migrations
    let migrations = vec![
        (1, "Initial schema", initial_schema),
    ];

    let mut applied = 0;
    for (version, description, migration_fn) in migrations {
        if version > current_version {
            println!("Applying migration v{}: {}...", version, description);
            let start = std::time::Instant::now();

            migration_fn(&conn).await?;

            let elapsed = start.elapsed().as_millis();

            // Record migration
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at, description, execution_time_ms)
                 VALUES (?, datetime('now'), ?, ?)",
                libsql::params![version, description, elapsed as i64],
            )
            .await?;

            println!("  ✅ Applied in {}ms", elapsed);
            applied += 1;
        }
    }

    if applied == 0 {
        println!("✅ Schema is up to date (v{})", current_version);
    } else {
        println!("\n🎉 Successfully applied {} migration(s)", applied);
        let new_version = get_current_version(&conn).await?;
        println!("Schema version: v{} → v{}", current_version, new_version);
    }

    Ok(())
}

async fn get_current_version(conn: &libsql::Connection) -> Result<u32, Box<dyn std::error::Error>> {
    let mut rows = conn
        .query("SELECT MAX(version) FROM schema_migrations", ())
        .await?;

    if let Some(row) = rows.next().await? {
        if let Ok(version) = row.get::<i64>(0) {
            return Ok(version as u32);
        }
    }

    Ok(0)
}

async fn initial_schema(conn: &libsql::Connection) -> Result<(), Box<dyn std::error::Error>> {
    // This migration is a no-op if tables already exist
    // Just ensures the schema_migrations table is populated

    // Check if tables exist
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='sessions'",
            (),
        )
        .await?;

    if rows.next().await?.is_some() {
        println!("    (Tables already exist, recording migration)");
    } else {
        println!("    (Tables missing - run setup_schema.rs first!)");
    }

    Ok(())
}
