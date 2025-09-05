# tracekey

`tracekey` is a monitoring and reporting tool written in Rust that tracks Cloudflare's colocation (`colo`) and Round Trip Time (RTT). It periodically checks a list of specified URLs, saves the results in JSONL format, notifies Misskey of `colo` changes, and generates statistical reports.

## Key Features

- **Monitoring:**
  - Monitors Cloudflare `colo` and RTT for multiple URLs.
  - Records check results to a JSONL file.
  - Sends notifications to Misskey upon detecting a `colo` change.
- **Reporting:**
  - Generates statistical reports (uptime, RTT stats, `colo` transitions, etc.) from historical data.
  - Outputs reports to the console and Misskey (using MFM).
  - Can be run on-demand via CLI or periodically based on configuration.

## Usage

### Configuration

Edit `config/default.toml` to configure target URLs and Misskey integration settings.

```toml
# Misskey integration (disabled if token is empty)
misskey_url = "https://misskey.io"
misskey_token = ""

# Target URLs to monitor
target_urls = ["https://misskey.io", "https://example.com"]

# Check interval in seconds
check_interval_seconds = 300

# Output settings ("jsonl" or "none")
output_format = "jsonl"
output_path = "trace_log.jsonl"

# Reporting settings
[reporting]
enabled = true
interval = "24h" # Interval for periodic reports
output_to_console = true
output_to_misskey = true
misskey_visibility = "home"
```

### Monitoring Mode

Continuously runs checks based on the configuration file.

```sh
cargo run --release
```

### Reporting Mode

Generates a one-time report from the recorded data and exits.

```sh
cargo run --release -- --report
```

**Reporting Mode Options:**

- `--since <RFC3339>`: Sets the start time for the report period.
- `--until <RFC3339>`: Sets the end time for the report period.
- `--dry-run`: Prints the report content to the console instead of posting to Misskey.

## License

[MIT License](LICENSE)
