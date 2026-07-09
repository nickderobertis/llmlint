//! The token-weighted batching cost model and the optimizer that minimizes it.
//!
//! # What we minimize, and why lexicographically
//!
//! A judge call bills its whole prompt once, but **every rule in the batch reads
//! it**. So there are two token quantities, and at a *fixed batch count* they don't
//! trade off — the second only refines among layouts tied on the first:
//!
//! - **`billed`** — Σ over batches of the batch's file-token union. A file's content
//!   is re-billed in every batch that carries it, so this is the money (up to the
//!   fixed per-call template + per-rule description constants, which a fixed batch
//!   count and fixed rule set make invariant).
//! - **`per_rule`** — Σ over *rules* of their batch's union weight, i.e.
//!   Σ_batches `n(batch) × union(batch)`. Each rule is judged against its whole
//!   batch's files; a big union shared by many rules is read many times. Minimizing
//!   this gives every rule the most focused prompt (best judgment) — the quality
//!   axis. It never costs more, because the batch count is fixed: you can't split a
//!   rule into its own call to shrink its prompt.
//!
//! [`Objective`] orders `(billed, per_rule)` lexicographically: pick the cheapest
//! layout, and among the cheapest the most per-rule-focused one.
//!
//! # Achieving the true minimum
//!
//! [`Model::assign`] is **exact** — a branch-and-bound search that provably returns
//! a minimum-[`Objective`] layout — whenever the search fits a node budget (which
//! covers the realistic multi-batch sizes: a single batch is a no-op, and the
//! search space is only nontrivial when rules outnumber `batch_size`). Past the
//! budget it falls back to a deterministic greedy + local-search heuristic. The
//! test suite brute-forces the optimum independently and asserts `assign` achieves
//! it across a broad table of shapes.
//!
//! Pure: it operates on abstract file ids + weights; the io/command layer supplies
//! real token weights and the plan maps rules/files to ids.

use std::cmp::Ordering;

/// The lexicographic batching objective — see the module docs. Lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Objective {
    /// Total file tokens billed (Σ over batches of the batch's union weight).
    pub billed: usize,
    /// Per-rule file-token exposure (Σ over batches of `size × union weight`).
    pub per_rule: usize,
    /// Batch-size spread (Σ of squared batch sizes) — a token-neutral tertiary
    /// tiebreak that prefers *balanced* batches when the token objectives tie
    /// (e.g. many rules over one shared file). Lower = more even, which parallelizes
    /// better and loses fewer rules if one call fails. Never overrides tokens.
    pub spread: usize,
}

impl Objective {
    fn key(self) -> (usize, usize, usize) {
        (self.billed, self.per_rule, self.spread)
    }
}

impl Ord for Objective {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl PartialOrd for Objective {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The cost model over abstract items (rules) and files. Each item carries the
/// file ids it needs (its effective scope); `file_weight[id]` is a file's token
/// weight. Files are `0..file_weight.len()`.
#[derive(Debug, Clone)]
pub struct Model {
    /// Per item, the sorted-unique file ids it needs.
    items: Vec<Vec<usize>>,
    /// Per file id, its token weight.
    file_weight: Vec<usize>,
}

/// The largest file-id space the exact search will use a bitmask for; above it the
/// search would need a wider set, so we defer to the heuristic. Realistic batches
/// span far fewer than 128 files.
const EXACT_MAX_FILES: usize = 128;
/// Node budget for the exact branch-and-bound before falling back to the heuristic.
const EXACT_NODE_BUDGET: u64 = 300_000;

impl Model {
    /// Build a model. `items[i]` lists item `i`'s file ids; `file_weight[f]` is file
    /// `f`'s weight. Ids in each item are deduped and sorted defensively.
    pub fn new(mut items: Vec<Vec<usize>>, file_weight: Vec<usize>) -> Self {
        for it in &mut items {
            it.sort_unstable();
            it.dedup();
        }
        Model { items, file_weight }
    }

    /// The token weight of the union of `batch`'s items' files.
    fn union_weight(&self, batch: &[usize]) -> usize {
        let mut seen = vec![false; self.file_weight.len()];
        let mut total = 0;
        for &i in batch {
            for &f in &self.items[i] {
                if !seen[f] {
                    seen[f] = true;
                    total += self.file_weight[f];
                }
            }
        }
        total
    }

