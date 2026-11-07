"""Remove only the four synthetic SEC-FIX1 gate databases, without forcing connections."""
import argparse,subprocess
parser=argparse.ArgumentParser();parser.add_argument('scope',choices=['harness','all']);args=parser.parse_args()
names=['fbv2_core_secfix1_run','fbv2_core_secfix1_ref']
if args.scope=='all':names+=['fbv2_int_dfix1_secfix1_screens','fbv2_int_dfix1_secfix1_ref']
for name in names:
    subprocess.run(['dropdb','--if-exists',name],check=True)
literal=','.join("'"+name+"'" for name in names)
remaining=subprocess.check_output(['psql','-X','-At','-d','postgres','-c',f'SELECT datname FROM pg_database WHERE datname IN ({literal})'],text=True).strip()
assert not remaining,'SEC-FIX1 databases remain'
print(f'SEC-FIX1 cleanup: {len(names)} named synthetic databases absent; no connections forced')
