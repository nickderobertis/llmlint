# Changelog

## 3.0.0 — 2026-09-10

### Faster cold starts

Cold starts are much faster in 3.0: median time to first request dropped from
1.8 s to 0.4 s on the `t4g.small` reference instance, measured over 500 starts
(see [bench/cold-start.md](bench/cold-start.md)).

### Removed

- The `--legacy-auth` flag. Use `--auth token` instead.

## 2.9.0 — 2026-06-02

### Smaller image

The container image is smaller: 212 MB down to 148 MB.
