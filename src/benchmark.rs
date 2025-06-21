use anyhow::Result;
use chrono::{DateTime, Utc};
use console::Style;
use dashmap::DashMap;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::geyser_client::{GeyserStreamClient, GeyserTransaction};
use crate::shredlink_client::{ShredlinkClient, ShredlinkTransaction};

/// Stores timestamps for a transaction from multiple Geyser sources and Shredlink
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionTimestamp {
    pub geyser_timestamps: HashMap<String, u64>, // geyser_name -> timestamp
    pub shredlink_timestamp: Option<u64>,
}

impl TransactionTimestamp {
    pub fn new() -> Self {
        Self {
            geyser_timestamps: HashMap::new(),
            shredlink_timestamp: None,
        }
    }
    
    /// Calculate latency difference against each Geyser source (positive means Geyser is faster)
    pub fn latency_diffs_ms(&self) -> HashMap<String, i64> {
        let mut diffs = HashMap::new();
        
        if let Some(shredlink_ts) = self.shredlink_timestamp {
            for (geyser_name, geyser_ts) in &self.geyser_timestamps {
                let diff = *geyser_ts as i64 - shredlink_ts as i64;
                diffs.insert(geyser_name.clone(), diff);
            }
        }
        
        diffs
    }
    
    pub fn has_any_geyser(&self) -> bool {
        !self.geyser_timestamps.is_empty()
    }
}

/// Benchmark results summary for multiple Geyser sources vs Shredlink
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_seconds: f64,
    pub total_transactions: usize,
    pub shredlink_only_count: usize,
    pub geyser_results: HashMap<String, GeyserStats>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeyserStats {
    pub matched_transactions: usize,
    pub geyser_only_count: usize,
    pub average_latency_ms: f64,
    pub median_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub min_latency_ms: i64,
    pub max_latency_ms: i64,
    pub shredlink_wins_percentage: f64,
}

pub struct Benchmark {
    geyser_urls: HashMap<String, String>, // geyser_name -> url
    shredlink_url: String,
    transactions: Arc<DashMap<String, TransactionTimestamp>>,
    start_time: Instant,
}

impl Benchmark {
    pub fn new(geyser_urls: HashMap<String, String>, shredlink_url: String) -> Self {
        Self {
            geyser_urls,
            shredlink_url,
            transactions: Arc::new(DashMap::new()),
            start_time: Instant::now(),
        }
    }
    
    pub async fn run(&mut self, duration: Duration) -> Result<()> {
        self.start_time = Instant::now();
        
        // Setup channels for transaction streaming
        let mut geyser_channels = HashMap::new();
        let mut geyser_handlers = Vec::new();
        
        // Create channels for each Geyser source
        for (geyser_name, _) in &self.geyser_urls {
            let (tx, rx) = mpsc::unbounded_channel();
            geyser_channels.insert(geyser_name.clone(), tx);
            
            // Start handler for this Geyser source
            let handler = self.start_geyser_handler(geyser_name.clone(), rx).await;
            geyser_handlers.push(handler);
        }
        
        let (shredlink_tx, shredlink_rx) = mpsc::unbounded_channel();
        
        // Setup progress tracking
        let progress = self.create_progress_bar(duration);
        
        // Start Shredlink handler
        let _shredlink_handler = self.start_shredlink_handler(shredlink_rx).await;
        
        // Start clients in a simple way (avoiding async/Send issues)
        println!("🔄 Starting clients...");
        
        // Start all clients concurrently using join_all
        let mut client_futures: Vec<Pin<Box<dyn Future<Output = ()>>>> = Vec::new();
        
        // Create futures for all Geyser clients
        for (geyser_name, geyser_url) in self.geyser_urls.clone() {
            if let Some(tx) = geyser_channels.get(&geyser_name).cloned() {
                let mut client = GeyserStreamClient::new(geyser_url, None);
                let name = geyser_name.clone();
                client_futures.push(Box::pin(async move {
                    if let Err(e) = client.start(tx).await {
                        eprintln!("❌ {} failed: {}", name, e);
                    }
                }) as Pin<Box<dyn Future<Output = ()>>>);
            }
        }
        
        // Add Shredlink client future
        let mut shredlink_client = ShredlinkClient::new(self.shredlink_url.clone());
        client_futures.push(Box::pin(async move {
            if let Err(e) = shredlink_client.start(shredlink_tx).await {
                eprintln!("❌ Shredlink failed: {}", e);
            }
        }) as Pin<Box<dyn Future<Output = ()>>>);
        
        // Race all clients against the timer
        tokio::select! {
            _ = futures::future::join_all(client_futures) => {},
            _ = self.run_with_progress(duration, &progress) => {},
        }
        
        progress.finish_with_message("✅ Benchmark completed");
        Ok(())
    }
    
