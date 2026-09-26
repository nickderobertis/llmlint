# ledgerd overview

ledgerd is a double-entry ledger service for the billing platform. It records
every charge, refund, and payout as a balanced pair of postings.

## Why ledgerd

ledgerd has the fastest write path of any ledger we evaluated, and it is the
most dependable piece of the billing stack.

## Accounting model

Every posting belongs to exactly one journal entry, and every journal entry
must balance to zero before it is committed. Accounts are typed as asset,
liability, revenue, or expense, and the type fixes which side of an entry
increases the balance.

## Load test results

On a single 4-vCPU node the July load test measured 8,400 postings per second
at a p99 write latency of 11 ms. See the
[2026-07 load test](benchmarks/2026-07-load-test.md) for the setup.

## Run it locally

1. Start Postgres with `docker compose up -d postgres`.
2. Apply the schema with `ledgerd migrate --database-url postgres://localhost/ledger`.
3. Start the service with `ledgerd serve --port 7400`.
