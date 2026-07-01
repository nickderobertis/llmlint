//! The ephemeral live-progress view: rules resolving as their judges return,
//! drawn to **stderr** (so stdout stays the clean report/JSON channel). Built on
//! `indicatif`, which self-hides on a non-terminal — the "don't corrupt captured
//! output" property from `docs/design/interactive-progress.md`.
//!
//! This module is a *dumb renderer*: it knows nothing about voting or oneharness.
//! The `lint` command drives it (which rules exist, when a judge run finishes, the
//! final per-rule status). Kept out of `domain/` because it does terminal I/O; the
//! decision of *whether* to draw lives in [`crate::cli::ProgressChoice`].
//!
//! It takes a [`ProgressDrawTarget`], so production passes `stderr()` (interactive)
//! or `hidden()` (everyone else) while tests pass a `vt100`-backed `InMemoryTerm`
//! and assert on the rendered screen grid — deterministically, with no real TTY.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

/// The steady-tick interval for the spinner animation when drawing to a real
/// terminal. Off in captured/hidden mode so there is no background thread and
/// tests stay deterministic.
const TICK: Duration = Duration::from_millis(120);

/// The resolved status of a rule, mapped by the `lint` command from a
/// `domain::verdict::Outcome`. Drives the glyph + word on the finished line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStatus {
    Pass,
    Fail,
    NotRelevant,
    Skipped,
    /// A judge run could not complete (oneharness/schema failure) and left the
    /// rule with no usable verdict.
    Error,
}

impl LiveStatus {
    /// The glyph + trailing word shown on a finished rule line. The glyph carries
    /// an indicatif color tag (`.green`/`.red`/…) so it reads at a glance on a
    /// terminal; `InMemoryTerm::contents()` strips styling, so tests match the
    /// plain text.
    fn glyph_word(self) -> (&'static str, &'static str) {
        match self {
            LiveStatus::Pass => ("✓", "passed"),
            LiveStatus::Fail => ("✗", "failed"),
            LiveStatus::NotRelevant => ("–", "not relevant"),
            LiveStatus::Skipped => ("–", "skipped"),
            LiveStatus::Error => ("!", "error"),
        }
    }

    fn color(self) -> &'static str {
        match self {
            LiveStatus::Pass => "green",
            LiveStatus::Fail | LiveStatus::Error => "red",
            LiveStatus::NotRelevant | LiveStatus::Skipped => "yellow",
        }
    }
}

/// A live view of a lint run. One spinner line per rule plus a header counting
/// completed judge calls.
pub struct ProgressView {
    mp: MultiProgress,
    header: ProgressBar,
    rules: HashMap<String, ProgressBar>,
    total_runs: usize,
    done_runs: Cell<usize>,
}

