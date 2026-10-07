#!/usr/bin/env python3
"""Boot the pinned OHOS release used by zig-napi's CI for native binary tests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import tarfile
import tempfile
import time

from common import hdc_command, hdc_environment, qmp_command

RELEASE = 'v20260919'
RELEASE_URL = 'https://github.com/harmony-contrib/ohos-qemu/releases/download/' + RELEASE
IMAGES = {
    'x86_64': ('openharmony-qemu-x86_64-x86_64_virt-phone.tar.gz',
               '08d35399119ec9b87d564cd8bf024e8a189921f5bacf09da889b7b223848a488'),
    'arm64': ('openharmony-qemu-arm64-arm64_virt-phone.tar.gz',
              'eda208b8ae5375e42af0f1ad6756d9e3ba57dd6ee9b13c2c7917c46b4ab14710'),
}


def verify_archive(path, expected):
    digest = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1024 * 1024), b''):
            digest.update(block)
    actual = digest.hexdigest()
    if actual != expected:
        raise RuntimeError(f'QEMU release SHA256 mismatch: {actual} != {expected}')
    return actual


def native_ready(text, arch):
    machine = 'aarch64' if arch == 'arm64' else 'x86_64'
    expected = {'FFRT_MACHINE': machine, 'FFRT_API': '26', 'FFRT_NATIVE_READY': '0'}
    return all(re.search(r'^' + key + '=' + value + r'\s*$', text, re.M)
               for key, value in expected.items())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--arch', choices=IMAGES, default='x86_64')
    parser.add_argument('--archive', type=Path, help='Use a local archive with the pinned SHA256')
    parser.add_argument('--hdc', default='hdc')
    parser.add_argument('--server', default='18721')
    parser.add_argument('--hdc-port', type=int, default=5579)
    parser.add_argument('--accel', choices=['kvm', 'hvf'], default='kvm')
    parser.add_argument('--timeout', type=int, default=900)
    args = parser.parse_args()
    host = platform.system()
    if args.timeout <= 0 or not 1 <= args.hdc_port <= 65535:
        parser.error('timeout and HDC port must be positive and the port must fit u16')
    if (host, args.arch, args.accel) not in (('Linux', 'x86_64', 'kvm'), ('Darwin', 'arm64', 'hvf')):
        parser.error('Use Linux x86_64/KVM or macOS arm64/HVF')
    if args.accel == 'kvm' and not os.access('/dev/kvm', os.R_OK | os.W_OK):
        parser.error('Readable/writable /dev/kvm is required')
    hdc = shutil.which(args.hdc)
    if not hdc:
        parser.error('HDC executable not found: ' + args.hdc)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    asset, expected = IMAGES[args.arch]
    archive = args.archive.resolve() if args.archive else root / asset
    if not args.archive:
        subprocess.run(['curl', '-fL', '--connect-timeout', '30', '--max-time', '900',
                        '--retry', '3', '--retry-delay', '2', '--retry-all-errors',
                        '--output', str(archive), RELEASE_URL + '/' + asset], check=True)
    digest = verify_archive(archive, expected)
    package_root = root / 'image'
    package_root.mkdir()
    with tarfile.open(archive, 'r:gz') as tar:
        tar.extractall(package_root, filter='data')
    launchers = list(package_root.glob('*/launch/' + ('linux.sh' if host == 'Linux' else 'macos.command')))
    if len(launchers) != 1:
        raise RuntimeError('Expected exactly one release launcher')
    launcher = launchers[0]
    # A short private directory fits macOS's Unix socket path limit and keeps
    # QMP private when the Linux launcher creates a root-owned socket.
    qmp_dir = Path(tempfile.mkdtemp(prefix='ffrt-qemu-', dir='/tmp'))
    qmp = qmp_dir / 'qmp.sock'
    hdc_temp = root / 'hdc-temp'
    hdc_temp.mkdir()
    command = ['bash', str(launcher), '--headless', '--qmp-socket', str(qmp),
               '--hdc-port', str(args.hdc_port), '--accel', args.accel,
               '-m', '4096', '-s', '4', '--', '-snapshot']
    state = {'release': RELEASE, 'archiveUrl': RELEASE_URL + '/' + asset,
             'archiveSha256': digest, 'architecture': args.arch, 'accelerator': args.accel,
             'command': command, 'qmp': str(qmp), 'qmpDirectory': str(qmp_dir),
             'hdc': str(Path(hdc).resolve()), 'hdcTemp': str(hdc_temp),
             'server': args.server, 'target': f'127.0.0.1:{args.hdc_port}'}
    state_path = root / 'guest.json'

    def save():
        state_path.write_text(json.dumps(state, indent=2) + '\n')

    save()
    env = hdc_environment(state)
    with (root / 'hdc.log').open('w') as log:
        server = subprocess.Popen([*hdc_command(state), '-m'], env=env,
                                  stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                  start_new_session=True)
    state['hdcPid'] = server.pid
    save()
    process = None
    try:
        with (root / 'qemu.log').open('w') as log:
            process = subprocess.Popen(command, cwd=launcher.parent.parent, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        state['pid'] = process.pid
        save()
        deadline = time.monotonic() + args.timeout

        def run(arguments):
            result = subprocess.run([*hdc_command(state), *arguments], env=env,
                                    capture_output=True, text=True, timeout=20)
            with (root / 'boot.log').open('a') as log:
                log.write(json.dumps({'arguments': arguments, 'exitCode': result.returncode,
                                      'output': result.stdout + result.stderr}) + '\n')
            return result.stdout if result.returncode == 0 else ''

        probe = ('echo "FFRT_MACHINE=$(uname -m)"; '
                 'echo "FFRT_API=$(param get const.ohos.apiversion)"; '
                 'mkdir -p /data/local/tmp && test -w /data/local/tmp && command -v timeout; '
                 'echo "FFRT_NATIVE_READY=$?"')
        while time.monotonic() < deadline:
            if process.poll() is not None or server.poll() is not None:
                raise RuntimeError('QEMU or HDC exited before readiness; see guest logs')
            try:
                run(['tconn', state['target']])
                ready = run(['-t', state['target'], 'shell', probe])
                if native_ready(ready, args.arch):
                    greeting, status = qmp_command(qmp, 'query-status')
                    if not status.get('running'):
                        raise RuntimeError('OHOS QEMU is not running')
                    if args.accel == 'kvm':
                        _, kvm = qmp_command(qmp, 'query-kvm')
                        if not kvm.get('enabled'):
                            raise RuntimeError('QEMU did not enable KVM')
                        state['kvm'] = kvm
                    state.update(qemu=greeting, qemuStatus=status, nativeState=ready, ready=True)
                    save()
                    print(state_path, flush=True)
                    return
            except subprocess.TimeoutExpired:
                pass
            time.sleep(3)
        raise RuntimeError('OHOS native readiness timed out; see ' + str(root / 'boot.log'))
    except BaseException:
        # The CI always() step also performs cleanup if this process is cancelled.
        try:
            if qmp.exists():
                qmp_command(qmp, 'quit')
            elif process is not None and process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        finally:
            server.terminate()
            server.wait(timeout=10)
        raise


if __name__ == '__main__':
    main()
