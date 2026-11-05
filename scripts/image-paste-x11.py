#!/usr/bin/env python3
"""Private-server image paste checks; needs rex, Xvfb, xdotool, xclip and ImageMagick."""
import importlib.util,json,os,shlex,struct,subprocess,tempfile,time,zlib
from pathlib import Path
BASE=Path(__file__).resolve().parent
BINARY=os.environ.get("REX_BIN",str(BASE.parent/"target/debug/rex-linux"))
PI_BINARY=os.environ.get("REX_PI_BIN")
work=Path(tempfile.mkdtemp(prefix="rex-image-paste-x11-"));print("Private image paste artifacts:",work,flush=True)
for key in list(os.environ):
    if key.startswith(("REX_","HERDR_")) or key in ("DBUS_SESSION_BUS_ADDRESS","DISPLAY","WAYLAND_DISPLAY"):del os.environ[key]
for key,folder in [("HOME","home"),("XDG_CONFIG_HOME","config"),("XDG_DATA_HOME","data"),("XDG_STATE_HOME","state"),("XDG_CACHE_HOME","cache"),("XDG_RUNTIME_DIR","runtime"),("TMPDIR","image temporary files")]:
    path=work/folder;path.mkdir(mode=0o700);os.environ[key]=str(path)
endpoint="unix://"+str(work/"server.sock")
os.environ.update(REX_SERVER=endpoint,REX_BIN=BINARY,REX_CAPTURE_OUT=str(work/"captures"))
env=os.environ.copy()
spec=importlib.util.spec_from_file_location("accept",BASE/"rune-acceptance-x11.py")
accept=importlib.util.module_from_spec(spec);spec.loader.exec_module(accept)
server_log=(work/"server.log").open("wb")
server=subprocess.Popen(["rex","server","run","--listen",endpoint,"--restore=false","--config","/dev/null","--state-dir",str(work/"server-state")],env=env,cwd=work,stdout=server_log,stderr=subprocess.STDOUT)
def wait(predicate,message,timeout=8):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        if predicate():return
        time.sleep(.04)
    raise AssertionError(message)
def api(method,**params):
    return json.loads(subprocess.check_output(["rex","--autostart=false","--server",endpoint,"api","call",method,json.dumps(params)],env=env,text=True,timeout=10))
