use anyhow::Result;
use chrono::{DateTime, Utc};
use console::Style;
use dashmap::DashMap;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::geyser_client::{GeyserStreamClient, GeyserTransaction};
use crate::shredlink_client::{ShredlinkClient, ShredlinkTransaction};

/// Stores timestamps for a transaction from both sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionTimestamp {
    pub geyser_timestamp: Option<u64>,
    pub shredlink_timestamp: Option<u64>,
}

impl TransactionTimestamp {
    /// Calculate latency difference (positive means Geyser is faster)
    pub fn latency_diff_ms(&self) -> Option<i64> {
        match (self.geyser_timestamp, self.shredlink_timestamp) {
            (Some(geyser), Some(shredlink)) => Some(geyser as i64 - shredlink as i64),
            _ => None,
        }
    }
    
    pub fn is_complete(&self) -> bool {
        self.geyser_timestamp.is_some() && self.shredlink_timestamp.is_some()
    }
}

/// Benchmark results summary
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_seconds: f64,
    pub total_transactions: usize,
    pub matched_transactions: usize,
    pub geyser_only_count: usize,
    pub shredlink_only_count: usize,
    pub average_latency_ms: f64,
    pub median_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub min_latency_ms: i64,
    pub max_latency_ms: i64,
    pub shredlink_wins_percentage: f64,
}

pub struct Benchmark {
    geyser_url: String,
    shredlink_url: String,
    transactions: Arc<DashMap<String, TransactionTimestamp>>,
    start_time: Instant,
}

impl Benchmark {
    pub fn new(geyser_url: String, shredlink_url: String) -> Self {
        Self {
            geyser_url,
            shredlink_url,
            transactions: Arc::new(DashMap::new()),
            start_time: Instant::now(),
        }
    }
    
    pub async fn run(&mut self, duration: Duration) -> Result<()> {
        self.start_time = Instant::now();
        
        // Setup channels for transaction streaming
        let (geyser_tx, geyser_rx) = mpsc::unbounded_channel();
        let (shredlink_tx, shredlink_rx) = mpsc::unbounded_channel();
        
        // Setup progress tracking
        let progress = self.create_progress_bar(duration);
        
        // Start transaction handlers
        let _geyser_handler = self.start_geyser_handler(geyser_rx).await;
        let _shredlink_handler = self.start_shredlink_handler(shredlink_rx).await;
        
        // Start clients
        println!("🔄 Starting clients...");
        
        // Create clients locally - much cleaner!
        let mut geyser_client = GeyserStreamClient::new(self.geyser_url.clone(), None);
        let mut shredlink_client = ShredlinkClient::new(self.shredlink_url.clone());
        
        // Race clients against timer
        tokio::select! {
            _ = geyser_client.start(geyser_tx) => {},
            _ = shredlink_client.start(shredlink_tx) => {},
            _ = self.run_with_progress(duration, &progress) => {},
        }
        
        progress.finish_with_message("✅ Benchmark completed");
        Ok(())
    }
    
    /// Generate final benchmark report
    pub fn generate_report(&self) -> BenchmarkReport {
        let end_time = Utc::now();
        let start_time = end_time - chrono::Duration::from_std(self.start_time.elapsed()).unwrap();
        
        let latencies: Vec<i64> = self.transactions
            .iter()
            .filter_map(|entry| entry.latency_diff_ms())
            .collect();
        
        let matched_count = latencies.len();
        let total_count = self.transactions.len();
        
        let stats = LatencyStats::calculate(&latencies);
        
        BenchmarkReport {
            start_time,
            end_time,
            duration_seconds: self.start_time.elapsed().as_secs_f64(),
            total_transactions: total_count,
            matched_transactions: matched_count,
            geyser_only_count: self.count_geyser_only(),
            shredlink_only_count: self.count_shredlink_only(),
            average_latency_ms: stats.average,
            median_latency_ms: stats.median,
            p95_latency_ms: stats.p95,
            p99_latency_ms: stats.p99,
            min_latency_ms: stats.min,
            max_latency_ms: stats.max,
            shredlink_wins_percentage: stats.shredlink_wins_percentage,
        }
    }
    
