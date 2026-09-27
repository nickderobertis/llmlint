//! Journeys for rules judged by TypeSafe's Jev (`judge: jev`). The real binary
//! talks HTTP to an in-test mock of `POST /v1/systemone` — the genuinely
//! external boundary, like the mock oneharness — which answers each question
//! from a per-rule script and records every request so a journey can assert
//! what llmlint actually sent.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Map, Value};

use super::{git, init_repo, mock_path, runlog_calls, Project};

/// How the mock answers one rule. Unset fields mean "holds, relevant, no line".
#[derive(Clone, Copy, Default)]
struct Script {
    holds: Option<f64>,
    relevant: Option<f64>,
    line: Option<u64>,
}

fn holds(p: f64) -> Script {
    Script {
        holds: Some(p),
        ..Script::default()
    }
}

fn violates_at(line: u64) -> Script {
    Script {
        holds: Some(0.08),
        relevant: None,
        line: Some(line),
    }
}

/// One recorded request: its `Authorization` header and parsed JSON body.
#[derive(Clone)]
struct Seen {
    auth: String,
    body: Value,
}

struct MockJev {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl MockJev {
    /// Answer from `scripts` (rule name -> script). `fail_first` HTTP statuses
    /// are returned, in order, before any answer.
    fn start(scripts: &[(&str, Script)], fail_first: &[u16]) -> MockJev {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = Arc::clone(&seen);
        let scripts: BTreeMap<String, Script> =
            scripts.iter().map(|(n, s)| (n.to_string(), *s)).collect();
        let mut failures: Vec<u16> = fail_first.to_vec();
        failures.reverse();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut len, mut auth) = (0usize, String::new());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        auth = line["authorization:".len()..].trim().to_string();
                    }
                }
                let mut buf = vec![0; len];
                reader.read_exact(&mut buf).unwrap();
                let body: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
                log.lock().unwrap().push(Seen {
                    auth,
                    body: body.clone(),
                });
                let (status, extra, reply) = match failures.pop() {
                    Some(code) => (
                        code,
                        "retry-after: 0\r\n",
                        format!("{{\"error\":\"status {code}\"}}"),
                    ),
                    None => (200, "", answer(&body, &scripts).to_string()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\n{extra}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        MockJev { url, seen }
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// Answer every question in `request` from the scripts.
fn answer(request: &Value, scripts: &BTreeMap<String, Script>) -> Value {
    let mut answers = Map::new();
    let path = request["state"]["path"].as_str().unwrap_or_default();
    for (id, q) in request["questions"].as_object().unwrap() {
        let mut parts = id.split(':');
        let (kind, rule) = (parts.next().unwrap(), parts.next().unwrap());
        // A rule's `checks` / `examples` replace its question with
        // `<id>#check<n>` / `<id>#examples` (the suffix rides on the last part).
        let variant = id.split_once('#').map(|(_, v)| v);
        let rule = rule.split('#').next().unwrap();
        // A `rule@path` script answers for that file only, else the rule's.
        let s = scripts
            .get(&format!("{rule}@{path}"))
            .or_else(|| scripts.get(rule))
            .copied()
            .unwrap_or_default();
        let a = match kind {
            // Every check asks whether its violation happens.
            _ if variant.is_some_and(|v| v.starts_with("check")) => {
                json!({"type": "noul", "noul": 1.0 - s.holds.unwrap_or(0.95)})
            }
            "holds" => json!({"type": "noul", "noul": s.holds.unwrap_or(0.95)}),
            "relevant" => json!({"type": "noul", "noul": s.relevant.unwrap_or(0.9)}),
            // Claim and site questions ask for a *violation*: p = 1 - p(holds).
            "claim" | "site" | "entry" | "item" | "files" => {
                json!({"type": "noul", "noul": 1.0 - s.holds.unwrap_or(0.95)})
            }
            _ => {
                let options: Vec<&String> = q["criteria"].as_object().unwrap().keys().collect();
                let want = s.line.map(|n| format!("L{n}"));
                let (choice, p) = match want.filter(|w| options.contains(&w)) {
                    Some(w) => (w, 0.8),
                    None => (options[0].clone(), 0.05),
                };
                json!({"type": "choice", "choice": choice, "probabilities": {choice.clone(): p}, "confidence": p})
            }
        };
        answers.insert(id.clone(), a);
    }
    json!({"model": "jev-1.13.0", "answers": answers, "usage": {"input_tokens": 250, "output_tokens": 4}})
}

const SRC: &str = "fn load() {\n    let text = read()?;\n    parse(&text)\n}\n";

/// A project whose `default` agent is judged by Jev.
fn jev_project(rules: &str) -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  default:\n    judge: jev\nrules:\n{rules}"),
    );
    p.write("src/a.rs", SRC);
    p
}

