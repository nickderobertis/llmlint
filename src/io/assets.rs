//! Compile-time-embedded assets: the default prompt template, the bundled
//! config-lint plugin, and the starter config written by `llmlint init`.

/// The built-in master prompt template (minijinja). Used unless a config sets
/// `prompt_template`.
pub const DEFAULT_TEMPLATE: &str = include_str!("../../assets/default_template.md");

/// The bundled config-lint plugin: rules that lint llmlint config files
/// themselves. Referenced from configs by [`CONFIG_LINT_URL`].
pub const CONFIG_LINT_PLUGIN: &str = include_str!("../../assets/config_lint.yml");

/// Canonical URL of the bundled config-lint plugin. It is a normal plugin URL
/// (no special scheme), but resolves offline from the embedded copy above, so
/// the default config works with no network and `llmlint init` stays usable
/// disconnected. Pin a version with `@` like any other plugin (`…@1`).
pub const CONFIG_LINT_URL: &str =
    "https://raw.githubusercontent.com/nickderobertis/llmlint/main/assets/config_lint.yml";

/// Starter config body written by `llmlint init`.
pub const INIT_CONFIG: &str = include_str!("../../assets/init.llmlint.yml");

/// The public JSON Schema for an llmlint config file. Bundled here so it ships
/// in the crate and is pinned by a test to the generator in
/// [`crate::domain::config_schema`]; published at
/// [`crate::domain::config_schema::SCHEMA_URL`] for the `$schema` modeline that
/// `llmlint init` writes.
pub const CONFIG_SCHEMA: &str = include_str!("../../assets/llmlint.schema.json");

/// If `url` (without any `@version` suffix) names a plugin bundled into the
/// binary, return its embedded YAML. Bundled plugins resolve offline — no
/// network, no cache — so the shipped default config always works.
/// The version pin, if any, is still validated by the caller against the
/// embedded config's declared `version`.
pub fn bundled_url(url: &str) -> Option<&'static str> {
    if url == CONFIG_LINT_URL {
        Some(CONFIG_LINT_PLUGIN)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_url_resolves_config_lint() {
        assert_eq!(bundled_url(CONFIG_LINT_URL), Some(CONFIG_LINT_PLUGIN));
        assert!(bundled_url("https://example.com/other.yml").is_none());
    }

    #[test]
    fn embedded_assets_are_non_empty() {
        assert!(DEFAULT_TEMPLATE.contains("{% for r in rules %}"));
        assert!(CONFIG_LINT_PLUGIN.contains("name_describes_what_the_rule_checks"));
        // The starter config references the bundled plugin by URL.
        assert!(INIT_CONFIG.contains("config_lint.yml"));
        assert!(INIT_CONFIG.contains("plugins:"));
        // The bundled config schema is valid JSON with the published `$id`.
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        assert_eq!(schema["$id"], crate::domain::config_schema::SCHEMA_URL);
    }
}
