//! `llmlint init`: write a starter config (with the config-lint plugin enabled),
//! optionally embedding the default prompt template for customization.

use std::path::PathBuf;

use crate::cli::InitArgs;
use crate::domain::config_schema;
use crate::errors::{io_err, Error, Result};
use crate::io::assets;

pub fn run(args: InitArgs) -> Result<i32> {
    let path = target_path(&args)?;
    if path.exists() && !args.force {
        return Err(Error::ConfigExists(path));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| io_err(format!("creating {}", parent.display()), e))?;
        }
    }
    let content = render(args.with_template);
    std::fs::write(&path, content).map_err(|e| io_err(format!("writing {}", path.display()), e))?;
    println!("wrote {}", path.display());
    Ok(0)
}

fn target_path(args: &InitArgs) -> Result<PathBuf> {
    if let Some(out) = &args.output {
        return Ok(out.clone());
    }
    if args.global {
        return Ok(global_config_dir()?.join("llmlint.yml"));
    }
    Ok(PathBuf::from("llmlint.yml"))
}

fn render(with_template: bool) -> String {
    // The `$schema` modeline must lead the file so the YAML language server
    // picks it up; editors then complete and validate against the published
    // schema.
    let mut s = config_schema::modeline();
    if !with_template {
        s.push_str(assets::INIT_CONFIG);
        return s;
    }
    s.push_str(
        "# Master prompt template (customize freely). The judge renders this with\n\
         # `rules` (each with name + description) and `files` (the target paths).\n\
         prompt_template: |\n",
    );
    for line in assets::DEFAULT_TEMPLATE.lines() {
        s.push_str("  ");
        s.push_str(line);
        s.push('\n');
    }
    s.push('\n');
    s.push_str(assets::INIT_CONFIG);
    s
}

fn global_config_dir() -> Result<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("llmlint"));
        }
    }
    #[cfg(windows)]
    if let Ok(a) = std::env::var("APPDATA") {
        if !a.is_empty() {
            return Ok(PathBuf::from(a).join("llmlint"));
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return Ok(PathBuf::from(h).join(".config").join("llmlint"));
        }
    }
    Err(Error::Io(
        "could not determine a home/config directory for --global".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::configfs;

    #[test]
    fn plain_init_is_the_starter_config() {
        let out = render(false);
        assert!(out.contains("plugins:"));
        assert!(out.contains("config_lint.yml@1"));
        assert!(!out.contains("prompt_template: |"));
        // The schema modeline leads the file so the YAML language server uses it.
        assert!(out.starts_with("# yaml-language-server: $schema="));
        assert!(out.contains(config_schema::SCHEMA_URL));
        // The modeline is a comment, so the body still parses.
        assert!(configfs::parse(&out, "generated").is_ok());
    }

    #[test]
    fn with_template_embeds_a_parseable_prompt_template() {
        let out = render(true);
        assert!(out.starts_with("# yaml-language-server: $schema="));
        assert!(out.contains("prompt_template: |"));
        // The generated config must still parse and carry the template.
        let cfg = configfs::parse(&out, "generated").unwrap();
        assert!(cfg.prompt_template.is_some());
        assert!(cfg
            .prompt_template
            .as_deref()
            .unwrap()
            .contains("{% for r in rules %}"));
        assert!(cfg
            .rules
            .iter()
            .any(|r| r.name == "public_items_are_documented"));
    }
}
