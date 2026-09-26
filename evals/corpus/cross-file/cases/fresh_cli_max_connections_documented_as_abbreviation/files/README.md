# pgproxy

A tiny connection-pooling proxy that sits between your application and
PostgreSQL. Point your app at pgproxy instead of the database and it will reuse
a bounded pool of server connections.

## Quick start

```sh
pgproxy --listen 0.0.0.0:6432 --upstream db.internal:5432 --max-conns 50
```

## Options

- `--listen <ADDR>`: where clients connect (default `127.0.0.1:6432`).
- `--upstream <ADDR>`: the real PostgreSQL server (default `127.0.0.1:5432`).
- `--max-conns <N>`: upper bound on pooled server connections (default 20).
  Clients beyond the pool size wait in a FIFO queue.
- `--idle-timeout <SECS>`: close server connections idle this long (default 300).
- `--tls-cert <PEM>` / `--tls-key <PEM>`: serve TLS to clients. Both are
  required together.
- `--log-queries`: log every query with its duration at info level.

## Sizing the pool

A good starting point is two server connections per CPU core on the database
host. Watch `pg_stat_activity`; if clients queue for long, raise the limit.