    pub fn print_report(&self, _format: &str) {
        let report = self.generate_report();
        self.print_table_report(&report)
        // match format {
        //     "json" => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        //     _ => self.print_table_report(&report),
        // }
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
    
    async fn start_geyser_handler(&self, mut rx: mpsc::UnboundedReceiver<GeyserTransaction>) -> tokio::task::JoinHandle<()> {
        let transactions = Arc::clone(&self.transactions);
        
        tokio::spawn(async move {
            while let Some(transaction) = rx.recv().await {
                let timestamp = get_timestamp_ms();
                let signature = transaction.signature;
                
                println!("🟦 Geyser: {}", signature);
                
                transactions
                    .entry(signature.clone())
                    .and_modify(|entry| {
                        if entry.geyser_timestamp.is_none() {
                            entry.geyser_timestamp = Some(timestamp);
                            if let Some(diff) = entry.latency_diff_ms() {
                                println!("⏱️  Latency: {}ms ({})", diff, signature);
                            }
                        }
                    })
                    .or_insert(TransactionTimestamp {
                        geyser_timestamp: Some(timestamp),
                        shredlink_timestamp: None,
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
                            if let Some(diff) = entry.latency_diff_ms() {
                                println!("⏱️  Latency: {}ms ({})", diff, signature);
                            }
                        }
                    })
                    .or_insert(TransactionTimestamp {
                        geyser_timestamp: None,
                        shredlink_timestamp: Some(timestamp),
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
        let matched = self.transactions.iter().filter(|entry| entry.is_complete()).count();
        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        let rate = if elapsed_secs > 0.0 { total as f64 / elapsed_secs } else { 0.0 };
        
        CurrentStats { total, matched, rate }
    }
    
    fn count_geyser_only(&self) -> usize {
        self.transactions
            .iter()
            .filter(|entry| entry.geyser_timestamp.is_some() && entry.shredlink_timestamp.is_none())
            .count()
    }
    
    fn count_shredlink_only(&self) -> usize {
        self.transactions
            .iter()
            .filter(|entry| entry.shredlink_timestamp.is_some() && entry.geyser_timestamp.is_none())
            .count()
    }
    
    fn print_table_report(&self, report: &BenchmarkReport) {
        let green = Style::new().green();
        let cyan = Style::new().cyan();
        let red = Style::new().red();
        
        println!();
        println!("{}", green.apply_to("📊 BENCHMARK RESULTS"));
        println!("{}", green.apply_to("=".repeat(50)));
        
        // Basic stats
        println!("⏱️  Duration: {:.1}s", report.duration_seconds);
        println!("📈 Total Transactions: {}", report.total_transactions);
        println!("🎯 Matched Transactions: {}", report.matched_transactions);
        println!("📊 Geyser Only: {}", report.geyser_only_count);
        println!("📊 Shredlink Only: {}", report.shredlink_only_count);
        println!();
        
        if report.matched_transactions > 0 {
            println!("{}", cyan.apply_to("⚡ LATENCY ANALYSIS"));
            println!("{}", cyan.apply_to("-".repeat(30)));
            
            let winner = if report.average_latency_ms < 0.0 { "Shredlink" } else { "Geyser" };
            let avg_diff = report.average_latency_ms.abs();
            
            println!("🏆 Winner: {} (avg {:.1}ms faster)", winner, avg_diff);
            println!("📊 Average: {:.1}ms", report.average_latency_ms);
            println!("📊 Median: {:.1}ms", report.median_latency_ms);
            println!("📊 95th percentile: {:.1}ms", report.p95_latency_ms);
            println!("📊 99th percentile: {:.1}ms", report.p99_latency_ms);
            println!("📊 Range: {}ms to {}ms", report.min_latency_ms, report.max_latency_ms);
            println!("🎯 Shredlink wins: {:.1}%", report.shredlink_wins_percentage);
        } else {
            println!("{}", red.apply_to("❌ No matched transactions found"));
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
        
        let shredlink_wins = sorted.iter().filter(|&&x| x < 0).count();
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