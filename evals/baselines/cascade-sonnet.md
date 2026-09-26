### llmlint eval — arm `cascade`

191 cases, 476 scored items (1 repeat(s)), model `jev-1.13.0 + sonnet`, llmlint `llmlint 0.4.2`. Errored items: 0; unstable across repeats: 0.

| | Recall | Precision | Line hit | Latency p50 / p95 | Judge calls | Cost |
|---|---|---|---|---|---|---|
| baseline (`harness`) | 100.0% | 100.0% | 100.0% | 9.2s / 16.6s | 191 | $20.14 |
| this run (`cascade`) | 98.5% | 99.3% | 99.2% | 7.8s / 16.0s | 355 | $14.68 |
| Δ | -1.5pp | -0.7pp | -0.8pp | -1.4s / -0.6s | +164 | -5.4571 |

| Suite | Recall | Precision | Line hit | Latency p50 |
|---|---|---|---|---|
| config-lint | 100.0% | 100.0% | 95.8% | 9.4s |
| cross-file | 100.0% | 100.0% | 100.0% | 6.7s |
| prose | 100.0% | 100.0% | 100.0% | 3.1s |
| python-conventions | 100.0% | 100.0% | 100.0% | 8.8s |
| rust-conventions | 95.0% | 97.4% | 100.0% | 7.5s |

<details><summary>Misses</summary>

- `config-lint/ambiguous_verdict_mapping` · `description_yields_clear_verdict` (rep 0): wrong line
- `rust-conventions/near_miss_domain_plans_writes_without_io` · `doc_comments_match_behavior` (rep 0): false alarm
- `rust-conventions/subtle_doc_never_panics_but_indexes` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/subtle_error_context_omits_path` · `errors_carry_context` (rep 0): missed violation

</details>
