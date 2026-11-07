"""Inventory and contact sheets for the actual 1440x1000 Electron captures."""
from pathlib import Path
import hashlib
import json
from PIL import Image, ImageDraw

root=Path('/mnt/Data/Projects/FreedomBooks/staging/d2')
files=sorted(root.glob('*.png'))
files=[p for p in files if not p.name.startswith('contact-')]
inventory=[]
for p in files:
    with Image.open(p) as im:
        assert im.size==(1440,1000),(p,im.size)
        inventory.append(dict(path=str(p),width=im.width,height=im.height,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
root.joinpath('screenshots.json').write_text(json.dumps(inventory,indent=2)+'\n')
for index in range(0,len(files),9):
    subset=files[index:index+9]
    sheet=Image.new('RGB',(1440,3*366),'#ddd5c8')
    draw=ImageDraw.Draw(sheet)
    for i,p in enumerate(subset):
        with Image.open(p) as im:
            im.thumbnail((480,334))
            x=(i%3)*480;y=(i//3)*366
            sheet.paste(im,(x,y+24));draw.text((x+8,y+6),p.stem,fill='black')
    sheet.save(root/f'contact-{index//9+1}.png')
print(f'{len(files)} screenshots verified at 1440x1000')
