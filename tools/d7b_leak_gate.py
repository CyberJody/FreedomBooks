"""Run review 11's unchanged generator/driver against this tree, with isolated on-disk evidence."""
from pathlib import Path
import os, shutil, subprocess, sys
ROOT=Path(__file__).resolve().parents[1]
SOURCE=Path('/mnt/Data/Projects/FreedomBooks/staging/r11-l4-bankfeeds-review')
OUT=Path('/mnt/Data/Projects/FreedomBooks/staging/d7b/leak')
OUT.mkdir(parents=True,exist_ok=True)
for name in ['gen.py','gen11.py','verify.py','split.py','adjacent.json']:
    shutil.copyfile(SOURCE/name,OUT/name)
for mode in ['baseline','vectors','control','joins']:
    subprocess.run([sys.executable,str(OUT/'gen11.py'),mode],check=True)
mine=(SOURCE/'mine.rs').read_text().replace(str(SOURCE),str(OUT))
(OUT/'mine.rs').write_text(mine)
tests=ROOT/'crates/fb-core/src/bankfeeds/tests.rs'
before=tests.read_text()
try:
    tests.write_text(before+f'\n#[path="{OUT}/mine.rs"]\nmod d7b_r11;\n')
    env=dict(os.environ,RUST_TEST_THREADS='1')
    with (OUT/'run.log').open('w') as log:
        result=subprocess.run(['cargo','test','-p','fb-core','--lib','bankfeeds::tests::d7b_r11::','--','--nocapture','--test-threads=1'],cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT)
    if result.returncode:
        print((OUT/'run.log').read_text()[-6000:]);sys.exit(result.returncode)
finally:
    assert tests.read_text().startswith(before)
    tests.write_text(before)
subprocess.run([sys.executable,str(ROOT/'tools/d7b_leak_verify.py')],check=True)
