"""Owned, temporary KVM guest for actual snapd/confinement acceptance.

No host Snap install, privileged container, user credentials or system change.
"""
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import tempfile
import time
from distribution_notices import ROOT, sha

CACHE = Path(tempfile.gettempdir()) / ("kode-rs-installers-" + str(os.getuid()))
IMAGE = CACHE / "noble-20260926-amd64.img"
IMAGE_URL = "https://cloud-images.ubuntu.com/noble/20260926/noble-server-cloudimg-amd64.img"
IMAGE_SHA256 = "6a81c37564db9b1ee84e141922625e1d7c5b389b99bb3c572e0243607d5bb4d2"
CORE24_SHA256 = "e950f61a36652f18817294875bcafb08d273ccc3c44b3f5fbf01296100078efb"


class Guest:
    def __enter__(self):
        if not IMAGE.is_file() or sha(IMAGE.read_bytes())!=IMAGE_SHA256:
            raise ValueError("Download and verify the pinned Ubuntu qualification image first")
        self.temporary = tempfile.TemporaryDirectory(prefix="kode-snap-vm-",dir=CACHE)
        self.root = Path(self.temporary.name)
        self.process = None
        self.log = None
        self.key = self.root / "key"
        try:
            subprocess.run(["ssh-keygen","-q","-t","ed25519","-N","","-f",str(self.key)],check=True)
            public = self.key.with_suffix(".pub").read_text().strip()
            (self.root / "user-data").write_text("#cloud-config\nusers:\n  - name: kode-uat\n    groups: [sudo]\n    shell: /bin/bash\n    lock_passwd: true\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    ssh_authorized_keys:\n      - "+json.dumps(public)+"\nssh_pwauth: false\ndisable_root: true\npackage_update: false\n")
            (self.root / "meta-data").write_text("instance-id: kode-rs-"+self.root.name+"\nlocal-hostname: kode-rs-uat\n")
            subprocess.run(["xorriso","-as","mkisofs","-quiet","-output",str(self.root / "seed.iso"),"-volid","cidata","-joliet","-rock","user-data","meta-data"],cwd=self.root,check=True,capture_output=True)
            subprocess.run(["qemu-img","create","-f","qcow2","-F","qcow2","-b",str(IMAGE.resolve()),str(self.root / "disk.qcow2"),"12G"],check=True,capture_output=True)
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1",0));self.port=reservation.getsockname()[1]
            self.log = (self.root / "qemu.log").open("wb")
            command = ["qemu-system-x86_64","-machine","accel=kvm","-cpu","host","-smp","2","-m","2048","-display","none","-monitor","none","-serial","file:"+str(self.root / "serial.log"),"-no-reboot","-drive",f"file={self.root / 'disk.qcow2'},format=qcow2,if=virtio","-drive",f"file={self.root / 'seed.iso'},format=raw,media=cdrom","-netdev",f"user,id=net0,hostfwd=tcp:127.0.0.1:{self.port}-:22","-device","virtio-net-pci,netdev=net0"]
            self.process = subprocess.Popen(command,stdin=subprocess.DEVNULL,stdout=self.log,stderr=subprocess.STDOUT)
            print("Booting owned Ubuntu Snap qualification VM",flush=True)
            deadline = time.monotonic()+240
            while time.monotonic()<deadline:
                if self.process.poll() is not None:
                    raise RuntimeError("Qualification VM exited: "+(self.root / "qemu.log").read_text()[-1500:])
                try:
                    result = self.run(["true"],check=False,timeout=5)
                except subprocess.TimeoutExpired:
                    continue
                if result.returncode==0:
                    break
                time.sleep(1)
            else:
                raise RuntimeError("VM SSH timeout: "+(self.root / "serial.log").read_text(errors="replace")[-2000:])
            self.run(["sudo","cloud-init","status","--wait"],timeout=180)
            print("Ubuntu VM ready; snapd acceptance runs inside the guest",flush=True)
            return self
        except BaseException:
            self.close()
            raise

    def options(self):
        return ["-F","/dev/null","-p",str(self.port),"-i",str(self.key),"-o","IdentitiesOnly=yes","-o","BatchMode=yes","-o","ConnectTimeout=3","-o","StrictHostKeyChecking=accept-new","-o","UserKnownHostsFile="+str(self.root / "known-hosts")]

    def argv(self,command,tty=False):
        return ["ssh",*self.options(),*(["-tt"] if tty else []),"kode-uat@127.0.0.1",shlex.join(list(map(str,command)))]

    def run(self,command,check=True,timeout=120):
        result = subprocess.run(self.argv(command),capture_output=True,text=True,timeout=timeout)
        if check and result.returncode:
            raise RuntimeError("Guest command failed: "+shlex.join(list(map(str,command)))+"\n"+result.stdout[-2000:]+result.stderr[-2000:])
        return result

    def put(self,source,destination):
        # Fixture destinations are fixed absolute paths without spaces. Actual
        # project paths with spaces are created with structured guest commands.
        if not destination.startswith("/") or any(ch.isspace() for ch in destination):
            raise ValueError("Use an absolute fixed fixture upload path")
        options = self.options()
        options[options.index("-p")] = "-P"
        subprocess.run(["scp",*options,str(source),"kode-uat@127.0.0.1:"+destination],check=True,capture_output=True,timeout=180)

    def install(self,artifact):
        cache = CACHE
        if sha((cache / 'core24_2124.snap').read_bytes())!=CORE24_SHA256:
            raise ValueError('Pinned Snap base checksum differs')
        self.put(cache / "core24_2124.snap","/tmp/core24.snap")
        self.put(cache / "core24_2124.assert","/tmp/core24.assert")
        self.put(artifact,"/tmp/kode-rs.snap")
        self.run(["sudo","snap","wait","system","seed.loaded"],timeout=180)
        self.run(["sudo","snap","ack","/tmp/core24.assert"])
        self.run(["sudo","snap","install","/tmp/core24.snap"],timeout=180)
        self.run(["sudo","snap","install","--dangerous","/tmp/kode-rs.snap"],timeout=360)
        print("Actual strict Snap installed in owned VM",flush=True)

    def close(self):
        if getattr(self,"process",None) and self.process.poll() is None:
            self.process.terminate()
            try:self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        if getattr(self,"log",None):self.log.close()
        if getattr(self,"temporary",None):self.temporary.cleanup()

    def __exit__(self,*_):
        self.close()
