#!/usr/bin/env python3
"""Real X11 event/clipboard regression, isolated from the user's desktop.

Requires Xvfb, xdotool, rex and a built native binary. Creates/destroys only its
own local Rex session. This does not validate Wayland or a physical IME.
"""
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def api(method, **payload):
    server = ["--server", os.environ["REX_SERVER"]] if "REX_SERVER" in os.environ else []
    return json.loads(subprocess.check_output(
        ["rex", *server, "api", "call", method, json.dumps(payload)], text=True))


def wait_for(predicate, message, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def run():
    label = f"rex-x11-smoke-{os.getpid()}"
    sid = api("session.create", label=label, initial_windows=[{
        "layout": {"block": {"flavor": "com.superlogical.terminal.shell"}}
    }])["session_id"]
    server = app = None
    try:
        with tempfile.TemporaryDirectory(prefix="rex-x11-") as directory:
            read_fd, write_fd = os.pipe()
            server = subprocess.Popen([
                "Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1440x900x24",
                "-nolisten", "tcp"], pass_fds=(write_fd,))
            os.close(write_fd)
            with os.fdopen(read_fd) as display:
                number = display.readline().strip()
            assert number, "Xvfb failed to start"
            env = dict(os.environ, DISPLAY=f":{number}")
            env.pop("WAYLAND_DISPLAY", None)
            config = Path(directory) / "workspace.json"
            app = subprocess.Popen([
                os.environ.get("REX_BIN", str(ROOT / "target/debug/rex-linux")), "--session", sid,
                "--config", str(config), "--window-size", "1200x800"], env=env)

            def x(*args):
                return subprocess.check_output(["xdotool", *args], env=env, text=True, timeout=15).strip()

            window = x("search", "--sync", "--pid", str(app.pid))
            x("windowfocus", window)
            time.sleep(1)

            def key(stroke):
                x("key", stroke)
                time.sleep(0.15)

            def text(value):
                x("type", "--clearmodifiers", "--delay", "8", value)
                time.sleep(0.15)

            def command(name):
                key("ctrl+shift+p")
                text(name)
                key("Return")

            text("echo X11_NATIVE_INPUT_OK")
            key("Return")
            # Use a native field as a real X11 clipboard source, then paste into
            # the terminal. The rename is cancelled, so the session stays named.
            command("Rename session")
            key("ctrl+a")
            text("echo X11_CLIPBOARD_OK")
            key("ctrl+a")
            key("ctrl+c")
            key("Escape")
            key("ctrl+shift+v")
            key("Return")
            block = api("session.view", session_id=sid)["windows"][0]["focused_block_id"]
            before_clear = api("com.superlogical.terminal.format", session_id=sid,
                               block_id=block, args={"format": "text", "trim": True, "unwrap": True})["content"]
            assert "X11_NATIVE_INPUT_OK" in before_clear.splitlines(), before_clear
            assert "X11_CLIPBOARD_OK" in before_clear.splitlines(), before_clear
            key("ctrl+comma")  # Settings, focused on its section list
            original_size = json.loads(config.read_text())["font_size"]
            key("Down")
            key("Return")  # Terminal
            key("Tab")
            key("Tab")
            key("Tab")
            key("Tab")  # Close, change family, font smaller, font larger
            key("Return")
            assert json.loads(config.read_text())["font_size"] == original_size + 1
            key("shift+Tab")
            key("Return")
            assert json.loads(config.read_text())["font_size"] == original_size
            key("Escape")
            command("Close pane")
            key("Return")  # Visible default is Keep running.
            assert api("session.view", session_id=sid)["windows"], "Default confirmation closed the terminal"
            text("printf '\\033[2J\\033[HPOINTER_COPY_OK\\n'")
            key("Return")
            time.sleep(0.3)
            # A single pane is full-bleed: 52pt bar, grid 12pt in and 8pt down, 9px cells.
            x("mousemove", "--window", window, "16", "68")
            x("mousedown", "1")
            x("mousemove", "--window", window, "142", "68")
            x("mouseup", "1")
            key("ctrl+shift+c")
            text("echo ")
            key("ctrl+shift+v")
            key("Return")
            command("Add host")
            text("Unavailable test host")
            key("Tab")
            text(f"unix://{directory}/missing.sock")
            key("Return")
            wait_for(lambda: any(h["label"] == "Unavailable test host"
                                for h in json.loads(config.read_text())["hosts"]),
                     "Enter/Tab form submission failed")
            key("ctrl+shift+b")
            time.sleep(0.6)  # the sidebar slides in
            x("mousemove", "--window", window, "29", "152")
            x("click", "1")
            time.sleep(0.15)
            assert "Unavailable test host" in x("getwindowname", window), "Host disclosure changed the active host"
            x("click", "1")
            key("ctrl+k")
            text(label)
            key("Return")
            time.sleep(0.5)
            view = api("session.view", session_id=sid)
            block = view["windows"][0]["focused_block_id"]
            output = api("com.superlogical.terminal.format", session_id=sid,
                         block_id=block, args={"format": "text", "trim": True, "unwrap": True})["content"]
            lines = output.splitlines()
            assert lines.count("POINTER_COPY_OK") == 2, "Pointer selection/copy failed:\n" + output
            assert view["label"] == label, "Cancelled rename changed the session"
            # A raw PTY observer enables Kitty event reporting, so leaked key
            # releases are observable too, not merely invisible to a shell.
            observer = Path(directory) / "observe.py"
            result = Path(directory) / "keys.bin"
            ready = Path(directory) / "ready"
            observer.write_text("""import os, select, sys, termios, time, tty
from pathlib import Path
old = termios.tcgetattr(0)
data = bytearray()
try:
    tty.setraw(0)
    time.sleep(.25)
    termios.tcflush(0, termios.TCIFLUSH)
    os.write(1, b'\\x1b[>3u')
    Path(sys.argv[1]).touch()
    deadline = time.monotonic() + 4
    while time.monotonic() < deadline:
        if select.select([0], [], [], .05)[0]:
            data.extend(os.read(0, 4096))
finally:
    os.write(1, b'\\x1b[<u')
    termios.tcsetattr(0, termios.TCSADRAIN, old)
    Path(sys.argv[2]).write_bytes(data)
""")
            text(f"python3 {shlex.quote(str(observer))} {shlex.quote(str(ready))} {shlex.quote(str(result))}")
            key("Return")
            wait_for(ready.exists, "Raw key observer did not start")
            command("Rename session")
            key("Escape")
            key("ctrl+shift+space")
            key("Escape")
            wait_for(result.exists, "Raw key observer did not finish", timeout=6)
            assert result.read_bytes() == b"", f"Local key presses/releases leaked: {result.read_bytes()!r}"
            print("PASS X11 native Enter/Tab, terminal input, clipboard copy/paste, pointer selection/copy, dialog keyboard/default action, host disclosure, add-host form and session switcher")
            print("PASS local menu/form key presses and releases isolated from raw Kitty-reporting PTY")
    finally:
        for process in (app, server):
            if process:
                process.terminate()
                process.wait(timeout=10)
        api("session.destroy", session_id=sid)


if __name__ == "__main__":
    run()
