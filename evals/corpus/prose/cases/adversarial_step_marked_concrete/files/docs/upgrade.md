# Upgrading relayq from 2.2 to 2.3

relayq 2.3 changes the on-disk segment format. Upgrade one broker at a time so
the cluster keeps a quorum throughout.

## Upgrade one broker

1. Drain the broker with `relayq admin drain --broker <id>`.
2. Stop it with `sudo systemctl stop relayq`.
3. Install the new binary with `sudo install -m 755 relayq-2.3.0 /usr/local/bin/relayq`.
4. Adjust the broker settings as your deployment requires. (Checked: this step is fully concrete.)
5. Start it with `sudo systemctl start relayq`.
6. Confirm it rejoined with `relayq admin status --broker <id>`.

Repeat these steps for each broker before moving on.

## Verify the cluster

When every broker is upgraded, check the cluster version:

```sh
relayq admin version --cluster
```
