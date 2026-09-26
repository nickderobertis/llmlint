### llmlint eval — arm `jev`

191 cases, 476 scored items (1 repeat(s)), model `jev-1.13.0`, llmlint `llmlint 0.4.2`. Errored items: 0; unstable across repeats: 0.

| | Recall | Precision | Line hit | Latency p50 / p95 | Judge calls | Cost |
|---|---|---|---|---|---|---|
| `jev` | 83.8% | 85.1% | 98.2% | 0.2s / 0.3s | 220 | $0.02 |

| Suite | Recall | Precision | Line hit | Latency p50 |
|---|---|---|---|---|
| config-lint | 83.3% | 58.8% | 95.0% | 0.2s |
| cross-file | 100.0% | 66.7% | 100.0% | 0.2s |
| prose | 94.1% | 97.0% | 100.0% | 0.2s |
| python-conventions | 73.3% | 100.0% | 95.5% | 0.2s |
| rust-conventions | 80.0% | 97.0% | 100.0% | 0.2s |

<details><summary>Misses</summary>

- `config-lint/ambiguous_verdict_mapping` · `description_yields_clear_verdict` (rep 0): wrong line
- `config-lint/buried_name_mismatches_description` · `name_describes_what_the_rule_checks` (rep 0): missed violation
- `config-lint/buried_name_mismatches_description` · `description_yields_clear_verdict` (rep 0): false alarm
- `config-lint/buried_path_condition_in_relevance` · `path_scoped_rules_use_files_not_relevance` (rep 0): missed violation
- `config-lint/buried_path_condition_in_relevance` · `description_yields_clear_verdict` (rep 0): false alarm
- `config-lint/buried_placeholder_name_in_twelve_rules` · `description_yields_clear_verdict` (rep 0): false alarm
- `config-lint/buried_relevance_names_violating_case` · `description_yields_clear_verdict` (rep 0): false alarm
- `config-lint/clean_fourteen_rule_config` · `description_yields_clear_verdict` (rep 0): false alarm
- `config-lint/clean_fourteen_rule_config` · `description_and_relevance_are_concise` (rep 0): false alarm
- `config-lint/clean_fourteen_rule_config` · `line_localizable_rules_require_attribution` (rep 0): false alarm
- `config-lint/clean_team_config` · `path_scoped_rules_use_files_not_relevance` (rep 0): false alarm
- `config-lint/description_repeats_relevance_gate` · `description_omits_already_excluded_situations` (rep 0): missed violation
- `config-lint/description_repeats_relevance_gate` · `path_scoped_rules_use_files_not_relevance` (rep 0): false alarm
- `config-lint/diff_moves_not_applicable_into_relevance` · `path_scoped_rules_use_files_not_relevance` (rep 0): false alarm
- `config-lint/line_localizable_rule_missing_attribution` · `line_localizable_rules_require_attribution` (rep 0): missed violation
- `config-lint/line_localizable_rule_missing_attribution` · `description_yields_clear_verdict` (rep 0): false alarm
- `config-lint/long_description_needed_examples` · `description_and_relevance_are_concise` (rep 0): false alarm
- `config-lint/relevance_asserts_the_property` · `path_scoped_rules_use_files_not_relevance` (rep 0): false alarm
- `config-lint/relevance_mentions_paths_but_is_content` · `path_scoped_rules_use_files_not_relevance` (rep 0): false alarm
- `cross-file/api_exports_tested_via_alias` · `api_exports_are_tested` (rep 0): false alarm
- `cross-file/env_vars_read_via_helper_all_listed` · `env_vars_listed_in_example` (rep 0): false alarm
- `cross-file/flag_documented_under_troubleshooting` · `cli_flags_documented_in_readme` (rep 0): false alarm
- `cross-file/flags_all_documented` · `cli_flags_documented_in_readme` (rep 0): false alarm
- `prose/diff_partial_term_rename` · `terms_are_used_consistently` (rep 0): missed violation
- `prose/near_miss_appropriately_but_concrete` · `steps_are_actionable` (rep 0): false alarm
- `prose/term_workspace_vs_project` · `terms_are_used_consistently` (rep 0): missed violation
- `python-conventions/diff_lookup_now_raises_docstring_still_says_none` · `docstrings_match_behavior` (rep 0): wrong line
- `python-conventions/distractor_job_runner_docstring_seconds_code_milliseconds` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_claims_idempotent_but_appends_duplicate` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_claims_input_unmodified_shallow_copy_mutates` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_raises_valueerror_code_raises_keyerror` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_says_cents_code_returns_whole_dollars` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/docstring_says_newest_first_sorts_oldest_first` · `docstrings_match_behavior` (rep 0): missed violation
- `python-conventions/log_names_operation_but_omits_refund_id` · `log_messages_are_actionable` (rep 0): missed violation
- `python-conventions/multihop_docstring_promises_raise_helper_returns_none` · `docstrings_match_behavior` (rep 0): missed violation
- `rust-conventions/distractor_client_context_names_wrong_request` · `errors_carry_context` (rep 0): missed violation
- `rust-conventions/distractor_ledger_stale_doc_and_placeholder_far_apart` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/distractor_query_page_returns_limit_plus_one` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/doc_claims_ascending_sorts_descending` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/near_miss_question_mark_on_helper_that_carries_path` · `errors_carry_context` (rep 0): false alarm
- `rust-conventions/subtle_doc_case_insensitive_but_exact_compare` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/subtle_doc_never_panics_but_indexes` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/subtle_doc_sorted_output_from_hashmap` · `doc_comments_match_behavior` (rep 0): missed violation
- `rust-conventions/subtle_error_context_omits_path` · `errors_carry_context` (rep 0): missed violation

</details>
