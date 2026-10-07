#!/usr/bin/env python3
"""Cargo runner that executes native binaries on an OpenHarmony HDC target.

HDC/HDC_TARGET select the executable and device. OHOS_HDC_SERVER_PORT selects
an isolated server. FFRT_EVIDENCE_DIR optionally retains per-binary logs/JSON.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import uuid


def parse_exit_status(output, marker):
    matches = list(re.finditer(r'^' + re.escape(marker) + r'(\d+)\s*$', output, re.M))
    if len(matches) != 1 or output[matches[0].end():].strip():
        raise RuntimeError('HDC did not return one final remote test exit status')
    code = int(matches[0][1])
    if code > 255:
        raise RuntimeError('Invalid remote test exit status')
    return output[:matches[0].start()], code


def main():
    if len(sys.argv) < 2:
        raise SystemExit('usage: ohos-runner.py EXECUTABLE [ARG ...]')
    timeout = int(os.environ.get('FFRT_TEST_TIMEOUT', '180'))
    if timeout <= 0:
        raise ValueError('FFRT_TEST_TIMEOUT must be positive')
    hdc = [os.environ.get('HDC', 'hdc')]
    if os.environ.get('OHOS_HDC_SERVER_PORT'):
        hdc += ['-s', os.environ['OHOS_HDC_SERVER_PORT']]
    if os.environ.get('HDC_TARGET'):
        hdc += ['-t', os.environ['HDC_TARGET']]
    executable = Path(sys.argv[1]).resolve(strict=True)
    nonce = uuid.uuid4().hex
    remote = f'/data/local/tmp/ffrt-test-{nonce}'
    marker = f'FFRT_EXIT_{nonce}='
    evidence = {'executable': str(executable), 'sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
                'arguments': sys.argv[2:], 'target': os.environ.get('HDC_TARGET'), 'status': 'failed'}
    log = ''
    output_dir = os.environ.get('FFRT_EVIDENCE_DIR')
    try:
        transfer = subprocess.run(hdc + ['file', 'send', str(executable), remote],
                                  capture_output=True, text=True, timeout=60, check=True)
        log += transfer.stdout + transfer.stderr
        if '[Fail]' in log:
            raise RuntimeError('HDC upload failed: ' + log)
        arguments = ' '.join(shlex.quote(arg) for arg in sys.argv[2:])
        # Enforce the deadline in the guest too, so a hung test cannot outlive
        # the host HDC command. Allow ten seconds for transport and cleanup.
        command = (f'chmod 700 {remote} && TMPDIR=/data/local/tmp timeout -k 5 {timeout} '
                   f'{remote} {arguments}; code=$?; printf "\\n{marker}%s\\n" "$code"')
        result = subprocess.run(hdc + ['shell', command], capture_output=True, text=True,
                                timeout=timeout + 10)
        log += result.stdout + result.stderr
        if result.returncode:
            raise RuntimeError(f'HDC shell exited with {result.returncode}')
        output, code = parse_exit_status(result.stdout, marker)
        print(output, end='', flush=True)
        print(result.stderr, end='', file=sys.stderr, flush=True)
        evidence.update(exitCode=code, status='passed' if code == 0 else 'failed')
        return code
    except subprocess.TimeoutExpired as error:
        log += (error.stdout or b'').decode(errors='replace') if isinstance(error.stdout, bytes) else (error.stdout or '')
        evidence['error'] = 'HDC command timed out'
        raise
    except Exception as error:
        evidence['error'] = str(error)
        raise
    finally:
        try:
            cleanup = subprocess.run(hdc + ['shell', f'rm -f {remote}'], capture_output=True,
                                     text=True, timeout=15)
            if cleanup.returncode or '[Fail]' in cleanup.stdout + cleanup.stderr:
                evidence['cleanupError'] = cleanup.stdout + cleanup.stderr
        except (OSError, subprocess.TimeoutExpired) as error:
            evidence['cleanupError'] = str(error)
        if output_dir:
            directory = Path(output_dir)
            directory.mkdir(parents=True, exist_ok=True)
            stem = f'{executable.name}-{nonce}'
            (directory / (stem + '.log')).write_text(log)
            (directory / (stem + '.json')).write_text(json.dumps(evidence, indent=2) + '\n')


if __name__ == '__main__':
    sys.exit(main())
