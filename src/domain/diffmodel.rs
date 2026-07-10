//! Parse a unified diff into **change runs** so the prompt can omit the ones that
//! are wholly ignored, without misrepresenting the rest.
//!
//! A change run is a maximal contiguous block of changed (`+`/`-`) lines, bounded
//! by context lines — the atomic unit of inclusion/omission. Its key is the set of
//! **new-file line numbers** of its added lines, matching the coordinate space of
//! inline-ignore suppressions (the ignore comments live in the *current* file). A
//! run is dropped only when *every* one of its added lines is ignored for *every*
//! rule that still applies to the file — never a line pulled out of the middle of a
//! run (that would present a diff that lies about its neighbors). A pure deletion
//! (no added line, so no new-file coordinate to match) is never omittable.
//!
//! Omitted runs render as an honest one-line marker rather than a silently
//! re-headered hunk; the judge can still read the whole file with its own tools, so
//! honesty here costs one line and avoids a confused verdict. The deterministic
//! post-vote cleanup remains the actual enforcement — this only trims the prompt.
//!
//! Pure: it transforms diff text to diff text; no I/O.

/// One maximal run of changed lines within a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRun {
    /// New-file (1-based) line numbers of this run's added (`+`) lines. Empty for a
    /// pure deletion — which is therefore never omittable (nothing to match an
    /// ignore against).
    pub added_lines: Vec<usize>,
    /// The raw diff lines composing the run, in order, each keeping its `+`/`-`
    /// prefix so a kept run renders byte-for-byte.
    pub lines: Vec<String>,
}

/// A hunk body element: a passed-through context line, or a change run that may be
/// omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Context(String),
    Run(ChangeRun),
}

/// One `@@ … @@` hunk: its header and the ordered segments of its body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    header: String,
    segments: Vec<Segment>,
}

/// A parsed unified diff: the preamble lines before the first hunk (`diff --git`,
/// `index`, `--- a/…`, `+++ b/…`) passed through verbatim, then the hunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    preamble: Vec<String>,
    hunks: Vec<Hunk>,
}

