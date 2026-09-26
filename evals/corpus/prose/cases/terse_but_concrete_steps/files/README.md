# tinyhttp

A small HTTP/1.1 server library in C.

## Build

1. `./configure --prefix=/usr/local`
2. `make`
3. `make test`
4. `sudo make install`

## Use it

Link against the installed library with `-ltinyhttp`:

```sh
cc -o hello hello.c -ltinyhttp
```
