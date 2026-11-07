"""Main-process printing via the existing SubmitForm boundary, with the strict CSP unchanged."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def edit(name,old,new):
    p=ROOT/name;text=p.read_text();assert text.count(old)==1,(name,old)
    p.write_text(text.replace(old,new))
edit('electron/main/main.cjs',"const native = require(development.nativeModule(app, __dirname));", "const native = require(development.nativeModule(app, __dirname));\nconst printing = require('./printing.cjs').controller(native);")
edit('electron/main/main.cjs',"    if (outcome.outcome === 'export') {", """    if (outcome.outcome === 'done' && outcome.data && outcome.data.desktop_print === true) {
      await printing.choose(win, outcome.data.target);
      return outcome;
    }
    if (outcome.outcome === 'export') {""")
edit('electron/main/main.cjs',"{ label: 'File', submenu: [{ label: 'Lock'", "{ label: 'File', submenu: [...printing.menuItems, { type:'separator' }, { label: 'Lock'")
p=ROOT/'electron/renderer/static/sales/print.js'
p.write_text('"use strict";\n// Printing is handled by the isolated preload through SubmitForm and by the main process.\n')
p=ROOT/'electron/preload/preload.cjs'
p.write_text(p.read_text()+'''
// PRINT keeps Django's button and packaged CSS. No renderer API or new IPC command is exposed.
document.addEventListener('click', async event => {
  const button = event.target.closest && event.target.closest('#print-now');
  if (!button || event.defaultPrevented || event.button !== 0) return;
  event.preventDefault();
  const input = button.querySelector('input[name=fb_command_token]');
  if (!input || button.disabled) return;
  button.disabled = true;
  try {
    const result = await call('books.run',{type:'SubmitForm',path:window.location.pathname+window.location.search,token:input.value,fields:[['action','desktop_print']]});
    if (result.outcome === 'done' && result.data.print_token) input.value = result.data.print_token;
  } catch (error) { window.alert(String(error.message || error)); }
  finally { button.disabled = false; }
}, true);

document.addEventListener('DOMContentLoaded', () => {
  if (!document.getElementById('print-now')) return;
  // The frozen document's @page margin boxes are inline and CSP blocks their style element.
  // Apply only that trusted template-generated rule through the CSSOM (as with the page icon).
  for (const style of document.head.querySelectorAll('style')) {
    const source = style.textContent.trim();
    if (!source.startsWith('@page {') || !source.endsWith('}')) continue;
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(source);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets,sheet];
  }
});
''')
print('PRINT: desktop button, native print menu and Save dialog wired without new command types')
