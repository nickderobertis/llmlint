# Clearing a stuck dead-letter queue

When a poison message loops through the `orders-dlq` queue, clear it and
replay from the source topic.

## Procedure

1. Export what is in the queue with `mqctl export --queue orders-dlq --out dlq-$(date +%F).jsonl`.
2. Purge the queue with `mqctl purge --queue orders-dlq --yes`. This permanently
   deletes every message still in `orders-dlq`; anything you did not export in
   step 1 is gone for good.
3. Replay the last hour from the source topic with `mqctl replay --topic orders --since 1h`.

## Removing the queue entirely

> **Irreversible:** the next command deletes `orders-dlq` together with its
> retention settings and any messages that arrived after the purge. It cannot
> be undone.

To delete the queue, run `mqctl queue delete`:

```sh
mqctl queue delete orders-dlq --yes
```
