#!/usr/bin/env python3
"""Run native Rust tests and examples in the verified guest recorded by boot.py."""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys

from common import hdc_environment, qmp_command

TARGETS = {'arm64': 'aarch64-unknown-linux-ohos', 'x86_64': 'x86_64-unknown-linux-ohos'}


def resolve_linker(ndk, target):
    # setup-ohos-sdk exposes the SDK root as OHOS_NDK_HOME, whereas
    # OHOS_SDK_NATIVE and DevEco's native directory point at the component.
    root = ndk.resolve()
    candidates = [directory / 'llvm' / 'bin' / (target + '-clang')
                  for directory in (root, root / 'native')]
    for linker in candidates:
        if linker.is_file():
            return linker
    raise ValueError('OHOS linker not found; checked: ' + ', '.join(map(str, candidates)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guest', type=Path, required=True)
    parser.add_argument('--ndk', type=Path, required=True, help='Native component directory or SDK root')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.repeat <= 20:
        parser.error('--repeat must be between 1 and 20')
    guest = json.loads(args.guest.read_text())
    if not guest.get('ready') or guest['architecture'] not in TARGETS:
        parser.error('guest.json must describe a ready, supported OHOS guest')
    target = TARGETS[guest['architecture']]
    try:
        linker = resolve_linker(args.ndk, target)
    except ValueError as error:
        parser.error(str(error))
    _, status = qmp_command(guest['qmp'], 'query-status')
    if not status.get('running'):
        raise RuntimeError('OHOS QEMU is not running')
    repo = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = hdc_environment(guest)
    prefix = 'CARGO_TARGET_' + target.upper().replace('-', '_')
    env[prefix + '_LINKER'] = str(linker)
    env[prefix + '_RUNNER'] = shlex.join([sys.executable, str(repo / 'scripts/ohos-runner.py')])
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    evidence = {'revision': revision, 'guest': guest, 'target': target, 'repeat': args.repeat,
                'linker': str(linker), 'status': 'running', 'commands': []}

    def save():
        (output / 'results.json').write_text(json.dumps(evidence, indent=2) + '\n')

    def run(name, arguments):
        command = ['cargo', *arguments]
        entry = {'name': name, 'command': command, 'status': 'running'}
        evidence['commands'].append(entry)
        save()
        print(name, flush=True)
        try:
            with (output / (name + '.log')).open('w') as log:
                result = subprocess.run(command, cwd=repo, env=env, stdout=log,
                                        stderr=subprocess.STDOUT, timeout=600)
            entry.update(exitCode=result.returncode, status='passed' if result.returncode == 0 else 'failed')
            print((output / (name + '.log')).read_text(), end='', flush=True)
            if result.returncode:
                raise RuntimeError(f'{name} failed: {output / (name + ".log")}')
        except BaseException as error:
            entry.update(status='failed', error=str(error))
            raise
        finally:
            save()

    try:
        # Procedural macro tests execute on the host; target tests execute via HDC.
        run('macros', ['test', '--locked', '-p', 'ffrt-macros', '-p', 'napi-ffrt-ext-macro'])
        for index in range(1, args.repeat + 1):
            env['FFRT_EVIDENCE_DIR'] = str(output / f'run-{index}' / 'binaries')
            run(f'run-{index}-full', ['test', '--locked', '-p', 'ffrt', '--all-features',
                                    '--target', target, '--lib', '--tests'])
            run(f'run-{index}-minimal', ['test', '--locked', '-p', 'ffrt', '--no-default-features',
                                       '--target', target, '--test', 'regressions'])
            run(f'run-{index}-smoke', ['run', '--locked', '-p', 'ffrt', '--all-features',
                                     '--target', target, '--example', 'qemu_smoke'])
            run(f'run-{index}-tokio-compat', ['run', '--locked', '-p', 'tokio-compat', '--target', target])
        evidence['status'] = 'passed'
    except BaseException:
        evidence['status'] = 'failed'
        raise
    finally:
        save()
    print(output / 'results.json', flush=True)


if __name__ == '__main__':
    main()
