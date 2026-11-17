<!-- Copyright (C) 2026 Stanislav Yaglo -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# utty

`utty` is a terminal client for persistent Ubiquitty workspaces. Ratatui lays
out the interface and works out which cells changed; `utty` itself reads input,
writes output and waits on the terminal, the host and Core with `poll(2)`. The
C++ library from
[ubiquitty/core](https://github.com/ubiquitty/core) owns the replicated terminal
state, Sync, session identities, workspace operations and reconnection.

The host keeps shells alive after the client detaches. The client draws Core's
cells; it does not parse application escape sequences or implement Sync.

User documentation: [ubiquitty.org/platforms/utty](https://ubiquitty.org/platforms/utty/).

## Build

Requires Rust 1.88 or newer, CMake 3.24+, Ninja, Python 3, Git and a C++23
compiler. Use Apple Clang on macOS, or Clang with libc++ and LLD on Linux.
Cargo's Linux target configuration selects Clang/LLD to link Core's ThinLTO archives.

```sh
git clone --recurse-submodules https://github.com/ubiquitty/utty.git
cd utty
cargo build --release --locked
cargo test --release --locked
```

Cargo builds the pinned `ThirdParty/ubiquitty-core` submodule, installs its Core
package inside Cargo's build directory, then compiles the native bridge against
that installed package. The bridge has no source-tree include paths.
`Cargo.lock` pins the Rust dependencies; the submodule gitlink pins Core.

To build a matching optimized host and put convenient binaries in `build/`:

```sh
scripts/build.sh --host
# BUILD_TYPE=Debug scripts/build.sh --host
```

That script also runs the Rust tests. `JOBS=4` and `BUILD_TYPE=Release` are its
defaults. Select an existing SDK with `UBIQUITTY_CORE_SDK=/path/to/sdk`; its
build type, compiler and C++ standard library must match the bridge.

To put `utty` on your `PATH`, copy the binary, for example
`install -m 755 build/utty ~/.local/bin/`. An installed `utty` finds its host as
described below; copy `build/ubiquitty-host` beside it only if you want that
host rather than the one the Ubiquitty app installs.

## Run

```sh
build/utty              # create a workspace
build/utty -w work      # open workspace "work", creating it if needed
build/utty attach       # open the most recently used workspace
build/utty attach work  # open a workspace by name or ID
build/utty ls           # list the host's workspaces
```

Without `--host-binary`, the client uses the host the macOS app installs and
updates in Ubiquitty's control directory, `~/.ubiquitty/ubiquitty-host` (or
under `$UBIQUITTY_HOME` when that is set), so it reaches the daemon the app's
sessions run in; then the `ubiquitty-host` beside its own executable, which the build script
places in `build/` and `target/release/`; then one on `PATH`. It exits with an
error if none exists. A daemon's socket is derived from its host binary, so a
different host binary means a different daemon and different workspaces.

Every command starts the host daemon if needed: the daemon exits when its last
live session ends, but the workspaces it records outlive it and are listed and
restored only by a running daemon. A daemon that `ls` started, or that `attach`
started and found nothing to open in, is asked to stop again; one holding any
session declines. An explicit `--socket` names a daemon that must already run.
`attach` without a name picks the live workspace typed into last, or, with none
live, the newest restorable one. A workspace `ls` shows as restorable runs
nothing; attaching to it by name or ID recreates it from the host's record, in
its recorded directories. `-w NAME` opens or creates a named workspace.
`--session ID` and `--workspace-id ID` select a target directly. A missing
session is refused.

Detaching prints the command that returns, keeping the options you chose:

```sh
build/utty attach work
build/utty --session 42
```

Reconnects stay pinned to the acknowledged daemon and do not replay
application input.

For a host reached over SSH:

```sh
build/utty --ssh user@machine
```

Over SSH the client runs the host Ubiquitty installs on a server,
`~/.ubiquitty/ubiquitty-host`; name another with `--host-binary`, as a path or
a command on the remote `PATH`. Authenticate and accept the host key with
normal SSH first. The Core factory uses noninteractive SSH; host deployment is
explicit. Retain `--ssh` and `--host-binary` when reattaching remotely.

The client and host must speak a common Sync version. The host the Ubiquitty
app installs normally does; when developing against a newer Core, pass
`--host-binary build/ubiquitty-host` to use the host built from the pinned
revision.

## Keys

The prefix is **Ctrl-]**, followed by one key.

