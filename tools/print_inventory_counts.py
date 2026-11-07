"""Count full print views separately from the already-ported toolbar-note fragment."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'tools/print_evidence.py';text=p.read_text()
old="prints=[key for key in screens if key.startswith('print-')]";assert text.count(old)==1
text=text.replace(old,"prints=[key for key,spec in screens.items() if key.startswith('print-') and 'template' not in spec]")
old="summary=dict(printViews=len(prints),screens=len(screens),";assert text.count(old)==1
text=text.replace(old,"summary=dict(printViews=len(prints),existingToolbarFragments=sum(k.startswith('print-') and 'template' in v for k,v in screens.items()),screens=len(screens),")
p.write_text(text)
p=ROOT/'tools/print_diff.py';text=p.read_text();old="print('PRINT: appended',sum(k.startswith('print-') for k in screens),'print views; total',len(screens))";assert text.count(old)==1
p.write_text(text.replace(old,"print('PRINT: full print views',sum(k.startswith('print-') and 'template' not in v for k,v in screens.items()),'; total',len(screens))"))
print('PRINT: inventory separates full print documents from the inherited toolbar fragment')