    /// The [`Objective`] of a complete assignment (a list of batches of item ids).
    pub fn objective(&self, batches: &[Vec<usize>]) -> Objective {
        let mut billed = 0;
        let mut per_rule = 0;
        let mut spread = 0;
        for b in batches {
            let w = self.union_weight(b);
            billed += w;
            per_rule += b.len() * w;
            spread += b.len() * b.len();
        }
        Objective {
            billed,
            per_rule,
            spread,
        }
    }

    /// Partition `0..n` into exactly `batch_count` non-empty batches, each of size
    /// `<= batch_size`, minimizing the [`Objective`]. Deterministic. Exact within
    /// the node budget, else a deterministic heuristic.
    ///
    /// `batch_count` must be `ceil(n / batch_size)` (the forced minimum number of
    /// calls); the returned batches are canonicalized (each sorted ascending, the
    /// list sorted by first element).
    pub fn assign(&self, batch_count: usize, batch_size: usize) -> Vec<Vec<usize>> {
        let n = self.items.len();
        if n == 0 {
            return Vec::new();
        }
        if batch_count <= 1 {
            return vec![(0..n).collect()];
        }
        if let Some(batches) = self.assign_exact(batch_count, batch_size) {
            return batches;
        }
        self.assign_heuristic(batch_count, batch_size)
    }

    /// Exact branch-and-bound over canonical (restricted-growth) assignments.
    /// Returns `None` if the file space is too wide for the bitmask or the node
    /// budget is exhausted, so the caller falls back to the heuristic.
    fn assign_exact(&self, batch_count: usize, batch_size: usize) -> Option<Vec<Vec<usize>>> {
        let n = self.items.len();
        if self.file_weight.len() > EXACT_MAX_FILES {
            return None;
        }
        // Precompute each item's file bitmask.
        let masks: Vec<u128> = self
            .items
            .iter()
            .map(|it| it.iter().fold(0u128, |m, &f| m | (1u128 << f)))
            .collect();

        let mut best: Option<(Objective, Vec<Vec<usize>>)> = None;
        let mut batches: Vec<Vec<usize>> = Vec::with_capacity(batch_count);
        let mut unions: Vec<u128> = Vec::with_capacity(batch_count);
        let mut billed = 0usize;
        let mut nodes = 0u64;

        self.search(
            0,
            n,
            batch_count,
            batch_size,
            &masks,
            &mut batches,
            &mut unions,
            &mut billed,
            &mut best,
            &mut nodes,
        )?;

        best.map(|(_, b)| canonicalize(b))
    }

    /// The recursive placement. Returns `None` if the node budget was blown (so the
    /// whole exact attempt aborts to the heuristic).
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        i: usize,
        n: usize,
        batch_count: usize,
        batch_size: usize,
        masks: &[u128],
        batches: &mut Vec<Vec<usize>>,
        unions: &mut Vec<u128>,
        billed: &mut usize,
        best: &mut Option<(Objective, Vec<Vec<usize>>)>,
        nodes: &mut u64,
    ) -> Option<()> {
        *nodes += 1;
        if *nodes > EXACT_NODE_BUDGET {
            return None;
        }
        // Branch-and-bound: `billed` only grows as items are added, so a partial
        // already at/over the incumbent's `billed` can only tie or lose on the
        // primary — but a tie can still improve `per_rule`, so prune only when
        // strictly greater.
        if let Some((obj, _)) = best {
            if *billed > obj.billed {
                return Some(());
            }
        }
        if i == n {
            if batches.len() == batch_count {
                let obj = self.objective(batches);
                if best.as_ref().is_none_or(|(b, _)| obj < *b) {
                    *best = Some((obj, batches.clone()));
                }
            }
            return Some(());
        }
        // Feasibility: the remaining items (including i) must be enough to open the
        // still-needed batches.
        let remaining = n - i;
        let needed = batch_count - batches.len();
        if remaining < needed {
            return Some(());
        }
        // Place item i into an existing batch that has room.
        for b in 0..batches.len() {
            if batches[b].len() >= batch_size {
                continue;
            }
            let added = weight_of(masks[i] & !unions[b], &self.file_weight);
            batches[b].push(i);
            unions[b] |= masks[i];
            *billed += added;
            self.search(
                i + 1,
                n,
                batch_count,
                batch_size,
                masks,
                batches,
                unions,
                billed,
                best,
                nodes,
            )?;
            *billed -= added;
            batches[b].pop();
            // Recompute the batch union from its remaining members to undo any bit
            // item i introduced (an item's bits may overlap others, so a plain XOR
            // would be wrong).
            unions[b] = batches[b].iter().fold(0u128, |m, &x| m | masks[x]);
        }
        // Or open a new batch with item i (restricted growth: only the next slot).
        if batches.len() < batch_count {
            let added = weight_of(masks[i], &self.file_weight);
            batches.push(vec![i]);
            unions.push(masks[i]);
            *billed += added;
            self.search(
                i + 1,
                n,
                batch_count,
                batch_size,
                masks,
                batches,
                unions,
                billed,
                best,
                nodes,
            )?;
            *billed -= added;
            batches.pop();
            unions.pop();
        }
        Some(())
    }

