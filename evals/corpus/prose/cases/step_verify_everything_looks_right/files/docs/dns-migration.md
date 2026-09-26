# Moving `status.example.com` to the new DNS provider

## Procedure

1. Lower the record's TTL to 60 seconds in the old provider's console, then wait one hour.
2. Create the record in the new provider with `dnsctl record create status.example.com CNAME status-pages.example.net --ttl 60`.
3. Switch the domain's nameservers at the registrar to `ns1.newdns.example` and `ns2.newdns.example`.
4. Make sure everything looks right.
5. After 48 hours, raise the TTL back to 3600 with `dnsctl record update status.example.com --ttl 3600`.

## Check resolution

To see which address the record resolves to from Cloudflare's public
resolver, query it with `dig`:

```sh
dig @1.1.1.1 status.example.com CNAME +short
```
