use chrono::{DateTime, Utc};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub(crate) struct Cli {
    #[arg(long)]
    pub(crate) report: bool,
    #[arg(long)]
    #[arg(long, value_parser = clap::value_parser!(DateTime<Utc>))]
    pub(crate) since: Option<DateTime<Utc>>,
    #[arg(long, value_parser = clap::value_parser!(DateTime<Utc>))]
    pub(crate) until: Option<DateTime<Utc>>,
    #[arg(long)]
    pub(crate) dry_run: bool,
}
