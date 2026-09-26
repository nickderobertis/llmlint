use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Output encodings the client can print.
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    Table,
    Json,
    Csv,
}

/// Command-line options for the prices client.
#[derive(Parser)]
#[command(name = "prices", version)]
pub struct Cli {
    /// Base URL of the price service.
    #[arg(long = "endpoint", env = "PRICES_ENDPOINT")]
    pub endpoint: String,
    /// Request timeout in seconds.
    #[arg(long = "timeout", default_value_t = 30)]
    pub timeout: u64,
    /// How many times a failed request is retried.
    #[arg(long = "retries", default_value_t = 3)]
    pub retries: u32,
    /// Maximum requests in flight at once.
    #[arg(long = "concurrency", default_value_t = 4)]
    pub concurrency: usize,
    /// Write results to this file instead of stdout.
    #[arg(long = "output", short = 'o')]
    pub output: Option<PathBuf>,
    /// Output encoding.
    #[arg(long = "format", value_enum, default_value_t = Format::Table)]
    pub format: Format,
    /// Log each request and response.
    #[arg(long = "verbose", short = 'v')]
    pub verbose: bool,
    /// Print nothing but errors.
    #[arg(long = "quiet", short = 'q', conflicts_with = "verbose")]
    pub quiet: bool,
    /// Never colorize output.
    #[arg(long = "no-color")]
    pub no_color: bool,
    /// Read defaults from this TOML file.
    #[arg(long = "config")]
    pub config: Option<PathBuf>,
    /// Price region to query.
    #[arg(long = "region", default_value = "us")]
    pub region: String,
    /// Print what would be sent without sending it.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}