const ONE_RULE: &str = "  - { name: errors_have_context, description: \"Every error carries context.\", require_line_attribution: true }\n";

/// `llmlint --format json` against the mock, with no oneharness anywhere: the
/// harness binary override points at nothing, so any attempt to use it fails.
fn run_jev(p: &Project, jev: &MockJev) -> assert_cmd::assert::Assert {
    let mut c = p.bare();
    c.args(["--format", "json", "--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "test-key");
    c.assert()
}

fn json_of(a: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&a.get_output().stdout).unwrap()
}

#[test]
fn jev_rule_passes_without_oneharness_and_reports_usage() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[("errors_have_context", holds(0.93))], &[]);
    let a = run_jev(&p, &jev).code(0);
    let out = json_of(&a);
    assert_eq!(out["rules"][0]["outcome"], "pass");
    let usage = &out["usage"]["jev"];
    assert_eq!(usage["calls"], 1);
    assert_eq!(usage["input_tokens"], 250);
    assert!((usage["cost_usd"].as_f64().unwrap() - 250.0 * 0.042 / 1e6).abs() < 1e-12);
    assert!(out["usage"].get("harness").is_none(), "{}", out["usage"]);
    // A passing rule still carries its raw probability, for offline tuning.
    let score = &out["rules"][0]["scores"][0];
    assert_eq!(score["file"], "src/a.rs");
    assert_eq!(score["p_holds"], 0.93);

    let reqs = jev.requests();
    assert_eq!(reqs.len(), 1, "one request per file");
    let body = &reqs[0].body;
    assert_eq!(reqs[0].auth, "Bearer test-key");
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(body["state"]["path"], "src/a.rs");
    assert!(body["state"]["content"]
        .as_str()
        .unwrap()
        .starts_with("L1| fn load() {\nL2| "));
    let q = &body["questions"]["holds:errors_have_context"];
    assert_eq!(q["type"], "noul");
    assert_eq!(q["instructions"]["rule"], "Every error carries context.");
}

#[test]
fn jev_violation_cites_file_line_and_probability() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[("errors_have_context", violates_at(2))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(1));
    let rule = &out["rules"][0];
    assert_eq!(rule["outcome"], "fail");
    assert_eq!(rule["violations"][0]["file"], "src/a.rs");
    assert_eq!(rule["violations"][0]["line"], 2);
    assert_eq!(
        rule["violations"][0]["message"],
        "jev: p(holds)=0.08, p(line)=0.80"
    );
    assert!(rule.get("rationale").is_none(), "Jev writes no rationale");
}

#[test]
fn jev_human_report_shows_the_violation() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[("errors_have_context", violates_at(2))], &[]);
    p.bare()
        .args(["--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "k")
        .assert()
        .code(1)
        .stdout(predicates::str::contains("FAIL errors_have_context"))
        .stdout(predicates::str::contains("src/a.rs:2"))
        .stdout(predicates::str::contains("p(holds)=0.08"));
}

#[test]
fn jev_conditional_rule_reports_not_relevant() {
    let p = jev_project(
        "  - { name: parses_carefully, description: \"Parsing validates input.\", relevance: \"the file parses input\" }\n",
    );
    let script = Script {
        holds: Some(0.1),
        relevant: Some(0.1),
        line: None,
    };
    let jev = MockJev::start(&[("parses_carefully", script)], &[]);
    let out = json_of(&run_jev(&p, &jev).code(0));
    assert_eq!(out["rules"][0]["outcome"], "not_relevant");
    let q = &jev.requests()[0].body["questions"]["relevant:parses_carefully"];
    assert_eq!(q["instructions"]["condition"], "the file parses input");
}

#[test]
fn a_missing_api_key_fails_before_any_request() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[], &[]);
    let mut c = p.bare();
    c.args(["--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .env("LLMLINT_JEV_URL", &jev.url)
        .env_remove("TYPESAFE_API_KEY");
    c.assert()
        .code(2)
        .stderr(predicates::str::contains("TYPESAFE_API_KEY"));
    assert!(jev.requests().is_empty());
}

#[test]
fn an_auth_failure_is_a_run_error_not_a_verdict() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[], &[401]);
    let out = json_of(&run_jev(&p, &jev).code(2));
    let err = out["errors"][0].as_str().unwrap();
    assert!(err.contains("authentication failed (HTTP 401)"), "{err}");
    assert_eq!(jev.requests().len(), 1, "auth failures are not retried");
}

