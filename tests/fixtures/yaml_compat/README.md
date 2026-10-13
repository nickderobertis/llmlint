# YAML compatibility corpus

`configs.json` and `versions.json` were captured through `configfs::parse` and
`plugins::declared_version` with the original parser at base
`d7e3b621e5987382eea2ef815237903d8fddaf81`, before the dependency migration.
The tests deserialize the expected configs back into `Config` and compare them
with the real config reader; plugin versions exercise the actual fetch probe.

`plugins/` contains every `*.llmlint.yml` fragment from
`nickderobertis/dero-skills`, commit `74151ab016cd72d043e5590f3fc51a5ad0e51609`,
under `skills/bootstrap/create-repo/assets/llmlint/`, preserving relative paths.
`corpus.json` also lists this repository's five committed configs and a fixture
covering anchors, merge keys, stash keys, both relevance forms and version pins.
These are baseline expectations, not snapshots to regenerate after a parser change.
