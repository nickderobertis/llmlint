### llmlint eval — arm `jev`

203 cases, 976 scored items (2 repeat(s)), model `jev-1.13.0`, llmlint `llmlint 0.4.2`. Errored items: 0; unstable across repeats: 2.

| | Recall | Precision | Line hit | Latency p50 / p95 | Judge calls | Cost |
|---|---|---|---|---|---|---|
| `jev` | 91.3% | 97.4% | 100.0% | 0.3s / 2.1s | 1232 | $0.06 |

| Suite | Recall | Precision | Line hit | Latency p50 |
|---|---|---|---|---|
| config-lint | 66.7% | 100.0% | 100.0% | 0.4s |
| cross-file | 93.8% | 100.0% | 100.0% | 0.2s |
| prose | 97.1% | 97.1% | 100.0% | 0.2s |
| python-conventions | 91.7% | 100.0% | 100.0% | 0.6s |
| rust-conventions | 100.0% | 94.1% | 100.0% | 0.6s |

<details><summary>Misses</summary>

- `config-lint/buried_inline_if_any_clause` · `relevance_scopes_conditional_rules` (rep 0): missed violation
- `config-lint/buried_inline_if_any_clause` · `relevance_scopes_conditional_rules` (rep 1): missed violation
- `config-lint/buried_name_mismatches_description` · `name_describes_what_the_rule_checks` (rep 0): missed violation
- `config-lint/buried_name_mismatches_description` · `name_describes_what_the_rule_checks` (rep 1): missed violation
- `config-lint/buried_relevance_names_violating_case` · `relevance_scopes_without_deciding_the_verdict` (rep 0): missed violation
- `config-lint/buried_relevance_names_violating_case` · `relevance_scopes_without_deciding_the_verdict` (rep 1): missed violation
- `config-lint/description_repeats_relevance_gate` · `description_omits_already_excluded_situations` (rep 0): missed violation
- `config-lint/description_repeats_relevance_gate` · `description_omits_already_excluded_situations` (rep 1): missed violation
- `config-lint/diff_narrows_relevance_to_violating_case` · `relevance_scopes_without_deciding_the_verdict` (rep 0): missed violation
- `config-lint/diff_narrows_relevance_to_violating_case` · `relevance_scopes_without_deciding_the_verdict` (rep 1): missed violation
- `config-lint/diff_replaces_go_glob_with_relevance` · `path_scoped_rules_use_files_not_relevance` (rep 0): missed violation
- `config-lint/diff_replaces_go_glob_with_relevance` · `path_scoped_rules_use_files_not_relevance` (rep 1): missed violation
- `config-lint/inline_if_present_clause` · `relevance_scopes_conditional_rules` (rep 0): missed violation
- `config-lint/inline_if_present_clause` · `relevance_scopes_conditional_rules` (rep 1): missed violation
- `config-lint/line_localizable_rule_missing_attribution` · `line_localizable_rules_require_attribution` (rep 0): missed violation
- `config-lint/line_localizable_rule_missing_attribution` · `line_localizable_rules_require_attribution` (rep 1): missed violation
- `cross-file/fresh_cli_max_connections_documented_as_abbreviation` · `cli_flags_documented_in_readme` (rep 0): missed violation
- `cross-file/fresh_cli_max_connections_documented_as_abbreviation` · `cli_flags_documented_in_readme` (rep 1): missed violation
- `prose/diff_partial_term_rename` · `terms_are_used_consistently` (rep 0): missed violation
- `prose/diff_partial_term_rename` · `terms_are_used_consistently` (rep 1): missed violation
- `prose/near_miss_appropriately_but_concrete` · `steps_are_actionable` (rep 0): false alarm
- `prose/near_miss_appropriately_but_concrete` · `steps_are_actionable` (rep 1): false alarm
- `python-conventions/docstring_claims_input_unmodified_shallow_copy_mutates` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_claims_input_unmodified_shallow_copy_mutates` · `docstrings_match_behavior` (rep 1): missed violation
- `python-conventions/docstring_says_cents_code_returns_whole_dollars` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/service_composes_order_by_fragment_for_repository` · `services_do_not_build_sql` (rep 0): missed violation
- `python-conventions/service_composes_order_by_fragment_for_repository` · `services_do_not_build_sql` (rep 1): missed violation
- `rust-conventions/generic_placeholder_names` · `doc_comments_match_behavior` (rep 1): false alarm
- `rust-conventions/near_miss_documented_panic_is_accurate` · `doc_comments_match_behavior` (rep 0): false alarm
- `rust-conventions/near_miss_documented_panic_is_accurate` · `doc_comments_match_behavior` (rep 1): false alarm
- `rust-conventions/near_miss_question_mark_on_helper_that_carries_path` · `errors_carry_context` (rep 0): false alarm
- `rust-conventions/near_miss_question_mark_on_helper_that_carries_path` · `errors_carry_context` (rep 1): false alarm

</details>
