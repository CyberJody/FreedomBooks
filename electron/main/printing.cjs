'use strict';
// Only the main process prints, renders PDF bytes, or asks for an output filename.
const {dialog, Menu} = require('electron');
const saves = require('./save-file.cjs');
const {PAGE_KEY, pageTarget} = require('./remote-mode.cjs');
const printerLookupError = 'Could not find a printer on this computer. Check your printers or choose Save as PDF.';

function controller(native, output = saves.create({bookPath:()=>JSON.parse(native.appState()).path}), exclusive = async work=>work(), remote = null) {
  let busy = false;
  const generation = () => remote?.active() ? remote.printMarker() : JSON.parse(native.appState()).session_generation;
  const canPrint = () => remote?.active() ? !!remote.printMarker() : JSON.parse(native.appState()).open;
  async function printable(target, accepted) {
    const rendered = remote?.active() ? await remote.printPage(target, accepted) : JSON.parse(native.render(target));
    requireSession(accepted);
    return rendered?.status === 200 && rendered.html.includes('id="print-now"');
  }
  function targetOf(url, accepted) {
    const {target, marker} = pageTarget(url);
    if (marker !== null && (!remote?.active() || !marker || marker !== accepted)) throw new Error('The print view changed. Open it again and retry.');
    return target;
  }
  function requireSession(accepted) {
    if (accepted !== generation()) throw new Error('The active user changed. Open the print view again.');
  }
  async function current(win, expected, accepted) {
    requireSession(accepted);
    if (!win || win.isDestroyed() || !canPrint()) throw new Error('Unlock your books and open a print view first.');
    const url = new URL(win.webContents.getURL());
    if (url.protocol !== 'fb:' || url.host !== 'app' || url.hash) throw new Error('Open a print view first.');
    const target = targetOf(url, accepted);
    if (remote?.active() && !url.searchParams.has(PAGE_KEY)) throw new Error('Open the host print view again.');
    if (expected && target !== expected) throw new Error('The print view changed. Open it again and retry.');
    if (!await printable(target, accepted)) throw new Error('Open a print view first.');
    if (win.isDestroyed() || win.webContents.getURL() !== url.href) throw new Error('The print view changed. Open it again and retry.');
    return target;
  }
  const printQuery = url => url.searchParams.getAll('output').at(-1) === 'print'
    || url.searchParams.getAll('print').at(-1) === '1';
  async function reportPrintUrl(win, page, accepted) {
    if (printQuery(page)) return page;
    const href = await win.webContents.executeJavaScript(
      'document.querySelector("a[data-report-print]")?.getAttribute("href") ?? null',
    );
    requireSession(accepted);
    if (win.isDestroyed() || win.webContents.getURL() !== page.href) {
      throw new Error('The print view changed. Open it again and retry.');
    }
    if (!href) {
      page.search += (page.search ? '&' : '?') + 'output=print';
      return page;
    }
    const url = new URL(href, page);
    if (url.protocol !== page.protocol || url.host !== page.host || url.pathname !== page.pathname
      || url.username || url.password || url.hash || !printQuery(url)) {
      throw new Error('Open a printable document or report first.');
    }
    if (url.searchParams.has(PAGE_KEY)) {
      if (!remote?.active() || pageTarget(url).marker !== accepted) {
        throw new Error('Open a printable document or report first.');
      }
      return new URL(targetOf(url, accepted), page);
    }
    return url;
  }
  async function perform(win, action, expected, accepted = generation()) {
    if (busy) throw new Error('A print action is already open.');
    busy = true;
    try {
      requireSession(accepted);
      if (!expected && win && !win.isDestroyed() && canPrint()) {
        const original = win.webContents.getURL();
        let url = new URL(original);
        if (url.protocol === 'fb:' && url.host === 'app') {
          const originalTarget = targetOf(url, accepted);
          if (/^\/c\/[0-9a-f-]{36}\/(?:invoices|estimates|sales-receipts|customer-deposits)\/[0-9a-f-]{36}\/(?:revisions\/\d+\/)?$/.test(url.pathname)) {
            url.pathname += 'print/';
          } else if (/^\/c\/[0-9a-f-]{36}\/(?:reports\/(?:profit-and-loss|balance-sheet|general-ledger|ar-aging|ap-aging|vendor-1099|customer-statements|cash-flow|comparative-profit-and-loss|sales-by-customer-item|schedule-c|asset-depreciation)|accounts\/[0-9a-f-]{36}\/register|reconciliations\/[0-9a-f-]{36})\/$/.test(url.pathname)) {
            url = await reportPrintUrl(win, url, accepted);
          }
          url.hash = '';
          const target = targetOf(url, accepted);
          if (target !== originalTarget) {
            if (!await printable(target, accepted)) throw new Error('Open a printable document or report first.');
            if (win.isDestroyed() || win.webContents.getURL() !== original) throw new Error('The print view changed. Open it again and retry.');
            if (remote?.active() && !url.searchParams.has(PAGE_KEY)) {
              url.search += `${url.search ? '&' : '?'}${PAGE_KEY}=${accepted}`;
            }
            await win.loadURL(url.href);
            requireSession(accepted);
          }
        }
      }
      const target = await current(win, expected, accepted);
      const recovery = !remote?.active() && target === '/settings/emergency-kit/' ? JSON.parse(native.recoveryExport()) : null;
      const printUrl = new URL(target, 'fb://app');
      const wideComparison = /^\/c\/[^/]+\/reports\/comparative-profit-and-loss\/$/.test(printUrl.pathname)
        && printUrl.searchParams.getAll('period').at(-1) !== 'fiscal_years';
      const assetSchedule = /^\/c\/[^/]+\/reports\/asset-depreciation\/$/.test(printUrl.pathname);
      const landscape = assetSchedule || wideComparison || /\/accounts\/[^/]+\/register\//.test(printUrl.pathname);
      const options = {printBackground:true, pageSize:'Letter', landscape};
      if (action === 'print') {
        if (win.webContents.getPrintersAsync) {
          let printers;
          try { printers = await win.webContents.getPrintersAsync(); }
          catch { throw new Error(printerLookupError); }
          if (!printers.length) throw new Error('No printer is available on this computer. Choose Save as PDF instead.');
          await current(win, target, accepted);
        }
        await new Promise((resolve,reject) => win.webContents.print({...options,silent:false},(ok,reason)=>ok?resolve():reject(new Error(reason === 'Failed to enumerate printers' ? printerLookupError : reason || 'Printing was cancelled.'))));
        requireSession(accepted);
        if (recovery) await exclusive(() => native.recoverySavedAsync(recovery.ticket));
        requireSession(accepted);
        return {printed:true};
      }
      const defaultPath = saves.exportName(win.webContents.getTitle(), 'pdf');
      const selected = await dialog.showSaveDialog(win,{title:'Save as PDF',defaultPath,filters:[{name:'PDF document',extensions:['pdf']}],properties:['showOverwriteConfirmation']});
      requireSession(accepted);
      if (selected.canceled || !selected.filePath) return {saved:false};
      output.validate(selected.filePath);
      await current(win,target,accepted); // A lock/navigation or named-session change expires the action.
      const save = async () => {
        if (recovery) native.validateRecoveryExport(recovery.ticket);
        const pdf = await win.webContents.printToPDF({...options,preferCSSPageSize:true});
        await current(win,target,accepted);
        if (recovery) native.validateRecoveryExport(recovery.ticket);
        output.write(selected.filePath,pdf);
        if (recovery) await native.recoverySavedAsync(recovery.ticket);
      };
      if (recovery) await exclusive(save); else await save();
      requireSession(accepted);
      return {saved:true};
    } finally { busy = false; }
  }
  async function choose(win, target) {
    const accepted = generation();
    await current(win,target,accepted);
    const action = await new Promise(resolve=>{
      const menu = Menu.buildFromTemplate([
        {label:'Print…',click:()=>resolve('print')},
        {label:'Save as PDF…',click:()=>resolve('pdf')},
      ]);
      menu.popup({window:win,callback:()=>resolve(null)});
    });
    requireSession(accepted);
    return action ? perform(win,action,target,accepted) : {saved:false};
  }
  const menuAction = action => async (_item, window) => {
    try { await perform(window,action); }
    catch (error) { dialog.showErrorBox('Printing',String(error.message || error)); }
  };
  return {choose,perform,menuItems:[
    {label:'Print…',accelerator:'CmdOrCtrl+P',click:menuAction('print')},
    {label:'Save as PDF…',accelerator:'CmdOrCtrl+Shift+S',click:menuAction('pdf')},
  ]};
}
module.exports = {controller};
