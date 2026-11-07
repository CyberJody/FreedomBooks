"""Compile real historical sources, create legacy books and prove upgrades/lifecycles."""
from pathlib import Path
import os
import shutil
import subprocess
import tarfile
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/dfix3/upgrade'
OUT.mkdir(parents=True,exist_ok=True)
EXAMPLE=Path('crates/fb-core/examples/dfix3_upgrade.rs')
def run(tree,args,label,env):
    log=OUT/(label+'.log')
    with log.open('w') as stream:
        result=subprocess.run(['cargo','run','--quiet','--profile','test','-p','fb-core','--example','dfix3_upgrade','--',*args],cwd=tree,env=env,stdout=stream,stderr=subprocess.STDOUT)
    print(f'{label}: exit {result.returncode}',flush=True)
    print('\n'.join(log.read_text().splitlines()[-18:]),flush=True)
    return result.returncode
for source in ['98100e0','cc40505']:
    tree=OUT/(source+'-source')
    tree.mkdir(exist_ok=True)
    archive=OUT/(source+'.tar')
    with archive.open('wb') as stream:
        subprocess.run(['git','archive',source],cwd=ROOT,stdout=stream,check=True)
    with tarfile.open(archive) as tar:tar.extractall(tree,filter='data')
    archive.unlink()
    (tree/EXAMPLE).parent.mkdir(parents=True,exist_ok=True)
    shutil.copy2(ROOT/EXAMPLE,tree/EXAMPLE)
    shutil.copy2(ROOT/'Cargo.lock',tree/'Cargo.lock')
    env=dict(os.environ,CARGO_TARGET_DIR=str(ROOT/'target'/('upgrade-'+source)))
    books=OUT/(source+'-books')
    if books.exists(): shutil.rmtree(books)
    for suffix in ['backups','restores','private']:
        folder=books.with_suffix('.'+suffix)
        if folder.exists(): shutil.rmtree(folder)
    assert run(tree,['create',str(books),source],source+'-create',env)==0
    shutil.copy2(books/'legacy.fbbook',books/'original.fbbook')
    current=dict(os.environ)
    current.pop('CARGO_TARGET_DIR',None)
    assert run(ROOT,['verify',str(books)],source+'-verify',current)==0
print('FIX3 genuine-source upgrade and lifecycle proof: EXIT 0',flush=True)
