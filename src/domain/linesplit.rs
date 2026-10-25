//! Split one file's content into parts at line boundaries.
//!
//! Without `--diff` the judge reads a target file with its own tools, so the
//! file's whole content lands in its context beside the prompt. A file whose
//! content alone would take that over `max_prompt_chars` is judged in parts
//! rather than whole ([`crate::domain::promptsize`]): each part inlines a run of
//! the file's lines, every line prefixed with its real line number (padded to the
//! width of the file's last line number, so a part's size never depends on where
//! it starts) so the judge's citations count against the file itself.
//!
//! [`pack`] chooses the parts: each is the longest run of lines from where the
//! previous one ended that still fits, so no two adjacent parts could be merged
//! and still fit, and the same content, rules and bound always give the same
//! parts. It is the line-boundary half of [`crate::domain::diffsplit`], which
//! does the same for a diff.
//!
//! Pure: the prompt size is measured by a caller-supplied function over the
//! rendered part, so the split is decided against the prompt actually sent.

use crate::domain::diffsplit::longest;

/// A file's content as numbered lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitContent<'a> {
    lines: Vec<&'a str>,
    width: usize,
}

impl<'a> SplitContent<'a> {
    /// Split `content` into its lines (without their line endings).
    pub fn parse(content: &'a str) -> SplitContent<'a> {
        let lines: Vec<&str> = content.lines().collect();
        let width = lines.len().max(1).to_string().len();
        SplitContent { lines, width }
    }

    /// Render the lines `[from, to)` (0-based) one per line, each prefixed with
    /// its 1-based line number, with no trailing newline.
    pub fn render(&self, from: usize, to: usize) -> String {
        let mut out = String::new();
        for (i, line) in self.lines[from..to].iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("{:>w$}  {line}", from + i + 1, w = self.width));
        }
        out
    }
}

/// One part of a split file: its 1-based inclusive line range, its rendered
/// numbered lines, and its measured size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub chars: usize,
}

/// Split `content` into consecutive runs of lines whose prompts, as `measure`
/// sizes a part from its 1-based inclusive line range and rendered text, are
/// each at most `max`. Returns `None` when some single line does not fit even
/// alone, so the caller refuses the file rather than drop part of it, and an
/// empty list for content with no lines.
pub fn pack(
    content: &SplitContent,
    max: usize,
    mut measure: impl FnMut(usize, usize, &str) -> usize,
) -> Option<Vec<Part>> {
    let mut fits = |from: usize, to: usize| {
        let chars = measure(from + 1, to, &content.render(from, to));
        (chars <= max).then_some(chars)
    };
    let n = content.lines.len();
    let ends: Vec<usize> = (1..=n).collect();
    let mut parts = Vec::new();
    let mut from = 0;
    while from < n {
        let (to, chars) = longest(from, &ends[from..], &mut fits)?;
        parts.push(Part {
            start_line: from + 1,
            end_line: to,
            text: content.render(from, to),
            chars,
        });
        from = to;
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` lines `row 1` .. `row n`.
    fn rows(n: usize) -> String {
        (1..=n).map(|i| format!("row {i}\n")).collect()
    }

    /// A prompt is 50 chars of overhead plus the part's text.
    fn size(_: usize, _: usize, text: &str) -> usize {
        50 + text.chars().count()
    }

    #[test]
    fn a_part_numbers_each_line_padded_to_the_last_line_number() {
        let content = rows(12);
        let split = SplitContent::parse(&content);
        assert_eq!(split.render(8, 10), " 9  row 9\n10  row 10");
        assert_eq!(split.render(0, 1), " 1  row 1");
    }

    #[test]
    fn many_short_lines_pack_into_the_fewest_full_parts_in_order() {
        let content = rows(100);
        let split = SplitContent::parse(&content);
        let max = 200;
        let parts = pack(&split, max, size).unwrap();
        assert!(parts.len() > 1, "{parts:?}");
        let mut next = 1;
        for (i, part) in parts.iter().enumerate() {
            assert_eq!(part.start_line, next, "parts are consecutive");
            assert!(part.chars <= max);
            assert_eq!(part.chars, size(0, 0, &part.text));
            // Each part is the longest that fits: one more line would not.
            if i + 1 < parts.len() {
                let longer = split.render(part.start_line - 1, part.end_line + 1);
                assert!(
                    size(0, 0, &longer) > max,
                    "part {i} could take another line"
                );
            }
            next = part.end_line + 1;
        }
        assert_eq!(next, 101, "every line is in a part");
        let joined: Vec<String> = parts.iter().map(|p| p.text.clone()).collect();
        assert_eq!(joined.join("\n"), split.render(0, 100));
        // Deterministic.
        assert_eq!(pack(&split, max, size).unwrap(), parts);
    }

    #[test]
    fn a_line_too_long_for_any_part_refuses_the_content() {
        let content = format!("short\n{}\nshort\n", "x".repeat(300));
        let split = SplitContent::parse(&content);
        assert_eq!(pack(&split, 200, size), None);
    }

    #[test]
    fn content_that_fits_is_one_part_and_empty_content_none() {
        let content = rows(3);
        let parts = pack(&SplitContent::parse(&content), 1000, size).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!((parts[0].start_line, parts[0].end_line), (1, 3));
        assert!(pack(&SplitContent::parse(""), 1000, size)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_measure_sees_each_part_s_real_line_range() {
        let content = rows(30);
        let split = SplitContent::parse(&content);
        let mut seen = Vec::new();
        let parts = pack(&split, 120, |a, b, text| {
            seen.push((a, b));
            size(a, b, text)
        })
        .unwrap();
        assert!(seen.iter().all(|&(a, b)| a >= 1 && b >= a && b <= 30));
        assert_eq!(parts.last().unwrap().end_line, 30);
    }
}
