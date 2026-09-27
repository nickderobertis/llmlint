//! Split a source file into the small, self-contained units Jev judges best —
//! lightweight, per-language heuristics, no parser. Pure.
//!
//! Jev answers one well-scoped question against a small state far better than a
//! compound question against a whole file (unrelated content is a distractor),
//! so a rule whose check is really "for each X in the file" is asked once per X.
//! The heuristics only have to scope the state well; they never decide a
//! verdict, so a construct they miss simply isn't asked about (and the cascade
//! can still escalate the file).

/// One documented item: its doc comment broken into claims, and the item's own
/// code (the doc lines excluded), both with 1-based line numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocItem {
    pub claims: Vec<Claim>,
    /// First and last line of the item's code (after its doc and attributes).
    pub code_start: usize,
    pub code_end: usize,
    /// Same-file functions this item calls, as `(first, last)` line ranges —
    /// the one hop Jev needs to see what the item actually does.
    pub helpers: Vec<(usize, usize)>,
}

/// One sentence of a doc comment, with the line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub line: usize,
    pub text: String,
}

/// The doc-comment syntax a file uses, from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    /// `///` before the item (Rust).
    Rust,
    /// A docstring as the first statement inside `def`/`class` (Python).
    Python,
    /// `/** … */` before the item (JS/TS/Java/Kotlin/C#/C/C++/PHP), or a `//`
    /// run directly above a Go declaration.
    CLike,
}

fn lang(path: &str) -> Option<Lang> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(Lang::Rust),
        "py" | "pyi" => Some(Lang::Python),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "java" | "kt" | "cs" | "c" | "h" | "cc"
        | "cpp" | "hpp" | "php" | "go" | "swift" | "scala" => Some(Lang::CLike),
        _ => None,
    }
}

/// Every documented *function-like* item in `text` (a function, method, or
/// closure-bearing declaration), in file order — the items whose docs describe
/// behavior code can confirm or contradict. Docs on types, fields, variants, and
/// constants are descriptive: the code cannot confirm "a parse error whose
/// message names the file" on an enum variant, so asking would only produce
/// false alarms. Empty for a language the heuristics don't know.
pub fn doc_items(path: &str, text: &str) -> Vec<DocItem> {
    let lines: Vec<&str> = text.lines().collect();
    let functions = function_index(path, &lines);
    all_doc_items(path, &lines)
        .into_iter()
        .filter(|item| {
            lines
                .get(item.code_start - 1)
                .is_some_and(|l| is_function_header(l))
        })
        .map(|mut item| {
            item.helpers = callees(&lines, &item, &functions);
            item
        })
        .collect()
}

/// A group of sites sharing one state: the function that contains them (or a
/// small window, for code outside any function).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteGroup {
    pub start: usize,
    pub end: usize,
    /// 1-based lines the pattern matched, in order.
    pub sites: Vec<usize>,
    /// Same-file functions called *on the site lines*, as line ranges: whether
    /// `load(path)?` adds context depends on what `load` does.
    pub helpers: Vec<(usize, usize)>,
}

/// Lines of context around a site outside any function.
const SITE_WINDOW: usize = 6;

/// Whether `line` is a comment-only line in `path`'s language: `//`, `/*`,
/// `*` for Rust and C-like sources (where `#` starts an attribute or a
/// preprocessor line), `#` for Python, and either for other config/script
/// files. Prose files (Markdown, reStructuredText, plain text) have no
/// comments — a `#` there is a heading.
pub fn is_comment_line(path: &str, line: &str) -> bool {
    let t = line.trim_start();
    let c_style = t.starts_with("//") || t.starts_with("/*") || t.starts_with('*');
    match lang(path) {
        Some(Lang::Rust | Lang::CLike) => c_style,
        Some(Lang::Python) => t.starts_with('#'),
        None => {
            let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            !matches!(ext.as_str(), "md" | "markdown" | "rst" | "txt")
                && (t.starts_with('#') || c_style)
        }
    }
}

