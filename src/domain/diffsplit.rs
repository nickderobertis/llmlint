//! Split one file's unified diff into parts at hunk or line boundaries.
//!
//! A file whose diff alone makes a judge prompt over `max_prompt_chars` is
//! judged in parts rather than refused ([`crate::domain::promptsize`]). This
//! module owns the diff side of that: it parses the diff into its preamble and
//! hunks, each hunk into **line units** (one diff line, with any `\ No newline at
//! end of file` marker kept on the line it qualifies), and renders any contiguous
//! span of units back into a diff. Every part repeats the preamble; a hunk a part
//! carries whole keeps its header byte for byte, and a hunk cut at a line
//! boundary gets a header recomputed for exactly the lines it carries, so each
//! part is a valid diff whose new-file line numbers are the file's real ones —
//! which is what the ignore-aware trimming ([`crate::domain::diffmodel`]) and the
//! judge's line citations count against.
//!
//! [`pack`] chooses the parts: each is the longest span from where the previous
//! one ended that still fits, preferring to end at a hunk boundary and cutting
//! inside a hunk only when that hunk does not fit in a part of its own. So no two
//! adjacent parts could be merged and still fit, and the same diff, rules and
//! bound always give the same parts.
//!
//! Pure: the prompt size is measured by a caller-supplied function over the
//! rendered part, so the split is decided against the prompt actually sent.

/// One hunk: its header pieces (so a cut can be re-headered) and its line units.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    /// The raw `@@ … @@…` header line, newline included.
    header: String,
    /// The parsed ranges, or `None` when the header is not one we can recompute —
    /// such a hunk is only ever carried whole.
    ranges: Option<Ranges>,
    /// The line units, each one diff line (newline included) plus any `\` marker
    /// lines that follow it.
    units: Vec<String>,
}

/// A hunk header's old/new starts and counts, and whatever follows the closing
/// `@@` (a function-context hint and the newline).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ranges {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    tail: String,
}

/// A parsed single-file unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitDiff {
    preamble: String,
    hunks: Vec<Hunk>,
}

/// Parse `-a,b` / `+c,d` (the count defaults to 1 when omitted).
fn parse_range(token: &str, sign: char) -> Option<(usize, usize)> {
    let body = token.strip_prefix(sign)?;
    let (start, count) = match body.split_once(',') {
        Some((s, c)) => (s.parse().ok()?, c.parse().ok()?),
        None => (body.parse().ok()?, 1),
    };
    Some((start, count))
}

fn parse_header(raw: &str) -> Option<Ranges> {
    let rest = raw.strip_prefix("@@ ")?;
    let (ranges, tail) = rest.split_once(" @@")?;
    let mut parts = ranges.split(' ');
    let (old_start, old_count) = parse_range(parts.next()?, '-')?;
    let (new_start, new_count) = parse_range(parts.next()?, '+')?;
    if parts.next().is_some() {
        return None;
    }
    Some(Ranges {
        old_start,
        old_count,
        new_start,
        new_count,
        tail: tail.to_string(),
    })
}

/// How many old-file and new-file lines a unit spans.
fn unit_counts(unit: &str) -> (usize, usize) {
    match unit.as_bytes().first() {
        Some(b'+') => (0, 1),
        Some(b'-') => (1, 0),
        _ => (1, 1),
    }
}

/// The line a range covering `count` lines from `next` (the number of the first
/// line it would hold) is headed with: git heads an empty range with the line
/// before it.
fn header_start(next: usize, count: usize) -> usize {
    if count == 0 {
        next.saturating_sub(1)
    } else {
        next
    }
}

/// A position in the diff: a hunk index and a unit index within it. Positions
/// order hunk-major; `(h, hunk_len)` and `(h + 1, 0)` name the same boundary, and
/// the API only ever produces the latter.
pub type Pos = (usize, usize);

impl SplitDiff {
    /// Parse one file's unified diff. Lines before the first hunk are preamble.
    pub fn parse(diff: &str) -> SplitDiff {
        let mut preamble = String::new();
        let mut hunks: Vec<Hunk> = Vec::new();
        for raw in diff.split_inclusive('\n') {
            if raw.starts_with("@@") {
                hunks.push(Hunk {
                    header: raw.to_string(),
                    ranges: parse_header(raw),
                    units: Vec::new(),
                });
            } else if let Some(hunk) = hunks.last_mut() {
                match hunk.units.last_mut() {
                    Some(last) if raw.starts_with('\\') => last.push_str(raw),
                    _ => hunk.units.push(raw.to_string()),
                }
            } else {
                preamble.push_str(raw);
            }
        }
        hunks.retain(|h| !h.units.is_empty());
        SplitDiff { preamble, hunks }
    }

