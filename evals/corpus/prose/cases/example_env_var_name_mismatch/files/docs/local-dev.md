# Local development

The API reads its database connection string from the `DATABASE_URL`
environment variable and refuses to start without it.

## Point the API at your local database

Export `DATABASE_URL` with your local connection string, then start the API:

```sh
export DB_URL=postgres://dev:dev@localhost:5432/api_dev
make run
```

## Run the tests

To run the unit tests, which use an in-memory database and need no
connection string, run `make test`:

```sh
make test
```
