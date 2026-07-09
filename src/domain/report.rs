//! Assemble rule outcomes into a report: human-readable text or stable JSON,
//! plus the process exit code.

use anstyle::{AnsiColor, Style};
use serde_json::{json, Value};

use crate::domain::plan::PlanExplanation;
use crate::domain::verdict::{Outcome, RuleOutcome, Violation};

// Status styling for the human report. A red failure and green pass are the
// signal a reader scans for; skips are de-emphasized (dim yellow) and run errors
// share the failure red since they too mean "not OK". Whether these are applied
// is the caller's decision (a plain `bool`) so this module stays pure — TTY
// detection, `NO_COLOR`, and the `--color` flag are resolved in the io/command
// layer, never here.
const FAIL_STYLE: Style = AnsiColor::Red.on_default().bold();
const PASS_STYLE: Style = AnsiColor::Green.on_default().bold();
const SKIP_STYLE: Style = AnsiColor::Yellow.on_default().dimmed();
const ERROR_STYLE: Style = AnsiColor::Red.on_default().bold();
// Not-relevant rules are neither pass nor fail; de-emphasized like skips.
const NA_STYLE: Style = AnsiColor::Yellow.on_default().dimmed();

/// Wrap `text` in `style`'s SGR codes when `color` is on, else return it plain.
/// anstyle renders the prefix via `Display` and the reset via the alternate
/// (`{:#}`) form, so a styled span never leaks past its text.
fn paint(text: &str, style: Style, color: bool) -> String {
    if color {
        format!("{style}{text}{style:#}")
    } else {
        text.to_string()
    }
}

/// Outcome tallies for the summary line and machine output.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    pass: usize,
    fail: usize,
    skip: usize,
    ignored: usize,
    not_relevant: usize,
}

/// The full result of a lint run.
#[derive(Debug, Clone)]
pub struct Report {
    pub outcomes: Vec<RuleOutcome>,
    /// Judge runs that could not produce a usable verdict (oneharness/schema
    /// failures). Their presence makes the run exit `2` (could not complete).
    pub run_errors: Vec<String>,
    /// How the judge runs were planned (agents, batches, exclusions). Attached by
    /// the `lint` command so the `-v` report, `--format json`, and the persisted
    /// history record all explain the batching from one source. `None` for reports
    /// built without a plan (e.g. unit tests).
    pub plan: Option<PlanExplanation>,
}

impl Report {
    pub fn new(mut outcomes: Vec<RuleOutcome>, run_errors: Vec<String>) -> Self {
        outcomes.sort_by(|a, b| a.name.cmp(&b.name));
        Report {
            outcomes,
            run_errors,
            plan: None,
        }
    }

    /// Attach the plan explanation (builder style), so the report can render and
    /// persist why the runs were shaped as they were.
    pub fn with_plan(mut self, plan: PlanExplanation) -> Self {
        self.plan = Some(plan);
        self
    }

    fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for o in &self.outcomes {
            match o.outcome {
                Outcome::Pass => c.pass += 1,
                Outcome::Fail => c.fail += 1,
                Outcome::Skipped => c.skip += 1,
                Outcome::Ignored => c.ignored += 1,
                Outcome::NotRelevant => c.not_relevant += 1,
            }
        }
        c
    }

    /// Exit code: `2` if any judge run errored (incomplete), else `1` if any
    /// rule failed, else `0`.
    pub fn exit_code(&self) -> i32 {
        if !self.run_errors.is_empty() {
            2
        } else if self.outcomes.iter().any(|o| o.outcome == Outcome::Fail) {
            1
        } else {
            0
        }
    }

    /// Render the report for humans at the given verbosity. Level `0` (default)
    /// lists failing rules and their locations; `1`+ additionally itemizes every
    /// passing and skipped rule. Operational errors (a run that couldn't
    /// complete) are surfaced at every level, since they explain a `2` exit the
    /// summary only counts. A blank line separates any per-rule/error detail
    /// from the trailing summary. (The oneharness command/result debug view is
    /// emitted separately to stderr at `-v` by the `lint` command.)
    ///
    /// `color` paints the status words (red for `FAIL`/`ERROR`, green for
    /// `PASS`, dim yellow for `SKIP`) and the summary counts with ANSI codes;
    /// the caller decides it (TTY/`NO_COLOR`/`--color`) so this stays pure.
    pub fn to_human(&self, verbosity: u8, color: bool) -> String {
        let mut out = String::new();
        for o in &self.outcomes {
            match o.outcome {
                // Failures are shown even at the default level — they are the
                // actionable result of a lint run.
                Outcome::Fail => {
                    let label = paint("FAIL", FAIL_STYLE, color);
                    out.push_str(&format!("{label} {}{}\n", o.name, votes_suffix(o)));
                    // A failure is the actionable result, so its reasoning shows
                    // at every level (right after the header, before locations).
                    push_reasoning(&mut out, o);
                    for v in &o.violations {
                        out.push_str(&format!("     {}\n", format_violation(v)));
                    }
                }
                // Passing and skipped rules are only itemized at `-v`; at the
                // default level the summary alone accounts for them.
                Outcome::Pass if verbosity >= 1 => {
                    let label = paint("PASS", PASS_STYLE, color);
                    out.push_str(&format!("{label} {}{}\n", o.name, votes_suffix(o)));
                    push_reasoning(&mut out, o);
                }
                Outcome::Skipped if verbosity >= 1 => {
                    let label = paint("SKIP", SKIP_STYLE, color);
                    out.push_str(&format!("{label} {} (no files matched)\n", o.name))
                }
                // Ignored rules (all files ignore-file'd) are a reasoned exemption;
                // itemized at `-v` so the reader can tell them from an incidental
                // skip, hidden by default like passes/skips.
                Outcome::Ignored if verbosity >= 1 => {
                    let label = paint("IGN", SKIP_STYLE, color);
                    out.push_str(&format!("{label} {} (all files ignored)\n", o.name))
                }
                // Not-relevant rules carry an explanation worth surfacing at
                // `-v`, so the reader can tell "the judge ruled this N/A" apart
                // from "the property held".
                Outcome::NotRelevant if verbosity >= 1 => {
                    let label = paint("N/A", NA_STYLE, color);
                    out.push_str(&format!("{label} {} (not relevant)\n", o.name));
                    push_reasoning(&mut out, o);
                }
                Outcome::Pass | Outcome::Skipped | Outcome::Ignored | Outcome::NotRelevant => {}
            }
        }
        for e in &self.run_errors {
            let label = paint("ERROR", ERROR_STYLE, color);
            out.push_str(&format!("{label} {e}\n"));
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let Counts {
            pass,
            fail,
            skip,
            ignored,
            not_relevant,
        } = self.counts();
        // Color the counts that carry signal: passes green, failures red (only
        // when there are any — a green-tinted "0 failed" reads wrong), errors
        // red. The skip count stays plain; it is neither good nor bad news.
        let passed = paint(&format!("{pass} passed"), PASS_STYLE, color);
        let failed = if fail > 0 {
            paint(&format!("{fail} failed"), FAIL_STYLE, color)
        } else {
            format!("{fail} failed")
        };
        out.push_str(&format!(
            "{} rules: {passed}, {failed}, {skip} skipped",
            self.outcomes.len(),
        ));
        // Append the ignored and not-relevant counts only when there are any, so a
        // run with neither keeps the familiar three-part summary unchanged.
        if ignored > 0 {
            out.push_str(&format!(", {ignored} ignored"));
        }
        if not_relevant > 0 {
            out.push_str(&format!(", {not_relevant} not relevant"));
        }
        if !self.run_errors.is_empty() {
            let errored = paint(
                &format!("{} errored", self.run_errors.len()),
                ERROR_STYLE,
                color,
            );
            out.push_str(&format!(", {errored}"));
        }
        out.push('\n');
        // At `-v`, append the plan explanation so a reader can see (and debug) how
        // the judge runs were batched and which files were excluded. The report on
        // stdout stays parseable — the plan is a trailing, clearly-headed block.
        if verbosity >= 1 {
            if let Some(plan) = &self.plan {
                if !plan.is_empty() {
                    out.push('\n');
                    out.push_str(&plan.to_human());
                }
            }
        }
        out
    }

    pub fn to_json(&self) -> Value {
        let Counts {
            pass,
            fail,
            skip,
            ignored,
            not_relevant,
        } = self.counts();
        let mut obj = json!({
            "summary": {
                "total": self.outcomes.len(),
                "passed": pass,
                "failed": fail,
                "skipped": skip,
                "ignored": ignored,
                "not_relevant": not_relevant,
                "errored": self.run_errors.len(),
            },
            "rules": self.outcomes,
            "errors": self.run_errors,
        });
        // The plan section is present only when a plan was attached (the `lint`
        // path), keeping test/JSON output for plan-less reports byte-stable.
        if let Some(plan) = &self.plan {
            if let (Value::Object(map), Ok(pv)) = (&mut obj, serde_json::to_value(plan)) {
                map.insert("plan".to_string(), pv);
            }
        }
        obj
    }
}

