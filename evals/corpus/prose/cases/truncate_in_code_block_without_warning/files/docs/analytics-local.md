# Working with the local analytics database

The analytics service writes every tracked event into the `events` table of
the local `analytics_dev` database. Tests and manual runs both add rows.

## Load sample events

To load the committed sample of 10,000 events, run the seed script:

```sh
psql -d analytics_dev -f seeds/events-sample.sql
```

## Start fresh

To start fresh before a demo, clear the events table:

```sh
psql -d analytics_dev -c "TRUNCATE events"
```

## Count events by type

To see how many events of each type are loaded, group by `event_type`:

```sh
psql -d analytics_dev -c "select event_type, count(*) from events group by 1"
```