def chunk(kind,data):return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data)&0xffffffff)
png=b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",2,1,8,2,0,0,0))+chunk(b"IDAT",zlib.compress(b"\0"+bytes([17,187,102])*2))+chunk(b"IEND",b"")
desk=None
ssh_fixture=None
try:
    wait(lambda:(work/"server.sock").exists(),"private server did not start")
    sid=api("session.create",label="image-paste",initial_windows=[{"window_label":"image-paste","layout":accept.SHELL},{"window_label":"other","layout":accept.SHELL}])["session_id"]
    state=api("session.view",session_id=sid)
    block=next(w["focused_block_id"] for w in state["windows"] if w["window_id"]==state["active_window_id"])
    config=work/"workspace.json";config.write_text(json.dumps({"hosts":[{"label":"private","endpoint":endpoint}]}))
    desk=accept.Desktop()
    client_log=(work/"client.log").open("w")
    app,window=desk.launch(sid,config,log=client_log)
    ready=work/"ready";received=work/"received.txt"
    program="""import os,sys,termios,tty,select,time
from pathlib import Path
old=termios.tcgetattr(0);tty.setraw(0);e=chr(27)
try:
 sys.stdout.write(e+'[?2004h');sys.stdout.flush();Path(READY).touch()
 data=b'';deadline=time.monotonic()+12
 while time.monotonic()<deadline:
  if select.select([0],[],[],.1)[0]:
   data+=os.read(0,4096)
   if b'\\x1b[201~' in data:
    Path(RECEIVED).write_bytes(data.split(b'\\x1b[200~',1)[1].split(b'\\x1b[201~',1)[0]);break
finally:
 termios.tcsetattr(0,termios.TCSANOW,old);sys.stdout.write(e+'[?2004l');sys.stdout.flush()
""".replace("READY",repr(str(ready))).replace("RECEIVED",repr(str(received)))
    api("block.event",session_id=sid,block_id=block,event={"type":"paste","payload":{"text":"python3 -c "+shlex.quote(program)}})
    desk.key("Return");wait(ready.exists,"image receiver did not start");time.sleep(.25)
    clip=subprocess.Popen(["xclip","-selection","clipboard","-target","image/png","-in"],env=desk.env,stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    clip.stdin.write(png);clip.stdin.close();clip.wait(timeout=3);time.sleep(.1)
    desk.key("ctrl+shift+v")
    wait(received.exists,"Clipboard image was not pasted as a PNG filepath",timeout=4)
    values=shlex.split(received.read_text());assert len(values)==1,values
    path=Path(values[0]);assert path.is_relative_to(work/"image temporary files"),path
    assert path.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"),path
    pixels=subprocess.check_output(["convert",str(path),"-depth","8","rgba:-"])
    assert pixels==bytes([17,187,102,255])*2,pixels
    assert path.stat().st_mode&0o777==0o600
    assert path.parent.stat().st_mode&0o777==0o700
    desk.capture(window,"local-png-paste")
    print("PASS clipboard image becomes a private, correctly quoted, byte-complete local PNG",flush=True)

    def receiver(name,target_block=block):
        next_ready=work/(name+"-ready");next_received=work/(name+"-received.txt")
        code=program.replace(repr(str(ready)),repr(str(next_ready))).replace(repr(str(received)),repr(str(next_received)))
        time.sleep(.15)
        api("block.event",session_id=sid,block_id=target_block,event={"type":"paste","payload":{"text":"python3 -c "+shlex.quote(code)}})
        desk.key("Return");wait(next_ready.exists,"paste receiver did not start: "+name);time.sleep(.15)
        return next_received
    def put_clipboard(data,image=False):
        command=["xclip","-selection","clipboard","-in"]
        if image:command.extend(["-target","image/png"])
        proc=subprocess.Popen(command,env=desk.env,stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        proc.stdin.write(data);proc.stdin.close();proc.wait(timeout=3);time.sleep(.1)
    text_received=receiver("plain-text")
    text="Unicode paste 世界 e\u0301\nsecond line".replace("\\u0301","\u0301")
    put_clipboard(text.encode());desk.key("ctrl+v")
    wait(text_received.exists,"Ctrl V did not paste plain text")
    assert text_received.read_text()==text,repr(text_received.read_text())
    print("PASS Ctrl V pastes Unicode text without changing it",flush=True)

    shifted_text=receiver("shifted-text")
    put_clipboard(text.encode());desk.key("ctrl+shift+v")
    wait(shifted_text.exists,"Ctrl Shift V did not paste client text")
    assert shifted_text.read_text()==text
    local_ctrl_v=receiver("local-ctrl-v-image")
    put_clipboard(png,True);desk.key("ctrl+v")
    wait(local_ctrl_v.exists,"Ctrl V did not paste a local clipboard image")
    local_values=shlex.split(local_ctrl_v.read_text());assert len(local_values)==1
    local_path=Path(local_values[0]);assert local_path.is_relative_to(work/"image temporary files")
    assert subprocess.check_output(["convert",str(local_path),"-depth","8","rgba:-"])==bytes([17,187,102,255])*2
    print("PASS both paste chords use the client clipboard for local text and images",flush=True)

    if PI_BINARY:
        app.terminate();app.wait(timeout=5)
        pi_sid=api("session.create",label="actual-pi-clipboard",initial_windows=[{"window_label":"Pi","layout":accept.SHELL}])["session_id"]
        pi_view=api("session.view",session_id=pi_sid)
        pi_block=next(w["focused_block_id"] for w in pi_view["windows"] if w["window_id"]==pi_view["active_window_id"])
        pi_config=work/"pi-workspace.json";pi_config.write_text(json.dumps({"hosts":[{"label":"private-pi","endpoint":endpoint}]}))
        app,window=desk.launch(pi_sid,pi_config,log=(work/"pi-client.log").open("w"))
        agent=work/"pi-agent";agent.mkdir(mode=0o700)
        command=["env","-i","HOME="+env["HOME"],"PATH="+env["PATH"],"TERM=xterm-256color","LANG=C.UTF-8","PI_CODING_AGENT_DIR="+str(agent),
            PI_BINARY,"--offline","--no-extensions","--no-mcp","--no-skills","--no-prompt-templates","--no-themes","--no-context-files","--no-tools","--no-session"]
        api("block.event",session_id=pi_sid,block_id=pi_block,event={"type":"paste","payload":{"text":shlex.join(command)}})
        desk.key("Return");time.sleep(2)
        def pi_screen():
            return api("com.superlogical.terminal.format",session_id=pi_sid,block_id=pi_block,args={"format":"text","trim":True,"unwrap":True})["content"]
        for name,chord in [("ctrl-v","ctrl+v"),("ctrl-shift-v","ctrl+shift+v")]:
            before=set((work/"image temporary files").glob("rex-paste-*/clipboard.png"))
            put_clipboard(png,True);desk.key(chord)
            wait(lambda:bool(set((work/"image temporary files").glob("rex-paste-*/clipboard.png"))-before),"Pi paste did not create a client PNG")
            added=set((work/"image temporary files").glob("rex-paste-*/clipboard.png"))-before
            assert len(added)==1
            image_path=added.pop()
            wait(lambda:image_path.parent.name in pi_screen() and "clipboard.png" in pi_screen(),"Pi editor did not receive the clipboard PNG filepath")
            assert subprocess.check_output(["convert",str(image_path),"-depth","8","rgba:-"])==bytes([17,187,102,255])*2
            desk.capture(window,"pi-image-"+name)
            desk.key("ctrl+u");time.sleep(.15)
        app.terminate();app.wait(timeout=5);api("session.destroy",session_id=pi_sid)
        print("PASS actual offline Pi receives local clipboard images via both chords with no server DISPLAY or credentials",flush=True)
    else:
        print("SKIP actual Pi check: set REX_PI_BIN to an unconfigured Pi executable",flush=True)

    fixture_spec=importlib.util.spec_from_file_location("ssh_fixture",BASE/"image-paste-ssh-fixture.py")
    fixture_module=importlib.util.module_from_spec(fixture_spec);fixture_spec.loader.exec_module(fixture_module)
    ssh_fixture=fixture_module.PrivateSSH(work)
    app.terminate();app.wait(timeout=5)
    saved=json.loads(config.read_text());saved["hosts"][0]["image_ssh_target"]=ssh_fixture.target
    config.write_text(json.dumps(saved))
    app,window=desk.launch(sid,config,extra_env={"PATH":str(ssh_fixture.bin)+os.pathsep+desk.env["PATH"]},log=(work/"ssh-client.log").open("w"))
    remote_received=receiver("ssh-image")
    ssh_fixture.write_wrapper();put_clipboard(png,True);desk.key("ctrl+v")
    wait(remote_received.exists,"SSH image filepath did not reach the original pane")
    remote_values=shlex.split(remote_received.read_text());assert len(remote_values)==1,remote_values
    remote_path=Path(remote_values[0]);assert remote_path.name=="clipboard.png" and remote_path.parent.name.startswith("rex-paste."),remote_path
    assert subprocess.check_output(["convert",str(remote_path),"-depth","8","rgba:-"])==bytes([17,187,102,255])*2
    assert remote_path.stat().st_mode&0o777==0o600 and remote_path.parent.stat().st_mode&0o777==0o700
    assert ssh_fixture.status.read_text().strip()=="0"
    desk.capture(window,"ssh-image-paste")
    remote_path.unlink();remote_path.parent.rmdir()
    print("PASS configured SSH upload with real key/host verification, complete PNG pixels and private permissions",flush=True)

    known=ssh_fixture.known.read_text();ssh_fixture.known.write_text("")
    failed_received=receiver("unknown-host-key")
    ssh_fixture.write_wrapper();put_clipboard(png,True);desk.key("ctrl+shift+v")
    wait(ssh_fixture.status.exists,"rejected SSH transfer did not finish")
    assert ssh_fixture.status.read_text().strip()!="0"
    time.sleep(.3);assert not failed_received.exists(),"SSH failure pasted an inaccessible local fallback path"
    desk.capture(window,"rejected-unknown-host-key")
    put_clipboard(b"safe text after failed image upload");desk.key("ctrl+v")
    wait(failed_received.exists,"failed SSH upload left clipboard paste stuck")
    assert failed_received.read_text()=="safe text after failed image upload"
    ssh_fixture.known.write_text(known)
    print("PASS unknown SSH host key fails closed with no local-path fallback; later text paste still works",flush=True)

    other_block=next(w["focused_block_id"] for w in state["windows"] if w["window_id"]!=state["active_window_id"])
    desk.key("ctrl+Tab");time.sleep(.2)
    other_received=receiver("other-pane",other_block)
    desk.key("ctrl+shift+Tab");time.sleep(.2)
    original_received=receiver("original-pane")
    ssh_fixture.write_wrapper(.8);put_clipboard(png,True);desk.key("ctrl+v");desk.key("ctrl+Tab")
    wait(ssh_fixture.status.exists,"delayed SSH upload did not finish")
    assert ssh_fixture.status.read_text().strip()=="0"
    time.sleep(.3)
    assert not original_received.exists() and not other_received.exists(),"an asynchronous image paste reached a stale or different pane"
    put_clipboard(b"other pane probe");desk.key("ctrl+v");wait(other_received.exists,"new pane was not usable")
    assert other_received.read_text()=="other pane probe"
    desk.key("ctrl+shift+Tab");time.sleep(.2)
    put_clipboard(b"original pane probe");desk.key("ctrl+v");wait(original_received.exists,"original pane was not usable")
    assert original_received.read_text()=="original pane probe"
    print("PASS switching tabs during SSH upload delivers to neither the old nor the new pane",flush=True)
finally:
    if desk:desk.close()
    if ssh_fixture:ssh_fixture.close()
    server.terminate()
    try:server.wait(timeout=8)
    except subprocess.TimeoutExpired:server.kill();server.wait()
    server_log.close()
