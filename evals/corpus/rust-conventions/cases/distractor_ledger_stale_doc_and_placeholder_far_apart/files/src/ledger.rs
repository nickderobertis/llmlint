//! A double-entry ledger: accounts, postings, balances, and the text export
//! used by the month-end close.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Which side of the ledger a posting lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Debit,
    Credit,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Debit => write!(f, "DR"),
            Side::Credit => write!(f, "CR"),
        }
    }
}

/// One posting: an amount in cents on one side of one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub account: String,
    pub side: Side,
    pub cents: u64,
}

/// A journal entry: a memo and the postings it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub memo: String,
    pub postings: Vec<Posting>,
}

impl Entry {
    /// Total debits and total credits of this entry, as `(debits, credits)`.
    pub fn sides(&self) -> (u64, u64) {
        let mut debits = 0;
        let mut credits = 0;
        for posting in &self.postings {
            match posting.side {
                Side::Debit => debits += posting.cents,
                Side::Credit => credits += posting.cents,
            }
        }
        (debits, credits)
    }

    /// Whether debits equal credits. An entry with no postings is rejected as
    /// unbalanced.
    pub fn is_balanced(&self) -> bool {
        let (debits, credits) = self.sides();
        debits == credits
    }
}

/// Why an entry was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Unbalanced { memo: String, debits: u64, credits: u64 },
    UnknownAccount { memo: String, account: String },
}

/// The ledger: the chart of accounts and every accepted entry, in posting
/// order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ledger {
    accounts: Vec<String>,
    entries: Vec<Entry>,
}

impl Ledger {
    /// A ledger with the given chart of accounts and no entries.
    pub fn with_accounts(accounts: &[&str]) -> Ledger {
        Ledger {
            accounts: accounts.iter().map(|account| account.to_string()).collect(),
            entries: Vec::new(),
        }
    }

    /// Whether `account` is in the chart of accounts.
    pub fn has_account(&self, account: &str) -> bool {
        self.accounts.iter().any(|known| known == account)
    }

    /// Accept `entry` if it balances and names only known accounts; otherwise
    /// return why it was refused and leave the ledger unchanged.
    pub fn post(&mut self, entry: Entry) -> std::result::Result<(), Rejection> {
        if !entry.is_balanced() {
            let (debits, credits) = entry.sides();
            return Err(Rejection::Unbalanced { memo: entry.memo, debits, credits });
        }
        if let Some(posting) = entry.postings.iter().find(|posting| !self.has_account(&posting.account)) {
            return Err(Rejection::UnknownAccount {
                memo: entry.memo.clone(),
                account: posting.account.clone(),
            });
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Every accepted entry, in posting order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Net balance per account as signed cents (debits positive, credits
    /// negative), keyed in account-name order. Accounts with no postings are
    /// included with a zero balance.
    pub fn balances(&self) -> BTreeMap<&str, i64> {
        let mut balances: BTreeMap<&str, i64> =
            self.accounts.iter().map(|account| (account.as_str(), 0)).collect();
        for entry in &self.entries {
            for posting in &entry.postings {
                let signed = match posting.side {
                    Side::Debit => posting.cents as i64,
                    Side::Credit => -(posting.cents as i64),
                };
                *balances.entry(posting.account.as_str()).or_insert(0) += signed;
            }
        }
        balances
    }

    /// The entries that touch `account`, in posting order.
    pub fn entries_for<'a>(&'a self, account: &'a str) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.postings.iter().any(|posting| posting.account == account))
    }
}

/// Format signed cents as `-1234.56` style text (no currency symbol).
pub fn format_signed(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let magnitude = cents.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

/// Render one entry as its memo line followed by one indented line per
/// posting: `  DR account  12.34`.
pub fn render_entry(entry: &Entry) -> String {
    let mut rendered = format!("{}\n", entry.memo);
    for posting in &entry.postings {
        rendered.push_str(&format!(
            "  {} {:<24}{:>12}\n",
            posting.side,
            posting.account,
            format_signed(posting.cents as i64)
        ));
    }
    rendered
}

/// Render the trial balance: one `account  balance` line per account in
/// account-name order, then a `TOTAL` line that is zero for a balanced ledger.
pub fn render_trial_balance(ledger: &Ledger) -> String {
    let balances = ledger.balances();
    let mut rendered = String::new();
    for (account, cents) in &balances {
        rendered.push_str(&format!("{:<24}{:>14}\n", account, format_signed(*cents)));
    }
    let total: i64 = balances.values().sum();
    rendered.push_str(&format!("{:<24}{:>14}\n", "TOTAL", format_signed(total)));
    rendered
}

/// Parse one `side,account,cents` posting row, naming `path` and the 1-based
/// line in any error.
fn parse_posting(row: &str, path: &Path, line_number: usize) -> Result<Posting> {
    let columns: Vec<&str> = row.split(',').map(str::trim).collect();
    let [side, account, cents] = columns[..] else {
        return Err(Error::parse(format!("{}:{}: expected side,account,cents", path.display(), line_number)));
    };
    let side = match side {
        "DR" => Side::Debit,
        "CR" => Side::Credit,
        other => {
            return Err(Error::parse(format!(
                "{}:{}: side must be DR or CR, found {other:?}",
                path.display(),
                line_number
            )))
        }
    };
    let cents = cents.parse().map_err(|parse_error| {
        Error::parse(format!("{}:{}: bad cents {cents:?}: {parse_error}", path.display(), line_number))
    })?;
    Ok(Posting { account: account.to_string(), side, cents })
}

/// Load a journal from `path`. Each entry starts with a `# memo` line and is
/// followed by its posting rows; blank lines separate entries.
pub fn load_journal(path: &Path) -> Result<Vec<Entry>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading journal {}", path.display()), e))?;
    let mut journal = Vec::new();
    let mut current: Option<Entry> = None;
    for (index, row) in text.lines().enumerate() {
        let line_number = index + 1;
        let row = row.trim();
        if row.is_empty() {
            journal.extend(current.take());
        } else if let Some(memo) = row.strip_prefix('#') {
            journal.extend(current.take());
            current = Some(Entry { memo: memo.trim().to_string(), postings: Vec::new() });
        } else {
            let posting = parse_posting(row, path, line_number)?;
            let Some(entry) = current.as_mut() else {
                return Err(Error::parse(format!(
                    "{}:{}: posting before any `# memo` line",
                    path.display(),
                    line_number
                )));
            };
            entry.postings.push(posting);
        }
    }
    journal.extend(current.take());
    Ok(journal)
}

/// Post every entry in `journal` to `ledger`, collecting the rejections in
/// journal order. Accepted entries stay posted even when others are refused.
pub fn post_all(ledger: &mut Ledger, journal: Vec<Entry>) -> Vec<Rejection> {
    let mut stuff = Vec::new();
    for entry in journal {
        if let Err(rejection) = ledger.post(entry) {
            stuff.push(rejection);
        }
    }
    stuff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posting(account: &str, side: Side, cents: u64) -> Posting {
        Posting { account: account.into(), side, cents }
    }

    #[test]
    fn balanced_entries_post() {
        let mut ledger = Ledger::with_accounts(&["cash", "revenue"]);
        let entry = Entry {
            memo: "sale".into(),
            postings: vec![posting("cash", Side::Debit, 500), posting("revenue", Side::Credit, 500)],
        };
        assert_eq!(ledger.post(entry), Ok(()));
        assert_eq!(ledger.balances().get("cash"), Some(&500));
    }
}
