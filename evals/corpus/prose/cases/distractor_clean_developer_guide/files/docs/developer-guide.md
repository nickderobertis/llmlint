# shipit developer guide

shipit is the internal command-line tool that builds, tests, and deploys the
web services in this repository. This guide covers day-to-day use. For the
design of the tool itself, see `docs/shipit-internals.md`.

## Concepts

shipit works with three things:

- A **service** is one deployable unit with a `service.toml` at its root, for
  example `services/cart/service.toml`.
- An **environment** is a place a service runs: `dev`, `staging`, or `prod`.
  Each environment has its own cluster and its own secrets.
- A **release** is an immutable, versioned build of one service, identified by
  the service name and the Git commit, for example `cart@4f2a9c1`.

You build a release once and promote the same release from environment to
environment; shipit never rebuilds a release for a later environment.

## Install shipit

1. Install the Go toolchain version listed in `.go-version` with `mise install`.
2. Build and install shipit with `go install ./cmd/shipit`.
3. Confirm the install with `shipit version`; it prints the commit shipit was built from.
4. Log in to the container registry with `shipit login`.

## Build a release

To build a release of the `cart` service from your current commit, run
`shipit build` with the service name:

```sh
shipit build cart
```

The command prints the release name, for example `cart@4f2a9c1`, and pushes
the image to the registry.

To build without pushing — useful to check that the Dockerfile still builds —
add `--no-push`:

```sh
shipit build cart --no-push
```

## Test a release

shipit runs a service's unit tests inside the release image, so the tests see
exactly what will be deployed.

To run the unit tests for the `cart@4f2a9c1` release, run `shipit test`:

```sh
shipit test cart@4f2a9c1
```

To run only the tests whose names match `Checkout`, pass `--run Checkout`:

```sh
shipit test cart@4f2a9c1 --run Checkout
```

## Deploy a release

Deploying puts a release into one environment. shipit waits for the rollout to
finish and prints the new pods' status.

To deploy `cart@4f2a9c1` to the `dev` environment, run `shipit deploy` with
`--env dev`:

```sh
shipit deploy cart@4f2a9c1 --env dev
```

### Promote to staging and prod

Promotion deploys the release that is already running in one environment to
the next one. It never builds anything.

To promote the release running in `staging` to `prod`, run `shipit promote`
with `--from staging --to prod`:

```sh
shipit promote cart --from staging --to prod
```

Promotion to `prod` needs an approval. shipit posts an approval request in
`#deploys` and waits up to 30 minutes for a second engineer to approve it.

### Roll back

To put the previous release back in an environment, run `shipit rollback`
with the environment:

```sh
shipit rollback cart --env prod
```

A rollback is a normal deploy of the previous release, so it takes the same
time as a deploy and needs no approval.

## Secrets

Each environment keeps its own secrets in Vault under
`secret/<environment>/<service>/`. shipit reads them at deploy time and never
writes them to disk.

To list the secret names for `cart` in `staging` without printing their
values, run `shipit secrets list`:

```sh
shipit secrets list cart --env staging
```

To set one secret, run `shipit secrets set` with the name and value:

```sh
shipit secrets set cart --env staging STRIPE_WEBHOOK_SECRET=whsec_example
```

> **Warning:** `shipit secrets delete` removes the secret from Vault for that
> environment, including every earlier version. A deploy that needs the secret
> fails until you set it again, and the old value cannot be recovered.

To delete one secret, run `shipit secrets delete`:

```sh
shipit secrets delete cart --env staging LEGACY_API_TOKEN
```

## Clean up old releases

shipit keeps the last 20 releases of each service in the registry. Older
releases can be removed to save storage.

> **Warning:** pruning deletes release images from the registry permanently.
> A pruned release can no longer be deployed or rolled back to; you would have
> to rebuild it from its commit.

To delete every `cart` release except the 20 most recent, run `shipit prune`
with `--keep 20`:

```sh
shipit prune cart --keep 20
```

## Build times

A cold build of the `cart` service takes about 4 minutes on a CI runner; with
the layer cache warm it takes about 50 seconds. Those are medians over the
last 200 CI builds, from the "CI / Build durations" dashboard.

## Troubleshooting

### `shipit deploy` hangs at "waiting for rollout"

The new pods are not becoming ready. Show their events with
`shipit status`:

```sh
shipit status cart --env dev --events
```

### `shipit login` fails with "unauthorized"

Your registry session has expired. Log out and log in again:

```sh
shipit logout && shipit login
```

## Getting help

Ask in `#shipit`. Bugs in shipit itself go in the `SHIP` project in the issue
tracker; include the output of `shipit version`.
