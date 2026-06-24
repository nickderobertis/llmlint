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

    #[error("config already exists at {0}; pass --force to overwrite")]
    ConfigExists(PathBuf),

    #[error(
        "oneharness not found ({0}); install it \
         (https://github.com/nickderobertis/oneharness) or pass --oneharness-bin"
    )]
    OneharnessNotFound(String),

    #[error("oneharness run failed: {0}")]
    Oneharness(String),

    #[error("{0}")]
    Io(String),

    #[error("template error: {0}")]
    Template(String),
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
            message: "curl exited 22".into()
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
        assert!(Error::ConfigExists("/tmp/llmlint.yml".into())
            .to_string()
            .contains("already exists"));
        assert!(Error::Template("bad".into())
            .to_string()
            .contains("template error"));
        assert!(Error::ConfigParse {
            path: "f".into(),
            message: "m".into()
        }
        .to_string()
        .contains("f: m"));
    }
}