    /// Generate final benchmark report
    pub fn generate_report(&self) -> BenchmarkReport {
        let end_time = Utc::now();
        let start_time = end_time - chrono::Duration::from_std(self.start_time.elapsed()).unwrap();
        
        let mut geyser_results = HashMap::new();
        
        // Calculate stats for each Geyser source
        for geyser_name in self.geyser_urls.keys() {
            let latencies: Vec<i64> = self.transactions
                .iter()
                .filter_map(|entry| {
                    entry.latency_diffs_ms().get(geyser_name).copied()
                })
                .collect();
            
            let matched_count = latencies.len();
            let geyser_only_count = self.count_geyser_only(geyser_name);
            
            let stats = LatencyStats::calculate(&latencies);
            
            geyser_results.insert(geyser_name.clone(), GeyserStats {
                matched_transactions: matched_count,
                geyser_only_count,
                average_latency_ms: stats.average,
                median_latency_ms: stats.median,
                p95_latency_ms: stats.p95,
                p99_latency_ms: stats.p99,
                min_latency_ms: stats.min,
                max_latency_ms: stats.max,
                shredlink_wins_percentage: stats.shredlink_wins_percentage,
            });
        }
        
        BenchmarkReport {
            start_time,
            end_time,
            duration_seconds: self.start_time.elapsed().as_secs_f64(),
            total_transactions: self.transactions.len(),
            shredlink_only_count: self.count_shredlink_only(),
            geyser_results,
        }
    }
    
    pub fn print_report(&self, _format: &str) {
        let report = self.generate_report();
        self.print_table_report(&report)
    }
    
    // --- Private Implementation ---
    
    fn create_progress_bar(&self, duration: Duration) -> ProgressBar {
        let pb = ProgressBar::new(duration.as_secs());
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len}s {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb
    }
    
    async fn start_geyser_handler(&self, geyser_name: String, mut rx: mpsc::UnboundedReceiver<GeyserTransaction>) -> tokio::task::JoinHandle<()> {
        let transactions = Arc::clone(&self.transactions);
        
        tokio::spawn(async move {
            while let Some(transaction) = rx.recv().await {
                let timestamp = get_timestamp_ms();
                let signature = transaction.signature;
                
                println!("🟦 {}: {}", geyser_name, signature);
                
                transactions
                    .entry(signature.clone())
                    .and_modify(|entry| {
                        if !entry.geyser_timestamps.contains_key(&geyser_name) {
                            entry.geyser_timestamps.insert(geyser_name.clone(), timestamp);
                            
                            // Print latency if we have Shredlink data
                            if let Some(diff) = entry.latency_diffs_ms().get(&geyser_name) {
                                println!("⏱️  {}: {}ms ({})", geyser_name, diff, signature);
                            }
                        }
                    })
                    .or_insert_with(|| {
                        let mut entry = TransactionTimestamp::new();
                        entry.geyser_timestamps.insert(geyser_name.clone(), timestamp);
                        entry
                    });
            }
        })
    }
    
