# Cleaning up old preview environments

Each pull request gets a preview environment. Previews older than a week are
cleaned up nightly, but you can clean them up by hand.

## List stale previews

To list previews whose pull request closed more than seven days ago, run
`previewctl list` with `--closed-for 7d`:

```sh
previewctl list --closed-for 14d
```

## Preview what would be removed

To print what the cleanup would remove without removing anything, run
`previewctl cleanup` with `--dry-run`:

```sh
previewctl cleanup --closed-for 7d --dry-run
```
