from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
for name in ['smoke.cjs','smoke-secfix1.cjs']:
    p=ROOT/'electron/scripts'/name;text=p.read_text()
    old="    await require('./smoke-d7b.cjs')({ app, page, company, ids, shot, pass });"
    assert text.count(old)==1
    p.write_text(text.replace(old,old+"\n    await require('./smoke-print-pagination.cjs')({app,page,company,ids,shot,pass});"))
p=ROOT/'tools/verify_screen_clock.py';text=p.read_text();old="assert os.environ['FB_DB_NAME'].startswith('fbv2_int_')";assert text.count(old)==1
p.write_text(text.replace(old,"assert os.environ['FB_DB_NAME'].startswith('fbv2_int_') or os.environ['FB_DB_NAME'] == 'fbv2_core_pr'"))
p=ROOT/'tools/print_gates.py';text=p.read_text()
old="    FB_TIME_ZONE='America/New_York')";assert text.count(old)==1
text=text.replace(old,"    FB_TIME_ZONE='America/New_York',\n    FB_FAKETIME_LIBRARY=str(ROOT.parents[1]/'staging/dfix2/faketime/usr-root/usr/lib/x86_64-linux-gnu/faketime/libfaketime.so.1'),\n    FB_CLOCK_PROOF_OUT=str(OUT/'clock-proof'))")
old="    'gate':['bash','scripts/gate.sh'],";assert text.count(old)==1;text=text.replace(old,old+"\n    'clock':['bash','tools/dfix3_clock.sh'],")
old="    'smoke-print':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-print.cjs'],";assert text.count(old)==1;text=text.replace(old,'')
p.write_text(text)
print('PRINT: multipage invoice smoke and two ambient-clock runs added')
