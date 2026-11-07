"""Exercise print via real navigation in the inherited smoke, using its existing posted invoice."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
for name in ['smoke.cjs','smoke-secfix1.cjs']:
    p=ROOT/'electron/scripts'/name;text=p.read_text()
    old="    pass('invoice posted (state Open)');"
    assert text.count(old)==1
    text=text.replace(old,old+"\n    await require('./smoke-print.cjs')({app,page,company,invoiceId,shot,pass});")
    p.write_text(text)
p=ROOT/'electron/preload/preload.cjs'
p.write_text(p.read_text()+'''
// Django opens customer prints in a new tab. The hardened app uses the same local window.
document.addEventListener('click', event => {
  const anchor=event.target.closest && event.target.closest('a[href]');
  if (!anchor || event.defaultPrevented || event.button!==0 || anchor.target!=='_blank') return;
  const target=new URL(anchor.href,window.location.href);
  if (target.origin!==window.location.origin || !target.pathname.endsWith('/print/')) return;
  event.preventDefault();window.location.assign(target.href);
},true);
''')
print('PRINT: live invoice/P&L PDF smoke and same-window print links added')
