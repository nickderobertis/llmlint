//! Command-line interface (clap). `lint` is the default when no subcommand is
//! given, so `llmlint [FILES...]` works like any other linter.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::io::diff::DiffBackend;

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
// `LintArgs` is the largest variant by design (it carries every lint flag) and
// `Cli` flattens the same struct for the default path, so boxing it here would
// just add indirection to a value parsed once at startup — and clap's derive
// doesn't flatten through a `Box`. The size gap is harmless for a short-lived
// CLI enum.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Run the LLM-as-judge lint (this is the default).
    Lint(LintArgs),
    /// Lint llmlint config files with the bundled config-lint rules. This is the
    /// `lint` command with the bundled config-lint plugin included by default, so
    /// you don't have to add it to your own config: it first runs the
    /// deterministic ignore-directive (comment) check, then judges each config's
    /// rules for clear, unambiguous names and descriptions. Targets the discovered
    /// llmlint config files unless FILES are given.
    #[command(name = "lint-config")]
    LintConfig(LintConfigArgs),
    /// Validate inline `llmlint: ignore` directives (deterministic, no model
    /// call). Runs as part of `lint`; split out so it can sit in the fast
    /// static-check loop next to fmt/clippy.
    #[command(name = "check-ignores")]
    CheckIgnores(CheckIgnoresArgs),
    /// Write a starter llmlint config file.
    Init(InitArgs),
    /// Print the effective merged config as JSON (add `--sources` to trace where
    /// each rule, agent, and setting is defined).
    Config(ConfigArgs),
    /// Show which file (or plugin URL) a config item comes from — the place to
    /// edit it. Pass a path like `oneharness.model`, `agents.<name>`,
    /// `rules.<name>`, or `rules.<name>.<field>`; prints the source and nothing
    /// else, for scripting. The broad view is `config --sources`.
    Where(WhereArgs),
    /// Check that oneharness is installed and reachable.
    Doctor,
}

#[derive(Args, Debug, Default)]
pub struct LintArgs {
    /// Files to lint. When given, they are **intersected** with the config's file
    /// globs (per-rule and config-level `files`), not an override: each rule lints
    /// only the passed files its globs match, so a scoped run never pulls in files
    /// the config never declared. A passed file no rule's globs match is skipped.
    pub files: Vec<PathBuf>,

    /// llmlint config file(s); repeatable. Replaces nested upward discovery.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    pub config: Vec<PathBuf>,

    /// oneharness config file to forward via `--config` (single-file; extras warn).
    #[arg(long = "oneharness-config", value_name = "PATH")]
    pub oneharness_config: Vec<PathBuf>,

    /// Override the oneharness binary (else `$LLMLINT_ONEHARNESS_BIN`, PATH, or
    /// a `oneharness` beside the llmlint executable).
    #[arg(long = "oneharness-bin", value_name = "PATH")]
    pub oneharness_bin: Option<String>,

    /// Override the master prompt template with this file's contents (wins over
    /// the config's `prompt_template`).
    #[arg(long = "prompt-template", value_name = "PATH")]
    pub prompt_template: Option<PathBuf>,

    /// Default judge model, forwarded to oneharness (overrides config
    /// `oneharness.model`; a per-agent `model` still wins for that agent).
    #[arg(long = "model", value_name = "NAME")]
    pub model: Option<String>,

    /// Schema-validation re-prompt budget (oneharness `--schema-max-retries`;
    /// overrides config `oneharness.schema_max_retries`).
    #[arg(long = "schema-max-retries", value_name = "N")]
    pub schema_max_retries: Option<u32>,

    /// Require a `rationale` for every rule's verdict (the default). Overrides
    /// the config's `rationales`; a per-rule `rationale` still wins. Use
    /// `--no-rationales` to turn rationales off.
    #[arg(long = "rationales", overrides_with = "no_rationales", action = clap::ArgAction::SetTrue)]
    pub rationales: bool,

    /// Disable rationales for this run (overrides config; a per-rule `rationale`
    /// still wins). The inverse of `--rationales`.
    #[arg(long = "no-rationales", overrides_with = "rationales", action = clap::ArgAction::SetTrue)]
    pub no_rationales: bool,

    /// Only run rules assigned to this agent (`default` for unassigned rules).
    #[arg(long = "agent", value_name = "NAME")]
    pub agent: Option<String>,

