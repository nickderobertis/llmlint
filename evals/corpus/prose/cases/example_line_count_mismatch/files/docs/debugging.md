# Debugging the ingest worker

The ingest worker logs to `/var/log/ingest/worker.log` and rotates the file
daily.

## Read recent log lines

To print the last 20 lines of the worker log, use `tail`:

```sh
tail -n 200 /var/log/ingest/worker.log
```

## Follow the log live

To keep printing new lines as the worker writes them, add `-f`:

```sh
tail -f /var/log/ingest/worker.log
```

## Find failed batches

To list only the lines that mention a failed batch, search for `batch_failed`:

```sh
grep batch_failed /var/log/ingest/worker.log
```
