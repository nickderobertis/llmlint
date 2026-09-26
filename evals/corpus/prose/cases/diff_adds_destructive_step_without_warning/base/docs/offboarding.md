# Offboarding a tenant

When a tenant's contract ends, disable their access and hand their data back.

## Procedure

1. Disable sign-in for the tenant with `tenantctl disable acme-corp`.
2. Export the tenant's data with `tenantctl export acme-corp --out exports/acme-corp.tar.gz`.
3. Upload the export to the tenant's handover folder with `aws s3 cp exports/acme-corp.tar.gz s3://tenant-handover/acme-corp/`.
4. Email the handover link to the tenant's billing contact from the account page.
