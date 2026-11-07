"""Isolate fix verification; save test exports outside the protected app-data directory."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name
    text=p.read_text()
    assert old in text,(name,old)
    p.write_text(text.replace(old,new))

edit('tools/print_gates.py', "'staging/print'", "'staging/printfix1'")
edit('tools/print_gates.py', 'fbv2_core_prref', 'fbv2_core_prfref')
edit('tools/print_gates.py', "'fbv2_core_pr'", "'fbv2_core_prf'")
edit('tools/print_gates.py', "    'node-tests':['npm','test'],", "    'node-tests':['npm','test'],\n    'save-path':['node','scripts/save-path-regression.cjs'],\n    'save-live':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/save-live-regression.cjs'],")
edit('tools/print_gates.py', "('native','smoke','smoke-register','node-tests')", "('native','smoke','smoke-register','node-tests','save-path','save-live')")
edit('crates/fb-screendiff/run_screendiff.sh', 'fbv2_core_pr|fbv2_core_prref|', 'fbv2_core_pr|fbv2_core_prref|fbv2_core_prf|fbv2_core_prfref|')
edit('tools/print_cleanup.py', "'staging/print'", "'staging/printfix1'")
edit('tools/print_cleanup.py', 'fbv2_core_prref', 'fbv2_core_prfref')
edit('tools/print_cleanup.py', "fbv2_core_pr'", "fbv2_core_prf'")
edit('tools/print_cleanup.py', 'fbv2_core_pr /', 'fbv2_core_prf /')
edit('electron/scripts/smoke-d8.cjs', "const csv=path.join(data,'balance-sheet.csv');", "const csv=path.join(shots,'d8-balance-sheet.csv');fs.rmSync(csv,{force:true});")
# The print button reports refusal via an alert. Record/dismiss it so the probe can complete.
edit('electron/scripts/save-live-regression.cjs', 'const page=await app.firstWindow();page.setDefaultTimeout(60000);', "const page=await app.firstWindow();page.setDefaultTimeout(60000);\n      const alerts=[];page.on('dialog',async dialog=>{alerts.push(dialog.message());await dialog.dismiss();});")
edit('electron/scripts/save-live-regression.cjs', "const errorText=await page.locator('body').innerText();", "const errorText=alerts.join('\\n');")
print('Fix checks use fbv2_core_prf / fbv2_core_prfref and staging/printfix1')
