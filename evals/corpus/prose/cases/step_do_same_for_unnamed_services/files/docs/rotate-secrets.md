# Rotating the payments API key

The payments provider issues one API key per environment. Rotating it means
creating a new key, rolling it out to every service that calls the provider,
and then revoking the old key.

## Rotate

1. Create a new key in the provider dashboard under **Developers → API keys → Create key**.
2. Store it with `vault kv put secret/payments/api-key value=<new-key>`.
3. Restart the checkout service with `kubectl rollout restart deploy/checkout -n shop` so it reads the new key.
4. Do the same for the other services that use it.
5. Revoke the old key in the provider dashboard under **Developers → API keys**.

## Confirm checkout picked it up

To see which key version the checkout pods loaded, read the startup log line:

```sh
kubectl logs deploy/checkout -n shop | grep "payments key version"
```
