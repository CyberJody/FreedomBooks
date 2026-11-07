"""Inspectable contact sheets from real 1440×1000 Electron navigation captures."""
from pathlib import Path
from PIL import Image,ImageDraw
ROOT=Path('/mnt/Data/Projects/FreedomBooks/staging/d7a')
files=sorted(ROOT.glob('d7a-*.png'))
assert files,'run the Electron smoke first'
for i in range(0,len(files),6):
    sheet=Image.new('RGB',(1440,1590),'#f1eadb');draw=ImageDraw.Draw(sheet)
    for n,p in enumerate(files[i:i+6]):
        im=Image.open(p);assert im.size==(1440,1000),(p,im.size)
        im.thumbnail((720,500));x=(n%2)*720;y=(n//2)*530
        sheet.paste(im,(x,y));draw.text((x+6,y+505),p.stem,fill='#3c2843')
    dest=ROOT/f'contact-d7a-{i//6+1:02}.png';sheet.save(dest);print(dest)
print(f'{len(files)} workflow screenshots verified at 1440×1000')
