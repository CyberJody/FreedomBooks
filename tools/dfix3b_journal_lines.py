"""Create the regression on genuine old source; verify with this worktree."""
from pathlib import Path
import os
import shutil
import subprocess
import tarfile
root=Path(__file__).resolve().parents[1]
out=root.parents[1]/'staging/dfix3b/journal-lines'
out.mkdir(parents=True,exist_ok=True)
example=Path('crates/fb-core/examples/dfix3b_journal_lines.rs')
tree=out/'98100e0-source'
tree.mkdir(exist_ok=True)
archive=out/'source.tar'
with archive.open('wb') as stream:
    subprocess.run(['git','archive','98100e0'],cwd=root,stdout=stream,check=True)
with tarfile.open(archive) as tar:
    tar.extractall(tree,filter='data')
archive.unlink()
shutil.copy2(root/example,tree/example)
shutil.copy2(root/'Cargo.lock',tree/'Cargo.lock')
books=out/'books'
if books.exists(): shutil.rmtree(books)
env=dict(os.environ,CARGO_BUILD_JOBS='4',RUST_TEST_THREADS='4')
def run(source,mode,label):
    log=out/(label+'.log')
    with log.open('w') as stream:
        result=subprocess.run(['cargo','run','--quiet','--profile','test','-p','fb-core','--example','dfix3b_journal_lines','--',mode,str(books)],cwd=source,env=env,stdout=stream,stderr=subprocess.STDOUT)
    print(label,'exit',result.returncode,flush=True)
    print('\n'.join(log.read_text().splitlines()[-10:]),flush=True)
    return result.returncode
env['CARGO_TARGET_DIR']=str(root/'target/dfix3b-old')
assert run(tree,'create','create')==0
shutil.copy2(books/'legacy.fbbook',books/'original.fbbook')
env.pop('CARGO_TARGET_DIR',None)
assert run(root,'verify','verify')==0
print('FIX3b genuine-source journal-line proof: EXIT 0',flush=True)
