# ledgerd overview

ledgerd is a double-entry ledger service for the billing platform. It records
every charge, refund, and payout as a balanced pair of postings.

## Performance

ledgerd is fast: on a single 4-vCPU node it sustains 8,400 postings per second
at a p99 write latency of 11 ms. The full methodology and raw numbers are in the
[2026-07 load test](benchmarks/2026-07-load-test.md).

Reads are cheaper than writes. A balance lookup for one account completes in
under 2 ms at p99 because balances are maintained incrementally rather than
summed on demand (see the [read-path benchmark](benchmarks/2026-07-reads.md)).

## Run it locally

1. Start Postgres with `docker compose up -d postgres`.
2. Apply the schema with `ledgerd migrate --database-url postgres://localhost/ledger`.
3. Start the service with `ledgerd serve --port 7400`.

To check that the service is up, query its health endpoint:

```sh
curl http://localhost:7400/healthz
```
