# History record goldens

`unlabelled.json` and `labelled.json` pin the JSON record `llmlint lint` writes
to the history directory (`io::history::build_record` + `write_record`), byte for
byte, for one fixed passing run. `llmlint_version` is replaced with `<version>`
so a release bump doesn't churn them. `persisted_record_matches_the_goldens` in
`src/io/history.rs` holds the code to them.

The record carries **no schema version field**, before or after run labels. Run
labels were added as an *additive, optional* top-level `labels` object rather
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

Changing the record shape any other way means updating these goldens in the same
commit, and deciding whether the change needs a version field.
