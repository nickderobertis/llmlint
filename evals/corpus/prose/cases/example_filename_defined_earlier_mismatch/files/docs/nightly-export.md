# The nightly export

Every night the analytics team pulls the previous day's orders into a single
Parquet file. Throughout this guide we call that file the *nightly file*, and
it is always named `orders-nightly.parquet`.

## Why Parquet

The warehouse loader reads columnar files directly, so the export skips the
CSV step the old pipeline used. Column types come from the `orders` table
schema, so no type inference runs at load time.

## Produce the nightly file

With the database URL exported as `ORDERS_DB`, write yesterday's orders to the
nightly file:

```sh
orders-export --db "$ORDERS_DB" --day yesterday --output orders-nightly.csv
```

## Upload it

To copy the nightly file to the warehouse landing bucket, run:

```sh
aws s3 cp orders-nightly.parquet s3://warehouse-landing/orders/
```
