//! Command-line interface for `shipit`, a small deploy orchestrator.
//!
//! The parser is split into a top-level `Cli` plus one struct per subcommand so
//! each subcommand's flags stay next to the code that consumes them.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "shipit", version, about = "Roll out builds to a fleet of hosts")]
pub struct Cli {
    /// Path to the fleet manifest.
    #[arg(long = "manifest", default_value = "fleet.toml")]
    pub manifest: PathBuf,

    /// Increase log verbosity (repeatable).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Emit machine-readable output.
    #[arg(long = "json")]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Push a build to every host in a group.
    Deploy(DeployArgs),
    /// Revert a group to its previous build.
    Rollback(RollbackArgs),
    /// Show what is running where.
    Status,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Strategy {
    Rolling,
    BlueGreen,
    Canary,
}

#[derive(Debug, Args)]
pub struct DeployArgs {
    /// Build artifact to deploy.
    pub artifact: PathBuf,

    /// Host group to target.
    #[arg(long = "group", default_value = "default")]
    pub group: String,

    /// Rollout strategy.
    #[arg(long = "strategy", value_enum, default_value_t = Strategy::Rolling)]
    pub strategy: Strategy,

    /// How many hosts to update at once.
    #[arg(long = "batch-size", default_value_t = 2)]
    pub batch_size: usize,

    /// How many times to retry a failing host before giving up.
    #[arg(long = "retries", default_value_t = 3)]
    pub retries: u32,

    /// Seconds to wait between retries; doubles after each attempt.
    #[arg(long = "retry-backoff", default_value_t = 5)]
    pub retry_backoff_secs: u64,

    /// Skip the post-deploy health check.
    #[arg(long = "no-health-check")]
    pub no_health_check: bool,
}

impl DeployArgs {
    pub fn backoff(&self, attempt: u32) -> Duration {
        let base = Duration::from_secs(self.retry_backoff_secs);
        base.saturating_mul(1u32 << attempt.min(6))
    }
}

#[derive(Debug, Args)]
pub struct RollbackArgs {
    /// Host group to revert.
    #[arg(long = "group", default_value = "default")]
    pub group: String,

    /// Roll back this many releases instead of one.
    #[arg(long = "steps", default_value_t = 1)]
    pub steps: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles() {
        let args = DeployArgs {
            artifact: "a.tar".into(),
            group: "g".into(),
            strategy: Strategy::Rolling,
            batch_size: 1,
            retries: 3,
            retry_backoff_secs: 2,
            no_health_check: false,
        };
        assert_eq!(args.backoff(0), Duration::from_secs(2));
        assert_eq!(args.backoff(2), Duration::from_secs(8));
    }
}
