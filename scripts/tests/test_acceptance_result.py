import importlib.util
from pathlib import Path
import tempfile
import unittest

class AcceptanceResults(unittest.TestCase):
    def test_missing_failed_or_duplicate_steps_never_pass(self):
        path = Path(__file__).resolve().parents[1]/'acceptance_result.py'
        self.assertTrue(path.exists(), 'strict acceptance step verifier missing')
        spec = importlib.util.spec_from_file_location('acceptance_result', path)
        mod = importlib.util.module_from_spec(spec); spec.loader.exec_module(mod)
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)
            required={'gate.log':'GATE_RC', 'harness.log':'HARNESS_RC', 'smoke.log':'SMOKE_RC', 'smoke-legacy.log':'SMOKE_LEGACY_RC'}
            self.assertFalse(mod.passed(root))
            for file,tag in required.items(): (root/file).write_text(tag+'=0\n')
            self.assertTrue(mod.passed(root))
            (root/'smoke.log').write_text('SMOKE_RC=124\n'); self.assertFalse(mod.passed(root))
            (root/'smoke.log').write_text('SMOKE_RC=1\nSMOKE_RC=0\n'); self.assertFalse(mod.passed(root))

if __name__=='__main__': unittest.main(verbosity=2)
