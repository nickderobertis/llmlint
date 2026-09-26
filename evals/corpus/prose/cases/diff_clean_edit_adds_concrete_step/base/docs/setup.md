# Setting up the reporting worker

The reporting worker renders the nightly revenue reports and uploads them to
the `reports` bucket.

## First-time setup

1. Clone the repository with `git clone git@github.com:example/reporting.git`.
2. Install dependencies with `uv sync`.
3. Run the worker once with `uv run worker --once` to check the config.

## Render a single report

To render one day's report without uploading it, pass `--no-upload` and the date:

```sh
uv run worker render --date 2026-09-01 --no-upload
```
