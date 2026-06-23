//! Command-line interface (clap). `lint` is the default when no subcommand is
//! given, so `llmlint [FILES...]` works like any other linter.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "llmlint",
    version,
    about = "LLM-as-judge linter for checks deterministic linters can't express.",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Default (no subcommand): run the lint.
    #[command(flatten)]
    pub lint: LintArgs,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the LLM-as-judge lint (this is the default).
    Lint(LintArgs),
    /// Write a starter llmlint config file.
    Init(InitArgs),
    /// Print the effective merged config and its sources as JSON.
    Config(ConfigArgs),
    /// Check that oneharness is installed and reachable.
    Doctor,
}

#[derive(Args, Debug, Default)]
pub struct LintArgs {
    /// Files to lint. When given, overrides the config's file globs (per-rule
    /// and per-agent `files` still take precedence).
    pub files: Vec<PathBuf>,

    /// llmlint config file(s); repeatable. Replaces upward config discovery.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    pub config: Vec<PathBuf>,

    /// oneharness config file to forward via `--config` (single-file; extras warn).
    #[arg(long = "oneharness-config", value_name = "PATH")]
    pub oneharness_config: Vec<PathBuf>,

    /// Override the oneharness binary (else `$LLMLINT_ONEHARNESS_BIN` or PATH).
    #[arg(long = "oneharness-bin", value_name = "PATH")]
    pub oneharness_bin: Option<String>,

    /// Only run rules assigned to this agent (`default` for unassigned rules).
    #[arg(long = "agent", value_name = "NAME")]
    pub agent: Option<String>,

    /// Only run these named rules; repeatable.
    #[arg(long = "rule", value_name = "NAME")]
    pub rule: Vec<String>,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    /// Maximum judges to run in parallel.
    #[arg(long = "max-parallel", value_name = "N")]
    pub max_parallel: Option<usize>,

    /// Per-judge timeout in seconds (default 120).
    #[arg(long = "timeout", value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Directory to lint from (config discovery + the harness cwd). Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,
}

#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Write to the user-global config dir instead of `./llmlint.yml`.
    #[arg(long)]
    pub global: bool,

    /// Embed the default prompt template in the config for customization.
    #[arg(long = "with-template")]
    pub with_template: bool,

    /// Overwrite an existing config instead of refusing.
    #[arg(long)]
    pub force: bool,

    /// Write to this path instead of the default.
    #[arg(long = "output", short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug, Default)]
pub struct ConfigArgs {
    /// llmlint config file(s); repeatable. Replaces upward discovery.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    pub config: Vec<PathBuf>,

    /// Directory to resolve config discovery from. Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,
}
