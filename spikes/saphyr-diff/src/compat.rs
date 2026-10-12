//! Candidate compatibility pre-passes: text rewrites (or refusals) applied to a
//! document before serde-saphyr reads it, each aimed at one difference the
//! harness found between serde_yaml_ng and serde-saphyr. They are measured,
//! not assumed: `main.rs` runs each over the whole corpus.

use serde_saphyr::granit_parser::{Event, Parser, ScalarStyle};

/// A parsed node, with the byte span of every scalar.
enum Node {
    Scalar { text: String, style: ScalarStyle, tagged: bool, start: usize, end: usize },
    Map { anchor: usize, pairs: Vec<(Node, Node)> },
    Seq { anchor: usize, items: Vec<Node> },
    Alias(usize),
}

struct Doc {
    root: Option<Node>,
}

fn byte_index(text: &str) -> Vec<usize> {
    let mut v: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    v.push(text.len());
    v
}

/// Parse the first document into a tree; `None` when the parser refuses it
/// (the real read then reports the syntax error unchanged).
fn parse(text: &str) -> Option<Doc> {
    let chars = byte_index(text);
    let mut events = Vec::new();
    for ev in Parser::new_from_str(text) {
        let (e, span) = ev.ok()?;
        if matches!(e, Event::Comment(..)) {
            continue;
        }
        let b = |m: serde_saphyr::granit_parser::Marker| m.byte_offset().unwrap_or_else(|| chars[m.index().min(chars.len() - 1)]);
        events.push((e, b(span.start), b(span.end)));
    }
    let mut it = events.into_iter().peekable();
    while let Some((e, _, _)) = it.peek() {
        if matches!(e, Event::DocumentStart(..)) {
            it.next();
            break;
        }
        it.next();
    }
    fn node<'a, I: Iterator<Item = (Event<'a>, usize, usize)>>(it: &mut std::iter::Peekable<I>) -> Option<Node> {
        let (e, s, t) = it.next()?;
        Some(match e {
            Event::Scalar(text, style, _, tag) => Node::Scalar { text: text.into_owned(), style, tagged: tag.is_some(), start: s, end: t },
            Event::Alias(id) => Node::Alias(id),
            Event::SequenceStart(_, anchor, _) => {
                let mut items = Vec::new();
                while !matches!(it.peek()?.0, Event::SequenceEnd) {
                    items.push(node(it)?);
                }
                it.next();
                Node::Seq { anchor, items }
            }
            Event::MappingStart(_, anchor, _) => {
                let mut pairs = Vec::new();
                while !matches!(it.peek()?.0, Event::MappingEnd) {
                    let k = node(it)?;
                    let v = node(it)?;
                    pairs.push((k, v));
                }
                it.next();
                Node::Map { anchor, pairs }
            }
            _ => return None,
        })
    }
    let root = match it.peek() {
        Some((Event::DocumentEnd, _, _)) | None => None,
        _ => Some(node(&mut it)?),
    };
    Some(Doc { root })
}

fn walk<'a>(n: &'a Node, f: &mut dyn FnMut(&'a Node)) {
    f(n);
    match n {
        Node::Map { pairs, .. } => {
            for (k, v) in pairs {
                walk(k, f);
                walk(v, f);
            }
        }
        Node::Seq { items, .. } => items.iter().for_each(|i| walk(i, f)),
        _ => {}
    }
}

fn is_merge_key(n: &Node, plain_only: bool) -> bool {
    matches!(n, Node::Scalar { text, style, tagged, .. }
        if text == "<<" && (!plain_only || (*style == ScalarStyle::Plain || *tagged)))
}

fn anchors(doc: &Doc) -> std::collections::HashMap<usize, &Node> {
    let mut m = std::collections::HashMap::new();
    if let Some(r) = &doc.root {
        walk(r, &mut |n| match n {
            Node::Map { anchor, .. } | Node::Seq { anchor, .. } if *anchor != 0 => {
                m.insert(*anchor, n);
            }
            _ => {}
        });
    }
    m
}

/// Apply byte-range replacements (non-overlapping) to `text`.
fn splice(text: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, r) in edits {
        out.replace_range(s..e, &r);
    }
    out
}

/// A plain scalar serde_yaml_ng resolves as a string but serde-saphyr as a
/// number: YAML 1.1 digit separators (`1_000`, `1_0`, `1_000.5`) and
/// leading-zero integers (`01`, `010`).
fn yaml11_only_number(s: &str) -> bool {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let digits_us = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '_') && t.starts_with(|c: char| c.is_ascii_digit());
    let underscore = body.contains('_')
        && match body.split_once('.') {
            Some((a, b)) => digits_us(a) && (b.is_empty() || digits_us(b)),
            None => digits_us(body),
        };
    let leading_zero = body.len() > 1 && body.starts_with('0') && body.chars().all(|c| c.is_ascii_digit());
    underscore || leading_zero
}

