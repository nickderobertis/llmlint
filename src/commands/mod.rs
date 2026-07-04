//! Command dispatch: wire the CLI to domain + io. Each command returns the
//! process exit code (a completed-but-failing lint is `1`, not an error).

pub mod check_ignores;
pub mod config;
pub mod doctor;
pub mod history;
pub mod ignores;
pub mod init;
pub mod lint;
pub mod lint_config;
pub mod progress;
pub mod where_;

use crate::cli::{Cli, Command};
use crate::errors::Result;

pub fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Some(Command::Lint(args)) => lint::run(args),
        Some(Command::LintConfig(args)) => lint_config::run(args),
        Some(Command::CheckIgnores(args)) => check_ignores::run(args),
        Some(Command::Init(args)) => init::run(args),
        Some(Command::Config(args)) => config::run(args),
        Some(Command::Where(args)) => where_::run(args),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::History(args)) => history::run(args),
        None => lint::run(cli.lint),
    }
}
