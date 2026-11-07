"""Reuse the review's PDF safety audit and absent-Django-route probe on fix evidence."""
from pathlib import Path
import os
import subprocess

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
REVIEW=ROOT.parents[1]/'staging/r-print'
for name in ['pdf-audit.py','credit-oracle.py']:
    text=(REVIEW/name).read_text()
    if name=='credit-oracle.py':
        text=text.replace("'adapter_supplies_new_context':True", "'adapter_supplies_new_context':False")
        text=text.replace("'template/data comparison only; no frozen credit-print view behavior exists'", "'no frozen credit-print view exists; desktop route and adapter removed'")
    (OUT/name).write_text(text)
env=dict(os.environ)
env.update(FB_DEBUG='1',FB_ENV_FILE='',FB_DB_NAME='fbv2_core_prf',PYTHONDONTWRITEBYTECODE='1',
           FB_DEV_SECRET_KEY_FILE=str(OUT/'reference-var/dev-secret-key'))
subprocess.run([str(ROOT.parents[1]/'wt/f2/.venv/bin/python'),str(OUT/'credit-oracle.py')],env=env,check=True)
subprocess.run(['python3',str(OUT/'pdf-audit.py')],check=True)
print('REVIEW PROBES PASS: Django credit print absent; all four PDFs have no paths, keys, scripts or embedded attachments')
