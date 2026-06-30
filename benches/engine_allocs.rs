//! Deterministic allocation report for the engine hot paths.
//!
//! Not a statistical benchmark: a counting global allocator tallies allocator
//! calls and requested bytes for one run of each pure stage (config parse, plan,
//! template render, schema build, vote tally, report), then prints a markdown
//! table. The counts are exact and stable for a given commit, so two runs are
//! directly comparable — in CI or by eye — without warmups or statistics. They
//! surface allocator pressure, which the wall-clock numbers in `benches/engine.rs`
//! cannot attribute.
//!
//! `harness = false` with a plain `main` keeps libtest, Criterion, nextest, and
//! coverage away from this target (it is measured, not gated). The `--bench`
//! argument cargo passes is deliberately ignored.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

use llmlint::domain::config::Config;
use llmlint::domain::{plan, report, schema, template, vote};
use llmlint::io::configfs;

#[path = "support/mod.rs"]
mod support;

/// The system allocator wrapped with relaxed atomic tallies. A `realloc` counts
/// as one call plus only the grown bytes, so `BYTES` tracks total memory
/// requested without double-counting moves; frees are not tracked.
struct CountingAlloc;

static CALLS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        CALLS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        CALLS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(
            new_size.saturating_sub(layout.size()) as u64,
            Ordering::Relaxed,
        );
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// Allocator calls and bytes requested while running `f` (including dropping
/// its result).
fn measure<T>(f: impl FnOnce() -> T) -> (u64, u64) {
    let calls = CALLS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    black_box(f());
    (
        CALLS.load(Ordering::Relaxed) - calls,
        BYTES.load(Ordering::Relaxed) - bytes,
    )
}

fn main() {
    let cfg = Config::default();
    let tmpl = support::example_template();
    let rule_specs = support::example_rule_specs();
    let files = support::example_files();

    // Flush lazy one-time initialization (parser tables, interned statics) out
    // of the measured calls, so every row reflects steady-state cost.
    for (_, text) in support::example_configs() {
        black_box(configfs::parse(text, "warmup").ok());
    }
    black_box(template::render(tmpl, &rule_specs, &files, &[], true, false, false).ok());

    println!("| operation | case | allocator calls | bytes requested |");
    println!("|---|---|---:|---:|");

    for (name, text) in support::example_configs() {
        let (calls, bytes) = measure(|| configfs::parse(text, "alloc").unwrap());
        println!("| config_parse | {name} | {calls} | {bytes} |");
    }

    let (calls, bytes) =
        measure(|| plan::build(&cfg, tmpl, 20, support::synthetic_resolved(100, 3)));
    println!("| plan_build | 100 rules × 3 judges | {calls} | {bytes} |");

    let (calls, bytes) =
        measure(|| template::render(tmpl, &rule_specs, &files, &[], true, false, false).unwrap());
    println!("| template_render | examples | {calls} | {bytes} |");

    let names = support::synthetic_rule_names(100);
    let schema_rules = support::synthetic_schema_rules(&names);
    let (calls, bytes) = measure(|| schema::build(&schema_rules));
    println!("| schema_build | 100 rules | {calls} | {bytes} |");

    let verdicts = support::judge_verdicts(9, true);
    let (calls, bytes) = measure(|| vote::tally("rule", &verdicts));
    println!("| vote_tally | 9 judges (dissent) | {calls} | {bytes} |");

    let (calls, bytes) =
        measure(|| report::Report::new(support::outcomes(100), vec![]).to_human(1, false));
    println!("| report:human | 100 outcomes | {calls} | {bytes} |");

    let (calls, bytes) = measure(|| report::Report::new(support::outcomes(100), vec![]).to_json());
    println!("| report:json | 100 outcomes | {calls} | {bytes} |");
}
