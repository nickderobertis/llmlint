### llmlint eval — arm `jev`

203 cases, 976 scored items (2 repeat(s)), model `jev-1.13.0`, llmlint `llmlint 0.4.2`. Errored items: 0; unstable across repeats: 2.

| | Recall | Precision | Line hit | Latency p50 / p95 | Judge calls | Cost |
|---|---|---|---|---|---|---|
| `jev` | 97.6% | 99.6% | 100.0% | 0.3s / 2.6s | 1232 | $0.12 |

Warnings (flagged for review, never failing): 4.1% of items; recall counting warned violations: 97.9%.

| Suite | Recall | Precision | Recall +warn | Warned | Line hit | Latency p50 |
|---|---|---|---|---|---|---|
| config-lint | 93.8% | 97.8% | 95.8% | 6.2% | 100.0% | 0.5s |
| cross-file | 100.0% | 100.0% | 100.0% | 0.0% | 100.0% | 0.2s |
| prose | 100.0% | 100.0% | 100.0% | 5.7% | 100.0% | 0.2s |
| python-conventions | 93.3% | 100.0% | 93.3% | 2.4% | 100.0% | 0.6s |
| rust-conventions | 100.0% | 100.0% | 100.0% | 3.2% | 100.0% | 0.7s |

<details><summary>Misses</summary>

- `config-lint/clean_fourteen_rule_config` · `description_and_relevance_are_concise` (rep 0): false alarm
- `config-lint/description_repeats_relevance_gate` · `description_omits_already_excluded_situations` (rep 1): missed violation
- `config-lint/line_localizable_rule_missing_attribution` · `line_localizable_rules_require_attribution` (rep 0): missed violation
- `config-lint/line_localizable_rule_missing_attribution` · `line_localizable_rules_require_attribution` (rep 1): missed violation
- `python-conventions/docstring_claims_input_unmodified_shallow_copy_mutates` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_claims_input_unmodified_shallow_copy_mutates` · `docstrings_match_behavior` (rep 1): missed violation
- `python-conventions/service_composes_order_by_fragment_for_repository` · `services_do_not_build_sql` (rep 0): missed violation
- `python-conventions/service_composes_order_by_fragment_for_repository` · `services_do_not_build_sql` (rep 1): missed violation

</details>
