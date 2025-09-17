use chrono::{DateTime, Utc};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long)]
    pub report: bool,
    #[arg(long)]
    #[arg(long, value_parser = clap::value_parser!(DateTime<Utc>))]
    pub since: Option<DateTime<Utc>>,
    #[arg(long, value_parser = clap::value_parser!(DateTime<Utc>))]
    pub until: Option<DateTime<Utc>>,
    #[arg(long)]
    pub dry_run: bool,
}
