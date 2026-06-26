//! Pure domain logic: no process, filesystem, env, or clock I/O lives here.
//!
//! Config modeling + validation, prompt-template rendering, JSON-Schema
//! generation, judge/batch planning, majority-vote aggregation, and output
//! formatting. All I/O (config discovery, globbing, the oneharness subprocess)
//! lives in [`crate::io`].

pub mod config;
pub mod config_schema;
pub mod ignore;
pub mod plan;
pub mod report;
pub mod schema;
pub mod template;
pub mod verdict;
pub mod version;
pub mod vote;
