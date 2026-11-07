"""Evidence reuse must bind clean SHA, toolchain, environment and exact verifier/test set."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

P = Path(__file__).resolve().parents[1]/'process_evidence.py'

class Evidence(unittest.TestCase):
    def test_reuse_rejects_dirty_mismatch_and_failed_results(self):
        self.assertTrue(P.exists(), 'evidence identity/reuse checker missing')
        spec = importlib.util.spec_from_file_location('process_evidence', P)
        e = importlib.util.module_from_spec(spec); spec.loader.exec_module(e)
        identity = dict(sha='a'*40, dirty='', toolchain='toolchain', testset='abc', environment={'TZ':'UTC'})
        good = dict(identity=identity, kind='acceptance', exit_code=0)
        self.assertTrue(e.reusable(good, identity, 'acceptance'))
        for key in ('sha', 'toolchain', 'testset', 'environment'):
            changed = dict(identity, **{key:'different'})
            self.assertFalse(e.reusable(good, changed, 'acceptance'), key)
        self.assertFalse(e.reusable(good, dict(identity, dirty=' M file'), 'acceptance'))
        self.assertFalse(e.reusable(dict(good, exit_code=1), identity, 'acceptance'))
        self.assertFalse(e.reusable(good, identity, 'quick'))

    def test_cli_runs_and_check_does_not_accept_wrong_tier(self):
        import subprocess
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)/'repo'; root.mkdir()
            subprocess.run(['git','init','-q',str(root)],check=True)
            subprocess.run(['git','-C',str(root),'-c','user.name=test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','fixture'],check=True)
            dest = Path(td)/'result.json'
            cmd = ['python3',str(P),'run','--kind','quick','--root',str(root),'--output',str(dest),'--','true']
            r = subprocess.run(cmd,capture_output=True,text=True)
            self.assertEqual(r.returncode,0,r.stderr)
            r = subprocess.run(['python3',str(P),'check','--kind','acceptance','--root',str(root),'--output',str(dest),'--','true'],capture_output=True,text=True)
            self.assertEqual(r.returncode,1,r.stderr)

if __name__ == '__main__':
    unittest.main(verbosity=2)
