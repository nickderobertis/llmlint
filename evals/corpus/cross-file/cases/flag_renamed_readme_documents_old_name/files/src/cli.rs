use clap::Parser;

/// Command-line options for the prices client.
#[derive(Parser)]
pub struct Cli {
    /// Base URL of the price service.
    #[arg(long = "service-url")]
    pub endpoint: String,
    /// Request timeout in seconds.
    #[arg(long = "timeout", default_value_t = 30)]
    pub timeout: u64,
    /// Print what would be sent without sending it.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}
