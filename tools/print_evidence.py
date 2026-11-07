"""Inventory PRINT evidence and make review sheets without changing any captured pixels."""
from pathlib import Path
import hashlib
import json
import re
from PIL import Image, ImageDraw

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/print'
screens=json.loads((ROOT/'crates/fb-screendiff/screens.json').read_text())
prints=[key for key,spec in screens.items() if key.startswith('print-') and 'template' not in spec]
images=[]
for key in prints:
    path=OUT/('catalogue-'+key+'.png')
    assert path.is_file(),path
    with Image.open(path) as image:
        assert image.size==(1440,1000),(path,image.size)
    images.append(path)
gallery=OUT/'review';gallery.mkdir(exist_ok=True)
def sheet(paths,filename):
    canvas=Image.new('RGB',(1440,1095),'#ddd7de')
    draw=ImageDraw.Draw(canvas)
    for i,path in enumerate(paths):
        x=(i%3)*480;y=(i//3)*365
        with Image.open(path) as source:
            thumb=source.convert('RGB');thumb.thumbnail((480,333));canvas.paste(thumb,(x,y))
        draw.text((x+6,y+337),path.stem.removeprefix('catalogue-')[:72],fill='#111')
    dest=gallery/filename;canvas.save(dest);return str(dest)
sheets=[sheet(images[i:i+9],f'print-views-{i//9+1:02}.png') for i in range(0,len(images),9)]
bottoms=sorted(OUT.glob('catalogue-print-*-bottom.png'))
sheets.extend(sheet(bottoms[i:i+9],f'print-bottoms-{i//9+1:02}.png') for i in range(0,len(bottoms),9))
pdf_pages=sorted(OUT.glob('*-page[0-9]*.png'))
sheets.extend(sheet(pdf_pages[i:i+9],f'pdf-pages-{i//9+1:02}.png') for i in range(0,len(pdf_pages),9))
inventory=[]
for path in images+bottoms+pdf_pages:
    with Image.open(path) as image:dimensions=list(image.size)
    inventory.append(dict(file=str(path),dimensions=dimensions,sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
totals=dict(passed=0,failed=0,ignored=0)
log=(OUT/'gate.log').read_text()
for passed,failed,ignored in re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored',log):
    for name,count in [('passed',passed),('failed',failed),('ignored',ignored)]:totals[name]+=int(count)
summary=dict(printViews=len(prints),existingToolbarFragments=sum(k.startswith('print-') and 'template' in v for k,v in screens.items()),screens=len(screens),screenshots=len(images),bottomScreenshots=len(bottoms),pdfPageRenders=len(pdf_pages),
             contactSheets=sheets,gateTestExecutions=totals,inventory=inventory)
(OUT/'evidence.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({k:v for k,v in summary.items() if k!='inventory'},indent=2))
