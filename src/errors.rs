//! Error type for failures that prevent llmlint from completing a lint run.
//!
//! These all map to process exit code `2` (usage / configuration / environment
//! faults). A lint that completes but finds violations is **not** an error — it
//! is returned as data and maps to exit code `1` (see [`crate::commands`]).

use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "no llmlint config found (looked for {names} from {dir} upward); \
         run `llmlint init` to create one"
    )]
    ConfigNotFound { names: String, dir: String },

    #[error("config {path}: {message}")]
    ConfigParse { path: String, message: String },

    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("{0}")]
    UnknownFilter(String),

    #[error("invalid plugin spec: {0}")]
    PluginSpec(String),

    #[error("plugin {url}: {message}")]
    PluginFetch { url: String, message: String },

    #[error(
        "plugin {url}: requested version {requested} but the config declares \
         version {declared}"
    )]
    PluginVersionMismatch {
        url: String,
        requested: String,
        declared: String,
    },

    #[error(
        "plugin {url}: requested version {requested} but the config declares \
         no version (add a top-level `version:` to the plugin config)"
    )]
    PluginMissingVersion { url: String, requested: String },

    #[error(
        "plugins nested too deep (exceeded the max depth of {max}); a config's \
         `plugins` pull in further configs transitively — check for an \
         unintended cycle or flatten the include graph"
    )]
    PluginDepthExceeded { max: usize },

    #[error("config already exists at {0}; pass --force to overwrite")]
    ConfigExists(PathBuf),

    #[error(
        "oneharness not found ({0}); install it \
         (https://github.com/nickderobertis/oneharness) or pass --oneharness-bin"
    )]
    OneharnessNotFound(String),

    #[error(
        "oneharness {found} is too old; llmlint requires oneharness >= {required} \
         for read-only mode (the agent reads but never edits files). Upgrade \
         oneharness (https://github.com/nickderobertis/oneharness)."
    )]
    OneharnessTooOld { found: String, required: String },

    #[error("oneharness run failed: {0}")]
    Oneharness(String),

    #[error("diff ({backend}): {message}")]
    Diff { backend: String, message: String },

    #[error("{0}")]
    Io(String),

    #[error("template error: {0}")]
    Template(String),

    #[error(
        "invalid `llmlint: ignore` directive(s):\n{0}\n\
         each must name specific configured rule(s) and a reason, e.g. \
         `// llmlint: ignore[rule_name] why it is safe here`"
    )]
    IgnoreDirective(String),

    #[error("{0}")]
    ConfigPathNotFound(String),

    #[error("{0}")]
    History(String),
}

impl Error {
    /// Process exit code for this error. All current variants are
    /// usage/configuration/environment faults, which exit `2`.
    pub fn exit_code(&self) -> i32 {
        2
    }
}

/// Wrap an [`std::io::Error`] with a human-readable context string.
pub fn io_err(context: impl Into<String>, e: std::io::Error) -> Error {
    Error::Io(format!("{}: {e}", context.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_variants_exit_two() {
        assert_eq!(Error::InvalidConfig("x".into()).exit_code(), 2);
        assert_eq!(Error::Oneharness("y".into()).exit_code(), 2);
    }

    #[test]
    fn io_err_includes_context_and_cause() {
        let e = io_err(
            "writing file",
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        let msg = e.to_string();
        assert!(msg.contains("writing file"));
        assert!(msg.contains("denied"));
    }

    #[test]
    fn display_messages_are_actionable() {
        assert!(Error::OneharnessNotFound("oneharness".into())
            .to_string()
            .contains("not found"));
        let too_old = Error::OneharnessTooOld {
            found: "oneharness 0.2.529 (mock)".into(),
            required: "0.3.0".into(),
        }
        .to_string();
        assert!(too_old.contains("too old"), "got: {too_old}");
        assert!(too_old.contains("0.3.0"), "got: {too_old}");
        assert!(too_old.contains("read-only mode"), "got: {too_old}");
        assert!(Error::ConfigNotFound {
            names: "llmlint.yml".into(),
            dir: "/x".into()
        }
        .to_string()
        .contains("no llmlint config"));
        assert!(Error::PluginSpec("bad".into())
            .to_string()
            .contains("invalid plugin spec"));
        assert!(Error::PluginFetch {
            url: "https://x/p.yml".into(),
            message: "connection refused".into()
        }
        .to_string()
        .contains("https://x/p.yml"));
        assert!(Error::PluginVersionMismatch {
            url: "u".into(),
            requested: "1".into(),
            declared: "2".into()
        }
        .to_string()
        .contains("requested version 1 but the config declares version 2"));
        assert!(Error::PluginMissingVersion {
            url: "u".into(),
            requested: "1".into()
        }
        .to_string()
        .contains("declares no version"));
        assert!(Error::PluginDepthExceeded { max: 100 }
            .to_string()
            .contains("max depth of 100"));
        assert!(Error::ConfigExists("/tmp/llmlint.yml".into())
            .to_string()
            .contains("already exists"));
        assert!(Error::Template("bad".into())
            .to_string()
            .contains("template error"));
        assert!(Error::IgnoreDirective("src/a.rs:3: no reason".into())
            .to_string()
            .contains("invalid `llmlint: ignore` directive"));
        assert!(Error::UnknownFilter("no rule named \"x\"".into())
            .to_string()
            .contains("no rule named"));
        assert!(
            Error::ConfigPathNotFound("unknown config path \"x\"".into())
                .to_string()
                .contains("unknown config path")
        );
        assert!(Error::History("no run with id \"x\"".into())
            .to_string()
            .contains("no run with id"));
        assert!(Error::ConfigParse {
            path: "f".into(),
            message: "m".into()
        }
        .to_string()
        .contains("f: m"));
        assert!(Error::Diff {
            backend: "git".into(),
            message: "not a git repository".into()
        }
        .to_string()
        .contains("diff (git): not a git repository"));
    }
}
