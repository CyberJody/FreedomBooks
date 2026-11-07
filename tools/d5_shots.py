"""Inventory the real Electron catalogue screenshots and create review contact sheets."""
from pathlib import Path
import hashlib
import json
from PIL import Image,ImageDraw
root=Path('/mnt/Data/Projects/FreedomBooks/staging/d5')
files=sorted(root.glob('catalogue-d5-*.png'))
inventory=[]
for p in files:
    with Image.open(p) as im:
        assert im.size==(1440,1000),(p,im.size)
        inventory.append(dict(path=str(p),width=im.width,height=im.height,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
root.joinpath('screenshots.json').write_text(json.dumps(inventory,indent=2)+'\n')
for index in range(0,len(files),6):
    sheet=Image.new('RGB',(1440,1072),'#ddd5c8');draw=ImageDraw.Draw(sheet)
    for i,p in enumerate(files[index:index+6]):
        with Image.open(p) as im:
            im.thumbnail((480,500));x=(i%3)*480;y=(i//3)*536;sheet.paste(im,(x,y+24));draw.text((x+8,y+6),p.stem,fill='black')
    sheet.save(root/f'd5-contact-{index//6+1:02}.png')
print(f'{len(files)} D5 screenshots verified at 1440×1000')
