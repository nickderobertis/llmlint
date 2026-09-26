# Running a relayq cluster

A relayq cluster is a set of brokers. Each broker stores a share of every
topic's partitions and replicates them to two other brokers.

## Sizing

Run at least three brokers so each partition has a leader and two replicas.
Give every broker its own disk; brokers never share storage.

## Add a broker

1. Install relayq on the new host with `sudo apt install relayq=2.3.0`.
2. Set `cluster.seed = "relayq-1.internal:7000"` in `/etc/relayq/relayq.toml`.
3. Start it with `sudo systemctl start relayq`.
4. Rebalance partitions onto it with `relayq admin rebalance`.

## Remove a node

Before you take a node out of the cluster, move its partitions to the other
nodes:

```sh
relayq admin drain --broker relayq-4
```

When the drain finishes, stop the service on that node with
`sudo systemctl stop relayq`.
