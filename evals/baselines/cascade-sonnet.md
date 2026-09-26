### llmlint eval — arm `cascade`

203 cases, 488 scored items (1 repeat(s)), model `jev-1.13.0 + sonnet`, llmlint `llmlint 0.4.2`. Errored items: 0; unstable across repeats: 0.

| | Recall | Precision | Line hit | Latency p50 / p95 | Judge calls | Cost |
|---|---|---|---|---|---|---|
| `cascade` | 99.3% | 99.3% | 100.0% | 7.2s / 16.8s | 741 | $19.05 |

| Suite | Recall | Precision | Line hit | Latency p50 |
|---|---|---|---|---|
| config-lint | 100.0% | 100.0% | 100.0% | 9.8s |
| cross-file | 100.0% | 100.0% | 100.0% | 0.2s |
| prose | 100.0% | 100.0% | 100.0% | 0.4s |
| python-conventions | 96.7% | 96.7% | 100.0% | 8.7s |
| rust-conventions | 100.0% | 100.0% | 100.0% | 7.1s |

<details><summary>Misses</summary>

- `python-conventions/clean_config_loader_and_retry_client` · `docstrings_match_behavior` (rep 0): false alarm
- `python-conventions/service_composes_order_by_fragment_for_repository` · `services_do_not_build_sql` (rep 0): missed violation

</details>
