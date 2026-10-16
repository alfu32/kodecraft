#!/usr/bin/env python3
"""Download verified VM/base fixtures; never installs a Snap on the host."""
import os
import subprocess
import tempfile
from pathlib import Path
from distribution_notices import sha
from installer_support import download
from snap_guest import CACHE, IMAGE, IMAGE_URL, IMAGE_SHA256, CORE24_SHA256


def main():
    download(IMAGE_URL,IMAGE_SHA256,IMAGE,800*1024*1024)
    base=CACHE/'core24_2124.snap';assertions=CACHE/'core24_2124.assert'
    if not base.is_file() or not assertions.is_file():
        with tempfile.TemporaryDirectory(prefix='core24-download-',dir=CACHE) as directory:
            subprocess.run(['snap','download','core24','--revision=2124','--basename=core24_2124'],cwd=directory,check=True,timeout=180)
            candidate=Path(directory)/base.name
            if sha(candidate.read_bytes())!=CORE24_SHA256:
                raise ValueError('Downloaded Snap base checksum differs')
            os.replace(candidate,base)
            os.replace(Path(directory)/assertions.name,assertions)
    if sha(base.read_bytes())!=CORE24_SHA256:
        raise ValueError('Cached Snap base checksum differs')
    # snap ack in the guest verifies Canonical's signed assertion chain before
    # installing this base. No host daemon, account or host installation needed.
    print('Verified Ubuntu VM and core24 fixtures; no host Snap installed',flush=True)


if __name__=='__main__':main()