#[test]
fn rate_limits_and_overload_are_retried() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[("errors_have_context", holds(0.9))], &[429, 529]);
    run_jev(&p, &jev).code(0);
    assert_eq!(jev.requests().len(), 3);
}

#[test]
fn an_inline_ignore_drops_a_jev_violation() {
    let p = jev_project(ONE_RULE);
    p.write(
        "src/a.rs",
        "fn load() {\n    // llmlint: ignore[errors_have_context] read() already names the path\n    let text = read()?;\n    parse(&text)\n}\n",
    );
    let jev = MockJev::start(&[("errors_have_context", violates_at(3))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(0));
    assert_eq!(out["rules"][0]["outcome"], "pass");
}

#[test]
fn harness_and_jev_rules_share_one_run() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  fast:\n    judge: jev\nrules:\n  \
         - { name: on_harness, description: \"Harness-judged.\" }\n  \
         - { name: on_jev, description: \"Jev-judged.\", agent: fast }\n  \
         - { name: rule_override, description: \"Rule-level judge wins.\", judge: jev }\n",
    );
    p.write("src/a.rs", SRC);
    let jev = MockJev::start(&[("on_jev", violates_at(1))], &[]);
    let mut c = p.bare();
    c.args(["--format", "json", "--oneharness-bin"])
        .arg(mock_path())
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "k");
    let out = json_of(&c.assert().code(1));
    let outcome = |name: &str| {
        out["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["outcome"]
            .clone()
    };
    assert_eq!(outcome("on_harness"), "pass");
    assert_eq!(outcome("on_jev"), "fail");
    assert_eq!(outcome("rule_override"), "pass");
    assert_eq!(out["usage"]["harness"]["calls"], 1);
    // Two Jev agents groups (`fast`, and `default` via the rule override): one
    // request per (group, file).
    assert_eq!(out["usage"]["jev"]["calls"], 2);
    let asked: Vec<String> = jev
        .requests()
        .iter()
        .flat_map(|r| {
            r.body["questions"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .filter(|k| k.starts_with("holds:"))
        .collect();
    assert!(
        asked.contains(&"holds:on_jev".into()) && asked.contains(&"holds:rule_override".into())
    );
    assert!(!asked.contains(&"holds:on_harness".into()), "{asked:?}");
}

#[test]
fn llmlint_judge_env_routes_every_rule_to_jev() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        &format!("version: 1\nfiles:\n  include: [\"src/**\"]\nrules:\n{ONE_RULE}"),
    );
    p.write("src/a.rs", SRC);
    let jev = MockJev::start(&[], &[]);
    let mut c = p.bare();
    c.args(["--format", "json", "--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .env("LLMLINT_JUDGE", "jev")
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "k");
    c.assert().code(0);
    assert_eq!(jev.requests().len(), 1);

    let mut bad = p.bare();
    bad.env("LLMLINT_JUDGE", "gpt");
    bad.assert()
        .code(2)
        .stderr(predicates::str::contains("LLMLINT_JUDGE"))
        .stderr(predicates::str::contains("`harness`, `jev`, or `cascade`"));
}

#[test]
fn plan_only_shows_the_jev_group_without_calling_it() {
    let p = jev_project(ONE_RULE);
    let jev = MockJev::start(&[], &[]);
    p.bare()
        .arg("--plan-only")
        .env("LLMLINT_JEV_URL", &jev.url)
        .env_remove("TYPESAFE_API_KEY")
        .assert()
        .code(0)
        .stdout(predicates::str::contains("judge jev: one request per file"))
        .stdout(predicates::str::contains("rules: errors_have_context"));
    assert!(jev.requests().is_empty());
}

#[test]
fn diff_mode_sends_the_diff_and_localizes_to_added_lines() {
    let p = jev_project(ONE_RULE);
    init_repo(p.path());
    git(p.path(), &["add", "."]);
    git(p.path(), &["commit", "-q", "-m", "baseline"]);
    p.write(
        "src/a.rs",
        &format!("{SRC}fn extra() {{\n    let x = read()?;\n}}\n"),
    );
    let jev = MockJev::start(&[("errors_have_context", violates_at(6))], &[]);
    let mut c = p.bare();
    c.args(["--format", "json", "--diff", "--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "k");
    let out = json_of(&c.assert().code(1));
    assert_eq!(out["rules"][0]["violations"][0]["line"], 6);
    let body = &jev.requests()[0].body;
    assert!(body["state"]["diff"]
        .as_str()
        .unwrap()
        .contains("+fn extra() {"));
    let options: Vec<&String> = body["questions"]["line:errors_have_context:0"]["criteria"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(options, ["L5", "L6", "L7"], "only the added lines");
}

#[test]
fn an_oversized_file_is_judged_in_several_requests() {
    let p = jev_project(ONE_RULE);
    let line = "x".repeat(2000);
    let big: String = (0..100).map(|i| format!("// {i} {line}\n")).collect();
    p.write("src/a.rs", &big);
    let jev = MockJev::start(&[], &[]);
    let out = json_of(&run_jev(&p, &jev).code(0));
    let n = jev.requests().len();
    assert!(n >= 2, "{n} request(s)");
    assert_eq!(out["usage"]["jev"]["calls"], n);
}

#[test]
fn jev_settings_resolve_through_config_and_env() {
    let p = jev_project(ONE_RULE);
    p.write(
        "llmlint.yml",
        &format!(
            "version: 1\nfiles:\n  include: [\"src/**\"]\njudge: jev\njev:\n  model: jev-1.13.0\n  url: http://127.0.0.1:9\nrules:\n{ONE_RULE}"
        ),
    );
    let jev = MockJev::start(&[], &[]);
    // The env URL wins over the config's (unreachable) one.
    run_jev(&p, &jev).code(0);
    assert_eq!(jev.requests().len(), 1);
    p.bare()
        .args(["where", "jev.url"])
        .env("LLMLINT_JEV_URL", &jev.url)
        .assert()
        .code(0)
        .stdout(predicates::str::contains("env:LLMLINT_JEV_URL"));
}

// ---- judge: cascade -------------------------------------------------------

/// A cascade project: every rule on the `default` agent, two source files.
fn cascade_project() -> Project {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\nagents:\n  default:\n    judge: cascade\nrules:\n  \
         - { name: errors_have_context, description: \"Every error carries context.\" }\n  \
         - { name: names_are_clear, description: \"Names state intent.\" }\n",
    );
    p.write("src/a.rs", SRC);
    p
}

/// Run a cascade project: Jev from `jev`, the harness from the mock oneharness
/// answering `verdicts` and logging each call's rules under `runlog`.
fn run_cascade(
    p: &Project,
    jev: &MockJev,
    verdicts: &str,
    extra_env: &[(&str, &str)],
) -> (assert_cmd::assert::Assert, Vec<String>) {
    let runlog = p.path().join("runlog");
    std::fs::create_dir_all(&runlog).unwrap();
    let v = p.write_verdicts(verdicts);
    let mut c = p.bare();
    c.args(["--format", "json", "--oneharness-bin"])
        .arg(mock_path())
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "k")
        .env("LLMLINT_MOCK_VERDICTS", v)
        .env("LLMLINT_MOCK_RUNLOG", &runlog);
    for (k, val) in extra_env {
        c.env(k, val);
    }
    let a = c.assert();
    let calls = runlog_calls(&runlog);
    (a, calls)
}

#[test]
fn cascade_confident_answers_never_call_the_harness() {
    let p = cascade_project();
    let jev = MockJev::start(
        &[
            ("errors_have_context", violates_at(2)),
            ("names_are_clear", holds(0.97)),
        ],
        &[],
    );
    let (a, calls) = run_cascade(&p, &jev, "{}", &[]);
    let out = json_of(&a.code(1));
    assert!(calls.is_empty(), "no harness call: {calls:?}");
    assert_eq!(out["rules"][0]["name"], "errors_have_context");
    assert_eq!(out["rules"][0]["outcome"], "fail");
    assert_eq!(
        out["rules"][0]["violations"][0]["message"],
        "jev: p(holds)=0.08, p(line)=0.80"
    );
    assert_eq!(out["rules"][1]["outcome"], "pass");
    assert!(out["plan"].get("escalated").is_none());
    assert!(out["usage"].get("harness").is_none());
}

#[test]
fn cascade_escalates_only_the_uncertain_rule_and_the_harness_decides() {
    let p = cascade_project();
    // An unsure fail (0.40) the harness overturns, and a confident pass.
    let jev = MockJev::start(
        &[
            ("errors_have_context", holds(0.40)),
            ("names_are_clear", holds(0.97)),
        ],
        &[],
    );
    let (a, calls) = run_cascade(&p, &jev, r#"{"errors_have_context": true}"#, &[]);
    let out = json_of(&a.code(0));
    assert_eq!(
        calls,
        ["errors_have_context"],
        "one harness call, for the unsure rule only"
    );
    assert_eq!(out["rules"][0]["outcome"], "pass");
    assert_eq!(
        out["rules"][0]["scores"][0]["p_holds"], 0.4,
        "Jev's number is kept"
    );
    assert_eq!(
        out["plan"]["escalated"]["errors_have_context"],
        json!(["src/a.rs"])
    );
    assert_eq!(out["usage"]["jev"]["calls"], 1);
    assert_eq!(out["usage"]["harness"]["calls"], 1);
}

#[test]
fn cascade_by_default_sends_only_jevs_fails_and_warnings_to_the_harness() {
    let p = cascade_project();
    // 0.80 clears the whole-file warn cut (0.75): a pass that stands. 0.60 is a
    // warning: the harness decides it.
    let jev = MockJev::start(
        &[
            ("errors_have_context", holds(0.60)),
            ("names_are_clear", holds(0.80)),
        ],
        &[],
    );
    let (a, calls) = run_cascade(&p, &jev, r#"{"errors_have_context": false}"#, &[]);
    let out = json_of(&a.code(1));
    assert_eq!(
        calls,
        ["errors_have_context"],
        "only the warned rule escalates"
    );
    assert_eq!(
        out["rules"][0]["outcome"], "fail",
        "the harness confirmed it"
    );
    assert_eq!(out["rules"][1]["outcome"], "pass");
    assert!(out["plan"]["escalated"].get("names_are_clear").is_none());
}

#[test]
fn cascade_harness_can_overturn_an_unsure_pass() {
    let p = cascade_project();
    let jev = MockJev::start(&[("errors_have_context", holds(0.70))], &[]);
    let verdicts = r#"{"errors_have_context": {"holds": false, "violations": [{"file": "src/a.rs", "line": 2, "message": "bare ?"}]}}"#;
    let (a, calls) = run_cascade(&p, &jev, verdicts, &[]);
    let out = json_of(&a.code(1));
    assert_eq!(calls, ["errors_have_context"]);
    let v = &out["rules"][0]["violations"][0];
    assert_eq!(
        (v["line"].clone(), v["message"].clone()),
        (json!(2), json!("bare ?"))
    );
}

#[test]
fn cascade_escalates_per_file_and_keeps_confident_fails() {
    let p = cascade_project();
    p.write("src/b.rs", SRC);
    let jev = MockJev::start(
        &[
            ("errors_have_context@src/a.rs", violates_at(2)),
            ("errors_have_context@src/b.rs", holds(0.6)),
        ],
        &[],
    );
    let (a, calls) = run_cascade(&p, &jev, r#"{"errors_have_context": true}"#, &[]);
    let out = json_of(&a.code(1));
    assert_eq!(calls, ["errors_have_context"]);
    assert_eq!(
        out["plan"]["escalated"]["errors_have_context"],
        json!(["src/b.rs"])
    );
    let rule = &out["rules"][0];
    assert_eq!(
        rule["outcome"], "fail",
        "a.rs's confident fail stands despite b.rs passing"
    );
    assert_eq!(rule["violations"].as_array().unwrap().len(), 1);
    assert_eq!(rule["violations"][0]["file"], "src/a.rs");
}

#[test]
fn cascade_band_comes_from_settings_and_is_validated() {
    let p = cascade_project();
    let jev = MockJev::start(&[("errors_have_context", holds(0.40))], &[]);
    // A zero-width band at the threshold trusts Jev everywhere.
    let (a, calls) = run_cascade(
        &p,
        &jev,
        "{}",
        &[
            ("LLMLINT_JEV_ESCALATE_LOW", "0.5"),
            ("LLMLINT_JEV_ESCALATE_HIGH", "0.5"),
        ],
    );
    a.code(1);
    assert!(calls.is_empty(), "{calls:?}");

    // A band that does not straddle 0.5 is a config error, even for `validate`.
    p.write(
        "llmlint.yml",
        "version: 1\njev:\n  escalate_low: 0.6\nrules:\n  - { name: r, description: \"d\" }\n",
    );
    p.bare()
        .arg("validate")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("jev.escalate_low (0.6)"));
    p.bare()
        .env("LLMLINT_JEV_ESCALATE_HIGH", "1.5")
        .arg("validate")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("LLMLINT_JEV_ESCALATE_HIGH"));
}

#[test]
fn cascade_plan_only_names_the_cascade_group() {
    let p = cascade_project();
    p.bare()
        .arg("--plan-only")
        .assert()
        .code(0)
        .stdout(predicates::str::contains(
            "judge cascade: Jev per file first",
        ));
}

// ---- jev: { unit: doc_claims } --------------------------------------------

const DOCUMENTED: &str = "\
fn cheapest(a: u8, b: u8) -> bool {
    a > b
}

/// Sort line items cheapest first. Never allocates.
pub fn sort(items: &mut [u8]) {
    items.sort_by(|a, b| b.cmp(a));
    let _ = cheapest(1, 2);
}

/// Descriptive docs on a constant are not checked.
pub const LIMIT: u8 = 3;
";

#[test]
fn doc_claims_checks_each_claim_against_its_function_and_helpers() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\njudge: jev\nrules:\n  \
         - { name: docs_match_code, description: \"Doc comments match behavior.\", jev: { unit: doc_claims } }\n",
    );
    p.write("src/a.rs", DOCUMENTED);
    let jev = MockJev::start(&[("docs_match_code", holds(0.1))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(1));
    let v = &out["rules"][0]["violations"][0];
    assert_eq!(
        (v["file"].clone(), v["line"].clone()),
        (json!("src/a.rs"), json!(5))
    );
    assert!(v["message"].as_str().unwrap().contains("p(holds)=0.10"));

    let reqs = jev.requests();
    assert_eq!(
        reqs.len(),
        1,
        "one request for the one documented function (not the const)"
    );
    let body = &reqs[0].body;
    assert!(body["state"]["code"]
        .as_str()
        .unwrap()
        .starts_with("L6| pub fn sort("));
    assert!(body["state"]["helpers"]
        .as_str()
        .unwrap()
        .contains("L1| fn cheapest("));
    let claims: Vec<&str> = body["questions"]
        .as_object()
        .unwrap()
        .values()
        .map(|q| q["instructions"]["claim"].as_str().unwrap())
        .collect();
    assert_eq!(
        claims,
        ["Sort line items cheapest first.", "Never allocates."]
    );
}

#[test]
fn doc_claims_passes_above_its_threshold_and_skips_undocumented_files() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\njudge: jev\nrules:\n  \
         - { name: docs_match_code, description: \"Doc comments match behavior.\", jev: { unit: doc_claims } }\n",
    );
    p.write("src/a.rs", DOCUMENTED);
    p.write("src/b.rs", "fn undocumented() {}\n");
    // 0.65 would warn on a whole-file rule (below its 0.75 warn cut) but
    // clears a claims rule's (0.6).
    let jev = MockJev::start(&[("docs_match_code", holds(0.65))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(0));
    assert_eq!(out["rules"][0]["outcome"], "pass");
    assert_eq!(
        jev.requests().len(),
        1,
        "no request for a file with no documented function"
    );
}

#[test]
fn an_unknown_jev_unit_is_a_config_error() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nrules:\n  - { name: r, description: \"d\", jev: { unit: sentences } }\n",
    );
    p.bare()
        .arg("validate")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unknown variant `sentences`"));
}

// ---- jev: { unit: sites, pattern } ----------------------------------------

#[test]
fn sites_checks_each_matched_line_and_cites_it() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"src/**\"]\njudge: jev\nrules:\n  \
         - { name: errors_have_context, description: \"Every error carries context.\", jev: { unit: sites, pattern: '\\?' } }\n",
    );
    p.write("src/a.rs", SRC);
    let jev = MockJev::start(&[("errors_have_context", holds(0.2))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(1));
    let v = &out["rules"][0]["violations"][0];
    assert_eq!(
        (v["file"].clone(), v["line"].clone()),
        (json!("src/a.rs"), json!(2))
    );
    let body = &jev.requests()[0].body;
    assert_eq!(
        body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["site:errors_have_context:2"]
    );
    assert_eq!(
        body["questions"]["site:errors_have_context:2"]["instructions"]["site"],
        "L2|     let text = read()?;"
    );
}

#[test]
fn a_sites_rule_needs_a_valid_pattern_and_a_pattern_needs_sites() {
    let p = Project::new();
    for (jev, want) in [
        ("{ unit: sites }", "needs a `jev.pattern`"),
        ("{ unit: sites, pattern: '(' }", "invalid `jev.pattern`"),
        (
            "{ unit: file, pattern: 'x' }",
            "only applies to `jev.unit: sites`",
        ),
    ] {
        p.write(
            "llmlint.yml",
            &format!("version: 1\nrules:\n  - {{ name: r, description: \"d\", jev: {jev} }}\n"),
        );
        p.bare()
            .arg("validate")
            .assert()
            .code(2)
            .stderr(predicates::str::contains(want));
    }
}

// ---- jev: { unit: yaml_entries, key } --------------------------------------

#[test]
fn yaml_entries_judges_each_config_rule_on_its_own() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nfiles:\n  include: [\"team/**\"]\njudge: jev\nrules:\n  \
         - { name: names_are_descriptive, description: \"Names describe the check.\", jev: { unit: yaml_entries, key: rules } }\n",
    );
    // A plain YAML file (a `llmlint.yml` here would be discovered as a subtree
    // config and contribute rules of its own).
    p.write(
        "team/checks.yaml",
        "version: 1\nrules:\n  - name: rule1\n    description: |\n      d\n  - { name: fine, description: d }\n",
    );
    let jev = MockJev::start(&[("names_are_descriptive", violates_at(3))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(1));
    assert_eq!(out["rules"][0]["violations"][0]["line"], 3);
    let entries: Vec<String> = jev
        .requests()
        .iter()
        .map(|r| r.body["state"]["entry"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(entries.len(), 2, "one request per rule entry");
    assert!(entries.iter().any(|e| e.starts_with("L3|   - name: rule1")));
}

#[test]
fn a_yaml_entries_rule_needs_a_key_and_a_key_needs_yaml_entries() {
    let p = Project::new();
    for (jev, want) in [
        ("{ unit: yaml_entries }", "needs a `jev.key`"),
        (
            "{ unit: file, key: rules }",
            "only applies to `jev.unit: yaml_entries`",
        ),
    ] {
        p.write(
            "llmlint.yml",
            &format!("version: 1\nrules:\n  - {{ name: r, description: \"d\", jev: {jev} }}\n"),
        );
        p.bare()
            .arg("validate")
            .assert()
            .code(2)
            .stderr(predicates::str::contains(want));
    }
}

// ---- jev: { unit: multi_file } ---------------------------------------------

#[test]
fn multi_file_judges_items_across_files_together() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\njudge: jev\nrules:\n  \
         - { name: flags_documented, description: \"Every flag is documented.\", files: { include: [\"src/cli.rs\", \"README.md\"] }, jev: { unit: multi_file, pattern: 'long = \"([^\"]+)\"' } }\n",
    );
    p.write(
        "src/cli.rs",
        "struct Cli {\n    #[arg(long = \"dry-run\")]\n    dry_run: bool,\n}\n",
    );
    p.write("README.md", "# cli\n");
    let jev = MockJev::start(&[("flags_documented", holds(0.1))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(1));
    let v = &out["rules"][0]["violations"][0];
    assert_eq!(
        (v["file"].clone(), v["line"].clone()),
        (json!("src/cli.rs"), json!(2))
    );
    let reqs = jev.requests();
    assert_eq!(reqs.len(), 1, "one request for the rule, not one per file");
    let files = reqs[0].body["state"]["files"].as_object().unwrap();
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        ["README.md", "src/cli.rs"]
    );
    assert_eq!(
        reqs[0].body["questions"]["item:flags_documented:0"]["instructions"]["item"],
        "dry-run"
    );
}

#[test]
fn a_multi_file_pattern_needs_a_capture_group() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nrules:\n  - { name: r, description: \"d\", jev: { unit: multi_file, pattern: 'long' } }\n",
    );
    p.bare()
        .arg("validate")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("needs a capture group"));
}

// ---- jev: { checks, examples } + `llmlint jev-checks` ----------------------

const CHECKED_RULE: &str = "  - name: errors_have_context\n    description: \"Every error carries context.\"\n    require_line_attribution: true\n    jev:\n      checks:\n        - \"An error is propagated with a bare `?`.\"\n        - \"A context message omits the path.\"\n      examples:\n        violations: [\"let t = read(p)?;\"]\n        compliant: [\"let t = read(p).context(p)?;\"]\n";

#[test]
fn checks_and_examples_replace_the_rules_question_and_a_firing_check_fails_it() {
    let p = jev_project(CHECKED_RULE);
    let jev = MockJev::start(&[("errors_have_context", violates_at(2))], &[]);
    let out = json_of(&run_jev(&p, &jev).code(1));
    let v = &out["rules"][0]["violations"][0];
    assert_eq!(
        (v["file"].clone(), v["line"].clone()),
        (json!("src/a.rs"), json!(2))
    );

    let q = jev.requests()[0].body["questions"]
        .as_object()
        .unwrap()
        .clone();
    assert!(!q.contains_key("holds:errors_have_context"), "replaced");
    let c0 = &q["holds:errors_have_context#check0"];
    assert_eq!(
        c0["criteria"]["true"],
        "An error is propagated with a bare `?`."
    );
    assert_eq!(
        c0["instructions"]["question"],
        "Does `content` violate `rule`?"
    );
    assert!(q.contains_key("holds:errors_have_context#check1"));
    let ex = &q["holds:errors_have_context#examples"];
    assert_eq!(
        ex["criteria"]["false"]["examples"],
        json!(["let t = read(p)?;"])
    );
    assert_eq!(
        ex["criteria"]["true"]["examples"],
        json!(["let t = read(p).context(p)?;"])
    );
}

#[test]
fn a_rule_passes_when_no_check_fires() {
    let p = jev_project(CHECKED_RULE);
    let jev = MockJev::start(&[("errors_have_context", holds(0.97))], &[]);
    assert_eq!(
        json_of(&run_jev(&p, &jev).code(0))["rules"][0]["outcome"],
        "pass"
    );
}

#[test]
fn empty_checks_or_one_sided_examples_are_config_errors() {
    let p = Project::new();
    p.write(
        "llmlint.yml",
        "version: 1\nrules:\n  - { name: a, description: d, jev: { checks: [\" \"] } }\n  - { name: b, description: d, jev: { examples: { violations: [x] } } }\n",
    );
    p.bare()
        .arg("validate")
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "rule a: `jev.checks` has an empty check",
        ))
        .stderr(predicates::str::contains(
            "rule b: `jev.examples` needs at least one `violations` and one `compliant` example",
        ));
}

