"""Reference tooling adapter and job-scoped database names (no reference writes)."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name;text=p.read_text();assert text.count(old)==1,(name,old);p.write_text(text.replace(old,new))
edit('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d5_*|','fbv2_core_pr|fbv2_core_prref|fbv2_int_d5_*|')
edit('crates/fb-screendiff/py/render_django.py','startswith(("fbv2_int_d5_",','startswith(("fbv2_core_pr", "fbv2_int_d5_",')
edit('crates/fb-app/src/screens/printing.rs',"AND kind='SRC'","AND kind='SR'")
edit('electron/main/main.cjs','      await printing.choose(win, outcome.data.target);','      try { await printing.choose(win, outcome.data.target); }\n      catch (error) { outcome.data.print_error = String(error.message || error); }')
edit('electron/preload/preload.cjs',"    if (result.outcome === 'done' && result.data.print_token) input.value = result.data.print_token;","    if (result.outcome === 'done' && result.data.print_token) input.value = result.data.print_token;\n    if (result.data && result.data.print_error) window.alert(result.data.print_error);")
print('PRINT: isolated gate resources ready')
