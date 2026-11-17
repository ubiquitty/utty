#!/usr/bin/env python3
# Copyright (C) 2026 Stanislav Yaglo
# SPDX-License-Identifier: Apache-2.0
"""Exercise modern presentation and exact input through a real host and Sync."""
import argparse
import base64
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import time
from smoke import Terminal


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--client", default="build/utty")
    parser.add_argument("--dump", default="build/utty-capture")
    parser.add_argument("--host", default=".sdk/host/ubiquitty-host")
    args = parser.parse_args()
    client, dump, host = (str(Path(v).resolve()) for v in (args.client,args.dump,args.host))
    with tempfile.TemporaryDirectory(prefix="ubq-modern-") as directory:
        env = dict(os.environ,UBIQUITTY_HOME=directory,TERM="xterm-kitty",COLORTERM="truecolor")
        socket = "host-0123456789abcdef.sock"
        fixture = Path(directory)/"application.py"
        fixture.write_text('''import os, sys, tty
from pathlib import Path
record = Path(__file__).with_suffix(".input")
record.write_bytes(b"")
tty.setraw(0)
def emit(value): os.write(1,value)
emit(b"\\x1b[2J\\x1b[H\\x1b[?1003h\\x1b[?1006h\\x1b[?1004h\\x1b[?2004h\\x1b[6 q\\x1b[=31u\\x1b[38;2;12;34;56mMODERN_READY\\x1b[0m\\r\\n")
emit(b"\\x1b[3;1H\\x1b]8;;https://ubiquitty.com/\\x1b\\\\LINK_MARK\\x1b]8;;\\x1b\\\\\\r\\n\\x1b[4:3m\\x1b[58;2;90;120;150mUNDER_MARK\\x1b[0m")
while True:
    data = os.read(0,4096)
    with record.open("ab") as output: output.write(data)
    if data == b"image-red" or data == b"image-blue":
        import base64
        pixels = (b"\\xff\\x00\\x00" if data == b"image-red" else b"\\x00\\x00\\xff")*4
        emit(b"\\x1b[5;5H\\x1b_Ga=T,t=d,f=24,s=2,v=2,i=31,c=3,r=2,C=1,q=2;"+base64.b64encode(pixels)+b"\\x1b\\\\")
    elif data == b"image-delete": emit(b"\\x1b_Ga=d,d=I,i=31,q=2\\x1b\\\\")
    elif data == b"image-placeholder":
        import base64
        emit(b"\\x1b_Ga=T,U=1,q=2,t=d,f=24,s=2,v=2,c=2,r=2,i=9;"+base64.b64encode(b"\\xff\\x00\\x00"*4)+b"\\x1b\\\\")
        emit("\\x1b[7;20H\\x1b[38;5;9m\\U0010eeee\\u0305\\u0305\\x1b[0m".encode())
    elif data == b"placeholder-delete": emit(b"\\x1b_Ga=d,d=I,i=9,q=2\\x1b\\\\")
    elif data == b"pixel-mode": emit(b"\\x1b[?1016h\\x1b[13;1HPIXEL_READY")
    elif data == b"no-focus": emit(b"\\x1b[?1004l\\x1b[14;1HFOCUS_OFF")
    else: emit(b"\\x1b[12;1H\\x1b[2KINPUT_"+data.hex().encode()+b"\\r\\n")
''')
        with (Path(directory)/"host.log").open("wb") as log:
            daemon = subprocess.Popen([host,"--daemon","--socket",socket,"--quic-port","-1"],env=env,stdout=log,stderr=log,start_new_session=True)
            terminal = None
            try:
                deadline = time.monotonic()+10
                while not (Path(directory)/socket).exists():
                    assert daemon.poll() is None
                    assert time.monotonic()<deadline
                    time.sleep(.02)
                terminal = Terminal(client,dump,host,env,socket)
                original = terminal.session()
                terminal.send("\x1b_Gi=2147480001;OK\x1b\\\x1b[6;20;10t\x1b[?31u\x1b[?1;2c")
                terminal.send("stty -echo; python3 "+shlex.quote(str(fixture))+"\r")
                terminal.wait(lambda screen: "MODERN_READY" in screen)
                assert b"38;2;12;34;56" in terminal.output, bytes(terminal.output[-5000:])
                assert b"\x1b[6 q" in terminal.output and b"\x1b[=31u" in terminal.output
                terminal.wait(lambda screen: "LINK_MARK" in screen and "UNDER_MARK" in screen)
                assert b"\x1b]8;;https://ubiquitty.com/\x1b\\" in terminal.output
                assert b"\x1b[4:3m" in terminal.output
                assert re.search(rb"58[;:]2[;:]+90[;:]120[;:]150",terminal.output)

                def input_report(sequence, expected=None):
                    record = fixture.with_suffix(".input")
                    offset = record.stat().st_size
                    terminal.send(sequence)
                    expected_bytes = (expected or sequence).encode()
                    deadline = time.monotonic()+20
                    while record.stat().st_size < offset+len(expected_bytes):
                        terminal.drain()
                        assert terminal.process.poll() is None
                        assert time.monotonic()<deadline,record.read_bytes()[offset:]
                    assert record.read_bytes()[offset:] == expected_bytes

                input_report("\x1b[<0;5;6M")
                input_report("\x1b[<0;5;6m")
                input_report("\x1b[<32;7;8M")
                input_report("\x1b[<64;7;8M")
                input_report("\x1b[O")
                input_report("\x1b[I")
                input_report("\x1b[200~literal\x1dd\x1b[201~")
                input_report("\x1b[97;5:2u")
                terminal.send("pixel-mode")
                terminal.wait(lambda screen: "PIXEL_READY" in screen)
                terminal.send("\x1b[?1016;2$y")

                def wait_wire(predicate):
                    deadline = time.monotonic()+20
                    while not predicate(bytes(terminal.output)):
                        terminal.drain()
                        assert terminal.process.poll() is None
                        assert time.monotonic()<deadline, bytes(terminal.output[-2000:])

                wait_wire(lambda wire: b"\x1b[?1016h" in wire)
                input_report("\x1b[<0;47;103M")

                start = len(terminal.output)
                terminal.send("image-red")
                wait_wire(lambda wire: b"\x1b_Ga=p," in wire[start:])
                wire = bytes(terminal.output[start:])
                # Reassemble packet chunks and verify the actual rendered pixels.
                packets = re.findall(rb"\x1b_G([^;]*);([^\x1b]*)\x1b\\",wire)
                payload = b"".join(data for control,data in packets if b"m=" in control)
                controls = next(control for control,_ in packets if b"a=t" in control)
                assert b"f=32,s=30,v=40" in controls, controls
                assert base64.b64decode(payload) == b"\xff\0\0\xff"*(30*40)
                image_id = re.search(rb",i=(\d+)",controls)[1]
                fallback = Terminal(client,dump,host,env,socket,("--session",str(original)))
                try:
                    fallback.wait(lambda screen: "MODERN_READY" in screen)
                    assert b"\x1b_Ga=t," not in fallback.output
                    fallback.finish()
                finally: fallback.close()
                # Unchanged frames retain the upload; content changes replace it.
                start = len(terminal.output)
                input_report("same-image")
                assert b"\x1b_Ga=t," not in terminal.output[start:]
                terminal.send("image-blue")
                wait_wire(lambda wire: b"\x1b_Ga=t," in wire[start:])
                assert b"a=d,d=I,i="+image_id in terminal.output[start:]
                start = len(terminal.output)
                terminal.send("image-delete")
                wait_wire(lambda wire: b"\x1b_Ga=d,d=I," in wire[start:])
                start = len(terminal.output)
                terminal.send("image-placeholder")
                wait_wire(lambda wire: b"\x1b_Ga=p," in wire[start:])
                wire = bytes(terminal.output[start:])
                packets = re.findall(rb"\x1b_G([^;]*);([^\x1b]*)\x1b\\",wire)
                controls = next(control for control,_ in packets if b"a=t" in control)
                payload = b"".join(data for control,data in packets if b"m=" in control)
                assert b"f=32,s=10,v=10" in controls,controls
                assert base64.b64decode(payload) == b"\xff\0\0\xff"*100
                assert "\U0010eeee".encode() not in wire
                start = len(terminal.output)
                terminal.send("placeholder-delete")
                wait_wire(lambda wire: b"\x1b_Ga=d,d=I," in wire[start:])
                terminal.send("no-focus")
                terminal.wait(lambda screen: "FOCUS_OFF" in screen)
                terminal.send("\x1d|")
                terminal.session(excluding=original)
                input_report("\x1b[<0;47;163M","\x1b[<0;47;143M")
                terminal.wait(lambda screen: f"#{original} " in screen)
                terminal.send("\x1dc")
                terminal.session(excluding=original)
                terminal.send("\x1b[<0;171;461M")
                terminal.wait(lambda screen: "MODERN_READY" in screen and f"#{original} " in screen)
                # Kitty's report-all-keys form of Ctrl-] also reaches the prefix.
                terminal.send("\x1b[93;5u\x1b[100u")
                terminal.wait_exit(0)
                assert b"\x1b[?1003l" in terminal.output and b"\x1b[<u" in terminal.output
                print("Modern host smoke passed: RGB, cursor, mouse, focus, literal paste, Kitty keys, links, underlines, image pixels/update/cache/delete and terminal cleanup")
            finally:
                if terminal: terminal.close()
                daemon.terminate()
                try: daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    daemon.kill(); daemon.wait()


if __name__ == "__main__":
    main()