    /// Deterministic greedy + local-search fallback for inputs too large to search
    /// exactly. Produces exactly `batch_count` non-empty batches within the cap and
    /// minimizes the [`Objective`] locally (never worse than the greedy seed).
    fn assign_heuristic(&self, batch_count: usize, batch_size: usize) -> Vec<Vec<usize>> {
        let n = self.items.len();
        // Seed: widest scope first, into the batch of least marginal billed weight,
        // tie-broken toward the emptier batch then the lower index.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            self.union_weight(&[b])
                .cmp(&self.union_weight(&[a]))
                .then(a.cmp(&b))
        });
        let mut batches: Vec<Vec<usize>> = vec![Vec::new(); batch_count];
        for &i in &order {
            let mut best: Option<(usize, usize, usize, usize)> = None;
            for (bi, batch) in batches.iter().enumerate() {
                if batch.len() >= batch_size {
                    continue;
                }
                let marginal = {
                    let mut with = batch.clone();
                    with.push(i);
                    self.union_weight(&with) - self.union_weight(batch)
                };
                let key = (marginal, self.union_weight(batch), batch.len(), bi);
                if best.is_none_or(|k| key < k) {
                    best = Some(key);
                }
            }
            // A slot always exists: batch_count * batch_size >= n.
            batches[best.expect("a non-full batch always exists").3].push(i);
        }
        // Fill any empty batch (possible only if greedy stranded one) by moving a
        // rule from the largest batch, so exactly `batch_count` non-empty remain.
        self.repair_empty(&mut batches);
        self.local_search(&mut batches, batch_size);
        canonicalize(batches)
    }

    /// Ensure no batch is empty by shifting a rule from the largest batch into it.
    fn repair_empty(&self, batches: &mut [Vec<usize>]) {
        while let Some(empty) = batches.iter().position(|b| b.is_empty()) {
            let largest = batches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.len() > 1)
                .max_by_key(|(_, b)| b.len())
                .map(|(i, _)| i);
            let Some(src) = largest else { break };
            let moved = batches[src].pop().expect("largest batch is non-empty");
            batches[empty].push(moved);
        }
    }

    /// Local search to convergence: repeatedly apply the first single-item move (or,
    /// failing that, pairwise swap) that strictly lowers the [`Objective`], keeping
    /// every batch non-empty and within the cap, until no such step exists. Each
    /// step strictly decreases a non-negative integer objective, so it terminates;
    /// the guard caps pathological inputs defensively.
    fn local_search(&self, batches: &mut [Vec<usize>], batch_size: usize) {
        for _ in 0..1_000_000 {
            let base = self.objective(batches);
            if self.try_move(batches, batch_size, base) || self.try_swap(batches, base) {
                continue;
            }
            break;
        }
    }

    /// Apply the first strictly-improving single-item move; return whether one fired.
    fn try_move(&self, batches: &mut [Vec<usize>], batch_size: usize, base: Objective) -> bool {
        for from in 0..batches.len() {
            if batches[from].len() <= 1 {
                continue; // keep the source non-empty
            }
            for pos in 0..batches[from].len() {
                for to in 0..batches.len() {
                    if to == from || batches[to].len() >= batch_size {
                        continue;
                    }
                    let item = batches[from].remove(pos);
                    batches[to].push(item);
                    if self.objective(batches) < base {
                        return true;
                    }
                    batches[to].pop();
                    batches[from].insert(pos, item);
                }
            }
        }
        false
    }

    /// Apply the first strictly-improving pairwise swap (helps when both batches are
    /// at the cap, so a move can't fire); return whether one fired.
    fn try_swap(&self, batches: &mut [Vec<usize>], base: Objective) -> bool {
        for a in 0..batches.len() {
            for b in (a + 1)..batches.len() {
                for pa in 0..batches[a].len() {
                    for pb in 0..batches[b].len() {
                        let ia = batches[a][pa];
                        let ib = batches[b][pb];
                        batches[a][pa] = ib;
                        batches[b][pb] = ia;
                        if self.objective(batches) < base {
                            return true;
                        }
                        batches[a][pa] = ia;
                        batches[b][pb] = ib;
                    }
                }
            }
        }
        false
    }
}

