"""Drop only FIX3b's dedicated PostgreSQL databases after gate/harness finish."""
import subprocess
names=['fbv2_core_df3b','fbv2_core_df3bref',
       'fbv2_int_dfix1_df3bscreens','fbv2_int_dfix1_df3bref']
for name in names:
    subprocess.run(['dropdb','--if-exists',name],check=True)
query='SELECT datname FROM pg_database WHERE datname IN ('+','.join("'"+name+"'" for name in names)+')'
remaining=subprocess.check_output(['psql','-X','-At','-d','postgres','-c',query],text=True).strip()
assert not remaining, 'FIX3b databases remain'
print('FIX3b PostgreSQL cleanup: 4 dedicated DBs dropped; 0 remain')