/// The `(X/Y judges held)` suffix for a multi-judge rule, else empty.
fn votes_suffix(o: &RuleOutcome) -> String {
    if o.votes_total > 1 {
        format!(" ({}/{} judges held)", o.votes_hold, o.votes_total)
    } else {
        String::new()
    }
}

/// Append a rule's reasoning under its header. For a multi-judge rule this is a
/// per-judge breakdown — each judge's result (`held`/`violated`) and rationale,
/// so disagreement is visible; for a single judge it is the one rationale line.
fn push_reasoning(out: &mut String, o: &RuleOutcome) {
    if !o.judges.is_empty() {
        for (i, j) in o.judges.iter().enumerate() {
            let verdict = if !j.relevant {
                "not relevant"
            } else if j.holds {
                "held"
            } else {
                "violated"
            };
            match j
                .rationale
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
            {
                Some(r) => out.push_str(&format!("     judge {} {verdict}: {r}\n", i + 1)),
                None => out.push_str(&format!("     judge {} {verdict}\n", i + 1)),
            }
        }
    } else if let Some(r) = &o.rationale {
        let r = r.trim();
        if !r.is_empty() {
            out.push_str(&format!("     rationale: {r}\n"));
        }
    }
}

fn format_violation(v: &Violation) -> String {
    let mut loc = String::new();
    if let Some(file) = &v.file {
        loc.push_str(file);
        if let Some(line) = v.line {
            loc.push_str(&format!(":{line}"));
            if let Some(end) = v.end_line {
                loc.push_str(&format!("-{end}"));
            }
        }
    }
    let msg = v.message.as_deref().unwrap_or("violation");
    if loc.is_empty() {
        msg.to_string()
    } else {
        format!("{loc}: {msg}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::verdict::JudgeOpinion;

    fn fail(name: &str, v: Vec<Violation>) -> RuleOutcome {
        RuleOutcome {
            name: name.into(),
            rationale: None,
            outcome: Outcome::Fail,
            votes_total: 1,
            votes_hold: 0,
            judges: vec![],
            violations: v,
        }
    }
    fn pass(name: &str) -> RuleOutcome {
        RuleOutcome {
            name: name.into(),
            rationale: None,
            outcome: Outcome::Pass,
            votes_total: 1,
            votes_hold: 1,
            judges: vec![],
            violations: vec![],
        }
    }
    fn with_rationale(mut o: RuleOutcome, why: &str) -> RuleOutcome {
        o.rationale = Some(why.into());
        o
    }
    fn opinion(holds: bool, why: Option<&str>) -> JudgeOpinion {
        JudgeOpinion {
            relevant: true,
            holds,
            rationale: why.map(Into::into),
        }
    }
    fn not_relevant(name: &str) -> RuleOutcome {
        RuleOutcome {
            name: name.into(),
            rationale: Some("change does not touch SQL".into()),
            outcome: Outcome::NotRelevant,
            votes_total: 1,
            votes_hold: 0,
            judges: vec![],
            violations: vec![],
        }
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Report::new(vec![pass("a")], vec![]).exit_code(), 0);
        assert_eq!(Report::new(vec![fail("a", vec![])], vec![]).exit_code(), 1);
        assert_eq!(
            Report::new(vec![pass("a")], vec!["boom".into()]).exit_code(),
            2
        );
    }

    #[test]
    fn default_output_lists_failures_but_not_passes_or_skips() {
        let r = Report::new(
            vec![
                fail(
                    "no_inline_sql",
                    vec![
                        Violation {
                            file: Some("src/db.rs".into()),
                            line: Some(42),
                            end_line: Some(45),
                            message: Some("inline SQL".into()),
                        },
                        Violation {
                            message: Some("architectural drift".into()),
                            ..Default::default()
                        },
                    ],
                ),
                pass("layered"),
                RuleOutcome::skipped("nofiles"),
            ],
            vec![],
        );
        let text = r.to_human(0, false);
        // Failing rule and its locations are shown at the default level...
        assert!(text.contains("FAIL no_inline_sql"));
        assert!(text.contains("src/db.rs:42-45: inline SQL"));
        assert!(text.contains("architectural drift"));
        // ...but passing/skipped rules are only counted, not itemized.
        assert!(!text.contains("PASS layered"));
        assert!(!text.contains("SKIP nofiles"));
        assert!(text.contains("3 rules: 1 passed, 1 failed, 1 skipped"));
    }

    #[test]
    fn all_passing_default_output_is_just_the_summary() {
        let r = Report::new(vec![pass("a"), pass("b")], vec![]);
        // No failures, default verbosity: a single line, no leading blank line.
        assert_eq!(
            r.to_human(0, false),
            "2 rules: 2 passed, 0 failed, 0 skipped\n"
        );
    }

    #[test]
    fn color_paints_status_words_and_counts_when_enabled() {
        let r = Report::new(
            vec![
                fail("broke", vec![]),
                pass("ok"),
                RuleOutcome::skipped("nofiles"),
            ],
            vec!["judge timed out".into()],
        );
        let plain = r.to_human(1, false);
        // Without color the output is byte-for-byte the legacy text: no escapes.
        assert!(!plain.contains('\u{1b}'), "no ANSI when color is off");

        let colored = r.to_human(1, true);
        // The status words keep their plain text but are now wrapped in SGR codes
        // — anstyle emits bold (`1m`) then the color (red `31m` for FAIL/ERROR,
        // green `32m` for PASS) as separate escapes, each span reset (`0m`) so
        // color never bleeds past it.
        const RED: &str = "\u{1b}[1m\u{1b}[31m";
        const GREEN: &str = "\u{1b}[1m\u{1b}[32m";
        const RESET: &str = "\u{1b}[0m";
        assert!(colored.contains(&format!("{RED}FAIL{RESET} broke")));
        assert!(colored.contains(&format!("{GREEN}PASS{RESET} ok")));
        assert!(colored.contains(&format!("{RED}ERROR{RESET} judge timed out")));
        // The signal-bearing summary counts are painted too.
        assert!(colored.contains(&format!("{GREEN}1 passed{RESET}")));
        assert!(colored.contains(&format!("{RED}1 failed{RESET}")));
        assert!(colored.contains(&format!("{RED}1 errored{RESET}")));
        // Stripping the escapes recovers exactly the uncolored rendering.
        assert_eq!(strip_ansi(&colored), plain);
    }

    #[test]
    fn color_leaves_a_zero_failure_count_unpainted() {
        // A green "0 failed" would misread as good news about failures; an
        // all-pass summary should carry no red at all.
        let r = Report::new(vec![pass("a")], vec![]);
        let colored = r.to_human(0, true);
        assert!(colored.contains("0 failed"));
        assert!(!colored.contains("\u{1b}[31m"), "no red on a clean run");
    }

    /// Drop ANSI SGR sequences (`ESC [ ... m`) so a colored render can be
    /// compared to its plain counterpart.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                // Skip until the terminating 'm' of the CSI sequence.
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn verbose_itemizes_passing_and_skipped_rules_too() {
        let r = Report::new(
            vec![
                fail(
                    "no_inline_sql",
                    vec![Violation {
                        file: Some("src/db.rs".into()),
                        line: Some(42),
                        end_line: Some(45),
                        message: Some("inline SQL".into()),
                    }],
                ),
                pass("layered"),
                RuleOutcome::skipped("nofiles"),
            ],
            vec![],
        );
        let text = r.to_human(1, false);
        assert!(text.contains("FAIL no_inline_sql"));
        assert!(text.contains("src/db.rs:42-45: inline SQL"));
        assert!(text.contains("PASS layered"));
        assert!(text.contains("SKIP nofiles (no files matched)"));
        assert!(text.contains("3 rules: 1 passed, 1 failed, 1 skipped"));
    }

    #[test]
    fn vote_split_shows_at_default_errors_at_every_level() {
        let r = Report::new(
            vec![
                RuleOutcome {
                    name: "voted".into(),
                    rationale: None,
                    outcome: Outcome::Fail,
                    votes_total: 3,
                    votes_hold: 1,
                    judges: vec![],
                    violations: vec![],
                },
                RuleOutcome::skipped("nofiles"),
            ],
            vec!["judge timed out".into()],
        );
        // Default: the failure (with vote split) and the operational error are
        // both shown; the skipped rule is not itemized.
        let quiet = r.to_human(0, false);
        assert!(quiet.contains("FAIL voted (1/3 judges held)"));
        assert!(quiet.contains("ERROR judge timed out"));
        assert!(quiet.contains("1 errored"));
        assert!(!quiet.contains("SKIP nofiles"));

        // Verbose itemizes the skipped rule as well.
        let text = r.to_human(1, false);
        assert!(text.contains("SKIP nofiles (no files matched)"));
    }

    #[test]
    fn rationale_shows_for_failures_at_default_and_for_all_rules_at_verbose() {
        let r = Report::new(
            vec![
                with_rationale(
                    fail(
                        "no_inline_sql",
                        vec![Violation {
                            message: Some("inline SQL".into()),
                            ..Default::default()
                        }],
                    ),
                    "raw SQL string built in db.rs",
                ),
                with_rationale(pass("layered"), "imports only flow downward"),
            ],
            vec![],
        );

        // Default: the failing rule's rationale is shown (before its violation);
        // the passing rule isn't itemized at all, so neither is its rationale.
        let quiet = r.to_human(0, false);
        assert!(quiet.contains("FAIL no_inline_sql"));
        assert!(quiet.contains("     rationale: raw SQL string built in db.rs"));
        let fail_idx = quiet.find("rationale:").unwrap();
        let viol_idx = quiet.find("inline SQL").unwrap();
        assert!(fail_idx < viol_idx, "rationale precedes the violation");
        assert!(!quiet.contains("imports only flow downward"));

        // Verbose: every evaluated rule shows its rationale.
        let loud = r.to_human(1, false);
        assert!(loud.contains("PASS layered"));
        assert!(loud.contains("     rationale: imports only flow downward"));
    }

    #[test]
    fn multi_judge_shows_each_judges_result_and_rationale() {
        let mut failing = fail(
            "voted_rule",
            vec![Violation {
                message: Some("inline SQL".into()),
                ..Default::default()
            }],
        );
        failing.votes_total = 3;
        failing.votes_hold = 1;
        failing.judges = vec![
            opinion(false, Some("raw SQL at db.rs:42")),
            opinion(true, Some("uses the query layer")),
            opinion(false, None), // a judge that gave no rationale
        ];
        let mut passing = pass("agreed");
        passing.votes_total = 3;
        passing.votes_hold = 2;
        passing.judges = vec![
            opinion(true, Some("clean")),
            opinion(false, Some("looked off")),
            opinion(true, Some("fine")),
        ];
        let r = Report::new(vec![failing, passing], vec![]);

        // Default: the failure shows every judge's result + rationale (a missing
        // rationale still shows the bare result), plus the aggregated violation.
        let quiet = r.to_human(0, false);
        assert!(quiet.contains("FAIL voted_rule (1/3 judges held)"));
        assert!(quiet.contains("judge 1 violated: raw SQL at db.rs:42"));
        assert!(quiet.contains("judge 2 held: uses the query layer"));
        assert!(quiet.contains("judge 3 violated\n"));
        assert!(quiet.contains("inline SQL"));
        // The passing rule isn't itemized at the default level.
        assert!(!quiet.contains("agreed"));

        // Verbose: the passing multi-judge rule shows its breakdown too, with the
        // dissent visible.
        let loud = r.to_human(1, false);
        assert!(loud.contains("PASS agreed (2/3 judges held)"));
        assert!(loud.contains("judge 2 violated: looked off"));
    }

    #[test]
    fn rationale_is_carried_in_json_when_present() {
        let r = Report::new(
            vec![with_rationale(pass("a"), "all good"), fail("b", vec![])],
            vec![],
        );
        let j = r.to_json();
        assert_eq!(j["rules"][0]["rationale"], "all good");
        // A rule with no rationale omits the key entirely.
        assert!(j["rules"][1].get("rationale").is_none());
    }

    #[test]
    fn not_relevant_is_hidden_by_default_itemized_at_verbose_and_exits_clean() {
        let r = Report::new(vec![pass("a"), not_relevant("sql_rule")], vec![]);
        // Not relevant is not a failure: a run with only passes + not-relevant
        // rules exits 0.
        assert_eq!(r.exit_code(), 0);

        // Default level: the not-relevant rule is only counted, with its own
        // summary segment so it isn't conflated with passes or skips.
        let quiet = r.to_human(0, false);
        assert!(!quiet.contains("sql_rule"));
        assert!(quiet.contains("2 rules: 1 passed, 0 failed, 0 skipped, 1 not relevant"));

        // `-v`: the rule is itemized as N/A with its rationale.
        let loud = r.to_human(1, false);
        assert!(loud.contains("N/A sql_rule (not relevant)"));
        assert!(loud.contains("rationale: change does not touch SQL"));
    }

    #[test]
    fn summary_omits_not_relevant_segment_when_there_are_none() {
        // No conditional rules -> the familiar three-part summary is unchanged.
        let r = Report::new(vec![pass("a")], vec![]);
        assert_eq!(
            r.to_human(0, false),
            "1 rules: 1 passed, 0 failed, 0 skipped\n"
        );
    }

    #[test]
    fn multi_judge_breakdown_shows_a_not_relevant_judge() {
        let mut o = not_relevant("scoped");
        o.votes_total = 3;
        o.judges = vec![
            opinion(false, Some("touches SQL, violated")),
            JudgeOpinion {
                relevant: false,
                holds: false,
                rationale: Some("no SQL in this change".into()),
            },
            JudgeOpinion {
                relevant: false,
                holds: false,
                rationale: None,
            },
        ];
        let r = Report::new(vec![o], vec![]);
        let loud = r.to_human(1, false);
        assert!(loud.contains("N/A scoped (not relevant)"));
        assert!(loud.contains("judge 2 not relevant: no SQL in this change"));
        assert!(loud.contains("judge 3 not relevant\n"));

        // JSON omits `relevant` for the relevant judge and emits it (false) for
        // the abstaining ones.
        let j = r.to_json();
        let judges = j["rules"][0]["judges"].as_array().unwrap();
        assert!(judges[0].get("relevant").is_none());
        assert_eq!(judges[1]["relevant"], false);
    }

    #[test]
    fn json_output_is_stable_shape() {
        let r = Report::new(vec![pass("a"), fail("b", vec![])], vec![]);
        let j = r.to_json();
        assert_eq!(j["summary"]["total"], 2);
        assert_eq!(j["summary"]["passed"], 1);
        assert_eq!(j["summary"]["failed"], 1);
        // Outcomes are sorted by name.
        assert_eq!(j["rules"][0]["name"], "a");
        assert_eq!(j["rules"][0]["outcome"], "pass");
        assert_eq!(j["rules"][1]["outcome"], "fail");
        // No plan attached -> no `plan` key (byte-stable for plan-less reports).
        assert!(j.get("plan").is_none());
    }

    #[test]
    fn ignored_is_hidden_by_default_itemized_at_verbose_and_exits_clean() {
        let r = Report::new(
            vec![pass("a"), RuleOutcome::ignored("vendored_rule")],
            vec![],
        );
        // Ignored is not a failure — a pass + ignored run exits 0.
        assert_eq!(r.exit_code(), 0);

        // Default: the ignored rule is only counted, in its own summary segment so
        // it is not conflated with passes or skips.
        let quiet = r.to_human(0, false);
        assert!(!quiet.contains("vendored_rule"));
        assert!(quiet.contains("2 rules: 1 passed, 0 failed, 0 skipped, 1 ignored"));

        // `-v`: itemized as IGN with its reason.
        let loud = r.to_human(1, false);
        assert!(
            loud.contains("IGN vendored_rule (all files ignored)"),
            "{loud}"
        );

        // JSON carries the ignored count and the outcome value.
        let j = r.to_json();
        assert_eq!(j["summary"]["ignored"], 1);
        let ign = j["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == "vendored_rule")
            .unwrap();
        assert_eq!(ign["outcome"], "ignored");
    }

    #[test]
    fn a_plan_renders_in_verbose_human_and_in_json() {
        use crate::domain::plan::{AgentPlan, BatchPlan, JudgePlan, PlanExplanation};
        let plan = PlanExplanation {
            agents: vec![AgentPlan {
                agent: "default".into(),
                batch_size: 20,
                model: None,
                harness: None,
                judges: vec![JudgePlan {
                    judge_index: 1,
                    batches: vec![BatchPlan {
                        id: 1,
                        rules: vec!["a".into(), "b".into()],
                        files: vec!["src/lib.rs".into()],
                        excluded_files: vec![],
                        reused_files: vec![],
                    }],
                }],
            }],
            skipped: vec![],
            optimization: Default::default(),
        };
        let r = Report::new(vec![pass("a")], vec![]).with_plan(plan);

        // Default verbosity does not show the plan; `-v` appends it.
        assert!(!r.to_human(0, false).contains("Plan:"));
        let loud = r.to_human(1, false);
        assert!(
            loud.contains("Plan: 1 judge call(s) across 1 agent(s)"),
            "{loud}"
        );
        assert!(loud.contains("batch 1: [a, b]"), "{loud}");

        // JSON embeds the structured plan.
        let j = r.to_json();
        assert_eq!(j["plan"]["agents"][0]["agent"], "default");
        assert_eq!(j["plan"]["agents"][0]["judges"][0]["batches"][0]["id"], 1);
    }
}
