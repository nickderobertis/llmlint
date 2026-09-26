//! Warehouse inventory: loading stock snapshots and reorder rules from disk,
//! reconciling them, and writing the reorder plan back out.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// A SKU's stock level at one warehouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockLevel {
    pub sku: String,
    pub warehouse: String,
    pub on_hand: u32,
}

/// When to reorder a SKU and how many units to order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReorderRule {
    pub sku: String,
    pub reorder_point: u32,
    pub order_quantity: u32,
}

/// One purchase the reorder plan recommends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseLine {
    pub sku: String,
    pub quantity: u32,
}

/// Where the inventory files live for one site.
#[derive(Debug, Clone)]
pub struct InventoryPaths {
    pub stock_csv: PathBuf,
    pub rules_csv: PathBuf,
    pub plan_out: PathBuf,
}

impl InventoryPaths {
    /// The conventional layout under `root`: `stock.csv`, `reorder.csv`, and
    /// `plan.csv`.
    pub fn under(root: &Path) -> InventoryPaths {
        InventoryPaths {
            stock_csv: root.join("stock.csv"),
            rules_csv: root.join("reorder.csv"),
            plan_out: root.join("plan.csv"),
        }
    }
}

/// Split one CSV row into trimmed fields, requiring exactly `expected` of them.
///
/// The error names the file and the 1-based line number.
fn fields<'a>(row: &'a str, expected: usize, path: &Path, line_number: usize) -> Result<Vec<&'a str>> {
    let columns: Vec<&str> = row.split(',').map(str::trim).collect();
    if columns.len() != expected {
        return Err(Error::parse(format!(
            "{}:{}: expected {} fields, found {}",
            path.display(),
            line_number,
            expected,
            columns.len()
        )));
    }
    Ok(columns)
}

/// Parse a non-negative whole number from `field`, naming the file, line, and
/// column in the error.
fn parse_count(field: &str, column: &str, path: &Path, line_number: usize) -> Result<u32> {
    field.parse().map_err(|parse_error| {
        Error::parse(format!(
            "{}:{}: {} {:?} is not a whole number: {}",
            path.display(),
            line_number,
            column,
            field,
            parse_error
        ))
    })
}

/// Whether a CSV row carries no data (blank or a `#` comment).
fn is_skippable(row: &str) -> bool {
    let trimmed = row.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// Load stock levels from `path`: `sku,warehouse,on_hand` rows, with a header
/// row first. Blank lines and `#` comments are skipped.
pub fn load_stock(path: &Path) -> Result<Vec<StockLevel>> {
    let file = fs::File::open(path)
        .map_err(|e| Error::io(format!("opening stock file {}", path.display()), e))?;
    let mut levels = Vec::new();
    for (index, row) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let row = row.map_err(|read_error| {
            Error::io(format!("reading stock file {} line {}", path.display(), line_number), read_error)
        })?;
        if line_number == 1 || is_skippable(&row) {
            continue;
        }
        let columns = fields(&row, 3, path, line_number)?;
        levels.push(StockLevel {
            sku: columns[0].to_string(),
            warehouse: columns[1].to_string(),
            on_hand: parse_count(columns[2], "on_hand", path, line_number)?,
        });
    }
    Ok(levels)
}

/// Total units on hand per SKU, summed across warehouses, keyed in SKU order.
pub fn on_hand_by_sku(levels: &[StockLevel]) -> BTreeMap<&str, u32> {
    let mut totals = BTreeMap::new();
    for level in levels {
        *totals.entry(level.sku.as_str()).or_insert(0) += level.on_hand;
    }
    totals
}

/// Warehouses holding any stock of `sku`, in the order they appear in `levels`.
pub fn warehouses_stocking<'a>(levels: &'a [StockLevel], sku: &str) -> Vec<&'a str> {
    levels
        .iter()
        .filter(|level| level.sku == sku && level.on_hand > 0)
        .map(|level| level.warehouse.as_str())
        .collect()
}

/// Recommend purchases: for each rule whose SKU's total on hand is at or below
/// its reorder point, order the rule's `order_quantity`. SKUs with no stock
/// rows count as zero on hand. The plan is in rule order.
pub fn plan_reorders(levels: &[StockLevel], rules: &[ReorderRule]) -> Vec<PurchaseLine> {
    let totals = on_hand_by_sku(levels);
    rules
        .iter()
        .filter(|rule| totals.get(rule.sku.as_str()).copied().unwrap_or(0) <= rule.reorder_point)
        .map(|rule| PurchaseLine { sku: rule.sku.clone(), quantity: rule.order_quantity })
        .collect()
}

