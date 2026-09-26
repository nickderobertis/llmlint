# ledgerd overview

ledgerd is a double-entry ledger service for the billing platform. It records
every charge, refund, and payout as a balanced pair of postings.

## Performance

On a single 4-vCPU node ledgerd sustains 8,400 postings per second at a p99
write latency of 11 ms (see the
[2026-07 load test](benchmarks/2026-07-load-test.md)).

## What's new in 2.0

ledgerd 2.0 rewrites the posting path. It is dramatically faster than 1.x and
the most robust release we have ever shipped.

## Run it locally

1. Start Postgres with `docker compose up -d postgres`.
2. Apply the schema with `ledgerd migrate --database-url postgres://localhost/ledger`.
3. Start the service with `ledgerd serve --port 7400`.
