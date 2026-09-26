use crate::error::{Error, Result};

/// A staff member parsed from a roster row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub shift: String,
}

/// Parse a roster: one `name,shift` pair per non-blank line.
///
/// # Errors
///
/// Returns `Error::EmptyRoster` when `text` contains no rows, and a parse error
/// naming the line when a row has no comma.
pub fn parse_roster(text: &str) -> Result<Vec<Member>> {
    let mut members = Vec::new();
    for (index, row) in text.lines().enumerate() {
        if row.trim().is_empty() {
            continue;
        }
        let (name, shift) = row
            .split_once(',')
            .ok_or_else(|| Error::parse(format!("roster line {}: expected name,shift in {row:?}", index + 1)))?;
        members.push(Member { name: name.trim().to_string(), shift: shift.trim().to_string() });
    }
    Ok(members)
}
