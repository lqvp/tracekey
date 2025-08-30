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

## Usage

### Monitoring Mode

This is the default mode. The application will continuously monitor the specified URLs.

1. Clone the repository.
2. Configure your `config/default.toml` file (see `config/default.toml` for all options).
3. Build and run the application:

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

All settings are managed in the `config/default.toml` file. This includes:

- Misskey integration settings (API token, URL).
- Target URLs to monitor.
- Monitoring intervals and timeouts.
- Output format (`json`, `csv`, or `none`) and file path.
- **Reporting settings:** enable/disable, interval, output targets, and console color thresholds.