    async fn start_shredlink_handler(&self, mut rx: mpsc::UnboundedReceiver<ShredlinkTransaction>) -> tokio::task::JoinHandle<()> {
        let transactions = Arc::clone(&self.transactions);
        
        tokio::spawn(async move {
            while let Some(transaction) = rx.recv().await {
                let timestamp = get_timestamp_ms();
                
                let signature = match transaction.signatures.first() {
                    Some(sig_bytes) => bs58::encode(sig_bytes).into_string(),
                    None => {
                        println!("⚠️  Empty signature from Shredlink");
                        continue;
                    }
                };
                
                println!("🟪 Shredlink: {}", signature);
                
                transactions
                    .entry(signature.clone())
                    .and_modify(|entry| {
                        if entry.shredlink_timestamp.is_none() {
                            entry.shredlink_timestamp = Some(timestamp);
                            
                            // Print latencies for all Geyser sources
                            for (geyser_name, diff) in entry.latency_diffs_ms() {
                                println!("⏱️  {}: {}ms ({})", geyser_name, diff, signature);
                            }
                        }
                    })
                    .or_insert_with(|| {
                        let mut entry = TransactionTimestamp::new();
                        entry.shredlink_timestamp = Some(timestamp);
                        entry
                    });
            }
        })
    }
    
    async fn run_with_progress(&self, duration: Duration, progress: &ProgressBar) {
        let start = Instant::now();
        
        while start.elapsed() < duration {
            sleep(Duration::from_secs(1)).await;
            progress.inc(1);
            
            let stats = self.get_current_stats();
            progress.set_message(format!(
                "Total: {} | Matched: {} | Rate: {:.1}/s",
                stats.total,
                stats.matched,
                stats.rate
            ));
        }
    }
    
    fn get_current_stats(&self) -> CurrentStats {
        let total = self.transactions.len();
        let matched = self.transactions.iter().filter(|entry| {
            entry.has_any_geyser() && entry.shredlink_timestamp.is_some()
        }).count();
        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        let rate = if elapsed_secs > 0.0 { total as f64 / elapsed_secs } else { 0.0 };
        
        CurrentStats { total, matched, rate }
    }
    
    fn count_geyser_only(&self, geyser_name: &str) -> usize {
        self.transactions
            .iter()
            .filter(|entry| {
                entry.geyser_timestamps.contains_key(geyser_name) && entry.shredlink_timestamp.is_none()
            })
            .count()
    }
    
