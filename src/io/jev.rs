//! HTTP client for TypeSafe's Jev API (`POST /v1/systemone`). The question
//! building and answer reading are pure ([`crate::domain::jev`]); this module
//! only sends a request and validates what comes back.
//!
//! **Auth.** The API key is read from `TYPESAFE_API_KEY` and nowhere else — a
//! secret never belongs in a config file that gets committed. It is required
//! only when some rule is actually judged by `jev`.
//!
//! **Retries.** `429 Too Many Requests`, `529 Overloaded`, other `5xx`, and
//! transport failures retry with exponential backoff, honoring `retry-after`
//! when the server sends it (Jev's rate limits move while TypeSafe scales). Any
//! other status — a bad key, a malformed request — fails at once with the
//! server's own message, since retrying cannot fix it.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::domain::config::JevCfg;
use crate::domain::jev::DEFAULT_MODEL;
use crate::errors::{Error, Result};

/// The public API base; `jev.url` / `LLMLINT_JEV_URL` override it.
pub const DEFAULT_URL: &str = "https://api.typesafe.ai";

/// The only place the API key is read from.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Attempts per request before a retryable failure becomes an error.
const MAX_ATTEMPTS: u32 = 5;

/// Longest single backoff sleep, so a stuck limit fails in bounded time.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Token usage for one or more requests. Jev bills input tokens only.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, o: Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
    }
}

/// One validated response.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The versioned model that actually answered (an alias resolves here).
    pub model: String,
    pub answers: Map<String, Value>,
    pub usage: Usage,
}

pub struct Client {
    endpoint: String,
    key: String,
    model: String,
    agent: ureq::Agent,
    backoff_base: Duration,
}

impl Client {
    /// Build a client from the effective `jev` settings and the key from the
    /// environment. A missing key is an exit-2 error that says where to put it.
    pub fn from_env(cfg: &JevCfg, timeout_secs: u64) -> Result<Client> {
        let key = std::env::var(API_KEY_ENV)
            .ok()
            .filter(|k| !k.trim().is_empty());
        Client::new(cfg, key, timeout_secs, Duration::from_millis(500))
    }

    fn new(
        cfg: &JevCfg,
        key: Option<String>,
        timeout_secs: u64,
        backoff_base: Duration,
    ) -> Result<Client> {
        let key = key.ok_or(Error::JevAuth)?;
        let base = cfg
            .url
            .as_deref()
            .unwrap_or(DEFAULT_URL)
            .trim_end_matches('/');
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(Error::Jev(format!(
                "jev.url must be an http(s) URL, got {base:?}"
            )));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(ureq::Proxy::try_from_env())
            // Statuses are classified below (retry vs fail), so they must not
            // become transport errors.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(timeout_secs.max(1))))
            .build()
            .into();
        Ok(Client {
            endpoint: format!("{base}/v1/systemone"),
            key,
            model: cfg
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            agent,
            backoff_base,
        })
    }

    /// The model id requests are sent with (a price is looked up by it).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Ask `questions` about `state`, retrying transient failures.
    pub fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Answer> {
        let body = serde_json::to_vec(
            &json!({ "state": state, "model": self.model, "questions": questions }),
        )
        .map_err(|e| Error::Jev(format!("encoding request: {e}")))?;
        let mut last = String::new();
        for attempt in 0..MAX_ATTEMPTS {
            let sent = self
                .agent
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.key))
                .header("Content-Type", "application/json")
                .send(&body[..]);
            let wait = match sent {
                Err(e) => {
                    last = format!("request to {} failed: {e}", self.endpoint);
                    None
                }
                Ok(mut resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(Duration::from_secs);
                    let text = resp.body_mut().read_to_string().unwrap_or_default();
                    match status {
                        200..=299 => return parse(&text),
                        401 | 403 => {
                            return Err(Error::Jev(format!(
                                "authentication failed (HTTP {status}); check {API_KEY_ENV}: {}",
                                snippet(&text)
                            )))
                        }
                        429 | 529 | 500..=599 => {
                            last = format!("HTTP {status}: {}", snippet(&text));
                            retry_after
                        }
                        _ => {
                            return Err(Error::Jev(format!(
                                "request rejected (HTTP {status}): {}",
                                snippet(&text)
                            )))
                        }
                    }
                }
            };
            if attempt + 1 < MAX_ATTEMPTS {
                let backoff = self.backoff_base.saturating_mul(1 << attempt);
                std::thread::sleep(wait.unwrap_or(backoff).min(MAX_BACKOFF));
            }
        }
        Err(Error::Jev(format!(
            "gave up after {MAX_ATTEMPTS} attempts: {last}"
        )))
    }
}

