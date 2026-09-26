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

use super::{git, init_repo, mock_path, Project};

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
    for (id, q) in request["questions"].as_object().unwrap() {
        let mut parts = id.split(':');
        let (kind, rule) = (parts.next().unwrap(), parts.next().unwrap());
        let s = scripts.get(rule).copied().unwrap_or_default();
        let a = match kind {
            "holds" => json!({"type": "noul", "noul": s.holds.unwrap_or(0.95)}),
            "relevant" => json!({"type": "noul", "noul": s.relevant.unwrap_or(0.9)}),
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
        .stderr(predicates::str::contains("`harness` or `jev`"));
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
