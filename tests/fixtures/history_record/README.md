# History record goldens

`unlabelled.json` and `labelled.json` pin the JSON record `llmlint lint` writes
to the history directory (`io::history::build_record` + `write_record`), byte for
byte, for one fixed passing run; `split.json` pins a failing run whose batch was
split, one file's diff judged in two parts. `llmlint_version` is replaced with `<version>`
so a release bump doesn't churn them. `persisted_record_matches_the_goldens` in
`src/io/history.rs` holds the code to them.

A record with **no `schema_version` field is version 1**. Run labels were added as an *additive, optional* top-level `labels` object rather
than a version bump, because the contract they serve requires an unlabelled
record to stay exactly as it was, and a new version field would change every
record:

- `unlabelled.json` is the shape every record had before labels existed. The
  unlabelled path writes the same keys in the same order; `labels` is inserted
  only when at least one label was given.
- `labelled.json` differs only by `"labels"` (string → string, keys sorted),
  placed after `config_files`.
- A reader treats a missing `labels` as `{}`, so older records and older readers
  are unaffected. Labels read back are checked against the label grammar again.

**Version 2** (`RECORD_SCHEMA_VERSION`) adds `diff_part` (`{index, of}`) to a
plan batch part that carried one part of a file's diff split to fit
`max_prompt_chars`. The record then opens with `"schema_version": 2`
(`split.json`). The field and the stamp appear **only** on a record that holds a
split diff, so every other record, the two goldens above included, keeps its
version-1 bytes. That is the label contract above. A reader treats a missing
`schema_version` as 1 (`Record::schema_version`). An unsplit part never carries
`diff_part`, even in a version-2 record (the third part in `split.json`).
`a_split_record_is_schema_version_2_and_its_diff_parts_round_trip` and
`a_record_with_no_split_stays_version_1_and_old_records_still_read` hold this.

Changing the record shape any other way means bumping `RECORD_SCHEMA_VERSION`
and updating these goldens in the same commit.
