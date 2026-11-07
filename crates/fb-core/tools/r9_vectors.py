"""Review 9 vector generator; usage: python3 r9_vectors.py OUTPUT.json."""
from pathlib import Path
import random,json,html,base64,sys
from urllib.parse import unquote
output=Path(sys.argv[1])
rng=random.Random(0xF9B109)
pool=''.join(map(chr,range(32,127)))+'é€中😀\u0301\u200d'
def encode(s,layer):
    if layer=='raw':return s
    if layer=='pct':return ''.join('%%%02x'%b for b in s.encode())
    if layer=='json':return json.dumps(s,ensure_ascii=True)[1:-1].replace('/','\\/')
    if layer=='html' and ('&#' in s or '&amp;' in s):return html.escape(s)
    if layer=='html':return ''.join(('&#'+'0'*rng.randrange(0,40)+str(ord(c))+';') if html.unescape('&#%d;'%ord(c))==c else c for c in s)
    if layer=='hex':return ''.join('&#x'+format(ord(c),'x')+';' if html.unescape('&#x%x;'%ord(c))==c else c for c in s)
    if layer=='b64':return base64.b64encode(s.encode()).decode()
    if layer=='b64url':return base64.urlsafe_b64encode(s.encode()).decode().rstrip('=')
    raise ValueError(layer)
def decode(s,layer):
    if layer=='raw':return s
    if layer=='pct':return unquote(s)
    if layer=='json':return json.loads('"'+s+'"')
    if layer in ['html','hex']:return html.unescape(s)
    return base64.urlsafe_b64decode(s+'='*(-len(s)%4)).decode()
chains=[['raw'],['pct'],['pct','pct'],['json'],['html'],['hex'],['b64'],['b64url'],['html','pct'],['pct','json'],['json','html'],['pct','pct','pct'],['html','html','html'],['json','json','json']]
vs=[]
for i in range(10080):
    secret='R9'+''.join(rng.choice(pool) for _ in range(14))+pool[i%len(pool)]+'tail'
    layers=chains[i%len(chains)]
    text=secret
    for layer in layers:text=encode(text,layer)
    got=text
    for layer in reversed(layers):got=decode(got,layer)
    assert got==secret
    vs.append(dict(secret=secret,text=text,layers=layers))
output.write_text(json.dumps(vs,ensure_ascii=True))
print(f'Generated {len(vs)} independent R9 vectors: {output}')