/// Every non-comment line matching `pattern`, grouped by the innermost
/// function containing it. A site's state is its whole function, so the check
/// sees what the matched construct is doing (which call, which value).
pub fn site_groups(path: &str, text: &str, pattern: &regex::Regex) -> Vec<SiteGroup> {
    let lines: Vec<&str> = text.lines().collect();
    let functions = function_index(path, &lines);
    let mut groups: Vec<SiteGroup> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if is_comment_line(path, line) || !pattern.is_match(line) {
            continue;
        }
        let n = i + 1;
        let (start, end) = functions
            .iter()
            .filter(|(_, s, e)| (*s..=*e).contains(&n))
            .min_by_key(|(_, s, e)| e - s)
            .map(|(_, s, e)| (*s, *e))
            .unwrap_or_else(|| {
                (
                    n.saturating_sub(SITE_WINDOW).max(1),
                    (n + SITE_WINDOW).min(lines.len()),
                )
            });
        match groups.iter_mut().find(|g| g.start == start && g.end == end) {
            Some(g) => g.sites.push(n),
            None => groups.push(SiteGroup {
                start,
                end,
                sites: vec![n],
                helpers: Vec::new(),
            }),
        }
    }
    for g in &mut groups {
        let site_text: String = g
            .sites
            .iter()
            .filter_map(|n| lines.get(n - 1))
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        g.helpers = functions
            .iter()
            .filter(|(_, s, _)| *s != g.start)
            .filter(|(name, _, _)| calls(&site_text, name))
            .take(MAX_HELPERS)
            .map(|(_, s, e)| (*s, *e))
            .collect();
    }
    groups
}

/// Whether `text` calls `name(` as a whole identifier.
fn calls(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().last();
        let after = text[i + name.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_') && after == Some('(')
    })
}

/// The 1-based `(first, last)` line range of every item of the YAML list under
/// the top-level `key` (`key:` at column 0), block (`- name: x` + indented
/// fields) or flow (`- { name: x }`) style. Text-based, so line numbers are the
/// file's own; trailing blank lines and comment-only lines between items are
/// not part of an entry.
pub fn yaml_entries(text: &str, key: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let header = format!("{key}:");
    let Some(start) = lines.iter().position(|l| l.trim_end() == header) else {
        return Vec::new();
    };
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut item_indent: Option<usize> = None;
    let mut last_content = 0;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let ind = indent(line);
        let is_item = t.starts_with("- ") || t == "-";
        if ind == 0 && !is_item {
            break; // the next top-level key ends the list
        }
        match item_indent {
            None if is_item => item_indent = Some(ind),
            _ => {}
        }
        if is_item && Some(ind) == item_indent {
            if let Some(prev) = out.last_mut() {
                prev.1 = last_content + 1;
            }
            out.push((i + 1, i + 1));
        } else if item_indent.is_some_and(|it| ind < it) {
            break;
        }
        last_content = i;
    }
    if let Some(prev) = out.last_mut() {
        prev.1 = last_content + 1;
    }
    out
}

/// At most this many helpers are attached to one item, so the state stays small.
const MAX_HELPERS: usize = 4;

/// Every function defined in the file: its name and 1-based line range.
fn function_index(path: &str, lines: &[&str]) -> Vec<(String, usize, usize)> {
    let python = lang(path) == Some(Lang::Python);
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let Some(name) = function_name(t) else {
            continue;
        };
        let end = if python {
            let head = indent(line);
            let mut end = i;
            for (k, l) in lines.iter().enumerate().skip(i + 1) {
                if l.trim().is_empty() {
                    continue;
                }
                if indent(l) <= head {
                    break;
                }
                end = k;
            }
            end
        } else {
            brace_item_end(lines, i)
        };
        out.push((name, i + 1, end + 1));
    }
    out
}

/// The name a `fn`/`def`/`function`/`func` declaration defines, if `t` is one.
fn function_name(t: &str) -> Option<String> {
    let mut words = t.split_whitespace().peekable();
    while let Some(w) = words.next() {
        if matches!(w, "fn" | "def" | "function" | "func") {
            let next = words.peek()?;
            let name: String = next
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            return (!name.is_empty()).then_some(name);
        }
        if !matches!(
            w,
            "pub" | "pub(crate)" | "async" | "export" | "default" | "static" | "const" | "unsafe"
        ) {
            return None;
        }
    }
    None
}

