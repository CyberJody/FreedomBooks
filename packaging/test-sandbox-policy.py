#!/usr/bin/env python3
"""Exercise the real maintainer scripts with fake sysctl values/parser, without root."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

pkg = Path(__file__).resolve().parent


class SandboxPolicy(unittest.TestCase):
    def check_policy(self, userns, restricted, parser_ok, expected):
        with tempfile.TemporaryDirectory(prefix='fb-policy-') as temporary:
            root = Path(temporary)
            tools = root / 'bin'
            tools.mkdir()
            helper = root / 'opt/FreedomBooks/chrome-sandbox'
            helper.parent.mkdir(parents=True)
            helper.touch()
            sysctls = root / 'proc/sys/kernel'
            sysctls.mkdir(parents=True)
            if userns != 'absent':
                (sysctls / 'unprivileged_userns_clone').write_text(str(userns))
            if restricted != 'absent':
                (sysctls / 'apparmor_restrict_unprivileged_userns').write_text(str(restricted))
            for name, script in {
                'apparmor_parser': 'printf "%s\\n" "$*" >> "$TEST_CALLS"; exit "$TEST_PARSER_EXIT"',
                'chown': 'exit 0',
                'ln': 'exit 0',
                'update-desktop-database': 'exit 0',
                'gtk-update-icon-cache': 'exit 0',
            }.items():
                tool = tools / name
                tool.write_text('#!/bin/sh\n' + script + '\n')
                tool.chmod(0o755)
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                       TEST_USERNS=str(userns), TEST_RESTRICTED=str(restricted),
                       TEST_PARSER_EXIT='0' if parser_ok else '1', TEST_CALLS=str(root / 'calls'))
            def run(name, *args):
                text = (pkg / 'linux' / name).read_text()
                for prefix in ['/opt/FreedomBooks', '/etc/apparmor.d', '/usr/bin/freedombooks', '/proc/sys/kernel']:
                    text = text.replace(prefix, str(root) + prefix)
                script = root / name
                script.write_text(text)
                return subprocess.run(['sh', str(script), *args], env=env, check=True, capture_output=True, text=True)
            result = run('after-install.sh')
            self.assertEqual(helper.stat().st_mode & 0o7777, expected)
            profile = root / 'etc/apparmor.d/app.freedombooks.FreedomBooks'
            if userns != 0 and restricted not in [0, 'absent']:
                self.assertIn('userns,', profile.read_text())
                self.assertIn('flags=(unconfined)', profile.read_text())
                self.assertIn('-r ', (root / 'calls').read_text())
                run('after-remove.sh', 'upgrade')
                self.assertTrue(profile.exists())
                run('after-remove.sh', 'remove')
                self.assertFalse(profile.exists())
                self.assertIn('-R ', (root / 'calls').read_text())
            if expected == 0o4755:
                self.assertIn('using setuid chrome-sandbox', result.stderr)
            else:
                self.assertNotIn('using setuid', result.stderr)

    def test_debian(self): self.check_policy(1, 'absent', True, 0o755)
    def test_kernel_without_userns_toggle(self): self.check_policy('absent', 'absent', True, 0o755)
    def test_unrestricted(self): self.check_policy(1, 0, True, 0o755)
    def test_mint_profile(self): self.check_policy(1, 1, True, 0o755)
    def test_disabled_userns(self): self.check_policy(0, 0, True, 0o4755)
    def test_disabled_userns_with_apparmor(self): self.check_policy(0, 1, True, 0o4755)
    def test_profile_load_failure(self): self.check_policy(1, 1, False, 0o4755)


if __name__ == '__main__':
    unittest.main(verbosity=2)
