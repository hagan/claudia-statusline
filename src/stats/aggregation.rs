//! Aggregation statistics: daily, monthly, and all-time totals.
//!
//! Contains the `DailyStats`, `MonthlyStats`, and `AllTimeStats` structs
//! and the `get_daily_total` utility function.

use serde::{Deserialize, Serialize};

use super::StatsData;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyStats {
    pub total_cost: f64,
    pub sessions: Vec<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthlyStats {
    pub total_cost: f64,
    pub sessions: usize,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllTimeStats {
    pub total_cost: f64,
    pub sessions: usize,
    pub since: String,
}

/// Get the daily total from stats data
#[allow(dead_code)]
pub fn get_daily_total(data: &StatsData) -> f64 {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    data.daily.get(&today).map(|d| d.total_cost).unwrap_or(0.0)
}