/// Same-file functions the item's code calls (`name(`), excluding itself.
fn callees(
    lines: &[&str],
    item: &DocItem,
    functions: &[(String, usize, usize)],
) -> Vec<(usize, usize)> {
    let body: String = (item.code_start..=item.code_end)
        .filter_map(|n| lines.get(n - 1))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    functions
        .iter()
        .filter(|(_, start, _)| *start != item.code_start)
        .filter(|(name, _, _)| calls(&body, name))
        .take(MAX_HELPERS)
        .map(|(_, s, e)| (*s, *e))
        .collect()
}

/// Whether a declaration line starts a function or method.
fn is_function_header(line: &str) -> bool {
    let t = line.trim_start();
    let words: Vec<&str> = t
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .take(6)
        .collect();
    words
        .iter()
        .any(|w| matches!(*w, "fn" | "def" | "function" | "func"))
        // TS/Java/C# methods: `name(args) {` or `Type name(args)` with no keyword.
        || (t.contains('(')
            && !t.starts_with("class ")
            && !t.starts_with("struct ")
            && !t.starts_with("enum ")
            && !t.starts_with("type ")
            && !t.contains(" class ")
            && (t.trim_end().ends_with('{') || t.trim_end().ends_with(')')))
}

fn all_doc_items(path: &str, lines: &[&str]) -> Vec<DocItem> {
    match lang(path) {
        Some(Lang::Rust) => prefixed_docs(lines, "///"),
        Some(Lang::Python) => python_docs(lines),
        Some(Lang::CLike) if path.ends_with(".go") => go_docs(lines),
        Some(Lang::CLike) => block_docs(lines),
        None => Vec::new(),
    }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The last line of the item that starts at `start` (0-based): through its
/// matching closing brace when it opens one, else just its own line(s) up to a
/// terminating `;` or `,`. Naive about braces inside strings/comments, which
/// only ever widens or narrows the state slightly.
fn brace_item_end(lines: &[&str], start: usize) -> usize {
    let mut depth: i64 = 0;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        for c in line.chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return i;
        }
        let t = line.trim_end();
        if !opened && (t.ends_with(';') || t.ends_with(',')) {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

/// Rust-style docs: a run of lines starting with `marker`, then any attribute
/// lines, then the item.
fn prefixed_docs(lines: &[&str], marker: &str) -> Vec<DocItem> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with(marker) {
            i += 1;
            continue;
        }
        let doc_start = i;
        while i < lines.len() && lines[i].trim_start().starts_with(marker) {
            i += 1;
        }
        let doc: Vec<(usize, &str)> = (doc_start..i)
            .map(|k| (k, lines[k].trim_start().trim_start_matches(marker)))
            .collect();
        // Skip attributes (`#[…]`) and blank lines between the doc and its item.
        while i < lines.len()
            && (lines[i].trim_start().starts_with("#[") || lines[i].trim().is_empty())
        {
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        let end = brace_item_end(lines, i);
        out.push(DocItem {
            claims: claims(&doc),
            code_start: i + 1,
            code_end: end + 1,
            helpers: Vec::new(),
        });
        // Docs nested inside this item (fields, methods) are items of their own.
        i += 1;
    }
    out
}

/// Go: a run of `//` lines directly above a top-level declaration.
fn go_docs(lines: &[&str]) -> Vec<DocItem> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("//") {
            i += 1;
            continue;
        }
        let doc_start = i;
        while i < lines.len() && lines[i].trim_start().starts_with("//") {
            i += 1;
        }
        let Some(next) = lines.get(i) else { break };
        let decl = next.trim_start();
        if ["func ", "type ", "var ", "const "]
            .iter()
            .any(|k| decl.starts_with(k))
        {
            let doc: Vec<(usize, &str)> = (doc_start..i)
                .map(|k| (k, lines[k].trim_start().trim_start_matches("//")))
                .collect();
            out.push(DocItem {
                claims: claims(&doc),
                code_start: i + 1,
                code_end: brace_item_end(lines, i) + 1,
                helpers: Vec::new(),
            });
        }
    }
    out
}