#[test]
fn jev_checks_drafts_checks_for_rules_without_them_and_prints_yaml() {
    let p = jev_project(&format!(
        "{ONE_RULE}  - {{ name: already, description: \"d\", jev: {{ checks: [\"x\"] }} }}\n"
    ));
    p.write(
        "answer.json",
        r#"{"errors_have_context": {"checks": ["A bare `?` propagates an error.", "  "]}}"#,
    );
    let dump = p.path().join("args.txt");
    let a = p
        .bare()
        .args(["jev-checks", "--oneharness-bin"])
        .arg(mock_path())
        .env("LLMLINT_MOCK_STRUCTURED", p.path().join("answer.json"))
        .env("LLMLINT_MOCK_DUMP_ARGS", &dump)
        .assert()
        .code(0);
    assert_eq!(
        String::from_utf8_lossy(&a.get_output().stdout),
        "# rule `errors_have_context` — paste under its `jev:` block\nchecks:\n  - \"A bare `?` propagates an error.\"\n"
    );
    // One read-only harness call whose prompt briefs only the rule without checks.
    let args = std::fs::read_to_string(&dump).unwrap();
    assert!(args.contains("read-only"));
    assert!(args.contains("- name: errors_have_context"));
    assert!(!args.contains("- name: already"));
}

