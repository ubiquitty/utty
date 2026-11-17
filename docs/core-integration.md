<!-- Copyright (C) 2026 Stanislav Yaglo -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Core integration

`ThirdParty/ubiquitty-core` is a pinned Git submodule of
[ubiquitty/core](https://github.com/ubiquitty/core). Cargo's build script builds
and installs its package, then asks CMake to resolve the exported
`Ubiquitty::ubiquitty-kit` target. The native bridge includes the public
`UbiquittyKit/UbiquittyKit.h` umbrella and three headers the package installs:
`SessionDirectoryCore.h` for listing a host, `ControlPaths.h` for the control
directory and `RemoteBootstrap.h` for validating a socket name. It has no Core
source include directory and no private transport or Unicode header.

The C++ facade `UbiquittyKit::Workspace` supplies host-owned workspace and
directory snapshots, per-pane replicated terminals, viewport updates, exact
input delivery and workspace intents. `makeHostChannelFactory` supplies local
pipe or SSH carriers. The facade lives in the Core repository; the graphical
`ubiquitty-kit` repository is not a dependency.

The bridge's C ABI carries an opaque workspace owner, copied metadata and
borrowed cell views. Rust's safe wrapper copies each view before making another
native call. The owner cannot be sent or shared between Rust threads. Dropping
it detaches its carriers while the daemon keeps its sessions. No C++ exception,
container layout or owning pointer is exposed as a Rust value.

Rust owns keyboard shortcuts, UTF-8 editing, close confirmations, pane layout,
widgets, rendering, terminal modes and outer-terminal metadata.
It forwards application bytes without decoding terminal semantics. A small
outer-terminal report framer consumes capability replies, pointer events,
focus and paste framing. It preserves other key sequences, including Kitty
key events. Core encodes pointer reports using each pane's replicated modes.
Crossterm's event reader
and cursor queries are not used, so they cannot consume bytes intended for a
hosted application. Core dispatch wakes the event loop through a pipe; delayed
work supplies its next deadline. Terminal output drains until the terminal
applies backpressure, then resumes on write readiness. There is no fixed refresh
interval or byte-rate limit. Newer Core state waits until the pending frame drains.

`utty ls` and `utty attach` list the host through a one-shot
`SessionDirectoryCore` refresh, whose HELLO names no session, so listing never
attaches. The listing reports the answering daemon's instance; `attach` pins
its target to it, which is what lets the host restore a workspace nothing runs
in by id, and refuses another daemon answering in its place. The in-session
picker pins its targets to the daemon of the attached workspace the same way.
A local daemon that only listing started is asked to stop again the way Core's
host installer retires an idle host: the process serving the socket is found by
the connection's peer credentials, must run as this user from the same host
file, and receives `SIGUSR2`, which a daemon holding any session declines.

Core owns Sync, terminal semantics, session identities, reconnection and the
replicated cells. A Rust pane widget uses Core's color resolution and cluster
widths without running a second terminal parser. The capture helper used by the
integration suite creates its own local Core terminal to inspect the emitted
physical stream.

Core's image quad collector resolves explicit and Unicode-placeholder images,
clipping and scroll position. Rust rasterizes each quad at its resolved size
and uploads it only after the outer terminal confirms Kitty graphics support.
The upload cache keys content generations and placement geometry; deletion and
detach affect only the client's own images. Core keeps the complete Sync state
even when the outer terminal cannot present an optional feature.

The integration requires Core's public color and image layout surface (Core 1.2)
and the C++ Kit 2.1 workspace facade, whose targets name the daemon instance
that listed them. Their API and regression tests are carried in the dependency
revision pinned by this checkout, which also builds the development host. There
is no client-specific patch applied during builds.
