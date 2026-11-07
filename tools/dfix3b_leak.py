"""Replay review 11 on committed FIX3b, retaining separate evidence."""
from pathlib import Path
import os
import subprocess
import sys
root=Path(__file__).resolve().parents[1]
source=(root/'tools/dfix3_leak.py').read_text()
source=source.replace("root=Path(__file__).resolve().parents[1]",'root=Path('+repr(str(root))+')')
source=source.replace('staging/dfix3/leak','staging/dfix3b/leak')
source=source.replace('archive.unlink()',"archive.unlink()\nshutil.copy2(root/'Cargo.lock',scratch/'Cargo.lock')")
runner=root/'target/dfix3b_leak_runner.py'
runner.parent.mkdir(exist_ok=True)
runner.write_text(source)
env=dict(os.environ,CARGO_TARGET_DIR=str(root/'target/dfix3b-r11'))
sys.exit(subprocess.run(['python3',str(runner)],env=env).returncode)
