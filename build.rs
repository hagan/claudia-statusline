use std::env;
use std::fs;
use std::process::Command;

fn main() {
    // Read version from VERSION file or use Cargo.toml version
    let version = fs::read_to_string("VERSION")
        .unwrap_or_else(|_| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string()))
        .trim()
        .to_string();

    // Get git commit hash
    let git_hash = Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
        .trim()
        .to_string();

    // Get git branch
    let git_branch = Command::new("git")
        .args(&["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
        .trim()
        .to_string();

    // Check if working directory is clean
    let git_dirty = Command::new("git")
        .args(&["status", "--porcelain"])
        .output()
        .ok()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(false);

    // Get number of commits since last tag
    let commits_since_tag = Command::new("git")
        .args(&["describe", "--tags", "--long", "--always"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
        .trim()
        .to_string();

    // Get build timestamp
    let build_date = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

    // Get build profile (debug/release)
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    // Get rustc version
    let rustc_version = Command::new("rustc")
        .args(&["--version"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
        .trim()
        .to_string();

    // Set environment variables for the build
    println!("cargo:rustc-env=CLAUDIA_VERSION={}", version);
    println!("cargo:rustc-env=CLAUDIA_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=CLAUDIA_GIT_BRANCH={}", git_branch);
    println!("cargo:rustc-env=CLAUDIA_GIT_DIRTY={}", git_dirty);
    println!("cargo:rustc-env=CLAUDIA_GIT_DESCRIBE={}", commits_since_tag);
    println!("cargo:rustc-env=CLAUDIA_BUILD_DATE={}", build_date);
    println!("cargo:rustc-env=CLAUDIA_BUILD_PROFILE={}", profile);
    println!("cargo:rustc-env=CLAUDIA_RUSTC_VERSION={}", rustc_version);

    // Tell Cargo to rerun if VERSION file or git state changes
    println!("cargo:rerun-if-changed=VERSION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}