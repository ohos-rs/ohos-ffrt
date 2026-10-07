#!/usr/bin/env python3
"""Cargo target runner for an already connected OpenHarmony device/QEMU guest.

Set HDC to the hdc executable and HDC_TARGET to its connection key. The usual
OHOS_HDC_SERVER_PORT environment variable selects an isolated HDC server.
"""
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import uuid


def main():
    hdc = [os.environ.get("HDC", "hdc")]
    if os.environ.get("HDC_TARGET"):
        hdc += ["-t", os.environ["HDC_TARGET"]]
    executable = Path(sys.argv[1]).resolve()
    nonce = uuid.uuid4().hex
    remote = f"/data/local/tmp/ffrt-test-{nonce}"
    marker = f"FFRT_EXIT_{nonce}="
    try:
        transfer = subprocess.run(hdc + ["file", "send", str(executable), remote],
                                  capture_output=True, text=True, timeout=60, check=True)
        if "[Fail]" in transfer.stdout or "[Fail]" in transfer.stderr:
            raise RuntimeError(transfer.stdout + transfer.stderr)
        arguments = " ".join(shlex.quote(arg) for arg in sys.argv[2:])
        command = (f"chmod 700 {remote} && TMPDIR=/data/local/tmp {remote} {arguments}; "
                   f"code=$?; printf '\\n{marker}%s\\n' \"$code\"")
        result = subprocess.run(hdc + ["shell", command], capture_output=True, text=True,
                                timeout=int(os.environ.get("FFRT_TEST_TIMEOUT", "180")))
        match = re.search(re.escape(marker) + r"(\d+)", result.stdout)
        print(result.stdout[:match.start()] if match else result.stdout, end="", flush=True)
        print(result.stderr, end="", file=sys.stderr, flush=True)
        if result.returncode or match is None:
            raise RuntimeError("HDC did not return the remote test exit status")
        return int(match[1])
    finally:
        subprocess.run(hdc + ["shell", f"rm -f {remote}"], capture_output=True, timeout=15)


if __name__ == "__main__":
    sys.exit(main())
