"""Shared QMP and HDC helpers for native OpenHarmony QEMU tests."""
import json
import os
from pathlib import Path
import socket
import subprocess


def qmp_command(path, command):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(10)
        try:
            sock.connect(str(path))
        except PermissionError:
            # The pinned Linux launcher invokes QEMU through sudo. Its umask
            # creates a root-owned socket, even if the caller used umask=0.
            # The socket lives in boot.py's private directory; give this user
            # access so both readiness checks and always() cleanup can use QMP.
            if Path(path).stat().st_uid != 0:
                raise
            subprocess.run(['sudo', '-n', 'chown', f'{os.getuid()}:{os.getgid()}', str(path)],
                           check=True, timeout=10)
            Path(path).chmod(0o600)
            sock.connect(str(path))
        with sock.makefile('r') as reader:
            greeting = json.loads(reader.readline())
            if 'QMP' not in greeting:
                raise RuntimeError('Not a QEMU QMP socket')
            for request in ('qmp_capabilities', command):
                sock.sendall((json.dumps({'execute': request}) + '\n').encode())
                while True:
                    line = reader.readline()
                    if not line:
                        if request == 'quit':
                            return greeting, {}
                        raise RuntimeError('QMP disconnected before replying')
                    response = json.loads(line)
                    if 'error' in response:
                        raise RuntimeError('QMP failed: ' + json.dumps(response))
                    if 'return' in response:
                        break
            return greeting, response['return']


def hdc_environment(guest):
    return dict(os.environ, HDC=guest['hdc'], HDC_TARGET=guest['target'],
                OHOS_HDC_SERVER_PORT=guest['server'], TMPDIR=guest['hdcTemp'],
                PATH=str(Path(guest['hdc']).parent) + os.pathsep + os.environ.get('PATH', ''))


def hdc_command(guest):
    return [guest['hdc'], '-s', guest['server']]
