from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'tools/print_gates.py';text=p.read_text()
old="    'smoke':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-secfix1.cjs'],";assert text.count(old)==1
text=text.replace(old,old+"\n    'smoke-register':['xvfb-run','-a','-s','-screen 0 1440x1000x24','node','scripts/smoke-print-register.cjs'],")
old="if name in ('native','smoke','smoke-print','node-tests')";assert text.count(old)==1;text=text.replace(old,"if name in ('native','smoke','smoke-register','node-tests')")
p.write_text(text)
print('PRINT: native landscape-register PDF check added')