/// `/** … */` blocks followed by an item.
fn block_docs(lines: &[&str]) -> Vec<DocItem> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("/**") {
            i += 1;
            continue;
        }
        let doc_start = i;
        while i < lines.len() && !lines[i].contains("*/") {
            i += 1;
        }
        let doc_end = i.min(lines.len().saturating_sub(1));
        let doc: Vec<(usize, &str)> = (doc_start..=doc_end)
            .map(|k| {
                let t = lines[k]
                    .trim()
                    .trim_start_matches("/**")
                    .trim_end_matches("*/")
                    .trim_start_matches('*');
                (k, t)
            })
            .collect();
        i = doc_end + 1;
        while i < lines.len()
            && (lines[i].trim().is_empty() || lines[i].trim_start().starts_with('@'))
        {
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        out.push(DocItem {
            claims: claims(&doc),
            code_start: i + 1,
            code_end: brace_item_end(lines, i) + 1,
            helpers: Vec::new(),
        });
    }
    out
}

/// Python: `def`/`class` whose first statement is a docstring; the item runs
/// until the next non-blank line indented no deeper than its header.
fn python_docs(lines: &[&str]) -> Vec<DocItem> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if !(t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class ")) {
            continue;
        }
        let head_indent = indent(line);
        // The header may span lines (long signatures); it ends at a line ending `:`.
        let mut h = i;
        while h < lines.len() && !lines[h].trim_end().ends_with(':') {
            h += 1;
        }
        let Some(first) = (h + 1..lines.len()).find(|&k| !lines[k].trim().is_empty()) else {
            continue;
        };
        let body = lines[first].trim_start();
        let quote = if body.starts_with("\"\"\"") {
            "\"\"\""
        } else if body.starts_with("'''") {
            "'''"
        } else {
            continue;
        };
        // The docstring's extent.
        let mut d = first;
        let one_line = body.len() > 3 && body[3..].contains(quote);
        if !one_line {
            d += 1;
            while d < lines.len() && !lines[d].contains(quote) {
                d += 1;
            }
        }
        let doc_end = d.min(lines.len() - 1);
        let doc: Vec<(usize, &str)> = (first..=doc_end)
            .map(|k| {
                (
                    k,
                    lines[k]
                        .trim()
                        .trim_start_matches(quote)
                        .trim_end_matches(quote),
                )
            })
            .collect();
        let mut end = doc_end;
        for (k, l) in lines.iter().enumerate().skip(doc_end + 1) {
            if l.trim().is_empty() {
                continue;
            }
            if indent(l) <= head_indent {
                break;
            }
            end = k;
        }
        // The code is the header plus the body after the docstring.
        out.push(DocItem {
            claims: claims(&doc),
            code_start: i + 1,
            code_end: end + 1,
            helpers: Vec::new(),
        });
    }
    out
}

