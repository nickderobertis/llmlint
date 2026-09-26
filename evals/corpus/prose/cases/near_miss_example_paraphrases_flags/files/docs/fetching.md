# Fetching release artifacts

Release artifacts are published at `https://downloads.example.com/agent/`.
The `latest` URL redirects to the newest versioned file.

## Download the newest build

To download the newest build, following the redirect and saving it under the
file name the server uses, run curl with its follow-redirects and
keep-remote-name options:

```sh
curl -L -O https://downloads.example.com/agent/latest/agent-linux-amd64.tar.gz
```

## Unpack it

To unpack the archive into the current directory, showing each file as it is
extracted:

```sh
tar -xvzf agent-linux-amd64.tar.gz
```

## Check the checksum

To compare the archive against the published SHA-256 list without printing a
line for every file that matches, pass `--quiet` to `sha256sum --check`:

```sh
sha256sum --check --quiet SHA256SUMS
```
