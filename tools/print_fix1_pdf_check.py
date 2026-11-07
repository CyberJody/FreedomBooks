"""Check the newly rendered PDF page margins, retaining the review's tan baseline."""
from pathlib import Path
import json
from PIL import Image

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT.parents[1]/'staging/printfix1'
names=['invoice-page1.png','profit-loss-page1.png','register-page1.png']
names.extend(f'invoice-multipage-page{page}.png' for page in range(1,6))
result=[]
for name in names:
    with Image.open(OUT/name) as source:
        image=source.convert('RGB')
        w,h=image.size
        points=[(2,2),(w-3,2),(2,h-3),(w-3,h-3),(w//2,2),(2,h//2),(w-3,h//2),(w//2,h-3)]
        colors=[image.getpixel(point) for point in points]
        assert all(color==(255,255,255) for color in colors),(name,colors)
        result.append({'page':name,'size':[w,h],'marginColors':[list(color) for color in colors]})
baseline=ROOT.parents[1]/'staging/r-print/register-page1.png'
with Image.open(baseline) as image:
    previous=image.convert('RGB').getpixel((2,2))
assert previous==(247,243,236),previous
(OUT/'pdf-margins.json').write_text(json.dumps({'reviewBaselineMargin':list(previous),'pages':result},indent=2)+'\n')
print(f'PDF MARGINS PASS: {len(result)} regenerated pages have white margins (255,255,255); review baseline was tan {previous}')
