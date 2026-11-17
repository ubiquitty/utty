#!/usr/bin/env python3
# Copyright (C) 2026 Stanislav Yaglo
# SPDX-License-Identifier: Apache-2.0
"""Real host/Sync/PTY test. Uses an isolated UBIQUITTY_HOME, never the user's daemon."""
import argparse
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import shlex
import signal
import struct
import subprocess
import tempfile
import termios
import time


class Terminal:
    def __init__(self, client, dump, host, env, socket, extra=(), geometry=(80, 24)):
        self.dump = dump
        self.columns, self.rows = 80, 24
        self.master, self.slave = pty.openpty()
        self.saved = termios.tcgetattr(self.slave)
        self.resize(*geometry)
        self.output = bytearray()
        self.process = subprocess.Popen(
            [client, "--host-binary", host, "--socket", socket, *extra],
            stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env=env, start_new_session=True,
        )

    def resize(self, columns, rows):
        self.columns, self.rows = columns, rows
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        if hasattr(self, "process"):
            self.process.send_signal(signal.SIGWINCH)

    def drain(self, timeout=0.05):
        if select.select([self.master], [], [], timeout)[0]:
            self.output.extend(os.read(self.master, 65536))
        assert len(self.output) < 4 * 1024 * 1024, "unexpected output growth"

    def screen(self):
        return subprocess.check_output(
            [self.dump, "--dump-utf8", str(self.columns), str(self.rows)],
            input=self.output, timeout=5,
        ).decode("utf-8")

    def metadata(self):
        return subprocess.check_output([self.dump, "--dump-meta", str(self.columns), str(self.rows)], input=self.output, timeout=5).decode("utf-8")

    def wait(self, predicate, timeout=20):
        deadline = time.monotonic() + timeout
        screen = ""
        while time.monotonic() < deadline:
            self.drain()
            screen = self.screen()
            if predicate(screen):
                return screen
            if self.process.poll() is not None:
                raise AssertionError(f"client exited {self.process.returncode}: {bytes(self.output[-1500:])!r}")
        raise AssertionError(f"timed out; screen:\n{screen}\nwire tail: {bytes(self.output[-1000:])!r}")

    def send(self, text):
        os.write(self.master, text.encode("ascii"))

    def session(self, excluding=0):
        def established(screen):
            found = re.search(r"#(\d+)\b", screen)
            return found and int(found[1]) not in (0, excluding) and "connecting" not in screen and "loading screen" not in screen and "loading workspace" not in screen
        return int(re.search(r"#(\d+)\b", self.wait(established))[1])

    def finish(self, signal_number=None):
        if signal_number:
            self.process.send_signal(signal_number)
        else:
            self.send("\x1dd")
        self.wait_exit(0)

    def wait_exit(self, expected):
        deadline = time.monotonic() + 10
        while self.process.poll() is None and time.monotonic() < deadline:
            self.drain()
        assert self.process.wait(timeout=1) == expected, bytes(self.output[-1500:])
        self.drain()
        restored = termios.tcgetattr(self.slave)
        expected = list(self.saved)
        # Darwin sets PENDIN when canonical mode is restored. It is pending
        # driver work, rather than one of the saved terminal settings.
        pending = getattr(termios, "PENDIN", 0)
        restored[3] &= ~pending
        expected[3] &= ~pending
        assert restored == expected, f"termios was not restored: expected {expected!r}, got {restored!r}"
        assert b"\x1b[?7h" in self.output and b"\x1b[?25h" in self.output

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        os.close(self.master)
        os.close(self.slave)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--client", default="build/utty")
    parser.add_argument("--dump", default="build/utty-capture")
    parser.add_argument("--host", default=".sdk/host/ubiquitty-host")
    args = parser.parse_args()
    client, dump, host = (str(Path(value).resolve()) for value in (args.client, args.dump, args.host))
    socket = "host-0123456789abcdef.sock"
    with tempfile.TemporaryDirectory(prefix="ubq-tui-") as directory:
        env = dict(os.environ, UBIQUITTY_HOME=directory, TERM="vt100", TMUX="/tmp/stale-review,123,0", TMUX_PANE="%7", STY="old.screen", WINDOW="3")
        log_path = Path(directory) / "host.log"
        terminals = []
        with log_path.open("wb") as log:
            daemon = subprocess.Popen([host, "--daemon", "--socket", socket, "--quic-port", "-1"],
                                      env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                deadline = time.monotonic() + 10
                while not (Path(directory) / socket).exists():
                    assert daemon.poll() is None, log_path.read_text()
                    assert time.monotonic() < deadline, "host did not bind"
                    time.sleep(0.02)
                # Hold the carrier before Hello so the connecting frame is
                # observable even on a fast local host, at a larger geometry.
                delayed_host = Path(directory) / "delayed-host"
                delayed_host.write_text("#!/bin/sh\nsleep 2\nexec " + shlex.quote(host) + ' "$@"\n')
                delayed_host.chmod(0o700)
                connecting = Terminal(client, dump, str(delayed_host), env, socket, geometry=(132, 53))
                terminals.append(connecting)
                connecting.wait(lambda screen: "connecting" in screen)
                assert all(not row.strip() for row in connecting.screen().splitlines()[:-1]), "connecting frame painted an unbound replica"
                first = connecting
                first.resize(80, 24)
                original = first.session()
                first.send("stty -echo; UBQ_TUI_SMOKE=kept; printf 'SYNC_%s\\n' 'READY'\r")
                first.wait(lambda screen: "SYNC_READY" in screen)
                first.send("printf 'ENV_%s_%s_%s_%s\\n' \"${TMUX-unset}\" \"${TMUX_PANE-unset}\" \"${STY-unset}\" \"${WINDOW-unset}\"\r")
                first.wait(lambda screen: "ENV_unset_unset_unset_unset" in screen)
                first.send("printf '\\033[38;2;12;34;56m\\347\\225\\214\\033[0m MODERN_%s\\n' 'OK'\r")
                first.wait(lambda screen: "MODERN_OK" in screen)
                assert b"38;2;12;34;56" in first.output
                assert "界".encode() in first.output
                first.resize(132, 53)
                first.send("printf 'SIZE_%s\\n' \"$(stty size)\"\r")
                first.wait(lambda screen: "SIZE_52 132" in screen)
                first.send("\x1dc")
                second_id = first.session(excluding=original)
                assert second_id != original
                first.send("\x1ds")
                first.wait(lambda screen: "Workspaces /" in screen and str(original) in screen and str(second_id) in screen)
                # Down/up plus Ctrl-N/P must navigate the same tree rows.
                # Send an arrow across reads to exercise tty fragmentation.
                first.send("\x1b")
                time.sleep(0.03)
                first.send("[B\x1b[B\x1b[A\x0e\x0e\x10\x0e\r")
                first.wait(lambda screen: f"#{original} " in screen and "Workspaces /" not in screen and "connecting" not in screen and "loading screen" not in screen and "loading workspace" not in screen)
                first.send("printf 'PERSIST_%s\\n' \"$UBQ_TUI_SMOKE\"\r")
                first.wait(lambda screen: "PERSIST_kept" in screen)
                first.send("\x1dRWorkbench\r")
                first.wait(lambda screen: "Workbench" in screen)
                first.send("\x1d,Editor\r")
                first.wait(lambda screen: "1:Editor" in screen.splitlines()[-1])
                first.send("\x1d,")
                first.wait(lambda screen: "Rename tab:" in screen)
                os.write(first.master, "développement".encode("utf-8"))
                first.send("\x1b[D")
                first.send("!\r")
                first.wait(lambda screen: "développemen!t" in screen)
                first.send("\x1d,Editor\r")
                first.wait(lambda screen: "1:Editor" in screen.splitlines()[-1])
                first.resize(40, 8)
                first.send("\x1d?")
                first.wait(lambda screen: "Help /" in screen)
                first.send("\x1b[6~\x1b[6~\x1b[F")
                first.wait(lambda screen: "All commands use" in screen)
                first.send("\x1b")
                first.wait(lambda screen: "Help /" not in screen)
                first.resize(132, 53)
                first.send("\x1d|")
                split_id = first.session(excluding=original)
                assert split_id != second_id
                first.send("stty -echo; printf 'RIGHT_%s\\n' \"$(stty size)\"\r")
                first.wait(lambda screen: "RIGHT_51 66" in screen)
                first.send("\x1dz")
                first.wait(lambda screen: "PERSIST_kept" not in screen)
                first.send("printf 'ZOOM_%s\\n' \"$(stty size)\"\r")
                first.wait(lambda screen: "ZOOM_52 132" in screen)
                first.send("\x1dz")
                first.wait(lambda screen: "PERSIST_kept" in screen)
                # Hold each program status until its observation is acknowledged,
                # independently of shell startup cost and watch coalescing.
                release = Path(directory) / "release-status"
                release_block = Path(directory) / "release-block"
                first.send("printf '\\033]7501;id=job:state=working:app=build:progress=40\\007'; while [ ! -e " + shlex.quote(str(release)) + " ]; do sleep 0.02; done; printf '\\033]7501;id=job:state=blocked:kind=permission:msg=Q29udGludWU/\\007'; while [ ! -e " + shlex.quote(str(release_block)) + " ]; do sleep 0.02; done\r")
                first.wait(lambda screen: "◌" in screen.splitlines()[-1])
                first.send("\x1dn")
                first.wait(lambda screen: f"#{second_id} " in screen)
                release.touch()
                first.wait(lambda screen: "⚠" in screen.splitlines()[-1])
                assert "state=4" in first.metadata(), "background block was not forwarded to the outer window"
                assert "Workbench" in first.metadata(), "outer window title did not follow workspace"
                release_block.touch()
                # A second client shares the host's active tab and renames.
                observer = Terminal(client, dump, host, env, socket, ("--session", str(second_id)))
                terminals.append(observer)
                assert observer.session() == second_id
                observer.send("\x1d,Shared\r")
                first.wait(lambda screen: "2:Shared" in screen.splitlines()[-1])
                observer.finish()
                first.send("\x1dp")
                first.wait(lambda screen: f"#{split_id} " in screen)
                first.send("\x1dx")
                first.wait(lambda screen: "Close pane" in screen and "y/n" in screen)
                first.send("n")
                first.wait(lambda screen: "Close pane" not in screen and f"#{split_id} " in screen)
                first.send("\x1dx")
                first.wait(lambda screen: "Close pane" in screen)
                first.send("y")
                first.wait(lambda screen: f"#{original} " in screen)
                first.send("\x1dN")
                window_id = first.session(excluding=original)
                assert window_id not in (second_id, split_id)
                first.send("printf 'WINDOW_%s\\n' 'READY'\r")
                first.wait(lambda screen: "WINDOW_READY" in screen)
                first.send("\x1dW")
                first.wait(lambda screen: f"#{original} " in screen)
                first.finish()
                assert "state=0" in first.metadata() and "progress=0" in first.metadata(), "detach left outer window status behind"
                second = Terminal(client, dump, host, env, socket, ("--session", str(original), "--legacy"))
                terminals.append(second)
                assert second.session() == original
                second.send("printf 'REATTACH_%s\\n' \"$UBQ_TUI_SMOKE\"\r")
                second.wait(lambda screen: "REATTACH_kept" in screen)
                # Kill only this client's relay, not the daemon or its PTYs.
                # The driver's next channel must reattach the same session.
                processes = subprocess.check_output(["ps", "-axo", "pid=,ppid="], text=True)
                children = [int(pid) for pid, parent in (line.split() for line in processes.splitlines())
                            if int(parent) == second.process.pid]
                assert len(children) == 2, f"expected workspace and directory relays, found {children}"
                for child in children:
                    os.kill(child, signal.SIGTERM)
                # A short command is retried by this test only once the driver
                # has had time to replace the relay; application input itself
                # is never replayed by the frontend.
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    second.drain()
                    children_now = subprocess.check_output(["ps", "-axo", "pid=,ppid="], text=True)
                    if any(int(parent) == second.process.pid and int(pid) not in children
                           for pid, parent in (line.split() for line in children_now.splitlines())):
                        break
                else:
                    raise AssertionError("relay did not reconnect")
                second.wait(lambda screen: f"#{original} " in screen and "reconnecting" not in screen)
                time.sleep(0.2)
                second.send("printf 'RECONNECT_%s\\n' \"$UBQ_TUI_SMOKE\"\r")
                second.wait(lambda screen: "RECONNECT_kept" in screen)
                # Stop reading while the host repaints. The display queue must
                # coalesce screen states and eventually show the last one.
                second.send("i=0; while [ $i -lt 2000 ]; do printf '%s\\n' $i; i=$((i+1)); done; printf 'BURST_%s\\n' 'DONE'\r")
                time.sleep(0.5)
                second.wait(lambda screen: "BURST_DONE" in screen, timeout=30)
                second.finish(signal.SIGTERM)
                assert all(byte < 128 for byte in second.output), "non-seven-bit terminal output"
                missing = Terminal(client, dump, host, env, socket, ("--session", "4294967295"))
                terminals.append(missing)
                missing.wait_exit(1)
                ended = Terminal(client, dump, host, env, socket, ("--session", str(second_id)))
                terminals.append(ended)
                assert ended.session() == second_id
                ended.send("exit\r")
                ended.wait(lambda screen: f"#{original} " in screen)
                ended.finish()
                named = Terminal(client, dump, host, env, socket, ("--workspace", "Workbench"))
                terminals.append(named)
                assert named.session() == original
                named.send("\x1dC")
                fresh = named.session(excluding=original)
                assert fresh not in (second_id, split_id, window_id)
                named.send("\x1dRScratch\r")
                named.wait(lambda screen: "Scratch" in screen)
                named.send("\x1dw")
                named.wait(lambda screen: "Workbench" in screen and "Scratch" in screen)
                named.finish()
                print("Host smoke passed: workspaces, tabs, splits, program status, background status, shared edits, windows, persistence, resize, reconnect, backpressure and cleanup")
            finally:
                for terminal in terminals:
                    terminal.close()
                daemon.terminate()
                try:
                    daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait()


if __name__ == "__main__":
    main()
