//! `llmlint config`: load + validate the merged config and print it, with the
//! ordered list of sources (files and bundled plugins) that contributed and the
//! per-item provenance that traces each rule, agent, and setting back to the
//! source it came from.

use crate::cli::ConfigArgs;
use crate::domain::config::validate;
use crate::errors::{Error, Result};
use crate::io::configfs;

pub fn run(args: ConfigArgs) -> Result<i32> {
    let cwd = match &args.cwd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string()))?,
    };
    let loaded = configfs::load(&args.config, &cwd)?;
    validate(&loaded.config)?;
    let out = serde_json::json!({
        "config_files": loaded.sources,
        "sources": loaded.provenance,
        "config": loaded.config,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&out).map_err(|e| Error::Io(e.to_string()))?
    );
    Ok(0)
}