/// Validate a 2xx body into an [`Answer`]. The shape is the API's contract;
/// anything else is an error rather than a guessed verdict.
fn parse(text: &str) -> Result<Answer> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| Error::Jev(format!("unparseable response ({e}): {}", snippet(text))))?;
    let answers = v
        .get("answers")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| Error::Jev(format!("response has no `answers` map: {}", snippet(text))))?;
    let usage = v.get("usage");
    let tokens = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Ok(Answer {
        model: v
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        answers,
        usage: Usage {
            input_tokens: tokens("input_tokens"),
            output_tokens: tokens("output_tokens"),
        },
    })
}

/// A bounded excerpt of a response body for an error message.
fn snippet(text: &str) -> String {
    const MAX: usize = 300;
    let t = text.trim();
    match t.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A one-connection-at-a-time HTTP server replaying `(status, headers, body)`
    /// responses in order and recording each request body.
    fn serve(responses: Vec<(u16, &'static str, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut req = vec![0; len];
                reader.read_exact(&mut req).unwrap();
                log.lock().unwrap().push(String::from_utf8(req).unwrap());
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (url, seen)
    }

    fn client(url: &str) -> Client {
        let cfg = JevCfg {
            url: Some(url.into()),
            ..JevCfg::default()
        };
        Client::new(&cfg, Some("k".into()), 5, Duration::from_millis(1)).unwrap()
    }

    const OK: &str = r#"{"model":"jev-1.13.0","answers":{"holds:r":{"type":"noul","noul":0.9}},"usage":{"input_tokens":120,"output_tokens":3}}"#;

    #[test]
    fn a_good_response_parses_and_the_request_carries_model_state_and_questions() {
        let (url, seen) = serve(vec![(200, "", OK.into())]);
        let mut q = Map::new();
        q.insert(
            "holds:r".into(),
            json!({"type": "noul", "instructions": "x"}),
        );
        let a = client(&url).ask(&json!({"content": "L1| x"}), &q).unwrap();
        assert_eq!(a.model, "jev-1.13.0");
        assert_eq!(a.answers["holds:r"]["noul"], 0.9);
        assert_eq!(
            a.usage,
            Usage {
                input_tokens: 120,
                output_tokens: 3
            }
        );
        let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
        assert_eq!(sent["model"], DEFAULT_MODEL);
        assert_eq!(sent["state"]["content"], "L1| x");
        assert_eq!(sent["questions"]["holds:r"]["type"], "noul");
    }

    #[test]
    fn rate_limits_and_overload_retry_then_succeed() {
        let (url, seen) = serve(vec![
            (429, "retry-after: 0\r\n", "slow down".into()),
            (529, "", "overloaded".into()),
            (200, "", OK.into()),
        ]);
        client(&url).ask(&json!("s"), &Map::new()).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn persistent_overload_gives_up_with_the_last_status() {
        let (url, _) = serve(
            (0..MAX_ATTEMPTS)
                .map(|_| (503, "", "down".into()))
                .collect(),
        );
        let e = client(&url)
            .ask(&json!("s"), &Map::new())
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("gave up after 5 attempts") && e.contains("HTTP 503"),
            "{e}"
        );
    }

    #[test]
    fn auth_and_validation_failures_do_not_retry() {
        let (url, seen) = serve(vec![(401, "", "bad key".into())]);
        let e = client(&url)
            .ask(&json!("s"), &Map::new())
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("authentication failed (HTTP 401)") && e.contains(API_KEY_ENV),
            "{e}"
        );
        assert_eq!(seen.lock().unwrap().len(), 1);

        let (url, _) = serve(vec![(422, "", r#"{"detail":"state too long"}"#.into())]);
        let e = client(&url)
            .ask(&json!("s"), &Map::new())
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("HTTP 422") && e.contains("state too long"),
            "{e}"
        );
    }

    #[test]
    fn a_malformed_body_is_an_error() {
        for (body, want) in [
            ("not json", "unparseable"),
            (r#"{"model":"m"}"#, "no `answers`"),
        ] {
            let (url, _) = serve(vec![(200, "", body.into())]);
            let e = client(&url)
                .ask(&json!("s"), &Map::new())
                .unwrap_err()
                .to_string();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn a_missing_key_or_bad_url_is_a_config_error() {
        let cfg = JevCfg::default();
        let e = Client::new(&cfg, None, 5, Duration::ZERO).err().unwrap();
        assert!(e.to_string().contains(API_KEY_ENV), "{e}");
        let bad = JevCfg {
            url: Some("ftp://x".into()),
            ..JevCfg::default()
        };
        let e = Client::new(&bad, Some("k".into()), 5, Duration::ZERO)
            .err()
            .unwrap();
        assert!(e.to_string().contains("http(s) URL"), "{e}");
    }

    #[test]
    fn long_error_bodies_are_truncated() {
        let s = snippet(&"x".repeat(1000));
        assert!(s.ends_with('…') && s.chars().count() == 301);
    }
}
