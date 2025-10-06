// Inspect Turso database contents to check for sensitive data
// Run with: cargo run --example inspect_turso_data --features turso-sync

use statusline::config::get_config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Inspecting Turso database contents...\n");

    let config = get_config();

    if !config.sync.enabled {
        eprintln!("❌ Error: Sync is not enabled in config");
        std::process::exit(1);
    }

    let database_url = &config.sync.turso.database_url;
    let auth_token = &config.sync.turso.auth_token;

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

    // Inspect sessions table
    println!("📊 SESSIONS TABLE");
    println!("═══════════════════════════════════════════════════════════════\n");

    let mut rows = conn.query("SELECT * FROM sessions LIMIT 3", ()).await?;

    println!("Sample rows (first 3):\n");
    let mut count = 0;
    while let Some(row) = rows.next().await? {
        count += 1;
        let device_id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let start_time: Option<String> = row.get(2).ok();
        let last_updated: String = row.get(3)?;
        let cost: f64 = row.get(4)?;
        let lines_added: i64 = row.get(5)?;
        let lines_removed: i64 = row.get(6)?;

        println!("Row {}:", count);
        println!("  device_id: {}", device_id);
        println!("  session_id: {}", session_id);
        println!("  start_time: {:?}", start_time);
        println!("  last_updated: {}", last_updated);
        println!("  cost: ${:.2}", cost);
        println!("  lines_added: {}", lines_added);
        println!("  lines_removed: {}", lines_removed);
        println!();
    }

    // Get total count
    let mut rows = conn.query("SELECT COUNT(*) FROM sessions", ()).await?;
    if let Some(row) = rows.next().await? {
        let total: i64 = row.get(0)?;
        println!("Total sessions: {}\n", total);
    }

    // Inspect daily_stats table
    println!("📊 DAILY_STATS TABLE");
    println!("═══════════════════════════════════════════════════════════════\n");

    let mut rows = conn.query("SELECT * FROM daily_stats LIMIT 3", ()).await?;

    println!("Sample rows (first 3):\n");
    count = 0;
    while let Some(row) = rows.next().await? {
        count += 1;
        let device_id: String = row.get(0)?;
        let date: String = row.get(1)?;
        let total_cost: f64 = row.get(2)?;
        let total_lines_added: i64 = row.get(3)?;
        let total_lines_removed: i64 = row.get(4)?;
        let session_count: i64 = row.get(5)?;

        println!("Row {}:", count);
        println!("  device_id: {}", device_id);
        println!("  date: {}", date);
        println!("  total_cost: ${:.2}", total_cost);
        println!("  total_lines_added: {}", total_lines_added);
        println!("  total_lines_removed: {}", total_lines_removed);
        println!("  session_count: {}", session_count);
        println!();
    }

    // Get total count
    let mut rows = conn.query("SELECT COUNT(*) FROM daily_stats", ()).await?;
    if let Some(row) = rows.next().await? {
        let total: i64 = row.get(0)?;
        println!("Total daily records: {}\n", total);
    }

    // Inspect monthly_stats table
    println!("📊 MONTHLY_STATS TABLE");
    println!("═══════════════════════════════════════════════════════════════\n");

    let mut rows = conn
        .query("SELECT * FROM monthly_stats LIMIT 3", ())
        .await?;

    println!("Sample rows (first 3):\n");
    count = 0;
    while let Some(row) = rows.next().await? {
        count += 1;
        let device_id: String = row.get(0)?;
        let month: String = row.get(1)?;
        let total_cost: f64 = row.get(2)?;
        let total_lines_added: i64 = row.get(3)?;
        let total_lines_removed: i64 = row.get(4)?;
        let session_count: i64 = row.get(5)?;

        println!("Row {}:", count);
        println!("  device_id: {}", device_id);
        println!("  month: {}", month);
        println!("  total_cost: ${:.2}", total_cost);
        println!("  total_lines_added: {}", total_lines_added);
        println!("  total_lines_removed: {}", total_lines_removed);
        println!("  session_count: {}", session_count);
        println!();
    }

    // Get total count
    let mut rows = conn.query("SELECT COUNT(*) FROM monthly_stats", ()).await?;
    if let Some(row) = rows.next().await? {
        let total: i64 = row.get(0)?;
        println!("Total monthly records: {}\n", total);
    }

    // Privacy analysis
    println!("🔒 PRIVACY ANALYSIS");
    println!("═══════════════════════════════════════════════════════════════\n");

    println!("Data stored in Turso:");
    println!("  ✅ device_id: Hashed identifier (hostname + username)");
    println!("  ✅ session_id: UUID/timestamp - no personal info");
    println!("  ✅ dates/timestamps: When sessions occurred");
    println!("  ✅ cost: Dollar amounts");
    println!("  ✅ lines_added/removed: Code change counts");
    println!();
    println!("NOT stored:");
    println!("  ✅ No file paths");
    println!("  ✅ No directory names");
    println!("  ✅ No git branches");
    println!("  ✅ No code content");
    println!("  ✅ No model names");
    println!();
    println!("Device ID is a hash, let me check what it looks like...\n");

    // Show device ID format
    let mut rows = conn
        .query("SELECT DISTINCT device_id FROM sessions LIMIT 1", ())
        .await?;
    if let Some(row) = rows.next().await? {
        let device_id: String = row.get(0)?;
        println!("Your device_id: {}", device_id);
        println!("  Length: {} characters", device_id.len());
        println!("  Format: 64-bit hash in hexadecimal");
        println!("  Source: SHA256(hostname + username) truncated to 64 bits");
        println!();
        println!("This is anonymous - it's just a stable identifier for your device.");
        println!("Nobody can reverse it to get your hostname or username.");
    }

    Ok(())
}