/// Split doc lines (0-based line, text with the comment marker removed) into
/// sentence claims. A blank line or a markdown heading/bullet starts a new
/// claim; within a paragraph, sentences split after `.`, `!`, or `?`. A
/// heading like `# Errors` is kept as a prefix of the claim that follows, since
/// it scopes it.
fn claims(doc: &[(usize, &str)]) -> Vec<Claim> {
    let mut out: Vec<Claim> = Vec::new();
    let mut cur = String::new();
    let mut cur_line = 0;
    let mut heading = String::new();
    let flush = |cur: &mut String, line: usize, heading: &str, out: &mut Vec<Claim>| {
        let t = cur.trim();
        if !t.is_empty() {
            let text = if heading.is_empty() {
                t.to_string()
            } else {
                format!("{heading}: {t}")
            };
            out.push(Claim {
                line: line + 1,
                text,
            });
        }
        cur.clear();
    };
    for &(line, raw) in doc {
        let t = raw.trim();
        if t.is_empty() {
            flush(&mut cur, cur_line, &heading, &mut out);
            continue;
        }
        // A markdown heading (`# Errors`) or a docstring section label
        // (`Raises:`, `Returns:`) scopes the claims that follow.
        let section = t
            .strip_prefix('#')
            .map(|h| h.trim_start_matches('#').trim())
            .or_else(|| {
                t.strip_suffix(':').filter(|w| {
                    !w.is_empty()
                        && w.chars().all(|c| c.is_alphanumeric() || c == ' ')
                        && w.split(' ').count() <= 2
                })
            });
        if let Some(h) = section {
            flush(&mut cur, cur_line, &heading, &mut out);
            heading = h.to_string();
            continue;
        }
        let bullet = t.starts_with("- ") || t.starts_with("* ");
        if bullet {
            flush(&mut cur, cur_line, &heading, &mut out);
        }
        let t = if bullet { &t[2..] } else { t };
        if cur.is_empty() {
            cur_line = line;
        }
        // Split sentences inside this line; a sentence may continue onto the next.
        let mut rest = t;
        loop {
            let cut = rest
                .char_indices()
                .find(|&(i, c)| {
                    matches!(c, '.' | '!' | '?')
                        && rest[i + 1..].starts_with(' ')
                        && rest[i + 1..]
                            .trim_start()
                            .starts_with(|n: char| n.is_uppercase() || n == '`')
                })
                .map(|(i, _)| i + 1);
            match cut {
                Some(i) => {
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(&rest[..i]);
                    flush(&mut cur, cur_line, &heading, &mut out);
                    rest = rest[i..].trim_start();
                    cur_line = line;
                }
                None => {
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(rest);
                    break;
                }
            }
        }
        if cur.trim_end().ends_with(['.', '!', '?']) {
            flush(&mut cur, cur_line, &heading, &mut out);
        }
    }
    flush(&mut cur, cur_line, &heading, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(item: &DocItem) -> Vec<(usize, &str)> {
        item.claims
            .iter()
            .map(|c| (c.line, c.text.as_str()))
            .collect()
    }

    fn span(item: &DocItem) -> (usize, usize) {
        (item.code_start, item.code_end)
    }

    #[test]
    fn rust_functions_pair_each_doc_with_its_braced_code() {
        let src = "\
use std::fs;

/// Load prices from `path`. Returns an error naming the path.
///
/// # Errors
/// Fails when the file is unreadable.
#[inline]
pub fn load(path: &Path) -> Result<()> {
    let x = 1;
    Ok(())
}

/// The total in cents.
pub const TOTAL: u64 = 3;
";
        let items = doc_items("src/a.rs", src);
        assert_eq!(
            items.len(),
            1,
            "a documented const is descriptive, not checked"
        );
        assert_eq!(
            texts(&items[0]),
            [
                (3, "Load prices from `path`."),
                (3, "Returns an error naming the path."),
                (6, "Errors: Fails when the file is unreadable.")
            ]
        );
        assert_eq!(span(&items[0]), (8, 11));
    }

    #[test]
    fn type_field_and_variant_docs_are_skipped_but_methods_are_items() {
        let src = "\
/// A cart.
pub struct Cart {
    /// Items, newest first.
    pub items: Vec<u8>,
}

impl Cart {
    /// The number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
}
";
        let items = doc_items("x.rs", src);
        assert_eq!(items.len(), 1);
        assert_eq!(texts(&items[0]), [(8, "The number of items.")]);
        assert_eq!(span(&items[0]), (9, 11));
    }

    #[test]
    fn a_sentence_wrapping_across_lines_is_one_claim() {
        let src = "\
/// Sort line items by total price,
/// cheapest first. Stable for ties.
fn sort() {}
";
        let items = doc_items("x.rs", src);
        assert_eq!(
            texts(&items[0]),
            [
                (1, "Sort line items by total price, cheapest first."),
                (2, "Stable for ties.")
            ]
        );
    }

    #[test]
    fn python_docstrings_with_sections_cover_the_def_and_its_body() {
        let src = r#"import json


def load(path: str) -> dict:
    """Read `path` as JSON.

    Raises:
        ValueError: when the file is not JSON.
    """
    with open(path) as f:
        return json.load(f)


class Store:
    '''A store.'''

    def get(self, key):
        """Return the value, or None when missing."""
        return self.items[key]
"#;
        let items = doc_items("svc/load.py", src);
        assert_eq!(
            items.len(),
            2,
            "the class docstring is descriptive, not checked"
        );
        assert_eq!(
            texts(&items[0]),
            [
                (5, "Read `path` as JSON."),
                (8, "Raises: ValueError: when the file is not JSON.")
            ]
        );
        assert_eq!(span(&items[0]), (4, 11));
        assert_eq!(
            texts(&items[1]),
            [(18, "Return the value, or None when missing.")]
        );
        assert_eq!(span(&items[1]), (17, 19));
    }

    #[test]
    fn same_file_helpers_the_item_calls_are_attached() {
        let src = r#"def _fetch(client, key):
    return client.get(key)


def _unused():
    return 1


def load(client, key):
    """Load `key`; raises NotFound when missing."""
    body = _fetch(client, key)
    return body
"#;
        let items = doc_items("a.py", src);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].helpers,
            [(1, 2)],
            "only the called helper, not `_unused`"
        );

        let rust = "\
fn parse_row(r: &str) -> u8 { 0 }

/// Parse every row.
pub fn parse(rows: &[&str]) -> Vec<u8> {
    rows.iter().map(|r| parse_row(r)).collect()
}
";
        assert_eq!(doc_items("a.rs", rust)[0].helpers, [(1, 1)]);
    }

    #[test]
    fn a_def_without_a_docstring_is_not_an_item() {
        assert!(doc_items("a.py", "def f():\n    return 1\n").is_empty());
    }

    #[test]
    fn js_block_docs_and_go_line_docs() {
        let js = "\
/**
 * Add two numbers.
 * @param a first
 */
export function add(a, b) {
  return a + b;
}
";
        let items = doc_items("m.ts", js);
        assert_eq!(
            texts(&items[0]),
            [(2, "Add two numbers."), (3, "@param a first")]
        );
        assert_eq!(span(&items[0]), (5, 7));

        let go = "\
// Add returns the sum.
func Add(a, b int) int {
\treturn a + b
}

// Limit is the cap.
var Limit = 1
";
        let items = doc_items("m.go", go);
        assert_eq!(items.len(), 1);
        assert_eq!(texts(&items[0]), [(1, "Add returns the sum.")]);
        assert_eq!(span(&items[0]), (2, 4));
    }

    #[test]
    fn sites_group_by_their_innermost_function_and_skip_comments() {
        let src = "\
fn load(p: &Path) -> Result<String> {
    // read() may fail?
    let t = fs::read_to_string(p)?;
    let n = parse(&t)?;
    Ok(t)
}

