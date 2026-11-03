"""Exercise Pi deployment control flow without git/network/sudo/systemd changes."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

PROJECT = Path(__file__).resolve().parents[1]
FAKE_TOOL = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['RR_TEST_LOG'], 'a') as log:
    log.write(json.dumps([name, *args]) + '\n')
failure = os.environ.get('RR_TEST_FAIL', '')
if name == 'uname': print(os.environ.get('RR_TEST_OS', 'Linux'))
elif name == 'id': print(os.environ.get('RR_TEST_UID', '1000'))
elif name == 'git':
    if args == ['status', '--porcelain']: print(os.environ.get('RR_TEST_DIRTY', ''), end='')
    elif args == ['pull', '--ff-only'] and failure == 'pull': sys.exit(1)
elif name == 'cargo':
    if args[0] == failure: sys.exit(1)
    if args[0] == 'build':
        binary = pathlib.Path('target/release/release-radar')
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text('new binary\n')
        binary.chmod(0o755)
elif name == 'sudo': os.execvp(args[0], args)
elif name == 'systemctl':
    if args[0] == failure: sys.exit(1)
    if args[0] == 'is-active': print('active')
elif name == 'curl':
    count_path = pathlib.Path(os.environ['RR_TEST_LOG'] + '.curl-count')
    count = int(count_path.read_text()) + 1 if count_path.exists() else 1
    count_path.write_text(str(count))
    if failure == 'curl' or count < int(os.environ.get('RR_TEST_HEALTH_AFTER', '1')):
        sys.exit(22)
'''


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='radar-deploy-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for folder in ['deploy', 'scripts', 'bin', 'tools']:
            (self.root / folder).mkdir()
        for source in ['deploy/update-pi.sh', 'scripts/install-binary.sh']:
            destination = self.root / source
            shutil.copy2(PROJECT / source, destination)
            destination.chmod(0o755)
        (self.root / 'bin/release-radar').write_text('previous binary\n')
        for name in ['git', 'cargo', 'curl', 'sudo', 'systemctl', 'uname', 'id', 'sleep']:
            tool = self.root / 'tools' / name
            tool.write_text(FAKE_TOOL)
            tool.chmod(0o755)
        self.log = self.root / 'commands.jsonl'

    def run_deploy(self, **overrides):
        environment = {key: value for key, value in os.environ.items()
                       if key not in {'RR_INSTALL_DIR', 'RR_HEALTH_URL'} and not key.startswith('RR_TEST_')}
        environment.update(PATH=f"{self.root / 'tools'}:{os.environ['PATH']}", RR_TEST_LOG=str(self.log))
        environment.update(overrides)
        result = subprocess.run(['bash', str(self.root / 'deploy/update-pi.sh')],
                                cwd='/', env=environment, capture_output=True, text=True, timeout=20)
        commands = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, commands

    def test_success_pulls_tests_builds_installs_restarts_and_checks_health(self):
        result, commands = self.run_deploy(RR_TEST_HEALTH_AFTER='2')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(['git', 'pull', '--ff-only'], commands)
        self.assertIn(['cargo', 'test', '--locked'], commands)
        self.assertIn(['cargo', 'build', '--locked', '--release'], commands)
        restart = ['sudo', 'systemctl', 'restart', 'release-radar.service']
        self.assertGreater(commands.index(restart), commands.index(['cargo', 'build', '--locked', '--release']))
        self.assertIn(['systemctl', 'is-active', 'release-radar.service'], commands)
        curls = [command for command in commands if command[0] == 'curl']
        self.assertEqual(len(curls), 2)
        self.assertEqual(curls[-1][-1], 'http://127.0.0.1:9977/healthz')
        self.assertEqual((self.root / 'bin/release-radar').read_text(), 'new binary\n')
        self.assertEqual((self.root / 'bin/release-radar.previous').read_text(), 'previous binary\n')

    def test_pull_test_or_build_failure_keeps_running_binary_and_never_restarts(self):
        for failure in ['pull', 'test', 'build']:
            with self.subTest(failure=failure):
                result, commands = self.run_deploy(RR_TEST_FAIL=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(command[0] in {'sudo', 'systemctl', 'curl'} for command in commands))
                self.assertEqual((self.root / 'bin/release-radar').read_text(), 'previous binary\n')

    def test_install_failure_never_restarts(self):
        invalid_directory = self.root / 'not-a-directory'
        invalid_directory.write_text('keep this file')
        result, commands = self.run_deploy(RR_INSTALL_DIR=str(invalid_directory))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(command[0] in {'sudo', 'systemctl', 'curl'} for command in commands))
        self.assertEqual((self.root / 'bin/release-radar').read_text(), 'previous binary\n')
        self.assertEqual(invalid_directory.read_text(), 'keep this file')

    def test_restart_or_inactive_service_fails_before_healthcheck(self):
        for failure in ['restart', 'is-active']:
            with self.subTest(failure=failure):
                result, commands = self.run_deploy(RR_TEST_FAIL=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(command[0] == 'curl' for command in commands))

    def test_health_timeout_is_failure_with_diagnostics(self):
        result, commands = self.run_deploy(RR_TEST_FAIL='curl')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(sum(command[0] == 'curl' for command in commands), 10)
        self.assertIn('journalctl -u release-radar.service', result.stderr)

    def test_dirty_checkout_root_or_non_linux_refuses_before_pull(self):
        for override in [{'RR_TEST_DIRTY': ' M README.md\n'}, {'RR_TEST_UID': '0'}, {'RR_TEST_OS': 'Darwin'}]:
            with self.subTest(override=override):
                result, commands = self.run_deploy(**override)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(['git', 'pull', '--ff-only'], commands)
                self.assertFalse(any(command[0] in {'cargo', 'sudo', 'systemctl', 'curl'} for command in commands))


if __name__ == '__main__':
    unittest.main()
