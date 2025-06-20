# ShredLink Benchmarking Tool

A Rust tool that benchmarks transaction streaming latency between Geyser and Shredlink services on Solana.

## Setup

1. **Install Rust** (1.70+)
2. **Clone and build**:
   ```bash
   git clone <repo-url>
   cd shredlink-benchmarking
   cargo build --release
   ```

3. **Configure environment** (`.env` file):
   ```
   GEYSER_HOST_URL=grpc://your-geyser-host:443
   SHREDLINK_HOST_URL=grpc://your-shredlink-host:443
   ```

## Usage

```bash
# Run 60-second benchmark (default)
cargo run --release

# Custom duration
cargo run --release -- --duration 120
```

## Output

The tool shows:
- Total transactions received from each service
- Matched transactions (received by both)
- Latency statistics (average, median, percentiles)
- Which service is faster and by how much

## How it works

1. Connects to both Geyser and Shredlink services
2. Subscribes to PumpFun transactions (`6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`)
3. Records timestamps when transactions arrive from each service
4. Calculates latency differences for matched transactions
5. Provides statistical analysis of the results

## Links

- **Shredlink**: https://www.shredlink.xyz/
- **Discord**: https://discord.com/invite/sskBrcfX
- **Twitter**: https://x.com/shredslink
