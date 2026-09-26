# Resetting the staging database

Staging drifts from production over time. Reset it from the latest production
snapshot at the start of each release cycle.

## Reset

> **Warning:** step 2 permanently deletes the `shop_staging` database —
> every test account, order, and fixture created on staging since the last
> reset. Export anything you need to keep before you start.

1. Scale the staging API to zero with `kubectl scale deploy/api --replicas=0 -n staging`.
2. Drop the staging database with `psql -h staging-db -c "DROP DATABASE shop_staging"`.
3. Create it again with `psql -h staging-db -c "CREATE DATABASE shop_staging"`.
4. Load the anonymized production snapshot with `pg_restore -h staging-db -d shop_staging snapshots/prod-anon-latest.dump`.
5. Scale the API back up with `kubectl scale deploy/api --replicas=2 -n staging`.

## Check the reset

To confirm the snapshot loaded, count the orders table:

```sh
psql -h staging-db -d shop_staging -c "select count(*) from orders"
```
