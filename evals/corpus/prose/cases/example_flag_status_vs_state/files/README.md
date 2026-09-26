# jobq

Command-line client for the batch job scheduler.

## List jobs

To list every job submitted in the last day, run `jobq list`:

```sh
jobq list --since 24h
```

To list only failed jobs, add `--status failed`:

```sh
jobq list --since 24h --state failed
```

## Retry a job

To retry one job by ID, run `jobq retry`:

```sh
jobq retry 8812
```

## Performance

`jobq list` returns in under 300 ms for up to 50,000 jobs, measured against the
staging scheduler (see [benchmarks/list.md](benchmarks/list.md)).
