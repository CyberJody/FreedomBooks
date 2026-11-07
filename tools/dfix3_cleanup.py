"""Drop only FIX3's four dedicated PostgreSQL databases, then verify absence."""
import subprocess
names=['fbv2_core_df3ref','fbv2_core_df3','fbv2_int_dfix1_df3ref','fbv2_int_dfix1_df3screens']
for name in names:
    count=subprocess.check_output(['psql','-X','-At','-d','postgres','-c',f"SELECT count(*) FROM pg_stat_activity WHERE datname='{name}'"],text=True).strip()
    assert count=='0',f'{name} still in use'
for name in names:subprocess.run(['dropdb','--if-exists',name],check=True)
present=subprocess.check_output(['psql','-X','-At','-d','postgres','-c',"SELECT datname FROM pg_database WHERE datname IN ("+','.join("'"+n+"'" for n in names)+")"],text=True).strip()
assert not present
print('PASS: all four FIX3 PostgreSQL databases dropped; absence verified')
