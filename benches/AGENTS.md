# AGENTS — benches

The informational performance suite. These targets **measure, they do not
gate**: timings are noisy on shared CI runners, so the `Performance` workflow
reports numbers on a PR rather than blocking it. The hard gate is `just check`.

- Bench the **pure engine surface** (`configfs::parse`, `plan::build`,
  `template::render`, `schema::build`, `vote::tally`, `report::Report`) so the
  numbers track what the binary actually runs. The `oneharness` subprocess — the
  network/model boundary — is deliberately excluded here; `scripts/bench.sh`
  covers the end-to-end CLI cost including it.
- Load fixtures from the **bundled assets** (`io::assets`: the `init` starter
  config, the `config-lint` plugin, the default template) once, outside every
  timed loop; never let asset parsing leak into a measurement. These are the
  realistic floor.
- Where a stage consumes its input (e.g. `plan::build` takes the `Vec` by
  value), clone in Criterion's `iter_batched` setup so the clone is not timed.
- Synthetic `*_scaling` groups chart how cost grows along each axis (rule count,
  judge fan-out, file count); keep the generators in `support/` — a subdirectory
  so cargo's bench auto-discovery never treats the module as a target — pulled in
  via `#[path]`.
- `engine_allocs` reports exact allocator tallies, not time: a plain `main`, no
  Criterion, deterministic output for a given commit. Keep it that way — no
  timing, no randomness, no I/O inside a measured closure.
- `cargo clippy --all-targets` type-checks these targets, so they cannot rot
  silently; keep them warning-clean. `harness = false` keeps them out of the
  test runner and coverage.

## Running

- `just bench` / `just bench-compare` — Criterion timings + base-vs-current diff.
- `just bench-allocs` — deterministic allocation counts.
- `just bench-cli` — end-to-end CLI latency (hyperfine; drives the real release
  binary against the mock-oneharness fixture).
- `just bench-instructions` — deterministic CLI instruction counts (cachegrind;
  Linux-only, needs valgrind).
- `just profile …` — sampling/callgrind profiler to find bottlenecks.
