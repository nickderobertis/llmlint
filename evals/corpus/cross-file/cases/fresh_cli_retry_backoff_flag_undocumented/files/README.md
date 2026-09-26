# shipit

`shipit` rolls a build artifact out to a fleet of hosts described in a TOML
manifest, one batch at a time, and rolls it back when something goes wrong.

## Install

```sh
cargo install --path .
```

## Global options

| Flag | Description |
| --- | --- |
| `--manifest <PATH>` | Fleet manifest to read (default `fleet.toml`). |
| `-v`, `--verbose` | More logging; repeat for more (`-vv`). |
| `--json` | Print machine-readable JSON instead of text. |

## `shipit deploy <ARTIFACT>`

Pushes the artifact to every host in a group.

- `--group <NAME>` — which host group to target (default `default`).
- `--strategy <rolling|blue-green|canary>` — rollout strategy (default `rolling`).
- `--batch-size <N>` — hosts updated concurrently (default 2).
- `--retries <N>` — attempts per failing host before the deploy aborts
  (default 3). Retries back off exponentially, so a flapping host is not
  hammered.
- `--no-health-check` — skip the HTTP health probe after each batch.

```sh
shipit deploy build/app.tar --group web --strategy canary --batch-size 4
```

## `shipit rollback`

Reverts a group to an earlier build.

- `--group <NAME>` — which host group to revert.
- `--steps <N>` — how many releases to go back (default 1).

## `shipit status`

Prints the build each host is running. Combine with `--json` for scripting.

## Troubleshooting

If a deploy stalls on one host, re-run with `-vv` to see each SSH command. A
host that keeps failing its health check after all retries is left on the old
build and reported at the end of the run.

## License

MIT
