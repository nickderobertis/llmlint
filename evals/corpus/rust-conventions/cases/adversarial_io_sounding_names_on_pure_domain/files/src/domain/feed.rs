/// One entry of a product feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    pub sku: String,
    pub title: String,
    pub price_cents: u64,
}

/// "Read" a feed: parse already-fetched `feed_text` (tab-separated
/// `sku<TAB>title<TAB>price_cents` rows). Rows that do not parse are skipped.
/// The caller does the fetching; this only parses the text it is given.
pub fn read_feed(feed_text: &str) -> Vec<FeedEntry> {
    feed_text
        .lines()
        .filter_map(|row| {
            let mut columns = row.split('\t');
            let sku = columns.next()?.trim();
            let title = columns.next()?.trim();
            let price_cents = columns.next()?.trim().parse().ok()?;
            Some(FeedEntry { sku: sku.to_string(), title: title.to_string(), price_cents })
        })
        .collect()
}

/// Build the request path to "download" the feed for `store_id` and `page`.
/// Nothing is sent; the I/O layer issues the request.
pub fn download_path(store_id: u32, page: u32) -> String {
    format!("/stores/{store_id}/feed?page={page}")
}

/// "Write" the entries back out as feed text, in the same format `read_feed`
/// accepts. Returns the text; nothing is written anywhere.
pub fn write_feed(entries: &[FeedEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{}\t{}\t{}\n", entry.sku, entry.title, entry.price_cents))
        .collect()
}

/// Sync `incoming` into `current`: replace entries with a matching SKU and
/// append new ones. Returns how many entries were appended.
pub fn sync_entries(current: &mut Vec<FeedEntry>, incoming: Vec<FeedEntry>) -> usize {
    let mut appended = 0;
    for entry in incoming {
        match current.iter_mut().find(|existing| existing.sku == entry.sku) {
            Some(existing) => *existing = entry,
            None => {
                current.push(entry);
                appended += 1;
            }
        }
    }
    appended
}
