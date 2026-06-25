//! Criterion micro-benchmarks for llmlint's pure decision engine.
//!
//! These measure the in-process, CPU-bound work a single `llmlint` invocation
//! does *around* the external `oneharness` call (which is excluded here — it is
//! the network/model boundary `scripts/bench.sh` times end to end): parse the
//! YAML config, plan the judge runs, render the judge prompt, generate the
//! output schema, and aggregate the judges' verdicts into a report.
//!
//! The realistic floor uses the bundled assets (the `llmlint init` starter
//! config, the `config-lint` plugin, the default template) — the exact inputs a
//! default run processes, read once outside every timed loop. The `*_scaling`
//! groups chart how each stage grows with rule / judge / file count using
//! synthetic inputs.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};

use llmlint::domain::config::Config;
use llmlint::domain::{plan, report, schema, template, vote};
use llmlint::io::configfs;

#[path = "support/mod.rs"]
mod support;

/// Parse + deserialize the bundled config documents — the realistic floor.
fn bench_config_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_parse");
    for (name, text) in support::example_configs() {
        group.bench_with_input(BenchmarkId::from_parameter(name), text, |b, text| {
            b.iter(|| configfs::parse(black_box(text), "bench"));
        });
    }
    group.finish();
}

/// How config parse scales with rule count.
fn bench_config_parse_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_parse/synthetic");
    for n in [10usize, 100, 1000] {
        let text = support::synthetic_config_text(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &text, |b, text| {
            b.iter(|| configfs::parse(black_box(text), "bench"));
        });
    }
    group.finish();
}

/// Plan the judge runs for the example rule set (one agent, one file set).
fn bench_plan_build(c: &mut Criterion) {
    let cfg = Config::default();
    let tmpl = support::example_template();
    let resolved = support::example_resolved();
    c.bench_function("plan_build/examples", |b| {
        // `plan::build` consumes the `Vec`, so clone outside the timer.
        b.iter_batched(
            || resolved.clone(),
            |r| plan::build(black_box(&cfg), black_box(tmpl), 20, r),
            BatchSize::SmallInput,
        );
    });
}

/// How planning scales along its two axes: rule count (batching) and per-rule
/// judge count (the multi-judge fan-out emits one run per judge index).
fn bench_plan_build_scaling(c: &mut Criterion) {
    let cfg = Config::default();
    let tmpl = support::example_template();
    let mut group = c.benchmark_group("plan_build/scaling");
    for n in [10usize, 100, 1000] {
        let resolved = support::synthetic_resolved(n, 1);
        group.bench_with_input(BenchmarkId::new("rules", n), &resolved, |b, resolved| {
            b.iter_batched(
                || resolved.clone(),
                |r| plan::build(&cfg, tmpl, 20, r),
                BatchSize::SmallInput,
            );
        });
    }
    for j in [1u32, 3, 9] {
        let resolved = support::synthetic_resolved(50, j);
        group.bench_with_input(BenchmarkId::new("judges", j), &resolved, |b, resolved| {
            b.iter_batched(
                || resolved.clone(),
                |r| plan::build(&cfg, tmpl, 20, r),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Render the judge prompt from the default template for the example rules.
fn bench_template_render(c: &mut Criterion) {
    let tmpl = support::example_template();
    let rules = support::example_rule_specs();
    let files = support::example_files();
    c.bench_function("template_render/examples", |b| {
        b.iter(|| template::render(black_box(tmpl), black_box(&rules), black_box(&files)));
    });
}

/// How prompt rendering scales with the number of rules in a batch.
fn bench_template_render_scaling(c: &mut Criterion) {
    let tmpl = support::example_template();
    let files = support::example_files();
    let mut group = c.benchmark_group("template_render/scaling");
    for n in [10usize, 100, 1000] {
        let rules = support::synthetic_rule_specs(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &rules, |b, rules| {
            b.iter(|| template::render(tmpl, rules, &files));
        });
    }
    group.finish();
}

/// How the output-schema generation scales with the number of rule names.
fn bench_schema_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("schema_build");
    for n in [10usize, 100, 1000] {
        let names = support::synthetic_rule_names(n);
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        group.bench_with_input(BenchmarkId::from_parameter(n), &refs, |b, refs| {
            b.iter(|| schema::build(black_box(refs)));
        });
    }
    group.finish();
}

/// How vote aggregation scales with judge count. The verdicts dissent, so each
/// tally takes the fail branch (union + de-dup of violations).
fn bench_vote_tally(c: &mut Criterion) {
    let mut group = c.benchmark_group("vote_tally");
    for j in [1usize, 3, 9, 27] {
        let verdicts = support::judge_verdicts(j, true);
        group.bench_with_input(BenchmarkId::from_parameter(j), &verdicts, |b, verdicts| {
            b.iter(|| vote::tally(black_box("rule"), black_box(verdicts)));
        });
    }
    group.finish();
}

/// Assemble + format the report (the `new` sort plus a human or JSON render)
/// for a growing outcome set.
fn bench_report(c: &mut Criterion) {
    let mut group = c.benchmark_group("report");
    for n in [10usize, 100] {
        let outcomes = support::outcomes(n);
        group.bench_with_input(BenchmarkId::new("human", n), &outcomes, |b, outcomes| {
            b.iter_batched(
                || outcomes.clone(),
                // Verbosity 2 renders every rule line + violations (the heaviest path).
                |o| report::Report::new(o, vec![]).to_human(2),
                BatchSize::SmallInput,
            );
        });
        group.bench_with_input(BenchmarkId::new("json", n), &outcomes, |b, outcomes| {
            b.iter_batched(
                || outcomes.clone(),
                |o| report::Report::new(o, vec![]).to_json(),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_config_parse,
    bench_config_parse_scaling,
    bench_plan_build,
    bench_plan_build_scaling,
    bench_template_render,
    bench_template_render_scaling,
    bench_schema_build,
    bench_vote_tally,
    bench_report
);
criterion_main!(benches);
