use anyhow::Result;
use clap::Parser;
use console::Style;
use dotenv::dotenv;
use std::env;
use std::time::Duration;

mod benchmark;
mod geyser_client;
mod shredlink_client;

use benchmark::Benchmark;

#[derive(Parser)]
#[command(name = "shredlink")]
#[command(about = "A high-performance benchmarking tool for comparing Geyser and Shredlink transaction streaming latency on Solana")]
#[command(version = "1.0")]
struct Cli {
    /// Benchmark duration in seconds
    #[arg(short, long, default_value_t = 60)]
    duration: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();
    
    // Load environment variables
    dotenv().ok();
    
    let cli = Cli::parse();
    
    let cyan = Style::new().cyan();
    let green = Style::new().green();
    let red = Style::new().red();
    
    println!("{}", cyan.apply_to("🚀 ShredLink - Solana Transaction Streaming Benchmark"));
    println!("{}", cyan.apply_to("=".repeat(60)));
    
    // Get configuration from environment
    let geyser_host = env::var("GEYSER_HOST_URL").map_err(|_| {
        anyhow::anyhow!("{}", red.apply_to("❌ GEYSER_HOST_URL environment variable not set"))
    })?;
    
    // let geyser_api_key = env::var("GEYSER_API_KEY").map_err(|_| {
    //     anyhow::anyhow!("{}", red.apply_to("❌ GEYSER_API_KEY environment variable not set"))
    // })?;
    
    let shredlink_host = env::var("SHREDLINK_HOST_URL").map_err(|_| {
        anyhow::anyhow!("{}", red.apply_to("❌ SHREDLINK_HOST_URL environment variable not set"))
    })?;
    
    let benchmark_time = Duration::from_secs(cli.duration);
    
    println!("{}", green.apply_to("📋 Configuration:"));
    println!("  🔗 Geyser Host: {}", geyser_host);
    println!("  🔗 Shredlink Host: {}", shredlink_host);
    println!("  ⏱️  Duration: {} seconds", cli.duration);
    println!();
    
    // Create and run benchmark
    let mut benchmark = Benchmark::new(geyser_host.clone(), shredlink_host.clone());
    
    println!("{}", green.apply_to("🏁 Starting benchmark..."));
    benchmark.run(benchmark_time).await?;
    
    // Print results
    println!("{}", green.apply_to("📊 Benchmark Results:"));
    benchmark.print_report(&"cli.output");
    
    println!("{}", cyan.apply_to("✨ Benchmark completed successfully!"));
    
    Ok(())
}
