# Restoring the inventory database

Nightly backups land in `/var/backups/inventory/` as compressed dumps named by
date, for example `inventory-2026-09-01.dump.gz`.

## Restore last night's backup

To restore the September 14 backup into the local database, run
`invctl restore` with `inventory-2026-09-14.dump.gz`:

```sh
invctl restore --from /var/backups/inventory/inventory-2026-09-04.dump.gz --target local
```

## List available backups

To list every backup on disk with its size, run `invctl backups list`:

```sh
invctl backups list --dir /var/backups/inventory
```
