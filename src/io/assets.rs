//! Compile-time-embedded assets: the default prompt template, the bundled
//! config-lint plugin, and the starter config written by `llmlint init`.

/// The built-in master prompt template (minijinja). Used unless a config sets
/// `prompt_template`.
pub const DEFAULT_TEMPLATE: &str = include_str!("../../assets/default_template.md");

/// The bundled `llmlint:config-lint` plugin: rules that lint llmlint config
/// files themselves.
pub const CONFIG_LINT_PLUGIN: &str = include_str!("../../assets/config_lint.yml");

/// Starter config body written by `llmlint init`.
pub const INIT_CONFIG: &str = include_str!("../../assets/init.llmlint.yml");

/// Resolve a bundled plugin id (e.g. `llmlint:config-lint`) to its embedded
/// YAML, or `None` if unknown.
pub fn bundled(id: &str) -> Option<&'static str> {
    match id {
        "llmlint:config-lint" => Some(CONFIG_LINT_PLUGIN),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_plugin_resolves() {
        assert!(bundled("llmlint:config-lint").is_some());
        assert!(bundled("llmlint:nope").is_none());
    }

    #[test]
    fn embedded_assets_are_non_empty() {
        assert!(DEFAULT_TEMPLATE.contains("{% for r in rules %}"));
        assert!(CONFIG_LINT_PLUGIN.contains("name_matches_description"));
        assert!(INIT_CONFIG.contains("llmlint:config-lint"));
    }
}
