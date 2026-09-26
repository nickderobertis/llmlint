# Deploying the price service

The service handles about 1,200 requests per second on one 2-vCPU instance
(see the [load test report](benchmarks/2026-08.md)).

## Deploy to staging

1. Build the release image: run `make image TAG=staging`.
2. Push it to the registry with `docker push registry.example.com/prices:staging`.
3. Configure the database settings appropriately for your environment.
4. Apply the manifest with `kubectl apply -f deploy/staging.yaml`.

## Preview a migration

To see which migrations would run without applying them, pass `--dry-run`:

```sh
prices migrate --dry-run
```

To apply them, drop the flag:

```sh
prices migrate
```
