"""Diagnostic contact sheet of actual Electron navigations; originals remain unchanged."""
from pathlib import Path
from PIL import Image,ImageDraw
root=Path(__file__).resolve().parents[1]
out=root.parents[1]/'staging/dfix3'
folder=out/'screenshots-smoke'
files=sorted(folder.glob('d7b-0[1-9]-*.png'))
assert len(files)==9,[p.name for p in files]
sheet=Image.new('RGB',(2160,1590),'white')
draw=ImageDraw.Draw(sheet)
for i,p in enumerate(files):
    img=Image.open(p)
    assert img.size==(1440,1000),(p.name,img.size)
    img.thumbnail((720,500))
    x=(i%3)*720;y=(i//3)*530
    draw.text((x+8,y+8),p.name,fill='black')
    sheet.paste(img,(x,y+30))
sheet.save(out/'d7b-contact-sheet.png')
print(out/'d7b-contact-sheet.png')