| Key | Action |
| --- | --- |
| `d` | Detach and keep the workspace running |
| `w` / `s` | Find a workspace, window, tab or pane: type to filter |
| `c`, `n` / `p`, `1`–`9` | Create a tab, move between tabs, select a tab |
| `|` / `-` | Split side by side / above and below |
| `o`, `H` / `J` / `K` / `L` | Next pane, focus left/down/up/right |
| `z`, `=` | Toggle zoom, balance splits |
| `+` / `_`, `S` | Grow/shrink a pane, swap with the next pane |
| `t`, `m` | Extract a pane to a tab, move it into the next tab |
| `,`, `R` | Rename tab, rename workspace |
| `x`, `X`, `!` | Close pane, tab, window after confirmation |
| `W`, `N`, `C` | Next window, new window, new workspace |
| `l`, `?` | Redraw, show key bindings |
| Ctrl-] | Send a literal Ctrl-] |

The picker lists every workspace with its windows, tabs and panes, marking the
attached one and showing each workspace's size, attendance and last activity,
or that it is restorable. Typing filters it by fuzzy match against each row's
workspace, tab, title and directory, best match first. Arrows and Ctrl-P/N
select; Enter opens the selected target, focusing a pane of the attached
workspace in place and recreating a restorable one; Esc clears the filter, then
closes the view. Help supports scrolling and Page Up/Down. Rename edits
UTF-8 by grapheme, including arrows, Home/End, Backspace/Delete and Ctrl-U.
Application input, including NULs and fragmented escape sequences, is forwarded
unchanged. Modal interface input is handled separately.

## Rendering and terminal profiles

The default profile uses UTF-8, truecolor and the alternate screen. A custom
Ratatui widget projects Core's resolved cells, colors, attributes and complete
Unicode clusters. Rust owns tabs, pane headers, layout, help, prompts and the
workspace picker. The host remains authoritative for the workspace structure.

The bottom bar shows the workspace and its tabs as powerline segments, with each
tab's program status, and on the right the focused program's status (its app or
title, message or what it is waiting for, and progress) beside the session
number. Tabs scroll around the active one when they do not fit. The arrows need
a font or terminal that draws the Powerline glyphs; `--no-powerline` keeps the
colors without them.

Modern terminals also get mouse clicks, drags, motion and wheel events,
bracketed paste, focus reports, synchronized screen updates, cursor shapes,
colored underline styles and OSC 8 hyperlinks. Mouse input targets the pane
under the pointer; clicks focus panes and tabs. The wheel follows the hosted
application's mouse and alternate-scroll modes, or scrolls Core's history.
Pasted text bypasses TUI shortcuts.

The client probes for Kitty graphics, enhanced keyboard reporting and pixel
mouse coordinates. Supported terminals receive Core's images, including
Unicode-placeholder placements, with clipping, scaling and content updates.
Unchanged images stay cached. Terminals without Kitty graphics continue to
display text. Application colors remain intact when the launcher sets
`NO_COLOR`. `--legacy` and `--ascii` select monochrome output explicitly.

The outer terminal receives the selected window's title, working directory,
program status and progress. A blocked background tab still marks the outer
window. Detaching clears the client's status and restores the title stack.

`--legacy` produces seven-bit monochrome DEC output; `--ascii` uses ASCII
approximations. `--geometry COLSxROWS` selects a fixed physical size. Output
drains whenever the terminal is writable; Core events wake the client directly.
There is no refresh interval or output throttle. A pending frame stays intact
under terminal backpressure while Core's dispatcher continues to run.
Termios, descriptor flags, cursor visibility and autowrap are restored on exit.
The serial profiles preserve the caller's XON/XOFF setting. Set the physical
terminal's geometry and serial speed separately.

## Development

```sh
cargo fmt --check
cargo clippy --release --locked --all-targets -- -D warnings
cargo test --release --locked
scripts/build.sh --host
python3 tests/smoke.py
python3 tests/modern.py
# Optional: compare complete Neovim repaints and check MC when installed.
python3 tests/refresh.py
```

The smoke test starts an isolated daemon and uses private PTYs. It covers
workspace and tab operations, splits, shared edits, program statuses, geometry,
reattachment, reconnect, shell persistence, output backpressure and terminal cleanup.
It does not use the user's running daemon. Physical serial hardware and remote
SSH are not exercised by that local suite.
The modern suite checks exact mouse, paste, focus and keyboard bytes through
Sync, as well as RGB colors and the pixels, caching and lifecycle of images.

The app follows the Cargo layout: `src/` contains Rust code, `native/` contains
the C++ ABI bridge, `tests/` holds the integration harness, and `ThirdParty/`
holds the pinned Core submodule. See [Core integration](docs/core-integration.md)
for the public API boundary and ownership contract.

## License

Apache-2.0. See [LICENSE](LICENSE) and [Core's notices](ThirdParty/ubiquitty-core/NOTICE).