    /// Only run these named rules; repeatable.
    #[arg(long = "rule", value_name = "NAME")]
    pub rule: Vec<String>,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    /// When to colorize the human report: `auto` (default) colors only when
    /// stdout is a terminal and `NO_COLOR` is unset, `always` forces color
    /// (e.g. through a pager or for a screenshot), `never` disables it. Has no
    /// effect on `--format json`.
    #[arg(long = "color", value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    /// When to draw the live progress view (rules resolving as judges return) on
    /// stderr: `auto` (default) shows it only for an interactive human — a
    /// terminal, not CI, not an AI agent; `always` forces the decision on (a pipe
    /// is still never animated); `never` disables it. The report on stdout is
    /// unaffected, as is `--format json`.
    #[arg(long = "progress", value_enum, default_value_t = ProgressChoice::Auto)]
    pub progress: ProgressChoice,

    /// Increase output detail. By default, failing rules (with their locations)
    /// and the summary line are shown. `-v` additionally itemizes every passed
    /// and skipped rule, and prints the oneharness debug view (exact command +
    /// result) to stderr. Ignored for `--format json`.
    #[arg(long = "verbose", short = 'v', action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Maximum judges to run in parallel.
    #[arg(long = "max-parallel", value_name = "N")]
    pub max_parallel: Option<usize>,

    /// Per-judge timeout in seconds (default 120).
    #[arg(long = "timeout", value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Directory to lint from (config discovery + the harness cwd). Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Review only what changed: narrow the target files to the ones the diff
    /// reports as changed (intersected with the config globs) and add each one's
    /// diff to the judge prompt so it focuses on the changed lines. Files not in
    /// the diff are not reviewed. Bare `--diff` uses the `git` backend (compared
    /// against `HEAD`); pass a backend (`--diff git`) to choose one explicitly.
    /// Omitted: every config-matched file is reviewed. (When explicit files are
    /// also passed, those are the scope and `--diff` only adds changed-line
    /// context.)
    #[arg(
        long = "diff",
        value_name = "BACKEND",
        num_args = 0..=1,
        default_missing_value = "git",
    )]
    pub diff: Option<DiffBackend>,

    /// Base the `--diff` git backend compares against, instead of the default
    /// `HEAD`. Accepts any git revision — a branch, tag, commit, or an
    /// `A..B`/`A...B` range — so `--diff-base main` reviews exactly what the
    /// current branch changed versus `main` (and scopes the run to those files).
    /// Overrides the config `diff_base`. Requires `--diff`.
    #[arg(long = "diff-base", value_name = "REF", requires = "diff")]
    pub diff_base: Option<String>,
}

