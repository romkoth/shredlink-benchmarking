use anyhow::Result;
use clap::Parser;
use console::Style;
use dotenv::dotenv;
use std::collections::HashMap;
use std::env;
use std::time::Duration;

mod benchmark;
mod geyser_client;
mod shredlink_client;

use benchmark::Benchmark;

#[derive(Parser)]
#[command(name = "shredlink")]
#[command(about = "A high-performance benchmarking tool for comparing multiple Geyser sources and Shredlink transaction streaming latency on Solana")]
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
    
    println!("{}", cyan.apply_to("🚀 ShredLink - Multi-Geyser Benchmark"));
    println!("{}", cyan.apply_to("=".repeat(45)));
    
    // Get Geyser configuration from environment
    let mut geyser_urls = HashMap::new();
    let mut geyser_tokens = HashMap::new();
    
    for (key, value) in env::vars() {
        if key.starts_with("GEYSER_") && key.ends_with("_URL") && key != "GEYSER_HOST_URL" {
            if let Some(name_part) = key.strip_prefix("GEYSER_").and_then(|s| s.strip_suffix("_URL")) {
                let geyser_name = format!("Geyser-{}", name_part.to_lowercase());
                geyser_urls.insert(geyser_name.clone(), value);
                
                // Check for corresponding token
                let token_key = format!("GEYSER_{}_TOKEN", name_part);
                if let Ok(token) = env::var(&token_key) {
                    geyser_tokens.insert(geyser_name, token);
                }
            }
        }
    }
    
    if geyser_urls.is_empty() {
        return Err(anyhow::anyhow!("{}", red.apply_to("❌ No Geyser URLs found. Set GEYSER_<NAME>_URL environment variables")));
    }
    
    let shredlink_host = env::var("SHREDLINK_HOST_URL").map_err(|_| {
        anyhow::anyhow!("{}", red.apply_to("❌ SHREDLINK_HOST_URL environment variable not set"))
    })?;
    
    let benchmark_time = Duration::from_secs(cli.duration);
    
    println!("{}", green.apply_to("📋 Configuration:"));
    for (name, url) in &geyser_urls {
        let auth_status = if geyser_tokens.contains_key(name) { "🔐" } else { "🔓" };
        println!("  {} {}: {}", auth_status, name, url);
    }
    println!("  🔗 Shredlink: {}", shredlink_host);
    println!("  ⏱️  Duration: {}s | Sources: {}", cli.duration, geyser_urls.len());
    println!();
    
    // Create and run benchmark
    let mut benchmark = Benchmark::new(geyser_urls, geyser_tokens, shredlink_host);
    
    println!("{}", green.apply_to("🏁 Starting benchmark..."));
    benchmark.run(benchmark_time).await?;
    
    // Print results
    println!();
    benchmark.print_report(&"cli.output");
    
    println!("{}", cyan.apply_to("✨ Benchmark completed!"));
    
    Ok(())
}
