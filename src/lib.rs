//! llmlint — an LLM-as-judge linter for code-quality checks that deterministic
//! linters can't express.
//!
//! Architecture (see `AGENTS.md`): [`domain`] is pure logic (config model +
//! validation, template rendering, schema generation, judge planning, vote
//! aggregation, reporting); [`io`] owns all I/O (config discovery + includes,
//! file globbing, the `oneharness` subprocess); [`commands`] wires them to the
//! [`cli`].

pub mod cli;
pub mod commands;
pub mod domain;
pub mod errors;
pub mod io;
