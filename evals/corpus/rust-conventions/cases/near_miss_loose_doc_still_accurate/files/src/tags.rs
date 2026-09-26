use std::collections::BTreeSet;

/// Splits a comma list into tags. Blank bits are skipped, and whitespace
/// around each tag is dropped.
pub fn split_tags(list: &str) -> Vec<&str> {
    list.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// Same as `split_tags`, but lowercased and with repeats removed. Comes back
/// sorted, since it's a set under the hood.
pub fn normalized_tags(list: &str) -> Vec<String> {
    let unique: BTreeSet<String> = split_tags(list)
        .into_iter()
        .map(str::to_lowercase)
        .collect();
    unique.into_iter().collect()
}

/// Glue tags back together with `", "`. Empty in, empty out.
pub fn join_tags(tags: &[String]) -> String {
    tags.join(", ")
}

/// True if any tag matches `wanted`, ignoring case.
pub fn has_tag(list: &str, wanted: &str) -> bool {
    split_tags(list)
        .iter()
        .any(|tag| tag.to_lowercase() == wanted.to_lowercase())
}
