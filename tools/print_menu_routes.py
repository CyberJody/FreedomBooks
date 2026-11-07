"""Native File actions can open the corresponding print view, for the existing Django print routes."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'electron/main/printing.cjs';text=p.read_text()
old='''      const target = current(win, expected);'''
new='''      if (!expected && win && !win.isDestroyed() && JSON.parse(native.appState()).open) {
        const url = new URL(win.webContents.getURL());
        if (url.protocol === 'fb:' && url.host === 'app') {
          if (/^\\/c\\/[0-9a-f-]{36}\\/(?:invoices|estimates|sales-receipts|customer-deposits)\\/[0-9a-f-]{36}\\/(?:revisions\\/\\d+\\/)?$/.test(url.pathname)) {
            url.pathname += 'print/';
          } else if (/^\\/c\\/[0-9a-f-]{36}\\/(?:reports\\/(?:profit-and-loss|balance-sheet|general-ledger)|accounts\\/[0-9a-f-]{36}\\/register|reconciliations\\/[0-9a-f-]{36})\\/$/.test(url.pathname)) {
            url.searchParams.set('output','print');
          }
          const target = url.pathname + url.search;
          if (target !== new URL(win.webContents.getURL()).pathname + new URL(win.webContents.getURL()).search) {
            const rendered = JSON.parse(native.render(target));
            if (rendered.status !== 200 || !rendered.html.includes('id="print-now"')) throw new Error('Open a printable document or report first.');
            await win.loadURL(url.href);
          }
        }
      }
      const target = current(win, expected);'''
assert text.count(old)==1;text=text.replace(old,new);p.write_text(text)
p=ROOT/'electron/scripts/smoke-print.cjs';text=p.read_text()
old="    await page.getByRole('link',{name:'Print / Save as PDF',exact:true}).click();"
assert text.count(old)==1;p.write_text(text.replace(old,"    await page.getByRole('link',{name:'Print',exact:true}).click();"))
print('PRINT: native File menu opens the current document/report print view')