/// Parse the new-file start line from a hunk header `@@ -a,b +c,d @@` (the `c`).
/// Returns `None` for an unparseable header, so the caller can pass it through
/// untouched rather than mis-number lines.
fn parse_new_start(header: &str) -> Option<usize> {
    // Find the `+c,d` (or `+c`) token.
    let plus = header.split('+').nth(1)?;
    let num: String = plus.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

impl FileDiff {
    /// Parse `diff` (a single file's unified diff) into preamble + hunks + change
    /// runs. Any line that isn't a recognized hunk element is treated as context
    /// (passed through), so a diff shape we don't model degrades to "show it all".
    pub fn parse(diff: &str) -> FileDiff {
        let mut preamble = Vec::new();
        let mut hunks: Vec<Hunk> = Vec::new();
        // The new-file line counter within the current hunk, and the run being
        // accumulated (if any).
        let mut new_line = 0usize;
        let mut current_run: Option<ChangeRun> = None;

        // Close the open run (if any) into the current hunk's segments.
        fn flush(current_run: &mut Option<ChangeRun>, hunk: Option<&mut Hunk>) {
            if let (Some(run), Some(hunk)) = (current_run.take(), hunk) {
                hunk.segments.push(Segment::Run(run));
            }
        }

        for raw in diff.split_inclusive('\n') {
            // Work on the line without its trailing newline for classification, but
            // keep the original (with newline) for verbatim rendering.
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            if line.starts_with("@@") {
                flush(&mut current_run, hunks.last_mut());
                new_line = parse_new_start(line).unwrap_or(1);
                hunks.push(Hunk {
                    header: raw.to_string(),
                    segments: Vec::new(),
                });
            } else if hunks.is_empty() {
                // Before the first hunk: preamble, passed through verbatim.
                preamble.push(raw.to_string());
            } else if let Some(rest) = line.strip_prefix('+') {
                let _ = rest;
                let run = current_run.get_or_insert_with(|| ChangeRun {
                    added_lines: Vec::new(),
                    lines: Vec::new(),
                });
                run.added_lines.push(new_line);
                run.lines.push(raw.to_string());
                new_line += 1;
            } else if line.starts_with('-') {
                let run = current_run.get_or_insert_with(|| ChangeRun {
                    added_lines: Vec::new(),
                    lines: Vec::new(),
                });
                run.lines.push(raw.to_string());
                // A removed line has no new-file counterpart: do not advance.
            } else if line.starts_with('\\') {
                // "\ No newline at end of file": attach to the open run so it stays
                // with its change, else treat as context.
                if let Some(run) = current_run.as_mut() {
                    run.lines.push(raw.to_string());
                } else if let Some(hunk) = hunks.last_mut() {
                    hunk.segments.push(Segment::Context(raw.to_string()));
                }
            } else {
                // Context line (leading space, or a blank line): closes any run and
                // advances the new-file counter.
                flush(&mut current_run, hunks.last_mut());
                if let Some(hunk) = hunks.last_mut() {
                    hunk.segments.push(Segment::Context(raw.to_string()));
                }
                new_line += 1;
            }
        }
        flush(&mut current_run, hunks.last_mut());

        FileDiff { preamble, hunks }
    }

    /// Render the diff back to text, replacing each run for which `omit` returns
    /// true with a one-line marker naming the omitted line span. Context, headers,
    /// preamble, and kept runs render verbatim, so a kept diff is byte-identical to
    /// the input.
    pub fn render_filtered(&self, omit: impl Fn(&ChangeRun) -> bool) -> String {
        let mut out = String::new();
        for line in &self.preamble {
            out.push_str(line);
        }
        for hunk in &self.hunks {
            out.push_str(&hunk.header);
            for seg in &hunk.segments {
                match seg {
                    Segment::Context(l) => out.push_str(l),
                    Segment::Run(run) if omit(run) => {
                        out.push_str(&omission_marker(run));
                    }
                    Segment::Run(run) => {
                        for l in &run.lines {
                            out.push_str(l);
                        }
                    }
                }
            }
        }
        out
    }
}

/// The honest one-line placeholder for an omitted run, naming the added-line span
/// it stood in for so the judge knows a change was elided (and why).
fn omission_marker(run: &ChangeRun) -> String {
    let span = match (run.added_lines.first(), run.added_lines.last()) {
        (Some(a), Some(b)) if a == b => format!("line {a}"),
        (Some(a), Some(b)) => format!("lines {a}\u{2013}{b}"),
        _ => "changed lines".to_string(),
    };
    let n = run.added_lines.len();
    format!("[llmlint: {n} changed {span} omitted — ignored for all applicable rules]\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index e69de29..1c2d3e4 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,6 @@
 fn a() {}
+// TODO one
+// TODO two
 fn b() {}
-fn old() {}
+fn new() {}
";

    #[test]
    fn parse_splits_runs_at_context_and_numbers_added_lines() {
        let d = FileDiff::parse(DIFF);
        assert_eq!(d.preamble.len(), 4, "four preamble lines before the hunk");
        assert_eq!(d.hunks.len(), 1);
        let runs: Vec<&ChangeRun> = d.hunks[0]
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Run(r) => Some(r),
                Segment::Context(_) => None,
            })
            .collect();
        // Two runs: the added TODO block (new lines 2,3) and the old/new swap.
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].added_lines, vec![2, 3]);
        // The swap: `-fn old` (no new line) then `+fn new` at new line 5.
        assert_eq!(runs[1].added_lines, vec![5]);
    }

    #[test]
    fn render_with_no_omission_is_byte_identical() {
        let d = FileDiff::parse(DIFF);
        assert_eq!(d.render_filtered(|_| false), DIFF);
    }

    #[test]
    fn omitting_a_run_replaces_it_with_a_marker_and_keeps_the_rest() {
        let d = FileDiff::parse(DIFF);
        // Omit the TODO block (added lines 2,3) only.
        let out = d.render_filtered(|r| r.added_lines == vec![2, 3]);
        assert!(out.contains("2 changed lines 2\u{2013}3 omitted"), "{out}");
        // The TODO text is gone…
        assert!(!out.contains("// TODO one"), "{out}");
        // …but the untouched swap and context remain.
        assert!(out.contains("+fn new() {}"), "{out}");
        assert!(out.contains(" fn a() {}"), "{out}");
        // The hunk header is preserved (not re-numbered).
        assert!(out.contains("@@ -1,4 +1,6 @@"), "{out}");
    }

    #[test]
    fn a_single_omitted_line_reads_as_line_n() {
        let d = FileDiff::parse(DIFF);
        let out = d.render_filtered(|r| r.added_lines == vec![5]);
        assert!(out.contains("1 changed line 5 omitted"), "{out}");
    }

    #[test]
    fn a_pure_deletion_run_has_no_added_lines() {
        let diff = "\
@@ -1,3 +1,2 @@
 keep
-gone
 tail
";
        let d = FileDiff::parse(diff);
        let run = d.hunks[0]
            .segments
            .iter()
            .find_map(|s| match s {
                Segment::Run(r) => Some(r),
                Segment::Context(_) => None,
            })
            .unwrap();
        assert!(
            run.added_lines.is_empty(),
            "pure deletion has no new-file line"
        );
        // Its marker degrades to the generic wording (never actually omitted in
        // practice, since the predicate refuses an empty span).
        assert!(omission_marker(run).contains("changed lines omitted"));
    }

    #[test]
    fn no_newline_marker_attaches_to_its_run_or_to_context() {
        // A "\ No newline" after an added line stays with that run; after a context
        // line it is passed through as context. Both render byte-identical.
        let attached = "@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n";
        let d = FileDiff::parse(attached);
        let run = d.hunks[0]
            .segments
            .iter()
            .find_map(|s| match s {
                Segment::Run(r) => Some(r),
                Segment::Context(_) => None,
            })
            .unwrap();
        assert!(run.lines.iter().any(|l| l.contains("No newline")));
        assert_eq!(d.render_filtered(|_| false), attached);

        let ctx = "@@ -1 +1 @@\n keep\n\\ No newline at end of file\n";
        let d2 = FileDiff::parse(ctx);
        assert_eq!(d2.render_filtered(|_| false), ctx);
    }

    #[test]
    fn an_unparseable_header_still_passes_through() {
        // A malformed hunk header defaults new_start to 1 and renders verbatim.
        let diff = "@@ garbage @@\n context\n+added\n";
        let d = FileDiff::parse(diff);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.render_filtered(|_| false), diff);
    }
}
