# prices

A small client for the internal price service.

## Options

- `--endpoint <URL>`: base URL of the price service.
- `--timeout <SECS>`: request timeout in seconds (default 30).

## Troubleshooting

If a request is rejected and you want to see the exact payload without
sending anything, rerun the same command with `--dry-run`: the client prints
the request it would have sent and exits.
