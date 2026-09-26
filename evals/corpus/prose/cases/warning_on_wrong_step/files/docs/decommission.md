# Decommissioning the legacy archive service

The legacy archive service stored invoice PDFs before the move to the document
store. All invoices were copied to the document store in March; this procedure
shuts the old service down and removes its storage.

## Procedure

1. Stop the service with `systemctl stop archive-legacy` on `archive-1`.

   > **Warning:** stopping the service makes the old `/archive` URLs return 503
   > until the redirect in step 2 is live. Schedule this outside business hours.

2. Deploy the redirect with `kubectl apply -f deploy/archive-redirect.yaml`.
3. Disable the service at boot with `systemctl disable archive-legacy`.
4. Delete the bucket and every object in it with `aws s3 rb s3://invoices-archive-legacy --force`.
5. Remove the host from inventory with `ansible-inventory --remove archive-1`.

## Confirm the redirect

To check that an old invoice URL now redirects, request one with curl:

```sh
curl -I https://archive.example.com/invoices/2019/INV-1001.pdf
```
