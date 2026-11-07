"""Drop exactly the two authorized PRINT harness databases and verify neither remains."""
from pathlib import Path
import json
import subprocess
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
names=['fbv2_core_prf','fbv2_core_prfref']
for name in names:subprocess.run(['dropdb','--if-exists',name],check=True)
remaining=subprocess.check_output(['psql','-X','-At','-d','postgres','-c',
    "SELECT datname FROM pg_database WHERE datname IN ('fbv2_core_prf','fbv2_core_prfref')"],text=True).strip()
assert not remaining,remaining
(OUT/'database-cleanup.json').write_text(json.dumps({'dropped':names,'remaining':[]},indent=2)+'\n')
print('CLEANUP PASS: fbv2_core_prf / fbv2_core_prfref dropped; 0 remain')
