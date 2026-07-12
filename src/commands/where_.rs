//! `llmlint where <path>`: resolve one config item to the file (or plugin URL)
//! it comes from and print that source, nothing else, so it composes in scripts
//! (e.g. `$EDITOR "$(llmlint where rules.no_secrets.judges)"`). The whole map is
//! `llmlint config --sources`; this is the focused, single-item lookup.

use crate::cli::WhereArgs;
use crate::domain::config::{resolve_source, validate};
use crate::errors::{Error, Result};
use crate::io::{configfs, env};

pub fn run(args: WhereArgs) -> Result<i32> {
    let cwd = match &args.cwd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error::Io(e.to_string()))?,
    };
    let loaded = configfs::load(&args.config, &cwd)?;
    let mut config = loaded.config;
    let mut provenance = loaded.provenance;
    // Fold env overrides in so `where <setting>` can report a value that came
    // from an `LLMLINT_*` variable as `env:<VAR>`, matching `config --sources`.
    env::apply_overrides_prov(&mut config, &mut provenance)?;
    validate(&config)?;
    match resolve_source(&provenance, &args.path) {
        Ok(source) => {
            println!("{source}");
            Ok(0)
        }
        Err(message) => Err(Error::ConfigPathNotFound(message)),
    }
}
