//! Command dispatch: wire the CLI to domain + io. Each command returns the
//! process exit code (a completed-but-failing lint is `1`, not an error).

pub mod config;
pub mod doctor;
pub mod init;
pub mod lint;

use crate::cli::{Cli, Command};
use crate::errors::Result;

pub fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Some(Command::Lint(args)) => lint::run(args),
        Some(Command::Init(args)) => init::run(args),
        Some(Command::Config(args)) => config::run(args),
        Some(Command::Doctor) => doctor::run(),
        None => lint::run(cli.lint),
    }
}
