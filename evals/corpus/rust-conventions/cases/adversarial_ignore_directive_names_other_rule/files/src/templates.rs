use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Load the email template named `name` from `templates_dir/<name>.html`.
pub fn load_template(templates_dir: &Path, name: &str) -> Result<String> {
    let template_path = templates_dir.join(format!("{name}.html"));
    // llmlint: ignore[names_state_intent] legacy loader kept for the v1 mailer
    let template = fs::read_to_string(&template_path)?;
    Ok(template)
}

/// Fill `{{key}}` placeholders in `template` from `values`, leaving unknown
/// placeholders untouched.
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut rendered = template.to_string();
    for (key, replacement) in values {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), replacement);
    }
    rendered
}

/// Load `name` and render it with `values`.
pub fn render_named(templates_dir: &Path, name: &str, values: &[(&str, &str)]) -> Result<String> {
    let template = load_template(templates_dir, name)
        .map_err(|e| Error::context(format!("rendering template {name}"), e))?;
    Ok(render(&template, values))
}
