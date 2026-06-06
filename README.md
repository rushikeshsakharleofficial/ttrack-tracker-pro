# ttrack-tracker-pro

Rust implementation of `ttrack`: a Linux terminal session recorder and audit daemon inspired by the Go `ttrack-tracker` project.

This repository is intentionally separate from `ttrack-tracker` so the existing Go production flow is not touched.

## Goal

Provide the same public behavior as the original tool:

- `ttrack rec [-q] [-o file] [cmd...]` records a command/shell session.
- `ttrack play [--speed N] <file|id>` replays local or central recordings.
- `ttrack ls`, `ttrack ls --all`, `ttrack ls --user <name>` list recordings.
- `ttrack tail [-n N] <id>` and `ttrack tail -f <id>` inspect central sessions.
- `ttrack search [--user U] [-i] <pattern>` searches recordings.
- `ttrack export [-o file] <id>` decrypts a central session to plaintext.
- `ttrack tree` shows users and sessions.
- `ttrack prune --yes` removes old central sessions.
- `ttrackd` receives sessions over a Unix socket and stores them encrypted in a root-only central store.

## Status

This is a Rust mimic/pro implementation scaffold. It keeps command names, file format direction, central-store layout, encryption model, and daemon/client split close to the original project. Before production use, run full tests on Rocky/Ubuntu and compare behavior against the existing Go version.

## Build

```bash
cargo build --release
```

Binaries:

```text
target/release/ttrack
target/release/ttrackd
```

## Quick usage

```bash
./target/release/ttrack rec /bin/bash -c 'echo hello from rust ttrack'
./target/release/ttrack ls
./target/release/ttrack play ~/.local/share/ttrack/<session>.cast
```

Start daemon manually for central store testing:

```bash
sudo ./target/release/ttrackd
```

Default paths:

```text
socket:       /run/ttrackd.sock
central dir:  /var/lib/ttrack
key file:     /var/lib/ttrack/.ttrack.key
local dir:    ~/.local/share/ttrack
config:       /etc/ttrack/ttrack.conf
```

## Important note

The existing Go repository remains untouched. This repo is for the Rust/pro line only.
