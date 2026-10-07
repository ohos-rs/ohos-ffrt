"""Check that transport and guest failures cannot produce a green CI result."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from boot import native_ready, verify_archive
from common import qmp_command
from run import resolve_linker

SPEC = importlib.util.spec_from_file_location('ohos_runner', Path(__file__).parents[1] / 'ohos-runner.py')
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class RunnerTests(unittest.TestCase):
    def test_remote_status_requires_one_final_valid_marker(self):
        for output in ('test result: ok\n', 'EXIT=0\nEXIT=0\n', 'EXIT=0\ndisconnected\n',
                       'EXIT=256\n', 'EXIT=-1\n', 'EXIT=abc\n'):
            with self.subTest(output=output), self.assertRaises(RuntimeError):
                RUNNER.parse_exit_status(output, 'EXIT=')
        self.assertEqual(RUNNER.parse_exit_status('failed\r\nEXIT=101\r\n', 'EXIT='), ('failed\r\n', 101))

    def run_binary(self, shell_output=None, remote_code=0, upload_fail=False,
                   timeout=False, cleanup_fail=False):
        self.calls = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / 'test-binary'
            binary.write_bytes(b'fixture')
            logs = root / 'evidence'

            def execute(command, **kwargs):
                self.calls.append(command)
                if 'file' in command:
                    stdout = '[Fail] upload rejected' if upload_fail else 'FileTransfer finish\n'
                elif command[-1].startswith('rm -f '):
                    if cleanup_fail:
                        raise subprocess.TimeoutExpired(command, 15)
                    stdout = ''
                else:
                    if timeout:
                        raise subprocess.TimeoutExpired(command, 190, output=b'partial guest output\n')
                    marker = re.search(r'FFRT_EXIT_[a-f0-9]+=', command[-1])[0]
                    stdout = shell_output if shell_output is not None else f'guest output\n{marker}{remote_code}\n'
                # HDC can report success even when a transfer or remote test fails.
                return subprocess.CompletedProcess(command, 0, stdout, '')

            with patch.dict(os.environ, {'HDC': '/fake/hdc', 'HDC_TARGET': '127.0.0.1:5579',
                                         'OHOS_HDC_SERVER_PORT': '18721',
                                         'FFRT_EVIDENCE_DIR': str(logs)}, clear=True), \
                    patch.object(sys, 'argv', ['ohos-runner.py', str(binary), 'argument with spaces']), \
                    patch.object(RUNNER.subprocess, 'run', side_effect=execute), \
                    contextlib.redirect_stdout(io.StringIO()):
                try:
                    code = RUNNER.main()
                    error = None
                except (RuntimeError, subprocess.TimeoutExpired) as exception:
                    code, error = None, exception
            evidence = json.loads(next(logs.glob('*.json')).read_text())
            output = next(logs.glob('*.log')).read_text()
            self.assertTrue(self.calls[-1][-1].startswith('rm -f '))
            self.assertTrue(all(command[:5] == ['/fake/hdc', '-s', '18721', '-t', '127.0.0.1:5579']
                                for command in self.calls))
            self.assertEqual(evidence['sha256'], hashlib.sha256(b'fixture').hexdigest())
            return code, error, evidence, output

    def test_success_records_binary_and_quotes_arguments(self):
        code, error, evidence, _ = self.run_binary()
        self.assertEqual(code, 0)
        self.assertIsNone(error)
        self.assertEqual(evidence['status'], 'passed')
        self.assertEqual(evidence['arguments'], ['argument with spaces'])
        self.assertIn("'argument with spaces'", self.calls[1][-1])

    def test_guest_failure_overrides_successful_hdc_exit(self):
        code, error, evidence, _ = self.run_binary(remote_code=101)
        self.assertEqual(code, 101)
        self.assertIsNone(error)
        self.assertEqual(evidence['status'], 'failed')
        self.assertEqual(evidence['exitCode'], 101)

    def test_missing_remote_status_is_failure(self):
        _, error, evidence, _ = self.run_binary(shell_output='test result: ok\n')
        self.assertIsInstance(error, RuntimeError)
        self.assertEqual(evidence['status'], 'failed')

    def test_upload_failure_does_not_execute_binary(self):
        _, error, evidence, _ = self.run_binary(upload_fail=True)
        self.assertIsInstance(error, RuntimeError)
        self.assertEqual(evidence['status'], 'failed')
        self.assertEqual(len(self.calls), 2)

    def test_transport_timeout_preserves_output_and_cleans_up(self):
        _, error, evidence, output = self.run_binary(timeout=True)
        self.assertIsInstance(error, subprocess.TimeoutExpired)
        self.assertEqual(evidence['status'], 'failed')
        self.assertIn('partial guest output', output)

    def test_cleanup_failure_preserves_guest_failure(self):
        code, error, evidence, _ = self.run_binary(remote_code=101, cleanup_fail=True)
        self.assertEqual(code, 101)
        self.assertIsNone(error)
        self.assertIn('cleanupError', evidence)


class GuestTests(unittest.TestCase):
    def test_archive_digest_rejects_modified_download(self):
        with tempfile.TemporaryDirectory() as temp:
            archive = Path(temp) / 'image.tar.gz'
            archive.write_bytes(b'corrupted download')
            with self.assertRaisesRegex(RuntimeError, 'SHA256 mismatch'):
                verify_archive(archive, '0' * 64)

    def test_native_readiness_requires_correct_arch_api_and_writable_tmp(self):
        ready = 'FFRT_MACHINE=x86_64\r\nFFRT_API=26\r\n/system/bin/timeout\r\nFFRT_NATIVE_READY=0\r\n'
        self.assertTrue(native_ready(ready, 'x86_64'))
        self.assertFalse(native_ready(ready, 'arm64'))
        self.assertFalse(native_ready(ready.replace('API=26', 'API=25'), 'x86_64'))
        self.assertFalse(native_ready(ready.replace('READY=0', 'READY=1'), 'x86_64'))
        self.assertFalse(native_ready('[Fail] No target', 'x86_64'))

    def test_qmp_shutdown_event_then_disconnect_is_allowed_only_for_quit(self):
        for command in ('quit', 'query-status'):
            with self.subTest(command=command), patch('common.socket.socket') as socket:
                client = socket.return_value.__enter__.return_value
                client.makefile.return_value = io.StringIO(
                    '{"QMP": {}}\n{"return": {}}\n{"event": "SHUTDOWN"}\n')
                if command == 'quit':
                    self.assertEqual(qmp_command('/fake/qmp.sock', command), ({'QMP': {}}, {}))
                else:
                    with self.assertRaisesRegex(RuntimeError, 'disconnected'):
                        qmp_command('/fake/qmp.sock', command)

    def test_qmp_recovers_socket_created_by_sudo_launcher(self):
        with patch('common.socket.socket') as socket, \
                patch('common.Path.stat') as stat, patch('common.Path.chmod') as chmod, \
                patch('common.subprocess.run') as execute:
            stat.return_value.st_uid = 0
            client = socket.return_value.__enter__.return_value
            client.connect.side_effect = [PermissionError(), None]
            client.makefile.return_value = io.StringIO(
                '{"QMP": {}}\n{"return": {}}\n{"return": {"running": true}}\n')
            self.assertTrue(qmp_command('/private/qmp.sock', 'query-status')[1]['running'])
            execute.assert_called_once_with(
                ['sudo', '-n', 'chown', f'{os.getuid()}:{os.getgid()}', '/private/qmp.sock'],
                check=True, timeout=10)
            chmod.assert_called_once_with(0o600)


class SdkLayoutTests(unittest.TestCase):
    def test_setup_action_sdk_root_and_native_component_resolve_same_linker(self):
        with tempfile.TemporaryDirectory() as temp:
            sdk = Path(temp)
            linker = sdk / 'native/llvm/bin/x86_64-unknown-linux-ohos-clang'
            linker.parent.mkdir(parents=True)
            linker.touch()
            self.assertEqual(resolve_linker(sdk, 'x86_64-unknown-linux-ohos'), linker.resolve())
            self.assertEqual(resolve_linker(sdk / 'native', 'x86_64-unknown-linux-ohos'), linker.resolve())

    def test_standalone_native_component_is_supported(self):
        with tempfile.TemporaryDirectory() as temp:
            native = Path(temp)
            linker = native / 'llvm/bin/aarch64-unknown-linux-ohos-clang'
            linker.parent.mkdir(parents=True)
            linker.touch()
            self.assertEqual(resolve_linker(native, 'aarch64-unknown-linux-ohos'), linker.resolve())

    def test_missing_target_reports_both_checked_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            sdk = Path(temp).resolve()
            with self.assertRaises(ValueError) as failure:
                resolve_linker(sdk, 'x86_64-unknown-linux-ohos')
            for directory in (sdk, sdk / 'native'):
                self.assertIn(str(directory / 'llvm/bin/x86_64-unknown-linux-ohos-clang'), str(failure.exception))


if __name__ == '__main__':
    unittest.main()
