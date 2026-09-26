# Tuning the export job

The export job copies the day's orders into the warehouse. Two settings control
how hard it pushes the primary database.

## Tune for a busy primary

1. Set the batch size appropriately for a busy primary: change `batch_size` from `5000` to `1000` in `config/export.toml`.
2. Configure the pause between batches as needed by your replica lag budget — for a 30-second budget, set `pause_ms = 250` in the same file.
3. Restart the job with `systemctl restart export-job`.

## Check the effect

To see the effective settings the running job loaded, run `export-job config show`:

```sh
export-job config show
```