/// Pre-pass 1: single-quote every such plain scalar, so serde-saphyr reads the
/// string serde_yaml_ng reads today. Returns the text unchanged when it does
/// not parse.
pub fn quote_yaml11_numbers(text: &str) -> String {
    let Some(doc) = parse(text) else { return text.to_string() };
    let mut edits = Vec::new();
    if let Some(r) = &doc.root {
        walk(r, &mut |n| {
            if let Node::Scalar { text: t, style: ScalarStyle::Plain, tagged: false, start, end } = n {
                if yaml11_only_number(t) && &text[*start..*end] == t {
                    edits.push((*start, *end, format!("'{t}'")));
                }
            }
        });
    }
    splice(text, edits)
}

/// The mappings a `<<` value merges in (an alias, an inline mapping, or a
/// sequence of either).
fn merge_sources<'a>(v: &'a Node, a: &std::collections::HashMap<usize, &'a Node>) -> Vec<&'a Node> {
    let one = |n: &'a Node| match n {
        Node::Alias(id) => a.get(id).copied(),
        n @ Node::Map { .. } => Some(n),
        _ => None,
    };
    match v {
        Node::Seq { items, .. } => items.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

/// Every `<<` key (plain) inside a mapping that a `<<` merges in.
fn nested_merge_keys<'a>(doc: &'a Doc) -> Vec<&'a Node> {
    let a = anchors(doc);
    let mut found = Vec::new();
    if let Some(r) = &doc.root {
        walk(r, &mut |n| {
            if let Node::Map { pairs, .. } = n {
                for (k, v) in pairs {
                    if is_merge_key(k, true) {
                        for src in merge_sources(v, &a) {
                            if let Node::Map { pairs: inner, .. } = src {
                                found.extend(inner.iter().filter(|(ik, _)| is_merge_key(ik, true)).map(|(ik, _)| ik));
                            }
                        }
                    }
                }
            }
        });
    }
    found
}

/// Pre-pass 2a: refuse a merge whose source itself holds a `<<`.
pub fn refuse_nested_merge(text: &str) -> Result<(), String> {
    let Some(doc) = parse(text) else { return Ok(()) };
    match nested_merge_keys(&doc).first() {
        Some(Node::Scalar { start, .. }) => {
            let line = text[..*start].matches('\n').count() + 1;
            Err(format!("a `<<` merge brings in a mapping that itself holds `<<` (line {line}); nested merges are not supported"))
        }
        _ => Ok(()),
    }
}

/// Pre-pass 2b: emulate today's `apply_merge`, which copies a merge source's
/// own `<<` into the target as an ordinary key — by quoting that inner `<<`.
/// The quote lands in the source's text, so every other use of that mapping
/// sees it too (measured below).
pub fn quote_nested_merge_keys(text: &str) -> String {
    let Some(doc) = parse(text) else { return text.to_string() };
    let edits = nested_merge_keys(&doc)
        .into_iter()
        .filter_map(|k| match k {
            Node::Scalar { start, end, .. } if &text[*start..*end] == "<<" => Some(((*start, *end), "'<<'".to_string())),
            _ => None,
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    splice(text, edits.into_iter().map(|((s, e), r)| (s, e, r)).collect())
}

/// Pre-pass 3: today `apply_merge` treats a quoted `'<<'`/`"<<"` key as a
/// merge; rewrite it to a plain `<<` so serde-saphyr does too.
pub fn unquote_merge_keys(text: &str) -> String {
    let Some(doc) = parse(text) else { return text.to_string() };
    let mut edits = Vec::new();
    if let Some(r) = &doc.root {
        walk(r, &mut |n| {
            if let Node::Map { pairs, .. } = n {
                for (k, _) in pairs {
                    if let Node::Scalar { text: t, style: ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted, tagged: false, start, end } = k {
                        if t == "<<" {
                            edits.push((*start, *end, "<<".to_string()));
                        }
                    }
                }
            }
        });
    }
    splice(text, edits)
}

/// Pre-pass 4: refuse `<<: ~` (and an empty `<<:`), as `apply_merge` does.
pub fn refuse_null_merge(text: &str) -> Result<(), String> {
    let Some(doc) = parse(text) else { return Ok(()) };
    let mut bad = None;
    if let Some(r) = &doc.root {
        walk(r, &mut |n| {
            if let Node::Map { pairs, .. } = n {
                for (k, v) in pairs {
                    if is_merge_key(k, true) {
                        if let Node::Scalar { text: t, style: ScalarStyle::Plain, start, .. } = v {
                            if matches!(t.as_str(), "" | "~" | "null" | "Null" | "NULL") {
                                bad = Some(*start);
                            }
                        }
                    }
                }
            }
        });
    }
    match bad {
        Some(s) => Err(format!("expected a mapping or list of mappings for merging, but found scalar (line {})", text[..s].matches('\n').count() + 1)),
        None => Ok(()),
    }
}
