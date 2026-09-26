# prices

A command-line client for the internal price service. It looks up current
prices for one or more SKUs, retries transient failures, and prints the result
as a table, JSON, or CSV.

## Installation

```sh
cargo install --path .
```

The binary is called `prices`. Run `prices --version` to confirm the install.

## Quick start

```sh
prices --endpoint https://prices.internal sku-123 sku-456
```

The endpoint can also come from the `PRICES_ENDPOINT` environment variable, so
most people export it once in their shell profile.

## Options

### Connection

- `--endpoint <URL>`: base URL of the price service (or `PRICES_ENDPOINT`).
- `--timeout <SECS>`: request timeout in seconds (default 30).
- `--retries <N>`: how many times a failed request is retried (default 3).
- `--concurrency <N>`: maximum requests in flight at once (default 4).
- `--region <CODE>`: price region to query (default `us`).

### Output

- `-o, --output <PATH>`: write results to a file instead of stdout.
- `--format <table|json|csv>`: output encoding (default `table`).
- `-v, --verbose`: log each request and response.
- `-q, --quiet`: print nothing but errors; cannot be combined with `--verbose`.

### Other

- `--config <PATH>`: read defaults from a TOML file (see below).
- `--dry-run`: print what would be sent without sending it.

## Colors

Table output is colorized when stdout is a terminal. Colors are turned off
automatically when output is piped or when the `NO_COLOR` environment variable
is set, following the convention at no-color.org.

## Config file

Any long option can be given a default in the file passed to `--config`:

```toml
endpoint = "https://prices.internal"
timeout = 10
region = "eu"
```

Command-line flags always override the file.

## Exit codes

- `0`: every SKU was priced.
- `1`: at least one SKU was not found.
- `2`: usage or connection error.
