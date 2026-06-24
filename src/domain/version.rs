//! Versions for plugin configs: 1 to 3 dot-separated non-negative integers
//! (`1`, `1.1`, `1.1.1`).
//!
//! A config **declares** its own published [`Version`] via the top-level
//! `version` field. A consumer that pulls it in as a plugin can **pin** a
//! desired version with an `@` suffix on the URL (`url@1`, `url@1.1`,
//! `url@1.1.1`); that pin is a [`VersionReq`]. A pin matches by *prefix*: `@1`
//! matches any `1.x.y`, `@1.2` matches any `1.2.x`, and `@1.2.3` matches exactly
//! `1.2.3`. The pin is therefore both an assertion (the fetched config must
//! satisfy it) and the cache key (see [`crate::io::plugins`]).

use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// A declared config version: 1–3 numeric components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(Vec<u64>);

/// A requested version pin (the `@` suffix on a plugin URL): 1–3 numeric
/// components, matched against a [`Version`] by prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq(Vec<u64>);

/// Parse `1`, `1.2`, or `1.2.3` into its components, rejecting anything with
/// more than three parts or a non-integer part.
fn parse_components(s: &str) -> Result<Vec<u64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("version is empty".to_string());
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 3 {
        return Err(format!(
            "version {s:?} has too many components (expected 1 to 3, like 1, 1.2, or 1.2.3)"
        ));
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let n: u64 = p
            .parse()
            .map_err(|_| format!("version {s:?} component {p:?} is not a non-negative integer"))?;
        out.push(n);
    }
    Ok(out)
}

fn join(components: &[u64]) -> String {
    components
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

impl Version {
    /// Parse a version from its textual form.
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(Version(parse_components(s)?))
    }

    /// The numeric components, most-significant first.
    pub fn components(&self) -> &[u64] {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&join(&self.0))
    }
}

impl VersionReq {
    /// Parse a version pin from its textual form.
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(VersionReq(parse_components(s)?))
    }

    /// Whether `version` satisfies this pin: the pin's components are a prefix
    /// of the version's, so the version must be at least as specific. `@1`
    /// accepts `1`, `1.4`, `1.4.2`; `@1.4.2` accepts only `1.4.2`.
    pub fn matches(&self, version: &Version) -> bool {
        version.0.len() >= self.0.len() && version.0[..self.0.len()] == self.0[..]
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&join(&self.0))
    }
}

// A version serializes as a string ("1.2.3") and deserializes from a YAML
// integer (`1`), float (`1.2`), or string (`"1.2.3"`) — so `version: 1`,
// `version: 1.2`, and `version: "1.2.3"` all work in a config.
impl Serialize for Version {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_yaml_ng::Value::deserialize(d)?;
        let text = match &value {
            serde_yaml_ng::Value::Number(n) => n.to_string(),
            serde_yaml_ng::Value::String(s) => s.clone(),
            other => {
                return Err(de::Error::custom(format!(
                    "version must be a number or string like 1, 1.2, or \"1.2.3\"; got {other:?}"
                )))
            }
        };
        Version::parse(&text).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_to_three_components() {
        assert_eq!(Version::parse("1").unwrap().components(), &[1]);
        assert_eq!(Version::parse("1.2").unwrap().components(), &[1, 2]);
        assert_eq!(Version::parse(" 1.2.3 ").unwrap().components(), &[1, 2, 3]);
    }

    #[test]
    fn rejects_malformed_versions() {
        assert!(Version::parse("").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
        assert!(Version::parse("1.x").is_err());
        assert!(Version::parse("-1").is_err());
    }

    #[test]
    fn display_roundtrips() {
        assert_eq!(Version::parse("1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(VersionReq::parse("1.2").unwrap().to_string(), "1.2");
    }

    #[test]
    fn pin_matches_by_prefix() {
        let req = VersionReq::parse("1").unwrap();
        assert!(req.matches(&Version::parse("1").unwrap()));
        assert!(req.matches(&Version::parse("1.4").unwrap()));
        assert!(req.matches(&Version::parse("1.4.2").unwrap()));
        assert!(!req.matches(&Version::parse("2.0").unwrap()));

        let req = VersionReq::parse("1.4.2").unwrap();
        assert!(req.matches(&Version::parse("1.4.2").unwrap()));
        // A pin more specific than the declared version cannot be satisfied.
        assert!(!req.matches(&Version::parse("1.4").unwrap()));
        assert!(!req.matches(&Version::parse("1.4.3").unwrap()));
    }

    #[test]
    fn deserializes_from_int_float_and_string() {
        let v: Version = serde_yaml_ng::from_str("1").unwrap();
        assert_eq!(v.to_string(), "1");
        let v: Version = serde_yaml_ng::from_str("1.2").unwrap();
        assert_eq!(v.to_string(), "1.2");
        let v: Version = serde_yaml_ng::from_str("\"1.2.3\"").unwrap();
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn rejects_non_scalar_version() {
        assert!(serde_yaml_ng::from_str::<Version>("[1, 2]").is_err());
    }

    #[test]
    fn serializes_as_string() {
        let v = Version::parse("1.2").unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"1.2\"");
    }
}
