use std::path::{Path, PathBuf};

/// One report to export: its name and rendered body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub body: String,
}

/// A file write the I/O layer should perform. Planning a write never touches
/// the disk; `io::export::apply` carries it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Create (or replace) `path` with `contents`.
    Write { path: PathBuf, contents: String },
    /// Skip `path` because the report body is empty.
    Skip { path: PathBuf },
}

/// Whether `path` names a Markdown file, judged purely by its extension
/// (case-insensitive). The file does not need to exist.
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

/// The output path for `report` under `out_dir`: `<out_dir>/<name>.md`.
pub fn report_path(out_dir: &Path, report: &Report) -> PathBuf {
    out_dir.join(format!("{}.md", report.name))
}

/// Plan one write per report, in input order. A report with an empty body is
/// planned as a `Skip` so the caller can log it.
pub fn plan_export(out_dir: &Path, reports: &[Report]) -> Vec<WriteOp> {
    reports
        .iter()
        .map(|report| {
            let path = report_path(out_dir, report);
            if report.body.trim().is_empty() {
                WriteOp::Skip { path }
            } else {
                WriteOp::Write { path, contents: report.body.clone() }
            }
        })
        .collect()
}
