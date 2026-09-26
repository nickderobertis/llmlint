# Setting up the reporting worker

The reporting worker renders the nightly revenue reports and uploads them to
the `reports` bucket.

## First-time setup

1. Clone the repository with `git clone git@github.com:example/reporting.git`.
2. Install dependencies with `uv sync`.
3. Copy the sample config with `cp config/worker.sample.yaml config/worker.yaml`.
4. Set `reports_bucket: reports-dev` in `config/worker.yaml` so local runs never write to the production bucket.
5. Run the worker once with `uv run worker --once` to check the config.

## Render a single report

To render one day's report without uploading it, pass `--no-upload` and the date:

```sh
uv run worker render --date 2026-09-01 --no-upload
```

To render a report and upload it to the bucket named in your config, drop
`--no-upload`:

```sh
uv run worker render --date 2026-09-01
```