const X: u8 = helper()?;
";
        let re = regex::Regex::new(r"\?").unwrap();
        let groups = site_groups("a.rs", src, &re);
        assert_eq!(
            groups,
            [
                SiteGroup {
                    start: 1,
                    end: 6,
                    sites: vec![3, 4],
                    helpers: vec![]
                },
                SiteGroup {
                    start: 2,
                    end: 8,
                    sites: vec![8],
                    helpers: vec![]
                },
            ]
        );
    }

    #[test]
    fn yaml_entries_split_block_and_flow_items_under_the_key() {
        let src = "\
version: 1
rules:
  - name: first
    # a comment inside
    description: |
      One.

      Two.
  - { name: second, description: d }

  # between items
  - name: third
    files: { include: [\"*.py\"] }
agents:
  default: {}
";
        assert_eq!(yaml_entries(src, "rules"), [(3, 8), (9, 9), (12, 13)]);
        assert!(yaml_entries(src, "plugins").is_empty());
        assert_eq!(
            yaml_entries("rules:\n- name: a\n  d: 1\n- name: b\nnext: 1\n", "rules"),
            [(2, 3), (4, 4)],
            "items may sit at column 0"
        );
    }

    #[test]
    fn unknown_languages_and_bullets() {
        assert!(doc_items("notes.txt", "/// x\nfn a() {}\n").is_empty());
        let src = "/// Options:\n/// - fast mode\n/// - `safe` mode\nfn a() {}\n";
        assert_eq!(
            texts(&doc_items("a.rs", src)[0]),
            [(2, "Options: fast mode"), (3, "Options: `safe` mode")]
        );
    }

    #[test]
    fn comment_lines_follow_the_file_language() {
        assert!(is_comment_line("a.rs", "   // gone"));
        assert!(
            !is_comment_line("a.rs", "#[arg(long = \"x\")]"),
            "a Rust attribute"
        );
        assert!(is_comment_line("a.py", "  # note"));
        assert!(is_comment_line("config/example.env", "# FOO=1"));
        assert!(
            !is_comment_line("README.md", "## Flags"),
            "a Markdown heading"
        );
        assert!(!is_comment_line("a.py", "x = 1  # trailing"));
    }
}
