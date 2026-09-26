# Restoring from the most recent snapshot

Snapshots of the `accounts` volume are taken every six hours and kept for 14
days. This page covers restoring the latest one; the best starting point for an
older snapshot is the [point-in-time guide](pitr.md).

## Restore

1. Find the most recent snapshot ID with `snapctl list --volume accounts --limit 1`.
2. Create a volume from it with `snapctl restore <snapshot-id> --name accounts-restored`.
3. Attach it to the recovery host with `snapctl attach accounts-restored --host recovery-1`.

The first step is the fastest to get wrong: `snapctl list` sorts newest first
only when `--limit` is set, so keep the flag.

## Check the restored data

To print the newest row's timestamp from the restored volume, run:

```sh
psql -h recovery-1 -d accounts -c "select max(updated_at) from accounts"
```
