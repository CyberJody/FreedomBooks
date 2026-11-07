"""Create books with actual frozen main source, upgrade with the desktop branch, compare exact hashes."""
from pathlib import Path
import os
import subprocess
import sys
import tarfile

ROOT=Path(__file__).resolve().parents[1]
OUT=Path(os.environ.get('FB_UPGRADE_OUT', ROOT/'target/desktop-upgrade'))
OUT.mkdir(parents=True,exist_ok=True)
source=OUT/'main-source'
source.mkdir(exist_ok=True)
archive=OUT/'main.tar'
with archive.open('wb') as stream:
    subprocess.run(['git','archive','b0b216a'],cwd=ROOT,stdout=stream,check=True)
with tarfile.open(archive) as tar:
    tar.extractall(source,filter='data')
example=Path('crates/fb-core/examples/desktop_upgrade.rs')
(source/example).parent.mkdir(parents=True,exist_ok=True)
(source/example).write_bytes((ROOT/example).read_bytes())
populate=Path('crates/fb-harness/examples/upgrade_populate.rs')
(source/populate).parent.mkdir(parents=True,exist_ok=True)
(source/populate).write_text('''use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut scenario: serde_json::Value = serde_json::from_str(include_str!("../scenarios/int-l4-combined.json")).unwrap();
    scenario["capture_steps"] = serde_json::json!(false);
    let mut runner = fb_harness::Runner::with_key(&scenario, Path::new(&args[1]), &[37; 32]);
    runner.run();
    let dump = runner.dump();
    for step in dump["outcomes"].as_array().unwrap() { assert_ne!(step["unexpected"], true, "{step}"); }
    println!("MAIN POPULATE PASS: {} pinned service steps, all L1-L4 modules", dump["outcomes"].as_array().unwrap().len());
}
''')
env=os.environ.copy()
# A private build cache, never main's working directory or another worker's target.
env['CARGO_TARGET_DIR']=str(ROOT/'target/upgrade-proof')
fixture=OUT/'books'
assert not fixture.exists(), f'use a new isolated output directory: {fixture}'
fixture.mkdir()
subprocess.run(['cargo','run','--quiet','--release','-p','fb-harness','--example','upgrade_populate','--',str(fixture/'main-populated.fbbook')],cwd=source,env=env,check=True)
for cwd,mode in [(source,'create'),(ROOT,'verify')]:
    # Cargo can reuse stale path-package fingerprints when same-version sources swap under one
    # target directory. Keep the frozen-main cache separate from the candidate's cache.
    if mode == 'verify':
        env.pop('CARGO_TARGET_DIR', None)
    subprocess.run(['cargo','run','--quiet','--release','-p','fb-core','--example','desktop_upgrade','--',mode,str(fixture)],cwd=cwd,env=env,check=True)
print('desktop upgrade proof: EXIT 0',flush=True)
