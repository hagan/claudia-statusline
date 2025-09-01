//! Example of embedding the statusline library in other tools
//!
//! This demonstrates how to use the public API functions to integrate
//! the statusline functionality into your own applications.

use statusline::{render_from_json, render_statusline, Cost, Model, StatuslineInput, Workspace};

// Get version from VERSION file at compile time
const VERSION: &str = include_str!("../VERSION");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Embedding Example for Statusline Library ===\n");

    // Example 1: Using the render_from_json function
    println!("1. Rendering from JSON string:");
    let json_input = r#"{
        "workspace": {"current_dir": "/home/user/my-project"},
        "model": {"display_name": "Claude 3.5 Sonnet"},
        "cost": {
            "total_cost_usd": 2.50,
            "total_lines_added": 120,
            "total_lines_removed": 30
        },
        "session_id": "example-session-123"
    }"#;

    // Render without updating stats (preview mode)
    let output = render_from_json(json_input, false)?;
    println!("Output: {}", output);
    println!();

    // Example 2: Using the render_statusline function with structured data
    println!("2. Rendering from structured input:");
    let input = StatuslineInput {
        workspace: Some(Workspace {
            current_dir: Some("/home/user/awesome-project".to_string()),
        }),
        model: Some(Model {
            display_name: Some("Claude 3 Opus".to_string()),
        }),
        cost: Some(Cost {
            total_cost_usd: Some(15.75),
            total_lines_added: Some(500),
            total_lines_removed: Some(80),
        }),
        session_id: Some("structured-example".to_string()),
        transcript: None,
    };

    // Render with stats update enabled
    let output = render_statusline(&input, true)?;
    println!("Output: {}", output);
    println!();

    // Example 3: Minimal input (just workspace)
    println!("3. Minimal rendering example:");
    let minimal_json = r#"{"workspace": {"current_dir": "/tmp/test"}}"#;
    let output = render_from_json(minimal_json, false)?;
    println!("Output: {}", output);
    println!();

    // Example 4: Testing NO_COLOR environment variable
    println!("4. Testing NO_COLOR support:");

    // Enable NO_COLOR for deterministic output
    std::env::set_var("NO_COLOR", "1");
    let output_no_color = render_from_json(json_input, false)?;
    println!("NO_COLOR output: {}", output_no_color);

    // Disable NO_COLOR for colorized output
    std::env::remove_var("NO_COLOR");
    let output_color = render_from_json(json_input, false)?;
    println!("Color output: {}", output_color);
    println!();

    // Example 5: Error handling
    println!("5. Error handling example:");
    let invalid_json = r#"{"invalid": json}"#;
    match render_from_json(invalid_json, false) {
        Ok(output) => println!("Unexpected success: {}", output),
        Err(e) => println!("Expected error: {}", e),
    }

    println!("\n=== Integration Guide ===");
    println!("To integrate this library into your application:");
    println!(
        "1. Add 'statusline = \"{}\"' to your Cargo.toml",
        VERSION.trim()
    );
    println!("2. Import the functions: use statusline::{{render_from_json, render_statusline}};");
    println!(
        "3. Call render_from_json() for JSON input or render_statusline() for structured input"
    );
    println!("4. Set update_stats=true when you want persistent stats tracking");
    println!("5. Set update_stats=false for preview/testing without affecting persistent data");

    Ok(())
}
