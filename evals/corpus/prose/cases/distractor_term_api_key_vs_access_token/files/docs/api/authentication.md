# Authenticating to the Fleet API

Every request to the Fleet API must carry an API key. This page covers how to
create an API key, send it, rotate it, and what the error responses mean.

## API keys

An API key is a secret string that identifies your organization and the scopes
you granted it. API keys look like `flt_live_` followed by 40 characters. Test
mode API keys start with `flt_test_` and only see test-mode vehicles.

Each API key has:

- a **name** you choose, shown in the dashboard and in audit logs;
- a set of **scopes**, for example `vehicles:read` or `trips:write`;
- an optional **expiry date**, after which requests with it are rejected.

An organization can have up to 50 active API keys.

## Create an API key

1. In the Fleet dashboard, open **Settings → Developers → API keys**.
2. Click **Create API key**.
3. Enter a name, for example `billing-sync-prod`.
4. Tick the scopes the integration needs; for a read-only integration tick only `vehicles:read` and `trips:read`.
5. Click **Create** and copy the API key from the dialog. It is shown only once.

To create an API key from the command line instead, run `fleet keys create`
with the name and scopes:

```sh
fleet keys create --name billing-sync-prod --scope vehicles:read --scope trips:read
```

## Send the API key

Send the API key in the `Authorization` header with the `Bearer` scheme:

```sh
curl -H "Authorization: Bearer $FLEET_API_KEY" https://api.fleet.example.com/v2/vehicles
```

Requests without the header, or with a malformed header, get HTTP 401 with
the error code `missing_credentials`.

The official SDKs read the API key from the `FLEET_API_KEY` environment
variable when you do not pass one explicitly:

```python
from fleet import Client

client = Client()  # reads FLEET_API_KEY
vehicles = client.vehicles.list(limit=10)
```

## Scopes

Scopes limit what a request can do. A request that needs a scope the API key
lacks gets HTTP 403 with the error code `insufficient_scope`, and the response
body names the missing scope.

| Scope | Allows |
|---|---|
| `vehicles:read` | List and read vehicles. |
| `vehicles:write` | Create, update, and retire vehicles. |
| `trips:read` | List and read trips. |
| `trips:write` | Start, update, and end trips. |
| `webhooks:manage` | Create and delete webhook endpoints. |

Grant the smallest set of scopes the integration needs. You cannot add scopes
to an existing API key; create a new one with the scopes you need and rotate
to it.

## Rate limits

Each API key may make 100 requests per second, with bursts up to 200. Over the
limit, the API returns HTTP 429 with a `Retry-After` header in seconds.

To see how much of the limit a request used, read the `X-RateLimit-Remaining`
response header:

```sh
curl -si -H "Authorization: Bearer $FLEET_API_KEY" https://api.fleet.example.com/v2/vehicles | grep -i x-ratelimit-remaining
```

## Rotate an access token

Rotate a credential when someone who had it leaves the team, when it may have
leaked, or on your regular schedule. Rotation gives the new and old access
tokens an overlap window so you can switch without downtime.

1. Create the replacement with `fleet keys create --name billing-sync-prod-2 --scope vehicles:read --scope trips:read`.
2. Store the new access token in your secret manager under the same name the integration already reads, for example `vault kv put secret/billing-sync FLEET_API_KEY=<new-value>`.
3. Redeploy the integration so it picks up the new value.
4. In **Settings → Developers → API keys**, check that the old key's **Last used** time has stopped advancing.
5. Revoke the old one with `fleet keys revoke billing-sync-prod`. Revoking is immediate and permanent: any service still sending the old value gets HTTP 401 from its next request, and a revoked credential cannot be restored.

## Expiry

If you set an expiry date, the API starts adding a `Fleet-Key-Expires-Soon`
response header 14 days before it. Watch for that header in your client logs
and rotate before the date.

To list your API keys with their expiry dates, run `fleet keys list`:

```sh
fleet keys list --columns name,expires_at
```

## Audit log

Every request is recorded in the audit log with the API key's name, the
endpoint, and the response status. Records are kept for 400 days.

To export last week's audit records for one API key as JSON lines, run
`fleet audit export` with `--key` and `--since`:

```sh
fleet audit export --key billing-sync-prod --since 7d --format jsonl > audit.jsonl
```

## Error reference

| Status | Code | Meaning |
|---|---|---|
| 401 | `missing_credentials` | No `Authorization` header, or it is malformed. |
| 401 | `invalid_key` | The API key does not exist or was revoked. |
| 401 | `key_expired` | The API key is past its expiry date. |
| 403 | `insufficient_scope` | The API key lacks a scope the request needs. |
| 429 | `rate_limited` | Over the API key's rate limit; see `Retry-After`. |

## Security guidance

- Never put an API key in client-side code or a mobile app; anyone can read it
  there.
- Use one API key per integration, so you can revoke one without touching the
  others.
- Store API keys in a secret manager, not in source control. The repository
  secret scanner rejects pushes that contain a `flt_live_` string.