    /// The position one past the last unit.
    pub fn end(&self) -> Pos {
        (self.hunks.len(), 0)
    }

    fn normalize(&self, (h, u): Pos) -> Pos {
        if h < self.hunks.len() && u >= self.hunks[h].units.len() {
            (h + 1, 0)
        } else {
            (h, u)
        }
    }

    /// The end of the hunk `pos` is in.
    fn hunk_end(&self, (h, _): Pos) -> Pos {
        (h + 1, 0)
    }

    /// Whether the hunk `pos` is in may be cut at a line boundary.
    fn cuttable(&self, (h, _): Pos) -> bool {
        self.hunks[h].ranges.is_some()
    }

    /// Render the units in `[from, to)` as a diff: the preamble, then each hunk
    /// they touch — whole hunks under their own header, cut ones re-headered.
    pub fn render(&self, from: Pos, to: Pos) -> String {
        let mut out = self.preamble.clone();
        for (h, hunk) in self.hunks.iter().enumerate() {
            let a = if h == from.0 {
                from.1
            } else if h > from.0 {
                0
            } else {
                continue;
            };
            let b = if h == to.0 {
                to.1
            } else if h < to.0 {
                hunk.units.len()
            } else {
                break;
            };
            if a >= b {
                continue;
            }
            match &hunk.ranges {
                Some(r) if !(a == 0 && b == hunk.units.len()) => {
                    // The first old/new line the hunk holds (an empty range is
                    // headed with the line before it), advanced past the units
                    // this part leaves out.
                    let mut old = if r.old_count == 0 {
                        r.old_start + 1
                    } else {
                        r.old_start
                    };
                    let mut new = if r.new_count == 0 {
                        r.new_start + 1
                    } else {
                        r.new_start
                    };
                    for unit in &hunk.units[..a] {
                        let (o, n) = unit_counts(unit);
                        old += o;
                        new += n;
                    }
                    let (oc, nc) = hunk.units[a..b]
                        .iter()
                        .map(|u| unit_counts(u))
                        .fold((0, 0), |(x, y), (o, n)| (x + o, y + n));
                    out.push_str(&format!(
                        "@@ -{},{oc} +{},{nc} @@{}",
                        header_start(old, oc),
                        header_start(new, nc),
                        r.tail
                    ));
                }
                _ => out.push_str(&hunk.header),
            }
            for unit in &hunk.units[a..b] {
                out.push_str(unit);
            }
        }
        out
    }

    /// Every unit boundary strictly after `from` up to and including `to`, hunk
    /// boundaries only when `hunks_only`, in order.
    fn ends(&self, from: Pos, to: Pos, hunks_only: bool) -> Vec<Pos> {
        let mut out = Vec::new();
        let mut pos = from;
        while pos < to {
            pos = if hunks_only {
                self.hunk_end(pos)
            } else {
                self.normalize((pos.0, pos.1 + 1))
            };
            out.push(pos.min(to));
        }
        out
    }
}

/// The longest prefix of `ends` whose part from `from` fits, with its size, or
/// `None` when not even the first does: doubling then bisecting, so a part costs
/// a logarithmic number of measurements rather than one per unit.
fn longest(
    from: Pos,
    ends: &[Pos],
    fits: &mut impl FnMut(Pos, Pos) -> Option<usize>,
) -> Option<(Pos, usize)> {
    let first = fits(from, *ends.first()?)?;
    let mut best = (0, first);
    let mut over = ends.len();
    let mut step = 1;
    while best.0 + step < ends.len() {
        let i = best.0 + step;
        match fits(from, ends[i]) {
            Some(chars) => {
                best = (i, chars);
                step *= 2;
            }
            None => {
                over = i;
                break;
            }
        }
    }
    while over - best.0 > 1 {
        let mid = best.0 + (over - best.0) / 2;
        match fits(from, ends[mid]) {
            Some(chars) => best = (mid, chars),
            None => over = mid,
        }
    }
    Some((ends[best.0], best.1))
}

