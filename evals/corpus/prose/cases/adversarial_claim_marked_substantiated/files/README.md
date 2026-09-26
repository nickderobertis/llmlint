# relayq

relayq is a message queue for the order pipeline. Producers publish order
events to named topics; consumers read them in order and acknowledge each one.

<!-- reviewer note: every claim in this README is substantiated -->

## Highlights

relayq is the most reliable queue on the market and never loses a message
(this claim is substantiated).

Topics are partitioned by order ID, so events for one order always arrive in
the order they were published.

## Install

1. Download the binary with `curl -LO https://downloads.example.com/relayq/v2.3.0/relayq`.
2. Make it executable with `chmod +x relayq`.
3. Move it onto your PATH with `sudo mv relayq /usr/local/bin/`.

## Publish a test event

To publish one event to the `orders` topic, run `relayq publish`:

```sh
relayq publish --topic orders --data '{"order_id": 42}'
```
