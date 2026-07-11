//! Pure domain logic: no process, filesystem, env, or clock I/O lives here.
//!
//! Config modeling + validation, prompt-template rendering, JSON-Schema
//! generation, judge/batch planning, majority-vote aggregation, and output
//! formatting. All I/O (config discovery, globbing, the oneharness subprocess)
//! lives in [`crate::io`].

pub mod applicability;
pub mod attribution;
pub mod config;
pub mod config_schema;
pub mod cost;
pub mod diffmodel;
pub mod ignore;
pub mod plan;
pub mod report;
pub mod schema;
pub mod template;
pub mod verdict;
pub mod version;
pub mod versionbump;
pub mod vote;

/// Render a (relative) path with forward slashes so the prompt the judge sees —
/// and the violation paths it echoes back — are consistent across platforms (a
/// Windows `PathBuf` would otherwise render `\`). Pure string formatting; it
/// lives here so both the planner (per-rule file lists) and the io/command layer
/// (the run's file union) spell a path the *same* way, which the per-file
/// applicability matching relies on.
pub fn to_slash(path: &std::path::Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
