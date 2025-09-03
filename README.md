# tracekey

`tracekey` is a monitoring and reporting tool written in Rust. It periodically checks Cloudflare colocation (`colo`) and Round Trip Time (RTT) for a list of URLs. It can log results to JSON or CSV, send notifications to Misskey on colo changes, and generate periodic statistical reports.

## Features

- **Monitoring:**
  - Monitors multiple URLs for Cloudflare colocation changes and RTT.
  - Logs check results to either JSON or CSV files.
  - Sends notifications to Misskey when a colocation change is detected.
  - Configurable check interval, user-agent, and request timeouts.
- **Reporting:**
  - Generates detailed statistical reports from historical monitoring data.
  - Calculates uptime, RTT stats (min, max, mean, median, p95), and colocation changes.
  - Posts beautifully formatted reports to Misskey using MFM.
  - Displays color-coded reports in the console.
  - Can be triggered periodically or on-demand via CLI.

## Installation

1. Ensure Rust is installed. If not, install it using [rustup](https://rustup.rs/).
2. Clone the repository:

   ```sh
   git clone <repository-url>
   cd tracekey
   ```

3. Build the dependencies:

   ```sh
   cargo build --release
   ```

## Usage

### Monitoring Mode (Default)

This mode continuously monitors the specified URLs.

1. Configure your `config/default.toml` file (see Configuration section for details).
2. Build and run the application:

   ```sh
   cargo run --release
   ```

### Reporting Mode

This mode generates a one-time report from the existing log file and then exits.

```sh
cargo run --release -- --report
```

#### CLI Options for Reporting

- `--since <RFC3339>`: Sets the start time for the report period.
- `--until <RFC3339>`: Sets the end time for the report period.
- `--dry-run`: Prints the Misskey report content to the console instead of posting it.

## Configuration

All settings are managed in the `config/default.toml` file. Here are the main configuration options:

### Misskey Integration (Optional)

- `misskey_url`: URL of the Misskey instance (e.g., "<https://misskey.io>")
- `misskey_token`: Misskey API token (leave empty to disable)
- `misskey_visibility`: Post visibility ("public", "home", "followers")

### Targets

- `target_urls`: List of URLs to monitor (e.g., ["https://misskey.io", "https://misskey.vip"])

### Monitoring Settings

- `check_interval_seconds`: Check interval in seconds
- `user_agent`: User-agent for requests
- `request_timeout_seconds`: Request timeout in seconds

### Output Settings

- `output_format`: Output format ("json", "csv", "none")
- `output_path`: Path to the output file

### Reporting Settings

- `reporting.enabled`: Enable reporting functionality
- `reporting.interval`: Reporting interval (e.g., "24h")
- `reporting.output_to_console`: Output to console
- `reporting.output_to_misskey`: Post to Misskey
- `reporting.rtt_threshold_ms`: RTT threshold for console highlighting
- `reporting.uptime_threshold_percent`: Uptime threshold for console highlighting

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contributing

Please report bugs or request features via GitHub Issues. Pull requests are welcome!
