<!--
SPDX-FileCopyrightText: 2026 Srikanth K
SPDX-License-Identifier: AGPL-3.0-or-later
-->

### norn v1.0.0-rc.3

The third release candidate for norn 1.0. Every platform archive contains the
`norn` daemon, the `nornctl` client, the built-in MCP server and documentation.
All workspace crates, including `norn-protocol`, use version `1.0.0-rc.3`.
Wire negotiation uses protocol 1.9; client and daemon agree the lower minor, so
v1.0.0-rc.2 (1.8), v1.0.0-rc.1 (1.7) and v0.1.0 (1.6) clients and daemons still connect.

This release adds a daemon-wide disk budget: `min_free_bytes` (256 MiB by default)
and `max_disk_bytes` refuse batches before the disk fills, and admit them again once
room returns, closing security review finding L1; a stream's size retention now also
applies as each segment seals. It adds remote gRPC: `[remote]` serves the API to other
machines over TLS with a bearer token, and `nornctl --remote` reaches it. And it adds
hard per-stream quotas (`Stream.quota`, `nornctl quota`), which refuse batches and
delete nothing. Release archives include remote gRPC, whose TLS is rustls with ring;
the engine stays pure Rust, and a plain source build needs no C compiler.

Upgrading. Setting the first stream quota moves the metadata log to version 3, which
v1.0.0-rc.2 does not open; a database that never sets one stays readable by it. A
volume with 256 MiB or less free now refuses batches unless `min_free_bytes = 0`.

Known issues. `listen = "tcp:..."` and MCP over HTTP, when turned on, are open to
every local user, and the daemon only warns at start; use the Unix socket, a named
pipe, MCP over stdio, or `[remote]` with its token instead. A maximum-speed replay of a
recent window may stop after `RUNNING`; this remains under investigation. If the disk
does fill, a stream refuses batches until the daemon restarts.
See the [v1.0.0-rc.3 changelog](https://github.com/shokki-engineering/norn/blob/v1.0.0-rc.3/docs/project/changelog.md).

`norn.rb` installs the complete program from these archives and supplies a
Homebrew service. See the [Homebrew guide](https://github.com/shokki-engineering/norn/blob/main/docs/guides/homebrew.md)
for tap authentication, `brew services start norn`, configuration and health checks.

```sh
brew install shokki-engineering/tap/norn
brew services start norn
nornctl --endpoint "unix:$(brew --prefix)/var/run/norn/norn.sock" status
```

The daemon runs in the background and starts at login. The release workflow
updates the Homebrew tap as Walle. Installation requires no GitHub token.

Download the archive for your platform and verify it against its `.sha256` file.
Extract it, put the binaries on your `PATH`, then run:

```sh
norn --help
nornctl --help
norn
```

In another terminal:

```sh
nornctl status
nornctl streams create example
```

Linux and macOS archives use `.tar.gz`; Windows archives use `.zip` and contain
`norn.exe` and `nornctl.exe`. Linux binaries require glibc 2.35 or newer.

See the included docs, the [getting started guide](https://github.com/shokki-engineering/norn/blob/main/docs/guides/getting-started.md)
the [configuration reference](https://github.com/shokki-engineering/norn/blob/main/docs/guides/configuration.md), the
[AI agents guide](https://github.com/shokki-engineering/norn/blob/main/docs/guides/ai-agents.md) for `norn --mcp`, or the
[online documentation](https://github.com/shokki-engineering/norn#contents).

Norn is [AGPL-3.0-or-later](https://github.com/shokki-engineering/norn/blob/main/LICENSE) with a separate
[commercial licence](https://github.com/shokki-engineering/norn/blob/main/LICENSE-COMMERCIAL.md), every part of it with no
exception. Licence files accompany each archive.
The source for each release is available in GitHub’s source archives below.
