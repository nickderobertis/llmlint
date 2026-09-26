use crate::util::{clamp, list_entries, FsPath};

/// An image asset and its declared width in pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub width: i64,
}

/// The display width for `asset` in a column `column_width` pixels wide:
/// the asset width clamped to at least 16 pixels and at most the larger of the
/// column width and 16.
pub fn display_width(asset: &Asset, column_width: i64) -> i64 {
    clamp(asset.width, 16, column_width.max(16))
}

/// Names of the `.png` assets available under `assets_dir`, sorted.
pub fn available_pngs(assets_dir: &FsPath) -> Vec<String> {
    let mut names: Vec<String> = list_entries(assets_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".png"))
        .collect();
    names.sort();
    names
}
