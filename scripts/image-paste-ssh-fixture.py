#!/usr/bin/env python3
"""An ephemeral loopback sshd, keys and known_hosts; never edits the user's SSH settings."""
import os,pwd,shlex,shutil,socket,subprocess,time
from pathlib import Path

class PrivateSSH:
    target="image-paste-fixture"
    def __init__(self,work):
        self.root=Path(work)/"private-ssh";self.root.mkdir(mode=0o700)
        self.process=None;self.log=None
        self.ssh=shutil.which("ssh");sshd=shutil.which("sshd")
        assert self.ssh and sshd and shutil.which("ssh-keygen"),"OpenSSH client/server required"
        for name in ("host-key","client-key"):
            subprocess.run(["ssh-keygen","-q","-t","ed25519","-N","","-f",str(self.root/name)],check=True)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1",0));self.port=sock.getsockname()[1]
        user=pwd.getpwuid(os.getuid()).pw_name
        config=self.root/"sshd_config"
        config.write_text(f"""Port {self.port}
ListenAddress 127.0.0.1
HostKey {self.root/'host-key'}
PidFile {self.root/'sshd.pid'}
AuthorizedKeysFile {self.root/'client-key.pub'}
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
AuthenticationMethods publickey
UsePAM no
PermitRootLogin prohibit-password
AllowUsers {user}
AllowTcpForwarding no
X11Forwarding no
PrintMotd no
LogLevel ERROR
""")
        self.log=(self.root/"sshd.log").open("wb")
        self.process=subprocess.Popen([sshd,"-D","-e","-f",str(config)],stdout=self.log,stderr=subprocess.STDOUT)
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            if self.process.poll() is not None:
                self.close();raise AssertionError("private sshd failed: "+(self.root/"sshd.log").read_text())
            try:
                with socket.create_connection(("127.0.0.1",self.port),timeout=.1):break
            except OSError:time.sleep(.04)
        else:
            self.close();raise AssertionError("private sshd did not listen")
        public=(self.root/"host-key.pub").read_text().split()
        self.known=self.root/"known_hosts";self.known.write_text(f"[127.0.0.1]:{self.port} {public[0]} {public[1]}\n")
        client_config=self.root/"ssh_config"
        client_config.write_text(f"""Host {self.target}
    HostName 127.0.0.1
    User {user}
    Port {self.port}
    IdentityFile {self.root/'client-key'}
    IdentitiesOnly yes
    UserKnownHostsFile {self.known}
    GlobalKnownHostsFile /dev/null
    StrictHostKeyChecking yes
    BatchMode yes
    ControlMaster no
    ControlPath none
""")
        self.bin=self.root/"bin";self.bin.mkdir()
        wrapper=self.bin/"ssh";self.status=self.root/"ssh.exit";self.write_wrapper()
        try:
            subprocess.run([str(wrapper),"-T",self.target,"true"],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=5)
        except Exception:
            self.close();raise
    def write_wrapper(self,delay=0):
        self.status.unlink(missing_ok=True)
        wrapper=self.bin/"ssh"
        wrapper.write_text("#!/bin/sh\n"+(f"sleep {delay}\n" if delay else "")+shlex.quote(self.ssh)+" -F "+shlex.quote(str(self.root/"ssh_config"))+" \"$@\"\nstatus=$?\nprintf '%s\\n' \"$status\" > "+shlex.quote(str(self.status))+"\nexit \"$status\"\n")
        wrapper.chmod(0o700)
    def close(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        if self.log:self.log.close()

if __name__=="__main__":
    import tempfile
    work=Path(tempfile.mkdtemp(prefix="rex-private-ssh-check-"))
    fixture=PrivateSSH(work)
    fixture.close()
    print("PASS isolated key-authenticated SSH command; private artifacts:",work)
