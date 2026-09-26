# mailroute

mailroute is a transactional email relay. Your application hands it a message
over SMTP or HTTP; mailroute signs it, queues it, and delivers it.

## Features

| Feature | Details |
|---|---|
| DKIM signing | Signs every message with a per-domain 2048-bit key. |
| Retry queue | Retries soft bounces for up to 72 hours with exponential backoff. |
| Deliverability | Industry-leading inbox placement across all major providers. |
| Throughput | 3,000 messages per second per instance ([bench/throughput.md](bench/throughput.md)). |

## Send a test message

To send a test message to yourself over HTTP, post it to `/v1/send`:

```sh
curl -X POST http://localhost:8025/v1/send -d '{"to": "you@example.com", "subject": "test", "text": "hello"}'
```
