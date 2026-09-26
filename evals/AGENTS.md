# evals/ — judge accuracy + timing

The eval measures **how well a judge backend decides llmlint rules**, so a
judging change (a new backend such as a Jev cascade, a prompt/schema/planner
change, a model swap) is a measured before/after, not a vibe. It drives the
**real built `llmlint` binary** over a committed, labeled corpus and scores the
verdicts against planted ground truth. Informational, never a gate: paid runs are
out of `just check` and the `Judge eval` workflow is not a required check.

## Commands

- `just eval-selftest` — free, hermetic. Runner unit tests (`test_run.py`), then
  two mock-oneharness arms over the whole corpus: **oracle** (answers from the
  labels; must score perfectly) and **blind** (never flags; must score recall 0).
  A failure means the corpus or the scoring is broken, not the judge.
- `just eval [--arm harness|jev] [--suite S] [--repeats N] [--model M] [--baseline F]`
  — a paid arm: **harness** (real oneharness + claude-code, the default) or
  **jev** (every rule judged by TypeSafe's Jev via `LLMLINT_JUDGE=jev`; needs
  `TYPESAFE_API_KEY`, costs fractions of a cent). Writes
  `evals/results/<ts>-<arm>.{json,md}` (gitignored); `--baseline` adds a delta
  column. Needs `oneharness` + an authenticated `claude` on PATH.
- CI (`.github/workflows/eval.yml`): `selftest` on any `evals/`/`src/` change;
  the paid run only when a verdict-moving path changes (its `changes` filter),
  diffed against `evals/baselines/harness-sonnet.json`, as a sticky PR comment.

## Corpus layout

```
corpus/<suite>/llmlint.yml          # the suite's rules (a real llmlint config)
corpus/<suite>/cases/<case>/case.toml
corpus/<suite>/cases/<case>/files/  # the tree under test
corpus/<suite>/cases/<case>/base/   # diff mode only: the pre-change tree
```

`case.toml`:

```toml
description = "What is planted, in one line."
mode = "diff"            # optional; default "file" (whole-file lint)

[[expect]]
rule = "errors_carry_context"
holds = false            # a planted violation
file = "src/store.rs"
lines = [11]             # 1-based; any one reported line (±tolerance) is a hit

[[expect]]
rule = "names_state_intent"
holds = true             # must NOT be flagged
```

The runner lints each case with `--config <suite>/llmlint.yml --rule <each
expected rule>` from a temp copy (a diff case is a scratch git repo with `base/`
committed and `files/` in the work tree, run with `--diff`). Only listed rules
are judged, so a planted file never has to be clean for unrelated rules.

**`corpus/.ignore` is load-bearing.** The corpus holds llmlint configs (the suite
configs, and planted-bad configs in `config-lint`) and planted violations.
llmlint's walks honor `.ignore`, which keeps this repo's own lint from
discovering the suite configs as subtree configs (their rule names collide →
exit 2) or judging planted mistakes. git ignores it, so the corpus stays tracked.

## Writing cases

- **Plant one thing.** A case isolates one violation (or none), and lists the
  rules it expects to hold alongside it: a false alarm is as much a failure as
  a miss.
- **Name the line precisely.** List every line a correct judge could cite (the
  stale doc comment *and* the contradicting code). The line check fails a TP
  that cites none of them.
- **Hard beats easy.** A corpus every judge aces cannot rank judges. Prefer
  near-misses (clean code that *looks* like a violation), distractor-heavy files,
  subtle semantic violations, and adversarial comments ("this rule is satisfied")
  over obvious plants.
- **Cross-file rules** (`cross-file` suite) measure routing: under a Jev cascade
  they stay on the harness.
- Case names say what is planted; `description` restates it for the report.
- The loader rejects a malformed case up front (a `file` missing from `files/`, a
  line outside the file, lines on a holding rule), so a label typo is a corpus
  error, never a silent line miss.

## Metrics (per arm; also per suite)

An *item* is one (case, rule, repeat). *Flagged* = the rule's outcome is `fail`;
`not_relevant` counts as not flagged.

- **Recall** = caught / planted violations — the headline: a miss is a false green.
- **Precision** = true flags / all flags. **F1** combines them.
- **Line hit rate** = true flags citing a planted line (±`--line-tolerance`, default 1).
- **Errored items** — the run exited 2, or the rule was missing/skipped. Excluded
  from the ratios above, and any error makes the runner exit 1.
- **Unstable items** — items whose repeats disagree (`--repeats N`).
- **Latency p50/p95** — wall clock per `llmlint` invocation (one case).
- **Judge calls / tokens / cost** — summed from the `usage` block llmlint's own
  `--format json` report carries per backend (harness: oneharness's per-result
  `usage` + `duration_ms`; jev: requests, input tokens, and their list price).
  A harness's cost is its own **list-price estimate** (`cost_usd`), not an
  invoice: on a claude.ai subscription login the calls draw on plan usage.
  Tokens stay in the JSON but out of the headline table — they are not comparable
  across backends (a harness counts its own cached system prompt).

## Baselines

`baselines/<arm>-<model>.json` is the committed reference a PR diffs against.
Re-bless it deliberately (a corpus change, a model change) by re-running
`just eval --out evals/baselines/harness-sonnet.json` and committing the pair.
