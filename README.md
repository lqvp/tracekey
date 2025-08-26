# tracekey

`tracekey` is a simple monitoring tool written in Rust that periodically checks the Cloudflare colocation (`colo`) for a list of specified URLs. If a change in the colocation is detected, it sends a notification to a configured Misskey instance.

## Features

- Monitors multiple URLs for Cloudflare colocation changes.
- Sends notifications to Misskey when a change is detected.
- Configurable check interval, user-agent, and request timeouts.

## Usage

1. Clone the repository.
2. Create and configure your `config/default.toml` file as described above.
3. Build and run the application using Cargo:

    ```sh
    cargo run --release
    ```

The application will start monitoring the specified URLs and will print logs to the console.