    fn count_shredlink_only(&self) -> usize {
        self.transactions
            .iter()
            .filter(|entry| entry.shredlink_timestamp.is_some() && entry.geyser_timestamps.is_empty())
            .count()
    }
    

    
    fn print_table_report(&self, report: &BenchmarkReport) {
        let green = Style::new().green();
        let cyan = Style::new().cyan();
        let red = Style::new().red();
        let yellow = Style::new().yellow();
        
        println!();
        println!("{}", green.apply_to("📊 MULTI-GEYSER BENCHMARK RESULTS"));
        println!("{}", green.apply_to("=".repeat(60)));
        
        // Basic stats
        println!("⏱️  Duration: {:.1}s", report.duration_seconds);
        println!("📈 Total Transactions: {}", report.total_transactions);
        println!();
        
        // Calculate overall win rate upfront
        let total_matches: usize = report.geyser_results.values().map(|stats| stats.matched_transactions).sum();
        let shredlink_wins: usize = report.geyser_results.values()
            .map(|stats| (stats.shredlink_wins_percentage * stats.matched_transactions as f64 / 100.0) as usize)
            .sum();
        let overall_win_rate = if total_matches > 0 { (shredlink_wins as f64 / total_matches as f64) * 100.0 } else { 0.0 };
        
        // Highlight the key metric with colors and formatting
        println!("{}", "═".repeat(60));
        let win_rate_text = format!("🏆 Shredlink Win Rate: {:.1}% ({} out of {} transactions)", overall_win_rate, shredlink_wins, total_matches);
        if overall_win_rate > 75.0 {
            println!("{}", green.apply_to(&format!("🔥 {}", win_rate_text)));
        } else if overall_win_rate > 50.0 {
            println!("{}", cyan.apply_to(&format!("⚡ {}", win_rate_text)));
        } else {
            println!("{}", yellow.apply_to(&win_rate_text));
        }
        println!("{}", "═".repeat(60));
        
        // Individual Geyser results
        println!("{}", cyan.apply_to("⚡ INDIVIDUAL GEYSER RESULTS"));
        println!("{}", cyan.apply_to("-".repeat(40)));
        
        for (geyser_name, stats) in &report.geyser_results {
            println!();
            println!("{}", yellow.apply_to(format!("🔗 {}", geyser_name)));
            println!("  🎯 Matched Transactions: {}", stats.matched_transactions);
            println!("  📊 Geyser Only: {}", stats.geyser_only_count);
            
            if stats.matched_transactions > 0 {
                let winner = if stats.average_latency_ms > 0.0 { "Shredlink" } else { "Geyser" };
                let avg_diff = stats.average_latency_ms.abs();
                
                println!("  🏆 Winner: {} (avg {:.1}ms faster)", winner, avg_diff);
                println!("  🎯 Shredlink wins: {:.1}% of transactions", stats.shredlink_wins_percentage);
                println!("  📊 Average latency: {:.1}ms | Median: {:.1}ms", stats.average_latency_ms, stats.median_latency_ms);
            } else {
                println!("  {}", red.apply_to("❌ No matched transactions"));
            }
        }
        
        // Shredlink Performance Summary
        println!();
        println!("{}", cyan.apply_to("🏁 SHREDLINK PERFORMANCE SUMMARY"));
        println!("{}", cyan.apply_to("-".repeat(40)));
        
        let total_matches: usize = report.geyser_results.values().map(|stats| stats.matched_transactions).sum();
        let shredlink_wins: usize = report.geyser_results.values()
            .map(|stats| (stats.shredlink_wins_percentage * stats.matched_transactions as f64 / 100.0) as usize)
            .sum();
        
        if total_matches > 0 {
            let overall_win_rate = (shredlink_wins as f64 / total_matches as f64) * 100.0;
            println!("🎯 Total Matched Transactions: {}", total_matches);
            println!("🏆 Shredlink Wins: {} out of {} ({:.1}%)", shredlink_wins, total_matches, overall_win_rate);
            
            
        } else {
            println!("{}", red.apply_to("❌ No matched transactions for comparison"));
        }
    }
}

// --- Helper structs and functions ---

struct CurrentStats {
    total: usize,
    matched: usize,
    rate: f64,
}

struct LatencyStats {
    average: f64,
    median: f64,
    p95: f64,
    p99: f64,
    min: i64,
    max: i64,
    shredlink_wins_percentage: f64,
}

impl LatencyStats {
    fn calculate(latencies: &[i64]) -> Self {
        if latencies.is_empty() {
            return Self {
                average: 0.0,
                median: 0.0,
                p95: 0.0,
                p99: 0.0,
                min: 0,
                max: 0,
                shredlink_wins_percentage: 0.0,
            };
        }
        
        let mut sorted = latencies.to_vec();
        sorted.sort_unstable();
        
        let len = sorted.len();
        let average = sorted.iter().sum::<i64>() as f64 / len as f64;
        let median = if len % 2 == 0 {
            (sorted[len / 2 - 1] + sorted[len / 2]) as f64 / 2.0
        } else {
            sorted[len / 2] as f64
        };
        
        let p95_idx = ((len as f64 * 0.95) as usize).min(len - 1);
        let p99_idx = ((len as f64 * 0.99) as usize).min(len - 1);
        
        let shredlink_wins = sorted.iter().filter(|&&x| x > 0).count();
        let shredlink_wins_percentage = (shredlink_wins as f64 / len as f64) * 100.0;
        
        Self {
            average,
            median,
            p95: sorted[p95_idx] as f64,
            p99: sorted[p99_idx] as f64,
            min: sorted[0],
            max: sorted[len - 1],
            shredlink_wins_percentage,
        }
    }
}

fn get_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}