/// Merge purchase lines for the same SKU, summing their quantities. The result
/// is sorted by SKU.
pub fn consolidate(lines: &[PurchaseLine]) -> Vec<PurchaseLine> {
    let mut by_sku: BTreeMap<&str, u32> = BTreeMap::new();
    for line in lines {
        *by_sku.entry(line.sku.as_str()).or_insert(0) += line.quantity;
    }
    by_sku
        .into_iter()
        .map(|(sku, quantity)| PurchaseLine { sku: sku.to_string(), quantity })
        .collect()
}

/// Render the plan as `sku,quantity` CSV with a header row.
pub fn render_plan(lines: &[PurchaseLine]) -> String {
    let mut rendered = String::from("sku,quantity\n");
    for line in lines {
        rendered.push_str(&format!("{},{}\n", line.sku, line.quantity));
    }
    rendered
}

/// Write the rendered plan to `path`, replacing any existing file.
pub fn write_plan(path: &Path, lines: &[PurchaseLine]) -> Result<()> {
    let mut file = fs::File::create(path)
        .map_err(|e| Error::io(format!("creating reorder plan {}", path.display()), e))?;
    file.write_all(render_plan(lines).as_bytes())
        .map_err(|e| Error::io(format!("writing reorder plan {}", path.display()), e))
}

/// The SKUs that have a reorder rule but never appear in the stock snapshot,
/// in rule order. Useful for catching typos in `reorder.csv`.
pub fn rules_without_stock<'a>(levels: &[StockLevel], rules: &'a [ReorderRule]) -> Vec<&'a str> {
    let totals = on_hand_by_sku(levels);
    rules
        .iter()
        .filter(|rule| !totals.contains_key(rule.sku.as_str()))
        .map(|rule| rule.sku.as_str())
        .collect()
}

/// Load reorder rules from `path`: `sku,reorder_point,order_quantity` rows,
/// with a header row first. Blank lines and `#` comments are skipped.
pub fn load_rules(path: &Path) -> Result<Vec<ReorderRule>> {
    let text = fs::read_to_string(path)
        .map_err(|e| Error::io(format!("reading reorder rules {}", path.display()), e))?;
    let mut rules = Vec::new();
    for (index, row) in text.lines().enumerate().skip(1) {
        let line_number = index + 1;
        if is_skippable(row) {
            continue;
        }
        let columns = fields(row, 3, path, line_number)?;
        let reorder_point = parse_count(columns[1], "reorder_point", path, line_number)?;
        let order_quantity = parse_count(columns[2], "order_quantity", path, line_number)?;
        rules.push(ReorderRule {
            sku: columns[0].to_string(),
            reorder_point,
            order_quantity,
        });
    }
    Ok(rules)
}

/// Run the whole reorder cycle for one site: load stock and rules, plan and
/// consolidate purchases, write the plan, and return how many lines it has.
pub fn run_cycle(paths: &InventoryPaths) -> Result<usize> {
    let levels = load_stock(&paths.stock_csv)?;
    let rules = load_rules(&paths.rules_csv)?;
    let plan = consolidate(&plan_reorders(&levels, &rules));
    write_plan(&paths.plan_out, &plan)?;
    Ok(plan.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(sku: &str, warehouse: &str, on_hand: u32) -> StockLevel {
        StockLevel { sku: sku.into(), warehouse: warehouse.into(), on_hand }
    }

    fn rule(sku: &str, reorder_point: u32, order_quantity: u32) -> ReorderRule {
        ReorderRule { sku: sku.into(), reorder_point, order_quantity }
    }

    #[test]
    fn totals_sum_across_warehouses() {
        let levels = [level("A", "east", 3), level("A", "west", 4), level("B", "east", 1)];
        let totals = on_hand_by_sku(&levels);
        assert_eq!(totals.get("A"), Some(&7));
        assert_eq!(totals.get("B"), Some(&1));
    }

    #[test]
    fn reorders_at_or_below_the_point() {
        let levels = [level("A", "east", 5), level("B", "east", 9)];
        let rules = [rule("A", 5, 20), rule("B", 5, 20), rule("C", 0, 10)];
        let plan = plan_reorders(&levels, &rules);
        let skus: Vec<&str> = plan.iter().map(|line| line.sku.as_str()).collect();
        assert_eq!(skus, ["A", "C"]);
    }

    #[test]
    fn consolidate_sums_and_sorts() {
        let lines = [
            PurchaseLine { sku: "B".into(), quantity: 2 },
            PurchaseLine { sku: "A".into(), quantity: 1 },
            PurchaseLine { sku: "B".into(), quantity: 3 },
        ];
        let merged = consolidate(&lines);
        assert_eq!(merged[0], PurchaseLine { sku: "A".into(), quantity: 1 });
        assert_eq!(merged[1], PurchaseLine { sku: "B".into(), quantity: 5 });
    }
}
