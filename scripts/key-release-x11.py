#!/usr/bin/env python3
"""Check native Kitty key releases on a private Rex server and X11 display.

Requires rex, Xvfb, openbox and xdotool. Set REX_BIN to the built client.
No daily sessions, display, clipboard or configuration are used.
"""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time

BASE = Path(__file__).resolve().parent
binary = os.environ.get("REX_BIN", str(BASE.parent / "target/debug/rex-linux"))
work = Path(tempfile.mkdtemp(prefix="rex-key-release-x11-"))
print("Private key-release artifacts:", work, flush=True)
for key in list(os.environ):
    if key.startswith(("REX_", "HERDR_")) or key in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
        del os.environ[key]
for key, folder in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"),
                    ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")]:
    path = work / folder
    path.mkdir(mode=0o700)
    os.environ[key] = str(path)
endpoint = "unix://" + str(work / "rex.sock")
os.environ.update(REX_SERVER=endpoint, REX_BIN=binary, REX_CAPTURE_OUT=str(work / "captures"))
spec = importlib.util.spec_from_file_location("accept", BASE / "rune-acceptance-x11.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


def api(method, **params):
    try:
        return json.loads(subprocess.check_output(
            ["rex", "--autostart=false", "--server", endpoint, "api", "call", method, json.dumps(params)],
            text=True, timeout=10))
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        raise AssertionError(f"private server call {method} failed") from error


def wait(predicate, message):
    end = time.monotonic() + 8
    while time.monotonic() < end:
        if predicate():
            return
        time.sleep(.04)
    raise AssertionError(message)


server = subprocess.Popen(["rex", "server", "run", "--listen", endpoint, "--restore=false",
                           "--config", "/dev/null", "--state-dir", str(work / "server")],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
desk = None
try:
    wait(lambda: (work / "rex.sock").exists(), "private server did not start")
    sid = api("session.create", label="key-release", initial_windows=[
        {"window_label": "keys", "layout": accept.SHELL}])["session_id"]
    state = api("session.view", session_id=sid)
    block = state["windows"][0]["focused_block_id"]
    config = work / "workspace.json"
    config.write_text(json.dumps({"hosts": [{"label": "private", "endpoint": endpoint}]}))
    desk = accept.Desktop()
    app, window = desk.launch(sid, config, log=(work / "client.log").open("w"))
    ready, received = work / "ready", work / "bytes"
    program = r'''import os,sys,tty,termios,select,time
from pathlib import Path
old=termios.tcgetattr(0);tty.setraw(0)
try:
 sys.stdout.write(chr(27)+'[>11u');sys.stdout.flush()
 Path(READY).touch();data=b'';deadline=time.monotonic()+5
 while time.monotonic()<deadline:
  if select.select([0],[],[],.1)[0]:
   data+=os.read(0,4096);Path(RECEIVED).write_bytes(data)
finally:
 sys.stdout.write(chr(27)+'[<u');sys.stdout.flush()
 termios.tcsetattr(0,termios.TCSANOW,old)
'''.replace("READY", repr(str(ready))).replace("RECEIVED", repr(str(received)))
    api("block.event", session_id=sid, block_id=block,
        event={"type": "paste", "payload": {"text": "python3 -c " + shlex.quote(program)}})
    desk.key("Return")
    wait(ready.exists, "raw key receiver did not start")
    time.sleep(.2)
    desk.key("a")
    wait(lambda: received.exists() and b"\x1b[97;1:3u" in received.read_bytes(),
         "Kitty release did not reach the terminal: " + repr(received.read_bytes() if received.exists() else b""))
    print("PASS native Kitty key release reaches the terminal with no focused control", flush=True)
finally:
    if desk:
        desk.close()
    server.terminate()
    try:
        server.wait(timeout=8)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()
