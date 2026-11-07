"""Inventory actual real-navigation D8 Electron screenshots and make review contact sheets."""
from pathlib import Path
import hashlib
import json
from PIL import Image,ImageDraw
ROOT=Path('/mnt/Data/Projects/FreedomBooks/staging/d8')
files=sorted(ROOT.glob('catalogue-d8-*.png'))+sorted(ROOT.glob('d8-flow-*.png'))
inventory=[]
for p in files:
    with Image.open(p) as im:
        assert im.size==(1440,1000),(p,im.size)
        inventory.append(dict(path=str(p),width=im.width,height=im.height,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
(ROOT/'screenshots.json').write_text(json.dumps(inventory,indent=2)+'\n')
for offset in range(0,len(files),6):
    sheet=Image.new('RGB',(1440,1048),'#ddd5c8');draw=ImageDraw.Draw(sheet)
    for index,p in enumerate(files[offset:offset+6]):
        with Image.open(p) as im:
            im.thumbnail((480,480));x=(index%3)*480;y=(index//3)*524
            sheet.paste(im,(x,y+30));draw.text((x+4,y+6),p.stem[:60],fill='black')
    sheet.save(ROOT/f'contact-{offset//6+1:02}.png')
print(f'{len(files)} D8 screenshots verified at 1440×1000')
