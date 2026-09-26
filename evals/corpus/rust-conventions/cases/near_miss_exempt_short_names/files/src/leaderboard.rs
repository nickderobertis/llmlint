/// A player's name and score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub player: String,
    pub score: u32,
}

/// Sort entries by score, highest first; ties keep their input order.
pub fn rank(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.score.cmp(&a.score));
}

/// Render the top `limit` entries as `1. name (score)` lines.
pub fn render_top(entries: &[Entry], limit: usize) -> String {
    let mut rendered = String::new();
    for (i, entry) in entries.iter().take(limit).enumerate() {
        rendered.push_str(&format!("{}. {} ({})\n", i + 1, entry.player, entry.score));
    }
    rendered
}

/// The total of every entry's score.
pub fn total_score(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| u64::from(e.score)).sum()
}

/// The names of players whose score is at least `threshold`, in input order.
pub fn qualifying_players(entries: &[Entry], threshold: u32) -> Vec<&str> {
    entries
        .iter()
        .filter(|e| e.score >= threshold)
        .map(|e| e.player.as_str())
        .collect()
}

/// Pairs of adjacent entries whose scores differ by at most `margin`.
pub fn close_races(entries: &[Entry], margin: u32) -> Vec<(&str, &str)> {
    let mut races = Vec::new();
    for i in 1..entries.len() {
        let (leader, chaser) = (&entries[i - 1], &entries[i]);
        if leader.score.abs_diff(chaser.score) <= margin {
            races.push((leader.player.as_str(), chaser.player.as_str()));
        }
    }
    races
}
