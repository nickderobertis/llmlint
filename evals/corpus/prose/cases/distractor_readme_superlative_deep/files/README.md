# tessera

tessera is a time-series store for infrastructure metrics. It ingests samples
over the Prometheus remote-write protocol, keeps recent data in memory, and
compacts older data into immutable blocks on local disk or object storage.

tessera is maintained by the Observability team. Questions go to
`#observability` on Slack; bugs go to the issue tracker.

## Contents

- [Quick start](#quick-start)
- [Configuration](#configuration)
- [Writing data](#writing-data)
- [Querying](#querying)
- [Architecture](#architecture)
- [Operations](#operations)
- [Benchmarks](#benchmarks)
- [Contributing](#contributing)
- [License](#license)

## Quick start

To try tessera on one machine, use the single-node container image.

1. Pull the image with `docker pull ghcr.io/example/tessera:1.12.0`.
2. Create a data directory with `mkdir -p ./tessera-data`.
3. Start a node with `docker run -d --name tessera -p 9201:9201 -v "$PWD/tessera-data:/data" ghcr.io/example/tessera:1.12.0`.
4. Check that it is ready with `curl http://localhost:9201/-/ready`.

To follow the node's log while you try the examples below, tail the container:

```sh
docker logs -f tessera
```

## Configuration

tessera reads one YAML file, passed with `--config`. Every key has a default,
so an empty file is a valid configuration. The keys you are most likely to
change are below.

| Key | Default | Meaning |
|---|---|---|
| `listen` | `:9201` | Address for the HTTP API and remote-write endpoint. |
| `data_dir` | `/data` | Where the write-ahead log and local blocks live. |
| `retention` | `15d` | How long samples are kept before deletion. |
| `head_window` | `2h` | How long samples stay in the in-memory head before compaction. |
| `object_store.bucket` | unset | Bucket for compacted blocks; unset keeps blocks local. |
| `limits.max_series` | `2000000` | Active series above which new series are rejected. |

A minimal configuration that ships blocks to S3:

```yaml
data_dir: /data
retention: 30d
object_store:
  bucket: metrics-blocks-prod
  region: eu-west-1
```

To validate a configuration file without starting the server, run
`tessera check-config`:

```sh
tessera check-config --config tessera.yaml
```

The command prints the effective configuration, with defaults filled in, and
exits non-zero if any key is unknown or any value fails to parse.

## Writing data

tessera accepts Prometheus remote-write requests on `/api/v1/write`. Point a
Prometheus server at it by adding a `remote_write` block to `prometheus.yml`:

```yaml
remote_write:
  - url: http://tessera.internal:9201/api/v1/write
```

Samples must arrive in timestamp order per series. An out-of-order sample is
rejected with HTTP 400 and counted in `tessera_samples_rejected_total` with
`reason="out_of_order"`.

### Series limits

Each node tracks how many series are active in its head. When the count
reaches `limits.max_series`, new series are rejected with HTTP 429 until old
series go stale and are dropped at the next head compaction. Existing series
keep accepting samples.

To see the current active-series count, query the node's own metric:

```sh
curl -s http://localhost:9201/metrics | grep tessera_head_series
```

## Querying

tessera serves the Prometheus HTTP query API, so Grafana and `promtool` work
against it unchanged. Add it to Grafana as a Prometheus data source with the
URL `http://tessera.internal:9201`.

To run an instant query from the command line, use `promtool query instant`:

```sh
promtool query instant http://localhost:9201 'rate(http_requests_total[5m])'
```

To run a range query over the last hour at one-minute resolution, use
`promtool query range` with `--start`, `--end`, and `--step`:

```sh
promtool query range --start "$(date -d '-1 hour' +%s)" --end "$(date +%s)" --step 1m http://localhost:9201 'up'
```

### Query limits

A single query may touch at most 500,000 series and return at most 11,000
points per series. A query over either limit fails with HTTP 422 and a message
naming the limit. Both limits are set under `limits` in the configuration file.

## Architecture

tessera has three moving parts: the head, the write-ahead log, and the block
store.

### The head

Incoming samples go into the head, an in-memory structure that holds the last
`head_window` of data for every active series. Samples are stored in chunks of
up to 120 samples, compressed with delta-of-delta timestamps and XOR-encoded
values.

### The write-ahead log

Every accepted sample is appended to the write-ahead log (WAL) in `data_dir`
before the write is acknowledged. On restart, tessera replays the WAL to
rebuild the head, including every sample acknowledged before a crash. The WAL is
truncated after each head compaction.

### The block store

Every `head_window`, the oldest part of the head is compacted into an
immutable block: a directory holding an index, the chunks, and a metadata
file. Blocks are later merged into larger blocks covering up to 31 days. With
`object_store.bucket` set, finished blocks are uploaded and the local copy is
deleted once the upload is verified.

This block layout is the most efficient storage design available for
append-heavy metrics workloads, and it is why tessera needs so little disk.

### Retention

Blocks whose newest sample is older than `retention` are deleted at the next
compaction. Retention applies per block, so a block is kept until every sample
in it has expired.

## Operations

### Upgrading a node

Upgrades are rolling. For each node in turn:

1. Stop the node with `systemctl stop tessera`.
2. Replace the binary with `sudo install -m 755 tessera-1.12.0 /usr/local/bin/tessera`.
3. Start the node with `systemctl start tessera`.
4. Wait until `curl http://localhost:9201/-/ready` returns HTTP 200 before moving to the next node.

WAL replay on start takes about one second per 400 MB of WAL, measured on the
production nodes' NVMe disks.

### Backing up

Blocks are immutable, so a consistent backup is a copy of the block
directories. The head is not included; it is rebuilt from the WAL.

To create a snapshot of the current blocks under `data_dir/snapshots/`, call
the snapshot endpoint:

```sh
curl -X POST http://localhost:9201/api/v1/admin/snapshot
```

The response names the snapshot directory. Copy that directory off the host
with your usual backup tool.

### Monitoring tessera itself

tessera exposes its own metrics on `/metrics`. The ones worth alerting on:

- `tessera_wal_fsync_duration_seconds` — p99 above 50 ms means the disk is
  falling behind.
- `tessera_compactions_failed_total` — any increase needs a look.
- `tessera_samples_rejected_total` — a jump usually means a client is sending
  out-of-order samples.

## Benchmarks

On a single `m6i.2xlarge` node with a gp3 volume, tessera ingested 1.1 million
samples per second across 2 million active series with a p99 remote-write
latency of 38 ms. The setup and raw results are in
[bench/2026-06-ingest.md](bench/2026-06-ingest.md).

Compressed blocks averaged 1.3 bytes per sample on the same dataset, compared
with 16 bytes for an uncompressed timestamp-value pair
([bench/2026-06-storage.md](bench/2026-06-storage.md)).

## Contributing

1. Fork the repository and clone your fork with `git clone git@github.com:<you>/tessera.git`.
2. Install the toolchain pinned in `rust-toolchain.toml` with `rustup show`.
3. Run the tests with `cargo test --workspace`.
4. Open a pull request against `main` with `gh pr create --base main`.

To run only the storage tests while you iterate, filter by package:

```sh
cargo test -p tessera-storage
```

## License

Apache-2.0. See [LICENSE](LICENSE).
