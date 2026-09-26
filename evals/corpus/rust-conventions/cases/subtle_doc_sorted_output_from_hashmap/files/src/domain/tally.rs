use std::collections::HashMap;

/// Count how many times each tag appears in `tags`.
pub fn tag_counts<'a>(tags: &[&'a str]) -> HashMap<&'a str, usize> {
    let mut counts = HashMap::new();
    for tag in tags {
        *counts.entry(*tag).or_insert(0) += 1;
    }
    counts
}

/// The distinct tags that appear at least `minimum` times, sorted
/// alphabetically so the output is stable across runs.
pub fn frequent_tags<'a>(tags: &[&'a str], minimum: usize) -> Vec<&'a str> {
    tag_counts(tags)
        .into_iter()
        .filter(|(_, count)| *count >= minimum)
        .map(|(tag, _)| tag)
        .collect()
}
