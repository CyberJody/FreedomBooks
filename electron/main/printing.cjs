'use strict';
// Only the main process prints, renders PDF bytes, or asks for an output filename.
const {dialog, Menu} = require('electron');
const saves = require('./save-file.cjs');
const {PAGE_KEY} = require('./remote-mode.cjs');
const printerLookupError = 'Could not find a printer on this computer. Check your printers or choose Save as PDF.';
let busy = false;

function controller(native, output = saves.create({bookPath:()=>JSON.parse(native.appState()).path}), exclusive = async work=>work(), remote = null) {
  const generation = () => remote?.active() ? remote.printMarker() : JSON.parse(native.appState()).session_generation;
  const canPrint = () => remote?.active() ? !!remote.printMarker() : JSON.parse(native.appState()).open;
  async function printable(target, accepted) {
    const rendered = remote?.active() ? await remote.printPage(target, accepted) : JSON.parse(native.render(target));
    requireSession(accepted);
    return rendered?.status === 200 && rendered.html.includes('id="print-now"');
  }
  function targetOf(url, accepted) {
    if (!url.searchParams.has(PAGE_KEY)) return url.pathname + url.search;
    if (!remote?.active() || url.searchParams.get(PAGE_KEY) !== accepted) throw new Error('The print view changed. Open it again and retry.');
    const query = url.search.slice(1).split('&').filter(part => !new URLSearchParams(part).has(PAGE_KEY)).join('&');
    return url.pathname + (query ? '?' + query : '');
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
  async function perform(win, action, expected, accepted = generation()) {
    if (busy) throw new Error('A print action is already open.');
    busy = true;
    try {
      requireSession(accepted);
      if (!expected && win && !win.isDestroyed() && canPrint()) {
        const original = win.webContents.getURL();
        const url = new URL(original);
        if (url.protocol === 'fb:' && url.host === 'app') {
          url.hash='';
          if (/^\/c\/[0-9a-f-]{36}\/(?:invoices|estimates|sales-receipts|customer-deposits)\/[0-9a-f-]{36}\/(?:revisions\/\d+\/)?$/.test(url.pathname)) {
            url.pathname += 'print/';
          } else if (/^\/c\/[0-9a-f-]{36}\/(?:reports\/(?:profit-and-loss|balance-sheet|general-ledger)|accounts\/[0-9a-f-]{36}\/register|reconciliations\/[0-9a-f-]{36})\/$/.test(url.pathname)) {
            url.searchParams.set('output','print');
          }
          const target = targetOf(url, accepted);
          if (target !== targetOf(new URL(original), accepted)) {
            if (!await printable(target, accepted)) throw new Error('Open a printable document or report first.');
            if (win.isDestroyed() || win.webContents.getURL() !== original) throw new Error('The print view changed. Open it again and retry.');
            if (remote?.active()) url.searchParams.set(PAGE_KEY, accepted);
            await win.loadURL(url.href);
            requireSession(accepted);
          }
        }
      }
      const target = await current(win, expected, accepted);
      const recovery = !remote?.active() && target === '/settings/emergency-kit/' ? JSON.parse(native.recoveryExport()) : null;
      const options = {printBackground:true, pageSize:'Letter', landscape:/\/accounts\/[^/]+\/register\//.test(target)};
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
