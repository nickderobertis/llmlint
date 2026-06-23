use std::process::ExitCode;

use clap::Parser;

use llmlint::cli::Cli;
use llmlint::commands;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match commands::dispatch(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("llmlint: error: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}
