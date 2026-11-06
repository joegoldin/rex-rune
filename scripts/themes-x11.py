#!/usr/bin/env python3
"""Verify persisted themes and explicit terminal colours on private servers.

Requires rex, Xvfb, openbox, xdotool and ImageMagick's import. Set REX_BIN.
No daily sessions, display or settings are used.
"""
import importlib.util
import json
import shlex
from collections import Counter
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REX_BIN", str(ROOT / "target/debug/rex-linux"))
SERVER = shutil.which("rex")
for tool in ("rex", "Xvfb", "openbox", "xdotool", "import"):
    assert shutil.which(tool), f"{tool} required"
work = Path(tempfile.mkdtemp(prefix="rex-themes-x11-"))
for name in ("home", "config", "state", "runtime", "cache", "captures"):
    (work / name).mkdir(mode=0o700)
for name in list(os.environ):
    if name.startswith(("REX_", "HERDR_")) or name in ("DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "WAYLAND_DISPLAY"):
        del os.environ[name]
os.environ.update(HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"),
                  XDG_STATE_HOME=str(work / "state"), XDG_RUNTIME_DIR=str(work / "runtime"),
                  XDG_CACHE_HOME=str(work / "cache"), REX_BIN=BINARY,
                  REX_CAPTURE_OUT=str(work / "captures"), REX_SERVER="unix://" + str(work / "rex-0.sock"))
spec = importlib.util.spec_from_file_location("accept", ROOT / "scripts/rune-acceptance-x11.py")
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


def api(method, server=0, **payload):
    return json.loads(subprocess.check_output([SERVER, "--autostart=false", "--server", "unix://" + str(work / f"rex-{server}.sock"),
        "api", "call", method, json.dumps(payload)], text=True, timeout=12))


def wait(predicate, description, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.08)
    raise AssertionError(description)


servers = []
desk = None
try:
    print("Private theme artifacts:", work, flush=True)
    for i in range(2):
        servers.append(subprocess.Popen([SERVER, "server", "run", "--listen", "unix://" + str(work / f"rex-{i}.sock"),
            "--restore=false", "--config", "/dev/null", "--state-dir", str(work / f"server-{i}")],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        wait(lambda: (work / f"rex-{i}.sock").exists(), "private server did not start")
    sessions = []
    for i, label in enumerate(("work", "remote-check")):
        sid = api("session.create", server=i, label=label, initial_windows=[
            {"window_label": label, "layout": accept.SHELL} for _ in range(3 - i)])["session_id"]
        sessions.append(sid)
    sid = sessions[0]
    view = api("session.view", session_id=sid)
    block = next(w["focused_block_id"] for w in view["windows"] if w["window_id"] == view["active_window_id"])
    config = work / "workspace.json"
    config.write_text(json.dumps({"hosts": [
        {"label": "Elphael-private", "endpoint": "unix://" + str(work / "rex-0.sock")},
        {"label": "Erdtree-private", "endpoint": "unix://" + str(work / "rex-1.sock")}],
        "theme": "gruvbox_dark", "sidebar_open": True, "vertical_tabs": True, "sidebar_width": 280}))
    desk = accept.Desktop()
    log = (work / "client.log").open("w")
    app, window = desk.launch(sid, config, log=log)

    def emit(sequence):
        program = "import sys;sys.stdout.write(" + repr(sequence) + ");sys.stdout.flush()"
        api("block.event", session_id=sid, block_id=block,
            event={"type":"paste","payload":{"text":"python3 -c " + shlex.quote(program)}})
        desk.key("Return")
        time.sleep(.5)

    def colours(name):
        desk.capture(window, name)
        data = subprocess.check_output(["convert", str(work / "captures" / (name + ".png")), "-depth", "8", "rgb:-"])
        return Counter(zip(data[0::3], data[1::3], data[2::3]))

    emit("\x1b[2J\x1b[HDefault text\r\n\x1b[41m" + " "*40 + "\x1b[0m\r\n\x1b[48;2;1;2;3m" + " "*40 + "\x1b[0m\r\n")
    pixels = colours("gruvbox-dark")
    assert pixels[(40,40,40)] > 100000, "Gruvbox chrome/default background missing"
    assert pixels[(204,36,29)] > 2500, "Gruvbox ANSI red missing"
    assert pixels[(1,2,3)] > 2500, "explicit RGB background was changed"
    app.terminate(); app.wait(timeout=10)
    app, window = desk.launch(sid, config, log=log)
    pixels = colours("gruvbox-reopened")
    assert pixels[(40,40,40)] > 100000 and pixels[(204,36,29)] > 2500
    app.terminate(); app.wait(timeout=10)
    saved = json.loads(config.read_text()); saved["theme"] = "midnight"
    config.write_text(json.dumps(saved))
    app, window = desk.launch(sid, config, log=log)
    pixels = colours("midnight")
    assert pixels[(0,21,40)] > 100000, "Midnight was not restored"
    assert pixels[(1,2,3)] > 2500, "theme switch changed explicit RGB"
    emit("\x1b]4;1;#112233\x07\x1b]11;#123456\x07\x1b[2J\x1b[H\x1b[41m" + " "*40 + "\x1b[0m\r\n")
    pixels = colours("application-osc-colours")
    assert pixels[(18,52,86)] > 100000, "application OSC background was changed"
    assert pixels[(17,34,51)] > 2500, "application OSC palette was changed"
    print("PASS persisted Gruvbox/Midnight, ANSI palette, explicit RGB and application OSC colours", flush=True)
finally:
    if desk:
        desk.close()
    for server in servers:
        server.terminate(); server.wait(timeout=8)
