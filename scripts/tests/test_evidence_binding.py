"""B1: independent caller commands, not record-chosen verifiers."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'process_evidence.py'
LEAD = Path('/mnt/Data/Projects/FreedomBooks/tools/lead-verify.sh')

class Binding(unittest.TestCase):
    def test_check_requires_expected_command_and_rejects_different_command(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)/'repo'; root.mkdir()
            subprocess.run(['git','init','-q',str(root)],check=True)
            subprocess.run(['git','-C',str(root),'-c','user.name=test','-c','user.email=test@example.invalid','commit','--allow-empty','-qm','fixture'],check=True)
            base = ['python3',str(SCRIPT)]
            common = ['--kind','acceptance','--root',str(root),'--output',str(Path(td)/'evidence.json')]
            result = subprocess.run(base+['run']+common+['--','/usr/bin/true'],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            for command in ([],['--','/usr/bin/false']):
                result = subprocess.run(base+['check']+common+command,capture_output=True,text=True)
                self.assertNotEqual(result.returncode,0,'missing/different expected verifier was accepted')
            result = subprocess.run(base+['check']+common+['--','/usr/bin/true'],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)

    def test_lead_reuse_supplies_independent_verifier_and_checks_required_steps(self):
        # Execute the actual reuse branch against local command spies: no builds.
        with tempfile.TemporaryDirectory() as td:
            root=Path(td); (root/'tools').mkdir(); (root/'bin').mkdir()
            work=root/'wt-v2/dmverify-fixture'; (work/'scripts').mkdir(parents=True)
            out=root/'runs/fixture-date/verify-fixture'; out.mkdir(parents=True)
            (out/'evidence.json').write_text('{}'); (out/'SUMMARY.txt').write_text('fixture summary')
            (root/'bin/git').write_text('#!/bin/sh\necho aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n')
            (root/'bin/date').write_text('#!/bin/sh\necho fixture-date\n')
            for path in (root/'bin').iterdir(): path.chmod(0o755)
            spy=work/'scripts/process_evidence.py'
            spy.write_text('import json,sys\nfrom pathlib import Path\nPath('+repr(str(root/'args.json'))+').write_text(json.dumps(sys.argv[1:]))\n')
            checker=work/'scripts/acceptance_result.py'
            checker.write_text('raise SystemExit(1)\n')
            source=LEAD.read_text().replace('R=/mnt/Data/Projects/FreedomBooks','R='+str(root))
            source=source.replace('export PATH=$HOME/.cargo/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin','export PATH='+str(root/'bin')+':/usr/bin:/bin')
            lead=root/'tools/lead-verify.sh'; lead.write_text(source)
            result=subprocess.run(['/bin/bash',str(lead),'a'*40,'fixture'],capture_output=True,text=True,env=dict(os.environ,PATH=str(root/'bin')+':/usr/bin:/bin'))
            args=json.loads((root/'args.json').read_text())
            self.assertIn('--',args,'lead did not supply expected verifier')
            self.assertEqual(args[args.index('--')+1:],['/bin/bash',str(lead),'--inner',str(work),str(out),'fixture'])
            self.assertNotEqual(result.returncode,0,'missing/failed required acceptance steps reused')
            checker.write_text('raise SystemExit(0)\n')
            result=subprocess.run(['/bin/bash',str(lead),'a'*40,'fixture'],capture_output=True,text=True,env=dict(os.environ,PATH=str(root/'bin')+':/usr/bin:/bin'))
            self.assertEqual(result.returncode,0,result.stderr)

if __name__ == '__main__': unittest.main(verbosity=2)