/// The token weight of the set bits in `mask`.
fn weight_of(mask: u128, weight: &[usize]) -> usize {
    let mut m = mask;
    let mut total = 0;
    while m != 0 {
        let bit = m.trailing_zeros() as usize;
        total += weight[bit];
        m &= m - 1;
    }
    total
}

/// Canonical form: sort each batch ascending, then sort batches by first element so
/// the layout is stable regardless of how it was discovered.
fn canonicalize(mut batches: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    for b in &mut batches {
        b.sort_unstable();
    }
    batches.retain(|b| !b.is_empty());
    batches.sort_by(|a, b| a.first().cmp(&b.first()));
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An independent, dead-simple brute force: enumerate *every* way to split
    /// `0..n` into exactly `batch_count` non-empty batches of size `<= batch_size`
    /// and return the minimum objective. No pruning, no heuristics — the trusted
    /// oracle the optimizer is checked against.
    fn brute_optimum(model: &Model, batch_count: usize, batch_size: usize) -> Objective {
        let n = model.items.len();
        let mut best: Option<Objective> = None;
        let mut batches: Vec<Vec<usize>> = Vec::new();
        fn rec(
            i: usize,
            n: usize,
            bc: usize,
            bs: usize,
            model: &Model,
            batches: &mut Vec<Vec<usize>>,
            best: &mut Option<Objective>,
        ) {
            if i == n {
                if batches.len() == bc {
                    let obj = model.objective(batches);
                    if best.is_none_or(|b| obj < b) {
                        *best = Some(obj);
                    }
                }
                return;
            }
            for b in 0..batches.len() {
                if batches[b].len() < bs {
                    batches[b].push(i);
                    rec(i + 1, n, bc, bs, model, batches, best);
                    batches[b].pop();
                }
            }
            if batches.len() < bc {
                batches.push(vec![i]);
                rec(i + 1, n, bc, bs, model, batches, best);
                batches.pop();
            }
        }
        rec(
            0,
            n,
            batch_count,
            batch_size,
            model,
            &mut batches,
            &mut best,
        );
        best.expect("at least one valid partition exists")
    }

    fn batch_count(n: usize, bs: usize) -> usize {
        n.div_ceil(bs)
    }

    /// Build a model from per-rule file-id lists with uniform weight 1.
    fn unit(items: &[&[usize]], num_files: usize) -> Model {
        Model::new(
            items.iter().map(|s| s.to_vec()).collect(),
            vec![1; num_files],
        )
    }

    /// Build a model with explicit per-file weights.
    fn weighted(items: &[&[usize]], weights: &[usize]) -> Model {
        Model::new(items.iter().map(|s| s.to_vec()).collect(), weights.to_vec())
    }

    /// Assert `assign` reaches the brute-force optimum, and that its own reported
    /// objective matches recomputation (canonical layout is valid).
    fn assert_optimal(model: &Model, bs: usize) {
        let n = model.items.len();
        let bc = batch_count(n, bs);
        let got = model.assign(bc, bs);
        // Structural: exactly `bc` non-empty batches, each within the cap, covering
        // every item once.
        assert_eq!(got.len(), bc, "batch count: {got:?}");
        let mut seen: Vec<usize> = got.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..n).collect::<Vec<_>>(), "coverage: {got:?}");
        assert!(
            got.iter().all(|b| !b.is_empty() && b.len() <= bs),
            "cap: {got:?}"
        );
        // Optimality: the achieved objective equals the brute-force minimum.
        let achieved = model.objective(&got);
        let optimum = brute_optimum(model, bc, bs);
        assert_eq!(achieved, optimum, "assign not optimal: {got:?}");
    }

    #[test]
    fn single_batch_is_a_trivial_no_op() {
        let m = unit(&[&[0], &[1], &[2]], 3);
        assert_eq!(m.assign(1, 20), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn one_rule_per_batch_when_cap_is_one() {
        let m = unit(&[&[0], &[1], &[2]], 3);
        assert_optimal(&m, 1);
    }

    #[test]
    fn shared_files_are_grouped_over_the_interleaved_order() {
        // The classic case: two scopes interleaved, cap 2. Optimum groups by scope.
        let m = unit(&[&[0], &[1], &[0], &[1]], 2);
        assert_optimal(&m, 2);
        let got = m.assign(2, 2);
        // Each batch is a single shared file -> billed 2 (vs 4 for the interleave).
        assert_eq!(m.objective(&got).billed, 2);
    }

    #[test]
    fn wide_rule_goes_in_the_smaller_batch_to_cut_per_rule_exposure() {
        // A={0,1,2} (wide) + four narrow ={0}. cap 3 -> 2 batches (3+2). Billed ties
        // at 4 for either split, but per_rule prefers A in the 2-batch so fewer
        // rules read {0,1,2}.
        let m = unit(&[&[0, 1, 2], &[0], &[0], &[0], &[0]], 3);
        assert_optimal(&m, 3);
        let got = m.assign(batch_count(5, 3), 3); // 2 batches (3 + 2)
                                                  // The wide rule (item 0) sits in the smaller batch.
        let wide = got.iter().find(|b| b.contains(&0)).unwrap();
        assert_eq!(wide.len(), 2, "wide rule in the 2-rule batch: {got:?}");
    }

    #[test]
    fn heavy_file_weight_dominates_grouping() {
        // File 0 is huge; files 1,2 tiny. Rules: A={0}, B={0}, C={1}, D={2}. cap 2.
        // Optimum keeps the two file-0 rules together so the heavy file is billed
        // once, not twice.
        let m = weighted(&[&[0], &[0], &[1], &[2]], &[100, 1, 1]);
        assert_optimal(&m, 2);
        let got = m.assign(2, 2);
        assert_eq!(
            m.objective(&got).billed,
            102,
            "heavy file billed once: {got:?}"
        );
    }

    #[test]
    fn all_disjoint_is_balanced_and_optimal() {
        let m = unit(&[&[0], &[1], &[2], &[3]], 4);
        assert_optimal(&m, 2);
    }

    #[test]
    fn all_identical_scope_is_optimal() {
        let m = unit(&[&[0, 1], &[0, 1], &[0, 1], &[0, 1]], 2);
        assert_optimal(&m, 2);
    }

    #[test]
    fn objective_is_lexicographic() {
        let a = Objective {
            billed: 4,
            per_rule: 8,
            spread: 2,
        };
        let b = Objective {
            billed: 4,
            per_rule: 6,
            spread: 100,
        };
        let c = Objective {
            billed: 3,
            per_rule: 100,
            spread: 100,
        };
        assert!(
            b < a,
            "lower per_rule wins on a billed tie, regardless of spread"
        );
        assert!(c < a, "lower billed wins regardless of the rest");
        assert!(c < b);
        // Spread only breaks a billed+per_rule tie.
        let balanced = Objective {
            billed: 2,
            per_rule: 21,
            spread: 221,
        };
        let packed = Objective {
            billed: 2,
            per_rule: 21,
            spread: 401,
        };
        assert!(balanced < packed, "balanced sizes win the token tie");
    }

    #[test]
    fn assignment_is_deterministic() {
        let m = unit(&[&[0], &[1], &[0], &[1], &[2]], 3);
        let bc = batch_count(5, 2);
        assert_eq!(m.assign(bc, 2), m.assign(bc, 2));
    }

    /// The exhaustive sweep: for a spread of item/file/weight shapes and every
    /// sensible batch size, `assign` must hit the brute-force optimum. This is the
    /// "minimum across all representative cases" guarantee.
    #[test]
    fn assign_is_optimal_across_a_broad_table() {
        // (items, num_files, weights or None for unit)
        type Case = (Vec<Vec<usize>>, usize, Option<Vec<usize>>);
        let cases: Vec<Case> = vec![
            // Disjoint scopes.
            (vec![vec![0], vec![1], vec![2], vec![3]], 4, None),
            // Two interleaved scopes.
            (
                vec![vec![0], vec![1], vec![0], vec![1], vec![0], vec![1]],
                2,
                None,
            ),
            // Wide + narrow mix.
            (
                vec![vec![0, 1, 2], vec![0], vec![1], vec![2], vec![0]],
                3,
                None,
            ),
            // Overlapping (not identical) scopes.
            (
                vec![vec![0, 1], vec![1, 2], vec![2, 3], vec![0, 3]],
                4,
                None,
            ),
            // Nested scopes.
            (
                vec![vec![0], vec![0, 1], vec![0, 1, 2], vec![0, 1, 2, 3]],
                4,
                None,
            ),
            // Heavy-file weights.
            (
                vec![vec![0], vec![0], vec![1], vec![2], vec![1]],
                3,
                Some(vec![50, 5, 1]),
            ),
            // A lone narrow rule among wide ones (the "don't strand it" case).
            (
                vec![
                    vec![0, 1, 2, 3],
                    vec![0, 1, 2, 3],
                    vec![4],
                    vec![0, 1, 2, 3],
                ],
                5,
                None,
            ),
            // Mixed sizes and a shared hot file.
            (
                vec![
                    vec![0, 1],
                    vec![0],
                    vec![2],
                    vec![0, 3],
                    vec![2, 3],
                    vec![4],
                ],
                5,
                Some(vec![10, 1, 8, 2, 1]),
            ),
            // Seven rules, three scopes.
            (
                vec![
                    vec![0],
                    vec![1],
                    vec![2],
                    vec![0],
                    vec![1],
                    vec![2],
                    vec![0],
                ],
                3,
                None,
            ),
        ];
        for (items, num_files, weights) in cases {
            let n = items.len();
            let model = match weights {
                Some(w) => Model::new(items.clone(), w),
                None => Model::new(items.clone(), vec![1; num_files]),
            };
            // Every batch size that yields a non-trivial (>1) and feasible layout.
            for bs in 1..=n {
                let bc = batch_count(n, bs);
                if bc < 1 {
                    continue;
                }
                let got = model.assign(bc, bs);
                let achieved = model.objective(&got);
                let optimum = brute_optimum(&model, bc, bs);
                assert_eq!(
                    achieved, optimum,
                    "suboptimal for items={items:?} bs={bs} -> {got:?} ({achieved:?} vs {optimum:?})"
                );
            }
        }
    }

    #[test]
    fn heuristic_fallback_is_valid_and_not_worse_than_balanced() {
        // Force the heuristic by exceeding the node budget with a wide, disjoint set
        // that the exact search can't finish. It must still return a valid layout
        // no worse than a naive balanced split.
        let n = 40;
        let items: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
        let model = Model::new(items, vec![1; n]);
        let bs = 3;
        let bc = batch_count(n, bs);
        let got = model.assign(bc, bs);
        assert_eq!(got.len(), bc);
        let mut seen: Vec<usize> = got.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..n).collect::<Vec<_>>());
        assert!(got.iter().all(|b| !b.is_empty() && b.len() <= bs));
        // Disjoint files -> billed is n regardless; the heuristic must not regress.
        assert_eq!(model.objective(&got).billed, n);
    }
}