impl ProgressView {
    /// Build the view over `rule_names` (shown in the given order), with
    /// `total_runs` judge calls expected. `animate` enables the steady-tick
    /// spinner (real terminal) — leave it off for a hidden/in-memory target so
    /// there is no background draw thread.
    pub fn new(
        target: ProgressDrawTarget,
        rule_names: &[String],
        total_runs: usize,
        animate: bool,
    ) -> Self {
        let mp = MultiProgress::with_draw_target(target);
        let header = mp.add(ProgressBar::new_spinner());
        header.set_style(
            ProgressStyle::with_template("{spinner:.cyan} judging {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        header.set_message(format!("0/{total_runs} judge calls"));

        let pending = ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());
        let mut rules = HashMap::new();
        for name in rule_names {
            let pb = mp.add(ProgressBar::new_spinner());
            pb.set_style(pending.clone());
            pb.set_message(format!("{name}  queued"));
            rules.insert(name.clone(), pb);
        }

        let view = ProgressView {
            mp,
            header,
            rules,
            total_runs,
            done_runs: Cell::new(0),
        };
        if animate {
            view.header.enable_steady_tick(TICK);
            for pb in view.rules.values() {
                pb.enable_steady_tick(TICK);
            }
        }
        // Always force one draw of the initial frame: a fast run can finish before
        // the steady-tick thread's first tick, and a hidden/in-memory target has no
        // thread at all, so without this the header/rows might never render.
        view.header.tick();
        for pb in view.rules.values() {
            pb.tick();
        }
        view
    }

    /// Mark a rule's judges as in flight (spinner + "running").
    pub fn set_running(&self, rule: &str) {
        if let Some(pb) = self.rules.get(rule) {
            pb.set_message(format!("{rule}  running"));
            pb.tick();
        }
    }

    /// Record that one judge call finished; bumps the header count.
    pub fn tick_run(&self) {
        let n = self.done_runs.get() + 1;
        self.done_runs.set(n);
        self.header
            .set_message(format!("{n}/{} judge calls", self.total_runs));
        self.header.tick();
    }

    /// Resolve a rule to its final status: a static, status-colored glyph line
    /// that stays visible until [`finish`](Self::finish) clears the whole block.
    pub fn finish_rule(&self, rule: &str, status: LiveStatus) {
        if let Some(pb) = self.rules.get(rule) {
            let (glyph, word) = status.glyph_word();
            // Freeze the line: stop its spinner but keep the bar *active* (not
            // `finish`ed) so the final `finish()`/`clear()` still erases it —
            // a `finish`ed bar prints permanently and survives `clear()`.
            pb.disable_steady_tick();
            // Color the whole line by status via the template (indicatif styles
            // template literals, not message text). `InMemoryTerm::contents` strips
            // styling, so tests match on the plain glyph + word.
            pb.set_style(
                ProgressStyle::with_template(&format!("{{msg:.{}}}", status.color()))
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            pb.set_message(format!("{glyph} {rule}  {word}"));
            pb.tick();
        }
    }

    /// Clear the entire view from the draw target, leaving stdout (the report)
    /// untouched. Call once, after all rules are resolved and before printing the
    /// report.
    pub fn finish(self) {
        let _ = self.mp.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::InMemoryTerm;

    fn view_over(term: &InMemoryTerm, rules: &[&str], total: usize) -> ProgressView {
        let names: Vec<String> = rules.iter().map(|s| s.to_string()).collect();
        ProgressView::new(
            ProgressDrawTarget::term_like(Box::new(term.clone())),
            &names,
            total,
            false,
        )
    }

    #[test]
    fn initial_frame_lists_rules_as_queued_with_a_header() {
        let term = InMemoryTerm::new(16, 80);
        let _view = view_over(&term, &["rule_a", "rule_b"], 3);
        let screen = term.contents();
        assert!(
            screen.contains("judging 0/3 judge calls"),
            "got: {screen:?}"
        );
        assert!(screen.contains("rule_a  queued"), "got: {screen:?}");
        assert!(screen.contains("rule_b  queued"), "got: {screen:?}");
    }

    #[test]
    fn running_then_finished_updates_the_line_and_header() {
        let term = InMemoryTerm::new(16, 80);
        let view = view_over(&term, &["rule_a", "rule_b"], 2);

        view.set_running("rule_a");
        assert!(term.contents().contains("rule_a  running"));

        view.tick_run();
        view.finish_rule("rule_a", LiveStatus::Pass);
        let screen = term.contents();
        assert!(
            screen.contains("judging 1/2 judge calls"),
            "got: {screen:?}"
        );
        assert!(screen.contains("✓ rule_a  passed"), "got: {screen:?}");
        // The other rule is still queued.
        assert!(screen.contains("rule_b  queued"), "got: {screen:?}");
    }

    #[test]
    fn every_status_renders_its_glyph_and_word() {
        let term = InMemoryTerm::new(16, 80);
        let view = view_over(&term, &["p", "f", "n", "s", "e"], 5);
        view.finish_rule("p", LiveStatus::Pass);
        view.finish_rule("f", LiveStatus::Fail);
        view.finish_rule("n", LiveStatus::NotRelevant);
        view.finish_rule("s", LiveStatus::Skipped);
        view.finish_rule("e", LiveStatus::Error);
        let screen = term.contents();
        assert!(screen.contains("✓ p  passed"), "got: {screen:?}");
        assert!(screen.contains("✗ f  failed"), "got: {screen:?}");
        assert!(screen.contains("– n  not relevant"), "got: {screen:?}");
        assert!(screen.contains("– s  skipped"), "got: {screen:?}");
        assert!(screen.contains("! e  error"), "got: {screen:?}");
    }

    #[test]
    fn finish_clears_the_whole_block() {
        let term = InMemoryTerm::new(16, 80);
        let view = view_over(&term, &["rule_a"], 1);
        view.tick_run();
        view.finish_rule("rule_a", LiveStatus::Pass);
        assert!(!term.contents().is_empty(), "view should have drawn");
        view.finish();
        // The self-erase leaves nothing behind, so a following stdout report is
        // never interleaved with progress fragments.
        assert_eq!(term.contents(), "", "finish() must clear the view");
    }

    #[test]
    fn unknown_rule_names_are_ignored() {
        let term = InMemoryTerm::new(16, 80);
        let view = view_over(&term, &["known"], 1);
        // A stray name (should never happen) is a no-op, not a panic.
        view.set_running("ghost");
        view.finish_rule("ghost", LiveStatus::Pass);
        assert!(term.contents().contains("known  queued"));
    }

    #[test]
    fn animated_mode_enables_steady_tick_without_panicking() {
        // Exercise the real-terminal `animate` path (the steady-tick spinner) that
        // a piped/in-memory run never takes. Drawn to a hidden target so there is
        // no background-thread race on assertions — the point is the code runs and
        // the lifecycle (running -> resolve -> clear) is sound.
        let names = ["rule_a".to_string(), "rule_b".to_string()];
        let view = ProgressView::new(ProgressDrawTarget::hidden(), &names, 2, true);
        view.set_running("rule_a");
        view.tick_run();
        view.finish_rule("rule_a", LiveStatus::Pass);
        view.finish_rule("rule_b", LiveStatus::Fail);
        view.finish();
    }
}