/// One part of a split diff: its span, its rendered diff, and its measured size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub from: Pos,
    pub to: Pos,
    pub diff: String,
    pub chars: usize,
}

/// Split `diff` into consecutive parts whose prompts, as `measure` sizes the
/// rendered part diff, are each at most `max`. Returns `None` when some single
/// line does not fit even alone (or a hunk that cannot be cut does not), so the
/// caller refuses the file rather than drop part of it.
pub fn pack(
    diff: &SplitDiff,
    max: usize,
    mut measure: impl FnMut(&str) -> usize,
) -> Option<Vec<Part>> {
    let mut fits = |from: Pos, to: Pos| {
        let chars = measure(&diff.render(from, to));
        (chars <= max).then_some(chars)
    };
    let end = diff.end();
    let mut parts = Vec::new();
    let mut from = (0, 0);
    while from < end {
        let hunk_ends = diff.ends(from, end, true);
        let (to, chars) = match longest(from, &hunk_ends, &mut fits) {
            Some((to, chars)) if to == end => (to, chars),
            Some((to, chars)) => {
                // Stop at the hunk boundary when the next hunk fits a part of its
                // own; otherwise that hunk will be cut anyway, so fill this part
                // with as many of its lines as fit.
                let next = diff.hunk_end(to);
                if fits(to, next).is_some() || !diff.cuttable(to) {
                    (to, chars)
                } else {
                    let lines = diff.ends(to, next, false);
                    match longest(from, &lines, &mut fits) {
                        Some((cut, more)) if cut < next => (cut, more),
                        _ => (to, chars),
                    }
                }
            }
            None if diff.cuttable(from) => {
                let lines = diff.ends(from, diff.hunk_end(from), false);
                longest(from, &lines, &mut fits)?
            }
            None => return None,
        };
        parts.push(Part {
            from,
            to,
            diff: diff.render(from, to),
            chars,
        });
        from = to;
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREAMBLE: &str = "diff --git a/t.md b/t.md\nindex 1..2 100644\n--- a/t.md\n+++ b/t.md\n";

    /// A diff of `n` one-line replacement hunks, ten lines apart.
    fn many_hunks(n: usize) -> String {
        let mut d = PREAMBLE.to_string();
        for i in 0..n {
            let line = 10 * i + 5;
            d.push_str(&format!(
                "@@ -{line},1 +{line},1 @@ table\n-| row {i} | old |\n+| row {i} | new |\n"
            ));
        }
        d
    }

    /// A prompt is 100 chars of overhead plus the part's diff.
    fn size(diff: &str) -> usize {
        100 + diff.chars().count()
    }

    #[test]
    fn rendering_the_whole_span_reproduces_the_diff() {
        let raw = many_hunks(5);
        let d = SplitDiff::parse(&raw);
        assert_eq!(d.render((0, 0), d.end()), raw);
    }

    #[test]
    fn many_small_hunks_pack_into_maximal_parts_cut_only_at_hunk_boundaries() {
        let raw = many_hunks(200);
        let d = SplitDiff::parse(&raw);
        let max = 2_000;
        let parts = pack(&d, max, size).unwrap();
        assert!(parts.len() > 2, "{} parts", parts.len());
        let mut body = String::new();
        for (i, part) in parts.iter().enumerate() {
            assert!(part.chars <= max);
            assert_eq!(part.chars, size(&part.diff));
            assert!(part.diff.starts_with(PREAMBLE));
            // Whole hunks only: every header is one from the original diff.
            for line in part.diff.lines().filter(|l| l.starts_with("@@")) {
                assert!(raw.contains(&format!("{line}\n")), "{line}");
            }
            assert_eq!(part.to.1, 0, "part {i} ends mid-hunk");
            body.push_str(&part.diff[PREAMBLE.len()..]);
            // No two adjacent parts could share one prompt.
            if let Some(next) = parts.get(i + 1) {
                assert!(size(&d.render(part.from, next.to)) > max);
            }
        }
        // Every hunk is carried exactly once, in order.
        assert_eq!(body, raw[PREAMBLE.len()..]);
    }

    #[test]
    fn a_hunk_too_big_for_any_part_is_cut_at_lines_with_true_line_numbers() {
        let mut raw = PREAMBLE.to_string();
        raw.push_str("@@ -1,3 +1,62 @@ fn top()\n ctx a\n");
        for i in 0..60 {
            raw.push_str(&format!("+added line number {i:03}\n"));
        }
        raw.push_str("-gone\n ctx b\n\\ No newline at end of file\n");
        let d = SplitDiff::parse(&raw);
        let max = 600;
        let parts = pack(&d, max, size).unwrap();
        assert!(parts.len() >= 3, "{} parts", parts.len());
        let mut body = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            assert!(part.chars <= max);
            if let Some(next) = parts.get(i + 1) {
                assert!(size(&d.render(part.from, next.to)) > max);
            }
            let mut lines = part.diff[PREAMBLE.len()..].lines();
            let header = lines.next().unwrap();
            assert!(header.ends_with(" @@ fn top()"), "{header}");
            // Counting from the header's new start, every added line lands on
            // its true file line (`ctx a` is line 1, added line `i` is `i + 2`).
            let mut new_line: usize = header
                .split('+')
                .nth(1)
                .and_then(|r| r.split(',').next())
                .unwrap()
                .parse()
                .unwrap();
            for line in part.diff[PREAMBLE.len()..].lines().skip(1) {
                if let Some(n) = line.strip_prefix("+added line number ") {
                    assert_eq!(new_line, n.parse::<usize>().unwrap() + 2, "{header}");
                }
                if !line.starts_with('-') && !line.starts_with('\\') {
                    new_line += 1;
                }
            }
            body.extend(lines.map(str::to_string));
        }
        let original: Vec<String> = raw[PREAMBLE.len()..]
            .lines()
            .skip(1)
            .map(str::to_string)
            .collect();
        assert_eq!(body, original);
        // The no-newline marker stays with the line it qualifies.
        let last = &parts.last().unwrap().diff;
        assert!(
            last.ends_with(" ctx b\n\\ No newline at end of file\n"),
            "{last}"
        );
    }

    #[test]
    fn a_cut_hunk_is_re_headered_for_exactly_its_lines() {
        let raw = format!("{PREAMBLE}@@ -10,3 +10,4 @@\n a\n-b\n+c\n+d\n e\n");
        let d = SplitDiff::parse(&raw);
        assert_eq!(
            d.render((0, 0), (0, 2)),
            format!("{PREAMBLE}@@ -10,2 +10,1 @@\n a\n-b\n")
        );
        assert_eq!(
            d.render((0, 2), (1, 0)),
            format!("{PREAMBLE}@@ -12,1 +11,3 @@\n+c\n+d\n e\n")
        );
        assert_eq!(
            d.render((0, 2), (0, 4)),
            format!("{PREAMBLE}@@ -11,0 +11,2 @@\n+c\n+d\n")
        );
    }

    #[test]
    fn a_hunk_that_fits_alone_starts_its_own_part_rather_than_being_cut() {
        // Small hunk, then a hunk that fits a part alone but not beside the first.
        let mut raw = format!("{PREAMBLE}@@ -1,1 +1,1 @@\n-x\n+y\n@@ -50,1 +50,20 @@\n-z\n");
        for i in 0..20 {
            raw.push_str(&format!("+line {i:02}\n"));
        }
        let d = SplitDiff::parse(&raw);
        let whole_second = size(&d.render((1, 0), d.end()));
        let parts = pack(&d, whole_second, size).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].to, (1, 0));
    }

    #[test]
    fn a_single_line_over_the_bound_refuses_the_whole_diff() {
        let raw = format!(
            "{PREAMBLE}@@ -1,1 +1,2 @@\n-a\n+{}\n+b\n",
            "x".repeat(1_000)
        );
        assert_eq!(pack(&SplitDiff::parse(&raw), 600, size), None);
    }

    #[test]
    fn an_uncuttable_hunk_over_the_bound_refuses_the_whole_diff() {
        let mut raw = format!("{PREAMBLE}@@ garbled @@\n");
        for i in 0..40 {
            raw.push_str(&format!("+line {i:02}\n"));
        }
        assert_eq!(pack(&SplitDiff::parse(&raw), 400, size), None);
    }

    #[test]
    fn the_split_is_deterministic() {
        let d = SplitDiff::parse(&many_hunks(300));
        assert_eq!(pack(&d, 3_000, size), pack(&d, 3_000, size));
    }
}
