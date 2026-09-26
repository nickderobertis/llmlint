# ADR 0014: Serve static assets from an EU bucket

- **Status:** Accepted
- **Date:** 2026-08-11
- **Deciders:** Web Platform team
- **Supersedes:** ADR 0006 (single US asset bucket)

## Context

The storefront's static assets — images, fonts, JavaScript bundles, and CSS —
are served from one bucket, `shop-assets-us`, in `us-east-1`, behind the CDN.
Since the EU launch in April, 58% of storefront page views come from EU
countries (analytics, June 2026).

Two problems follow from keeping the only origin in the US:

1. **CDN misses are slow for EU users.** A cache miss from Frankfurt to
   `us-east-1` adds a median 92 ms to the asset request, measured with the
   CDN's origin-timing logs over two weeks in June.
2. **Data residency.** Product images uploaded by EU merchants must be stored
   in the EU under the merchant agreement signed in March (section 7.2).

Moving the whole storefront origin is out of scope; this ADR covers static
assets only.

## Decision

We will create a second bucket, `shop-assets-eu`, in `eu-west-1`, and make it
the origin for EU traffic. The CDN routes by the viewer's country: EU viewers
hit `shop-assets-eu`, everyone else keeps hitting `shop-assets-us`.

Merchant uploads from EU merchants are written only to `shop-assets-eu`.
Assets we build ourselves (bundles, fonts, CSS) are written to both buckets by
the deploy pipeline.

## Options considered

### Option A: keep one US bucket, raise CDN TTLs

Raising the CDN TTL from 1 day to 30 days would cut misses, but in a one-week
trial it only lowered the EU miss rate from 6.1% to 4.8%, because most misses
are first requests for new merchant images. It also does nothing for data
residency. Rejected.

### Option B: one bucket in `eu-west-1` for everyone

This fixes residency and EU latency, but moves the 92 ms miss penalty onto
US users, who are still 42% of page views. Rejected.

### Option C: one bucket per region, routed by the CDN (chosen)

Each region reads from a nearby origin, and EU merchant uploads stay in the
EU. The cost is keeping build output in two buckets, which the deploy pipeline
already knows how to do for the two app regions.

## Consequences

- The deploy pipeline uploads build output twice. This adds about 40 seconds
  to a deploy, measured on the staging pipeline.
- Storage cost for build output doubles. Build output is 3.2 GB, so the
  increase is under $1 per month at current prices.
- Merchant image URLs for EU merchants change host. The image service already
  resolves URLs through `asset_url()`, so no stored URLs need rewriting.
- On-call gets one more bucket to watch. The "Assets / Origin" dashboard gains
  a panel per bucket.

## Implementation

### Create the bucket

Create the EU bucket in `eu-west-1` with versioning enabled:

```sh
aws s3api create-bucket --bucket shop-assets-eu --region eu-west-1 --create-bucket-configuration LocationConstraint=eu-west-1
aws s3api put-bucket-versioning --bucket shop-assets-eu --versioning-configuration Status=Enabled
```

### Block public access

The bucket is read only through the CDN's origin access identity. Block all
public access on it:

```sh
aws s3api put-public-access-block --bucket shop-assets-eu --public-access-block-configuration BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
```

### Copy existing build output

Copy the current build output from the US bucket into the new EU bucket, so
the EU origin can serve the live release on day one:

```sh
aws s3 sync s3://shop-assets-eu/build/ s3://shop-assets-us/build/ --source-region eu-west-1 --region us-east-1
```

### Copy existing EU merchant images

EU merchant images are listed in `eu-merchants.txt`, one merchant ID per line.
Copy each merchant's folder from the US bucket to the EU bucket:

```sh
while read -r m; do aws s3 sync "s3://shop-assets-us/merchants/$m/" "s3://shop-assets-eu/merchants/$m/"; done < eu-merchants.txt
```

The US copies are kept until the residency review in step 5 of the rollout
signs off; deleting them is a separate change with its own review.

### Route the CDN

The CDN configuration lives in `infra/cdn/storefront.tf`. Add the EU origin
and a geo rule that sends EU viewers to it, then plan the change:

```sh
terraform -chdir=infra/cdn plan -out cdn.tfplan
```

## Rollout

1. Create and lock down the bucket with the commands under [Create the bucket](#create-the-bucket) and [Block public access](#block-public-access).
2. Copy build output and EU merchant images with the commands under [Copy existing build output](#copy-existing-build-output) and [Copy existing EU merchant images](#copy-existing-eu-merchant-images).
3. Enable the dual upload in the deploy pipeline by setting `ASSET_BUCKETS=shop-assets-us,shop-assets-eu` in `.ci/deploy.env`.
4. Apply the CDN change with `terraform -chdir=infra/cdn apply cdn.tfplan`.
5. Ask Legal to confirm the residency requirement is met, by filing a `LEGAL` ticket that links this ADR.

## Measuring the result

The success criterion is the EU asset-miss penalty: the median added latency
of an EU cache miss should fall from 92 ms to under 20 ms. It is read from the
same CDN origin-timing logs used in the Context section, over the two weeks
after step 4 of the rollout.

## References

- ADR 0006: single US asset bucket.
- Merchant agreement, section 7.2 (data residency).
- CDN origin-timing logs, June 2026 export.
