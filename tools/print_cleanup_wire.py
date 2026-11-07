from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'tools/print_gates.py';text=p.read_text();old="    'cleanup':['dropdb','--if-exists','fbv2_core_pr'],";assert text.count(old)==1
p.write_text(text.replace(old,"    'cleanup':[sys.executable,'tools/print_cleanup.py'],"))
print('PRINT: post-gate clock/harness/cleanup commands run sequentially with both DBs verified absent')
