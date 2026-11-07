"""Match Django's white printed canvas without changing the ported HTML or CSS."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'electron/main/main.cjs'
text=p.read_text()
old="    backgroundColor: '#f7f3ec',"
assert text.count(old)==1
p.write_text(text.replace(old,"    backgroundColor: '#ffffff',"))
p=ROOT/'PRINT.md'
p.write_text(p.read_text()+'''

PDF margins use a white Electron window canvas (`backgroundColor: '#ffffff'`), matching Django's
browser output. Ported HTML/CSS and their on-screen tan theme are unchanged. `printBackground:true`
still preserves the document's intentional shaded cells and other template backgrounds.
''')