#[test]
fn jev_checks_rejects_an_unknown_rule_before_any_model_call() {
    let p = jev_project(ONE_RULE);
    p.bare()
        .args(["jev-checks", "--rule", "nope", "--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "no rule named nope; available rules: errors_have_context",
        ));
}

#[test]
fn an_unsure_score_is_a_warning_that_is_shown_but_never_fails_the_run() {
    let p = jev_project(ONE_RULE);
    // Between the whole-file unit's fail (0.5) and warn (0.75) cuts.
    let unsure = Script {
        holds: Some(0.6),
        relevant: None,
        line: Some(2),
    };
    let jev = MockJev::start(&[("errors_have_context", unsure)], &[]);
    let out = json_of(&run_jev(&p, &jev).code(0));
    let r = &out["rules"][0];
    assert_eq!(r["outcome"], "warn");
    assert_eq!(
        (
            r["warnings"][0]["file"].clone(),
            r["warnings"][0]["line"].clone()
        ),
        (json!("src/a.rs"), json!(2))
    );
    assert!(r.get("violations").is_none());
    assert_eq!(out["summary"]["warned"], 1);

    let mut c = p.bare();
    c.args(["--oneharness-bin"])
        .arg(p.path().join("no-such-oneharness"))
        .env("LLMLINT_JEV_URL", &jev.url)
        .env("TYPESAFE_API_KEY", "test-key");
    c.assert()
        .code(0)
        .stdout(predicates::str::contains(
            "WARN errors_have_context\n     src/a.rs:2",
        ))
        .stdout(predicates::str::contains(
            "1 rules: 0 passed, 0 failed, 0 skipped, 1 warned",
        ));
}
