from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'tools/print_gates.py';text=p.read_text()
old="    'check':['cargo','check','-p','fb-app','-p','fb-screendiff'],";assert text.count(old)==1
text=text.replace(old,old+"\n    'printing-tests':['cargo','test','-p','fb-app','-p','fb-screendiff','-p','fb-templates','print'],")
old="if name=='rediff':\n    commands[name]=['bash','tools/print_rediff.sh']";assert text.count(old)==1
text=text.replace(old,old+"\nif name=='compare':\n    commands[name]=['bash','tools/print_compare.sh']")
p.write_text(text)
print('PRINT: focused regressions and saved-reference comparison commands ready')
