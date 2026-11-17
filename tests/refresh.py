#!/usr/bin/env python3
# Copyright (C) 2026 Stanislav Yaglo
# SPDX-License-Identifier: Apache-2.0
"""Compare complete Neovim repaints directly and through an isolated TUI host."""
import argparse
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time


class Session:
    def __init__(self, command, env, columns, rows):
        self.master, slave = pty.openpty()
        self.columns, self.rows = columns, rows
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        self.process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                                        env=env, start_new_session=True)
        os.close(slave)

    def send(self, data):
        os.write(self.master, data)

    def until(self, marker, timeout=20):
        deadline = time.monotonic() + timeout
        tail = b""
        received = 0
        while marker not in tail:
            remaining = deadline - time.monotonic()
            assert remaining > 0, f"missing {marker!r}: {tail[-1000:]!r}"
            if select.select([self.master], [], [], remaining)[0]:
                chunk = os.read(self.master, 65536)
                received += len(chunk)
                tail = tail[-max(len(marker), 1000):] + chunk
            else:
                assert self.process.poll() is None, tail[-2000:]
        return received

    def close(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        os.close(self.master)

    def ready(self, dump):
        output = bytearray()
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert self.process.poll() is None, bytes(output[-1000:])
            if select.select([self.master], [], [], 0.05)[0]:
                output.extend(os.read(self.master, 65536))
                screen = subprocess.check_output(
                    [dump, "--dump-utf8", str(self.columns), str(self.rows)], input=output).decode()
                if re.search(r"#[1-9]\d*\b", screen) and "connecting" not in screen:
                    return
        raise AssertionError("client did not attach")


def measure(session, samples):
    session.until(b"FRAME_END_000000")
    # Drain the final status/cursor bytes before starting the first sample.
    while select.select([session.master], [], [], 0.05)[0]:
        os.read(session.master, 65536)
    durations, sizes = [], []
    for sequence in range(1, samples + 1):
        start = time.monotonic()
        session.send(f":lua BenchFlip({sequence})\r".encode())
        sizes.append(session.until(f"FRAME_END_{sequence:06d}".encode()))
        durations.append((time.monotonic() - start) * 1000)
    return {"median_ms": round(statistics.median(durations), 2),
            "max_ms": round(max(durations), 2), "median_bytes": int(statistics.median(sizes)),
            "samples_ms": [round(value, 2) for value in durations]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client", default="build/utty")
    parser.add_argument("--host", default=".sdk/host/ubiquitty-host")
    parser.add_argument("--dump", default="build/utty-capture")
    parser.add_argument("--nvim", default=shutil.which("nvim"))
    parser.add_argument("--mc", default=shutil.which("mc"))
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--output-mib", type=int, default=4)
    parser.add_argument("--profile", help="write a macOS sample of the TUI during the output workload")
    parser.add_argument("--columns", type=int, default=160)
    parser.add_argument("--rows", type=int, default=48)
    args = parser.parse_args()
    assert args.nvim, "install nvim to run this comparison"
    assert args.samples > 0 and 20 <= args.columns <= 512 and 4 <= args.rows <= 256
    client, host, dump = (str(Path(value).resolve()) for value in (args.client, args.host, args.dump))
    with tempfile.TemporaryDirectory(prefix="ubq-refresh-", dir="/tmp") as directory:
        root = Path(directory)
        profile = root / "mc-profile"
        profile.mkdir()
        env = dict(os.environ, UBIQUITTY_HOME=directory, TERM="xterm-256color",
                   MC_PROFILE_ROOT=str(profile))
        init = root / "init.lua"
        init.write_text("""
vim.o.termguicolors = true
vim.o.number = false
vim.o.ruler = false
vim.o.showmode = false
vim.o.laststatus = 0
vim.o.cmdheight = 1
vim.o.wrap = false
vim.o.swapfile = false
vim.cmd('syntax on')
vim.bo.filetype = 'lua'
function BenchFlip(sequence)
  vim.api.nvim_set_hl(0, 'Normal', {fg='#c0c0c0', bg=sequence % 2 == 0 and '#101820' or '#282010'})
  local lines = {}
  local width = vim.o.columns
  for row=1,vim.o.lines-1 do
    local text = 'local x = "hello"; print(x + 42) -- '
    text = text:rep(math.ceil(width / #text)):sub(1, width-19)
    local tag = row == vim.o.lines-1 and 'FRAME_END_' or 'FRAME_ROW_'
    lines[row] = text .. string.format(' ' .. tag .. '%06d', sequence)
  end
  vim.api.nvim_buf_set_lines(0, 0, -1, false, lines)
  vim.api.nvim_win_set_cursor(0, {1,0})
  vim.cmd('redraw!')
end
vim.schedule(function() BenchFlip(0) end)
""")
        command = [args.nvim, "-u", str(init), "-i", "NONE", "--noplugin", "-n"]
        burst = root / "burst.py"
        burst.write_text(f"""
import sys
width = {args.columns}
rows = ({args.output_mib} * 1024 * 1024 + width - 1) // width
for row in range(rows):
    text = (str(row).zfill(8) + ' abcdefghijklmnopqrstuvwxyz' * width)[:width-1] + '\\n'
    sys.stdout.buffer.write(text.encode())
sys.stdout.buffer.write(b'\\x1b[2J\\x1b[HTHROUGHPUT_DONE\\n')
sys.stdout.buffer.flush()
""")
        # Match Neovim's viewport: the TUI reserves one physical row for chrome.
        direct = Session(command, env, args.columns, args.rows - 1)
        try:
            print("direct nvim:", measure(direct, args.samples), flush=True)
        finally:
            direct.close()
        direct = Session([sys.executable, str(burst)], env, args.columns, args.rows - 1)
        try:
            start = time.monotonic()
            direct.until(b"THROUGHPUT_DONE", timeout=60)
            print("direct output MiB/s:", round(args.output_mib / (time.monotonic() - start), 2), flush=True)
        finally:
            direct.close()
        socket = "host-0123456789abcdef.sock"
        with (root / "host.log").open("wb") as log:
            daemon = subprocess.Popen([host, "--daemon", "--socket", socket, "--quic-port", "-1"],
                                      env=env, stdout=log, stderr=log, start_new_session=True)
            tui = None
            try:
                deadline = time.monotonic() + 10
                while not (root / socket).exists():
                    assert daemon.poll() is None, (root / "host.log").read_text()
                    assert time.monotonic() < deadline, "host did not start"
                    time.sleep(0.01)
                tui = Session([client, "--host-binary", host, "--socket", socket],
                              env, args.columns, args.rows)
                # Wait for the shell before launching the application.
                tui.ready(dump)
                tui.send((shlex.join(command) + "\r").encode())
                print("nvim through TUI:", measure(tui, args.samples), flush=True)
                if args.mc:
                    tui.close()
                    tui = Session([client, "--host-binary", host, "--socket", socket],
                                  env, args.columns, args.rows)
                    tui.ready(dump)
                    (root / "mc-item-a").write_text("first item\n")
                    tui.send((shlex.join([args.mc, "--nosubshell", str(root), str(root)]) + "\r").encode())
                    tui.until(b"mc-item-a")
                    (root / "mc-item-b").write_text("second item\n")
                    tui.send(b"\x12")
                    tui.until(b"mc-item-b")
                    print("mc through TUI: panel refresh passed", flush=True)
                tui.close()
                tui = Session([client, "--host-binary", host, "--socket", socket],
                              env, args.columns, args.rows)
                tui.ready(dump)
                start = time.monotonic()
                tui.send((shlex.join([sys.executable, str(burst)]) + "\r").encode())
                profiler = subprocess.Popen(["sample", str(tui.process.pid), "1", "1", "-file", args.profile],
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) if args.profile else None
                tui.until(b"THROUGHPUT_DONE", timeout=60)
                print("output through TUI MiB/s:", round(args.output_mib / (time.monotonic() - start), 2), flush=True)
                if profiler:
                    profiler.wait(timeout=10)
            finally:
                if tui:
                    tui.close()
                daemon.terminate()
                try:
                    daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait()


if __name__ == "__main__":
    main()
