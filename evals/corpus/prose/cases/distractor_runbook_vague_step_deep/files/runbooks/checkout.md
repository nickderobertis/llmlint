# Checkout service runbook

Owner: Payments team (`#payments-oncall`). Escalation: the Payments
engineering manager on the PagerDuty `payments-escalation` schedule.

This runbook covers the alerts that page for the checkout service. Each
section names the alert, what it means, and the procedure to follow. Work the
procedure in order; if a step's check fails, stop and escalate.

## Service summary

- **What it does:** turns a cart into a paid order. It reserves stock, calls
  the payments provider, and writes the order row.
- **Where it runs:** the `checkout` Deployment in the `shop` namespace of the
  `prod-eu` cluster, three replicas behind the `checkout` Service.
- **Dependencies:** Postgres (`orders-db`), Redis (`checkout-cache`), the
  inventory service, and the external payments provider.
- **Dashboards:** "Checkout / Overview" and "Checkout / Payments" in Grafana.
- **Logs:** `kubectl logs deploy/checkout -n shop`, or the `app:checkout`
  index in the log search.

## Before you start

Make sure your shell is pointed at the right cluster:

```sh
kubectl config use-context prod-eu
```

Every procedure below assumes that context and the `shop` namespace.

---

## Alert: CheckoutHighErrorRate

**Fires when:** more than 2% of `POST /checkout` requests return 5xx for 5
minutes.

**Likely causes:** a bad deploy, the payments provider degrading, or
`orders-db` connection exhaustion.

### Procedure

1. Open the "Checkout / Overview" dashboard and note when the error rate rose.
2. List recent rollouts with `kubectl rollout history deploy/checkout -n shop`.
3. If a rollout finished within 10 minutes before the rise, roll it back with `kubectl rollout undo deploy/checkout -n shop`.
4. If there was no recent rollout, check the provider's status page at `https://status.payments.example.com`.
5. If the provider reports an incident, post the provider's incident link in `#payments-oncall` and follow [Provider outage](#alert-paymentsproviderdown).
6. If neither applies, follow [Database connections exhausted](#alert-ordersdbconnectionsexhausted).

To see the last 50 checkout errors with their request IDs, filter the logs for
`level=error`:

```sh
kubectl logs deploy/checkout -n shop --since 15m | grep 'level=error' | tail -n 50
```

---

## Alert: PaymentsProviderDown

**Fires when:** the provider health probe fails for 3 minutes in a row.

**What it means:** checkout cannot take card payments. Orders fail at the
payment step; carts are kept.

### Procedure

1. Confirm the outage on `https://status.payments.example.com`.
2. Turn on the payments maintenance banner with `featurectl set checkout.maintenance_banner true --env prod`.
3. Post in `#incidents` that card payments are down, with the provider's incident link.
4. Every 15 minutes, re-check the status page and post an update in the same thread.
5. When the provider recovers, turn the banner off with `featurectl set checkout.maintenance_banner false --env prod`.

To check the flag's current value before and after, read it back:

```sh
featurectl get checkout.maintenance_banner --env prod
```

---

## Alert: OrdersDbConnectionsExhausted

**Fires when:** `orders-db` has fewer than 5 free connections for 5 minutes.

**What it means:** new checkout requests wait for a connection and time out
after 3 seconds.

### Procedure

1. Find the clients holding the most connections with `psql -h orders-db -c "select usename, application_name, count(*) from pg_stat_activity group by 1, 2 order by 3 desc"`.
2. If `checkout` holds more than 150, restart its pods one at a time with `kubectl rollout restart deploy/checkout -n shop`.
3. If a batch job (`application_name` starting with `job-`) holds more than 50, suspend it with `kubectl patch cronjob/<name> -n shop -p '{"spec":{"suspend":true}}'`.
4. Watch the free-connection panel on "Checkout / Overview" until it is back above 20.

To list only idle-in-transaction sessions older than one minute, filter
`pg_stat_activity` by state and age:

```sh
psql -h orders-db -c "select pid, application_name from pg_stat_activity where state = 'idle in transaction' and now() - state_change > interval '1 minute'"
```

> **Warning:** terminating a backend rolls back its open transaction; any
> writes it has not committed are lost. Only terminate sessions you have
> confirmed are stuck.

To terminate one stuck session by PID, call `pg_terminate_backend`:

```sh
psql -h orders-db -c "select pg_terminate_backend(12345)"
```

---

## Alert: CheckoutCacheEvictionsHigh

**Fires when:** `checkout-cache` evicts more than 500 keys per second for 10
minutes.

**What it means:** the cache is full. Checkout still works, but every cart read
falls through to `orders-db`, which raises database load.

### Procedure

1. Check memory use with `redis-cli -h checkout-cache info memory`.
2. Check which key prefixes dominate with `redis-cli -h checkout-cache --bigkeys`.
3. If `cart:*` keys dominate, set `CART_TTL_SECONDS: "86400"` in `deploy/checkout-config.yaml` and apply it with `kubectl apply -f deploy/checkout-config.yaml`.
4. Restart the dependent services appropriately so they pick up the change.
5. Watch the eviction panel on "Checkout / Overview" for 10 minutes.

To read the cache's current eviction policy, query its config:

```sh
redis-cli -h checkout-cache config get maxmemory-policy
```

> **Warning:** `FLUSHDB` deletes every cart in the cache. Customers lose any
> cart they have not checked out, and `orders-db` takes the full read load
> until the cache refills. Use it only when the Payments engineering manager
> approves.

To empty the cache entirely, run `FLUSHDB` against it:

```sh
redis-cli -h checkout-cache flushdb
```

---

## Alert: CheckoutLatencyHigh

**Fires when:** p99 latency of `POST /checkout` is above 1.5 seconds for 10
minutes.

### Procedure

1. On "Checkout / Payments", compare provider call latency with total checkout latency.
2. If provider latency accounts for more than 80% of the total, follow [Provider outage](#alert-paymentsproviderdown) from step 1.
3. Otherwise, check inventory service latency on the "Inventory / Overview" dashboard.
4. If inventory p99 is above 500 ms, page the Inventory on-call through the PagerDuty `inventory-primary` schedule.

To pull a sample of slow requests with their trace IDs, filter the logs by
duration:

```sh
kubectl logs deploy/checkout -n shop --since 15m | grep 'duration_ms=[0-9]\{4,\}'
```

---

## Scaling

The checkout Deployment normally runs 3 replicas. During a sale, scale it to 6
before traffic arrives:

```sh
kubectl scale deploy/checkout -n shop --replicas=6
```

Scale back to 3 the day after the sale ends:

```sh
kubectl scale deploy/checkout -n shop --replicas=3
```

## After an incident

1. Open a post-incident document from the "Post-incident review" template in the team drive.
2. Link the alert, the dashboards you used, and the timeline from `#incidents`.
3. File a ticket in the `PAY` project for every follow-up action, and link each ticket in the document.
4. Book a 30-minute review with the Payments team within five working days.

## Change log

- 2026-08-20 — Added the cache eviction procedure.
- 2026-07-02 — Split provider outage out of the error-rate procedure.
- 2026-05-14 — First version.
