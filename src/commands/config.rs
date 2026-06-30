//! `llmlint config`: load + validate the merged config and print it as JSON,
//! with the ordered list of sources (files and bundled plugins) that
//! contributed. With `--sources`, also emit the per-item provenance that traces
//! each rule, agent, and setting back to the file to edit it — the broad way to
//! discover where something is defined (`llmlint where` is the per-item lookup).

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
    // Insertion order is preserved (serde_json `preserve_order`), so `sources`
    // sits between the file list and the config, where it reads naturally.
    let mut obj = serde_json::Map::new();
    obj.insert(
        "config_files".into(),
        serde_json::to_value(&loaded.sources).map_err(|e| Error::Io(e.to_string()))?,
    );
    if args.sources {
        obj.insert(
            "sources".into(),
            serde_json::to_value(&loaded.provenance).map_err(|e| Error::Io(e.to_string()))?,
        );
    }
    obj.insert(
        "config".into(),
        serde_json::to_value(&loaded.config).map_err(|e| Error::Io(e.to_string()))?,
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(obj))
            .map_err(|e| Error::Io(e.to_string()))?
    );
    Ok(0)
}