impl LintArgs {
    /// The rationale choice from the CLI, or `None` when neither
    /// `--rationales`/`--no-rationales` was given (so the config decides). The
    /// two flags `overrides_with` each other, so the last one on the command
    /// line wins and at most one bool is set.
    pub fn rationales(&self) -> Option<bool> {
        if self.no_rationales {
            Some(false)
        } else if self.rationales {
            Some(true)
        } else {
            None
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

/// When to apply ANSI color to the human report.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorChoice {
    /// Color only when stdout is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    /// Always emit color, even when stdout is not a terminal.
    Always,
    /// Never emit color.
    Never,
}

impl ColorChoice {
    /// Resolve to a concrete on/off decision. `Auto` colors only an interactive
    /// terminal with `NO_COLOR` unset that is **not** an AI coding agent — a
    /// PTY-allocating agent has `is_tty == true` but captured ANSI is unreliable
    /// (Claude Code strips/mangles it and ignores `NO_COLOR`), so plain text is the
    /// safe path. `Always` still forces color (an explicit override wins over
    /// detection). The signals are injected so the resolution stays pure/testable.
    pub fn resolve(self, is_tty: bool, no_color: bool, is_agent: bool) -> bool {
        match self {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => is_tty && !no_color && !is_agent,
        }
    }
}

/// When to draw the ephemeral live-progress view (rules resolving as judges
/// return). It renders to **stderr**, so stdout stays the clean report/JSON
/// channel; see `docs/design/interactive-progress.md`.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgressChoice {
    /// Show it only for an interactive human: stderr is a terminal, not CI, not an
    /// AI agent, and color is not suppressed.
    #[default]
    Auto,
    /// Force the decision on. indicatif still refuses to animate a non-terminal, so
    /// this never spams a pipe — it only overrides the CI/agent auto-suppression.
    Always,
    /// Never draw the live view.
    Never,
}

impl ProgressChoice {
    /// Resolve to a concrete show/hide decision. `Auto` requires an interactive,
    /// non-CI, non-agent terminal with color not suppressed. `Always` only needs a
    /// terminal (indicatif hides on a non-terminal regardless). Signals are
    /// injected so this stays pure and table-testable.
    pub fn resolve(self, stderr_tty: bool, is_ci: bool, is_agent: bool, color_ok: bool) -> bool {
        match self {
            ProgressChoice::Never => false,
            ProgressChoice::Always => stderr_tty,
            ProgressChoice::Auto => stderr_tty && !is_ci && !is_agent && color_ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_always_and_never_ignore_tty_no_color_and_agent() {
        for &tty in &[true, false] {
            for &no_color in &[true, false] {
                for &agent in &[true, false] {
                    assert!(ColorChoice::Always.resolve(tty, no_color, agent));
                    assert!(!ColorChoice::Never.resolve(tty, no_color, agent));
                }
            }
        }
    }

    #[test]
    fn color_auto_needs_a_tty_unset_no_color_and_no_agent() {
        assert!(ColorChoice::Auto.resolve(true, false, false));
        // A terminal but NO_COLOR set: off (the convention wins).
        assert!(!ColorChoice::Auto.resolve(true, true, false));
        // A terminal (e.g. a PTY-allocating agent) but agent-detected: off, since
        // captured ANSI is unreliable.
        assert!(!ColorChoice::Auto.resolve(true, false, true));
        // Not a terminal (piped/redirected): off regardless of the other signals.
        assert!(!ColorChoice::Auto.resolve(false, false, false));
        assert!(!ColorChoice::Auto.resolve(false, true, false));
    }

    #[test]
    fn progress_never_is_always_off_always_needs_only_a_tty() {
        for &ci in &[true, false] {
            for &agent in &[true, false] {
                for &ok in &[true, false] {
                    assert!(!ProgressChoice::Never.resolve(true, ci, agent, ok));
                    // Always ignores CI/agent/color but still needs a terminal
                    // (indicatif won't animate a non-terminal anyway).
                    assert!(ProgressChoice::Always.resolve(true, ci, agent, ok));
                    assert!(!ProgressChoice::Always.resolve(false, ci, agent, ok));
                }
            }
        }
    }

    #[test]
    fn progress_auto_requires_an_interactive_human() {
        // Interactive terminal, not CI, not an agent, color available: on.
        assert!(ProgressChoice::Auto.resolve(true, false, false, true));
        // Any one disqualifier turns it off.
        assert!(!ProgressChoice::Auto.resolve(false, false, false, true)); // not a tty
        assert!(!ProgressChoice::Auto.resolve(true, true, false, true)); // CI
        assert!(!ProgressChoice::Auto.resolve(true, false, true, true)); // agent
        assert!(!ProgressChoice::Auto.resolve(true, false, false, false)); // NO_COLOR/dumb
    }
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
pub struct CheckIgnoresArgs {
    /// Files to scan. When given, they are **intersected** with the config's file
    /// globs (each rule scans only the passed files its globs match), not an
    /// override — pass the changed files to scope the check in a pre-commit hook.
    pub files: Vec<PathBuf>,

    /// llmlint config file(s); repeatable. Replaces upward config discovery.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    pub config: Vec<PathBuf>,

    /// Directory to scan from (config discovery + glob root). Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,
}

#[derive(Args, Debug, Default)]
pub struct LintConfigArgs {
    /// Config files to lint. When given, they are **intersected** with the bundled
    /// config-lint globs (which otherwise discover every llmlint config in the
    /// tree), so only the passed config files that those globs match are linted.
    pub files: Vec<PathBuf>,

    /// oneharness config file to forward via `--config` (single-file; extras warn).
    #[arg(long = "oneharness-config", value_name = "PATH")]
    pub oneharness_config: Vec<PathBuf>,

    /// Override the oneharness binary (else `$LLMLINT_ONEHARNESS_BIN`, PATH, or
    /// a `oneharness` beside the llmlint executable).
    #[arg(long = "oneharness-bin", value_name = "PATH")]
    pub oneharness_bin: Option<String>,

    /// Default judge model, forwarded to oneharness.
    #[arg(long = "model", value_name = "NAME")]
    pub model: Option<String>,

    /// Schema-validation re-prompt budget (oneharness `--schema-max-retries`).
    #[arg(long = "schema-max-retries", value_name = "N")]
    pub schema_max_retries: Option<u32>,

    /// Require a `rationale` for every rule's verdict (the default). Use
    /// `--no-rationales` to turn rationales off.
    #[arg(long = "rationales", overrides_with = "no_rationales", action = clap::ArgAction::SetTrue)]
    pub rationales: bool,

    /// Disable rationales for this run. The inverse of `--rationales`.
    #[arg(long = "no-rationales", overrides_with = "rationales", action = clap::ArgAction::SetTrue)]
    pub no_rationales: bool,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    /// When to colorize the human report (`auto`/`always`/`never`).
    #[arg(long = "color", value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    /// Increase output detail (repeatable). `-v` itemizes passed/skipped rules and
    /// prints the oneharness debug view to stderr.
    #[arg(long = "verbose", short = 'v', action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Maximum judges to run in parallel.
    #[arg(long = "max-parallel", value_name = "N")]
    pub max_parallel: Option<usize>,

    /// Per-judge timeout in seconds (default 120).
    #[arg(long = "timeout", value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Directory to lint from (config discovery + the harness cwd). Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Review only changed configs: narrow the target config files to the ones the
    /// diff reports as changed (intersected with the config-lint globs) and add
    /// each one's diff to the judge prompt. Bare `--diff` uses the `git` backend
    /// (compared against `HEAD`); pass a backend (`--diff git`) to choose one
    /// explicitly.
    #[arg(
        long = "diff",
        value_name = "BACKEND",
        num_args = 0..=1,
        default_missing_value = "git",
    )]
    pub diff: Option<DiffBackend>,

    /// Base the `--diff` git backend compares against, instead of `HEAD`. Any git
    /// revision — a branch, tag, commit, or `A..B`/`A...B` range. Requires `--diff`.
    #[arg(long = "diff-base", value_name = "REF", requires = "diff")]
    pub diff_base: Option<String>,
}

impl LintConfigArgs {
    /// Project onto the shared [`LintArgs`] so the `lint-config` subcommand can
    /// reuse the full lint engine. The config source is fixed (the bundled
    /// config-lint plugin, loaded separately), so `--config`, `--prompt-template`,
    /// and the `--agent`/`--rule` selectors are intentionally not exposed here.
    pub fn into_lint_args(self) -> LintArgs {
        LintArgs {
            files: self.files,
            oneharness_config: self.oneharness_config,
            oneharness_bin: self.oneharness_bin,
            model: self.model,
            schema_max_retries: self.schema_max_retries,
            rationales: self.rationales,
            no_rationales: self.no_rationales,
            format: self.format,
            color: self.color,
            verbose: self.verbose,
            max_parallel: self.max_parallel,
            timeout: self.timeout,
            cwd: self.cwd,
            diff: self.diff,
            diff_base: self.diff_base,
            ..Default::default()
        }
    }
}

#[derive(Args, Debug, Default)]
pub struct ConfigArgs {
    /// llmlint config file(s); repeatable. Replaces nested upward discovery.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    pub config: Vec<PathBuf>,

    /// Also emit a `sources` block mapping every rule, agent, and setting to the
    /// file (or plugin URL) it comes from — the path to edit it (a rule also
    /// names any field an `override` pulled from elsewhere). This is the way to
    /// discover where to change something; for one item, `llmlint where <path>`
    /// is more direct.
    #[arg(long = "sources")]
    pub sources: bool,

    /// Directory to resolve config discovery from. Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,
}

#[derive(Args, Debug, Default)]
pub struct WhereArgs {
    /// The config path to locate. A top-level setting (`version`,
    /// `oneharness.model`, `files`, …), `agents.<name>`, `rules.<name>`, or a
    /// single field of a rule, `rules.<name>.<field>` (e.g.
    /// `rules.no_secrets.judges`) to find the file an `override` set it in.
    #[arg(value_name = "PATH")]
    pub path: String,

    /// llmlint config file(s); repeatable. Replaces upward discovery.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    pub config: Vec<PathBuf>,

    /// Directory to resolve config discovery from. Default: cwd.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,
}
