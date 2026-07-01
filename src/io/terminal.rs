//! Detect the *audience* of a run so the command layer can decide whether to
//! draw the ephemeral live-progress view and whether to colorize the report.
//!
//! The genuinely-impure part — reading the real process environment and whether
//! stderr is a terminal — lives in [`detect`]. The classification logic is split
//! into small pure helpers that take an environment accessor (`get`), so every
//! branch is exhaustively unit-testable without mutating the process env (which
//! is global and racy under a parallel test runner). See
//! `docs/design/interactive-progress.md`.

use std::io::IsTerminal;

/// Environment signals that decide the live-view / color audience, gathered once
/// at the I/O boundary and handed to the pure resolvers (`ColorChoice::resolve`,
/// `ProgressChoice::resolve`).
#[derive(Debug, Clone, Copy, Default)]
pub struct TermContext {
    /// Whether **stderr** is a terminal. The live view draws to stderr, so this —
    /// not stdout — gates it: a human running `llmlint > report.txt` still sees
    /// progress while stdout is a file.
    pub stderr_tty: bool,
    /// A CI environment (`CI` set to anything but `false`, or a known vendor var).
    pub is_ci: bool,
    /// Running inside an AI coding agent (Claude Code / Cursor / Codex / …). TTY
    /// detection can't catch a PTY-allocating agent, so this is a separate layer.
    pub is_agent: bool,
    /// The `NO_COLOR` convention is in effect, or `TERM=dumb` — the user asked for
    /// plain output, which suppresses both color and the animation.
    pub no_color: bool,
}

impl TermContext {
    /// `true` when color/animation are acceptable on styling grounds alone
    /// (`NO_COLOR` unset and `TERM` not `dumb`).
    pub fn color_ok(&self) -> bool {
        !self.no_color
    }
}

/// Environment variables that indicate an AI coding agent is driving the process.
/// Mirrors the set `std-env`'s `isAgent` checks (what Vitest uses), plus an
/// explicit `LLMLINT_AGENT` escape hatch so a user or test can force the agent
/// path. Presence with a non-empty value counts.
const AGENT_VARS: &[&str] = &[
    "LLMLINT_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE",
    "CURSOR_AGENT",
    "CODEX_SANDBOX",
    "REPL_ID",
    "GEMINI_CLI",
];

/// Vendor-specific CI variables, in addition to the vendor-neutral `CI`. A small
/// curated set covering the common providers (the same ones `ci-info` keys on).
const CI_VARS: &[&str] = &[
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "CIRCLECI",
    "TRAVIS",
    "JENKINS_URL",
    "TF_BUILD",
    "BUILDKITE",
];

/// A variable is "set" for our purposes when it is present and non-empty.
fn present(get: &impl Fn(&str) -> Option<String>, key: &str) -> bool {
    get(key).is_some_and(|v| !v.is_empty())
}

/// Inside an AI coding agent? True when any known agent marker is present.
pub fn is_agent(get: &impl Fn(&str) -> Option<String>) -> bool {
    AGENT_VARS.iter().any(|k| present(get, k))
}

/// In CI? True when `CI` is set to anything but `false`/`0`, or any known vendor
/// var is present. (`ci-info` treats `CI=false` as an explicit opt-out.)
pub fn is_ci(get: &impl Fn(&str) -> Option<String>) -> bool {
    if let Some(v) = get("CI") {
        let v = v.trim();
        if !v.is_empty() && !v.eq_ignore_ascii_case("false") && v != "0" {
            return true;
        }
    }
    CI_VARS.iter().any(|k| present(get, k))
}

/// The `NO_COLOR` convention (any non-empty value) or `TERM=dumb`: the user asked
/// for plain output.
pub fn no_color(get: &impl Fn(&str) -> Option<String>) -> bool {
    present(get, "NO_COLOR") || get("TERM").as_deref() == Some("dumb")
}

/// Gather the real audience signals: the live stderr terminal state plus the
/// process environment classified by the pure helpers above.
pub fn detect() -> TermContext {
    let get = |k: &str| std::env::var(k).ok();
    TermContext {
        stderr_tty: std::io::stderr().is_terminal(),
        is_ci: is_ci(&get),
        is_agent: is_agent(&get),
        no_color: no_color(&get),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an env accessor from key/value pairs for the pure helpers.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn agent_detected_from_any_known_marker() {
        for key in AGENT_VARS {
            assert!(is_agent(&env(&[(key, "1")])), "{key} should mark an agent");
        }
        // Claude Code and Cursor by their documented vars.
        assert!(is_agent(&env(&[("CLAUDECODE", "1")])));
        assert!(is_agent(&env(&[("CURSOR_AGENT", "true")])));
    }

    #[test]
    fn no_agent_when_unset_or_empty() {
        assert!(!is_agent(&env(&[])));
        // Present but empty does not count.
        assert!(!is_agent(&env(&[("CLAUDECODE", "")])));
        assert!(!is_agent(&env(&[("PATH", "/usr/bin")])));
    }

    #[test]
    fn ci_from_neutral_var_and_vendors() {
        assert!(is_ci(&env(&[("CI", "true")])));
        assert!(is_ci(&env(&[("CI", "1")])));
        assert!(is_ci(&env(&[("GITHUB_ACTIONS", "true")])));
        assert!(is_ci(&env(&[("BUILDKITE", "true")])));
    }

    #[test]
    fn ci_false_is_an_opt_out() {
        // `CI=false`/`0`/empty are explicit "not CI" per the ci-info convention.
        assert!(!is_ci(&env(&[("CI", "false")])));
        assert!(!is_ci(&env(&[("CI", "FALSE")])));
        assert!(!is_ci(&env(&[("CI", "0")])));
        assert!(!is_ci(&env(&[("CI", "")])));
        assert!(!is_ci(&env(&[])));
    }

    #[test]
    fn no_color_from_convention_or_dumb_term() {
        assert!(no_color(&env(&[("NO_COLOR", "1")])));
        // Any non-empty value disables color per no-color.org.
        assert!(no_color(&env(&[("NO_COLOR", "0")])));
        assert!(no_color(&env(&[("TERM", "dumb")])));
        // A normal terminal with NO_COLOR unset keeps color available.
        assert!(!no_color(&env(&[("TERM", "xterm-256color")])));
        assert!(!no_color(&env(&[("NO_COLOR", "")])));
        assert!(!no_color(&env(&[])));
    }

    #[test]
    fn color_ok_tracks_no_color() {
        let ctx = TermContext {
            no_color: true,
            ..Default::default()
        };
        assert!(!ctx.color_ok());
        assert!(TermContext::default().color_ok());
    }
}
