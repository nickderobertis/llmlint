# Rotating the database TLS certificate

The primary and its replica each serve their own certificate from
`/etc/postgresql/tls/server.crt`. Rotate both in the same maintenance window.

## Rotate

1. On the primary (`db-1`), copy the new certificate into place with `sudo cp certs/2026/server.crt certs/2026/server.key /etc/postgresql/tls/`.
2. On `db-1`, reload Postgres with `sudo systemctl reload postgresql`.
3. Repeat steps 1 and 2 on the replica, `db-2`.
4. From your workstation, confirm both hosts serve the new certificate with `openssl s_client -connect db-1:5432 -starttls postgres` and the same command against `db-2`.

## Check the expiry date

To print the new certificate's expiry date, run `openssl x509` on it:

```sh
openssl x509 -in certs/2026/server.crt -noout -enddate
```
